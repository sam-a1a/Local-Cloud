//! Indexing the sync folder: collisions, renames on disk, and what deleting a
//! copy does to the holder set.
//!
//! These drive `Indexer` directly rather than writing files and waiting for
//! filesystem events, so nothing here depends on timing.

use localcloud::collision::CollisionQueue;
use localcloud::db::{Database, DeleteRequest, FileHolder, MergeOutcome, Tombstone};
use localcloud::watcher::{IndexOutcome, Indexer};
use localcloud::{new_ignore_set, storage};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

struct Device {
    indexer: Indexer,
    db: Arc<Mutex<Database>>,
    collisions: CollisionQueue,
    sync_dir: String,
    storage_dir: String,
    device_id: String,
    _dir: TempDir,
}

impl Device {
    fn new(device_id: &str) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let base = dir.path().to_string_lossy().to_string();

        let storage_dir = format!("{}/storage", base);
        let sync_dir = format!("{}/sync", base);
        storage::ensure_storage_dir(&storage_dir).expect("storage");
        std::fs::create_dir_all(&sync_dir).expect("sync dir");

        let db = Arc::new(Mutex::new(
            Database::init(&format!("{}/test.db", base)).expect("db"),
        ));
        let collisions = CollisionQueue::new();

        let indexer = Indexer::new(
            db.clone(),
            storage_dir.clone(),
            sync_dir.clone(),
            device_id.to_string(),
            new_ignore_set(),
            collisions.clone(),
        );

        Self {
            indexer,
            db,
            collisions,
            sync_dir,
            storage_dir,
            device_id: device_id.to_string(),
            _dir: dir,
        }
    }

    fn block_path(&self, block_id: &str) -> String {
        storage::get_block_path(&self.storage_dir, block_id)
            .to_string_lossy()
            .to_string()
    }

    /// Writes a file into the sync folder and returns its absolute path.
    fn write(&self, relative_path: &str, contents: &str) -> String {
        let absolute = format!("{}/{}", self.sync_dir, relative_path);
        if let Some(parent) = std::path::Path::new(&absolute).parent() {
            std::fs::create_dir_all(parent).expect("parent dir");
        }
        std::fs::write(&absolute, contents).expect("write");
        absolute
    }

    fn exists(&self, relative_path: &str) -> bool {
        std::path::Path::new(&format!("{}/{}", self.sync_dir, relative_path)).exists()
    }

    fn read(&self, relative_path: &str) -> String {
        std::fs::read_to_string(format!("{}/{}", self.sync_dir, relative_path)).expect("read")
    }

    /// Pretends another device's item already owns a name.
    fn seed_foreign_item(&self, file_id: &str, path: &str, owner: &str) {
        let db = self.db.lock().unwrap();
        db.insert_file(&localcloud::FileMetadata {
            id: file_id.to_string(),
            path: path.to_string(),
            size: 1,
            content_hash: "foreign".into(),
            modified_time: 1,
            created_by: owner.to_string(),
            trashed_at: 0,
            trashed_by: String::new(),
        })
        .expect("seed item");
        db.set_holder(&FileHolder {
            file_id: file_id.to_string(),
            device_id: owner.to_string(),
            content_hash: "foreign".into(),
            received_at: 1,
        })
        .expect("seed holder");
    }
}

#[tokio::test]
async fn a_new_file_becomes_an_item_this_device_holds() {
    let device = Device::new("android");
    let path = device.write("notes.txt", "hello");

    let outcome = device.indexer.index(&path);
    let IndexOutcome::Indexed { file_id, path } = outcome else {
        panic!("expected Indexed, got {:?}", outcome);
    };
    assert_eq!(path, "notes.txt");

    let db = device.db.lock().unwrap();
    assert!(db.is_holder(&file_id, "android").unwrap());
    assert_eq!(db.holder_count(&file_id).unwrap(), 1);
}

#[tokio::test]
async fn rewriting_identical_content_is_not_a_change() {
    let device = Device::new("android");
    let path = device.write("notes.txt", "hello");
    device.indexer.index(&path);

    // Same bytes written again, as an editor saving an untouched buffer would.
    std::fs::write(&path, "hello").unwrap();

    assert!(matches!(
        device.indexer.index(&path),
        IndexOutcome::Unchanged { .. }
    ));
}

#[tokio::test]
async fn editing_our_own_item_keeps_its_identity() {
    let device = Device::new("android");
    let path = device.write("notes.txt", "hello");
    let IndexOutcome::Indexed { file_id: first, .. } = device.indexer.index(&path) else {
        panic!("expected Indexed");
    };

    std::fs::write(&path, "hello, again").unwrap();
    let IndexOutcome::Indexed { file_id: second, .. } = device.indexer.index(&path) else {
        panic!("expected Indexed");
    };

    assert_eq!(first, second, "an edit must not create a second item");
    assert!(device.collisions.pending().is_empty());
}

#[tokio::test]
async fn a_name_owned_by_another_device_is_kept_both_and_renamed_on_disk() {
    let device = Device::new("linux");
    device.seed_foreign_item("android-item", "example.txt", "android");

    let path = device.write("example.txt", "linux version");
    let outcome = device.indexer.index(&path);

    let IndexOutcome::KeptBoth {
        file_id,
        requested_path,
        path: kept_as,
    } = outcome
    else {
        panic!("expected KeptBoth, got {:?}", outcome);
    };
    assert_eq!(requested_path, "example.txt");
    assert_eq!(kept_as, "example 1.txt");

    // The folder must agree with the catalog about the name, and the contents
    // must be the file that arrived - not the one that already had the name.
    assert!(!device.exists("example.txt"), "original name must be vacated");
    assert!(device.exists("example 1.txt"));
    assert_eq!(device.read("example 1.txt"), "linux version");

    let db = device.db.lock().unwrap();
    assert_eq!(db.get_file_by_path("example.txt").unwrap().unwrap().id, "android-item");
    assert_eq!(db.get_file_by_path("example 1.txt").unwrap().unwrap().id, file_id);

    let pending = device.collisions.pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].existing_created_by, "android");
}

#[tokio::test]
async fn numbering_steps_past_names_already_on_disk() {
    let device = Device::new("linux");
    device.seed_foreign_item("android-item", "example.txt", "android");
    // A file occupying the obvious alternative, unknown to the catalog.
    device.write("example 1.txt", "something else");

    let path = device.write("example.txt", "linux version");
    let IndexOutcome::KeptBoth { path: kept_as, .. } = device.indexer.index(&path) else {
        panic!("expected KeptBoth");
    };

    assert_eq!(kept_as, "example 2.txt");
    assert_eq!(device.read("example 1.txt"), "something else");
    assert_eq!(device.read("example 2.txt"), "linux version");
}

#[tokio::test]
async fn collisions_inside_subfolders_keep_their_directory() {
    let device = Device::new("linux");
    device.seed_foreign_item("android-item", "docs/example.txt", "android");

    let path = device.write("docs/example.txt", "linux version");
    let IndexOutcome::KeptBoth { path: kept_as, .. } = device.indexer.index(&path) else {
        panic!("expected KeptBoth");
    };

    assert_eq!(kept_as, "docs/example 1.txt");
    assert!(device.exists("docs/example 1.txt"));
}

#[tokio::test]
async fn deleting_a_copy_frees_space_while_others_remain() {
    let device = Device::new("android");
    let path = device.write("notes.txt", "hello");
    let IndexOutcome::Indexed { file_id, .. } = device.indexer.index(&path) else {
        panic!("expected Indexed");
    };

    // Another device also holds it.
    {
        let db = device.db.lock().unwrap();
        db.set_holder(&FileHolder {
            file_id: file_id.clone(),
            device_id: "macos".into(),
            content_hash: "whatever".into(),
            received_at: 1,
        })
        .unwrap();
    }

    let outcome = device.indexer.delete_local_copy(&file_id, true).unwrap();
    assert!(!outcome.trashed, "nothing can be lost, so nothing to protect");
    assert_eq!(outcome.remaining_copies, 1);
    assert!(!device.exists("notes.txt"));

    let db = device.db.lock().unwrap();
    assert!(!db.is_holder(&file_id, "android").unwrap());
    assert!(db.is_holder(&file_id, "macos").unwrap());
    // Still live in the catalog, because someone still has it.
    assert_eq!(db.get_all_files().unwrap().len(), 1);
}

#[tokio::test]
async fn deleting_the_last_copy_goes_to_trash_and_keeps_the_bytes() {
    let device = Device::new("android");
    let path = device.write("notes.txt", "hello");
    let IndexOutcome::Indexed { file_id, .. } = device.indexer.index(&path) else {
        panic!("expected Indexed");
    };

    let outcome = device.indexer.delete_local_copy(&file_id, true).unwrap();
    assert!(outcome.trashed);
    assert!(!device.exists("notes.txt"), "removed from the user's view");

    let db = device.db.lock().unwrap();
    assert!(db.get_all_files().unwrap().is_empty());
    assert_eq!(db.get_trashed_files().unwrap().len(), 1);

    // The bytes and the holder row are deliberately kept: an item nobody holds
    // could not be restored.
    assert!(db.is_holder(&file_id, "android").unwrap());
    let blocks = db.get_blocks_for_file(&file_id).unwrap();
    assert!(blocks.iter().all(|b| b.is_present == 1));
}

#[tokio::test]
async fn deleting_a_copy_leaves_blocks_shared_with_another_item_alone() {
    let device = Device::new("android");

    // Two items with identical content share every block, because blocks are
    // addressed by content hash.
    let first = device.write("a.txt", "identical bytes");
    let second = device.write("b.txt", "identical bytes");
    let IndexOutcome::Indexed { file_id: a, .. } = device.indexer.index(&first) else {
        panic!("expected Indexed");
    };
    let IndexOutcome::Indexed { file_id: b, .. } = device.indexer.index(&second) else {
        panic!("expected Indexed");
    };

    {
        let db = device.db.lock().unwrap();
        let a_blocks: Vec<String> = db
            .get_blocks_for_file(&a)
            .unwrap()
            .into_iter()
            .map(|x| x.block_id)
            .collect();
        let b_blocks: Vec<String> = db
            .get_blocks_for_file(&b)
            .unwrap()
            .into_iter()
            .map(|x| x.block_id)
            .collect();
        assert_eq!(a_blocks, b_blocks, "identical files must share blocks");

        // Give item `a` a second holder so deleting our copy actually purges.
        db.set_holder(&FileHolder {
            file_id: a.clone(),
            device_id: "macos".into(),
            content_hash: "whatever".into(),
            received_at: 1,
        })
        .unwrap();
    }

    device.indexer.delete_local_copy(&a, true).unwrap();

    // `b` still needs those blocks, so they must survive.
    let db = device.db.lock().unwrap();
    let b_blocks = db.get_blocks_for_file(&b).unwrap();
    assert!(
        b_blocks.iter().all(|x| x.is_present == 1),
        "blocks shared with another item must not be purged"
    );
    drop(db);
    assert_eq!(device.read("b.txt"), "identical bytes");
}

#[tokio::test]
async fn a_file_leaving_the_folder_deletes_this_devices_copy() {
    let device = Device::new("android");
    let path = device.write("notes.txt", "hello");
    let IndexOutcome::Indexed { file_id, .. } = device.indexer.index(&path) else {
        panic!("expected Indexed");
    };

    std::fs::remove_file(&path).unwrap();
    let outcome = device.indexer.forget(&path).unwrap();

    assert_eq!(outcome.file_id, file_id);
    assert!(outcome.trashed, "it was the only copy");
}

#[tokio::test]
async fn a_queued_request_deletes_this_devices_copy_when_it_returns() {
    let device = Device::new("macos");
    let path = device.write("photo.jpg", "bytes");
    let IndexOutcome::Indexed { file_id, .. } = device.indexer.index(&path) else {
        panic!("expected Indexed");
    };

    // Another device also holds it, so this delete frees space rather than
    // going to trash.
    {
        let db = device.db.lock().unwrap();
        db.set_holder(&FileHolder {
            file_id: file_id.clone(),
            device_id: "android".into(),
            content_hash: "whatever".into(),
            received_at: 1,
        })
        .unwrap();

        // The iPhone asked for the macOS copy while macOS was asleep; the
        // request reached it through the catalog.
        db.record_delete_request(&DeleteRequest {
            file_id: file_id.clone(),
            target_device: "macos".into(),
            requested_by: "iphone".into(),
            requested_at: 10,
        })
        .unwrap();
    }

    let carried_out = device.indexer.apply_pending_delete_requests();
    assert_eq!(carried_out.len(), 1);
    assert!(!device.exists("photo.jpg"));

    let db = device.db.lock().unwrap();
    assert!(!db.is_holder(&file_id, "macos").unwrap());
    assert!(db.is_holder(&file_id, "android").unwrap());
    assert!(
        db.get_delete_requests().unwrap().is_empty(),
        "a carried-out request must not linger"
    );
}

#[tokio::test]
async fn a_request_is_never_carried_out_twice() {
    let device = Device::new("macos");
    let path = device.write("photo.jpg", "bytes");
    let IndexOutcome::Indexed { file_id, .. } = device.indexer.index(&path) else {
        panic!("expected Indexed");
    };

    {
        let db = device.db.lock().unwrap();
        db.record_delete_request(&DeleteRequest {
            file_id: file_id.clone(),
            target_device: "macos".into(),
            requested_by: "iphone".into(),
            requested_at: 10,
        })
        .unwrap();
    }

    // This is the last copy, so it goes to trash and the holder row stays -
    // which is exactly the case where a naive check would re-run forever.
    assert_eq!(device.indexer.apply_pending_delete_requests().len(), 1);
    assert!(device.indexer.apply_pending_delete_requests().is_empty());

    let db = device.db.lock().unwrap();
    assert_eq!(db.get_trashed_files().unwrap().len(), 1);
}

#[tokio::test]
async fn requests_aimed_at_other_devices_are_left_alone() {
    let device = Device::new("macos");
    let path = device.write("photo.jpg", "bytes");
    let IndexOutcome::Indexed { file_id, .. } = device.indexer.index(&path) else {
        panic!("expected Indexed");
    };

    {
        let db = device.db.lock().unwrap();
        db.record_delete_request(&DeleteRequest {
            file_id: file_id.clone(),
            target_device: "android".into(),
            requested_by: "iphone".into(),
            requested_at: 10,
        })
        .unwrap();
    }

    assert!(device.indexer.apply_pending_delete_requests().is_empty());
    assert!(device.exists("photo.jpg"));

    let db = device.db.lock().unwrap();
    assert!(db.is_holder(&file_id, "macos").unwrap());
    assert_eq!(
        db.get_delete_requests().unwrap().len(),
        1,
        "the request must be carried forward for its actual target"
    );
}

#[tokio::test]
async fn a_satisfied_request_is_pruned_on_the_requesting_device() {
    // The iPhone's view: it asked macOS to drop a copy, and later learns from
    // the catalog that macOS is no longer a holder.
    let device = Device::new("iphone");

    {
        let db = device.db.lock().unwrap();
        db.insert_file(&localcloud::FileMetadata {
            id: "item".into(),
            path: "photo.jpg".into(),
            size: 1,
            content_hash: "hash".into(),
            modified_time: 1,
            created_by: "macos".into(),
            trashed_at: 0,
            trashed_by: String::new(),
        })
        .unwrap();
        db.set_holder(&FileHolder {
            file_id: "item".into(),
            device_id: "macos".into(),
            content_hash: "hash".into(),
            received_at: 1,
        })
        .unwrap();
        db.record_delete_request(&DeleteRequest {
            file_id: "item".into(),
            target_device: "macos".into(),
            requested_by: "iphone".into(),
            requested_at: 10,
        })
        .unwrap();

        // Still outstanding while macOS holds it.
        assert_eq!(db.prune_satisfied_delete_requests().unwrap(), 0);

        // macOS carried it out, which the next catalog sync reflects.
        db.remove_holder("item", "macos").unwrap();
        assert_eq!(db.prune_satisfied_delete_requests().unwrap(), 1);
        assert!(db.get_delete_requests().unwrap().is_empty());
    }
}

#[tokio::test]
async fn a_last_copy_delete_also_settles_the_request() {
    // The target held the only copy, so it went to trash rather than being
    // dropped. The holder row survives, so satisfaction cannot be judged on
    // holders alone.
    let device = Device::new("iphone");

    let db = device.db.lock().unwrap();
    db.insert_file(&localcloud::FileMetadata {
        id: "item".into(),
        path: "photo.jpg".into(),
        size: 1,
        content_hash: "hash".into(),
        modified_time: 1,
        created_by: "macos".into(),
        trashed_at: 0,
        trashed_by: String::new(),
    })
    .unwrap();
    db.set_holder(&FileHolder {
        file_id: "item".into(),
        device_id: "macos".into(),
        content_hash: "hash".into(),
        received_at: 1,
    })
    .unwrap();
    db.record_delete_request(&DeleteRequest {
        file_id: "item".into(),
        target_device: "macos".into(),
        requested_by: "iphone".into(),
        requested_at: 10,
    })
    .unwrap();

    db.trash_file("item", "macos", 20).unwrap();
    assert_eq!(db.prune_satisfied_delete_requests().unwrap(), 1);
}

const DAY: i64 = 24 * 60 * 60;
const RETENTION: i64 = 30 * DAY;
/// An arbitrary fixed "now". Cannot be 0, which is the sentinel for a live item.
const T0: i64 = 1_700_000_000;

/// Puts an item in trash with a chosen timestamp, as deleting its last copy
/// would, so retention can be tested without waiting a month.
fn trash_at(device: &Device, file_id: &str, when: i64) {
    let db = device.db.lock().unwrap();
    db.trash_file(file_id, "android", when).unwrap();
}

#[tokio::test]
async fn trash_survives_until_its_retention_runs_out() {
    let device = Device::new("android");
    let path = device.write("notes.txt", "hello");
    let IndexOutcome::Indexed { file_id, .. } = device.indexer.index(&path) else {
        panic!("expected Indexed");
    };
    trash_at(&device, &file_id, T0);

    // A day short of the limit it is still there, bytes and all.
    assert!(device.indexer.sweep_trash(T0 + RETENTION - DAY, RETENTION).is_empty());
    {
        let db = device.db.lock().unwrap();
        assert_eq!(db.get_trashed_files().unwrap().len(), 1);
        assert!(db.is_holder(&file_id, "android").unwrap());
    }

    // Restorable right up to that point.
    device.db.lock().unwrap().restore_file(&file_id).unwrap();
    assert!(device
        .db
        .lock()
        .unwrap()
        .get_file_by_path("notes.txt")
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn expired_trash_is_destroyed_and_leaves_a_tombstone() {
    let device = Device::new("android");
    let path = device.write("notes.txt", "hello");
    let IndexOutcome::Indexed { file_id, .. } = device.indexer.index(&path) else {
        panic!("expected Indexed");
    };

    let block_ids: Vec<String> = {
        let db = device.db.lock().unwrap();
        db.get_blocks_for_file(&file_id)
            .unwrap()
            .into_iter()
            .map(|b| b.block_id)
            .collect()
    };
    trash_at(&device, &file_id, T0);

    let purged = device.indexer.sweep_trash(T0 + RETENTION, RETENTION);
    assert_eq!(purged, vec![file_id.clone()]);

    let db = device.db.lock().unwrap();
    assert!(db.get_file_by_id(&file_id).unwrap().is_none());
    assert!(db.get_trashed_files().unwrap().is_empty());
    assert_eq!(db.holder_count(&file_id).unwrap(), 0);
    assert!(db.has_tombstone(&file_id).unwrap());

    // The space is actually released, not merely forgotten.
    for block_id in block_ids {
        assert!(
            !std::path::Path::new(&device.block_path(&block_id)).exists(),
            "block {} should be gone from storage",
            block_id
        );
    }
}

#[tokio::test]
async fn a_live_item_is_never_swept() {
    let device = Device::new("android");
    let path = device.write("notes.txt", "hello");
    device.indexer.index(&path);

    assert!(device
        .indexer
        .sweep_trash(T0 + RETENTION * 10, RETENTION)
        .is_empty());
    assert_eq!(device.db.lock().unwrap().get_all_files().unwrap().len(), 1);
}

#[tokio::test]
async fn a_destroyed_item_cannot_be_handed_back_by_a_stale_catalog() {
    let device = Device::new("android");
    let path = device.write("notes.txt", "hello");
    let IndexOutcome::Indexed { file_id, .. } = device.indexer.index(&path) else {
        panic!("expected Indexed");
    };
    trash_at(&device, &file_id, T0);
    device.indexer.sweep_trash(T0 + RETENTION, RETENTION);

    // A device that was away still lists it and offers it back on the next sync.
    let stale = localcloud::FileMetadata {
        id: file_id.clone(),
        path: "notes.txt".into(),
        size: 5,
        content_hash: "hash".into(),
        modified_time: 1,
        created_by: "android".into(),
        trashed_at: 0,
        trashed_by: String::new(),
    };

    let db = device.db.lock().unwrap();
    assert_eq!(
        db.merge_catalog_file(&stale).unwrap(),
        MergeOutcome::AlreadyDestroyed
    );
    assert!(db.get_all_files().unwrap().is_empty());
}

#[tokio::test]
async fn a_tombstone_from_a_peer_destroys_the_local_copy() {
    let device = Device::new("android");
    let path = device.write("notes.txt", "hello");
    let IndexOutcome::Indexed { file_id, .. } = device.indexer.index(&path) else {
        panic!("expected Indexed");
    };

    let applied = device
        .indexer
        .apply_tombstone(&Tombstone {
            file_id: file_id.clone(),
            deleted_at: 999,
            deleted_by: "macos".into(),
        })
        .unwrap();
    assert!(applied);

    assert!(!device.exists("notes.txt"));
    let db = device.db.lock().unwrap();
    assert!(db.get_file_by_id(&file_id).unwrap().is_none());
    assert_eq!(db.holder_count(&file_id).unwrap(), 0);

    // The original deletion time is kept, not the moment it was learned.
    let tombstones = db.get_all_tombstones().unwrap();
    assert_eq!(tombstones.len(), 1);
    assert_eq!(tombstones[0].deleted_at, 999);
    assert_eq!(tombstones[0].deleted_by, "macos");
}

#[tokio::test]
async fn applying_the_same_tombstone_twice_is_harmless() {
    let device = Device::new("android");
    let path = device.write("notes.txt", "hello");
    let IndexOutcome::Indexed { file_id, .. } = device.indexer.index(&path) else {
        panic!("expected Indexed");
    };

    let tombstone = Tombstone {
        file_id,
        deleted_at: 999,
        deleted_by: "macos".into(),
    };
    assert!(device.indexer.apply_tombstone(&tombstone).unwrap());
    assert!(
        !device.indexer.apply_tombstone(&tombstone).unwrap(),
        "a tombstone already known is not news"
    );
}

#[tokio::test]
async fn destroying_an_item_spares_blocks_another_item_shares() {
    let device = Device::new("android");
    let first = device.write("a.txt", "identical bytes");
    let second = device.write("b.txt", "identical bytes");
    let IndexOutcome::Indexed { file_id: a, .. } = device.indexer.index(&first) else {
        panic!("expected Indexed");
    };
    let IndexOutcome::Indexed { file_id: b, .. } = device.indexer.index(&second) else {
        panic!("expected Indexed");
    };

    trash_at(&device, &a, T0);
    device.indexer.sweep_trash(T0 + RETENTION, RETENTION);

    let db = device.db.lock().unwrap();
    assert!(db.get_file_by_id(&a).unwrap().is_none());
    for block in db.get_blocks_for_file(&b).unwrap() {
        assert!(
            std::path::Path::new(&device.block_path(&block.block_id)).exists(),
            "a block b.txt still needs must survive"
        );
    }
    drop(db);
    assert_eq!(device.read("b.txt"), "identical bytes");
}

#[tokio::test]
async fn indexing_refuses_paths_outside_the_sync_folder() {
    let device = Device::new("android");
    let outside = TempDir::new().unwrap();
    let stray = outside.path().join("notes.txt");
    std::fs::write(&stray, "hello").unwrap();

    assert!(matches!(
        device.indexer.index(stray.to_str().unwrap()),
        IndexOutcome::Skipped { .. }
    ));
    assert!(device.db.lock().unwrap().get_all_files().unwrap().is_empty());
}

#[tokio::test]
async fn deleting_a_copy_this_device_does_not_hold_is_refused() {
    let device = Device::new("linux");
    device.seed_foreign_item("android-item", "example.txt", "android");

    let error = device
        .indexer
        .delete_local_copy("android-item", false)
        .expect_err("only the holder can delete its own copy");
    assert!(error.to_string().contains("does not hold"));

    // Deleting a remote copy is a request to that device, never a local action.
    let db = device.db.lock().unwrap();
    assert!(db.is_holder("android-item", "android").unwrap());
    assert_eq!(device.device_id, "linux");
}
