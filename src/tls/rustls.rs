use std::io;
use std::sync::Arc;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio_rustls::rustls::{client::InvalidDnsNameError, ClientConfig, RootCertStore, ServerName};
use tokio_rustls::TlsConnector;

pub(crate) type TlsStream = tokio_rustls::client::TlsStream<TcpStream>;

#[derive(Debug, Error)]
pub(crate) enum TlsError {
    #[error("failed to load system certificate store")]
    LoadStore(#[source] io::Error),
    #[error("failed to add certificates from system store: all {bad} certs were invalid")]
    AddCerts { bad: usize },
    #[error("invalid TLS server name")]
    ServerName(#[from] InvalidDnsNameError),
    #[error("failed to establish TLS connection")]
    Connect(#[source] io::Error),
}

pub(crate) async fn connect(conn: TcpStream, servername: &str) -> Result<TlsStream, TlsError> {
    let mut root_cert_store = RootCertStore::empty();
    let system_certs = rustls_native_certs::load_native_certs()
        .map_err(TlsError::LoadStore)?
        .into_iter()
        .map(|cert| cert.to_vec())
        .collect::<Vec<Vec<u8>>>();
    let (good, bad) = root_cert_store.add_parsable_certificates(&system_certs);
    if good == 0 {
        return Err(TlsError::AddCerts { bad });
    }
    let config = ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();
    // Note to self: To make use of client certs, replace
    // with_no_client_auth() with with_single_cert(...).
    let connector = TlsConnector::from(Arc::new(config));
    let dnsname = ServerName::try_from(servername)?;
    connector
        .connect(dnsname, conn)
        .await
        .map_err(TlsError::Connect)
}
