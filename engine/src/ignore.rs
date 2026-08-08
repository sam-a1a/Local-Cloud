// engine/src/ignore.rs
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

pub type IgnoreSet = Arc<Mutex<HashSet<String>>>;

pub fn new_ignore_set() -> IgnoreSet {
    Arc::new(Mutex::new(HashSet::new()))
}

pub fn mark_ignored(set: &IgnoreSet, path: &str) {
    let mut guard = set.lock().unwrap();
    guard.insert(path.to_string());
}

#[cfg(desktop)]
pub fn is_ignored(set: &IgnoreSet, path: &str) -> bool {
    let guard = set.lock().unwrap();
    guard.contains(path)
}

pub fn unmark_ignored(set: &IgnoreSet, path: &str) {
    let mut guard = set.lock().unwrap();
    guard.remove(path);
}

/// Stops ignoring `path` after `delay_secs`, whoever is calling.
///
/// This used to be a bare `tokio::spawn`, which panics with "there is no
/// reactor running" unless the caller happens to be inside a runtime. Most were,
/// so it went unnoticed until one was not - and a caller has no way to know
/// which kind it is from the signature. Since the engine is headed for FFI,
/// where calls arrive from whatever thread Swift or Kotlin is on, it takes the
/// runtime when there is one and a plain thread when there is not.
pub fn schedule_unmark_ignored(ignore_set: IgnoreSet, path: String, delay_secs: u64) {
    let delay = std::time::Duration::from_secs(delay_secs);

    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(async move {
                tokio::time::sleep(delay).await;
                unmark_ignored(&ignore_set, &path);
            });
        }
        Err(_) => {
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                unmark_ignored(&ignore_set, &path);
            });
        }
    }
}