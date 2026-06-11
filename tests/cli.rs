#![cfg(test)]
#![cfg(unix)]
use assert_matches::assert_matches;
use bstr::ByteSlice;
use futures_util::{SinkExt, StreamExt};
use pty_process::{Command, Pty};
use serde::Deserialize;
use serde_jsonlines::json_lines;
use std::borrow::Cow;
use std::ffi::OsStr;
use std::io::{Seek, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitStatus;
use std::time::Duration;
use tempfile::{NamedTempFile, TempDir, tempdir};
use time::OffsetDateTime;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    process::Child,
    sync::oneshot::{Sender, channel},
    time::{Instant, sleep, timeout, timeout_at},
};
use tokio_util::codec::{AnyDelimiterCodec, Framed};

const EXPECT_TIMEOUT: Duration = Duration::from_millis(500);

trait StrMatcher: std::fmt::Debug {
    // Returns the index of `s` at which the match ends
    fn matches(&self, s: &[u8]) -> Option<usize>;
}

impl StrMatcher for &str {
    fn matches(&self, s: &[u8]) -> Option<usize> {
        s.find(self).map(|i| i + self.len())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Timestamped<'a>(&'a str);

impl StrMatcher for Timestamped<'_> {
    fn matches(&self, s: &[u8]) -> Option<usize> {
        // Timestamp format: [dd:dd:dd]
        for i in s.find_iter(b"[") {
            let mut iter = s[(i + 1)..].bytes();
            if iter.next().is_some_and(|c| char::from(c).is_ascii_digit())
                && iter.next().is_some_and(|c| char::from(c).is_ascii_digit())
                && iter.next() == Some(b':')
                && iter.next().is_some_and(|c| char::from(c).is_ascii_digit())
                && iter.next().is_some_and(|c| char::from(c).is_ascii_digit())
                && iter.next() == Some(b':')
                && iter.next().is_some_and(|c| char::from(c).is_ascii_digit())
                && iter.next().is_some_and(|c| char::from(c).is_ascii_digit())
                && iter.next() == Some(b']')
                && iter.next() == Some(b' ')
                && iter.as_bytes().starts_with_str(self.0)
            {
                return Some(i + "[dd:dd:dd] ".len() + self.0.len());
            }
        }
        None
    }
}

struct Tester {
    cmd: Command,
    transcript: bool,
    show_times: bool,
}

impl Tester {
    fn new() -> Tester {
        Tester {
            cmd: Command::new(env!("CARGO_BIN_EXE_confab")).kill_on_drop(true),
            transcript: false,
            show_times: false,
        }
    }

    fn arg<S: AsRef<OsStr>>(mut self, arg: S) -> Tester {
        self.cmd = self.cmd.arg(arg);
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
        #[allow(clippy::if_then_some_else_none)]
        let transcript = if self.transcript {
            let transcript = Transcript::new();
            self.cmd = self.cmd.arg("--transcript");
            self.cmd = self.cmd.arg(&transcript.path);
            Some(transcript)
        } else {
            None
        };
        if self.show_times {
            self.cmd = self.cmd.arg("--show-times");
        }
        self.cmd = self.cmd.arg(addr.ip().to_string());
        self.cmd = self.cmd.arg(addr.port().to_string());
        let (pty, pts) = pty_process::open().expect("Error creating pty");
        pty.resize(pty_process::Size::new(80, 24))
            .expect("Error resizing pty");
        let child = self.cmd.spawn(pts).expect("Error spawning command");
        let mut runner = Runner {
            pty,
            child,
            buffer: Vec::new(),
            addr,
            transcript,
            show_times: self.show_times,
        };
        runner.connect().await;
        runner.get("Welcome to the confab Test Server!").await;
        runner
    }
}

struct Runner {
    pty: Pty,
    child: Child,
    buffer: Vec<u8>,
    addr: SocketAddr,
    transcript: Option<Transcript>,
    show_times: bool,
}

impl Runner {
    // Returns `false` on EOF
    async fn read(&mut self) -> std::io::Result<bool> {
        let mut buf = vec![0u8; 2048];
        match self.pty.read(&mut buf).await {
            Ok(0) => Ok(false),
            #[cfg(target_os = "linux")]
            Err(e) if e.raw_os_error() == Some(5) => {
                // On Linux, attempting to read from a pty master after the
                // slave closes (due, e.g., to the child process exiting)
                // results in EIO (which Rust currently represents with the
                // undocumented ErrorKind::Uncategorized).
                Ok(false)
            }
            Ok(n) => {
                self.buffer.extend(&buf[..n]);
                Ok(true)
            }
            Err(e) => Err(e),
        }
    }

    fn contents(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.buffer)
    }

    #[allow(clippy::match_wild_err_arm)]
    async fn wait_for_contents<M: StrMatcher>(&mut self, expected: M) -> std::io::Result<()> {
        if let Some(i) = expected.matches(&self.buffer) {
            self.buffer.drain(..i);
            return Ok(());
        }
        let deadline = Instant::now() + EXPECT_TIMEOUT;
        loop {
            match timeout_at(deadline, self.read()).await {
                Ok(Ok(true)) => {
                    if let Some(i) = expected.matches(&self.buffer) {
                        self.buffer.drain(..i);
                        return Ok(());
                    }
                }
                Ok(Ok(false)) => {
                    panic!(
                        "Reached EOF while waiting for output {expected:?}; bytes read = {:?}",
                        self.contents(),
                    );
                }
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    panic!(
                        "Timed out while waiting for output {expected:?}; bytes read = {:?}",
                        self.contents(),
                    );
                }
            }
        }
    }

    async fn wait_for_exit(&mut self, d: Duration) -> std::io::Result<ExitStatus> {
        if let Ok(r) = timeout(d, self.inner_wait_for_exit()).await {
            r
        } else {
            panic!(
                "Timed out while waiting for exit; final content = {:?}",
                self.contents()
            );
        }
    }

    async fn inner_wait_for_exit(&mut self) -> std::io::Result<ExitStatus> {
        while self.read().await? {}
        self.child.wait().await
    }

    async fn connect(&mut self) {
        self.expect("* Connecting ...").await;
        self.expect(format!("* Connected to {}", self.addr)).await;
    }

    async fn finish(mut self) {
        self.expect("* Disconnected").await;
        let rc = self.wait_for_exit(EXPECT_TIMEOUT).await.unwrap();
        assert!(
            rc.success(),
            "child process did not exit successfully: {rc}"
        );
        if let Some(xscript) = self.transcript {
            xscript.check(self.addr);
        }
    }

    async fn expect<S: AsRef<str> + Send>(&mut self, s: S) {
        let s = s.as_ref();
        let r = if self.show_times {
            self.wait_for_contents(Timestamped(s)).await
        } else {
            self.wait_for_contents(s).await
        };
        if let Err(e) = r {
            panic!("confab did not print {s:?}: {e}");
        }
    }

    async fn enter<S: Into<Sent> + Send>(&mut self, entry: S) {
        let entry = entry.into();
        self.wait_for_contents("confab> ").await.unwrap();
        self.pty.write_all(entry.typed().as_bytes()).await.unwrap();
        self.expect(entry.printed()).await;
        self.transcribe(entry.transcription());
    }

    async fn script_enter<S: Into<Sent> + Send>(&mut self, entry: S) {
        let entry = entry.into();
        self.expect(entry.printed()).await;
        self.transcribe(entry.transcription());
    }

    async fn get<R: Into<Recv> + Send>(&mut self, r: R) {
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
        self.pty.write_all(b"\x04").await.unwrap();
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
        assert_matches!(events.next(), Some(Ok(Event::ConnectionStart {host, port, ..})) => {
            assert_eq!(host, addr.ip().to_string());
            assert_eq!(port, addr.port());
        });
        assert_matches!(events.next(), Some(Ok(Event::ConnectionComplete {peer_ip, ..})) => {
            assert_eq!(peer_ip, addr.ip());
        });
        for msg in &self.messages {
            match msg {
                Msg::Recv(s) => {
                    assert_matches!(events.next(), Some(Ok(Event::Recv { data, .. })) => {
                        assert_eq!(&data, s, "{:?}", s.as_ref());
                    });
                }
                Msg::Send(s) => {
                    assert_matches!(events.next(), Some(Ok(Event::Send { data, .. })) => {
                        assert_eq!(&data, s, "{:?}", s.as_ref());
                    });
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
        format!("> {}", self.printed.unwrap_or(self.typed))
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
            () = sleep(Duration::from_secs(1)) => {
                frame.send(format!("Ping {i}")).await.unwrap();
                i += 1;
            },
            r = frame.next() => match r {
                Some(Ok(line)) => {
                    let repr = if let Ok(s) = std::str::from_utf8(line.as_ref()) {
                        format!("{s:?}")
                    } else {
                        format!("{line:?}")
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
    fn unterminated(s: &'static str) -> Recv {
        Recv {
            printed: s,
            transcription: Some(s),
        }
    }

    let mut r = Tester::new()
        .arg("--max-line-length")
        .arg("42")
        .transcript()
        .build()
        .await;
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

#[tokio::test]
async fn startup_script() {
    let mut scriptfile = NamedTempFile::new().unwrap();
    writeln!(scriptfile, "Hello!").unwrap();
    writeln!(scriptfile, "This is from a startup script.").unwrap();
    scriptfile.flush().unwrap();
    scriptfile.rewind().unwrap();

    let mut r = Tester::new()
        .arg("--startup-script")
        .arg(scriptfile.path())
        .transcript()
        .build()
        .await;

    sleep(Duration::from_millis(500)).await;
    r.script_enter("Hello!").await;
    r.get(r#"You sent: "Hello!""#).await;
    sleep(Duration::from_millis(500)).await;
    r.script_enter("This is from a startup script.").await;
    r.get(r#"You sent: "This is from a startup script.""#).await;

    r.enter("Hello again!").await;
    r.get(r#"You sent: "Hello again!""#).await;
    r.enter("This is from the prompt.").await;
    r.get(r#"You sent: "This is from the prompt.""#).await;

    r.quit().await;
}

#[tokio::test]
async fn quit_from_startup_script() {
    let mut scriptfile = NamedTempFile::new().unwrap();
    writeln!(scriptfile, "Hello!").unwrap();
    writeln!(scriptfile, "quit").unwrap();
    writeln!(scriptfile, "wait no-").unwrap();
    scriptfile.flush().unwrap();
    scriptfile.rewind().unwrap();
    let mut r = Tester::new()
        .arg("--startup-script")
        .arg(scriptfile.path())
        .transcript()
        .build()
        .await;
    sleep(Duration::from_millis(500)).await;
    r.script_enter("Hello!").await;
    r.get(r#"You sent: "Hello!""#).await;
    sleep(Duration::from_millis(500)).await;
    r.script_enter("quit").await;
    r.get(r#"You sent: "quit""#).await;
    r.get("Goodbye.").await;
    r.finish().await;
}
