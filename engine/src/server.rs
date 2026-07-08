use anyhow::Result;
use axum::{
    body::Bytes, extract::Path, extract::State, response::IntoResponse, routing::get, Json, Router,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::danger::ClientCertVerifier;
use rustls::{DistinguishedName, ServerConfig};
use rustls_pemfile;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tower::ServiceExt;
use crate::db::Database;
use crate::tls::TrustedCerts;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Database>>,
    pub storage_dir: String,
    pub cert_pem: String,
}

#[derive(Debug)]
struct TrustedPeerVerifier {
    certs: TrustedCerts,
}

impl TrustedPeerVerifier {
    fn new(trusted_cert_pems: &[String]) -> Self {
        Self {
            certs: TrustedCerts::new(trusted_cert_pems),
        }
    }
}

impl ClientCertVerifier for TrustedPeerVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        if self.certs.is_trusted(end_entity) {
            Ok(rustls::server::danger::ClientCertVerified::assertion())
        } else {
            Err(rustls::Error::General("Certificate not in trusted peers list".into()))
        }
    }

    crate::impl_tls_verifier_methods!();
}

pub async fn start_server(
    listener: TcpListener,
    device_id: String,
    cert_pem: String,
    key_pem: String,
    db: Arc<Mutex<Database>>,
    storage_dir: String,
) -> Result<()> {
    let mut cert_cursor = Cursor::new(cert_pem.as_bytes());
    let mut key_cursor = Cursor::new(key_pem.as_bytes());

    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_cursor)
        .collect::<Result<Vec<_>, _>>()?;

    let key = PrivateKeyDer::from(rustls_pemfile::private_key(&mut key_cursor)?.unwrap());

    let trusted_certs = crate::storage::load_all_trusted_certs(&storage_dir)?;

    let config = if trusted_certs.is_empty() {
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?
    } else {
        let verifier = Arc::new(TrustedPeerVerifier::new(&trusted_certs));
        ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(certs, key)?
    };

    let acceptor = TlsAcceptor::from(Arc::new(config));
    let cert_pem_state = cert_pem.clone();

    let state = AppState {
        db,
        storage_dir,
        cert_pem: cert_pem_state,
    };

    let app = Router::new()
        .route("/ping", get({
            let id = device_id.clone();
            move || {
                let id = id.clone();
                async move { format!("pong: {}", id) }
            }
        }))
        .route("/hello", get(get_hello))
        .route("/metadata", get(get_metadata))
        .route("/tombstones", get(get_tombstones))
        .route("/block/{block_id}", get(get_block))
        .route("/file_blocks/{file_id}", get(get_file_blocks))
        .with_state(state);

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

async fn get_hello(State(state): State<AppState>) -> String {
    state.cert_pem.clone()
}

async fn get_metadata(State(state): State<AppState>) -> Json<Vec<crate::FileMetadata>> {
    let db = state.db.lock().unwrap();
    let files = db.get_all_files().unwrap_or_default();
    Json(files)
}

async fn get_tombstones(State(state): State<AppState>) -> Json<Vec<crate::db::Tombstone>> {
    let db = state.db.lock().unwrap();
    let tombstones = db.get_all_tombstones().unwrap_or_default();
    Json(tombstones)
}

async fn get_block(
    State(state): State<AppState>,
    Path(block_id): Path<String>,
) -> impl IntoResponse {
    match crate::storage::read_block(&state.storage_dir, &block_id) {
        Ok(data) => Bytes::from(data).into_response(),
        Err(_) => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

async fn get_file_blocks(
    State(state): State<AppState>,
    Path(file_id): Path<String>,
) -> Json<Vec<crate::db::FileBlock>> {
    let db = state.db.lock().unwrap();
    let blocks = db.get_blocks_for_file(&file_id).unwrap_or_default();
    Json(blocks)
}