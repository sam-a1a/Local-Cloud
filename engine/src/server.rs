use anyhow::Result;
use axum::{routing::get, Router};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tower::ServiceExt;

pub async fn start_server(identity: crate::crypto::DeviceIdentity) -> Result<()> {
    let certs: Vec<CertificateDer<'static>> =
        CertificateDer::pem_slice_iter(identity.cert_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()?;

    let key = PrivateKeyDer::from_pem_slice(identity.key_pem.as_bytes())?;

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;

    let acceptor = TlsAcceptor::from(Arc::new(config));
    let device_id = identity.device_id.clone();

    let app = Router::new()
        .route("/ping", get(move || async move { format!("pong: {}", device_id) }));

    let listener = TcpListener::bind("0.0.0.0:8080").await?;

    loop {
        let (stream, _addr) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let app = app.clone();

        tokio::spawn(async move {
            let tls_stream = match acceptor.accept(stream).await {
                Ok(s) => s,
                Err(_) => return,
            };

            let io = TokioIo::new(tls_stream);
            let _ = Builder::new(TokioExecutor::new())
                .serve_connection(io, hyper::service::service_fn(move |req| {
                    let app = app.clone();
                    async move { app.oneshot(req).await }
                }))
                .await;
        });
    }
}