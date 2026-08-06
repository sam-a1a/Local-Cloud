//! Indexing the sync folder: collisions, renames on disk, and what deleting a
//! copy does to the holder set.
//!
//! These drive `Indexer` directly rather than writing files and waiting for
//! filesystem events, so nothing here depends on timing.

use localcloud::collision::CollisionQueue;
use localcloud::db::{Database, FileHolder};
use localcloud::watcher::{IndexOutcome, Indexer};
use localcloud::{new_ignore_set, storage};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

struct Device {
    indexer: Indexer,
    db: Arc<Mutex<Database>>,
    collisions: CollisionQueue,
    sync_dir: String,
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
            storage_dir,
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
            device_id: device_id.to_string(),
            _dir: dir,
        }
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
