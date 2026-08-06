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
                sync_dir,
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
