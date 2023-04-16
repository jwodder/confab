use anyhow::Context;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_rustls::rustls::{ClientConfig, OwnedTrustAnchor, RootCertStore, ServerName};
use tokio_rustls::TlsConnector;

pub(crate) type TlsStream = tokio_rustls::client::TlsStream<TcpStream>;

pub(crate) async fn connect(conn: TcpStream, servername: &str) -> anyhow::Result<TlsStream> {
    let mut root_cert_store = RootCertStore::empty();
    root_cert_store.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
        OwnedTrustAnchor::from_subject_spki_name_constraints(
            ta.subject,
            ta.spki,
            ta.name_constraints,
        )
    }));
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
