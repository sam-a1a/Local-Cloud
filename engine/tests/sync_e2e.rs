//! Catalog replication between two live devices, over real mutually
//! authenticated TLS.
//!
//! `sync_with_peer` is what makes this a mesh rather than a folder, and it was
//! the one path with no end-to-end coverage: the merge rules underneath it are
//! tested against a database directly, but nothing exercised the exchange over
//! the wire.
//!
//! Pairing is done by pinning both certificates outright rather than running the
//! 6-digit exchange, which `pairing_e2e.rs` already covers. These are about what
//! happens once two devices trust each other.

use localcloud::db::{Database, DeleteRequest, FileHolder};
use localcloud::pairing::{DeviceInfo, PairingState};
use localcloud::tls::TrustStore;
use localcloud::watcher::{IndexOutcome, Indexer};
use localcloud::{
    new_ignore_set, server, storage, CollisionQueue, DeviceIdentity, EngineEvent, IgnoreSet,
};
use std::sync::{mpsc, Arc, Mutex};
use tempfile::TempDir;
use tokio::net::TcpListener;

fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

struct Device {
    info: DeviceInfo,
    key_pem: String,
    url: String,
    db: Arc<Mutex<Database>>,
    storage_dir: String,
    sync_dir: String,
    ignore_set: IgnoreSet,
    collisions: CollisionQueue,
    indexer: Indexer,
    trust: TrustStore,
    event_tx: mpsc::Sender<EngineEvent>,
    events: mpsc::Receiver<EngineEvent>,
    // Kept alive so the server's end of the channel stays open. Nothing here
    // pairs through the server, so nothing is ever sent on it.
    _sync_nudges: tokio::sync::mpsc::UnboundedReceiver<String>,
    _dir: TempDir,
}

impl Device {
    async fn start(name: &str) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let base = dir.path().to_string_lossy().to_string();

        let identity = DeviceIdentity::load_or_generate(&base).expect("identity");
        let storage_dir = format!("{}/storage", base);
        storage::ensure_storage_dir(&storage_dir).expect("storage dir");
        storage::ensure_trusted_peers_dir(&storage_dir).expect("trusted peers dir");

        let sync_dir = format!("{}/sync", base);
        std::fs::create_dir_all(&sync_dir).expect("sync dir");

        let db = Arc::new(Mutex::new(
            Database::init(&format!("{}/{}.db", base, name)).expect("db"),
        ));
        let ignore_set = new_ignore_set();
        let collisions = CollisionQueue::new();
        let trust = TrustStore::new();
        trust.reload(&storage_dir).expect("trust reload");

        let indexer = Indexer::new(
            db.clone(),
            storage_dir.clone(),
            sync_dir.clone(),
            identity.device_id.clone(),
            ignore_set.clone(),
            collisions.clone(),
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let (event_tx, events) = mpsc::channel();
        let (sync_nudge, _sync_nudges) = tokio::sync::mpsc::unbounded_channel();

        let info = DeviceInfo {
            device_id: identity.device_id.clone(),
            name: name.to_string(),
            platform: "test".to_string(),
            cert_pem: identity.cert_pem.clone(),
        };

        tokio::spawn(server::start_server(
            listener,
            identity.device_id.clone(),
            name.to_string(),
            identity.cert_pem.clone(),
            identity.key_pem.clone(),
            db.clone(),
            storage_dir.clone(),
            sync_dir.clone(),
            ignore_set.clone(),
            trust.clone(),
            PairingState::new(),
            indexer.clone(),
            event_tx.clone(),
            sync_nudge,
        ));

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        Self {
            info,
            key_pem: identity.key_pem.clone(),
            url: format!("https://127.0.0.1:{}", port),
            db,
            storage_dir,
            sync_dir,
            ignore_set,
            collisions,
            indexer,
            trust,
            event_tx,
            events,
            _sync_nudges,
            _dir: dir,
        }
    }

    fn id(&self) -> &str {
        &self.info.device_id
    }

    /// The state a completed pairing leaves behind, on both sides.
    fn pair_with(&self, peer: &Device) {
        server::pin_paired_device(&self.db, &self.storage_dir, &self.trust, &peer.info)
            .expect("pin peer");
        server::pin_paired_device(&peer.db, &peer.storage_dir, &peer.trust, &self.info)
            .expect("peer pins us");
    }

    /// Asks `peer` for its view of the catalog and merges it.
    async fn sync_from(&self, peer: &Device) {
        localcloud::discovery::sync_with_peer(
            peer.url.clone(),
            peer.id().to_string(),
            self.id().to_string(),
            self.db.clone(),
            self.storage_dir.clone(),
            self.sync_dir.clone(),
            self.info.cert_pem.clone(),
            self.key_pem.clone(),
            self.ignore_set.clone(),
            self.collisions.clone(),
            self.indexer.clone(),
            self.event_tx.clone(),
        )
        .await;
    }

    /// Puts a file in the sync folder and indexes it, as the watcher would.
    fn add(&self, relative_path: &str, contents: &str) -> String {
        let absolute = format!("{}/{}", self.sync_dir, relative_path);
        if let Some(parent) = std::path::Path::new(&absolute).parent() {
            std::fs::create_dir_all(parent).expect("parent dir");
        }
        std::fs::write(&absolute, contents).expect("write");

        match self.indexer.index(&absolute) {
            IndexOutcome::Indexed { file_id, .. } => file_id,
            other => panic!("expected the file to be indexed, got {:?}", other),
        }
    }

    fn holder_ids(&self, file_id: &str) -> Vec<String> {
        let db = self.db.lock().unwrap();
        let mut ids: Vec<String> = db
            .get_holders(file_id)
            .expect("holders")
            .into_iter()
            .map(|h| h.device_id)
            .collect();
        ids.sort();
        ids
    }

    fn sees(&self, path: &str) -> bool {
        self.db
            .lock()
            .unwrap()
            .get_file_by_path(path)
            .expect("query")
            .is_some()
    }

    fn exists_on_disk(&self, relative_path: &str) -> bool {
        std::path::Path::new(&format!("{}/{}", self.sync_dir, relative_path)).exists()
    }

    fn drain_events(&self) -> Vec<EngineEvent> {
        self.events.try_iter().collect()
    }
}

/// The base case: an item one device holds becomes visible on the other,
/// without its bytes moving.
#[tokio::test]
async fn a_peers_catalog_arrives_without_its_data() {
    install_crypto_provider();
    let alice = Device::start("alice").await;
    let bob = Device::start("bob").await;
    alice.pair_with(&bob);

    let file_id = bob.add("notes.txt", "bob's notes");
    alice.sync_from(&bob).await;

    assert!(alice.sees("notes.txt"), "the item must appear in the catalog");
    assert_eq!(
        alice.holder_ids(&file_id),
        vec![bob.id().to_string()],
        "bob holds it and alice does not"
    );
    assert!(
        !alice.exists_on_disk("notes.txt"),
        "seeing an item must not put its bytes in the folder"
    );
}

/// Trust comes from pairing and nowhere else.
#[tokio::test]
async fn an_unpaired_peer_is_not_synced_with() {
    install_crypto_provider();
    let alice = Device::start("alice").await;
    let stranger = Device::start("stranger").await;
    // Deliberately no pairing.

    stranger.add("secrets.txt", "not for alice");
    alice.sync_from(&stranger).await;

    assert!(
        !alice.sees("secrets.txt"),
        "an unpaired device's catalog must not be merged"
    );
}

/// The peer is the sole authority on its own copies, so its rows replace ours
/// wholesale - that is how a copy it deleted stops being listed.
#[tokio::test]
async fn a_copy_the_peer_deleted_stops_being_listed() {
    install_crypto_provider();
    let alice = Device::start("alice").await;
    let bob = Device::start("bob").await;
    alice.pair_with(&bob);

    let file_id = bob.add("notes.txt", "bob's notes");
    alice.sync_from(&bob).await;
    assert_eq!(alice.holder_ids(&file_id), vec![bob.id().to_string()]);

    // Alice takes a copy too - and bob has to know about it. Deleting what a
    // device believes is the last copy sends the item to trash and keeps the
    // holder row, because an item nobody holds could not be restored.
    for db in [&alice.db, &bob.db] {
        db.lock()
            .unwrap()
            .set_holder(&FileHolder {
                file_id: file_id.clone(),
                device_id: alice.id().to_string(),
                content_hash: "alice's copy".into(),
                received_at: 1,
            })
            .expect("alice claims a copy");
    }
    bob.indexer
        .delete_local_copy(&file_id, true)
        .expect("bob drops his copy");

    alice.sync_from(&bob).await;

    assert_eq!(
        alice.holder_ids(&file_id),
        vec![alice.id().to_string()],
        "bob's row must be gone, and alice's own left alone"
    );
}

/// A peer reports on third devices too, which is how a copy on something
/// currently unreachable is discovered.
#[tokio::test]
async fn a_third_devices_copy_is_learned_through_a_peer() {
    install_crypto_provider();
    let alice = Device::start("alice").await;
    let bob = Device::start("bob").await;
    let carol = Device::start("carol").await;
    alice.pair_with(&bob);
    bob.pair_with(&carol);

    let file_id = carol.add("shared.txt", "carol's file");
    bob.sync_from(&carol).await;
    // Alice has never spoken to carol.
    alice.sync_from(&bob).await;

    assert!(alice.sees("shared.txt"));
    assert!(
        alice.holder_ids(&file_id).contains(&carol.id().to_string()),
        "alice should learn carol holds it, through bob"
    );
}

/// What a peer says about *us* is never authoritative.
#[tokio::test]
async fn a_peer_cannot_rewrite_what_we_know_about_our_own_copies() {
    install_crypto_provider();
    let alice = Device::start("alice").await;
    let bob = Device::start("bob").await;
    alice.pair_with(&bob);

    let file_id = alice.add("mine.txt", "alice's file");
    bob.sync_from(&alice).await;

    // Bob's catalog now claims alice holds content that she does not.
    {
        let db = bob.db.lock().unwrap();
        db.set_holder(&FileHolder {
            file_id: file_id.clone(),
            device_id: alice.id().to_string(),
            content_hash: "a hash alice never had".into(),
            received_at: 999,
        })
        .expect("bob records a claim about alice");
    }

    alice.sync_from(&bob).await;

    let mine: Vec<FileHolder> = {
        let db = alice.db.lock().unwrap();
        db.get_holders(&file_id)
            .expect("holders")
            .into_iter()
            .filter(|h| h.device_id == alice.id())
            .collect()
    };
    assert_eq!(mine.len(), 1);
    assert_ne!(
        mine[0].content_hash, "a hash alice never had",
        "only this device may write its own holder row"
    );
}

/// A device that missed a destruction would otherwise hand the item straight
/// back on the next sync.
#[tokio::test]
async fn a_destroyed_item_is_not_reintroduced_by_a_stale_peer() {
    install_crypto_provider();
    let alice = Device::start("alice").await;
    let bob = Device::start("bob").await;
    alice.pair_with(&bob);

    let file_id = bob.add("doomed.txt", "gone soon");
    alice.sync_from(&bob).await;
    assert!(alice.sees("doomed.txt"));

    // Alice learns it was destroyed. Bob never hears about it.
    {
        let db = alice.db.lock().unwrap();
        db.purge_file(&file_id, alice.id(), 100)
            .expect("destroy the item, leaving a tombstone");
    }
    assert!(!alice.sees("doomed.txt"));

    alice.sync_from(&bob).await;

    assert!(
        !alice.sees("doomed.txt"),
        "a stale catalog must not resurrect a destroyed item"
    );
    assert!(
        alice.holder_ids(&file_id).is_empty(),
        "nor leave holder rows pointing at it"
    );
}

/// Deletion has to survive the target being away, so a request travels in the
/// catalog and is carried out whenever the target next syncs.
#[tokio::test]
async fn a_delete_request_aimed_at_us_is_carried_out_on_sync() {
    install_crypto_provider();
    let alice = Device::start("alice").await;
    let bob = Device::start("bob").await;
    alice.pair_with(&bob);

    let file_id = alice.add("shared.txt", "alice's copy");
    bob.sync_from(&alice).await;

    // Bob holds a copy too, so alice deleting hers is not the last one.
    {
        let db = bob.db.lock().unwrap();
        db.set_holder(&FileHolder {
            file_id: file_id.clone(),
            device_id: bob.id().to_string(),
            content_hash: "bob's copy".into(),
            received_at: 1,
        })
        .expect("bob holds it");
        db.record_delete_request(&DeleteRequest {
            file_id: file_id.clone(),
            target_device: alice.id().to_string(),
            requested_by: bob.id().to_string(),
            requested_at: 5,
        })
        .expect("bob asks alice to drop hers");
    }

    let _ = alice.drain_events();
    alice.sync_from(&bob).await;

    assert!(
        !alice.exists_on_disk("shared.txt"),
        "the copy must actually leave the folder"
    );
    assert!(
        !alice.holder_ids(&file_id).contains(&alice.id().to_string()),
        "and alice must stop being listed as a holder"
    );
    assert!(
        alice
            .drain_events()
            .iter()
            .any(|e| matches!(e, EngineEvent::CopyDeleted { .. })),
        "the deletion must be announced"
    );
}

/// Two devices can name the same thing while apart. The tie-break settles it,
/// and the folder has to follow the catalog.
#[tokio::test]
async fn a_name_claimed_on_both_devices_is_settled_and_renamed_on_disk() {
    install_crypto_provider();
    let alice = Device::start("alice").await;
    let bob = Device::start("bob").await;
    alice.pair_with(&bob);

    // Both create "report.txt" independently, each holding its own.
    let alice_file = alice.add("report.txt", "alice's report");
    let bob_file = bob.add("report.txt", "bob's report");

    // The tie-break is on file id, lower wins. Only the device that holds the
    // item which *loses* has anything to rename on disk, so sync from that side
    // - otherwise the losing item belongs to the peer and no local file moves.
    let (loser, loser_content) = if alice_file > bob_file {
        (&alice, "alice's report")
    } else {
        (&bob, "bob's report")
    };
    let winner = if alice_file > bob_file { &bob } else { &alice };

    let _ = loser.drain_events();
    loser.sync_from(winner).await;

    assert!(
        loser
            .drain_events()
            .iter()
            .any(|e| matches!(e, EngineEvent::NameCollision { .. })),
        "the contested name must be announced"
    );
    assert!(
        !loser.collisions.pending().is_empty(),
        "and queued for a person to settle"
    );

    // The folder must agree with the catalog: this device gave up the name, so
    // its own content moved aside and nothing of its own sits at the old name.
    assert!(
        loser.exists_on_disk("report 1.txt"),
        "the item that lost the name must be renamed on disk, not left in place"
    );
    assert_eq!(
        std::fs::read_to_string(format!("{}/report 1.txt", loser.sync_dir)).expect("moved file"),
        loser_content,
        "and it must be this device's own content that moved"
    );

    // Both items survive, under distinct names.
    let paths: std::collections::HashSet<String> = {
        let db = loser.db.lock().unwrap();
        db.get_catalog_files()
            .expect("files")
            .into_iter()
            .map(|f| f.path)
            .collect()
    };
    assert_eq!(
        paths,
        ["report.txt".to_string(), "report 1.txt".to_string()]
            .into_iter()
            .collect(),
        "both items must survive, and not both claim the same name"
    );
}

/// Syncing is also how a device records that a peer is currently around.
#[tokio::test]
async fn syncing_records_the_peer_as_seen() {
    install_crypto_provider();
    let alice = Device::start("alice").await;
    let bob = Device::start("bob").await;
    alice.pair_with(&bob);

    let before = {
        let db = alice.db.lock().unwrap();
        db.get_paired_devices().expect("devices")[0].last_seen
    };

    alice.sync_from(&bob).await;

    let after = {
        let db = alice.db.lock().unwrap();
        db.get_paired_devices().expect("devices")[0].last_seen
    };
    assert!(
        after >= before,
        "last_seen must move forward when a peer answers"
    );
}
