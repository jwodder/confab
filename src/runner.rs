use crate::codec::ConfabCodec;
use crate::errors::{InetError, InterfaceError, IoError};
use crate::events::Event;
use crate::input::{Input, StartupScript, readline_stream};
use crate::tls;
use crate::util::{CharEncoding, now_hms};
use futures_util::{SinkExt, Stream, StreamExt};
use rustyline_async::{Readline, SharedWriter};
use std::fs::File;
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::process::ExitCode;
use tokio::net::TcpStream;
use tokio_util::{codec::Framed, either::Either};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectState {
    Open,
    Closed,
}

pub(crate) struct Runner {
    pub(crate) startup_script: Option<StartupScript>,
    pub(crate) reporter: Reporter,
    pub(crate) connector: Connector,
}

impl Runner {
    pub(crate) async fn run(mut self) -> Result<ExitCode, InterfaceError> {
        match self.try_run().await {
            Ok(()) => Ok(ExitCode::SUCCESS),
            Err(IoError::Interface(e)) => Err(e),
            Err(IoError::Inet(e)) => {
                self.reporter.report(Event::error(anyhow::Error::new(e)))?;
                Ok(ExitCode::FAILURE)
            }
        }
    }

    async fn try_run(&mut self) -> Result<(), IoError> {
        let mut frame = self.connector.connect(&mut self.reporter).await?;
        if let Some(script) = self.startup_script.take() {
            let r = ioloop(&mut frame, script, &mut self.reporter).await;
            if let Err(e) = r {
                // Don't bother to report closing errors if ioloop errored (but
                // still close anyway)
                let _ = frame.close().await;
                return Err(e);
            } else if r.is_ok_and(|cs| cs == ConnectState::Closed) {
                frame.close().await?;
                self.reporter.report(Event::disconnect())?;
                return Ok(());
            }
        }
        let (mut rl, shared) = init_readline()?;
        // Lines written to the SharedWriter are only output when
        // Readline::readline() or Readline::flush() is called, so anything
        // written before we start getting input from the user should be
        // written directly to stdout instead.
        self.reporter.set_writer(Box::new(shared));
        let mut r = ioloop(&mut frame, readline_stream(&mut rl), &mut self.reporter)
            .await
            .map(|_| ());
        // Don't bother to report closing errors if ioloop errored (but still
        // close anyway)
        let r2 = frame.close().await.map_err(IoError::from);
        if r.is_ok() {
            r = r2;
        }
        if r.is_ok() {
            r = self
                .reporter
                .report(Event::disconnect())
                .map_err(IoError::from);
        }
        let _ = rl.flush();
        // Set the writer back to stdout so that errors reported by run() will
        // show up without having to call rl.flush().
        self.reporter.set_writer(Box::new(io::stdout()));
        r
    }
}

pub(crate) struct Reporter {
    pub(crate) writer: Box<dyn Write + Send>,
    pub(crate) transcript: Option<File>,
    pub(crate) show_times: bool,
}

impl Reporter {
    fn set_writer(&mut self, writer: Box<dyn Write + Send>) {
        self.writer = writer;
    }

    fn report(&mut self, event: Event) -> Result<(), InterfaceError> {
        self.report_inner(event).map_err(InterfaceError::Write)
    }

    fn report_inner(&mut self, event: Event) -> Result<(), io::Error> {
        writeln!(self.writer, "{}", event.to_message(self.show_times))?;
        if let Some(fp) = self.transcript.as_mut() {
            if let Err(e) = writeln!(fp, "{}", event.to_json()) {
                let _ = self.transcript.take();
                if self.show_times {
                    write!(self.writer, "[{}] ", now_hms())?;
                }
                writeln!(self.writer, "! Error writing to transcript: {e}")?;
            }
        }
        Ok(())
    }

    fn echo_ctrlc(&mut self) -> Result<(), InterfaceError> {
        writeln!(self.writer, "^C").map_err(InterfaceError::Write)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Connector {
    pub(crate) tls: bool,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) servername: Option<String>,
    pub(crate) encoding: CharEncoding,
    pub(crate) max_line_length: NonZeroUsize,
    pub(crate) crlf: bool,
}

impl Connector {
    async fn connect(&self, reporter: &mut Reporter) -> Result<Connection, IoError> {
        reporter.report(Event::connect_start(&self.host, self.port))?;
        let conn = TcpStream::connect((&*self.host, self.port))
            .await
            .map_err(InetError::Connect)?;
        reporter.report(Event::connect_finish(
            conn.peer_addr().map_err(InetError::PeerAddr)?,
        ))?;
        let conn = if self.tls {
            reporter.report(Event::tls_start())?;
            let conn = tls::connect(conn, self.servername.as_ref().unwrap_or(&self.host))
                .await
                .map_err(InetError::Tls)?;
            reporter.report(Event::tls_finish())?;
            Either::Right(conn)
        } else {
            Either::Left(conn)
        };
        Ok(Connection(Framed::new(conn, self.codec())))
    }

    fn codec(&self) -> ConfabCodec {
        ConfabCodec::new_with_max_length(self.max_line_length.get())
            .encoding(self.encoding)
            .crlf(self.crlf)
    }
}

#[derive(Debug)]
struct Connection(Framed<Either<TcpStream, tls::TlsStream>, ConfabCodec>);

impl Connection {
    async fn recv(&mut self) -> Option<Result<String, InetError>> {
        self.0.next().await.map(|r| r.map_err(InetError::Recv))
    }

    async fn send(&mut self, line: String) -> Result<String, InetError> {
        let line = self.0.codec().prepare_line(line);
        self.0.send(&line).await.map_err(InetError::Send)?;
        Ok(line)
    }

    async fn close(&mut self) -> Result<(), InetError> {
        SinkExt::<&str>::close(&mut self.0)
            .await
            .map_err(InetError::Close)
    }
}

async fn ioloop<S>(
    frame: &mut Connection,
    input: S,
    reporter: &mut Reporter,
) -> Result<ConnectState, IoError>
where
    S: Stream<Item = Result<Input, InterfaceError>> + Send,
{
    tokio::pin!(input);
    loop {
        tokio::select! {
            r = frame.recv() => match r {
                Some(Ok(msg)) => reporter.report(Event::recv(msg))?,
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(ConnectState::Closed),
            },
            r = input.next() => match r {
                Some(Ok(Input::Line(line))) => {
                    let line = frame.send(line).await?;
                    reporter.report(Event::send(line))?;
                }
                Some(Ok(Input::CtrlC)) => reporter.echo_ctrlc()?,
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(ConnectState::Open),
            }
        }
    }
}

fn init_readline() -> Result<(Readline, SharedWriter), InterfaceError> {
    let (mut rl, shared) = Readline::new(String::from("confab> ")).map_err(InterfaceError::Init)?;
    rl.should_print_line_on(false, false);
    Ok((rl, shared))
}
