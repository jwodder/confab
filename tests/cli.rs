// <https://github.com/zhiburt/expectrl/issues/52>
#![cfg(unix)]
use expectrl::session::{log, Session};
use expectrl::{ControlCode, Eof};
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::process::Command;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot::{channel, Sender};
use tokio::time::sleep;
use tokio_util::codec::{Framed, LinesCodec};

#[cfg(unix)]
use expectrl::WaitStatus;

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
    let (sender, receiver) = channel();
    tokio::spawn(async move { testing_server(sender).await });
    let addr = receiver.await.expect("Error receiving address from server");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_confab"));
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
    p.send("Hello!\r\n").await.unwrap();
    p.expect(r#"< You sent: "Hello!""#).await.unwrap();
    p.send("quit\r\n").await.unwrap();
    p.expect("> quit").await.unwrap();
    p.expect(r#"< You sent: "quit""#).await.unwrap();
    p.expect("< Goodbye.").await.unwrap();
    p.expect("* Disconnected").await.unwrap();
    p.expect(Eof).await.unwrap();
    #[cfg(unix)]
    assert_eq!(p.wait().unwrap(), WaitStatus::Exited(p.pid(), 0));
    #[cfg(windows)]
    assert_eq!(p.wait(None).unwrap(), 0);
}

#[tokio::test]
async fn test_async_recv() {
    let (sender, receiver) = channel();
    tokio::spawn(async move { testing_server(sender).await });
    let addr = receiver.await.expect("Error receiving address from server");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_confab"));
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
    sleep(Duration::from_secs(1)).await;
    p.expect("< Ping 1").await.unwrap();
    sleep(Duration::from_secs(1)).await;
    p.expect("< Ping 2").await.unwrap();
    p.send("quit\r\n").await.unwrap();
    p.expect("> quit").await.unwrap();
    p.expect(r#"< You sent: "quit""#).await.unwrap();
    p.expect("< Goodbye.").await.unwrap();
    p.expect("* Disconnected").await.unwrap();
    p.expect(Eof).await.unwrap();
    #[cfg(unix)]
    assert_eq!(p.wait().unwrap(), WaitStatus::Exited(p.pid(), 0));
    #[cfg(windows)]
    assert_eq!(p.wait(None).unwrap(), 0);
}

#[tokio::test]
async fn test_send_ctrl_d() {
    let (sender, receiver) = channel();
    tokio::spawn(async move { testing_server(sender).await });
    let addr = receiver.await.expect("Error receiving address from server");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_confab"));
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
    p.send("Hello!\r\n").await.unwrap();
    p.expect(r#"< You sent: "Hello!""#).await.unwrap();
    p.send(ControlCode::EndOfTransmission).await.unwrap();
    p.expect("* Disconnected").await.unwrap();
    p.expect(Eof).await.unwrap();
    #[cfg(unix)]
    assert_eq!(p.wait().unwrap(), WaitStatus::Exited(p.pid(), 0));
    #[cfg(windows)]
    assert_eq!(p.wait(None).unwrap(), 0);
}
