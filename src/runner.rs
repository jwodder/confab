use crate::codec::ConfabCodec;
use crate::errors::{InetError, InterfaceError, IoError};
use crate::events::Event;
use crate::input::{readline_stream, Input, StartupScript};
use crate::tls;
use crate::util::{now_hms, CharEncoding};
use futures::{SinkExt, Stream, StreamExt};
use rustyline_async::{Readline, SharedWriter};
use std::fs::File;
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::process::ExitCode;
use tokio::net::TcpStream;
use tokio_util::{codec::Framed, either::Either};

type Connection = Framed<Either<TcpStream, tls::TlsStream>, ConfabCodec>;

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
            ioloop(&mut frame, script, &mut self.reporter).await?;
        }
        let (mut rl, shared) = init_readline()?;
        // Lines written to the SharedWriter are only output when
        // Readline::readline() or Readline::flush() is called, so anything
        // written before we start getting input from the user should be
        // written directly to stdout instead.
        self.reporter.set_writer(Box::new(shared));
        let r = ioloop(
            &mut frame,
            readline_stream(&mut rl).await,
            &mut self.reporter,
        )
        .await;
        // TODO: Should this event not be emitted if an error occurs above?
        let r2 = self.reporter.report(Event::disconnect());
        // Flush after the disconnect event so that we're guaranteed a line in
        // the Readline buffer, leading to the prompt being cleared.
        let _ = rl.flush();
        drop(rl);
        // Set the writer back to stdout so that errors reported by run() will
        // show up without having to call rl.flush().
        self.reporter.set_writer(Box::new(io::stdout()));
        r.map_err(Into::into).and(r2.map_err(Into::into))
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
        Ok(Framed::new(conn, self.codec()))
    }

    fn codec(&self) -> ConfabCodec {
        ConfabCodec::new_with_max_length(self.max_line_length.get())
            .encoding(self.encoding)
            .crlf(self.crlf)
    }
}

async fn ioloop<S>(frame: &mut Connection, input: S, reporter: &mut Reporter) -> Result<(), IoError>
where
    S: Stream<Item = Result<Input, InterfaceError>> + Send,
{
    tokio::pin!(input);
    loop {
        tokio::select! {
            r = frame.next() => match r {
                Some(Ok(msg)) => reporter.report(Event::recv(msg))?,
                Some(Err(e)) => return Err(IoError::Inet(InetError::Recv(e))),
                None => break,
            },
            r = input.next() => match r {
                Some(Ok(Input::Line(line))) => {
                    let line = frame.codec().prepare_line(line);
                    frame.send(&line).await.map_err(InetError::Send)?;
                    reporter.report(Event::send(line))?;
                }
                Some(Ok(Input::CtrlC)) => reporter.echo_ctrlc()?,
                Some(Err(e)) => return Err(e.into()),
                None => break,
            }
        }
    }
    Ok(())
}

fn init_readline() -> Result<(Readline, SharedWriter), InterfaceError> {
    let (mut rl, shared) = Readline::new(String::from("confab> ")).map_err(InterfaceError::Init)?;
    rl.should_print_line_on(false, false);
    Ok((rl, shared))
}
