mod codec;
mod events;
mod util;
use crate::codec::ConfabCodec;
use crate::events::{now, Event, HMS_FMT};
use crate::util::{latin1ify, CharEncoding};
use anyhow::Context as _;
use clap::Parser;
use futures::{SinkExt, StreamExt};
use rustyline_async::{Readline, ReadlineError, SharedWriter};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;
use tokio_util::either::Either;

mod build {
    include!(concat!(env!("OUT_DIR"), "/build_info.rs"));
}

/// Asynchronous line-oriented interactive TCP client
///
/// See <https://github.com/jwodder/confab> for more information
#[derive(Clone, Debug, Eq, Parser, PartialEq)]
#[clap(version)]
struct Arguments {
    /// Show build information
    #[clap(long, exclusive = true)]
    build_info: bool,

    /// Terminate sent lines with CR LF instead of just LF
    #[clap(long)]
    crlf: bool,

    /// Set text encoding
    ///
    /// "utf8" converts invalid byte sequences to the replacement character.
    /// "utf8-latin1" handles invalid byte sequences by decoding the entire
    /// line as Latin-1.
    #[clap(
        short = 'E',
        long,
        default_value = "utf8",
        value_name = "utf8|utf8-latin1|latin1"
    )]
    encoding: CharEncoding,

    /// Set maximum length in bytes of lines read from remote server
    ///
    /// If the server sends a line longer than this (including the terminating
    /// newline), the first <LIMIT> bytes will be split off and treated as a
    /// whole line, with the remaining bytes treated as the start of a new
    /// line.
    #[clap(long, default_value = "65535", value_name = "LIMIT")]
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
    #[clap(default_value = "localhost", required = true)]
    // The dummy default value is just there so that `--build-info` can be made
    // exclusive.
    host: String,

    /// Remote port (integer) to which to connect
    #[clap(default_value_t = 80, required = true)]
    // The dummy default value is just there so that `--build-info` can be made
    // exclusive.
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
        self.report_inner(event).map_err(InterfaceError::Write)
    }

    fn report_inner(&mut self, event: Event) -> Result<(), io::Error> {
        writeln!(self.stdout, "{}", event.to_message(self.show_times))?;
        if let Some(fp) = self.transcript.as_mut() {
            if let Err(e) = writeln!(fp, "{}", event.to_json()) {
                let _ = self.transcript.take();
                if self.show_times {
                    write!(self.stdout, "[{}] ", now().format(&HMS_FMT).unwrap())?;
                }
                writeln!(self.stdout, "! Error writing to transcript: {e}")?;
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
            Either::Right(conn)
        } else {
            Either::Left(conn)
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
                        if self.encoding == CharEncoding::Latin1 {
                            // We need to convert non-Latin-1 characters to '?'
                            // here rather than waiting for the codec to do it
                            // so that the Event will reflect the actual
                            // characters sent.
                            line = latin1ify(line);
                        }
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

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let args = Arguments::parse();
    if args.build_info {
        build_info();
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(args.open()?.run().await?)
    }
}

fn build_info() {
    use build::*;
    println!(
        "This is {} version {}.",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    );
    println!();
    println!("Built: {BUILD_TIMESTAMP}");
    println!("Target triple: {TARGET_TRIPLE}");
    println!("Compiler: {RUSTC_VERSION}");
    println!("Compiler host triple: {HOST_TRIPLE}");
    if let Some(hash) = GIT_COMMIT_HASH {
        println!("Source Git revision: {hash}");
    }
    if FEATURES.is_empty() {
        println!("Enabled features: <none>");
    } else {
        println!("Enabled features: {FEATURES}");
    }
    println!();
    println!("Dependencies:");
    for (name, version) in DEPENDENCIES {
        println!(" - {name} {version}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;
    use clap::CommandFactory;

    #[test]
    fn validate_cli() {
        Arguments::command().debug_assert()
    }

    #[test]
    fn just_build_info() {
        let args = Arguments::try_parse_from(["confab", "--build-info"]).unwrap();
        assert!(args.build_info);
    }

    #[test]
    fn build_info_and_args() {
        let args = Arguments::try_parse_from(["confab", "--build-info", "localhost", "80"]);
        assert!(args.is_err());
        assert_eq!(args.unwrap_err().kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn no_args() {
        let args = Arguments::try_parse_from(["confab"]);
        assert!(args.is_err());
        assert_eq!(args.unwrap_err().kind(), ErrorKind::MissingRequiredArgument);
    }
}
