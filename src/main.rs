use anyhow::Context;
use clap::Parser;
use futures::{SinkExt, StreamExt};
use rustyline_async::{Readline, ReadlineError};
use std::io::Write;
use std::pin::Pin;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LinesCodec};

#[derive(Parser)]
struct Arguments {
    #[clap(long)]
    tls: bool,
    host: String,
    port: u16,
}

trait AsyncReadWrite: AsyncRead + AsyncWrite {}
impl<T> AsyncReadWrite for T where T: AsyncRead + AsyncWrite {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Arguments::parse();
    println!("* Connecting ...");
    let conn = TcpStream::connect((args.host.clone(), args.port))
        .await
        .context("Error connecting to server")?;
    println!(
        "* Connected to {}",
        conn.peer_addr().context("Error getting peer address")?
    );
    let conn: Pin<Box<dyn AsyncReadWrite>> = if args.tls {
        println!("* Initializing TLS ...");
        let cx = tokio_native_tls::TlsConnector::from(
            native_tls::TlsConnector::new().context("Error creating TLS connector")?,
        );
        let conn = cx
            .connect(&args.host, conn)
            .await
            .context("Error establishing TLS connection")?;
        println!("* TLS established");
        Box::pin(conn)
    } else {
        Box::pin(conn)
    };
    let mut frame = Framed::new(conn, LinesCodec::new_with_max_length(65535));
    let (mut rl, mut stdout) =
        Readline::new("netlion> ".into()).context("Error constructing Readline object")?;
    loop {
        tokio::select! {
            r = frame.next() => match r {
                Some(Ok(msg)) => writeln!(stdout, "< {msg}")?,
                Some(Err(e)) => {
                    writeln!(stdout, "! {e}")?;
                    break;
                }
                None => {
                    writeln!(stdout, "* Remote disconnected")?;
                    break;
                }
            },
            cmd = rl.readline() => match cmd {
                Ok(line) => {
                    frame.send(&line).await.context("Error sending message")?;
                    writeln!(stdout, "> {line}")?;
                    rl.add_history_entry(line);
                }
                Err(ReadlineError::Eof) | Err(ReadlineError::Interrupted) => break,
                Err(e) => {
                    writeln!(stdout, "! Readline error: {e}")?;
                    break;
                }
            }
        }
    }
    writeln!(stdout, "* Disconnected")?;
    rl.flush()?;
    Ok(())
}
