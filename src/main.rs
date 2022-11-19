use anyhow::Context;
use clap::Parser;
use futures::{SinkExt, StreamExt};
use rustyline_async::{Readline, ReadlineError};
use std::io::Write;
use tokio_util::codec::{Framed, LinesCodec};
use tokio::net::TcpStream;

#[derive(Parser)]
struct Arguments {
    host: String,
    port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Arguments::parse();
    println!("Connecting ...");
    let conn = TcpStream::connect((args.host, args.port))
        .await
        .context("Error connecting to server")?;
    println!(
        "Connected to {}",
        conn.peer_addr().context("Error getting peer address")?
    );
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
                    writeln!(stdout, "Remote disconnected")?;
                    break;
                }
            },
            cmd = rl.readline() => match cmd {
                Ok(line) => {
                    frame.send(&line).await.context("Error sending message")?;
                    writeln!(stdout, "> {line}")?;
                    rl.add_history_entry(line);
                }
                Err(ReadlineError::Eof) => {
                    //writeln!(stdout, "<EOF>")?;
                    break;
                }
                Err(ReadlineError::Interrupted) => {
                    //writeln!(stdout, "<Ctrl-C>")?;
                    break;
                }
                Err(e) => {
                    writeln!(stdout, "Readline error: {e}")?;
                    break;
                }
            }
        }
    }
    Ok(())
}
