mod codec;
mod events;
mod util;
use crate::codec::ConfabCodec;
use crate::events::Event;
use crate::util::CharEncoding;
use anyhow::Context as _;
use chrono::Local;
use clap::Parser;
use futures::{SinkExt, StreamExt};
use pin_project::pin_project;
use rustyline_async::{Readline, ReadlineError, SharedWriter};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::ExitCode;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

/// Asynchronous line-oriented interactive TCP client
///
/// See <https://github.com/jwodder/confab> for more information
#[derive(Parser)]
#[clap(version)]
struct Arguments {
    /// Terminate sent lines with CR LF instead of just LF
    #[clap(long)]
    crlf: bool,

    /// Set text encoding
    #[clap(
        short = 'E',
        long,
        default_value = "utf8",
        value_name = "utf8|utf8-latin1|latin1"
    )]
    encoding: CharEncoding,

    /// Set maximum length of lines read from remote server
    #[clap(short = 'M', long, default_value = "65535", value_name = "INT")]
    max_line_length: NonZeroUsize,

    /// Use the given domain name for SNI and certificate hostname validation
    /// [default: the remote host name]
    #[clap(long, value_name = "DOMAIN")]
    servername: Option<String>,

    /// Prepend timestamps to output messages
    #[clap(short = 't', long)]
    show_times: bool,

    /// Connect using SSL/TLS
    #[clap(long)]
    tls: bool,

    /// Append a transcript of events to the given file
    #[clap(short = 'T', long, value_name = "FILE")]
    transcript: Option<PathBuf>,

    /// Remote host (domain name or IP address) to which to connect
    host: String,

    /// Remote port (integer) to which to connect
    port: u16,
}

impl Arguments {
    fn open(self) -> anyhow::Result<Runner> {
        let (rl, stdout) =
            Readline::new("confab> ".into()).context("Error constructing Readline object")?;
        let transcript = match self.transcript {
            Some(path) => Some(
                OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(path)
                    .context("Error opening transcript file")?,
            ),
            None => None,
        };
        Ok(Runner {
            rl,
            stdout,
            transcript,
            crlf: self.crlf,
            encoding: self.encoding,
            max_line_length: self.max_line_length,
            tls: self.tls,
            host: self.host,
            port: self.port,
            show_times: self.show_times,
            servername: self.servername,
        })
    }
}

struct Runner {
    rl: Readline,
    stdout: SharedWriter,
    transcript: Option<File>,
    crlf: bool,
    encoding: CharEncoding,
    max_line_length: NonZeroUsize,
    tls: bool,
    host: String,
    port: u16,
    servername: Option<String>,
    show_times: bool,
}

impl Runner {
    fn report(&mut self, event: Event) -> Result<(), InterfaceError> {
        if self.show_times {
            write!(self.stdout, "[{}] ", event.display_time()).map_err(InterfaceError::Write)?;
        }
        write!(self.stdout, "{} ", event.sigil()).map_err(InterfaceError::Write)?;
        for chunk in event.message() {
            write!(self.stdout, "{chunk}").map_err(InterfaceError::Write)?;
        }
        writeln!(self.stdout).map_err(InterfaceError::Write)?;
        if let Some(fp) = self.transcript.as_mut() {
            if let Err(e) = writeln!(fp, "{}", event.to_json()) {
                let _ = self.transcript.take();
                if self.show_times {
                    write!(self.stdout, "[{}] ", Local::now().format("%H:%M:%S"))
                        .map_err(InterfaceError::Write)?;
                }
                writeln!(self.stdout, "! Error writing to transcript: {e}")
                    .map_err(InterfaceError::Write)?;
            }
        }
        Ok(())
    }

    fn codec(&self) -> ConfabCodec {
        ConfabCodec::new_with_max_length(self.max_line_length.get()).encoding(self.encoding)
    }

    async fn run(&mut self) -> Result<ExitCode, InterfaceError> {
        match self.try_run().await {
            Ok(()) => Ok(ExitCode::SUCCESS),
            Err(e) => match e.downcast::<InterfaceError>() {
                Ok(e) => Err(e),
                Err(e) => {
                    self.report(Event::error(e))?;
                    Ok(ExitCode::FAILURE)
                }
            },
        }
    }

    async fn try_run(&mut self) -> anyhow::Result<()> {
        self.report(Event::connect_start(&self.host, self.port))?;
        let conn = TcpStream::connect((self.host.clone(), self.port))
            .await
            .context("Error connecting to server")?;
        self.report(Event::connect_finish(
            conn.peer_addr().context("Error getting peer address")?,
        ))?;
        let conn = if self.tls {
            self.report(Event::tls_start())?;
            let cx = tokio_native_tls::TlsConnector::from(
                native_tls::TlsConnector::new().context("Error creating TLS connector")?,
            );
            let conn = cx
                .connect(self.servername.as_ref().unwrap_or(&self.host), conn)
                .await
                .context("Error establishing TLS connection")?;
            self.report(Event::tls_finish())?;
            Connection::Tls(conn)
        } else {
            Connection::Plain(conn)
        };
        tokio::pin!(conn);
        let mut frame = Framed::new(conn, self.codec());
        loop {
            let event = tokio::select! {
                r = frame.next() => match r {
                    Some(Ok(msg)) => Event::recv(msg),
                    Some(Err(e)) => return Err(e).context("Error reading from connection"),
                    None => break,
                },
                input = self.rl.readline() => match input {
                    Ok(mut line) => {
                        self.rl.add_history_entry(line.clone());
                        if self.crlf {
                            line.push_str("\r\n");
                        } else {
                            line.push('\n');
                        }
                        frame.send(&line).await.context("Error sending message")?;
                        Event::send(line)
                    }
                    Err(ReadlineError::Eof) | Err(ReadlineError::Closed) => break,
                    Err(ReadlineError::Interrupted) => {writeln!(self.stdout, "^C")?; continue; }
                    Err(ReadlineError::IO(e)) => return Err(anyhow::Error::new(InterfaceError::Read(e))),
                }
            };
            self.report(event)?;
        }
        self.report(Event::disconnect())?;
        Ok(())
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        let _ = self.rl.flush();
    }
}

enum InterfaceError {
    Read(io::Error),
    Write(io::Error),
}

impl fmt::Debug for InterfaceError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for InterfaceError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            InterfaceError::Read(e) => write!(f, "Error reading user input: {e}"),
            InterfaceError::Write(e) => write!(f, "Error writing output: {e}"),
        }
    }
}

impl std::error::Error for InterfaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            InterfaceError::Read(e) => Some(e),
            InterfaceError::Write(e) => Some(e),
        }
    }
}

#[pin_project(project=ConnectionProj)]
enum Connection {
    Plain(#[pin] TcpStream),
    Tls(#[pin] tokio_native_tls::TlsStream<TcpStream>),
}

impl AsyncRead for Connection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.project() {
            ConnectionProj::Plain(conn) => conn.poll_read(cx, buf),
            ConnectionProj::Tls(conn) => conn.poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Connection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.project() {
            ConnectionProj::Plain(conn) => conn.poll_write(cx, buf),
            ConnectionProj::Tls(conn) => conn.poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.project() {
            ConnectionProj::Plain(conn) => conn.poll_flush(cx),
            ConnectionProj::Tls(conn) => conn.poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.project() {
            ConnectionProj::Plain(conn) => conn.poll_shutdown(cx),
            ConnectionProj::Tls(conn) => conn.poll_shutdown(cx),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.project() {
            ConnectionProj::Plain(conn) => conn.poll_write_vectored(cx, bufs),
            ConnectionProj::Tls(conn) => conn.poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Connection::Plain(conn) => conn.is_write_vectored(),
            Connection::Tls(conn) => conn.is_write_vectored(),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    Ok(Arguments::parse().open()?.run().await?)
}
