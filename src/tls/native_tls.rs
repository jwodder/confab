use thiserror::Error;
use tokio::net::TcpStream;

pub(crate) type TlsStream = tokio_native_tls::TlsStream<TcpStream>;

#[derive(Debug, Error)]
pub(crate) enum TlsError {
    #[error("failed to create TLS connector")]
    Connector(#[source] tokio_native_tls::native_tls::Error),
    #[error("failed to establish TLS connection")]
    Connect(#[source] tokio_native_tls::native_tls::Error),
}

pub(crate) async fn connect(conn: TcpStream, servername: &str) -> Result<TlsStream, TlsError> {
    tokio_native_tls::TlsConnector::from(
        tokio_native_tls::native_tls::TlsConnector::new().map_err(TlsError::Connector)?,
    )
    .connect(servername, conn)
    .await
    .map_err(TlsError::Connect)
}
