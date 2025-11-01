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
    if args.build_info {
        build_info();
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(args.open().await?.run().await?)
    }
}

#[allow(clippy::const_is_empty)] // Shut clippy up about FEATURES.is_empty()
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
    use clap::CommandFactory;
    use clap::error::ErrorKind;

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
