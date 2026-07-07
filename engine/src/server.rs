use anyhow::Result;
use axum::{routing::get, Router, Json, extract::State, extract::Path, response::IntoResponse, body::Bytes};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tower::ServiceExt;
use crate::db::Database;

#[derive(Clone)]
struct AppState {
    db: Arc<Mutex<Database>>,
    storage_dir: String,
}

pub async fn start_server(listener: TcpListener, identity: crate::crypto::DeviceIdentity, db: Arc<Mutex<Database>>, storage_dir: String) -> Result<()> {
    let mut cert_cursor = Cursor::new(identity.cert_pem.as_bytes());
    let mut key_cursor = Cursor::new(identity.key_pem.as_bytes());

    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_cursor)
        .collect::<Result<Vec<_>, _>>()?;

    let key = PrivateKeyDer::from(rustls_pemfile::private_key(&mut key_cursor)?.unwrap());

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;

    let acceptor = TlsAcceptor::from(Arc::new(config));
    let device_id = identity.device_id.clone();

    let state = AppState { db, storage_dir };

    let app = Router::new()
        .route("/ping", get(move || async move { format!("pong: {}", device_id) }))
        .route("/metadata", get(get_metadata))
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

async fn get_metadata(State(state): State<AppState>) -> Json<Vec<crate::FileMetadata>> {
    let db = state.db.lock().unwrap();
    let files = db.get_all_files().unwrap_or_default();
    Json(files)
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