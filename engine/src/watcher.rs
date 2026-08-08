// engine/src/watcher.rs
//
// Turning files in the sync folder into catalog items, and removals into the
// loss of this device's copy.
//
// The work is split from the filesystem watcher into `Indexer` so it can be
// driven directly. Everything worth getting right here - collision renames,
// holder bookkeeping, deciding whether a delete frees space or goes to trash -
// used to be reachable only by writing a file and waiting for an event to
// arrive, which makes for tests that pass or fail on timing.

use crate::collision::{CollisionQueue, PendingCollision};
use crate::db::{Database, FileHolder, FileMetadata};
use crate::ignore::IgnoreSet;
use crate::storage;
#[cfg(desktop)]
use crate::EngineEvent;
use anyhow::{anyhow, Result};
// The folder watcher is desktop-only, and so is the crate behind it. iOS
// cannot watch a user directory or run a background daemon freely, and Android
// would need MANAGE_EXTERNAL_STORAGE - which is why mobile has `import_file`
// instead. Everything else in this module is platform-independent: `Indexer` is
// what turns a file into an item, and it does not care what prompted the call.
#[cfg(desktop)]
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
#[cfg(desktop)]
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
#[cfg(desktop)]
use std::sync::mpsc;
#[cfg(desktop)]
use std::time::{Duration, Instant};
#[cfg(desktop)]
use tokio::runtime::Handle;

#[cfg(desktop)]
type DebounceMap = Arc<Mutex<HashMap<String, Instant>>>;

/// How long a path stays ignored after the engine itself writes to it, so the
/// resulting event is not mistaken for a user edit.
const SELF_WRITE_GRACE_SECS: u64 = 3;

pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn new_file_id(device_id: &str) -> String {
    format!(
        "{}-{}",
        device_id,
        uuid::Uuid::new_v4().to_string().replace("-", "")
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexOutcome {
    /// The content matches what the catalog already records.
    Unchanged { file_id: String },
    /// Recorded under the name it asked for.
    Indexed { file_id: String, path: String },
    /// The name belonged to another device's item, so both were kept.
    KeptBoth {
        file_id: String,
        requested_path: String,
        path: String,
    },
    Skipped { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeleteOutcome {
    pub file_id: String,
    /// Copies left in the mesh once this one is gone.
    pub remaining_copies: i64,
    /// True when this was the last copy, so the item went to trash instead of
    /// being freed.
    pub trashed: bool,
}

/// Catalog bookkeeping for one device's sync folder.
#[derive(Clone)]
pub struct Indexer {
    db: Arc<Mutex<Database>>,
    storage_dir: String,
    sync_dir: String,
    device_id: String,
    ignore_set: IgnoreSet,
    collisions: CollisionQueue,
}

impl Indexer {
    pub fn new(
        db: Arc<Mutex<Database>>,
        storage_dir: String,
        sync_dir: String,
        device_id: String,
        ignore_set: IgnoreSet,
        collisions: CollisionQueue,
    ) -> Self {
        Self {
            db,
            storage_dir,
            sync_dir,
            device_id,
            ignore_set,
            collisions,
        }
    }

    pub fn absolute(&self, relative_path: &str) -> String {
        format!("{}/{}", self.sync_dir, relative_path)
    }

    fn relative(&self, absolute_path: &str) -> Option<String> {
        let relative = absolute_path
            .strip_prefix(&self.sync_dir)?
            .trim_start_matches('/')
            .to_string();
        if relative.is_empty() {
            None
        } else {
            Some(relative)
        }
    }

    /// The first numbered variant of `path` free in both the catalog and the
    /// folder. A name free in one but taken in the other still collides.
    pub(crate) fn free_path(&self, path: &str) -> String {
        let db = self.db.lock().unwrap();
        crate::collision::next_available_path(path, |candidate| {
            db.is_path_taken(candidate).unwrap_or(true) || Path::new(&self.absolute(candidate)).exists()
        })
    }

    /// Moves a file within the sync folder without the move looking like a user
    /// edit when the event comes back.
    fn rename_on_disk(&self, from: &str, to: &str) -> std::io::Result<()> {
        crate::ignore::mark_ignored(&self.ignore_set, from);
        crate::ignore::mark_ignored(&self.ignore_set, to);

        if let Some(parent) = Path::new(to).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let result = std::fs::rename(from, to);

        crate::ignore::schedule_unmark_ignored(
            self.ignore_set.clone(),
            from.to_string(),
            SELF_WRITE_GRACE_SECS,
        );
        crate::ignore::schedule_unmark_ignored(
            self.ignore_set.clone(),
            to.to_string(),
            SELF_WRITE_GRACE_SECS,
        );
        result
    }

    fn remove_from_disk(&self, absolute_path: &str) {
        if !Path::new(absolute_path).exists() {
            return;
        }
        crate::ignore::mark_ignored(&self.ignore_set, absolute_path);
        let _ = std::fs::remove_file(absolute_path);
        crate::ignore::schedule_unmark_ignored(
            self.ignore_set.clone(),
            absolute_path.to_string(),
            SELF_WRITE_GRACE_SECS,
        );
    }

    /// Records a file in the sync folder as an item this device holds.
    pub fn index(&self, absolute_path: &str) -> IndexOutcome {
        let skip = |reason: &str| IndexOutcome::Skipped {
            reason: reason.to_string(),
        };

        let Some(requested_path) = self.relative(absolute_path) else {
            return skip("outside the sync folder");
        };

        let metadata = match std::fs::metadata(absolute_path) {
            Ok(m) => m,
            Err(e) => return skip(&e.to_string()),
        };
        if !metadata.is_file() {
            return skip("not a file");
        }

        let size = metadata.len() as i64;
        let modified_time = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or_else(now_secs);

        // A name belongs to exactly one live item across the mesh. If this
        // device already holds the item sitting at this path then the file was
        // edited; if the name belongs to another device's item, this is an
        // unrelated file that happens to share it.
        let clash = {
            let db = self.db.lock().unwrap();
            match db.get_file_by_path(&requested_path) {
                Ok(Some(existing)) => {
                    if db.is_holder(&existing.id, &self.device_id).unwrap_or(false) {
                        None
                    } else {
                        Some(existing)
                    }
                }
                _ => None,
            }
        };

        let mut kept_both = false;
        let (path, absolute_path, file_id, known_hash) = match clash {
            None => {
                let db = self.db.lock().unwrap();
                match db.get_file_by_path(&requested_path) {
                    Ok(Some(existing)) => (
                        requested_path.clone(),
                        absolute_path.to_string(),
                        existing.id,
                        existing.content_hash,
                    ),
                    _ => (
                        requested_path.clone(),
                        absolute_path.to_string(),
                        new_file_id(&self.device_id),
                        String::new(),
                    ),
                }
            }
            Some(existing) => {
                // Keep both, which cannot lose anything, and record the conflict
                // so Override stays available as a decision.
                let free = self.free_path(&requested_path);
                let free_absolute = self.absolute(&free);

                if let Err(e) = self.rename_on_disk(absolute_path, &free_absolute) {
                    return skip(&format!("could not rename colliding file: {}", e));
                }

                let file_id = new_file_id(&self.device_id);
                self.collisions.record(PendingCollision {
                    id: uuid::Uuid::new_v4().to_string(),
                    incoming_file_id: file_id.clone(),
                    requested_path: requested_path.clone(),
                    current_path: free.clone(),
                    existing_file_id: existing.id,
                    existing_created_by: existing.created_by,
                    detected_at: modified_time,
                });

                kept_both = true;
                (free, free_absolute, file_id, String::new())
            }
        };

        let content_hash = {
            let db = self.db.lock().unwrap();

            // file_blocks references files(id), so the row has to exist before
            // blocks can be mapped to it.
            let placeholder = FileMetadata {
                id: file_id.clone(),
                path: path.clone(),
                size,
                content_hash: known_hash.clone(),
                modified_time,
                created_by: self.device_id.clone(),
                trashed_at: 0,
                trashed_by: String::new(),
            };
            if let Err(e) = db.insert_file(&placeholder) {
                return skip(&format!("could not record metadata: {}", e));
            }

            // Noted before the manifest is replaced, so what the previous
            // revision alone was using can be released once the new one is in.
            let superseded = db.blocks_exclusive_to_file(&file_id).unwrap_or_default();

            let _ = db.clear_blocks_for_file(&file_id);
            if let Err(e) =
                storage::chunk_and_store_file(&self.storage_dir, &db, &file_id, &absolute_path)
            {
                return skip(&format!("could not chunk: {}", e));
            }

            // Re-chunking keeps whatever the new manifest maps to again, so an
            // unchanged file loses nothing here. Without this every edit would
            // leave its previous contents in storage for good - and the move to
            // megabyte blocks re-chunks every existing file once, which would
            // otherwise mean carrying a second copy of everything forever.
            for block_id in superseded {
                if db.forget_block_if_unreferenced(&block_id).unwrap_or(false) {
                    let _ = storage::remove_block(&self.storage_dir, &block_id);
                }
            }

            let blocks = db.get_blocks_for_file(&file_id).unwrap_or_default();
            storage::content_hash(&blocks)
        };

        // Editors rewrite files without changing them. Comparing the block
        // manifest rather than size or mtime makes that a non-event.
        if content_hash == known_hash && !kept_both {
            return IndexOutcome::Unchanged { file_id };
        }

        {
            let db = self.db.lock().unwrap();
            let file = FileMetadata {
                id: file_id.clone(),
                path: path.clone(),
                size,
                content_hash: content_hash.clone(),
                modified_time,
                created_by: self.device_id.clone(),
                trashed_at: 0,
                trashed_by: String::new(),
            };
            if let Err(e) = db.insert_file(&file) {
                return skip(&format!("could not record content hash: {}", e));
            }

            // On desktop the folder holds exactly what this device holds, so a
            // file appearing in it makes this device a holder of that content.
            if let Err(e) = db.set_holder(&FileHolder {
                file_id: file_id.clone(),
                device_id: self.device_id.clone(),
                content_hash,
                received_at: modified_time,
            }) {
                return skip(&format!("could not record holder: {}", e));
            }
        }

        if kept_both {
            IndexOutcome::KeptBoth {
                file_id,
                requested_path,
                path,
            }
        } else {
            IndexOutcome::Indexed { file_id, path }
        }
    }

    /// Deletes this device's copy of an item.
    ///
    /// When other copies exist nothing can be lost, so the space is freed
    /// immediately. When it is the last copy the bytes and the holder row are
    /// deliberately kept and the item goes to trash instead: an item nobody
    /// holds could not be restored.
    pub fn delete_local_copy(&self, file_id: &str, remove_from_disk: bool) -> Result<DeleteOutcome> {
        let db = self.db.lock().unwrap();

        let file = db
            .get_file_by_id(file_id)?
            .ok_or_else(|| anyhow!("No such item in the catalog"))?;

        if !db.is_holder(file_id, &self.device_id)? {
            return Err(anyhow!("This device does not hold that item"));
        }

        if remove_from_disk {
            self.remove_from_disk(&self.absolute(&file.path));
        }

        let last_copy = db.holder_count(file_id)? <= 1;

        if last_copy {
            if !file.is_trashed() {
                db.trash_file(file_id, &self.device_id, now_secs())?;
            }
            return Ok(DeleteOutcome {
                file_id: file_id.to_string(),
                remaining_copies: 1,
                trashed: true,
            });
        }

        purge_exclusive_blocks(&db, &self.storage_dir, file_id)?;
        db.remove_holder(file_id, &self.device_id)?;
        self.collisions.forget_file(file_id);

        Ok(DeleteOutcome {
            file_id: file_id.to_string(),
            remaining_copies: db.holder_count(file_id)?,
            trashed: false,
        })
    }

    /// Destroys an item for good: its bytes, its catalog entry, and any
    /// requests still outstanding for it.
    ///
    /// A tombstone is left behind so a device that was away cannot hand the
    /// item back from a stale catalog.
    pub fn purge(&self, file_id: &str) -> Result<()> {
        let path = {
            let db = self.db.lock().unwrap();
            db.get_file_by_id(file_id)?
                .ok_or_else(|| anyhow!("No such item in the catalog"))?
                .path
        };

        self.remove_from_disk(&self.absolute(&path));

        {
            let db = self.db.lock().unwrap();
            purge_exclusive_blocks(&db, &self.storage_dir, file_id)?;
            db.purge_file(file_id, &self.device_id, now_secs())?;
        }

        self.collisions.forget_file(file_id);
        Ok(())
    }

    /// Destroys trashed items whose retention has run out.
    ///
    /// `now` is passed in rather than read from the clock so the retention rule
    /// can be tested without waiting a month.
    pub fn sweep_trash(&self, now: i64, retention_secs: i64) -> Vec<String> {
        let expired = {
            let db = self.db.lock().unwrap();
            db.expired_trash(now, retention_secs).unwrap_or_default()
        };

        let mut purged = Vec::new();
        for file in expired {
            match self.purge(&file.id) {
                Ok(()) => purged.push(file.id),
                Err(e) => println!("[Trash] Could not purge {}: {}", file.path, e),
            }
        }
        purged
    }

    /// Applies a destruction a peer has already carried out.
    pub fn apply_tombstone(&self, tombstone: &crate::db::Tombstone) -> Result<bool> {
        let known = {
            let db = self.db.lock().unwrap();
            if db.has_tombstone(&tombstone.file_id)? {
                return Ok(false);
            }
            db.get_file_by_id(&tombstone.file_id)?.is_some()
        };

        if known {
            self.purge(&tombstone.file_id)?;
        }

        // Recorded either way: a device that never saw the item still has to
        // refuse it if a third one offers it later.
        let db = self.db.lock().unwrap();
        db.insert_tombstone(tombstone)?;
        Ok(true)
    }

    /// Carries out any standing requests for this device to drop a copy.
    ///
    /// Only the holder can erase its own disk, so a delete aimed at this device
    /// arrives as a request - directly if it was reachable, otherwise through
    /// the catalog once it comes back. Applied requests are cleared so they
    /// cannot run twice.
    pub fn apply_pending_delete_requests(&self) -> Vec<DeleteOutcome> {
        let requests = {
            let db = self.db.lock().unwrap();
            db.get_delete_requests_for(&self.device_id).unwrap_or_default()
        };

        let mut carried_out = Vec::new();
        for request in requests {
            let holds = {
                let db = self.db.lock().unwrap();
                db.is_holder(&request.file_id, &self.device_id)
                    .unwrap_or(false)
            };

            if holds {
                match self.delete_local_copy(&request.file_id, true) {
                    Ok(outcome) => carried_out.push(outcome),
                    Err(e) => {
                        println!("[Delete] Could not carry out request: {}", e);
                        continue;
                    }
                }
            }

            // Either done, or there was no copy here to remove. Settled either
            // way, so the request should not linger.
            let db = self.db.lock().unwrap();
            let _ = db.clear_delete_request(&request.file_id, &self.device_id);
        }

        carried_out
    }

    /// Handles a file disappearing from the folder, which means the same thing
    /// as deleting this device's copy.
    pub fn forget(&self, absolute_path: &str) -> Result<DeleteOutcome> {
        let relative_path = self
            .relative(absolute_path)
            .ok_or_else(|| anyhow!("Path is outside the sync folder"))?;

        let file_id = {
            let db = self.db.lock().unwrap();
            db.get_file_by_path(&relative_path)?
                .ok_or_else(|| anyhow!("No catalog item at that path"))?
                .id
        };

        // Already gone from disk, so there is nothing to remove.
        self.delete_local_copy(&file_id, false)
    }
}

/// Deletes the stored blocks a file alone uses, leaving shared ones in place.
pub(crate) fn purge_exclusive_blocks(
    db: &Database,
    storage_dir: &str,
    file_id: &str,
) -> Result<()> {
    for block_id in db.blocks_exclusive_to_file(file_id)? {
        let _ = storage::remove_block(storage_dir, &block_id);
        let _ = db.set_block_present(&block_id, false);
    }
    Ok(())
}

#[cfg(desktop)]
pub fn start_watcher(
    indexer: Indexer,
    handle: Handle,
    ignore_set: IgnoreSet,
    event_tx: mpsc::Sender<EngineEvent>,
) -> Result<RecommendedWatcher> {
    let watch_path = indexer.sync_dir.clone();
    let debounce_map: DebounceMap = Arc::new(Mutex::new(HashMap::new()));

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| match res {
        Ok(event) => {
            let indexer = indexer.clone();
            let ignore_set = ignore_set.clone();
            let debounce_map = debounce_map.clone();
            let event_tx = event_tx.clone();

            handle.spawn(async move {
                handle_fs_event(event, indexer, ignore_set, debounce_map, event_tx).await;
            });
        }
        Err(e) => println!("[Watcher] Error: {}", e),
    })?;

    watcher.watch(Path::new(&watch_path), RecursiveMode::Recursive)?;
    println!("[Watcher] Watching directory: {}", watch_path);

    Ok(watcher)
}

#[cfg(desktop)]
fn is_engine_file(path: &str) -> bool {
    path.contains(".db") || path.contains(".pem") || path.contains("identity.json")
}

#[cfg(desktop)]
async fn handle_fs_event(
    event: Event,
    indexer: Indexer,
    ignore_set: IgnoreSet,
    debounce_map: DebounceMap,
    event_tx: mpsc::Sender<EngineEvent>,
) {
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            for path in event.paths {
                let Some(path_str) = path.to_str().map(|s| s.to_string()) else {
                    continue;
                };
                if !path.is_file() || is_engine_file(&path_str) {
                    continue;
                }
                if crate::ignore::is_ignored(&ignore_set, &path_str) {
                    continue;
                }

                {
                    let mut map = debounce_map.lock().unwrap();
                    let now = Instant::now();
                    if let Some(last) = map.get(&path_str) {
                        if now.duration_since(*last) < Duration::from_millis(500) {
                            continue;
                        }
                    }
                    map.insert(path_str.clone(), now);
                }

                // Let the writer finish before reading the file.
                tokio::time::sleep(Duration::from_millis(200)).await;

                match indexer.index(&path_str) {
                    IndexOutcome::Indexed { file_id, path } => {
                        println!("[Watcher] Indexed {}", path);
                        let _ = event_tx.send(EngineEvent::FileIndexed { file_id, path });
                    }
                    IndexOutcome::KeptBoth {
                        file_id,
                        requested_path,
                        path,
                    } => {
                        println!("[Watcher] \"{}\" was taken; kept as \"{}\"", requested_path, path);
                        let _ = event_tx.send(EngineEvent::NameCollision {
                            requested_path,
                            kept_as: path.clone(),
                        });
                        let _ = event_tx.send(EngineEvent::FileIndexed { file_id, path });
                    }
                    IndexOutcome::Unchanged { .. } => {}
                    IndexOutcome::Skipped { reason } => {
                        println!("[Watcher] Skipped {}: {}", path_str, reason);
                    }
                }
            }
        }

        EventKind::Remove(_) => {
            for path in event.paths {
                let Some(path_str) = path.to_str().map(|s| s.to_string()) else {
                    continue;
                };
                if crate::ignore::is_ignored(&ignore_set, &path_str) {
                    continue;
                }

                // Dragging a file out of the folder deletes this device's copy.
                // Other devices' copies are untouched.
                match indexer.forget(&path_str) {
                    Ok(outcome) if outcome.trashed => {
                        println!("[Watcher] Last copy of {} removed; moved to trash", path_str);
                        let _ = event_tx.send(EngineEvent::FileTrashed {
                            file_id: outcome.file_id,
                        });
                    }
                    Ok(outcome) => {
                        println!(
                            "[Watcher] Dropped local copy; {} copies remain",
                            outcome.remaining_copies
                        );
                        let _ = event_tx.send(EngineEvent::CopyDeleted {
                            file_id: outcome.file_id,
                            device_id: indexer.device_id.clone(),
                        });
                    }
                    Err(e) => println!("[Watcher] Ignoring removal of {}: {}", path_str, e),
                }
            }
        }

        _ => {}
    }
}
