pub mod crypto;
pub mod db;
pub mod discovery;
pub mod ignore;
pub mod server;
pub mod storage;
pub mod tls;
pub mod watcher;

pub use db::Database;
pub use db::FileMetadata;
pub use db::BlockMetadata;
pub use db::FileBlock;
pub use db::Tombstone;
pub use crypto::DeviceIdentity;
pub use ignore::{new_ignore_set, IgnoreSet};

use serde::Serialize;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Mutex as TokioMutex};
use notify::RecommendedWatcher;
use mdns_sd::ServiceDaemon;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum EngineError {
    #[error("{description}")]
    Generic { description: String },
}

impl EngineError {
    fn from<E: std::fmt::Display>(e: E) -> Self {
        EngineError::Generic { description: e.to_string() }
    }
}

#[derive(Clone, Debug, Serialize, uniffi::Enum)]
pub enum EngineEvent {
    EngineStarted,
    EngineStopped,
    PeerDiscovered { peer_id: String, addr: String },
    FileIndexed { path: String },
    FileDownloaded { path: String },
    FileDeleted { path: String },
    ErrorEvent { message: String },
}

#[derive(uniffi::Object)]
pub struct Engine {
    db: Arc<StdMutex<Database>>,
    identity: DeviceIdentity,
    storage_dir: String,
    sync_dir: String,
    ignore_set: IgnoreSet,
    event_tx: broadcast::Sender<EngineEvent>,
    event_rx: TokioMutex<broadcast::Receiver<EngineEvent>>,
    runtime: tokio::runtime::Runtime,
    watcher: StdMutex<Option<RecommendedWatcher>>,
    mdns_daemon: StdMutex<Option<ServiceDaemon>>,
    server_task: StdMutex<Option<tokio::task::JoinHandle<()>>>,
}

#[uniffi::export]
impl Engine {
    #[uniffi::constructor]
    pub fn new(base_dir: String) -> Result<Self, EngineError> {
        std::fs::create_dir_all(&base_dir).map_err(EngineError::from)?;

        // Spin up a dedicated Tokio runtime for the engine
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(EngineError::from)?;

        let identity = DeviceIdentity::load_or_generate(&base_dir).map_err(EngineError::from)?;
        let short_id = &identity.device_id[..8];

        let db_path = format!("{}/local-cloud-{}.db", base_dir, short_id);
        let storage_dir = format!("{}/storage_{}", base_dir, short_id);
        let sync_dir_path = format!("{}/sync_{}", base_dir, short_id);

        let database = Database::init(&db_path).map_err(EngineError::from)?;
        storage::ensure_storage_dir(&storage_dir).map_err(EngineError::from)?;
        storage::ensure_trusted_peers_dir(&storage_dir).map_err(EngineError::from)?;
        std::fs::create_dir_all(&sync_dir_path).map_err(EngineError::from)?;

        let sync_dir = std::fs::canonicalize(&sync_dir_path)
            .map_err(EngineError::from)?
            .to_string_lossy()
            .to_string();

        let db_state = Arc::new(StdMutex::new(database));
        let ignore_set = new_ignore_set();
        let (event_tx, event_rx) = broadcast::channel(100);

        Ok(Self {
            db: db_state,
            identity,
            storage_dir,
            sync_dir,
            ignore_set,
            event_tx,
            event_rx: TokioMutex::new(event_rx),
            runtime,
            watcher: StdMutex::new(None),
            mdns_daemon: StdMutex::new(None),
            server_task: StdMutex::new(None),
        })
    }

    pub fn device_short_id(&self) -> String {
        self.identity.device_id[..8].to_string()
    }

    pub fn get_sync_dir(&self) -> String {
        self.sync_dir.clone()
    }

    pub async fn next_event(&self) -> EngineEvent {
        let mut rx = self.event_rx.lock().await;
        rx.recv().await.unwrap_or(EngineEvent::ErrorEvent { message: "Event channel closed".to_string() })
    }

    pub async fn start(&self) -> Result<(), EngineError> {
        let handle = self.runtime.handle().clone();

        // 1. Start Server
        let listener = TcpListener::bind("0.0.0.0:0").await.map_err(EngineError::from)?;
        let port = listener.local_addr().map_err(EngineError::from)?.port();

        let server_db = self.db.clone();
        let server_storage = self.storage_dir.clone();
        let server_cert = self.identity.cert_pem.clone();
        let server_key = self.identity.key_pem.clone();
        let server_device_id = self.identity.device_id.clone();
        let server_tx = self.event_tx.clone();

        let server_task = handle.spawn(async move {
            if let Err(e) = server::start_server(
                listener,
                server_device_id,
                server_cert,
                server_key,
                server_db,
                server_storage,
            ).await {
                let _ = server_tx.send(EngineEvent::ErrorEvent { message: format!("Server error: {}", e) });
            }
        });
        *self.server_task.lock().unwrap() = Some(server_task);

        // 2. Start Discovery
        let disc_db = self.db.clone();
        let disc_storage = self.storage_dir.clone();
        let disc_sync = self.sync_dir.clone();
        let disc_cert = self.identity.cert_pem.clone();
        let disc_key = self.identity.key_pem.clone();
        let disc_tx = self.event_tx.clone();
        let disc_device_id = self.identity.device_id.clone();
        let disc_ignore = self.ignore_set.clone();

        let daemon = discovery::start_discovery(
            disc_device_id,
            port,
            handle.clone(),
            disc_db,
            disc_storage,
            disc_sync,
            disc_cert,
            disc_key,
            disc_ignore,
            disc_tx,
        ).map_err(EngineError::from)?;

        *self.mdns_daemon.lock().unwrap() = Some(daemon);

        // 3. Start Watcher
        let watch_db = self.db.clone();
        let watch_storage = self.storage_dir.clone();
        let watch_sync = self.sync_dir.clone();
        let watch_device_id = self.identity.device_id.clone();
        let watch_ignore = self.ignore_set.clone();
        let watch_tx = self.event_tx.clone();

        let watcher = watcher::start_watcher(
            watch_sync,
            watch_storage,
            watch_device_id,
            watch_db,
            handle,
            watch_ignore,
            watch_tx,
        ).map_err(EngineError::from)?;

        *self.watcher.lock().unwrap() = Some(watcher);

        let _ = self.event_tx.send(EngineEvent::EngineStarted);
        println!("[Engine] Started. Sync folder: {}", self.sync_dir);
        Ok(())
    }

    pub fn stop(&self) {
        if let Some(w) = self.watcher.lock().unwrap().take() {
            drop(w);
        }
        if let Some(d) = self.mdns_daemon.lock().unwrap().take() {
            let _ = d.shutdown();
        }
        if let Some(t) = self.server_task.lock().unwrap().take() {
            t.abort();
        }
        let _ = self.event_tx.send(EngineEvent::EngineStopped);
        println!("[Engine] Stopped.");
    }
}

uniffi::setup_scaffolding!();