// engine/src/lib.rs
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
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, mpsc};
use tokio::net::TcpListener;
use notify::RecommendedWatcher;
use mdns_sd::ServiceDaemon;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{description}")]
    Generic { description: String },
}

impl EngineError {
    fn from<E: std::fmt::Display>(e: E) -> Self {
        EngineError::Generic { description: e.to_string() }
    }
}

#[derive(Clone, Debug, Serialize)]
pub enum EngineEvent {
    EngineStarted,
    EngineStopped,
    PeerDiscovered { peer_id: String, addr: String },
    FileIndexed { path: String },
    FileSent { path: String },
    FileDownloaded { path: String },
    ErrorEvent { message: String },
}

pub struct Engine {
    db: Arc<StdMutex<Database>>,
    identity: DeviceIdentity,
    storage_dir: String,
    sync_dir: String,
    ignore_set: IgnoreSet,
    event_tx: mpsc::Sender<EngineEvent>,
    event_rx: StdMutex<mpsc::Receiver<EngineEvent>>,
    runtime: tokio::runtime::Runtime,
    watcher: StdMutex<Option<RecommendedWatcher>>,
    mdns_daemon: StdMutex<Option<ServiceDaemon>>,
    server_task: StdMutex<Option<tokio::task::JoinHandle<()>>>,
    known_peers: Arc<StdMutex<HashMap<String, String>>>,
}

impl Engine {
    pub fn new(base_dir: String, sync_dir_path: String) -> Result<Self, EngineError> {
        std::fs::create_dir_all(&base_dir).map_err(EngineError::from)?;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(EngineError::from)?;

        let identity = DeviceIdentity::load_or_generate(&base_dir).map_err(EngineError::from)?;
        let short_id = &identity.device_id[..8];

        let db_path = format!("{}/local-cloud-{}.db", base_dir, short_id);
        let storage_dir = format!("{}/storage_{}", base_dir, short_id);

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
        let (event_tx, event_rx) = mpsc::channel();
        let known_peers = Arc::new(StdMutex::new(HashMap::new()));

        Ok(Self {
            db: db_state,
            identity,
            storage_dir,
            sync_dir,
            ignore_set,
            event_tx,
            event_rx: StdMutex::new(event_rx),
            runtime,
            watcher: StdMutex::new(None),
            mdns_daemon: StdMutex::new(None),
            server_task: StdMutex::new(None),
            known_peers,
        })
    }

    pub fn device_short_id(&self) -> String {
        self.identity.device_id[..8].to_string()
    }

    pub fn get_sync_dir(&self) -> String {
        self.sync_dir.clone()
    }

    pub fn poll_event(&self, timeout_ms: u64) -> Option<EngineEvent> {
        let timeout = std::time::Duration::from_millis(timeout_ms);
        let rx = self.event_rx.lock().unwrap();
        rx.recv_timeout(timeout).ok()
    }

    pub fn get_known_peers(&self) -> Vec<String> {
        let peers = self.known_peers.lock().unwrap();
        peers.keys().cloned().collect()
    }

    pub fn get_local_files(&self) -> Vec<FileMetadata> {
        let db = self.db.lock().unwrap();
        db.get_all_files().unwrap_or_default()
    }

    pub fn send_file_to_peer(&self, peer_id: String, file_id: String) -> Result<(), EngineError> {
        let peer_url = {
            let peers = self.known_peers.lock().unwrap();
            peers.get(&peer_id).cloned()
        };

        if let Some(url) = peer_url {
            let db = self.db.clone();
            let storage = self.storage_dir.clone();
            let sync = self.sync_dir.clone();
            let cert = self.identity.cert_pem.clone();
            let key = self.identity.key_pem.clone();
            let ignore = self.ignore_set.clone();
            let tx = self.event_tx.clone();
            let pid = peer_id.clone();
            let fid = file_id.clone();

            self.runtime.spawn(async move {
                discovery::push_file_to_peer(url, pid, fid, db, storage, sync, cert, key, ignore, tx).await;
            });
            Ok(())
        } else {
            Err(EngineError::from("Peer not found"))
        }
    }

    pub fn start(&self) -> Result<(), EngineError> {
        let handle = self.runtime.handle().clone();

        // 1. Start Server — bind synchronously to avoid "runtime within runtime" panic.
        //    The CLI may already be running inside a Tokio runtime, and `block_on`
        //    from inside a runtime is forbidden by Tokio.
        let std_listener = std::net::TcpListener::bind("0.0.0.0:0").map_err(EngineError::from)?;
        let port = std_listener.local_addr().map_err(EngineError::from)?.port();
        std_listener.set_nonblocking(true).map_err(EngineError::from)?;
        let listener = TcpListener::from_std(std_listener).map_err(EngineError::from)?;

        let server_db = self.db.clone();
        let server_storage = self.storage_dir.clone();
        let server_sync = self.sync_dir.clone();
        let server_cert = self.identity.cert_pem.clone();
        let server_key = self.identity.key_pem.clone();
        let server_device_id = self.identity.device_id.clone();
        let server_tx = self.event_tx.clone();
        let server_tx_err = self.event_tx.clone();
        let server_ignore = self.ignore_set.clone();

        let server_task = handle.spawn(async move {
            if let Err(e) = server::start_server(
                listener,
                server_device_id,
                server_cert,
                server_key,
                server_db,
                server_storage,
                server_sync,
                server_ignore,
                server_tx,
            ).await {
                let _ = server_tx_err.send(EngineEvent::ErrorEvent {
                    message: format!("Server error: {}", e),
                });
            }
        });
        *self.server_task.lock().unwrap() = Some(server_task);

        // 2. Start Discovery
        let disc_tx = self.event_tx.clone();
        let disc_device_id = self.identity.device_id.clone();
        let disc_peers = self.known_peers.clone();

        let daemon = discovery::start_discovery(
            disc_device_id,
            port,
            disc_tx,
            disc_peers,
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