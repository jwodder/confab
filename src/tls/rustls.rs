use itertools::Itertools; // join
use rustls_pki_types::{InvalidDnsNameError, ServerName};
use std::io;
use std::sync::Arc;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio_rustls::{
    rustls::{ClientConfig, RootCertStore},
    TlsConnector,
};

pub(crate) type TlsStream = tokio_rustls::client::TlsStream<TcpStream>;

#[derive(Debug, Error)]
pub(crate) enum TlsError {
    #[error("failed to load system certificates: {0}")]
    LoadStore(String),
    #[error("failed to add certificates from system store: all {bad} certs were invalid")]
    AddCerts { bad: usize },
    #[error("invalid TLS server name")]
    ServerName(#[from] InvalidDnsNameError),
    #[error("failed to establish TLS connection")]
    Connect(#[source] io::Error),
}

pub(crate) async fn connect(conn: TcpStream, servername: &str) -> Result<TlsStream, TlsError> {
    let certs = rustls_native_certs::load_native_certs();
    if !certs.errors.is_empty() {
        let msg = certs.errors.into_iter().map(|e| e.to_string()).join("; ");
        return Err(TlsError::LoadStore(msg));
    }
    let mut root_cert_store = RootCertStore::empty();
    let (good, bad) = root_cert_store.add_parsable_certificates(certs.certs);
    if good == 0 {
        return Err(TlsError::AddCerts { bad });
    }
    let config = ClientConfig::builder()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();
    // Note to self: To make use of client certs, replace
    // with_no_client_auth() with with_client_auth_cert(...).
    let connector = TlsConnector::from(Arc::new(config));
    let dnsname = ServerName::try_from(servername)?.to_owned();
    connector
        .connect(dnsname, conn)
        .await
        .map_err(TlsError::Connect)
}
