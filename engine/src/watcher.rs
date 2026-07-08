use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use crate::db::{Database, FileMetadata};
use crate::ignore::IgnoreSet;
use crate::storage;
use crate::EngineEvent;

type DebounceMap = Arc<Mutex<HashMap<String, Instant>>>;

pub fn start_watcher(
    watch_dir: String,
    storage_dir: String,
    device_id: String,
    db: Arc<Mutex<Database>>,
    handle: Handle,
    ignore_set: IgnoreSet,
    event_tx: broadcast::Sender<EngineEvent>,
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
    event_tx: broadcast::Sender<EngineEvent>,
) {
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) => {
            for path in event.paths {
                if !path.is_file() {
                    continue;
                }

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

                // Debounce: skip if we processed this exact path within the last 500ms
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

                let relative_path = path_str
                    .strip_prefix(&watch_dir)
                    .unwrap_or(&path_str)
                    .trim_start_matches('/')
                    .to_string();

                if relative_path.is_empty() {
                    continue;
                }

                println!("[Watcher] Detected create/modify: {}", relative_path);

                // Small delay to let the OS finish flushing the write before we hash it
                tokio::time::sleep(Duration::from_millis(200)).await;

                let file_size = match std::fs::metadata(&path_str) {
                    Ok(m) => m.len() as i64,
                    Err(e) => {
                        println!("[Watcher] Failed to stat file: {}", e);
                        continue;
                    }
                };

                let modified_time = match std::fs::metadata(&path_str) {
                    Ok(m) => m
                        .modified()
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64,
                    Err(_) => 0,
                };

                let (file_id, new_version, should_process) = {
                    let db = db.lock().unwrap();
                    match db.get_file_by_path(&relative_path) {
                        Ok(Some(existing)) => {
                            if existing.size == file_size {
                                (existing.id.clone(), existing.version, false)
                            } else {
                                (existing.id.clone(), existing.version + 1, true)
                            }
                        }
                        _ => {
                            let new_id = format!(
                                "{}-{}",
                                device_id,
                                uuid::Uuid::new_v4().to_string().replace("-", "")
                            );
                            (new_id, 1, true)
                        }
                    }
                };

                if !should_process {
                    continue;
                }

                let file_meta = FileMetadata {
                    id: file_id.clone(),
                    path: relative_path.clone(),
                    size: file_size,
                    modified_time,
                    version: new_version,
                    created_by: device_id.clone(),
                };

                {
                    let db = db.lock().unwrap();
                    if let Err(e) = db.insert_file(&file_meta) {
                        println!("[Watcher] Failed to insert file metadata: {}", e);
                        continue;
                    }
                    if let Err(e) = storage::chunk_and_store_file(
                        &storage_dir,
                        &db,
                        &file_id,
                        &path_str,
                    ) {
                        println!("[Watcher] Failed to chunk file: {}", e);
                        continue;
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

                if crate::ignore::is_ignored(&ignore_set, &path_str) {
                    continue;
                }

                let relative_path = path_str
                    .strip_prefix(&watch_dir)
                    .unwrap_or(&path_str)
                    .trim_start_matches('/')
                    .to_string();

                if relative_path.is_empty() {
                    continue;
                }

                println!("[Watcher] Detected delete: {}", relative_path);

                let db = db.lock().unwrap();
                match db.get_file_by_path(&relative_path) {
                    Ok(Some(file)) => {
                        match db.delete_file_with_tombstone(
                            &file.id,
                            &device_id,
                            file.version + 1,
                        ) {
                            Ok(_) => {
                                println!(
                                    "[Watcher] Tombstone written for deleted file: {}",
                                    relative_path
                                );
                                let _ = event_tx.send(EngineEvent::FileDeleted { path: relative_path });
                            }
                            Err(e) => {
                                println!("[Watcher] Failed to write tombstone: {}", e)
                            }
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