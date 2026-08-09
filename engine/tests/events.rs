//! How events reach an application, and what they say when they get there.

use localcloud::{Engine, EngineEvent, EventListener};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Collects everything it is given, as an application's listener would.
#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<EngineEvent>>,
}

impl EventListener for Recorder {
    fn on_event(&self, event: EngineEvent) {
        self.seen.lock().unwrap().push(event);
    }
}

impl Recorder {
    fn events(&self) -> Vec<EngineEvent> {
        self.seen.lock().unwrap().clone()
    }

    /// Waits for an event the predicate accepts. Dispatch is on its own thread,
    /// so an assertion made immediately would be racing it.
    fn wait_for(&self, what: &str, matches: impl Fn(&EngineEvent) -> bool) -> EngineEvent {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(found) = self.events().into_iter().find(|e| matches(e)) {
                return found;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {}", what);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn engine(dir: &TempDir) -> Engine {
    let base = dir.path().to_string_lossy().to_string();
    Engine::new(base.clone(), format!("{}/sync", base)).expect("engine")
}

#[test]
fn a_listener_set_before_start_sees_the_engine_start() {
    let dir = TempDir::new().expect("temp dir");
    let engine = engine(&dir);
    let recorder = Arc::new(Recorder::default());

    engine.set_event_listener(recorder.clone());
    engine.start().expect("start");

    recorder.wait_for("EngineStarted", |e| {
        matches!(e, EngineEvent::EngineStarted)
    });
    engine.stop();
}

#[test]
fn events_produced_before_a_listener_existed_are_not_lost() {
    // An application that registers a moment late still gets what it missed,
    // in order. Losing EngineStarted would be the common case otherwise.
    let dir = TempDir::new().expect("temp dir");
    let engine = engine(&dir);

    engine.start().expect("start");
    std::thread::sleep(Duration::from_millis(100));

    let recorder = Arc::new(Recorder::default());
    engine.set_event_listener(recorder.clone());

    recorder.wait_for("the backlog to arrive", |e| {
        matches!(e, EngineEvent::EngineStarted)
    });
    engine.stop();
}

#[test]
fn more_than_one_listener_can_be_used_over_a_lifetime() {
    // Replacing a listener is how an application hands over between screens.
    let dir = TempDir::new().expect("temp dir");
    let engine = engine(&dir);

    let first = Arc::new(Recorder::default());
    engine.set_event_listener(first.clone());
    engine.start().expect("start");
    first.wait_for("the first listener to see the start", |e| {
        matches!(e, EngineEvent::EngineStarted)
    });

    let second = Arc::new(Recorder::default());
    engine.set_event_listener(second.clone());
    engine.stop();

    second.wait_for("the second listener to see the stop", |e| {
        matches!(e, EngineEvent::EngineStopped)
    });
}

#[test]
fn an_indexed_file_is_announced_with_its_identity() {
    // Not only its path: a path is not an identity here, because a collision
    // can rename an item. An application keying on the path would attach the
    // event to the wrong row exactly when it matters.
    let dir = TempDir::new().expect("temp dir");
    let engine = engine(&dir);
    let recorder = Arc::new(Recorder::default());

    engine.set_event_listener(recorder.clone());
    engine.start().expect("start");

    std::fs::write(format!("{}/notes.txt", engine.sync_dir()), "contents")
        .expect("write into the folder");

    let event = recorder.wait_for("the file to be indexed", |e| {
        matches!(e, EngineEvent::FileIndexed { path, .. } if path == "notes.txt")
    });

    match event {
        EngineEvent::FileIndexed { file_id, path } => {
            assert_eq!(path, "notes.txt");
            assert!(!file_id.is_empty(), "an event must carry the item's identity");
            assert!(
                engine.local_files().iter().any(|f| f.id == file_id),
                "and that identity must be the one in the catalog"
            );
        }
        other => panic!("expected FileIndexed, got {:?}", other),
    }

    engine.stop();
}

/// A share aimed at a device that is not there is refused, not reported later.
///
/// This is the dividing line the API draws: anything decidable from local state
/// comes back as a typed error the caller can act on immediately, and only what
/// depends on the network becomes an event. A device that has never been seen is
/// the former.
#[test]
fn a_share_to_a_device_that_is_not_there_is_refused_up_front() {
    let dir = TempDir::new().expect("temp dir");
    let engine = engine(&dir);
    let recorder = Arc::new(Recorder::default());

    engine.set_event_listener(recorder.clone());
    engine.start().expect("start");

    std::fs::write(format!("{}/notes.txt", engine.sync_dir()), "contents")
        .expect("write into the folder");
    let indexed = recorder.wait_for("the file to be indexed", |e| {
        matches!(e, EngineEvent::FileIndexed { path, .. } if path == "notes.txt")
    });
    let EngineEvent::FileIndexed { file_id, .. } = indexed else {
        unreachable!()
    };

    // Nothing is paired or visible, so this is refused up front rather than
    // becoming an event - which is itself the contract: a request that cannot
    // be acted on fails immediately.
    assert!(
        engine
            .share_to(file_id, vec!["nowhere".into()])
            .is_err(),
        "a share to a device that is not there must not be accepted"
    );

    engine.stop();
}
