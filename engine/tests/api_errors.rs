//! What the engine says when it is asked to do something it cannot.
//!
//! These are the contract an application binds against. Once bindings exist,
//! every one of these variants is a typed exception in Swift and Kotlin, and app
//! code branches on it - so a failure quietly turning into a different variant,
//! or collapsing into `Internal`, breaks callers without breaking the build.
//! Each test names the misuse and the variant it must produce.

use localcloud::{Engine, EngineError};
use tempfile::TempDir;

fn engine(dir: &TempDir) -> Engine {
    let base = dir.path().to_string_lossy().to_string();
    Engine::new(base.clone(), format!("{}/sync", base)).expect("engine")
}

#[test]
fn asking_about_an_item_that_does_not_exist() {
    let dir = TempDir::new().expect("temp dir");
    let engine = engine(&dir);

    assert!(matches!(
        engine.pull_copy("nothing".into()),
        Err(EngineError::NoSuchItem { .. })
    ));
    assert!(matches!(
        engine.delete_local_copy("nothing".into()),
        Err(EngineError::NoSuchItem { .. })
    ));
    assert!(matches!(
        engine.restore_file("nothing".into()),
        Err(EngineError::NoSuchItem { .. })
    ));
    assert!(matches!(
        engine.delete_permanently("nothing".into()),
        Err(EngineError::NoSuchItem { .. })
    ));
}

#[test]
fn selecting_no_devices_at_all() {
    let dir = TempDir::new().expect("temp dir");
    let engine = engine(&dir);

    assert!(matches!(
        engine.start_pairing(vec![]),
        Err(EngineError::NothingSelected)
    ));
    assert!(matches!(
        engine.share_to("anything".into(), vec![]),
        Err(EngineError::NothingSelected)
    ));
}

#[test]
fn selecting_devices_that_cannot_be_used() {
    let dir = TempDir::new().expect("temp dir");
    let engine = engine(&dir);

    // Nothing has been discovered, so no id can name a visible device. This is
    // distinct from selecting none at all: the caller did choose, and needs to
    // be told the choice cannot be acted on rather than that it was empty.
    assert!(matches!(
        engine.start_pairing(vec!["ghost".into()]),
        Err(EngineError::NoUsableDevices { .. })
    ));
}

#[test]
fn deleting_a_copy_from_a_device_that_is_not_paired() {
    let dir = TempDir::new().expect("temp dir");
    let engine = engine(&dir);

    assert!(matches!(
        engine.delete_copy("anything".into(), "a-stranger".into()),
        Err(EngineError::NotPaired { .. })
    ));
}

#[test]
fn entering_a_code_for_a_device_that_never_asked() {
    let dir = TempDir::new().expect("temp dir");
    let engine = engine(&dir);

    assert!(matches!(
        engine.confirm_pairing("a-stranger".into(), "123456".into()),
        Err(EngineError::Pairing { .. })
    ));
}

#[test]
fn settling_a_collision_that_is_not_waiting() {
    let dir = TempDir::new().expect("temp dir");
    let engine = engine(&dir);

    assert!(matches!(
        engine.resolve_collision("no-such-collision".into(), localcloud::CollisionResolution::KeepBoth),
        Err(EngineError::NoSuchCollision { .. })
    ));
}

#[test]
fn a_failure_carries_the_thing_it_was_about() {
    // The variant tells an application what happened; the fields tell it which
    // row to put the message next to.
    let dir = TempDir::new().expect("temp dir");
    let engine = engine(&dir);

    match engine.delete_local_copy("the-item".into()) {
        Err(EngineError::NoSuchItem { file_id }) => assert_eq!(file_id, "the-item"),
        other => panic!("expected NoSuchItem, got {:?}", other),
    }

    match engine.delete_copy("the-item".into(), "the-device".into()) {
        Err(EngineError::NotPaired { device_id }) => assert_eq!(device_id, "the-device"),
        other => panic!("expected NotPaired, got {:?}", other),
    }
}

#[test]
fn messages_are_for_people_and_variants_are_for_code() {
    let dir = TempDir::new().expect("temp dir");
    let engine = engine(&dir);

    let message = engine.delete_local_copy("nothing".into()).unwrap_err().to_string();
    assert!(
        !message.is_empty() && !message.contains("nothing"),
        "a displayed message should read as a sentence, not leak an id: {:?}",
        message
    );
}
