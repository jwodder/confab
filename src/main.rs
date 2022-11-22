mod codec;
mod events;
mod util;
use crate::codec::ConfabCodec;
use crate::events::Event;
use crate::util::CharEncoding;
use anyhow::Context;
use clap::Parser;
use futures::{SinkExt, StreamExt};
use rustyline_async::{Readline, ReadlineError, SharedWriter};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::pin::Pin;
use tokio::io::{AsyncRead, AsyncWrite};
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
    show_times: bool,
}

impl Runner {
    fn report(&mut self, event: Event) -> std::io::Result<()> {
        // TODO: Replace these with async calls
        if self.show_times {
            write!(self.stdout, "[{}] ", event.display_time())?;
        }
        write!(self.stdout, "{} ", event.sigil())?;
        for chunk in event.message() {
            write!(self.stdout, "{}", chunk)?;
        }
        writeln!(self.stdout)?;
        if let Some(fp) = self.transcript.as_mut() {
            // TODO: Warn if this errors, but keep running anyway
            writeln!(fp, "{}", event.to_json())?;
        }
        Ok(())
    }

    fn codec(&self) -> ConfabCodec {
        ConfabCodec::new_with_max_length(self.max_line_length.get()).encoding(self.encoding)
    }

    async fn run(&mut self) -> anyhow::Result<()> {
        self.report(Event::connect_start(&self.host, self.port))?;
        let conn = TcpStream::connect((self.host.clone(), self.port))
            .await
            .context("Error connecting to server")?;
        self.report(Event::connect_finish(
            conn.peer_addr().context("Error getting peer address")?,
        ))?;
        let conn: Pin<Box<dyn AsyncReadWrite>> = if self.tls {
            self.report(Event::tls_start())?;
            let cx = tokio_native_tls::TlsConnector::from(
                native_tls::TlsConnector::new().context("Error creating TLS connector")?,
            );
            let conn = cx
                .connect(&self.host, conn)
                .await
                .context("Error establishing TLS connection")?;
            self.report(Event::tls_finish())?;
            Box::pin(conn)
        } else {
            Box::pin(conn)
        };
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
                    Err(e) => return Err(e).context("Readline error"),
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

trait AsyncReadWrite: AsyncRead + AsyncWrite {}

impl<T> AsyncReadWrite for T where T: AsyncRead + AsyncWrite {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Arguments::parse().open()?.run().await
}
