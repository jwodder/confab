use anyhow::{bail, Context};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_rustls::rustls::{ClientConfig, RootCertStore, ServerName};
use tokio_rustls::TlsConnector;

pub(crate) type TlsStream = tokio_rustls::client::TlsStream<TcpStream>;

pub(crate) async fn connect(conn: TcpStream, servername: &str) -> anyhow::Result<TlsStream> {
    let mut root_cert_store = RootCertStore::empty();
    let system_certs = rustls_native_certs::load_native_certs()
        .context("Failed to load system certificate store")?
        .into_iter()
        .map(|cert| cert.0)
        .collect::<Vec<_>>();
    let (good, bad) = root_cert_store.add_parsable_certificates(&system_certs);
    if good == 0 {
        bail!("Failed to load any certificates from system store: all {bad} certs were invalid");
    }
    let config = ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();
    // Note to self: To make use of client certs, replace
    // with_no_client_auth() with with_single_cert(...).
    let connector = TlsConnector::from(Arc::new(config));
    let dnsname = ServerName::try_from(servername).context("Invalid TLS server name")?;
    connector
        .connect(dnsname, conn)
        .await
        .context("Error establishing TLS connection")
}
