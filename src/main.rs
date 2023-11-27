mod codec;
mod events;
mod input;
mod util;
use crate::codec::ConfabCodec;
use crate::events::Event;
use crate::input::{readline_stream, StartupScript};
use crate::util::{latin1ify, now_hms, CharEncoding, InterfaceError};
use anyhow::Context;
use clap::Parser;
use futures::stream::Stream;
use futures::{SinkExt, StreamExt};
use rustyline_async::{Readline, SharedWriter};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use tokio::fs::File as TokioFile;
use tokio::io::BufReader;
use tokio::net::TcpStream;
use tokio_util::{codec::Framed, either::Either};

cfg_if::cfg_if! {
    if #[cfg(feature = "rustls")] {
        mod rustls;
        use crate::rustls as tls;
    } else if #[cfg(feature = "native")] {
        mod native_tls;
        use crate::native_tls as tls;
    } else {
        compile_error("confab requires feature \"rustls\" or \"native\" to be enabled")
    }
}

type Connection = Framed<Either<TcpStream, tls::TlsStream>, ConfabCodec>;

mod build {
    include!(concat!(env!("OUT_DIR"), "/build_info.rs"));
}

/// Asynchronous line-oriented interactive TCP client
///
/// See <https://github.com/jwodder/confab> for more information
#[derive(Clone, Debug, Eq, Parser, PartialEq)]
#[command(version)]
struct Arguments {
    /// Display a summary of build information & dependencies and exit
    #[arg(long, exclusive = true)]
    build_info: bool,

    /// Terminate sent lines with CR LF instead of just LF
    #[arg(long)]
    crlf: bool,

    /// Set text encoding
    ///
    /// "utf8" converts invalid byte sequences to the replacement character.
    /// "utf8-latin1" handles invalid byte sequences by decoding the entire
    /// line as Latin-1.
    #[arg(
        short = 'E',
        long,
        default_value = "utf8",
        value_name = "utf8|utf8-latin1|latin1"
    )]
    encoding: CharEncoding,

    /// Set maximum length in bytes of lines read from remote server
    ///
    /// If the server sends a line longer than this (including the terminating
    /// newline), the first `<LIMIT>` bytes will be split off and treated as a
    /// whole line, with the remaining bytes treated as the start of a new
    /// line.
    #[arg(long, default_value = "65535", value_name = "LIMIT")]
    max_line_length: NonZeroUsize,

    /// Use the given domain name for SNI and certificate hostname validation
    /// [default: the remote host name]
    #[arg(long, value_name = "DOMAIN")]
    servername: Option<String>,

    #[arg(long, default_value_t = 500, value_name = "INT")]
    startup_line_wait_ms: u64,

    #[arg(short = 'S', long, value_name = "FILE")]
    startup_script: Option<PathBuf>,

    /// Prepend timestamps to output messages
    #[arg(short = 't', long)]
    show_times: bool,

    /// Connect using SSL/TLS
    #[arg(long)]
    tls: bool,

    /// Append a transcript of events to the given file
    #[arg(short = 'T', long, value_name = "FILE")]
    transcript: Option<PathBuf>,

    /// Remote host (domain name or IP address) to which to connect
    #[arg(default_value = "localhost", required = true)]
    // The dummy default value is just there so that `--build-info` can be made
    // exclusive.
    host: String,

    /// Remote port (integer) to which to connect
    #[arg(default_value_t = 80, required = true)]
    // The dummy default value is just there so that `--build-info` can be made
    // exclusive.
    port: u16,
}

impl Arguments {
    async fn open(self) -> anyhow::Result<Runner> {
        let transcript = self
            .transcript
            .map(|p| {
                OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(p)
                    .context("failed to open transcript file")
            })
            .transpose()?;
        let startup_script = if let Some(path) = self.startup_script {
            let fp = BufReader::new(
                TokioFile::open(path)
                    .await
                    .context("failed to open startup script")?,
            );
            Some(StartupScript::new(
                fp,
                Duration::from_millis(self.startup_line_wait_ms),
            ))
        } else {
            None
        };
        Ok(Runner {
            startup_script,
            reporter: Reporter {
                writer: Box::new(io::stdout()),
                transcript,
                show_times: self.show_times,
            },
            crlf: self.crlf,
            connector: Connector {
                tls: self.tls,
                host: self.host,
                port: self.port,
                servername: self.servername,
                encoding: self.encoding,
                max_line_length: self.max_line_length,
            },
        })
    }
}

struct Reporter {
    writer: Box<dyn Write + Send>,
    transcript: Option<File>,
    show_times: bool,
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Connector {
    tls: bool,
    host: String,
    port: u16,
    servername: Option<String>,
    encoding: CharEncoding,
    max_line_length: NonZeroUsize,
}

impl Connector {
    async fn connect(&self, reporter: &mut Reporter) -> anyhow::Result<Connection> {
        reporter.report(Event::connect_start(&self.host, self.port))?;
        let conn = TcpStream::connect((self.host.clone(), self.port))
            .await
            .context("Error connecting to server")?;
        reporter.report(Event::connect_finish(
            conn.peer_addr().context("Error getting peer address")?,
        ))?;
        let conn = if self.tls {
            reporter.report(Event::tls_start())?;
            let conn = tls::connect(conn, self.servername.as_ref().unwrap_or(&self.host)).await?;
            reporter.report(Event::tls_finish())?;
            Either::Right(conn)
        } else {
            Either::Left(conn)
        };
        Ok(Framed::new(conn, self.codec()))
    }

    fn codec(&self) -> ConfabCodec {
        ConfabCodec::new_with_max_length(self.max_line_length.get()).encoding(self.encoding)
    }
}

struct Runner {
    startup_script: Option<StartupScript>,
    reporter: Reporter,
    crlf: bool,
    connector: Connector,
}

impl Runner {
    async fn run(mut self) -> anyhow::Result<ExitCode> {
        match self.try_run().await {
            Ok(()) => Ok(ExitCode::SUCCESS),
            Err(e) if e.is::<InterfaceError>() => Err(e),
            Err(e) => {
                // TODO: This won't be written if reporter.writer is
                // SharedWriter
                self.reporter.report(Event::error(e))?;
                Ok(ExitCode::FAILURE)
            }
        }
    }

    async fn try_run(&mut self) -> anyhow::Result<()> {
        let mut frame = self.connector.connect(&mut self.reporter).await?;
        if let Some(script) = self.startup_script.take() {
            ioloop(&mut frame, script, self.crlf, &mut self.reporter).await?;
        }
        let (mut rl, shared) = init_readline()?;
        // Lines written to the SharedWriter are only output when
        // Readline::readline() is called, so anything written before we start
        // getting input from the user should be written directly to stdout
        // instead.
        self.reporter.set_writer(Box::new(shared));
        let r = ioloop(
            &mut frame,
            readline_stream(&mut rl).await,
            self.crlf,
            &mut self.reporter,
        )
        .await;
        let _ = rl.flush();
        drop(rl);
        // Set the writer back to stdout so that errors reported by run() will
        // show up
        self.reporter.set_writer(Box::new(io::stdout()));
        let r2 = self.reporter.report(Event::disconnect());
        r.and(r2.map_err(Into::into))
    }
}

async fn ioloop<S>(
    frame: &mut Connection,
    input: S,
    crlf: bool,
    reporter: &mut Reporter,
) -> anyhow::Result<()>
where
    S: Stream<Item = Result<String, InterfaceError>> + Send,
{
    tokio::pin!(input);
    loop {
        let event = tokio::select! {
            r = frame.next() => match r {
                Some(Ok(msg)) => Event::recv(msg),
                Some(Err(e)) => return Err(e).context("Error reading from connection"),
                None => break,
            },
            r = input.next() => match r {
                Some(Ok(mut line)) => {
                    if frame.codec().get_encoding() == CharEncoding::Latin1 {
                        // We need to convert non-Latin-1 characters to '?'
                        // here rather than waiting for the codec to do it
                        // so that the Event will reflect the actual
                        // characters sent.
                        line = latin1ify(line);
                    }
                    if crlf {
                        line.push_str("\r\n");
                    } else {
                        line.push('\n');
                    }
                    frame.send(&line).await.context("Error sending message")?;
                    Event::send(line)
                }
                Some(Err(e)) => return Err(e.into()),
                None => break,
            }
        };
        reporter.report(event)?;
    }
    Ok(())
}

fn init_readline() -> anyhow::Result<(Readline, SharedWriter)> {
    let (mut rl, shared) =
        Readline::new(String::from("confab> ")).context("Error constructing Readline object")?;
    rl.should_print_line_on(false, false);
    Ok((rl, shared))
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let args = Arguments::parse();
    if args.build_info {
        build_info();
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(args.open().await?.run().await?)
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
        Arguments::command().debug_assert();
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
