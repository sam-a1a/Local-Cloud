// engine/src/lib.rs
pub mod collision;
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
pub use collision::{CollisionQueue, CollisionResolution, PendingCollision};
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

/// How long a trashed item stays restorable before its bytes are released.
///
/// Only ever reached by the last copy of an item: deleting a copy while others
/// remain frees the space at once, because nothing can be lost.
pub const TRASH_RETENTION_SECS: i64 = 30 * 24 * 60 * 60;

/// How often expired trash is looked for. The retention is measured in days, so
/// checking hourly is ample and costs nothing when there is nothing to do.
const TRASH_SWEEP_INTERVAL_SECS: u64 = 60 * 60;

/// How often a device re-reads the catalogs of the peers it can see.
///
/// Syncing only on discovery is not enough to keep a catalog true. A peer is
/// announced when it is new or has moved, so two devices sitting on a network
/// would exchange catalogs once and never again - a file added on one would
/// stay invisible to the other for as long as both kept running. Every paired
/// device is supposed to hold a complete copy of the catalog, and that is a
/// property that has to be maintained, not established once.
const CATALOG_SYNC_INTERVAL_SECS: u64 = 30;

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
    NameCollision { requested_path: String, kept_as: String },
    CollisionResolved { path: String },
    CopyDeleted { file_id: String, device_id: String },
    FileTrashed { file_id: String },
    FileRestored { file_id: String },
    FilePurged { file_id: String },
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
    trash_sweep_task: StdMutex<Option<tokio::task::JoinHandle<()>>>,
    catalog_sync_task: StdMutex<Option<tokio::task::JoinHandle<()>>>,
    /// Asks the catalog sync task to reach a peer now.
    ///
    /// Renewed by every `start` and cleared by `stop`, so it always addresses
    /// the task that is currently running - and is absent while the engine is
    /// stopped, when there is nothing to ask.
    sync_nudge: StdMutex<Option<discovery::SyncNudge>>,
    known_peers: PeerMap,
    trust: TrustStore,
    pairing: PairingState,
    collisions: CollisionQueue,
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
            trash_sweep_task: StdMutex::new(None),
            catalog_sync_task: StdMutex::new(None),
            sync_nudge: StdMutex::new(None),
            known_peers,
            trust,
            pairing: PairingState::new(),
            collisions: CollisionQueue::new(),
        })
    }

    pub fn device_id(&self) -> String {
        self.identity.device_id.clone()
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
        // Absent when the engine is not running, in which case there is no
        // sync task to ask and the pairing simply stands until the next start.
        let nudge = self.sync_nudge.lock().unwrap().clone();

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

            // Newly paired and already visible, so its catalog can be read now
            // rather than at the next scheduled pass.
            if let Some(nudge) = nudge {
                let _ = nudge.send(peer.device_id.clone());
            }

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
        let sync = self.sync_dir.clone();
        let cert = self.identity.cert_pem.clone();
        let key = self.identity.key_pem.clone();
        let my_id = self.identity.device_id.clone();
        let ignore = self.ignore_set.clone();
        let collisions = self.collisions.clone();
        let indexer = self.indexer();
        let tx = self.event_tx.clone();

        self.runtime.spawn(async move {
            discovery::sync_with_peer(
                peer_url, peer_id, my_id, db, storage, sync, cert, key, ignore, collisions,
                indexer, tx,
            )
            .await;
        });
        Ok(())
    }

    /// Takes a copy of an item for this device.
    ///
    /// The counterpart to `share_to`: instead of a sender choosing where
    /// something goes, a device helps itself to something it can see. Both are
    /// deliberate acts by a person - nothing copies itself.
    pub fn pull_copy(&self, file_id: String) -> Result<(), EngineError> {
        let db = self.db.clone();
        let storage = self.storage_dir.clone();
        let sync = self.sync_dir.clone();
        let cert = self.identity.cert_pem.clone();
        let key = self.identity.key_pem.clone();
        let my_id = self.identity.device_id.clone();
        let peers = self.known_peers.clone();
        let ignore = self.ignore_set.clone();
        let tx = self.event_tx.clone();

        self.runtime.spawn(async move {
            if let Err(e) = discovery::pull_copy(
                file_id, my_id, db, storage, sync, cert, key, peers, ignore, tx.clone(),
            )
            .await
            {
                let _ = tx.send(EngineEvent::ErrorEvent { message: e });
            }
        });
        Ok(())
    }

    /// The shared namespace as this device currently knows it.
    pub fn get_catalog(&self) -> db::Catalog {
        let db = self.db.lock().unwrap();
        db::Catalog {
            files: db.get_all_files().unwrap_or_default(),
            holders: db.get_all_holders().unwrap_or_default(),
            delete_requests: db.get_delete_requests().unwrap_or_default(),
            tombstones: db.get_all_tombstones().unwrap_or_default(),
        }
    }

    fn indexer(&self) -> watcher::Indexer {
        watcher::Indexer::new(
            self.db.clone(),
            self.storage_dir.clone(),
            self.sync_dir.clone(),
            self.identity.device_id.clone(),
            self.ignore_set.clone(),
            self.collisions.clone(),
        )
    }

    // ---- Name collisions ----

    /// Conflicts kept both ways for safety, still awaiting a decision.
    pub fn pending_collisions(&self) -> Vec<PendingCollision> {
        self.collisions.pending()
    }

    /// Applies a decision to a name conflict.
    ///
    /// `KeepBoth` confirms what already happened. `Override` gives the name to
    /// the incoming item and moves the previous one to trash, where it stays
    /// restorable rather than being destroyed.
    pub fn resolve_collision(
        &self,
        collision_id: String,
        resolution: CollisionResolution,
    ) -> Result<(), EngineError> {
        let collision = self
            .collisions
            .take(&collision_id)
            .ok_or_else(|| EngineError::from("No such collision, or it was already resolved"))?;

        if resolution == CollisionResolution::KeepBoth {
            return Ok(());
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        {
            let db = self.db.lock().unwrap();
            if let Err(e) = db.override_file(
                &collision.existing_file_id,
                &collision.incoming_file_id,
                &collision.requested_path,
                &self.identity.device_id,
                now,
            ) {
                // Nothing changed, so put the question back rather than
                // silently dropping the decision.
                self.collisions.record(collision);
                return Err(EngineError::from(e));
            }
        }

        // Keep the sync folder agreeing with the catalog about the name.
        let from = format!("{}/{}", self.sync_dir, collision.current_path);
        let to = format!("{}/{}", self.sync_dir, collision.requested_path);
        if std::path::Path::new(&from).exists() {
            ignore::mark_ignored(&self.ignore_set, &from);
            ignore::mark_ignored(&self.ignore_set, &to);
            if let Err(e) = std::fs::rename(&from, &to) {
                println!("[Engine] Renamed in catalog but not on disk: {}", e);
            }
            ignore::schedule_unmark_ignored(self.ignore_set.clone(), from, 3);
            ignore::schedule_unmark_ignored(self.ignore_set.clone(), to, 3);
        }

        let _ = self.event_tx.send(EngineEvent::CollisionResolved {
            path: collision.requested_path,
        });
        Ok(())
    }

    // ---- Deleting copies ----

    /// Deletes a copy of an item from one device.
    ///
    /// This is the only delete there is: it removes one device's copy, never
    /// the item itself. Every other copy is untouched, and the item stays in
    /// the catalog for as long as anyone still holds it.
    ///
    /// `device_id` may be any device, including ones this one cannot currently
    /// reach. Only a holder can erase its own disk, so a delete aimed elsewhere
    /// is recorded and delivered - immediately if the target is reachable,
    /// otherwise through the catalog when it returns.
    pub fn delete_copy(&self, file_id: String, device_id: String) -> Result<(), EngineError> {
        if device_id == self.identity.device_id {
            self.delete_local_copy(file_id)?;
            return Ok(());
        }

        {
            let db = self.db.lock().unwrap();
            if !db.is_paired(&device_id).map_err(EngineError::from)? {
                return Err(EngineError::from("That device is not paired"));
            }
            if !db
                .is_holder(&file_id, &device_id)
                .map_err(EngineError::from)?
            {
                return Err(EngineError::from("That device does not hold that item"));
            }

            // Recorded before any attempt to deliver it, so the instruction
            // survives the target being unreachable.
            db.record_delete_request(&db::DeleteRequest {
                file_id: file_id.clone(),
                target_device: device_id.clone(),
                requested_by: self.identity.device_id.clone(),
                requested_at: watcher::now_secs(),
            })
            .map_err(EngineError::from)?;
        }

        let url = {
            let peers = self.known_peers.lock().unwrap();
            peers.get(&device_id).map(|d| d.url.clone())
        };

        // If it is not reachable the request simply waits; the catalog carries
        // it there whenever the device next syncs with anyone.
        if let Some(url) = url {
            let storage = self.storage_dir.clone();
            let cert = self.identity.cert_pem.clone();
            let key = self.identity.key_pem.clone();
            let requested_by = self.identity.device_id.clone();
            let tx = self.event_tx.clone();

            self.runtime.spawn(async move {
                let trusted = match storage::load_all_trusted_certs(&storage) {
                    Ok(c) => c,
                    Err(_) => return,
                };
                let client = match discovery::build_mtls_client(&cert, &key, &trusted) {
                    Ok(c) => c,
                    Err(_) => return,
                };

                let sent = client
                    .post(format!("{}/request_delete", url))
                    .json(&serde_json::json!({
                        "file_id": file_id,
                        "requested_by": requested_by,
                    }))
                    .send()
                    .await;

                if let Err(e) = sent {
                    let _ = tx.send(EngineEvent::ErrorEvent {
                        message: format!("Delete queued; {} was not reachable: {}", device_id, e),
                    });
                }
            });
        }

        Ok(())
    }

    /// Deletes this device's own copy.
    ///
    /// Frees the space at once when other copies exist. When it is the last
    /// copy the bytes are kept and the item moves to trash instead, because an
    /// item nobody holds could not be restored.
    pub fn delete_local_copy(
        &self,
        file_id: String,
    ) -> Result<watcher::DeleteOutcome, EngineError> {
        let outcome = self
            .indexer()
            .delete_local_copy(&file_id, true)
            .map_err(EngineError::from)?;

        let event = if outcome.trashed {
            EngineEvent::FileTrashed {
                file_id: outcome.file_id.clone(),
            }
        } else {
            EngineEvent::CopyDeleted {
                file_id: outcome.file_id.clone(),
                device_id: self.identity.device_id.clone(),
            }
        };
        let _ = self.event_tx.send(event);

        Ok(outcome)
    }

    /// Deletes still waiting on a device that has not been reachable.
    pub fn pending_delete_requests(&self) -> Vec<db::DeleteRequest> {
        let db = self.db.lock().unwrap();
        db.get_delete_requests().unwrap_or_default()
    }

    /// Items that have been moved aside. Their bytes are still on whichever
    /// devices held them, so restoring is possible until they are purged.
    pub fn get_trashed_files(&self) -> Vec<FileMetadata> {
        let db = self.db.lock().unwrap();
        db.get_trashed_files().unwrap_or_default()
    }

    /// Brings a trashed item back under its original name.
    ///
    /// Possible for as long as some device still holds its bytes, which is why
    /// deleting a last copy keeps them rather than freeing the space.
    pub fn restore_file(&self, file_id: String) -> Result<(), EngineError> {
        {
            let db = self.db.lock().unwrap();
            db.restore_file(&file_id).map_err(EngineError::from)?;
        }
        let _ = self.event_tx.send(EngineEvent::FileRestored {
            file_id: file_id.clone(),
        });
        Ok(())
    }

    /// Destroys a trashed item now instead of waiting out its retention.
    ///
    /// Frees the space immediately and cannot be undone. Peers purge it too
    /// once they see the tombstone.
    pub fn delete_permanently(&self, file_id: String) -> Result<(), EngineError> {
        {
            let db = self.db.lock().unwrap();
            let file = db
                .get_file_by_id(&file_id)
                .map_err(EngineError::from)?
                .ok_or_else(|| EngineError::from("No such item in the catalog"))?;
            if !file.is_trashed() {
                return Err(EngineError::from(
                    "Only a trashed item can be destroyed; delete the copies first",
                ));
            }
        }

        self.indexer().purge(&file_id).map_err(EngineError::from)?;
        let _ = self.event_tx.send(EngineEvent::FilePurged { file_id });
        Ok(())
    }

    /// How long an item has left in trash, in seconds, or None if it is live.
    pub fn trash_seconds_remaining(&self, file_id: &str) -> Option<i64> {
        let db = self.db.lock().unwrap();
        let file = db.get_file_by_id(file_id).ok().flatten()?;
        if !file.is_trashed() {
            return None;
        }
        Some((file.trashed_at + TRASH_RETENTION_SECS - watcher::now_secs()).max(0))
    }

    /// Runs the retention sweep once, rather than waiting for the next tick.
    /// Returns the items destroyed.
    pub fn sweep_trash_now(&self) -> Vec<String> {
        let purged = self
            .indexer()
            .sweep_trash(watcher::now_secs(), TRASH_RETENTION_SECS);
        for file_id in &purged {
            let _ = self.event_tx.send(EngineEvent::FilePurged {
                file_id: file_id.clone(),
            });
        }
        purged
    }

    /// Which devices hold this item, and which content each one has.
    pub fn get_file_holders(&self, file_id: &str) -> Vec<db::FileHolder> {
        let db = self.db.lock().unwrap();
        db.get_holders(file_id).unwrap_or_default()
    }

    /// Sends a copy of an item to specific devices.
    ///
    /// This is the primary way bytes move. The sender keeps its own copy; each
    /// recipient claims a holder row once it has assembled the content.
    ///
    /// Name collisions are not handled yet: a recipient that already has a
    /// different item at the same path will reject the metadata rather than
    /// prompting to override or keep both.
    pub fn share_to(
        &self,
        file_id: String,
        target_device_ids: Vec<String>,
    ) -> Result<(), EngineError> {
        if target_device_ids.is_empty() {
            return Err(EngineError::from("Select at least one device to share with"));
        }

        let (file, blocks) = {
            let db = self.db.lock().unwrap();
            let file = db
                .get_file_by_id(&file_id)
                .map_err(EngineError::from)?
                .ok_or_else(|| EngineError::from("No such item in the catalog"))?;
            let blocks = db.get_blocks_for_file(&file_id).unwrap_or_default();
            (file, blocks)
        };

        if blocks.is_empty() || !blocks.iter().all(|b| b.is_present == 1) {
            return Err(EngineError::from(
                "This device does not hold the data for that item",
            ));
        }

        // Only paired, currently visible devices can receive anything.
        let targets: Vec<(String, String)> = {
            let peers = self.known_peers.lock().unwrap();
            let db = self.db.lock().unwrap();
            target_device_ids
                .iter()
                .filter(|id| db.is_paired(id).unwrap_or(false))
                .filter_map(|id| peers.get(id).map(|d| (id.clone(), d.url.clone())))
                .collect()
        };

        if targets.is_empty() {
            return Err(EngineError::from(
                "None of those devices are paired and reachable",
            ));
        }

        let storage = self.storage_dir.clone();
        let cert = self.identity.cert_pem.clone();
        let key = self.identity.key_pem.clone();
        let tx = self.event_tx.clone();

        self.runtime.spawn(async move {
            let trusted_certs = match storage::load_all_trusted_certs(&storage) {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(EngineEvent::ErrorEvent { message: e.to_string() });
                    return;
                }
            };
            let client = match discovery::build_mtls_client(&cert, &key, &trusted_certs) {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(EngineEvent::ErrorEvent { message: e.to_string() });
                    return;
                }
            };

            let announce = serde_json::json!({ "file": file, "blocks": blocks });

            for (device_id, url) in targets {
                let announced = client
                    .post(format!("{}/push_metadata", url))
                    .json(&announce)
                    .send()
                    .await;

                match announced {
                    Ok(r) if r.status().is_success() => {}
                    Ok(r) => {
                        let _ = tx.send(EngineEvent::ErrorEvent {
                            message: format!("{} refused {}: {}", device_id, file.path, r.status()),
                        });
                        continue;
                    }
                    Err(e) => {
                        let _ = tx.send(EngineEvent::ErrorEvent {
                            message: format!("Could not reach {}: {}", device_id, e),
                        });
                        continue;
                    }
                }

                if !discovery::push_blocks_to_peer(&client, &url, &blocks, &storage).await {
                    let _ = tx.send(EngineEvent::ErrorEvent {
                        message: format!("Could not send all of {} to {}", file.path, device_id),
                    });
                    continue;
                }

                // The recipient claims its holder row on finalize; until then it
                // has the blocks but is not recorded as holding the item.
                let _ = client
                    .post(format!("{}/finalize_file/{}", url, file.id))
                    .send()
                    .await;

                let _ = tx.send(EngineEvent::FileSent { path: file.path.clone() });
            }
        });

        Ok(())
    }

    /// Keeps this device's catalog in step with the peers it can see.
    ///
    /// Convergence is the engine's own responsibility. It used to be left to
    /// whoever embedded the engine to notice `PeerDiscovered` and call
    /// `sync_with_peer` themselves, which meant a consumer that did not happen
    /// to do so simply never synced, with nothing to indicate anything was
    /// wrong - the catalog just stayed empty.
    ///
    /// A newly seen peer is synced with at once so a device that has just
    /// appeared is not left waiting for the next tick; the interval then keeps
    /// everything already visible up to date. `sync_with_peer` ignores anything
    /// unpaired, and decides that locally, so an unpaired device on the network
    /// costs nothing here.
    fn spawn_catalog_sync(
        &self,
        mut nudges: tokio::sync::mpsc::UnboundedReceiver<String>,
    ) -> tokio::task::JoinHandle<()> {
        let db = self.db.clone();
        let storage = self.storage_dir.clone();
        let sync = self.sync_dir.clone();
        let cert = self.identity.cert_pem.clone();
        let key = self.identity.key_pem.clone();
        let my_id = self.identity.device_id.clone();
        let ignore = self.ignore_set.clone();
        let collisions = self.collisions.clone();
        let indexer = self.indexer();
        let tx = self.event_tx.clone();
        let peers = self.known_peers.clone();

        self.runtime.spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                CATALOG_SYNC_INTERVAL_SECS,
            ));
            let mut discovering = true;

            loop {
                let due: Vec<DiscoveredDevice> = tokio::select! {
                    nudged = nudges.recv(), if discovering => match nudged {
                        // Resolved here rather than carried in the message, so
                        // a peer that has moved since is still reached, and one
                        // that is no longer visible is simply skipped.
                        Some(device_id) => peers
                            .lock()
                            .unwrap()
                            .get(&device_id)
                            .cloned()
                            .into_iter()
                            .collect(),
                        // Discovery has shut down. The interval carries on with
                        // whatever is already known rather than ending here.
                        None => {
                            discovering = false;
                            Vec::new()
                        }
                    },
                    _ = ticker.tick() => {
                        // Collected rather than iterated, so the lock is not
                        // held across a sync.
                        let known = peers.lock().unwrap();
                        known.values().cloned().collect()
                    }
                };

                for device in due {
                    discovery::sync_with_peer(
                        device.url,
                        device.device_id,
                        my_id.clone(),
                        db.clone(),
                        storage.clone(),
                        sync.clone(),
                        cert.clone(),
                        key.clone(),
                        ignore.clone(),
                        collisions.clone(),
                        indexer.clone(),
                        tx.clone(),
                    )
                    .await;
                }
            }
        })
    }

    pub fn start(&self) -> Result<(), EngineError> {
        let handle = self.runtime.handle().clone();

        // Registering the listener needs a reactor, and the engine has one of
        // its own - but nothing had put it in scope, so it used to find the
        // *caller's* instead. That works from inside `#[tokio::main]` and
        // panics anywhere else, which is precisely where this is headed: a
        // binding called from Swift or Kotlin is not running in a runtime, and
        // would have met "there is no reactor running" on the first call.
        let _entered = handle.enter();

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
        let server_indexer = self.indexer();
        let server_device_name = self.identity.device_name.clone();
        // Renewed here so a restarted engine addresses its new sync task
        // rather than the one `stop` aborted.
        let (nudge_tx, nudge_rx) = tokio::sync::mpsc::unbounded_channel();
        *self.sync_nudge.lock().unwrap() = Some(nudge_tx.clone());
        let server_nudge = nudge_tx.clone();

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
                server_indexer,
                server_tx,
                server_nudge,
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
            nudge_tx,
        ).map_err(EngineError::from)?;

        *self.mdns_daemon.lock().unwrap() = Some(daemon);

        let sync_task = self.spawn_catalog_sync(nudge_rx);
        if let Some(previous) = self.catalog_sync_task.lock().unwrap().replace(sync_task) {
            // Belt and braces: `stop` aborts it, but starting twice without a
            // stop in between must not leave two tasks syncing in parallel.
            previous.abort();
        }

        let watcher = watcher::start_watcher(
            self.indexer(),
            handle,
            self.ignore_set.clone(),
            self.event_tx.clone(),
        ).map_err(EngineError::from)?;

        *self.watcher.lock().unwrap() = Some(watcher);

        // Retention is measured in days, so an hourly check is ample. It also
        // runs once immediately, which is what releases trash that expired
        // while this device was switched off.
        let sweep_indexer = self.indexer();
        let sweep_tx = self.event_tx.clone();
        let sweep_task = self.runtime.spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                TRASH_SWEEP_INTERVAL_SECS,
            ));
            loop {
                ticker.tick().await;
                for file_id in
                    sweep_indexer.sweep_trash(watcher::now_secs(), TRASH_RETENTION_SECS)
                {
                    println!("[Trash] Retention expired, destroyed {}", file_id);
                    let _ = sweep_tx.send(EngineEvent::FilePurged { file_id });
                }
            }
        });
        *self.trash_sweep_task.lock().unwrap() = Some(sweep_task);

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
        if let Some(t) = self.trash_sweep_task.lock().unwrap().take() {
            t.abort();
        }
        if let Some(t) = self.catalog_sync_task.lock().unwrap().take() {
            t.abort();
        }
        // Nothing left to ask, until the next start hands out a new one.
        *self.sync_nudge.lock().unwrap() = None;
        let _ = self.event_tx.send(EngineEvent::EngineStopped);
        println!("[Engine] Stopped.");
    }
}