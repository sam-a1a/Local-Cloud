// engine/src/watcher.rs
use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use crate::db::{Database, FileHolder, FileMetadata};
use crate::ignore::IgnoreSet;
use crate::storage;
use crate::EngineEvent;

type DebounceMap = Arc<Mutex<HashMap<String, Instant>>>;

fn new_file_id(device_id: &str) -> String {
    format!(
        "{}-{}",
        device_id,
        uuid::Uuid::new_v4().to_string().replace("-", "")
    )
}

pub fn start_watcher(
    watch_dir: String,
    storage_dir: String,
    device_id: String,
    db: Arc<Mutex<Database>>,
    handle: Handle,
    ignore_set: IgnoreSet,
    collisions: crate::collision::CollisionQueue,
    event_tx: mpsc::Sender<EngineEvent>,
) -> Result<RecommendedWatcher> {
    let watch_path = watch_dir.clone();
    let debounce_map: DebounceMap = Arc::new(Mutex::new(HashMap::new()));

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        match res {
            Ok(event) => {
                let db_clone = db.clone();
                let storage_dir_clone = storage_dir.clone();
                let device_id_clone = device_id.clone();
                let watch_dir_clone = watch_dir.clone();
                let ignore_clone = ignore_set.clone();
                let debounce_clone = debounce_map.clone();
                let collisions_clone = collisions.clone();
                let event_tx_clone = event_tx.clone();

                handle.spawn(async move {
                    handle_fs_event(
                        event,
                        db_clone,
                        storage_dir_clone,
                        device_id_clone,
                        watch_dir_clone,
                        ignore_clone,
                        debounce_clone,
                        collisions_clone,
                        event_tx_clone,
                    )
                        .await;
                });
            }
            Err(e) => println!("[Watcher] Error: {}", e),
        }
    })?;

    watcher.watch(Path::new(&watch_path), RecursiveMode::Recursive)?;
    println!("[Watcher] Watching directory: {}", watch_path);

    Ok(watcher)
}

async fn handle_fs_event(
    event: Event,
    db: Arc<Mutex<Database>>,
    storage_dir: String,
    device_id: String,
    watch_dir: String,
    ignore_set: IgnoreSet,
    debounce_map: DebounceMap,
    collisions: crate::collision::CollisionQueue,
    event_tx: mpsc::Sender<EngineEvent>,
) {
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            for path in event.paths {
                if !path.is_file() { continue; }

                let path_str = match path.to_str() {
                    Some(s) => s.to_string(),
                    None => continue,
                };

                if path_str.contains(".db") || path_str.contains(".pem") || path_str.contains("identity.json") {
                    continue;
                }

                if crate::ignore::is_ignored(&ignore_set, &path_str) {
                    println!("[Watcher] Ignoring sync-engine write: {}", path_str);
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

                let relative_path = path_str.strip_prefix(&watch_dir).unwrap_or(&path_str).trim_start_matches('/').to_string();
                if relative_path.is_empty() { continue; }

                println!("[Watcher] Detected create/modify: {}", relative_path);
                tokio::time::sleep(Duration::from_millis(200)).await;

                let file_size = match std::fs::metadata(&path_str) {
                    Ok(m) => m.len() as i64,
                    Err(_) => continue,
                };

                let modified_time = match std::fs::metadata(&path_str) {
                    Ok(m) => m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH).duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64,
                    Err(_) => 0,
                };

                // A name is owned by exactly one live item across the whole
                // mesh. If this device already holds the item at this path then
                // the file was edited; if another device's item owns the name,
                // this is an unrelated file that happens to share it.
                let existing_owner = {
                    let db = db.lock().unwrap();
                    match db.get_file_by_path(&relative_path) {
                        Ok(Some(existing)) => {
                            let ours = db.is_holder(&existing.id, &device_id).unwrap_or(false);
                            if ours { None } else { Some(existing) }
                        }
                        _ => None,
                    }
                };

                let (relative_path, path_str, file_id, known_hash) = match existing_owner {
                    None => {
                        let db = db.lock().unwrap();
                        match db.get_file_by_path(&relative_path) {
                            Ok(Some(existing)) => (
                                relative_path,
                                path_str,
                                existing.id,
                                existing.content_hash,
                            ),
                            _ => (relative_path, path_str, new_file_id(&device_id), String::new()),
                        }
                    }
                    Some(existing) => {
                        // Keep both, which cannot lose anything, and surface the
                        // conflict so Override stays available as a decision.
                        let free_path = {
                            let db = db.lock().unwrap();
                            crate::collision::next_available_path(&relative_path, |candidate| {
                                db.is_path_taken(candidate).unwrap_or(true)
                                    || Path::new(&format!("{}/{}", watch_dir, candidate)).exists()
                            })
                        };
                        let free_abs = format!("{}/{}", watch_dir, free_path);

                        // Rename on disk too: on desktop the folder is supposed
                        // to show exactly what this device holds, so the two
                        // must not disagree about what the file is called.
                        crate::ignore::mark_ignored(&ignore_set, &path_str);
                        crate::ignore::mark_ignored(&ignore_set, &free_abs);
                        if let Err(e) = std::fs::rename(&path_str, &free_abs) {
                            println!("[Watcher] Failed to rename colliding file: {}", e);
                            continue;
                        }
                        crate::ignore::schedule_unmark_ignored(
                            ignore_set.clone(),
                            path_str.clone(),
                            3,
                        );
                        crate::ignore::schedule_unmark_ignored(
                            ignore_set.clone(),
                            free_abs.clone(),
                            3,
                        );

                        let file_id = new_file_id(&device_id);
                        collisions.record(crate::collision::PendingCollision {
                            id: uuid::Uuid::new_v4().to_string(),
                            incoming_file_id: file_id.clone(),
                            requested_path: relative_path.clone(),
                            current_path: free_path.clone(),
                            existing_file_id: existing.id.clone(),
                            existing_created_by: existing.created_by.clone(),
                            detected_at: modified_time,
                        });

                        println!(
                            "[Watcher] \"{}\" is taken; kept as \"{}\"",
                            relative_path, free_path
                        );
                        let _ = event_tx.send(EngineEvent::NameCollision {
                            requested_path: relative_path,
                            kept_as: free_path.clone(),
                        });

                        (free_path, free_abs, file_id, String::new())
                    }
                };

                let content_hash = {
                    let db = db.lock().unwrap();

                    // file_blocks references files(id), so the row has to exist
                    // before blocks can be mapped to it.
                    let placeholder = FileMetadata {
                        id: file_id.clone(),
                        path: relative_path.clone(),
                        size: file_size,
                        content_hash: known_hash.clone(),
                        modified_time,
                        created_by: device_id.clone(),
                        trashed_at: 0,
                        trashed_by: String::new(),
                    };
                    if let Err(e) = db.insert_file(&placeholder) {
                        println!("[Watcher] Failed to insert file metadata: {}", e);
                        continue;
                    }

                    let _ = db.clear_blocks_for_file(&file_id);
                    if let Err(e) =
                        storage::chunk_and_store_file(&storage_dir, &db, &file_id, &path_str)
                    {
                        println!("[Watcher] Failed to chunk file: {}", e);
                        continue;
                    }

                    let blocks = db.get_blocks_for_file(&file_id).unwrap_or_default();
                    storage::content_hash(&blocks)
                };

                // Editors touch files without changing them. Comparing the
                // block manifest rather than mtime or size means a rewrite with
                // identical content is correctly treated as a non-event.
                if content_hash == known_hash {
                    continue;
                }

                {
                    let db = db.lock().unwrap();
                    let file_meta = FileMetadata {
                        id: file_id.clone(),
                        path: relative_path.clone(),
                        size: file_size,
                        content_hash: content_hash.clone(),
                        modified_time,
                        created_by: device_id.clone(),
                        trashed_at: 0,
                        trashed_by: String::new(),
                    };
                    if let Err(e) = db.insert_file(&file_meta) {
                        println!("[Watcher] Failed to record content hash: {}", e);
                        continue;
                    }

                    // On desktop the sync folder holds exactly what this device
                    // holds, so a file appearing in it makes this device a
                    // holder of that content.
                    let holder = FileHolder {
                        file_id: file_id.clone(),
                        device_id: device_id.clone(),
                        content_hash,
                        received_at: modified_time,
                    };
                    if let Err(e) = db.set_holder(&holder) {
                        println!("[Watcher] Failed to record holder: {}", e);
                    }
                }

                println!("[Watcher] Indexed and chunked: {}", relative_path);
                let _ = event_tx.send(EngineEvent::FileIndexed { path: relative_path });
            }
        }

        EventKind::Remove(_) => {
            for path in event.paths {
                let path_str = match path.to_str() {
                    Some(s) => s.to_string(),
                    None => continue,
                };

                if crate::ignore::is_ignored(&ignore_set, &path_str) { continue; }

                let relative_path = path_str.strip_prefix(&watch_dir).unwrap_or(&path_str).trim_start_matches('/').to_string();
                if relative_path.is_empty() { continue; }

                println!("[Watcher] Detected local delete: {}", relative_path);

                // Dragging a file out of the sync folder deletes *this device's
                // copy*. Other devices' copies are untouched, and the item
                // stays in the catalog for as long as anyone still holds it.
                let db = db.lock().unwrap();
                match db.get_file_by_path(&relative_path) {
                    Ok(Some(file)) => {
                        let blocks = match db.get_blocks_for_file(&file.id) {
                            Ok(b) => b,
                            Err(e) => {
                                println!("[Watcher] Failed to get blocks for delete: {}", e);
                                continue;
                            }
                        };

                        for b in &blocks {
                            let _ = db.set_block_present(&b.block_id, false);
                            let block_path = storage::get_block_path(&storage_dir, &b.block_id);
                            let _ = std::fs::remove_file(block_path);
                        }

                        if let Err(e) = db.remove_holder(&file.id, &device_id) {
                            println!("[Watcher] Failed to drop holder row: {}", e);
                        }

                        // A conflict about a file that is gone is no longer a
                        // question anyone can answer.
                        collisions.forget_file(&file.id);

                        let remaining = db.holder_count(&file.id).unwrap_or(0);
                        if remaining == 0 {
                            // Last copy. Trash and the 30-day retention that
                            // should catch this are not built yet, so the
                            // catalog entry is kept and simply has no holders
                            // rather than the item disappearing outright.
                            println!(
                                "[Watcher] Last copy of {} removed; item now has no holders",
                                relative_path
                            );
                        } else {
                            println!(
                                "[Watcher] Dropped local copy of {}; {} copies remain",
                                relative_path, remaining
                            );
                        }
                    }
                    Ok(None) => {}
                    Err(e) => println!("[Watcher] DB lookup failed: {}", e),
                }
            }
        }

        _ => {}
    }
}