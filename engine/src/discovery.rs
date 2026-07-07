use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::collections::HashMap;
use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;
use crate::db::Database;

const SERVICE_TYPE: &str = "_local-cloud._tcp.local.";

fn get_local_ip() -> String {
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.connect("8.8.8.8:80").unwrap();
    socket.local_addr().unwrap().ip().to_string()
}

pub fn start_discovery(device_id: String, port: u16, handle: Handle, db: Arc<Mutex<Database>>, storage_dir: String) -> Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new()?;

    let short_id = &device_id[..8];
    let host_name = format!("{}.local.", short_id);
    let mut properties = HashMap::new();
    properties.insert("device_id".to_string(), device_id.clone());

    let local_ip = get_local_ip();
    let my_properties = Some(properties);

    let service_info = ServiceInfo::new(
        SERVICE_TYPE,
        short_id,
        &host_name,
        &local_ip,
        port,
        my_properties,
    )?;

    daemon.register(service_info)?;

    let receiver = daemon.browse(SERVICE_TYPE)?;
    let my_short_id = short_id.to_string();

    std::thread::spawn(move || {
        while let Ok(event) = receiver.recv() {
            if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
                let peer_name = info.get_fullname();
                if peer_name.starts_with(&my_short_id) {
                    continue;
                }

                let peer_id = info.get_property("device_id").map(|p| p.val_str().to_string());
                let peer_port = info.get_port();

                if let Some(peer_ip) = info.get_addresses().iter().next() {
                    if let Some(peer_id) = peer_id {
                        println!("[Discovery] Found peer: {} at {}:{}", peer_id, peer_ip, peer_port);

                        let url = format!("https://{}:{}", peer_ip, peer_port);
                        let db_clone = db.clone();
                        let storage_dir_clone = storage_dir.clone();

                        handle.spawn(async move {
                            sync_with_peer(url, db_clone, storage_dir_clone).await;
                        });
                    }
                }
            }
        }
    });

    Ok(daemon)
}

async fn sync_with_peer(url: String, db_clone: Arc<Mutex<Database>>, storage_dir_clone: String) {
    println!("[Sync] === Starting sync cycle with {} ===", url);

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();

    let meta_res = client.get(format!("{}/metadata", url)).send().await;

    let res = match meta_res {
        Ok(r) => r,
        Err(e) => {
            println!("[FATAL] Metadata request failed to send: {}", e);
            return;
        }
    };

    let text = match res.text().await {
        Ok(t) => t,
        Err(e) => {
            println!("[FATAL] Failed to read metadata response body: {}", e);
            return;
        }
    };

    let files: Vec<crate::FileMetadata> = match serde_json::from_str(&text) {
        Ok(f) => f,
        Err(e) => {
            println!("[FATAL] Failed to parse metadata JSON: {} | raw text: {}", e, text);
            return;
        }
    };

    println!("[Sync] Got {} files from peer metadata", files.len());

    let files_to_sync = {
        let db = db_clone.lock().unwrap();
        let mut to_sync = Vec::new();
        for file in files {
            println!("[Sync] Saving peer file metadata: {}", file.path);
            if let Err(e) = db.upsert_file_from_peer(&file) {
                println!("[FATAL] upsert_file_from_peer failed: {}", e);
            }

            let local_blocks = match db.get_blocks_for_file(&file.id) {
                Ok(b) => b,
                Err(e) => {
                    println!("[FATAL] get_blocks_for_file failed: {}", e);
                    Vec::new()
                }
            };
            let fully_present = !local_blocks.is_empty() && local_blocks.iter().all(|b| b.is_present == 1);
            println!("[Sync] File {} -> local_blocks count: {}, fully_present: {}", file.path, local_blocks.len(), fully_present);

            if !fully_present {
                to_sync.push(file);
            }
        }
        to_sync
    };

    println!("[Sync] {} files need syncing", files_to_sync.len());

    for file in files_to_sync {
        println!("[Sync] --- Syncing blocks for {} (id: {}) ---", file.path, file.id);

        let blocks_url = format!("{}/file_blocks/{}", url, file.id);
        println!("[Sync] Requesting: {}", blocks_url);

        let blocks_res = match client.get(&blocks_url).send().await {
            Ok(r) => r,
            Err(e) => {
                println!("[FATAL] file_blocks request failed to send: {}", e);
                continue;
            }
        };

        let status = blocks_res.status();
        println!("[Sync] file_blocks response status: {}", status);

        if !status.is_success() {
            println!("[FATAL] file_blocks returned non-success status: {}", status);
            continue;
        }

        let blocks_text = match blocks_res.text().await {
            Ok(t) => t,
            Err(e) => {
                println!("[FATAL] Failed to read file_blocks response body: {}", e);
                continue;
            }
        };

        println!("[Sync] file_blocks raw response: {}", blocks_text);

        let peer_blocks: Vec<crate::db::FileBlock> = match serde_json::from_str(&blocks_text) {
            Ok(b) => b,
            Err(e) => {
                println!("[FATAL] Failed to parse file_blocks JSON: {}", e);
                continue;
            }
        };

        println!("[Sync] Parsed {} peer blocks", peer_blocks.len());

        {
            let db = db_clone.lock().unwrap();
            for p_block in &peer_blocks {
                let block_meta = crate::db::BlockMetadata {
                    id: p_block.block_id.clone(),
                    size: p_block.size,
                    is_present: 0,
                };
                if let Err(e) = db.insert_block(&block_meta) {
                    println!("[FATAL] insert_block failed: {}", e);
                }
                if let Err(e) = db.map_block_to_file(&file.id, &p_block.block_id, p_block.block_index) {
                    println!("[FATAL] map_block_to_file failed: {}", e);
                }
            }
        }

        for p_block in &peer_blocks {
            let block_path = crate::storage::get_block_path(&storage_dir_clone, &p_block.block_id);
            if !block_path.exists() {
                let block_url = format!("{}/block/{}", url, p_block.block_id);
                println!("[Sync] Downloading block from: {}", block_url);

                match client.get(&block_url).send().await {
                    Ok(block_res) => {
                        match block_res.bytes().await {
                            Ok(block_bytes) => {
                                if let Err(e) = crate::storage::write_block(&storage_dir_clone, &p_block.block_id, &block_bytes) {
                                    println!("[FATAL] write_block failed: {}", e);
                                }
                                let db = db_clone.lock().unwrap();
                                if let Err(e) = db.set_block_present(&p_block.block_id, true) {
                                    println!("[FATAL] set_block_present failed: {}", e);
                                }
                                println!("[Sync] Downloaded block {}", &p_block.block_id[..8]);
                            }
                            Err(e) => println!("[FATAL] Failed to read block bytes: {}", e),
                        }
                    }
                    Err(e) => println!("[FATAL] Block download request failed: {}", e),
                }
            } else {
                let db = db_clone.lock().unwrap();
                if let Err(e) = db.set_block_present(&p_block.block_id, true) {
                    println!("[FATAL] set_block_present (existing) failed: {}", e);
                }
            }
        }

        let final_blocks = {
            let db = db_clone.lock().unwrap();
            db.get_blocks_for_file(&file.id).unwrap_or_default()
        };

        println!("[Sync] Final blocks for {}: {} total, all_present: {}",
                 file.path, final_blocks.len(),
                 !final_blocks.is_empty() && final_blocks.iter().all(|b| b.is_present == 1));

        if !final_blocks.is_empty() && final_blocks.iter().all(|b| b.is_present == 1) {
            match crate::storage::assemble_file_from_blocks(&storage_dir_clone, &file.path, &final_blocks) {
                Ok(_) => println!("[Sync] Assembled file: {}", file.path),
                Err(e) => println!("[FATAL] assemble_file_from_blocks failed: {}", e),
            }
        }
    }

    println!("[Sync] === Finished sync cycle with {} ===", url);
}