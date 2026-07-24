// engine/src/server.rs
use crate::db::Database;
use crate::ignore::IgnoreSet;
use crate::tls::TrustedCerts;
use crate::EngineEvent;
use anyhow::Result;
use axum::{
    body::Bytes, extract::Path, extract::State, response::IntoResponse, routing::{get, post}, Json, Router,
};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use rustls::pki_types::CertificateDer;
use rustls::server::danger::ClientCertVerifier;
use rustls::{DistinguishedName, ServerConfig};
use std::sync::{mpsc, Arc, Mutex};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Database>>,
    pub storage_dir: String,
    pub sync_dir: String,
    pub cert_pem: String,
    pub ignore_set: IgnoreSet,
    pub event_tx: mpsc::Sender<EngineEvent>,
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
    sync_dir: String,
    ignore_set: IgnoreSet,
    event_tx: mpsc::Sender<EngineEvent>,
) -> Result<()> {
    let (certs, key) = crate::tls::load_certs_and_key(&cert_pem, &key_pem)?;

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
        sync_dir,
        cert_pem: cert_pem_state,
        ignore_set,
        event_tx,
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
        .route("/list_files", get(list_files))
        .route("/get_block/{block_id}", get(get_block))
        .route("/push_metadata", post(push_metadata))
        .route("/push_block/{block_id}", post(push_block))
        .route("/finalize_file/{file_id}", post(finalize_file))
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

async fn list_files(State(state): State<AppState>) -> Json<Vec<crate::FileMetadata>> {
    let db = state.db.lock().unwrap();
    Json(db.get_all_files().unwrap_or_default())
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

#[derive(serde::Deserialize)]
struct PushMetadataRequest {
    file: crate::FileMetadata,
    blocks: Vec<crate::db::FileBlock>,
}

async fn push_metadata(
    State(state): State<AppState>,
    Json(req): Json<PushMetadataRequest>,
) -> impl IntoResponse {
    let db = state.db.lock().unwrap();

    // Clear old block mappings if we are updating an existing file
    if let Ok(Some(existing)) = db.get_file_by_id(&req.file.id) {
        if req.file.version > existing.version {
            let _ = db.clear_blocks_for_file(&req.file.id);
        }
    }

    if let Err(e) = db.upsert_file_from_peer(&req.file) {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to save metadata: {}", e));
    }
    for b in &req.blocks {
        let block_meta = crate::db::BlockMetadata {
            id: b.block_id.clone(),
            size: b.size,
            is_present: 0, // Default to not present locally
        };
        let _ = db.insert_block(&block_meta);
        let _ = db.map_block_to_file(&req.file.id, &b.block_id, b.block_index);
    }
    (axum::http::StatusCode::OK, String::new())
}

async fn push_block(
    State(state): State<AppState>,
    Path(block_id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    if let Err(e) = crate::storage::write_block(&state.storage_dir, &block_id, &body) {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to write block: {}", e));
    }
    let db = state.db.lock().unwrap();
    if let Err(e) = db.set_block_present(&block_id, true) {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to update DB: {}", e));
    }
    (axum::http::StatusCode::OK, String::new())
}

async fn finalize_file(
    State(state): State<AppState>,
    Path(file_id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.lock().unwrap();

    let file = match db.get_file_by_id(&file_id) {
        Ok(Some(f)) => f,
        _ => return (axum::http::StatusCode::NOT_FOUND, "File not in DB".to_string()),
    };

    let blocks = match db.get_blocks_for_file(&file_id) {
        Ok(b) => b,
        Err(e) => return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)),
    };

    if blocks.iter().all(|b| b.is_present == 1) {
        let output_path = format!("{}/{}", state.sync_dir, file.path);

        if let Some(parent) = std::path::Path::new(&output_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        crate::ignore::mark_ignored(&state.ignore_set, &output_path);

        match crate::storage::assemble_file_from_blocks(&state.storage_dir, &output_path, &blocks) {
            Ok(_) => {
                let _ = state.event_tx.send(EngineEvent::FileDownloaded { path: file.path.clone() });

                crate::ignore::schedule_unmark_ignored(state.ignore_set.clone(), output_path, 3);

                (axum::http::StatusCode::OK, "Assembled".to_string())
            }
            Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("Assemble failed: {}", e)),
        }
    } else {
        (axum::http::StatusCode::BAD_REQUEST, "Blocks missing".to_string())
    }
}