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

use anyhow::Result;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use notify::RecommendedWatcher;
use mdns_sd::ServiceDaemon;

#[derive(Clone, Debug, Serialize)]
pub enum EngineEvent {
    EngineStarted,
    EngineStopped,
    PeerDiscovered { peer_id: String, addr: String },
    FileIndexed { path: String },
    FileDownloaded { path: String },
    FileDeleted { path: String },
    Error { message: String },
}

pub struct Engine {
    db: Arc<Mutex<Database>>,
    identity: DeviceIdentity,
    storage_dir: String,
    sync_dir: String,
    ignore_set: IgnoreSet,
    event_tx: broadcast::Sender<EngineEvent>,
    handle: Handle,
    watcher: Option<RecommendedWatcher>,
    mdns_daemon: Option<ServiceDaemon>,
    server_task: Option<tokio::task::JoinHandle<()>>,
}

impl Engine {
    pub fn new(base_dir: &str, handle: Handle) -> Result<Self> {
        std::fs::create_dir_all(base_dir)?;

        let identity = DeviceIdentity::load_or_generate(base_dir)?;
        let short_id = &identity.device_id[..8];

        let db_path = format!("{}/local-cloud-{}.db", base_dir, short_id);
        let storage_dir = format!("{}/storage_{}", base_dir, short_id);
        let sync_dir_path = format!("{}/sync_{}", base_dir, short_id);

        let database = Database::init(&db_path)?;
        storage::ensure_storage_dir(&storage_dir)?;
        storage::ensure_trusted_peers_dir(&storage_dir)?;
        std::fs::create_dir_all(&sync_dir_path)?;

        // Canonicalize sync dir so path comparisons with OS events work perfectly
        let sync_dir = std::fs::canonicalize(&sync_dir_path)?
            .to_string_lossy()
            .to_string();

        let db_state = Arc::new(Mutex::new(database));
        let ignore_set = new_ignore_set();
        let (event_tx, _) = broadcast::channel(100);

        Ok(Self {
            db: db_state,
            identity,
            storage_dir,
            sync_dir,
            ignore_set,
            event_tx,
            handle,
            watcher: None,
            mdns_daemon: None,
            server_task: None,
        })
    }

    pub fn device_short_id(&self) -> &str {
        &self.identity.device_id[..8]
    }

    pub fn get_sync_dir(&self) -> &str {
        &self.sync_dir
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.event_tx.subscribe()
    }

    pub async fn start(&mut self) -> Result<()> {
        // 1. Start Server
        let listener = TcpListener::bind("0.0.0.0:0").await?;
        let port = listener.local_addr()?.port();

        let server_db = self.db.clone();
        let server_storage = self.storage_dir.clone();
        let server_cert = self.identity.cert_pem.clone();
        let server_key = self.identity.key_pem.clone();
        let server_device_id = self.identity.device_id.clone();
        let server_tx = self.event_tx.clone();

        self.server_task = Some(tokio::spawn(async move {
            if let Err(e) = server::start_server(
                listener,
                server_device_id,
                server_cert,
                server_key,
                server_db,
                server_storage,
            ).await {
                let _ = server_tx.send(EngineEvent::Error { message: format!("Server error: {}", e) });
            }
        }));

        // 2. Start Discovery
        let disc_db = self.db.clone();
        let disc_storage = self.storage_dir.clone();
        let disc_sync = self.sync_dir.clone();
        let disc_cert = self.identity.cert_pem.clone();
        let disc_key = self.identity.key_pem.clone();
        let disc_handle = self.handle.clone();
        let disc_tx = self.event_tx.clone();
        let disc_device_id = self.identity.device_id.clone();
        let disc_ignore = self.ignore_set.clone();

        self.mdns_daemon = Some(discovery::start_discovery(
            disc_device_id,
            port,
            disc_handle,
            disc_db,
            disc_storage,
            disc_sync,
            disc_cert,
            disc_key,
            disc_ignore,
            disc_tx,
        )?);

        // 3. Start Watcher
        let watch_db = self.db.clone();
        let watch_storage = self.storage_dir.clone();
        let watch_sync = self.sync_dir.clone();
        let watch_device_id = self.identity.device_id.clone();
        let watch_handle = self.handle.clone();
        let watch_ignore = self.ignore_set.clone();
        let watch_tx = self.event_tx.clone();

        self.watcher = Some(watcher::start_watcher(
            watch_sync,
            watch_storage,
            watch_device_id,
            watch_db,
            watch_handle,
            watch_ignore,
            watch_tx,
        )?);

        let _ = self.event_tx.send(EngineEvent::EngineStarted);
        println!("[Engine] Started. Sync folder: {}", self.sync_dir);
        Ok(())
    }

    pub fn stop(&mut self) {
        self.watcher.take(); // Dropping the watcher stops it
        if let Some(daemon) = self.mdns_daemon.take() {
            let _ = daemon.shutdown();
        }
        if let Some(handle) = self.server_task.take() {
            handle.abort();
        }
        let _ = self.event_tx.send(EngineEvent::EngineStopped);
        println!("[Engine] Stopped.");
    }
}