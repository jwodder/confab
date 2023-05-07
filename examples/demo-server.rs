use anyhow::Context;
use clap::Parser;
use futures::stream::iter;
use futures::{SinkExt, StreamExt};
use std::error;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use time::format_description::FormatItem;
use time::macros::format_description;
use time::OffsetDateTime;
use tokio::io::AsyncWriteExt;
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
    eprintln!(
        "Listening for connections at {} ...",
        listener
            .local_addr()
            .context("Error getting local address")?
    );
    eprintln!("Press Ctrl-C to terminate.");
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

    async fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), ServerError> {
        self.frame
            .get_mut()
            .write_all(bytes)
            .await
            .map_err(|e| ServerError::SendError(LinesCodecError::from(e)))
    }

    async fn recv(&mut self) -> Result<String, ServerError> {
        match self.frame.next().await {
            Some(Ok(line)) => Ok(line),
            Some(Err(e)) => Err(ServerError::RecvError(e)),
            None => Err(ServerError::Disconnect),
        }
    }

    async fn run(mut self) {
        self.log("Connection received");
        if let Err(e) = self.interact().await {
            self.log(e);
        }
        self.log("Disconnecting ...");
    }

    fn log<D: fmt::Display>(&self, event: D) {
        eprintln!("[{}] [{}] {}", hms_now(), self.addr, event);
    }

    async fn interact(&mut self) -> Result<(), ServerError> {
        self.send(&format!(
            "Welcome to the confab Demo Server, {}!",
            self.addr
        ))
        .await?;
        loop {
            self.send("Commands: debug, ping, async, ctrl, bytes, quit")
                .await?;
            match self.recv().await?.as_str() {
                "debug" => self.debug().await?,
                "ping" => self.ping().await?,
                "async" => self.async_().await?,
                "ctrl" => self.ctrl().await?,
                "bytes" => self.bytes().await?,
                "quit" => {
                    self.send("Goodbye.").await?;
                    return Ok(());
                }
                unk => self.send(&format!("Unknown command {unk:?}")).await?,
            }
        }
    }

    async fn debug(&mut self) -> Result<(), ServerError> {
        self.send("Enter lines to send back.").await?;
        self.send("Send \"quit\" to return to the main menu.")
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

    async fn async_(&mut self) -> Result<(), ServerError> {
        self.send("Enter lines to send back while I blather.")
            .await?;
        self.send("Send \"quit\" to return to the main menu.")
            .await?;
        loop {
            tokio::select! {
                _ = sleep(Duration::from_secs(1)) => {
                    self.send("Blah blah blah.").await?;
                }
                r = self.recv() => {
                    let line = r?;
                    if line == "quit" {
                        return Ok(());
                    }
                    self.send(&format!("You sent: {line:?}")).await?;
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

    async fn bytes(&mut self) -> Result<(), ServerError> {
        let blines = [
            &b"Here is some non-UTF-8 data:\n"[..],
            &b"Latin-1: Libert\xE9, \xE9galit\xE9, fraternit\xE9\n"[..],
            &b"General garbage: \x89\xAB\xCD\xEF\n"[..],
        ];
        let mut stream = IntervalStream::new(interval(Duration::from_secs(1))).zip(iter(blines));
        loop {
            tokio::select! {
                r = stream.next() => match r {
                    Some((_, ln)) => self.send_bytes(ln).await?,
                    None => return Ok(()),
                },
                _ = self.recv() => self.send_bytes("Not now, I'm sending stuff.\n".as_bytes()).await?,
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

fn hms_now() -> String {
    static HMS_FMT: &[FormatItem<'_>] = format_description!("[hour]:[minute]:[second]");
    OffsetDateTime::now_local()
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .format(&HMS_FMT)
        .unwrap()
}
