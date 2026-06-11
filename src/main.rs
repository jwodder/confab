mod codec;
mod errors;
mod events;
mod input;
mod runner;
mod tls;
mod util;
use crate::input::StartupScript;
use crate::runner::{Connector, Reporter, Runner};
use crate::util::CharEncoding;
use anyhow::Context;
use clap::Parser;
use std::fs::OpenOptions;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use tokio::{fs::File as TokioFile, io::BufReader};

/// Asynchronous line-oriented interactive TCP client
///
/// See <https://github.com/jwodder/confab> for more information
#[derive(Clone, Debug, Eq, Parser, PartialEq)]
#[command(version)]
struct Arguments {
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

    /// Time to wait in milliseconds before sending each line of the startup
    /// script
    #[arg(long, default_value_t = 500, value_name = "INT")]
    startup_wait_ms: u64,

    /// On startup, read lines from the given file and send them to the server
    /// one at a time.
    ///
    /// The user will not be prompted for input until after the end of the file
    /// is reached.
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
    host: String,

    /// Remote port (integer) to which to connect
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
                Duration::from_millis(self.startup_wait_ms),
            ))
        } else {
            None
        };
        Ok(Runner {
            startup_script,
            reporter: Reporter {
                writer: Box::new(std::io::stdout()),
                transcript,
                show_times: self.show_times,
            },
            connector: Connector {
                tls: self.tls,
                host: self.host,
                port: self.port,
                servername: self.servername,
                encoding: self.encoding,
                max_line_length: self.max_line_length,
                crlf: self.crlf,
            },
        })
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<ExitCode> {
    let args = Arguments::parse();
    args.open().await?.run().await.map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use clap::error::ErrorKind;

    #[test]
    fn validate_cli() {
        Arguments::command().debug_assert();
    }

    #[test]
    fn no_args() {
        let args = Arguments::try_parse_from(["confab"]);
        assert!(args.is_err());
        assert_eq!(args.unwrap_err().kind(), ErrorKind::MissingRequiredArgument);
    }
}
