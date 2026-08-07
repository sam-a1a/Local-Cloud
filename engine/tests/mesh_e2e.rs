//! Two whole engines on one machine: discovery, pairing, and a catalog that
//! converges without anyone asking it to.
//!
//! Everything else in the suite drives one layer with the rest stubbed or
//! called by hand. This drives `Engine` exactly as an application would - start
//! it, pair, drop a file in a folder - and asserts the other device ends up
//! knowing about it. Nothing here calls `sync_with_peer`; that is the point.
//!
//! It uses real mDNS on the loopback/LAN interfaces and the real filesystem
//! watcher, so it is slower and more environment-dependent than the rest. It
//! polls with generous deadlines rather than sleeping for a fixed time.

use localcloud::Engine;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// How long to let the network settle before calling a step failed. Discovery
/// is the slow part: mDNS announces and resolves on its own schedule.
const DEADLINE: Duration = Duration::from_secs(30);

fn start_engine(dir: &TempDir) -> Engine {
    let base = dir.path().to_string_lossy().to_string();
    let engine = Engine::new(base.clone(), format!("{}/sync", base)).expect("engine");
    engine.start().expect("start");
    engine
}

/// Polls until `check` returns something, or the deadline passes.
fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(value) = check() {
            return value;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for {}", what);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn two_engines_discover_pair_and_converge_on_their_own() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let alice_dir = TempDir::new().expect("temp dir");
    let bob_dir = TempDir::new().expect("temp dir");
    let alice = start_engine(&alice_dir);
    let bob = start_engine(&bob_dir);

    let alice_id = alice.device_id();
    let bob_id = bob.device_id();

    // Bob has something to share before the two ever meet, so what is being
    // measured is the pairing itself prompting a read rather than the periodic
    // pass coming round. `converges_on_a_change_made_after_pairing` covers the
    // other trigger.
    std::fs::write(format!("{}/notes.txt", bob.get_sync_dir()), "bob's notes")
        .expect("write into bob's folder");

    // 1. They find each other over mDNS.
    wait_for("alice to see bob", || {
        alice
            .get_known_peers()
            .into_iter()
            .find(|d| d.device_id == bob_id)
    });
    wait_for("bob to see alice", || {
        bob.get_known_peers()
            .into_iter()
            .find(|d| d.device_id == alice_id)
    });

    // 2. Alice starts pairing and bob enters the code, as two people would.
    let code = alice
        .start_pairing(vec![bob_id.clone()])
        .expect("pairing starts");

    wait_for("bob to be asked for the code", || {
        bob.pairing_offers()
            .into_iter()
            .find(|o| o.device_id == alice_id)
    });
    bob.confirm_pairing(alice_id.clone(), code)
        .expect("code accepted");

    wait_for("both to record the pairing", || {
        let alice_paired = alice.paired_devices().iter().any(|d| d.id == bob_id);
        let bob_paired = bob.paired_devices().iter().any(|d| d.id == alice_id);
        (alice_paired && bob_paired).then_some(())
    });

    // 3. Bob already had a file in his folder. The watcher indexed it; nothing
    //    else is called on either engine from here on.
    let file = wait_for("bob to index his own file", || {
        bob.get_local_files()
            .into_iter()
            .find(|f| f.path == "notes.txt")
    });

    // 4. Alice learns of it without being told to sync.
    let seen = wait_for("alice's catalog to catch up", || {
        alice
            .get_catalog()
            .items
            .into_iter()
            .find(|f| f.path == "notes.txt")
    });
    assert_eq!(seen.id, file.id, "it must be the same item, not a new one");

    // And she knows who has it, and that she does not.
    let holders = alice.get_file_holders(&file.id);
    assert!(
        holders.iter().any(|h| h.device_id == bob_id),
        "bob must be listed as holding it"
    );
    assert!(
        !holders.iter().any(|h| h.device_id == alice_id),
        "seeing an item is not having it"
    );
    assert!(
        !std::path::Path::new(&format!("{}/notes.txt", alice.get_sync_dir())).exists(),
        "and its bytes must not appear in her folder"
    );

    alice.stop();
    bob.stop();
}

/// Steady state: a file added long after pairing still reaches the other device.
///
/// Ignored by default because it can only be as fast as the periodic pass, and
/// waiting half a minute on every `cargo test` is not worth it. Run it with
/// `cargo test -p engine --test mesh_e2e -- --ignored`.
///
/// The delay is inherent to the design rather than a shortcoming of the test.
/// Catalog replication is pull-only - each device asks its peers what they know
/// - so nothing tells a peer that something has changed, and propagation can
/// take as long as `CATALOG_SYNC_INTERVAL_SECS`.
#[test]
#[ignore]
fn converges_on_a_change_made_after_pairing() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let alice_dir = TempDir::new().expect("temp dir");
    let bob_dir = TempDir::new().expect("temp dir");
    let alice = start_engine(&alice_dir);
    let bob = start_engine(&bob_dir);

    let alice_id = alice.device_id();
    let bob_id = bob.device_id();

    wait_for("alice to see bob", || {
        alice
            .get_known_peers()
            .into_iter()
            .find(|d| d.device_id == bob_id)
    });
    wait_for("bob to see alice", || {
        bob.get_known_peers()
            .into_iter()
            .find(|d| d.device_id == alice_id)
    });

    let code = alice
        .start_pairing(vec![bob_id.clone()])
        .expect("pairing starts");
    wait_for("bob to be asked for the code", || {
        bob.pairing_offers()
            .into_iter()
            .find(|o| o.device_id == alice_id)
    });
    bob.confirm_pairing(alice_id.clone(), code)
        .expect("code accepted");
    wait_for("both to record the pairing", || {
        let alice_paired = alice.paired_devices().iter().any(|d| d.id == bob_id);
        let bob_paired = bob.paired_devices().iter().any(|d| d.id == alice_id);
        (alice_paired && bob_paired).then_some(())
    });

    // Only now does bob add anything, so no pairing or discovery event is left
    // to prompt a read. The periodic pass is the only thing that can.
    std::fs::write(format!("{}/later.txt", bob.get_sync_dir()), "added afterwards")
        .expect("write into bob's folder");

    wait_for("alice's catalog to catch up on its own", || {
        alice
            .get_catalog()
            .items
            .into_iter()
            .find(|f| f.path == "later.txt")
    });

    alice.stop();
    bob.stop();
}

/// An engine that has been stopped and started again is still a working engine.
///
/// Mobile makes this the normal case rather than an edge one: an app is
/// backgrounded and resumed, and is expected to stop and restart the engine
/// around that. Anything `start` sets up has to survive being torn down once.
#[test]
fn an_engine_restarted_still_syncs() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let alice_dir = TempDir::new().expect("temp dir");
    let bob_dir = TempDir::new().expect("temp dir");
    let alice = start_engine(&alice_dir);
    let bob = start_engine(&bob_dir);

    std::fs::write(format!("{}/notes.txt", bob.get_sync_dir()), "bob's notes")
        .expect("write into bob's folder");

    // Backgrounded and resumed, before the two have ever paired.
    alice.stop();
    alice.start().expect("restart");

    let alice_id = alice.device_id();
    let bob_id = bob.device_id();

    wait_for("alice to see bob again", || {
        alice
            .get_known_peers()
            .into_iter()
            .find(|d| d.device_id == bob_id)
    });
    wait_for("bob to see alice", || {
        bob.get_known_peers()
            .into_iter()
            .find(|d| d.device_id == alice_id)
    });

    let code = alice
        .start_pairing(vec![bob_id.clone()])
        .expect("pairing starts");
    wait_for("bob to be asked for the code", || {
        bob.pairing_offers()
            .into_iter()
            .find(|o| o.device_id == alice_id)
    });
    bob.confirm_pairing(alice_id.clone(), code)
        .expect("code accepted");

    wait_for("alice's catalog to catch up after a restart", || {
        alice
            .get_catalog()
            .items
            .into_iter()
            .find(|f| f.path == "notes.txt")
    });

    alice.stop();
    bob.stop();
}
