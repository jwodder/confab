use anyhow::Context;
use clap::Parser;
use futures::stream::iter;
use futures::{SinkExt, StreamExt};
use std::error;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{interval, sleep};
use tokio_stream::wrappers::IntervalStream;
use tokio_util::codec::{Framed, LinesCodec, LinesCodecError};

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
        tokio::spawn(async move { Session::new(socket, addr).run().await });
    }
}

struct Session {
    frame: Framed<TcpStream, LinesCodec>,
    addr: SocketAddr,
}

impl Session {
    fn new(socket: TcpStream, addr: SocketAddr) -> Session {
        Session {
            frame: Framed::new(socket, LinesCodec::new_with_max_length(65535)),
            addr,
        }
    }

    async fn send(&mut self, line: &str) -> Result<(), ServerError> {
        self.frame.send(line).await.map_err(ServerError::SendError)
    }

    async fn recv(&mut self) -> Result<String, ServerError> {
        match self.frame.next().await {
            Some(Ok(line)) => Ok(line),
            Some(Err(e)) => Err(ServerError::RecvError(e)),
            None => Err(ServerError::Disconnect),
        }
    }

    async fn run(mut self) {
        eprintln!("[{}] Connection received", self.addr);
        if let Err(e) = self.interact().await {
            eprintln!("[{}] {}", self.addr, e);
        }
        eprintln!("[{}] Disconnecting ...", self.addr);
    }

    async fn interact(&mut self) -> Result<(), ServerError> {
        self.send("Welcome to the confab Demo Server!").await?;
        loop {
            self.send("Commands: debug, ping, ctrl, quit").await?;
            match self.recv().await?.as_str() {
                "debug" => self.debug().await?,
                "ping" => self.ping().await?,
                "ctrl" => self.ctrl().await?,
                "quit" => {
                    self.send("Goodbye.").await?;
                    return Ok(());
                }
                unk => self.send(&format!("Unknown command {unk:?}")).await?,
            }
        }
    }

    async fn debug(&mut self) -> Result<(), ServerError> {
        self.send("Enter lines to send back.  Send \"quit\" to return to the main menu.")
            .await?;
        loop {
            let line = self.recv().await?;
            if line == "quit" {
                return Ok(());
            }
            self.send(&format!("You sent: {line:?}")).await?;
        }
    }

    async fn ping(&mut self) -> Result<(), ServerError> {
        let mut i: usize = 1;
        self.send("I'm going to ping you now until you send something.")
            .await?;
        loop {
            tokio::select! {
                _ = sleep(Duration::from_secs(1)) => {
                    self.send(&format!("Ping {i}")).await?;
                    i += 1;
                },
                r = self.recv() => {
                    r?;
                    self.send("Ok, stopping.").await?;
                    return Ok(());
                }
            }
        }
    }

    async fn ctrl(&mut self) -> Result<(), ServerError> {
        self.blather([
            "Here are some special characters:",
            "NUL: <\x00>",
            "TAB: <\t>",
            "VTAB: <\x0B>",
            "CR: <\x0D>",
            "Private use: <\u{E011}>",
            "Reserved: <\u{FFFF}>",
        ])
        .await
    }

    async fn blather<I: IntoIterator<Item = &'static str>>(
        &mut self,
        lines: I,
    ) -> Result<(), ServerError> {
        let mut stream = IntervalStream::new(interval(Duration::from_secs(1))).zip(iter(lines));
        loop {
            tokio::select! {
                r = stream.next() => match r {
                    Some((_, ln)) => self.send(ln).await?,
                    None => return Ok(()),
                },
                _ = self.recv() => self.send("Not now, I'm sending stuff.").await?,
            }
        }
    }
}

#[derive(Debug)]
enum ServerError {
    RecvError(LinesCodecError),
    SendError(LinesCodecError),
    Disconnect,
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ServerError::RecvError(e) => write!(f, "Error reading: {e}"),
            ServerError::SendError(e) => write!(f, "Error writing: {e}"),
            ServerError::Disconnect => write!(f, "Client disconnected"),
        }
    }
}

impl error::Error for ServerError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            ServerError::RecvError(e) => Some(e),
            ServerError::SendError(e) => Some(e),
            ServerError::Disconnect => None,
        }
    }
}
