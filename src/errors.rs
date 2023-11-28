use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum IoError {
    #[error(transparent)]
    Interface(#[from] InterfaceError),
    #[error(transparent)]
    Inet(#[from] InetError),
}

#[derive(Debug, Error)]
pub(crate) enum InterfaceError {
    #[error("failed to initialize readline facility")]
    Init(#[source] rustyline_async::ReadlineError),
    #[error("error reading from startup script")]
    ReadScript(#[source] io::Error),
    #[error("error reading input from terminal")]
    ReadLine(#[source] io::Error),
    #[error("error writing output")]
    Write(#[source] io::Error),
}

#[derive(Debug, Error)]
pub(crate) enum InetError {
    #[error("failed to connect to server")]
    Connect(#[source] io::Error),
    #[error("failed to get peer address")]
    PeerAddr(#[source] io::Error),
    #[error("failed to establish TLS connection")]
    Tls(#[from] crate::tls::TlsError),
    #[error("failed to send line to server")]
    Send(#[source] io::Error),
    #[error("failed to receive line from server")]
    Recv(#[source] io::Error),
}
