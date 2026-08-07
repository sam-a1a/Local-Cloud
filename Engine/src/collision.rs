// Engine/src/collision.rs
//
// Two different files wanting the same name in a shared namespace.
//
// The catalog is replicated, so a name is owned by exactly one live item across
// the whole mesh. Dropping an unrelated `example.txt` into the sync folder when
// another device's `example.txt` already exists is therefore a conflict, and
// DESIGN.md says the person decides: Override, or Keep both under a numbered
// name.
//
// Asking cannot block indexing. The watcher runs in the background, and on
// mobile the app may not even be in the foreground, so a prompt that had to be
// answered before the file could be recorded would either stall or drop it.
// Instead the safe answer is applied immediately - the incoming file is kept
// under a free name, losing nothing - and the conflict is surfaced with
// Override offered as a follow-up. From the user's seat this is still a prompt
// with two buttons; the difference is that ignoring it cannot destroy anything.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, uniffi::Enum)]
pub enum CollisionResolution {
    /// Give the name to the incoming item and move the previous one to trash.
    Override,
    /// Leave the incoming item under its numbered name. Two separate items.
    KeepBoth,
}

/// A name conflict that has been resolved the safe way and is awaiting a
/// decision on whether it should have been an Override instead.
#[derive(Clone, Debug, Serialize, uniffi::Record)]
pub struct PendingCollision {
    pub id: String,
    /// The item that arrived and had to be renamed.
    pub incoming_file_id: String,
    /// The name it asked for.
    pub requested_path: String,
    /// The free name it is living under in the meantime.
    pub current_path: String,
    /// The item that already owned the name.
    pub existing_file_id: String,
    /// Which device created the item already holding the name.
    pub existing_created_by: String,
    pub detected_at: i64,
}

#[derive(Clone, Debug, Default)]
pub struct CollisionQueue {
    inner: Arc<Mutex<Vec<PendingCollision>>>,
}

impl CollisionQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a conflict, ignoring one already asked about. Catalog merges
    /// re-derive the same conflict on every sync, and the user should be asked
    /// once, not once per sync.
    pub fn record(&self, collision: PendingCollision) {
        let mut queue = self.inner.lock().unwrap();
        let duplicate = queue.iter().any(|c| {
            c.incoming_file_id == collision.incoming_file_id
                && c.existing_file_id == collision.existing_file_id
        });
        if !duplicate {
            queue.push(collision);
        }
    }

    pub fn pending(&self) -> Vec<PendingCollision> {
        self.inner.lock().unwrap().clone()
    }

    /// Removes and returns a conflict, so a decision can only be applied once.
    pub fn take(&self, id: &str) -> Option<PendingCollision> {
        let mut queue = self.inner.lock().unwrap();
        let index = queue.iter().position(|c| c.id == id)?;
        Some(queue.remove(index))
    }

    /// Drops any conflict mentioning a file, for when it leaves the catalog and
    /// the question stops being answerable.
    pub fn forget_file(&self, file_id: &str) {
        self.inner
            .lock()
            .unwrap()
            .retain(|c| c.incoming_file_id != file_id && c.existing_file_id != file_id);
    }
}

/// `notes.txt` -> `notes 1.txt`, keeping any directory prefix and extension.
pub fn suffixed_path(path: &str, n: u32) -> String {
    let as_path = std::path::Path::new(path);

    let stem = as_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let name = match as_path.extension() {
        Some(ext) => format!("{} {}.{}", stem, n, ext.to_string_lossy()),
        None => format!("{} {}", stem, n),
    };

    match as_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => format!("{}/{}", dir.to_string_lossy(), name),
        _ => name,
    }
}

/// The first numbered variant of `path` that nothing else claims.
///
/// `taken` is consulted for the catalog and, on desktop, the sync folder too -
/// a name free in one but not the other would still collide.
pub fn next_available_path(path: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(path) {
        return path.to_string();
    }
    // Bounded so a pathological directory cannot spin forever; past this the
    // caller gets a name that is almost certainly free and, if not, the unique
    // index refuses the insert rather than anything being overwritten.
    for n in 1..10_000 {
        let candidate = suffixed_path(path, n);
        if !taken(&candidate) {
            return candidate;
        }
    }
    suffixed_path(path, 10_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn suffix_goes_before_the_extension() {
        assert_eq!(suffixed_path("notes.txt", 1), "notes 1.txt");
        assert_eq!(suffixed_path("notes.txt", 12), "notes 12.txt");
    }

    #[test]
    fn suffix_survives_directories_and_missing_extensions() {
        assert_eq!(suffixed_path("docs/notes.txt", 1), "docs/notes 1.txt");
        assert_eq!(suffixed_path("a/b/report", 2), "a/b/report 2");
        assert_eq!(suffixed_path("README", 1), "README 1");
    }

    #[test]
    fn dotfiles_keep_their_leading_dot() {
        assert_eq!(suffixed_path(".gitignore", 1), ".gitignore 1");
    }

    #[test]
    fn a_free_name_is_used_unchanged() {
        assert_eq!(next_available_path("notes.txt", |_| false), "notes.txt");
    }

    #[test]
    fn numbering_skips_names_already_in_use() {
        let taken: HashSet<&str> = ["notes.txt", "notes 1.txt", "notes 2.txt"]
            .into_iter()
            .collect();
        assert_eq!(
            next_available_path("notes.txt", |p| taken.contains(p)),
            "notes 3.txt"
        );
    }

    #[test]
    fn a_decision_can_only_be_applied_once() {
        let queue = CollisionQueue::new();
        queue.record(PendingCollision {
            id: "c1".into(),
            incoming_file_id: "f2".into(),
            requested_path: "notes.txt".into(),
            current_path: "notes 1.txt".into(),
            existing_file_id: "f1".into(),
            existing_created_by: "android".into(),
            detected_at: 1,
        });

        assert_eq!(queue.pending().len(), 1);
        assert!(queue.take("c1").is_some());
        assert!(queue.take("c1").is_none());
        assert!(queue.pending().is_empty());
    }

    #[test]
    fn conflicts_are_forgotten_when_either_item_goes_away() {
        let queue = CollisionQueue::new();
        for (id, incoming, existing) in [("c1", "f2", "f1"), ("c2", "f4", "f3")] {
            queue.record(PendingCollision {
                id: id.into(),
                incoming_file_id: incoming.into(),
                requested_path: "notes.txt".into(),
                current_path: "notes 1.txt".into(),
                existing_file_id: existing.into(),
                existing_created_by: "android".into(),
                detected_at: 1,
            });
        }

        queue.forget_file("f1");
        let remaining = queue.pending();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "c2");
    }
}
