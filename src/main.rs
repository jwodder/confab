mod events;
mod json;
use crate::events::Event;
use anyhow::Context;
use clap::Parser;
use futures::{SinkExt, StreamExt};
use rustyline_async::{Readline, ReadlineError, SharedWriter};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::pin::Pin;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LinesCodec};

#[derive(Parser)]
struct Arguments {
    #[clap(short = 'T', long)]
    transcript: Option<PathBuf>,
    #[clap(long)]
    tls: bool,
    host: String,
    port: u16,
}

struct EventReporter {
    stdout: SharedWriter,
    transcript: Option<File>,
}

impl EventReporter {
    fn report(&mut self, event: Event) -> std::io::Result<()> {
        // TODO: Replace these with async calls
        writeln!(self.stdout, "{} {}", event.sigil(), event.message())?;
        if let Some(fp) = self.transcript.as_mut() {
            // TODO: Warn if this errors, but keep running anyway
            writeln!(fp, "{}", event.to_json())?;
        }
        Ok(())
    }
}

trait AsyncReadWrite: AsyncRead + AsyncWrite {}

impl<T> AsyncReadWrite for T where T: AsyncRead + AsyncWrite {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Arguments::parse();
    let (mut rl, stdout) =
        Readline::new("netlion> ".into()).context("Error constructing Readline object")?;
    let transcript = match args.transcript {
        Some(path) => Some(OpenOptions::new().append(true).create(true).open(path)?),
        None => None,
    };
    let reporter = EventReporter { stdout, transcript };
    let r = run(&mut rl, reporter, args.host, args.port, args.tls).await;
    rl.flush()?;
    r
}

async fn run(
    rl: &mut Readline,
    mut reporter: EventReporter,
    host: String,
    port: u16,
    tls: bool,
) -> anyhow::Result<()> {
    reporter.report(Event::connect_start(&host, port))?;
    let conn = TcpStream::connect((host.clone(), port))
        .await
        .context("Error connecting to server")?;
    reporter.report(Event::connect_finish(
        conn.peer_addr().context("Error getting peer address")?,
    ))?;
    let conn: Pin<Box<dyn AsyncReadWrite>> = if tls {
        reporter.report(Event::tls_start())?;
        let cx = tokio_native_tls::TlsConnector::from(
            native_tls::TlsConnector::new().context("Error creating TLS connector")?,
        );
        let conn = cx
            .connect(&host, conn)
            .await
            .context("Error establishing TLS connection")?;
        reporter.report(Event::tls_finish())?;
        Box::pin(conn)
    } else {
        Box::pin(conn)
    };
    let mut frame = Framed::new(conn, LinesCodec::new_with_max_length(65535));
    loop {
        let event = tokio::select! {
            r = frame.next() => match r {
                Some(Ok(msg)) => Event::recv(msg),
                Some(Err(e)) => return Err(e).context("Error reading from connection"),
                None => break,
            },
            input = rl.readline() => match input {
                Ok(line) => {
                    frame.send(&line).await.context("Error sending message")?;
                    rl.add_history_entry(line.clone());
                    Event::send(line)
                }
                Err(ReadlineError::Eof) | Err(ReadlineError::Interrupted) | Err(ReadlineError::Closed) => break,
                Err(e) => return Err(e).context("Readline error"),
            }
        };
        reporter.report(event)?;
    }
    reporter.report(Event::disconnect())?;
    Ok(())
}
