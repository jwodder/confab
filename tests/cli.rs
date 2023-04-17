// <https://github.com/zhiburt/expectrl/issues/52>
#![cfg(unix)]
use expectrl::session::{log, OsProcess, OsProcessStream, Session};
use expectrl::stream::log::LogStream;
use expectrl::{ControlCode, Eof, Regex};
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::process::Command;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::oneshot::{channel, Sender};
use tokio::time::sleep;
use tokio_util::codec::{Framed, LinesCodec};

#[cfg(unix)]
use expectrl::WaitStatus;

type ExpectrlSession = Session<OsProcess, LogStream<OsProcessStream, std::io::Stdout>>;

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
    let mut frame = Framed::new(socket, LinesCodec::new_with_max_length(65535));
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
                    frame.send(format!("You sent: {line:?}")).await.unwrap();
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
                    }
                }
                Some(Err(e)) => panic!("Error reading from connection: {e}"),
                None => break,
            }
        }
    }
}

async fn start_session(opts: &[&str]) -> ExpectrlSession {
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
    p
}

async fn end_session(mut p: ExpectrlSession) {
    p.expect("* Disconnected").await.unwrap();
    p.expect(Eof).await.unwrap();
    #[cfg(unix)]
    assert_eq!(p.wait().unwrap(), WaitStatus::Exited(p.pid(), 0));
    #[cfg(windows)]
    assert_eq!(p.wait(None).unwrap(), 0);
}

#[tokio::test]
async fn test_quit_session() {
    let mut p = start_session(&[]).await;
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
    let mut p = start_session(&[]).await;
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
    let mut p = start_session(&[]).await;
    p.send("Hello!\r\n").await.unwrap();
    p.expect("> Hello!").await.unwrap();
    p.expect(r#"< You sent: "Hello!""#).await.unwrap();
    p.send(ControlCode::EndOfTransmission).await.unwrap();
    end_session(p).await;
}

#[tokio::test]
async fn test_show_times() {
    static TIME_RGX: &str = r#"\[[0-9]{2}:[0-9]{2}:[0-9]{2}\]"#;
    let mut p = start_session(&["--show-times"]).await;
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
    let mut p = start_session(&[]).await;
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
}

#[tokio::test]
async fn test_long_line() {
    let mut p = start_session(&["--max-line-length", "42"]).await;
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
}
