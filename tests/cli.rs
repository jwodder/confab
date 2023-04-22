// <https://github.com/zhiburt/expectrl/issues/52>
#![cfg(unix)]
use assert_matches::assert_matches;
use chrono::{offset::FixedOffset, DateTime};
use expectrl::session::{log, OsProcess, OsProcessStream, Session};
use expectrl::stream::log::LogStream;
use expectrl::{ControlCode, Eof, Regex};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_jsonlines::json_lines;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tempfile::tempdir;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::oneshot::{channel, Sender};
use tokio::time::sleep;
use tokio_util::codec::{AnyDelimiterCodec, Framed};

#[cfg(unix)]
use expectrl::WaitStatus;

type ExpectrlSession = Session<OsProcess, LogStream<OsProcessStream, std::io::Stdout>>;

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", tag = "event")]
enum Event {
    ConnectionStart {
        timestamp: DateTime<FixedOffset>,
        host: String,
        port: u16,
    },
    ConnectionComplete {
        timestamp: DateTime<FixedOffset>,
        peer_ip: IpAddr,
    },
    TlsStart {
        timestamp: DateTime<FixedOffset>,
    },
    TlsComplete {
        timestamp: DateTime<FixedOffset>,
    },
    Recv {
        timestamp: DateTime<FixedOffset>,
        data: String,
    },
    Send {
        timestamp: DateTime<FixedOffset>,
        data: String,
    },
    Disconnect {
        timestamp: DateTime<FixedOffset>,
    },
    Error {
        timestamp: DateTime<FixedOffset>,
        data: String,
    },
}

#[derive(Debug, Eq, PartialEq)]
enum Msg {
    Recv(&'static str),
    Send(&'static str),
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

async fn start_session(opts: &[&str]) -> (ExpectrlSession, SocketAddr) {
    let (sender, receiver) = channel();
    tokio::spawn(async move { testing_server(sender).await });
    let addr = receiver.await.expect("Error receiving address from server");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_confab"));
    cmd.args(opts);
    cmd.arg(addr.ip().to_string());
    cmd.arg(addr.port().to_string());
    let mut p = log(
        Session::spawn(cmd).expect("Error spawning command"),
        std::io::stdout(),
    )
    .unwrap();
    p.set_expect_timeout(Some(Duration::from_millis(500)));
    p.expect("* Connecting ...").await.unwrap();
    p.expect(format!("* Connected to {addr}")).await.unwrap();
    p.expect("< Welcome to the confab Test Server!")
        .await
        .unwrap();
    p.expect("confab> ").await.unwrap();
    (p, addr)
}

async fn end_session(mut p: ExpectrlSession) {
    p.expect("* Disconnected").await.unwrap();
    p.expect(Eof).await.unwrap();
    #[cfg(unix)]
    assert_eq!(p.wait().unwrap(), WaitStatus::Exited(p.pid(), 0));
    #[cfg(windows)]
    assert_eq!(p.wait(None).unwrap(), 0);
}

fn check_transcript(path: PathBuf, addr: SocketAddr, messages: &[Msg]) {
    let mut events = json_lines::<Event, _>(path).unwrap();
    assert_matches!(events.next(), Some(Ok(Event::ConnectionStart {host, port, ..})) if host == addr.ip().to_string() && port == addr.port());
    assert_matches!(events.next(), Some(Ok(Event::ConnectionComplete {peer_ip, ..})) if peer_ip == addr.ip());
    assert_matches!(
        events.next(),
        Some(Ok(Event::Recv { data, .. })) if data == "Welcome to the confab Test Server!\n"
    );
    for msg in messages {
        match msg {
            Msg::Recv(s) => {
                assert_matches!(events.next(), Some(Ok(Event::Recv { data, .. })) if data == *s)
            }
            Msg::Send(s) => {
                assert_matches!(events.next(), Some(Ok(Event::Send { data, .. })) if data == *s)
            }
        }
    }
    assert_matches!(events.next(), Some(Ok(Event::Disconnect { .. })));
    assert_matches!(events.next(), None);
}

#[tokio::test]
async fn test_quit_session() {
    let (mut p, _) = start_session(&[]).await;
    p.send("Hello!\r\n").await.unwrap();
    p.expect("> Hello!").await.unwrap();
    p.expect(r#"< You sent: "Hello!""#).await.unwrap();
    p.send("quit\r\n").await.unwrap();
    p.expect("> quit").await.unwrap();
    p.expect(r#"< You sent: "quit""#).await.unwrap();
    p.expect("< Goodbye.").await.unwrap();
    end_session(p).await;
}

#[tokio::test]
async fn test_async_recv() {
    let (mut p, _) = start_session(&[]).await;
    sleep(Duration::from_secs(1)).await;
    p.expect("< Ping 1").await.unwrap();
    sleep(Duration::from_secs(1)).await;
    p.expect("< Ping 2").await.unwrap();
    p.send("quit\r\n").await.unwrap();
    p.expect("> quit").await.unwrap();
    p.expect(r#"< You sent: "quit""#).await.unwrap();
    p.expect("< Goodbye.").await.unwrap();
    end_session(p).await;
}

#[tokio::test]
async fn test_send_ctrl_d() {
    let (mut p, _) = start_session(&[]).await;
    p.send("Hello!\r\n").await.unwrap();
    p.expect("> Hello!").await.unwrap();
    p.expect(r#"< You sent: "Hello!""#).await.unwrap();
    p.send(ControlCode::EndOfTransmission).await.unwrap();
    end_session(p).await;
}

#[tokio::test]
async fn test_show_times() {
    static TIME_RGX: &str = r#"\[[0-9]{2}:[0-9]{2}:[0-9]{2}\]"#;
    let (mut p, _) = start_session(&["--show-times"]).await;
    sleep(Duration::from_secs(1)).await;
    p.expect(Regex(format!("{} < Ping 1", TIME_RGX)))
        .await
        .unwrap();
    sleep(Duration::from_secs(1)).await;
    p.expect(Regex(format!("{} < Ping 2", TIME_RGX)))
        .await
        .unwrap();
    p.send("quit\r\n").await.unwrap();
    p.expect(Regex(format!("{} > quit", TIME_RGX)))
        .await
        .unwrap();
    p.expect(Regex(format!(r#"{} < You sent: "quit""#, TIME_RGX)))
        .await
        .unwrap();
    p.expect(Regex(format!(r#"{} < Goodbye\."#, TIME_RGX)))
        .await
        .unwrap();
    end_session(p).await;
}

#[tokio::test]
async fn test_piecemeal_line() {
    let tmpdir = tempdir().unwrap();
    let transcript = tmpdir.path().join("transcript.jsonl");
    let (mut p, addr) = start_session(&["--transcript", transcript.to_str().unwrap()]).await;
    p.send("pieces\r\n").await.unwrap();
    p.expect("> pieces").await.unwrap();
    p.expect(r#"< You sent: "pieces""#).await.unwrap();
    p.expect("< This line is|being sent in|pieces.|Did you get it all?")
        .await
        .unwrap();
    p.send("quit\r\n").await.unwrap();
    p.expect("> quit").await.unwrap();
    p.expect(r#"< You sent: "quit""#).await.unwrap();
    p.expect("< Goodbye.").await.unwrap();
    end_session(p).await;
    check_transcript(
        transcript,
        addr,
        &[
            Msg::Send("pieces\n"),
            Msg::Recv("You sent: \"pieces\"\n"),
            Msg::Recv("This line is|being sent in|pieces.|Did you get it all?\n"),
            Msg::Send("quit\n"),
            Msg::Recv("You sent: \"quit\"\n"),
            Msg::Recv("Goodbye.\n"),
        ],
    );
}

#[tokio::test]
async fn test_long_line() {
    let tmpdir = tempdir().unwrap();
    let transcript = tmpdir.path().join("transcript.jsonl");
    let (mut p, addr) = start_session(&[
        "--max-line-length",
        "42",
        "--transcript",
        transcript.to_str().unwrap(),
    ])
    .await;
    p.send("long\r\n").await.unwrap();
    p.expect("> long").await.unwrap();
    p.expect(r#"< You sent: "long""#).await.unwrap();
    p.expect("< This is a very long line.  I'm not going t")
        .await
        .unwrap();
    p.expect("< o bore you with the details, so instead I'")
        .await
        .unwrap();
    p.expect("< ll bore you with some mangled Cicero: Lore")
        .await
        .unwrap();
    p.expect("< m ipsum dolor sit amet, consectetur adipis")
        .await
        .unwrap();
    p.expect("< icing elit, sed do eiusmod tempor incididu")
        .await
        .unwrap();
    p.expect("< nt ut labore et dolore magna aliqua.  Ut e")
        .await
        .unwrap();
    p.expect("< nim ad minim veniam, quis nostrud exercita")
        .await
        .unwrap();
    p.expect("< tion ullamco laboris nisi ut aliquip ex ea")
        .await
        .unwrap();
    p.expect("<  commodo consequat.").await.unwrap();
    p.send("quit\r\n").await.unwrap();
    p.expect("> quit").await.unwrap();
    p.expect(r#"< You sent: "quit""#).await.unwrap();
    p.expect("< Goodbye.").await.unwrap();
    end_session(p).await;
    check_transcript(
        transcript,
        addr,
        &[
            Msg::Send("long\n"),
            Msg::Recv("You sent: \"long\"\n"),
            Msg::Recv("This is a very long line.  I'm not going t"),
            Msg::Recv("o bore you with the details, so instead I'"),
            Msg::Recv("ll bore you with some mangled Cicero: Lore"),
            Msg::Recv("m ipsum dolor sit amet, consectetur adipis"),
            Msg::Recv("icing elit, sed do eiusmod tempor incididu"),
            Msg::Recv("nt ut labore et dolore magna aliqua.  Ut e"),
            Msg::Recv("nim ad minim veniam, quis nostrud exercita"),
            Msg::Recv("tion ullamco laboris nisi ut aliquip ex ea"),
            Msg::Recv(" commodo consequat.\n"),
            Msg::Send("quit\n"),
            Msg::Recv("You sent: \"quit\"\n"),
            Msg::Recv("Goodbye.\n"),
        ],
    );
}

#[tokio::test]
async fn test_send_utf8() {
    let tmpdir = tempdir().unwrap();
    let transcript = tmpdir.path().join("transcript.jsonl");
    let (mut p, addr) = start_session(&["--transcript", transcript.to_str().unwrap()]).await;
    p.send("Fëanor is an \u{1F9DD}.  Frosty is a \u{2603}.\r\n")
        .await
        .unwrap();
    p.expect("> Fëanor is an \u{1F9DD}.  Frosty is a \u{2603}.")
        .await
        .unwrap();
    p.expect("< You sent: \"Fëanor is an \u{1F9DD}.  Frosty is a \u{2603}.\"")
        .await
        .unwrap();
    p.send("quit\r\n").await.unwrap();
    p.expect("> quit").await.unwrap();
    p.expect(r#"< You sent: "quit""#).await.unwrap();
    p.expect("< Goodbye.").await.unwrap();
    end_session(p).await;
    check_transcript(
        transcript,
        addr,
        &[
            Msg::Send("Fëanor is an \u{1F9DD}.  Frosty is a \u{2603}.\n"),
            Msg::Recv("You sent: \"Fëanor is an \u{1F9DD}.  Frosty is a \u{2603}.\"\n"),
            Msg::Send("quit\n"),
            Msg::Recv("You sent: \"quit\"\n"),
            Msg::Recv("Goodbye.\n"),
        ],
    );
}

#[tokio::test]
async fn test_send_latin1() {
    let tmpdir = tempdir().unwrap();
    let transcript = tmpdir.path().join("transcript.jsonl");
    let (mut p, addr) =
        start_session(&["-E", "latin1", "--transcript", transcript.to_str().unwrap()]).await;
    p.send("Fëanor is an \u{1F9DD}.  Frosty is a \u{2603}.\r\n")
        .await
        .unwrap();
    p.expect("> Fëanor is an ?.  Frosty is a ?.").await.unwrap();
    p.expect(r#"< You sent: b"F\xebanor is an ?.  Frosty is a ?.""#)
        .await
        .unwrap();
    p.send("quit\r\n").await.unwrap();
    p.expect("> quit").await.unwrap();
    p.expect(r#"< You sent: "quit""#).await.unwrap();
    p.expect("< Goodbye.").await.unwrap();
    end_session(p).await;
    check_transcript(
        transcript,
        addr,
        &[
            Msg::Send("Fëanor is an ?.  Frosty is a ?.\n"),
            Msg::Recv("You sent: b\"F\\xebanor is an ?.  Frosty is a ?.\"\n"),
            Msg::Send("quit\n"),
            Msg::Recv("You sent: \"quit\"\n"),
            Msg::Recv("Goodbye.\n"),
        ],
    );
}

#[tokio::test]
async fn test_receive_non_utf8() {
    let tmpdir = tempdir().unwrap();
    let transcript = tmpdir.path().join("transcript.jsonl");
    let (mut p, addr) = start_session(&["--transcript", transcript.to_str().unwrap()]).await;
    p.send("bytes\r\n").await.unwrap();
    p.expect("> bytes").await.unwrap();
    p.expect(r#"< You sent: "bytes""#).await.unwrap();
    p.expect("< Here is some non-UTF-8 data:").await.unwrap();
    p.expect("< Latin-1: Libert\u{FFFD}, \u{FFFD}galit\u{FFFD}, fraternit\u{FFFD}")
        .await
        .unwrap();
    p.expect("< General garbage: \u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}")
        .await
        .unwrap();
    p.send("quit\r\n").await.unwrap();
    p.expect("> quit").await.unwrap();
    p.expect(r#"< You sent: "quit""#).await.unwrap();
    p.expect("< Goodbye.").await.unwrap();
    end_session(p).await;
    check_transcript(
        transcript,
        addr,
        &[
            Msg::Send("bytes\n"),
            Msg::Recv("You sent: \"bytes\"\n"),
            Msg::Recv("Here is some non-UTF-8 data:\n"),
            Msg::Recv("Latin-1: Libert\u{FFFD}, \u{FFFD}galit\u{FFFD}, fraternit\u{FFFD}\n"),
            Msg::Recv("General garbage: \u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\n"),
            Msg::Send("quit\n"),
            Msg::Recv("You sent: \"quit\"\n"),
            Msg::Recv("Goodbye.\n"),
        ],
    );
}

#[tokio::test]
async fn test_receive_non_utf8_with_latin1_fallback() {
    let tmpdir = tempdir().unwrap();
    let transcript = tmpdir.path().join("transcript.jsonl");
    let (mut p, addr) = start_session(&[
        "--encoding=utf8-latin1",
        "--transcript",
        transcript.to_str().unwrap(),
    ])
    .await;
    p.send("bytes\r\n").await.unwrap();
    p.expect("> bytes").await.unwrap();
    p.expect(r#"< You sent: "bytes""#).await.unwrap();
    p.expect("< Here is some non-UTF-8 data:").await.unwrap();
    p.expect("< Latin-1: Liberté, égalité, fraternité")
        .await
        .unwrap();
    p.expect("< General garbage: \x1B[7m<U+0089>\x1B[0m\u{AB}\u{CD}\u{EF}")
        .await
        .unwrap();
    p.send("quit\r\n").await.unwrap();
    p.expect("> quit").await.unwrap();
    p.expect(r#"< You sent: "quit""#).await.unwrap();
    p.expect("< Goodbye.").await.unwrap();
    end_session(p).await;
    check_transcript(
        transcript,
        addr,
        &[
            Msg::Send("bytes\n"),
            Msg::Recv("You sent: \"bytes\"\n"),
            Msg::Recv("Here is some non-UTF-8 data:\n"),
            Msg::Recv("Latin-1: Liberté, égalité, fraternité\n"),
            Msg::Recv("General garbage: \u{89}\u{AB}\u{CD}\u{EF}\n"),
            Msg::Send("quit\n"),
            Msg::Recv("You sent: \"quit\"\n"),
            Msg::Recv("Goodbye.\n"),
        ],
    );
}

#[tokio::test]
async fn test_transcript() {
    let tmpdir = tempdir().unwrap();
    let transcript = tmpdir.path().join("transcript.jsonl");
    let (mut p, addr) = start_session(&["--transcript", transcript.to_str().unwrap()]).await;
    sleep(Duration::from_secs(1)).await;
    p.expect("< Ping 1").await.unwrap();
    sleep(Duration::from_secs(1)).await;
    p.expect("< Ping 2").await.unwrap();
    p.send("Hello!\r\n").await.unwrap();
    p.expect("> Hello!").await.unwrap();
    p.expect(r#"< You sent: "Hello!""#).await.unwrap();
    p.send("quit\r\n").await.unwrap();
    p.expect("> quit").await.unwrap();
    p.expect(r#"< You sent: "quit""#).await.unwrap();
    p.expect("< Goodbye.").await.unwrap();
    end_session(p).await;
    check_transcript(
        transcript,
        addr,
        &[
            Msg::Recv("Ping 1\n"),
            Msg::Recv("Ping 2\n"),
            Msg::Send("Hello!\n"),
            Msg::Recv("You sent: \"Hello!\"\n"),
            Msg::Send("quit\n"),
            Msg::Recv("You sent: \"quit\"\n"),
            Msg::Recv("Goodbye.\n"),
        ],
    );
}

#[tokio::test]
async fn test_send_crlf() {
    let tmpdir = tempdir().unwrap();
    let transcript = tmpdir.path().join("transcript.jsonl");
    let (mut p, addr) =
        start_session(&["--crlf", "--transcript", transcript.to_str().unwrap()]).await;
    p.send("crlf\r\n").await.unwrap();
    p.expect("> crlf").await.unwrap();
    p.expect(r#"< You sent: "crlf\r""#).await.unwrap();
    // TODO: Properly assert that the carriage return isn't printed in any form
    // here:
    p.expect("< CR LF:").await.unwrap();
    p.send("quit\r\n").await.unwrap();
    p.expect("> quit").await.unwrap();
    p.expect(r#"< You sent: "quit\r""#).await.unwrap();
    p.expect("< Goodbye.").await.unwrap();
    end_session(p).await;
    check_transcript(
        transcript,
        addr,
        &[
            Msg::Send("crlf\r\n"),
            Msg::Recv("You sent: \"crlf\\r\"\n"),
            Msg::Recv("CR LF:\r\n"),
            Msg::Send("quit\r\n"),
            Msg::Recv("You sent: \"quit\\r\"\n"),
            Msg::Recv("Goodbye.\n"),
        ],
    );
}

#[tokio::test]
async fn test_no_crlf_recv_crlf() {
    let tmpdir = tempdir().unwrap();
    let transcript = tmpdir.path().join("transcript.jsonl");
    let (mut p, addr) = start_session(&["--transcript", transcript.to_str().unwrap()]).await;
    p.send("crlf\r\n").await.unwrap();
    p.expect("> crlf").await.unwrap();
    p.expect("< You sent: \"crlf\"").await.unwrap();
    // TODO: Properly assert that the carriage return isn't printed in any form
    // here:
    p.expect("< CR LF:").await.unwrap();
    p.send("quit\r\n").await.unwrap();
    p.expect("> quit").await.unwrap();
    p.expect(r#"< You sent: "quit""#).await.unwrap();
    p.expect("< Goodbye.").await.unwrap();
    end_session(p).await;
    check_transcript(
        transcript,
        addr,
        &[
            Msg::Send("crlf\n"),
            Msg::Recv("You sent: \"crlf\"\n"),
            Msg::Recv("CR LF:\r\n"),
            Msg::Send("quit\n"),
            Msg::Recv("You sent: \"quit\"\n"),
            Msg::Recv("Goodbye.\n"),
        ],
    );
}
