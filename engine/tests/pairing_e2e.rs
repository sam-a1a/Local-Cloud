//! End-to-end pairing over a real TLS listener.
//!
//! These exercise what the unit tests in `pairing.rs` cannot: that an unpaired
//! device is actually refused by the running server, that the 6-digit exchange
//! survives the wire, and that a completed pairing grants access without a
//! restart.

use localcloud::db::Database;
use localcloud::pairing::{self, DeviceInfo, PairingState};
use localcloud::tls::TrustStore;
use localcloud::{server, storage, DeviceIdentity};
use std::sync::{mpsc, Arc, Mutex};
use tempfile::TempDir;
use tokio::net::TcpListener;

fn install_crypto_provider() {
    // Harmless if another test got here first.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

struct TestDevice {
    info: DeviceInfo,
    key_pem: String,
    url: String,
    trust: TrustStore,
    pairing: PairingState,
    db: Arc<Mutex<Database>>,
    storage_dir: String,
    sync_dir: String,
    _dir: TempDir,
}

impl TestDevice {
    /// Brings up a device with its own identity, database and TLS listener on
    /// an ephemeral loopback port.
    async fn start() -> Self {
        let dir = TempDir::new().expect("temp dir");
        let base = dir.path().to_string_lossy().to_string();

        let identity = DeviceIdentity::load_or_generate(&base).expect("identity");
        let storage_dir = format!("{}/storage", base);
        storage::ensure_storage_dir(&storage_dir).expect("storage dir");
        storage::ensure_trusted_peers_dir(&storage_dir).expect("trusted peers dir");

        let sync_dir = format!("{}/sync", base);
        std::fs::create_dir_all(&sync_dir).expect("sync dir");
        let ignore_set = localcloud::new_ignore_set();

        let db = Arc::new(Mutex::new(
            Database::init(&format!("{}/test.db", base)).expect("db"),
        ));

        let trust = TrustStore::new();
        trust.reload(&storage_dir).expect("trust reload");
        let pairing_state = PairingState::new();

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();

        let (event_tx, _event_rx) = mpsc::channel();
        // The receiver is dropped, so sends fail silently; the engine already
        // ignores send errors everywhere.

        let info = DeviceInfo {
            device_id: identity.device_id.clone(),
            name: identity.device_name.clone(),
            platform: "test".to_string(),
            cert_pem: identity.cert_pem.clone(),
        };

        tokio::spawn(server::start_server(
            listener,
            identity.device_id.clone(),
            identity.device_name.clone(),
            identity.cert_pem.clone(),
            identity.key_pem.clone(),
            db.clone(),
            storage_dir.clone(),
            sync_dir.clone(),
            ignore_set.clone(),
            trust.clone(),
            pairing_state.clone(),
            localcloud::watcher::Indexer::new(
                db.clone(),
                storage_dir.clone(),
                sync_dir.clone(),
                identity.device_id.clone(),
                ignore_set,
                localcloud::CollisionQueue::new(),
            ),
            event_tx,
        ));

        // Give the listener a moment to start accepting.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        Self {
            info,
            key_pem: identity.key_pem.clone(),
            url: format!("https://127.0.0.1:{}", port),
            trust,
            pairing: pairing_state,
            db,
            storage_dir,
            sync_dir,
            _dir: dir,
        }
    }

    /// A client authenticated with this device's certificate, trusting `peer`.
    fn mtls_client_for(&self, peer: &TestDevice) -> reqwest::Client {
        localcloud::discovery::build_mtls_client(
            &self.info.cert_pem,
            &self.key_pem,
            &[peer.info.cert_pem.clone()],
        )
        .expect("mtls client")
    }
}

#[tokio::test]
async fn unpaired_device_reaches_pairing_but_nothing_else() {
    install_crypto_provider();
    let device = TestDevice::start().await;
    let anon = pairing::build_pairing_client().expect("client");

    let hello = anon
        .get(format!("{}/hello", device.url))
        .send()
        .await
        .expect("hello reachable");
    assert_eq!(hello.status(), 200, "pairing bootstrap must be reachable");

    let catalog = anon
        .get(format!("{}/catalog", device.url))
        .send()
        .await
        .expect("request completes");
    assert_eq!(
        catalog.status(),
        403,
        "an unpaired device must not read the catalog"
    );

    let block = anon
        .get(format!("{}/get_block/deadbeef", device.url))
        .send()
        .await
        .expect("request completes");
    assert_eq!(
        block.status(),
        403,
        "an unpaired device must not read block storage"
    );
}

#[tokio::test]
async fn correct_code_pairs_and_grants_access() {
    install_crypto_provider();
    let initiator = TestDevice::start().await;
    let target = TestDevice::start().await;
    let anon = pairing::build_pairing_client().expect("client");

    // Initiator selects the target and displays a code.
    let code = initiator
        .pairing
        .begin(vec![target.info.device_id.clone()]);

    // It announces itself so the target can prompt for that code.
    pairing::send_pair_request(&anon, &target.url, &initiator.info)
        .await
        .expect("pair request delivered");
    assert_eq!(target.pairing.offers().len(), 1, "target should prompt");

    // The user types the code on the target, which proves knowledge of it.
    let proof = pairing::pairing_proof(&code, &initiator.info.cert_pem, &target.info.cert_pem);
    let peer = pairing::send_pair_confirm(&anon, &initiator.url, &target.info, &proof)
        .await
        .expect("pairing accepted");
    assert_eq!(peer.device_id, initiator.info.device_id);

    // The initiator pinned the target, so mutually-authenticated calls work.
    assert!(!initiator.trust.is_empty(), "initiator should now trust someone");

    server::pin_paired_device(&target.db, &target.storage_dir, &target.trust, &peer)
        .expect("target pins initiator");

    let client = target.mtls_client_for(&initiator);
    let catalog = client
        .get(format!("{}/catalog", initiator.url))
        .send()
        .await
        .expect("request completes");
    assert_eq!(
        catalog.status(),
        200,
        "a paired device must be able to read the catalog"
    );
}

#[tokio::test]
async fn wrong_code_is_refused_and_grants_nothing() {
    install_crypto_provider();
    let initiator = TestDevice::start().await;
    let target = TestDevice::start().await;
    let anon = pairing::build_pairing_client().expect("client");

    initiator
        .pairing
        .begin(vec![target.info.device_id.clone()]);

    let wrong = pairing::pairing_proof(
        "000000-definitely-not-the-code",
        &initiator.info.cert_pem,
        &target.info.cert_pem,
    );

    let result = pairing::send_pair_confirm(&anon, &initiator.url, &target.info, &wrong).await;
    assert!(result.is_err(), "a bad proof must be rejected");
    assert!(
        initiator.trust.is_empty(),
        "a failed pairing must not pin anything"
    );
}

#[tokio::test]
async fn device_not_selected_cannot_pair_itself() {
    install_crypto_provider();
    let initiator = TestDevice::start().await;
    let selected = TestDevice::start().await;
    let intruder = TestDevice::start().await;
    let anon = pairing::build_pairing_client().expect("client");

    // The code is displayed for `selected` only.
    let code = initiator
        .pairing
        .begin(vec![selected.info.device_id.clone()]);

    // An onlooker who somehow learned the code still cannot use it.
    let proof = pairing::pairing_proof(&code, &initiator.info.cert_pem, &intruder.info.cert_pem);
    let result = pairing::send_pair_confirm(&anon, &initiator.url, &intruder.info, &proof).await;

    assert!(result.is_err(), "an unselected device must be refused");
    assert!(initiator.trust.is_empty(), "nothing should have been pinned");
}

/// Runs the full 6-digit exchange and returns a client authenticated as
/// `target` for calls to `initiator`.
async fn pair(initiator: &TestDevice, target: &TestDevice) -> reqwest::Client {
    let anon = pairing::build_pairing_client().expect("client");
    let code = initiator
        .pairing
        .begin(vec![target.info.device_id.clone()]);

    pairing::send_pair_request(&anon, &target.url, &initiator.info)
        .await
        .expect("pair request delivered");

    let proof = pairing::pairing_proof(&code, &initiator.info.cert_pem, &target.info.cert_pem);
    let peer = pairing::send_pair_confirm(&anon, &initiator.url, &target.info, &proof)
        .await
        .expect("pairing accepted");

    server::pin_paired_device(&target.db, &target.storage_dir, &target.trust, &peer)
        .expect("target pins initiator");

    target.mtls_client_for(initiator)
}

/// Pairing grants access to block storage, not to the filesystem around it.
#[tokio::test]
async fn a_paired_device_cannot_push_outside_block_storage() {
    install_crypto_provider();
    let initiator = TestDevice::start().await;
    let target = TestDevice::start().await;
    let client = pair(&initiator, &target).await;

    // Percent-encoded separators are decoded before the handler sees them, so
    // without a check on the shape of an id this arrives as a relative path and
    // is written wherever it points.
    let escaped = client
        .post(format!("{}/push_block/..%2Fpwned", initiator.url))
        .body("payload")
        .send()
        .await
        .expect("request completes");

    assert_eq!(escaped.status(), 400, "a block id that is a path must be refused");
    assert!(
        !std::path::Path::new(&format!("{}/../pwned", initiator.storage_dir)).exists(),
        "nothing may be written outside block storage"
    );

    // The device's private key sits one level above block storage, so this is
    // the read the same trick buys if ids are taken at face value.
    let key_path = format!("{}/../identity.json", initiator.storage_dir);
    assert!(
        std::path::Path::new(&key_path).exists(),
        "the test is pointless unless it names a file that is really there"
    );

    let read_back = client
        .get(format!("{}/get_block/..%2Fidentity.json", initiator.url))
        .send()
        .await
        .expect("request completes");
    assert_eq!(read_back.status(), 404, "and nothing outside it may be read");
}

/// The same check the pull path makes, on the pushing side.
#[tokio::test]
async fn a_paired_device_cannot_push_contents_that_belie_their_id() {
    install_crypto_provider();
    let initiator = TestDevice::start().await;
    let target = TestDevice::start().await;
    let client = pair(&initiator, &target).await;

    let asked_for = storage::block_id_for(b"the block this id names");
    let lied = client
        .post(format!("{}/push_block/{}", initiator.url, asked_for))
        .body("something else entirely")
        .send()
        .await
        .expect("request completes");

    assert_eq!(lied.status(), 400);
    assert!(
        !storage::get_block_path(&initiator.storage_dir, &asked_for).exists(),
        "storage is content-addressed; the id must not be claimable by other bytes"
    );
}

/// The whole transfer path, on a file whose blocks repeat.
///
/// This is the shape that used to arrive corrupt: a manifest was keyed by block
/// rather than by position, so a file made largely of one repeated block lost
/// every repeat. The sender's own copy on disk was untouched, so the damage only
/// ever appeared on the device that received it.
#[tokio::test]
async fn a_file_whose_blocks_repeat_arrives_whole() {
    install_crypto_provider();
    let receiver = TestDevice::start().await;
    let sender = TestDevice::start().await;
    let client = pair(&receiver, &sender).await;

    // Three identical blocks and a tail, as any file with a run of padding has.
    let mut payload = vec![0u8; storage::BLOCK_SIZE * 3];
    payload.extend_from_slice(b"tail");
    let source = format!("{}/padded.bin", sender.sync_dir);
    std::fs::write(&source, &payload).expect("write source");

    let file = localcloud::FileMetadata {
        id: "f1".into(),
        path: "padded.bin".into(),
        size: payload.len() as i64,
        content_hash: String::new(),
        modified_time: 1,
        created_by: sender.info.device_id.clone(),
        trashed_at: 0,
        trashed_by: String::new(),
    };

    let blocks = {
        let db = sender.db.lock().unwrap();
        db.insert_file(&file).expect("record item");
        storage::chunk_and_store_file(&sender.storage_dir, &db, "f1", &source).expect("chunk");
        db.get_blocks_for_file("f1").expect("blocks")
    };
    assert_eq!(blocks.len(), 4, "the manifest must keep every position");

    let announced = client
        .post(format!("{}/push_metadata", receiver.url))
        .json(&serde_json::json!({ "file": file, "blocks": blocks }))
        .send()
        .await
        .expect("request completes");
    assert!(announced.status().is_success());

    assert!(
        localcloud::discovery::push_blocks_to_peer(
            &client,
            &receiver.url,
            &blocks,
            &sender.storage_dir
        )
        .await,
        "every block must transfer"
    );

    let finalized = client
        .post(format!("{}/finalize_file/f1", receiver.url))
        .send()
        .await
        .expect("request completes");
    assert_eq!(
        finalized.status(),
        200,
        "the receiver must consider the item complete"
    );

    assert_eq!(
        std::fs::read(format!("{}/padded.bin", receiver.sync_dir)).expect("received file"),
        payload,
        "the copy must be the whole file, byte for byte"
    );
}
