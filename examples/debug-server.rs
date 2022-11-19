use anyhow::Context;
use clap::Parser;
use futures::{SinkExt, StreamExt};
use std::net::{IpAddr, SocketAddr};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::{Framed, LinesCodec};

#[derive(Parser)]
struct Arguments {
    #[clap(short, long, default_value = "127.0.0.1")]
    bind: IpAddr,

    #[clap(default_value_t = 0)]
    port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Arguments::parse();
    let listener = TcpListener::bind((args.bind, args.port))
        .await
        .context("Error binding to port")?;
    println!(
        "Listening for connections at {} ...",
        listener
            .local_addr()
            .context("Error getting local address")?
    );
    loop {
        let (socket, addr) = listener
            .accept()
            .await
            .context("Error listening for connections")?;
        tokio::spawn(async move { process(socket, addr).await });
    }
}

async fn process(socket: TcpStream, addr: SocketAddr) {
    eprintln!("[{addr}] Connection received");
    let mut frame = Framed::new(socket, LinesCodec::new_with_max_length(255));
    while let Some(r) = frame.next().await {
        match r {
            Ok(line) => {
                if let Err(e) = frame.send(format!("You sent: {line:?}")).await {
                    eprintln!("[{addr}] Error sending message: {e}");
                    break;
                }
                if line == "quit" {
                    eprintln!("[{addr}] Client quit");
                    break;
                }
            }
            Err(e) => {
                eprintln!("[{addr}] Error reading message: {e:?}");
                break;
            }
        }
    }
    eprintln!("[{addr}] Disconnecting ...");
}
