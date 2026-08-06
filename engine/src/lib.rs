// engine/src/lib.rs
pub mod crypto;
pub mod db;
pub mod discovery;
pub mod ignore;
pub mod pairing;
pub mod server;
pub mod storage;
pub mod tls;
pub mod watcher;

pub use db::Database;
pub use db::FileMetadata;
pub use db::BlockMetadata;
pub use db::FileBlock;
pub use db::PairedDevice;
pub use db::Tombstone;
pub use pairing::{PairingOffer, PairingState};
pub use crypto::DeviceIdentity;
pub use discovery::{DiscoveredDevice, PeerMap};
pub use ignore::{new_ignore_set, IgnoreSet};
pub use tls::TrustStore;

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
    PeerDiscovered { device: DiscoveredDevice },
    PairingRequested { device_id: String, name: String, platform: String },
    DevicePaired { device_id: String, name: String },
    PairingFailed { device_id: String, reason: String },
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
    known_peers: PeerMap,
    trust: TrustStore,
    pairing: PairingState,
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

        let trust = TrustStore::new();
        trust.reload(&storage_dir).map_err(EngineError::from)?;

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
            trust,
            pairing: PairingState::new(),
        })
    }

    pub fn device_short_id(&self) -> String {
        self.identity.device_id[..8].to_string()
    }

    pub fn device_name(&self) -> String {
        self.identity.device_name.clone()
    }

    pub fn device_platform(&self) -> String {
        crypto::platform_name().to_string()
    }

    // ---- Pairing ----

    fn own_device_info(&self) -> pairing::DeviceInfo {
        pairing::DeviceInfo {
            device_id: self.identity.device_id.clone(),
            name: self.identity.device_name.clone(),
            platform: crypto::platform_name().to_string(),
            cert_pem: self.identity.cert_pem.clone(),
        }
    }

    /// Initiator: pick devices from the discovery list to pair with.
    ///
    /// Returns the 6-digit code to put on screen. Each selected device is asked
    /// to prompt for it; pairing completes as each one enters it correctly.
    pub fn start_pairing(&self, target_device_ids: Vec<String>) -> Result<String, EngineError> {
        if target_device_ids.is_empty() {
            return Err(EngineError::from("Select at least one device to pair with"));
        }

        let targets: Vec<(String, String)> = {
            let peers = self.known_peers.lock().unwrap();
            target_device_ids
                .iter()
                .filter_map(|id| peers.get(id).map(|d| (id.clone(), d.url.clone())))
                .collect()
        };

        if targets.is_empty() {
            return Err(EngineError::from(
                "None of those devices are visible on the network",
            ));
        }

        let code = self
            .pairing
            .begin(targets.iter().map(|(id, _)| id.clone()).collect());

        let me = self.own_device_info();
        let tx = self.event_tx.clone();

        self.runtime.spawn(async move {
            let client = match pairing::build_pairing_client() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(EngineEvent::ErrorEvent {
                        message: format!("Could not start pairing: {}", e),
                    });
                    return;
                }
            };

            for (device_id, url) in targets {
                if let Err(e) = pairing::send_pair_request(&client, &url, &me).await {
                    let _ = tx.send(EngineEvent::PairingFailed {
                        device_id,
                        reason: format!("Could not reach device: {}", e),
                    });
                }
            }
        });

        Ok(code)
    }

    /// The code currently displayed, if one is still valid.
    pub fn pairing_code(&self) -> Option<String> {
        self.pairing.active_code()
    }

    /// Selected devices that have not yet entered the code.
    pub fn devices_awaiting_code(&self) -> Vec<String> {
        self.pairing.awaiting()
    }

    pub fn cancel_pairing(&self) {
        self.pairing.cancel();
    }

    /// Target: devices asking to pair, awaiting a code from the user.
    pub fn pairing_offers(&self) -> Vec<PairingOffer> {
        self.pairing.offers()
    }

    /// Target: submit the code the initiator is displaying.
    ///
    /// Runs in the background; the outcome arrives as `DevicePaired` or
    /// `PairingFailed` rather than being returned, so this is safe to call from
    /// inside another async runtime.
    pub fn confirm_pairing(
        &self,
        initiator_device_id: String,
        code: String,
    ) -> Result<(), EngineError> {
        let initiator = self
            .pairing
            .offer_details(&initiator_device_id)
            .ok_or_else(|| EngineError::from("No pending pairing request from that device"))?;

        let url = {
            let peers = self.known_peers.lock().unwrap();
            peers
                .get(&initiator_device_id)
                .map(|d| d.url.clone())
                .ok_or_else(|| {
                    EngineError::from("That device is no longer visible on the network")
                })?
        };

        let me = self.own_device_info();
        // Order matters: the initiator hashes its own certificate first.
        let proof = pairing::pairing_proof(&code, &initiator.cert_pem, &me.cert_pem);

        let db = self.db.clone();
        let storage_dir = self.storage_dir.clone();
        let trust = self.trust.clone();
        let pairing_state = self.pairing.clone();
        let tx = self.event_tx.clone();

        self.runtime.spawn(async move {
            let client = match pairing::build_pairing_client() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(EngineEvent::PairingFailed {
                        device_id: initiator_device_id,
                        reason: e.to_string(),
                    });
                    return;
                }
            };

            let peer = match pairing::send_pair_confirm(&client, &url, &me, &proof).await {
                Ok(info) => info,
                Err(e) => {
                    let _ = tx.send(EngineEvent::PairingFailed {
                        device_id: initiator_device_id,
                        reason: e.to_string(),
                    });
                    return;
                }
            };

            // The initiator accepted, so pin it in return.
            if let Err(e) = server::pin_paired_device(&db, &storage_dir, &trust, &peer) {
                let _ = tx.send(EngineEvent::PairingFailed {
                    device_id: initiator_device_id,
                    reason: format!("Paired, but failed to record it locally: {}", e),
                });
                return;
            }

            pairing_state.clear_offer(&initiator_device_id);
            let _ = tx.send(EngineEvent::DevicePaired {
                device_id: peer.device_id,
                name: peer.name,
            });
        });

        Ok(())
    }

    pub fn paired_devices(&self) -> Vec<PairedDevice> {
        let db = self.db.lock().unwrap();
        db.get_paired_devices().unwrap_or_default()
    }

    /// Revokes a pairing. The device stays visible on the network but loses all
    /// catalog and block access immediately.
    pub fn unpair(&self, device_id: String) -> Result<(), EngineError> {
        {
            let db = self.db.lock().unwrap();
            db.remove_paired_device(&device_id).map_err(EngineError::from)?;
        }
        storage::remove_peer_cert(&self.storage_dir, &device_id).map_err(EngineError::from)?;
        self.trust.reload(&self.storage_dir).map_err(EngineError::from)?;
        Ok(())
    }

    pub fn get_sync_dir(&self) -> String {
        self.sync_dir.clone()
    }

    pub fn poll_event(&self, timeout_ms: u64) -> Option<EngineEvent> {
        let timeout = std::time::Duration::from_millis(timeout_ms);
        let rx = self.event_rx.lock().unwrap();
        rx.recv_timeout(timeout).ok()
    }

    /// Every device currently visible on the network, paired or not.
    pub fn get_known_peers(&self) -> Vec<DiscoveredDevice> {
        let peers = self.known_peers.lock().unwrap();
        let mut devices: Vec<DiscoveredDevice> = peers.values().cloned().collect();
        devices.sort_by(|a, b| a.name.cmp(&b.name));
        devices
    }

    pub fn get_local_files(&self) -> Vec<FileMetadata> {
        let db = self.db.lock().unwrap();
        db.get_all_files().unwrap_or_default()
    }

    pub fn sync_with_peer(&self, peer_id: String, peer_url: String) -> Result<(), EngineError> {
        let db = self.db.clone();
        let storage = self.storage_dir.clone();
        let cert = self.identity.cert_pem.clone();
        let key = self.identity.key_pem.clone();
        let my_id = self.identity.device_id.clone();
        let tx = self.event_tx.clone();

        self.runtime.spawn(async move {
            discovery::sync_with_peer(peer_url, peer_id, my_id, db, storage, cert, key, tx).await;
        });
        Ok(())
    }

    pub fn fetch_file_on_demand(&self, file_id: String) -> Result<(), EngineError> {
        let db = self.db.clone();
        let storage = self.storage_dir.clone();
        let sync = self.sync_dir.clone();
        let cert = self.identity.cert_pem.clone();
        let key = self.identity.key_pem.clone();
        let peers = self.known_peers.clone();
        let ignore = self.ignore_set.clone();
        let tx = self.event_tx.clone();

        self.runtime.spawn(async move {
            match discovery::download_file_on_demand(
                file_id, db, storage, sync, cert, key, peers, ignore, tx
            ).await {
                Ok(_) => println!("[Engine] On-demand fetch successful"),
                Err(e) => println!("[Engine] On-demand fetch failed: {}", e),
            }
        });
        Ok(())
    }

    pub fn set_file_pinned_devices(&self, file_id: String, devices: Vec<String>) -> Result<(), EngineError> {
        let db = self.db.clone();
        let storage = self.storage_dir.clone();
        let cert = self.identity.cert_pem.clone();
        let key = self.identity.key_pem.clone();
        let peers = self.known_peers.clone();
        let tx = self.event_tx.clone();

        // Update local DB first
        {
            let db = db.lock().unwrap();
            if let Ok(Some(mut file)) = db.get_file_by_id(&file_id) {
                file.pinned_devices = devices.clone();
                file.version += 1; // bump version
                if let Err(e) = db.insert_file(&file) {
                    return Err(EngineError::from(e.to_string()));
                }
            } else {
                return Err(EngineError::from("File not found"));
            }
        }

        // Push updated metadata to all peers
        self.runtime.spawn(async move {
            let peers = peers.lock().unwrap().clone();
            let trusted_certs = match storage::load_all_trusted_certs(&storage) {
                Ok(c) => c,
                Err(_) => return,
            };
            let client = match discovery::build_mtls_client(&cert, &key, &trusted_certs) {
                Ok(c) => c,
                Err(_) => return,
            };

            let (file, blocks) = {
                let db = db.lock().unwrap();
                let file = match db.get_file_by_id(&file_id).ok().flatten() {
                    Some(f) => f,
                    None => return,
                };
                let blocks = db.get_blocks_for_file(&file_id).unwrap_or_default();
                (file, blocks)
            };

            let push_req = serde_json::json!({
                "file": file.clone(),
                "blocks": blocks.clone()
            });

            let we_have_data = blocks.iter().all(|b| b.is_present == 1);

            for (peer_id, peer) in peers {
                let peer_url = &peer.url;
                let _ = client.post(format!("{}/push_metadata", peer_url)).json(&push_req).send().await;

                let peer_is_pinned = file.pinned_devices.contains(&peer_id);

                if peer_is_pinned && we_have_data {
                    for b in &blocks {
                        if let Ok(data) = storage::read_block(&storage, &b.block_id) {
                            let _ = client.post(format!("{}/push_block/{}", peer_url, b.block_id)).body(data).send().await;
                        }
                    }
                    let _ = client.post(format!("{}/finalize_file/{}", peer_url, file_id)).send().await;
                }
            }

            let _ = tx.send(EngineEvent::FileSent { path: file.path.clone() });
        });
        Ok(())
    }

    pub fn start(&self) -> Result<(), EngineError> {
        let handle = self.runtime.handle().clone();

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
        let server_trust = self.trust.clone();
        let server_pairing = self.pairing.clone();
        let server_device_name = self.identity.device_name.clone();

        let server_task = handle.spawn(async move {
            if let Err(e) = server::start_server(
                listener,
                server_device_id,
                server_device_name,
                server_cert,
                server_key,
                server_db,
                server_storage,
                server_sync,
                server_ignore,
                server_trust,
                server_pairing,
                server_tx,
            ).await {
                let _ = server_tx_err.send(EngineEvent::ErrorEvent {
                    message: format!("Server error: {}", e),
                });
            }
        });
        *self.server_task.lock().unwrap() = Some(server_task);

        let disc_tx = self.event_tx.clone();
        let disc_device_id = self.identity.device_id.clone();
        let disc_device_name = self.identity.device_name.clone();
        let disc_peers = self.known_peers.clone();

        let daemon = discovery::start_discovery(
            disc_device_id,
            disc_device_name,
            port,
            disc_tx,
            disc_peers,
        ).map_err(EngineError::from)?;

        *self.mdns_daemon.lock().unwrap() = Some(daemon);

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