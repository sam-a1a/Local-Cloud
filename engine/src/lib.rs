//! LocalCloud's engine: a mesh of your own devices that behaves like shared
//! storage, with no server and nothing copied without a person asking.
//!
//! # The supported API
//!
//! [`Engine`] and the types it takes and returns:
//!
//! - [`EngineError`] — why an operation could not be carried out
//! - [`EngineEvent`] and [`EventListener`] — what happened, and how to hear it
//! - [`Catalog`], [`FileMetadata`], [`FileHolder`] — the shared namespace
//! - [`PairedDevice`], [`DiscoveredDevice`], [`PairingOffer`] — devices
//! - [`PendingCollision`], [`CollisionResolution`] — contested names
//! - [`DeleteRequest`], [`DeleteOutcome`] — deletion
//!
//! That is the whole of it. Everything else this crate exposes is machinery:
//! the database, the block store, the TLS server, the watcher, the discovery
//! and pairing protocols. Those are `#[doc(hidden)]` and carry no compatibility
//! promise — they are reachable only because the integration tests are separate
//! crates that drive them directly. Building against them means being broken by
//! an internal change, without warning.
//!
//! See §10a of DESIGN.md for the rules the API is held to.

// Nothing outside this crate names these.
mod collision;
mod crypto;
mod ignore;
mod tls;

// Reachable, but not API. See the note above.
#[doc(hidden)]
pub mod db;
#[doc(hidden)]
pub mod discovery;
#[doc(hidden)]
pub mod pairing;
#[doc(hidden)]
pub mod server;
#[doc(hidden)]
pub mod storage;
#[doc(hidden)]
pub mod watcher;

// The API's vocabulary.
pub use collision::{CollisionResolution, PendingCollision};
pub use db::{DeleteRequest, FileHolder, FileMetadata, PairedDevice};
pub use discovery::DiscoveredDevice;
pub use pairing::PairingOffer;
pub use watcher::DeleteOutcome;

// Machinery the tests construct directly.
#[doc(hidden)]
pub use collision::CollisionQueue;
#[doc(hidden)]
pub use crypto::DeviceIdentity;
#[doc(hidden)]
pub use db::{BlockMetadata, Database, FileBlock, Tombstone};
#[doc(hidden)]
pub use discovery::PeerMap;
#[doc(hidden)]
pub use ignore::{new_ignore_set, IgnoreSet};
#[doc(hidden)]
pub use pairing::PairingState;
#[doc(hidden)]
pub use tls::TrustStore;

use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, mpsc};
use tokio::net::TcpListener;
#[cfg(desktop)]
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

/// Why an operation could not be carried out.
///
/// Deliberately specific. These cross into Swift and Kotlin as typed
/// exceptions, and an application forced to match on English prose to tell
/// "that device is not paired" from "the disk is full" cannot react sensibly to
/// either - nor can it survive the wording being improved. Match on the
/// variant; show the message.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// Nothing in the catalog has this id.
    #[error("No such item in the catalog")]
    NoSuchItem { file_id: String },

    /// The item exists, but this device does not have its data, so it has
    /// nothing to send or delete.
    #[error("This device does not hold that item")]
    NotHeldHere { file_id: String },

    /// The item exists, but the named device is not one of its holders.
    #[error("That device does not hold that item")]
    NotAHolder { file_id: String, device_id: String },

    /// Only a trashed item can be destroyed; a live one has to be deleted from
    /// its holders first.
    #[error("Only a trashed item can be destroyed; delete the copies first")]
    NotTrashed { file_id: String },

    /// The item is in the trash, and trashed items do not take part in
    /// anything until they are restored.
    #[error("That item is in the trash; restore it first")]
    InTrash { file_id: String },

    /// The device has not been through pairing, so it may not be asked anything.
    #[error("That device is not paired with this one")]
    NotPaired { device_id: String },

    /// Paired, perhaps, but not on the network right now.
    #[error("That device is not visible on the network")]
    NotVisible { device_id: String },

    /// A name that is not a name: empty, hidden, or a path in disguise.
    #[error("That is not a usable name for an item")]
    InvalidName { name: String },

    /// There is no file to import at that path.
    #[error("There is no file at that path")]
    NoSuchFile { path: String },

    /// No devices were given for an operation that needs at least one.
    #[error("Select at least one device")]
    NothingSelected,

    /// Devices were given, but none of them can be used - not visible, not
    /// paired, or not holding what was asked for.
    #[error("{reason}")]
    NoUsableDevices { reason: String },

    /// No collision is awaiting this decision. Most often it was already
    /// settled, possibly on another device.
    #[error("No such collision, or it was already resolved")]
    NoSuchCollision { collision_id: String },

    /// Pairing could not proceed: no request from that device, a code that has
    /// expired, or one entered too many times.
    #[error("{reason}")]
    Pairing { reason: String },

    /// The database, the filesystem or the network stack failed. Nothing the
    /// caller did wrong, and nothing it can correct.
    #[error("{reason}")]
    Internal { reason: String },
}

impl EngineError {
    /// For failures that are genuinely this device's problem rather than a
    /// misuse of the API - a database error, a full disk, a socket that will
    /// not bind.
    fn internal<E: std::fmt::Display>(e: E) -> Self {
        EngineError::Internal {
            reason: e.to_string(),
        }
    }
}

/// Something the engine did, or could not do, without being asked at that
/// moment.
///
/// Operations that touch the network return as soon as their request is
/// understood and finish in the background, so this is where their outcome
/// arrives. Every failure names what it was about - the item, the device, or
/// both - because an application has to put the message beside the row that
/// caused it, and a bare string leaves it nowhere to go but a toast.
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
    FileIndexed { file_id: String, path: String },

    /// A copy of `file_id` reached `device_id`.
    FileSent { file_id: String, path: String, device_id: String },
    /// Sending a copy to `device_id` did not finish. Other devices in the same
    /// `share_to` may still have succeeded; each reports separately.
    ShareFailed { file_id: String, path: String, device_id: String, reason: String },

    /// This device now holds `file_id`, whether it was pushed here or pulled.
    FileDownloaded { file_id: String, path: String },
    /// Taking a copy did not finish, so this device still does not hold it.
    PullFailed { file_id: String, reason: String },

    /// A device was asked to drop its copy but could not be reached. The
    /// request stands and travels through the catalog instead.
    DeleteRequestDeferred { file_id: String, device_id: String, reason: String },

    /// Something went wrong that concerns no particular item or device - the
    /// server failing to bind, pairing failing before a peer was chosen.
    EngineFailed { reason: String },
}

/// The shared namespace: every item, and which devices hold which content.
///
/// Deliberately not what travels between devices. `db::CatalogPayload` also
/// carries tombstones and outstanding delete requests, which exist so peers can
/// converge and mean nothing to an application - exposing them would invite one
/// to reason about replication rather than about items. Delete requests that a
/// person should see are `pending_delete_requests`.
#[derive(Clone, Debug, Serialize)]
pub struct Catalog {
    pub items: Vec<FileMetadata>,
    pub holders: Vec<FileHolder>,
}

/// Receives events as the engine produces them.
///
/// Implemented by the application. A callback interface rather than a queue to
/// poll, because that is what an application actually wants and what bindings
/// generate idiomatically on both platforms - and because a queue has exactly
/// one consumer, so anything else that wanted to observe events had to be given
/// them by hand.
///
/// Calls arrive on a background thread, never on the thread that called into
/// the engine, and never two at once. An implementation must not assume it is
/// on a UI thread, and must not call back into the engine and block waiting for
/// an event.
pub trait EventListener: Send + Sync {
    fn on_event(&self, event: EngineEvent);
}

/// How many events are held for a listener that has not been set yet.
///
/// An application is expected to set one before `start`, so in practice this
/// holds nothing. It exists so that one which forgets leaks a bounded amount
/// rather than growing for as long as it runs.
const EVENT_BACKLOG: usize = 256;

/// Carries events from the engine's internals to whatever the application
/// registered, on one thread, in order.
///
/// Draining continuously rather than only once a listener exists is what bounds
/// the memory: a channel nobody reads grows for as long as the engine runs,
/// whereas a backlog can be capped and, when it overflows, say so.
fn spawn_event_dispatch(
    events: mpsc::Receiver<EngineEvent>,
    listener: Arc<StdMutex<Option<Arc<dyn EventListener>>>>,
) {
    use std::sync::mpsc::RecvTimeoutError;

    std::thread::spawn(move || {
        let mut backlog: Vec<EngineEvent> = Vec::new();
        let mut dropped: usize = 0;

        loop {
            // Everything is queued first and delivered second, so one thread
            // decides the order and a listener registered late is not a special
            // case. The timeout exists for exactly that: a listener set when no
            // further events are coming would otherwise never see the backlog,
            // because nothing would wake this loop to notice it.
            match events.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(event) => {
                    if backlog.len() == EVENT_BACKLOG {
                        backlog.remove(0);
                        dropped += 1;
                    }
                    backlog.push(event);
                }
                Err(RecvTimeoutError::Timeout) => {}
                // Every sender is gone, so the engine has been dropped.
                Err(RecvTimeoutError::Disconnected) => return,
            }

            if backlog.is_empty() {
                continue;
            }

            // Cloned out, and the lock released, before anything is delivered:
            // a listener is free to call back into the engine, and must not
            // find this held.
            let Some(current) = listener.lock().unwrap().clone() else {
                continue;
            };

            if dropped > 0 {
                current.on_event(EngineEvent::EngineFailed {
                    reason: format!("{} events were discarded before a listener was set", dropped),
                });
                dropped = 0;
            }
            for event in backlog.drain(..) {
                current.on_event(event);
            }
        }
    });
}

pub struct Engine {
    db: Arc<StdMutex<Database>>,
    identity: DeviceIdentity,
    storage_dir: String,
    sync_dir: String,
    ignore_set: IgnoreSet,
    event_tx: mpsc::Sender<EngineEvent>,
    listener: Arc<StdMutex<Option<Arc<dyn EventListener>>>>,
    runtime: tokio::runtime::Runtime,
    #[cfg(desktop)]
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
        std::fs::create_dir_all(&base_dir).map_err(EngineError::internal)?;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(EngineError::internal)?;

        let identity = DeviceIdentity::load_or_generate(&base_dir).map_err(EngineError::internal)?;
        let short_id = &identity.device_id[..8];

        let db_path = format!("{}/local-cloud-{}.db", base_dir, short_id);
        let storage_dir = format!("{}/storage_{}", base_dir, short_id);

        let database = Database::init(&db_path).map_err(EngineError::internal)?;
        storage::ensure_storage_dir(&storage_dir).map_err(EngineError::internal)?;
        storage::ensure_trusted_peers_dir(&storage_dir).map_err(EngineError::internal)?;

        std::fs::create_dir_all(&sync_dir_path).map_err(EngineError::internal)?;
        let sync_dir = std::fs::canonicalize(&sync_dir_path)
            .map_err(EngineError::internal)?
            .to_string_lossy()
            .to_string();

        let db_state = Arc::new(StdMutex::new(database));
        let ignore_set = new_ignore_set();
        let (event_tx, event_rx) = mpsc::channel();
        let listener: Arc<StdMutex<Option<Arc<dyn EventListener>>>> =
            Arc::new(StdMutex::new(None));
        spawn_event_dispatch(event_rx, listener.clone());
        let known_peers = Arc::new(StdMutex::new(HashMap::new()));

        let trust = TrustStore::new();
        trust.reload(&storage_dir).map_err(EngineError::internal)?;

        Ok(Self {
            db: db_state,
            identity,
            storage_dir,
            sync_dir,
            ignore_set,
            event_tx,
            listener,
            runtime,
            #[cfg(desktop)]
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
            return Err(EngineError::NothingSelected);
        }

        let targets: Vec<(String, String)> = {
            let peers = self.known_peers.lock().unwrap();
            target_device_ids
                .iter()
                .filter_map(|id| peers.get(id).map(|d| (id.clone(), d.url.clone())))
                .collect()
        };

        if targets.is_empty() {
            return Err(EngineError::NoUsableDevices {
                reason: "None of those devices are visible on the network".into(),
            });
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
                    let _ = tx.send(EngineEvent::EngineFailed {
                        reason: format!("Could not start pairing: {}", e),
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
            .ok_or_else(|| EngineError::Pairing {
                reason: "No pending pairing request from that device".into(),
            })?;

        let url = {
            let peers = self.known_peers.lock().unwrap();
            peers
                .get(&initiator_device_id)
                .map(|d| d.url.clone())
                .ok_or_else(|| {
                    EngineError::NotVisible { device_id: initiator_device_id.clone() }
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
            db.remove_paired_device(&device_id).map_err(EngineError::internal)?;
        }
        storage::remove_peer_cert(&self.storage_dir, &device_id).map_err(EngineError::internal)?;
        self.trust.reload(&self.storage_dir).map_err(EngineError::internal)?;
        Ok(())
    }

    /// Whether the engine is currently serving, discovering and syncing.
    ///
    /// An application that stops the engine when it is backgrounded needs to
    /// know what state it left it in, and asking is more reliable than
    /// remembering - a failed `start` leaves it stopped either way.
    pub fn is_running(&self) -> bool {
        self.server_task.lock().unwrap().is_some()
    }

    pub fn sync_dir(&self) -> String {
        self.sync_dir.clone()
    }

    /// Registers what receives events from here on.
    ///
    /// Set this before `start`. Anything the engine produced before a listener
    /// existed is delivered as soon as one does, up to `EVENT_BACKLOG`, so
    /// setting it a moment late costs nothing - but leaving it unset means an
    /// application eventually misses events, which is the honest trade for not
    /// growing without bound.
    ///
    /// Replacing a listener is allowed; the next event goes to the new one.
    pub fn set_event_listener(&self, listener: Arc<dyn EventListener>) {
        *self.listener.lock().unwrap() = Some(listener);
    }

    /// Every device currently visible on the network, paired or not.
    pub fn visible_devices(&self) -> Vec<DiscoveredDevice> {
        let peers = self.known_peers.lock().unwrap();
        let mut devices: Vec<DiscoveredDevice> = peers.values().cloned().collect();
        devices.sort_by(|a, b| a.name.cmp(&b.name));
        devices
    }

    /// Brings a file into the shared space from somewhere outside it.
    ///
    /// The way an item is created where there is no folder to watch. iOS cannot
    /// watch a user directory or run a background daemon freely, and Android
    /// would need `MANAGE_EXTERNAL_STORAGE`, so mobile hands the engine a file
    /// the share sheet produced and a name to give it.
    ///
    /// It works on desktop too, and means the same thing there: the bytes are
    /// copied into the sync folder, because the folder holds exactly what this
    /// device holds. Importing does not move or delete the original.
    ///
    /// `name` is what the item should be called, not a path - a share sheet
    /// supplies a filename, and anything that looks like a path is refused
    /// rather than quietly writing outside the folder. If that name is already
    /// taken, on disk or in the catalog, the item is numbered like any other
    /// collision rather than overwriting what is there.
    pub fn import_file(&self, source_path: String, name: String) -> Result<FileMetadata, EngineError> {
        let trimmed = name.trim();
        if trimmed.is_empty()
            || trimmed.contains('/')
            || trimmed.contains('\\')
            || trimmed.starts_with('.')
        {
            return Err(EngineError::InvalidName { name });
        }

        let source = std::path::Path::new(&source_path);
        if !source.is_file() {
            return Err(EngineError::NoSuchFile { path: source_path });
        }

        let indexer = self.indexer();
        let relative = indexer.free_path(trimmed);
        let destination = indexer.absolute(&relative);

        // Marked before the copy, not after: on desktop the watcher is looking
        // at this folder, and would otherwise index the file from underneath us
        // and race the call.
        ignore::mark_ignored(&self.ignore_set, &destination);
        let copied = std::fs::copy(source, &destination).map_err(EngineError::internal);
        ignore::schedule_unmark_ignored(self.ignore_set.clone(), destination.clone(), 3);
        copied?;

        let file_id = match indexer.index(&destination) {
            watcher::IndexOutcome::Indexed { file_id, .. } => file_id,
            watcher::IndexOutcome::KeptBoth { file_id, .. } => file_id,
            watcher::IndexOutcome::Unchanged { file_id } => file_id,
            watcher::IndexOutcome::Skipped { reason } => {
                // Nothing was catalogued, so nothing should be left behind.
                let _ = std::fs::remove_file(&destination);
                return Err(EngineError::Internal { reason });
            }
        };

        let file = {
            let db = self.db.lock().unwrap();
            db.get_file_by_id(&file_id)
                .map_err(EngineError::internal)?
                .ok_or_else(|| EngineError::NoSuchItem {
                    file_id: file_id.clone(),
                })?
        };

        let _ = self.event_tx.send(EngineEvent::FileIndexed {
            file_id,
            path: file.path.clone(),
        });

        Ok(file)
    }

    pub fn local_files(&self) -> Vec<FileMetadata> {
        let db = self.db.lock().unwrap();
        db.get_all_files().unwrap_or_default()
    }

    /// Takes a copy of an item for this device.
    ///
    /// The counterpart to `share_to`: instead of a sender choosing where
    /// something goes, a device helps itself to something it can see. Both are
    /// deliberate acts by a person - nothing copies itself.
    pub fn pull_copy(&self, file_id: String) -> Result<(), EngineError> {
        // Whatever can be decided from local state is decided now and returned,
        // so a caller learns straight away that it asked for something that
        // makes no sense. Only what depends on the network - whether a holder
        // answers, whether the blocks arrive - is left to the spawned task and
        // reported as a failure event.
        {
            let db = self.db.lock().unwrap();
            let file = db
                .get_file_by_id(&file_id)
                .map_err(EngineError::internal)?
                .ok_or_else(|| EngineError::NoSuchItem {
                    file_id: file_id.clone(),
                })?;
            if file.is_trashed() {
                return Err(EngineError::InTrash { file_id });
            }
        }

        let db = self.db.clone();
        let storage = self.storage_dir.clone();
        let sync = self.sync_dir.clone();
        let cert = self.identity.cert_pem.clone();
        let key = self.identity.key_pem.clone();
        let my_id = self.identity.device_id.clone();
        let peers = self.known_peers.clone();
        let ignore = self.ignore_set.clone();
        let tx = self.event_tx.clone();

        let pulled = file_id.clone();
        self.runtime.spawn(async move {
            if let Err(e) = discovery::pull_copy(
                file_id, my_id, db, storage, sync, cert, key, peers, ignore, tx.clone(),
            )
            .await
            {
                let _ = tx.send(EngineEvent::PullFailed {
                    file_id: pulled,
                    reason: e,
                });
            }
        });
        Ok(())
    }

    /// The shared namespace as this device currently knows it.
    pub fn catalog(&self) -> Catalog {
        let db = self.db.lock().unwrap();
        Catalog {
            items: db.get_all_files().unwrap_or_default(),
            holders: db.get_all_holders().unwrap_or_default(),
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
            .ok_or_else(|| EngineError::NoSuchCollision { collision_id: collision_id.clone() })?;

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
                return Err(EngineError::internal(e));
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
            if !db.is_paired(&device_id).map_err(EngineError::internal)? {
                return Err(EngineError::NotPaired { device_id: device_id.clone() });
            }
            if !db
                .is_holder(&file_id, &device_id)
                .map_err(EngineError::internal)?
            {
                return Err(EngineError::NotAHolder { file_id: file_id.clone(), device_id: device_id.clone() });
            }

            // Recorded before any attempt to deliver it, so the instruction
            // survives the target being unreachable.
            db.record_delete_request(&db::DeleteRequest {
                file_id: file_id.clone(),
                target_device: device_id.clone(),
                requested_by: self.identity.device_id.clone(),
                requested_at: watcher::now_secs(),
            })
            .map_err(EngineError::internal)?;
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
            let deferred_file_id = file_id.clone();

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
                    let _ = tx.send(EngineEvent::DeleteRequestDeferred {
                        file_id: deferred_file_id,
                        device_id: device_id.clone(),
                        reason: e.to_string(),
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
    ) -> Result<DeleteOutcome, EngineError> {
        // Checked here as well as in the indexer, so the caller is told *which*
        // thing was wrong. The indexer reports in prose, and flattening that
        // into `Internal` would lose the distinction the caller needs.
        {
            let db = self.db.lock().unwrap();
            if db
                .get_file_by_id(&file_id)
                .map_err(EngineError::internal)?
                .is_none()
            {
                return Err(EngineError::NoSuchItem { file_id });
            }
            if !db
                .is_holder(&file_id, &self.identity.device_id)
                .map_err(EngineError::internal)?
            {
                return Err(EngineError::NotHeldHere { file_id });
            }
        }

        let outcome = self
            .indexer()
            .delete_local_copy(&file_id, true)
            .map_err(EngineError::internal)?;

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
    pub fn pending_delete_requests(&self) -> Vec<DeleteRequest> {
        let db = self.db.lock().unwrap();
        db.get_delete_requests().unwrap_or_default()
    }

    /// Items that have been moved aside. Their bytes are still on whichever
    /// devices held them, so restoring is possible until they are purged.
    pub fn trashed_files(&self) -> Vec<FileMetadata> {
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
            if db
                .get_file_by_id(&file_id)
                .map_err(EngineError::internal)?
                .is_none()
            {
                return Err(EngineError::NoSuchItem { file_id });
            }
            db.restore_file(&file_id).map_err(EngineError::internal)?;
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
                .map_err(EngineError::internal)?
                .ok_or_else(|| EngineError::NoSuchItem { file_id: file_id.clone() })?;
            if !file.is_trashed() {
                return Err(EngineError::NotTrashed { file_id: file_id.clone() });
            }
        }

        self.indexer().purge(&file_id).map_err(EngineError::internal)?;
        let _ = self.event_tx.send(EngineEvent::FilePurged { file_id });
        Ok(())
    }

    /// How long an item has left in trash, in seconds, or None if it is live.
    pub fn trash_seconds_remaining(&self, file_id: String) -> Option<i64> {
        let db = self.db.lock().unwrap();
        let file = db.get_file_by_id(&file_id).ok().flatten()?;
        if !file.is_trashed() {
            return None;
        }
        Some((file.trashed_at + TRASH_RETENTION_SECS - watcher::now_secs()).max(0))
    }

    /// Which devices hold this item, and which content each one has.
    pub fn holders_of(&self, file_id: String) -> Vec<FileHolder> {
        let db = self.db.lock().unwrap();
        db.get_holders(&file_id).unwrap_or_default()
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
            return Err(EngineError::NothingSelected);
        }

        let (file, blocks) = {
            let db = self.db.lock().unwrap();
            let file = db
                .get_file_by_id(&file_id)
                .map_err(EngineError::internal)?
                .ok_or_else(|| EngineError::NoSuchItem { file_id: file_id.clone() })?;
            let blocks = db.get_blocks_for_file(&file_id).unwrap_or_default();
            (file, blocks)
        };

        if blocks.is_empty() || !blocks.iter().all(|b| b.is_present == 1) {
            return Err(EngineError::NotHeldHere { file_id: file_id.clone() });
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
            return Err(EngineError::NoUsableDevices {
                reason: "None of those devices are paired and reachable".into(),
            });
        }

        let storage = self.storage_dir.clone();
        let cert = self.identity.cert_pem.clone();
        let key = self.identity.key_pem.clone();
        let tx = self.event_tx.clone();

        self.runtime.spawn(async move {
            // Nothing has been sent anywhere yet, so one report per target
            // rather than a single anonymous one - each is a row in the UI that
            // has to stop showing as in progress.
            let fail_every_target = |reason: String| {
                for (device_id, _) in &targets {
                    let _ = tx.send(EngineEvent::ShareFailed {
                        file_id: file.id.clone(),
                        path: file.path.clone(),
                        device_id: device_id.clone(),
                        reason: reason.clone(),
                    });
                }
            };

            let trusted_certs = match storage::load_all_trusted_certs(&storage) {
                Ok(c) => c,
                Err(e) => {
                    fail_every_target(e.to_string());
                    return;
                }
            };
            let client = match discovery::build_mtls_client(&cert, &key, &trusted_certs) {
                Ok(c) => c,
                Err(e) => {
                    fail_every_target(e.to_string());
                    return;
                }
            };

            let announce = serde_json::json!({ "file": file, "blocks": blocks });

            for (device_id, url) in &targets {
                let failed = |reason: String| {
                    let _ = tx.send(EngineEvent::ShareFailed {
                        file_id: file.id.clone(),
                        path: file.path.clone(),
                        device_id: device_id.clone(),
                        reason,
                    });
                };

                let announced = client
                    .post(format!("{}/push_metadata", url))
                    .json(&announce)
                    .send()
                    .await;

                match announced {
                    Ok(r) if r.status().is_success() => {}
                    Ok(r) => {
                        failed(format!("It refused the item: {}", r.status()));
                        continue;
                    }
                    Err(e) => {
                        failed(format!("Could not reach it: {}", e));
                        continue;
                    }
                }

                if !discovery::push_blocks_to_peer(&client, url, &blocks, &storage).await {
                    failed("The transfer did not finish".to_string());
                    continue;
                }

                // The recipient claims its holder row on finalize; until then it
                // has the blocks but is not recorded as holding the item.
                let _ = client
                    .post(format!("{}/finalize_file/{}", url, file.id))
                    .send()
                    .await;

                let _ = tx.send(EngineEvent::FileSent {
                    file_id: file.id.clone(),
                    path: file.path.clone(),
                    device_id: device_id.clone(),
                });
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

        let std_listener = std::net::TcpListener::bind("0.0.0.0:0").map_err(EngineError::internal)?;
        let port = std_listener.local_addr().map_err(EngineError::internal)?.port();
        std_listener.set_nonblocking(true).map_err(EngineError::internal)?;
        let listener = TcpListener::from_std(std_listener).map_err(EngineError::internal)?;

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
                let _ = server_tx_err.send(EngineEvent::EngineFailed {
                    reason: format!("Server error: {}", e),
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
        ).map_err(EngineError::internal)?;

        *self.mdns_daemon.lock().unwrap() = Some(daemon);

        let sync_task = self.spawn_catalog_sync(nudge_rx);
        if let Some(previous) = self.catalog_sync_task.lock().unwrap().replace(sync_task) {
            // Belt and braces: `stop` aborts it, but starting twice without a
            // stop in between must not leave two tasks syncing in parallel.
            previous.abort();
        }

        // Desktop keeps the folder and the catalog in step by watching the
        // folder. Mobile has no folder to watch and uses `import_file`.
        #[cfg(desktop)]
        {
            let watcher = watcher::start_watcher(
                self.indexer(),
                handle,
                self.ignore_set.clone(),
                self.event_tx.clone(),
            )
            .map_err(EngineError::internal)?;

            *self.watcher.lock().unwrap() = Some(watcher);
        }
        #[cfg(not(desktop))]
        let _ = handle;

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
        #[cfg(desktop)]
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