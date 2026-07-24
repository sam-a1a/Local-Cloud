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

pub fn is_ignored(set: &IgnoreSet, path: &str) -> bool {
    let guard = set.lock().unwrap();
    guard.contains(path)
}

pub fn unmark_ignored(set: &IgnoreSet, path: &str) {
    let mut guard = set.lock().unwrap();
    guard.remove(path);
}

pub fn schedule_unmark_ignored(ignore_set: IgnoreSet, path: String, delay_secs: u64) {
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(delay_secs)).await;
        unmark_ignored(&ignore_set, &path);
    });
}