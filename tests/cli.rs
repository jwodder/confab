// <https://github.com/zhiburt/expectrl/issues/52>
#![cfg(unix)]
use assert_matches::assert_matches;
use expectrl::session::{log, OsProcess, OsProcessStream, Session};
use expectrl::stream::log::LogStream;
use expectrl::{ControlCode, Eof, Regex};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_jsonlines::json_lines;
use std::borrow::Cow;
use std::ffi::OsStr;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tempfile::{tempdir, TempDir};
use time::OffsetDateTime;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::oneshot::{channel, Sender};
use tokio::time::sleep;
use tokio_util::codec::{AnyDelimiterCodec, Framed};

#[cfg(unix)]
use expectrl::WaitStatus;

type ExpectrlSession = Session<OsProcess, LogStream<OsProcessStream, std::io::Stdout>>;

struct Tester {
    cmd: Command,
    transcript: bool,
    show_times: bool,
}

impl Tester {
    fn new() -> Tester {
        Tester {
            cmd: Command::new(env!("CARGO_BIN_EXE_confab")),
            transcript: false,
            show_times: false,
        }
    }

    fn arg<S: AsRef<OsStr>>(mut self, arg: S) -> Tester {
        self.cmd.arg(arg);
        self
    }

    fn transcript(mut self) -> Tester {
        self.transcript = true;
        self
    }

    fn show_times(mut self) -> Tester {
        self.show_times = true;
        self
    }

    async fn build(mut self) -> Runner {
        let (sender, receiver) = channel();
        tokio::spawn(async move { testing_server(sender).await });
        let addr = receiver.await.expect("Error receiving address from server");
        let transcript = if self.transcript {
            let transcript = Transcript::new();
            self.cmd.arg("--transcript");
            self.cmd.arg(&transcript.path);
            Some(transcript)
        } else {
            None
        };
        if self.show_times {
            self.cmd.arg("--show-times");
        }
        self.cmd.arg(addr.ip().to_string());
        self.cmd.arg(addr.port().to_string());
        let mut p = log(
            Session::spawn(self.cmd).expect("Error spawning command"),
            std::io::stdout(),
        )
        .unwrap();
        p.set_expect_timeout(Some(Duration::from_millis(500)));
        let mut runner = Runner {
            p,
            addr,
            transcript,
            show_times: self.show_times,
        };
        runner.connect().await;
        runner.get("Welcome to the confab Test Server!").await;
        runner.p.expect("confab> ").await.unwrap();
        runner
    }
}

struct Runner {
    p: ExpectrlSession,
    addr: SocketAddr,
    transcript: Option<Transcript>,
    show_times: bool,
}

impl Runner {
    async fn connect(&mut self) {
        self.expect("* Connecting ...").await;
        self.expect(format!("* Connected to {}", self.addr)).await;
    }

    async fn finish(mut self) {
        self.expect("* Disconnected").await;
        self.p.expect(Eof).await.unwrap();
        #[cfg(unix)]
        assert_eq!(self.p.wait().unwrap(), WaitStatus::Exited(self.p.pid(), 0));
        #[cfg(windows)]
        assert_eq!(self.p.wait(None).unwrap(), 0);
        if let Some(xscript) = self.transcript {
            xscript.check(self.addr);
        }
    }

    async fn expect<S: AsRef<str>>(&mut self, s: S) {
        static TIME_RGX: &str = r"\[[0-9]{2}:[0-9]{2}:[0-9]{2}\]";
        let s = s.as_ref();
        let r = if self.show_times {
            self.p
                .expect(Regex(format!("{} {}", TIME_RGX, regex::escape(s))))
                .await
        } else {
            self.p.expect(s).await
        };
        if let Err(e) = r {
            panic!("confab did not print {s:?}: {e}");
        }
    }

    async fn enter<S: Into<Sent>>(&mut self, entry: S) {
        let entry = entry.into();
        self.p.send(entry.typed()).await.unwrap();
        self.expect(entry.printed()).await;
        self.transcribe(entry.transcription());
    }

    async fn get<R: Into<Recv>>(&mut self, r: R) {
        let r = r.into();
        self.expect(r.printed()).await;
        self.transcribe(r.transcription());
    }

    async fn quit(mut self) {
        self.enter("quit").await;
        self.get(r#"You sent: "quit""#).await;
        self.get("Goodbye.").await;
        self.finish().await;
    }

    async fn cntrl_d(mut self) {
        self.p.send(ControlCode::EndOfTransmission).await.unwrap();
        self.finish().await;
    }

    fn transcribe(&mut self, msg: Msg) {
        if let Some(xscript) = self.transcript.as_mut() {
            xscript.log(msg);
        }
    }
}

struct Transcript {
    _tmpdir: TempDir,
    path: PathBuf,
    messages: Vec<Msg>,
}

impl Transcript {
    fn new() -> Transcript {
        let tmpdir = tempdir().unwrap();
        let path = tmpdir.path().join("transcript.jsonl");
        Transcript {
            _tmpdir: tmpdir,
            path,
            messages: Vec::new(),
        }
    }

    fn log(&mut self, msg: Msg) {
        self.messages.push(msg);
    }

    fn check(&self, addr: SocketAddr) {
        let mut events = json_lines::<Event, _>(&self.path).unwrap();
        assert_matches!(events.next(), Some(Ok(Event::ConnectionStart {host, port, ..})) if host == addr.ip().to_string() && port == addr.port());
        assert_matches!(events.next(), Some(Ok(Event::ConnectionComplete {peer_ip, ..})) if peer_ip == addr.ip());
        for msg in &self.messages {
            match msg {
                Msg::Recv(s) => {
                    assert_matches!(events.next(), Some(Ok(Event::Recv { data, .. })) if &data == s, "{:?}", s.as_ref())
                }
                Msg::Send(s) => {
                    assert_matches!(events.next(), Some(Ok(Event::Send { data, .. })) if &data == s, "{:?}", s.as_ref())
                }
            }
        }
        assert_matches!(events.next(), Some(Ok(Event::Disconnect { .. })));
        assert_matches!(events.next(), None);
    }
}

struct Sent {
    /// Text typed into confab (sans terminating CR LF)
    typed: &'static str,
    /// Text echoed by confab after "> "
    printed: Option<&'static str>,
    /// String stored in transcript, *including* terminating LF
    transcription: Option<&'static str>,
}

impl Sent {
    fn typed(&self) -> String {
        // We have to use CR LF as the line terminator when writing to a
        // terminal:
        format!("{}\r\n", self.typed)
    }

    fn printed(&self) -> String {
        match self.printed {
            Some(s) => format!("> {s}"),
            None => format!("> {}", self.typed),
        }
    }

    fn transcription(&self) -> Msg {
        match self.transcription {
            Some(s) => Msg::Send(s.into()),
            None => Msg::Send(format!("{}\n", self.typed).into()),
        }
    }
}

impl From<&'static str> for Sent {
    fn from(s: &'static str) -> Sent {
        Sent {
            typed: s,
            printed: None,
            transcription: None,
        }
    }
}

struct Recv {
    /// Response from server as output by confab (sans leading "< ")
    printed: &'static str,
    /// Response as stored in transcript, *including* trailing LF (if any)
    transcription: Option<&'static str>,
}

impl Recv {
    fn printed(&self) -> String {
        format!("< {}", self.printed)
    }

    fn transcription(&self) -> Msg {
        match self.transcription {
            Some(s) => Msg::Recv(s.into()),
            None => Msg::Recv(format!("{}\n", self.printed).into()),
        }
    }
}

impl From<&'static str> for Recv {
    fn from(s: &'static str) -> Recv {
        Recv {
            printed: s,
            transcription: None,
        }
    }
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", tag = "event")]
enum Event {
    ConnectionStart {
        #[serde(with = "time::serde::rfc3339")]
        timestamp: OffsetDateTime,
        host: String,
        port: u16,
    },
    ConnectionComplete {
        #[serde(with = "time::serde::rfc3339")]
        timestamp: OffsetDateTime,
        peer_ip: IpAddr,
    },
    TlsStart {
        #[serde(with = "time::serde::rfc3339")]
        timestamp: OffsetDateTime,
    },
    TlsComplete {
        #[serde(with = "time::serde::rfc3339")]
        timestamp: OffsetDateTime,
    },
    Recv {
        #[serde(with = "time::serde::rfc3339")]
        timestamp: OffsetDateTime,
        data: String,
    },
    Send {
        #[serde(with = "time::serde::rfc3339")]
        timestamp: OffsetDateTime,
        data: String,
    },
    Disconnect {
        #[serde(with = "time::serde::rfc3339")]
        timestamp: OffsetDateTime,
    },
    Error {
        #[serde(with = "time::serde::rfc3339")]
        timestamp: OffsetDateTime,
        data: String,
    },
}

#[derive(Debug, Eq, PartialEq)]
enum Msg {
    Recv(Cow<'static, str>),
    Send(Cow<'static, str>),
}

async fn testing_server(sender: Sender<SocketAddr>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Error binding listener");
    sender
        .send(
            listener
                .local_addr()
                .expect("Error getting listener's local address"),
        )
        .expect("Error sending address");
    let (socket, _) = listener
        .accept()
        .await
        .expect("Error listening for connection");
    drop(listener);
    let mut frame = Framed::new(
        socket,
        AnyDelimiterCodec::new_with_max_length(b"\n".to_vec(), b"\n".to_vec(), 65535),
    );
    frame
        .send("Welcome to the confab Test Server!")
        .await
        .unwrap();
    let mut i: usize = 1;
    loop {
        tokio::select! {
            _ = sleep(Duration::from_secs(1)) => {
                frame.send(format!("Ping {i}")).await.unwrap();
                i += 1;
            },
            r = frame.next() => match r {
                Some(Ok(line)) => {
                    let repr = match std::str::from_utf8(line.as_ref()) {
                        Ok(s) => format!("{s:?}"),
                        Err(_) => format!("{line:?}"),
                    };
                    frame.send(format!("You sent: {repr}")).await.unwrap();
                    let line = if line.ends_with(&b"\r"[..]) {
                        line.slice(..(line.len() - 1))
                    } else { line };
                    if line == "quit" {
                        frame.send("Goodbye.").await.unwrap();
                        break;
                    } else if line == "pieces" {
                        let conn = frame.get_mut();
                        conn.write_all(b"This line is|").await.unwrap();
                        sleep(Duration::from_millis(50)).await;
                        conn.write_all(b"being sent in|").await.unwrap();
                        sleep(Duration::from_millis(50)).await;
                        conn.write_all(b"pieces.|").await.unwrap();
                        sleep(Duration::from_millis(50)).await;
                        conn.write_all(b"Did you get it all?\n").await.unwrap();
                    } else if line == "long" {
                        frame.send(concat!(
                            "This is a very long line.  I'm not going t",
                            "o bore you with the details, so instead I'",
                            "ll bore you with some mangled Cicero: Lore",
                            "m ipsum dolor sit amet, consectetur adipis",
                            "icing elit, sed do eiusmod tempor incididu",
                            "nt ut labore et dolore magna aliqua.  Ut e",
                            "nim ad minim veniam, quis nostrud exercita",
                            "tion ullamco laboris nisi ut aliquip ex ea",
                            " commodo consequat."
                        )).await.unwrap();
                    } else if line == "bytes" {
                        let conn = frame.get_mut();
                        conn.write_all(b"Here is some non-UTF-8 data:\n").await.unwrap();
                        conn.write_all(b"Latin-1: Libert\xE9, \xE9galit\xE9, fraternit\xE9\n").await.unwrap();
                        conn.write_all(b"General garbage: \x89\xAB\xCD\xEF\n").await.unwrap();
                    } else if line == "crlf" {
                        frame.send("CR LF:\r").await.unwrap();
                    }
                }
                Some(Err(e)) => panic!("Error reading from connection: {e}"),
                None => break,
            }
        }
    }
}

#[tokio::test]
async fn test_quit_session() {
    let mut r = Tester::new().build().await;
    r.enter("Hello!").await;
    r.get(r#"You sent: "Hello!""#).await;
    r.quit().await;
}

#[tokio::test]
async fn test_async_recv() {
    let mut r = Tester::new().transcript().build().await;
    sleep(Duration::from_secs(1)).await;
    r.get("Ping 1").await;
    sleep(Duration::from_secs(1)).await;
    r.get("Ping 2").await;
    r.quit().await;
}

#[tokio::test]
async fn test_send_ctrl_d() {
    let mut r = Tester::new().build().await;
    r.enter("Hello!").await;
    r.get(r#"You sent: "Hello!""#).await;
    r.cntrl_d().await;
}

#[tokio::test]
async fn test_show_times() {
    let mut r = Tester::new().show_times().build().await;
    sleep(Duration::from_secs(1)).await;
    r.get("Ping 1").await;
    sleep(Duration::from_secs(1)).await;
    r.get("Ping 2").await;
    r.quit().await;
}

#[tokio::test]
async fn test_piecemeal_line() {
    let mut r = Tester::new().transcript().build().await;
    r.enter("pieces").await;
    r.get(r#"You sent: "pieces""#).await;
    r.get("This line is|being sent in|pieces.|Did you get it all?")
        .await;
    r.quit().await;
}

#[tokio::test]
async fn test_long_line() {
    let mut r = Tester::new()
        .arg("--max-line-length")
        .arg("42")
        .transcript()
        .build()
        .await;

    fn unterminated(s: &'static str) -> Recv {
        Recv {
            printed: s,
            transcription: Some(s),
        }
    }

    r.enter("long").await;
    r.get(r#"You sent: "long""#).await;
    r.get(unterminated("This is a very long line.  I'm not going t"))
        .await;
    r.get(unterminated("o bore you with the details, so instead I'"))
        .await;
    r.get(unterminated("ll bore you with some mangled Cicero: Lore"))
        .await;
    r.get(unterminated("m ipsum dolor sit amet, consectetur adipis"))
        .await;
    r.get(unterminated("icing elit, sed do eiusmod tempor incididu"))
        .await;
    r.get(unterminated("nt ut labore et dolore magna aliqua.  Ut e"))
        .await;
    r.get(unterminated("nim ad minim veniam, quis nostrud exercita"))
        .await;
    r.get(unterminated("tion ullamco laboris nisi ut aliquip ex ea"))
        .await;
    r.get(" commodo consequat.").await;
    r.quit().await;
}

#[tokio::test]
async fn test_send_utf8() {
    let mut r = Tester::new().transcript().build().await;
    r.enter("Fëanor is an \u{1F9DD}.  Frosty is a \u{2603}.")
        .await;
    r.get("You sent: \"Fëanor is an \u{1F9DD}.  Frosty is a \u{2603}.\"")
        .await;
    r.quit().await;
}

#[tokio::test]
async fn test_send_latin1() {
    let mut r = Tester::new()
        .arg("-E")
        .arg("latin1")
        .transcript()
        .build()
        .await;
    r.enter(Sent {
        typed: "Fëanor is an \u{1F9DD}.  Frosty is a \u{2603}.",
        printed: Some("Fëanor is an ?.  Frosty is a ?."),
        transcription: Some("Fëanor is an ?.  Frosty is a ?.\n"),
    })
    .await;
    r.get(r#"You sent: b"F\xebanor is an ?.  Frosty is a ?.""#)
        .await;
    r.quit().await;
}

#[tokio::test]
async fn test_receive_non_utf8() {
    let mut r = Tester::new().transcript().build().await;
    r.enter("bytes").await;
    r.get(r#"You sent: "bytes""#).await;
    r.get("Here is some non-UTF-8 data:").await;
    r.get("Latin-1: Libert\u{FFFD}, \u{FFFD}galit\u{FFFD}, fraternit\u{FFFD}")
        .await;
    r.get("General garbage: \u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}")
        .await;
    r.quit().await;
}

#[tokio::test]
async fn test_receive_non_utf8_with_latin1_fallback() {
    let mut r = Tester::new()
        .arg("--encoding=utf8-latin1")
        .transcript()
        .build()
        .await;
    r.enter("bytes").await;
    r.get(r#"You sent: "bytes""#).await;
    r.get("Here is some non-UTF-8 data:").await;
    r.get("Latin-1: Liberté, égalité, fraternité").await;
    r.get(Recv {
        printed: "General garbage: \x1B[7m<U+0089>\x1B[0m\u{AB}\u{CD}\u{EF}",
        transcription: Some("General garbage: \u{89}\u{AB}\u{CD}\u{EF}\n"),
    })
    .await;
    r.quit().await;
}

#[tokio::test]
async fn test_send_crlf() {
    let mut r = Tester::new().arg("--crlf").transcript().build().await;
    r.enter(Sent {
        typed: "crlf",
        printed: None,
        transcription: Some("crlf\r\n"),
    })
    .await;
    r.get(r#"You sent: "crlf\r""#).await;
    // TODO: Properly assert that the carriage return isn't printed in any form
    // here:
    r.get(Recv {
        printed: "CR LF:",
        transcription: Some("CR LF:\r\n"),
    })
    .await;
    r.enter(Sent {
        typed: "quit",
        printed: None,
        transcription: Some("quit\r\n"),
    })
    .await;
    r.get(r#"You sent: "quit\r""#).await;
    r.get("Goodbye.").await;
    r.finish().await;
}

#[tokio::test]
async fn test_no_crlf_recv_crlf() {
    let mut r = Tester::new().transcript().build().await;
    r.enter("crlf").await;
    r.get("You sent: \"crlf\"").await;
    // TODO: Properly assert that the carriage return isn't printed in any form
    // here:
    r.get(Recv {
        printed: "CR LF:",
        transcription: Some("CR LF:\r\n"),
    })
    .await;
    r.quit().await;
}
