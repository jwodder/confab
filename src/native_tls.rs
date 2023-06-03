use anyhow::Context;
use tokio::net::TcpStream;

pub(crate) type TlsStream = tokio_native_tls::TlsStream<TcpStream>;

pub(crate) async fn connect(conn: TcpStream, servername: &str) -> anyhow::Result<TlsStream> {
    tokio_native_tls::TlsConnector::from(
        native_tls::TlsConnector::new().context("Error creating TLS connector")?,
    )
    .connect(servername, conn)
    .await
    .context("Error establishing TLS connection")
}
