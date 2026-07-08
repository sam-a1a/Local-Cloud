use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use std::collections::HashMap;
use std::io::Cursor;
use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use crate::db::Database;
use crate::tls::TrustedCerts;
use crate::EngineEvent;

const SERVICE_TYPE: &str = "_local-cloud._tcp.local.";

fn get_local_ip() -> String {
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.connect("8.8.8.8:80").unwrap();
    socket.local_addr().unwrap().ip().to_string()
}

#[derive(Debug)]
struct TrustedPeerServerVerifier {
    certs: TrustedCerts,
}

impl TrustedPeerServerVerifier {
    fn new(trusted_cert_pems: &[String]) -> Self {
        Self {
            certs: TrustedCerts::new(trusted_cert_pems),
        }
    }
}

impl ServerCertVerifier for TrustedPeerServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if self.certs.is_trusted(end_entity) {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "Server cert not in trusted peers list".into(),
            ))
        }
    }

    crate::impl_tls_verifier_methods!();
}

fn build_mtls_client(
    cert_pem: &str,
    key_pem: &str,
    trusted_cert_pems: &[String],
) -> Result<reqwest::Client> {
    let mut cert_cursor = Cursor::new(cert_pem.as_bytes());
    let mut key_cursor = Cursor::new(key_pem.as_bytes());

    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_cursor)
        .collect::<Result<Vec<_>, _>>()?;

    let key = rustls::pki_types::PrivateKeyDer::from(
        rustls_pemfile::private_key(&mut key_cursor)?.unwrap(),
    );

    let verifier = Arc::new(TrustedPeerServerVerifier::new(trusted_cert_pems));

    let tls_config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(certs, key)?;

    let client = reqwest::Client::builder()
        .use_preconfigured_tls(tls_config)
        .build()?;

    Ok(client)
}

pub fn start_discovery(
    device_id: String,
    port: u16,
    handle: Handle,
    db: Arc<Mutex<Database>>,
    storage_dir: String,
    sync_dir: String,
    cert_pem: String,
    key_pem: String,
    ignore_set: crate::ignore::IgnoreSet,
    event_tx: broadcast::Sender<EngineEvent>,
) -> Result<ServiceDaemon> {
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

                let peer_id = info
                    .get_property("device_id")
                    .map(|p| p.val_str().to_string());
                let peer_port = info.get_port();

                if let Some(peer_ip) = info.get_addresses().iter().next() {
                    if let Some(pid) = peer_id.clone() {
                        let url = format!("https://{}:{}", peer_ip, peer_port);

                        let _ = event_tx.send(EngineEvent::PeerDiscovered {
                            peer_id: pid,
                            addr: url.clone(),
                        });

                        let db_clone = db.clone();
                        let storage_dir_clone = storage_dir.clone();
                        let sync_dir_for_task = sync_dir.clone();
                        let cert_pem_clone = cert_pem.clone();
                        let key_pem_clone = key_pem.clone();
                        let ignore_set_for_task = ignore_set.clone();
                        let event_tx_for_task = event_tx.clone();

                        handle.spawn(async move {
                            sync_with_peer(
                                url,
                                db_clone,
                                storage_dir_clone,
                                sync_dir_for_task,
                                cert_pem_clone,
                                key_pem_clone,
                                ignore_set_for_task,
                                event_tx_for_task,
                            )
                                .await;
                        });
                    }
                }
            }
        }
    });

    Ok(daemon)
}

async fn sync_with_peer(
    url: String,
    db_clone: Arc<Mutex<Database>>,
    storage_dir_clone: String,
    sync_dir_clone: String,
    cert_pem: String,
    key_pem: String,
    ignore_set: crate::ignore::IgnoreSet,
    event_tx: broadcast::Sender<EngineEvent>,
) {
    println!("[Sync] === Starting sync cycle with {} ===", url);

    let untrusted_client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();

    let peer_cert_pem = match untrusted_client
        .get(format!("{}/hello", url))
        .send()
        .await
    {
        Ok(res) => match res.text().await {
            Ok(pem) => pem,
            Err(e) => {
                println!("[FATAL] Failed to read hello response: {}", e);
                return;
            }
        },
        Err(e) => {
            println!("[FATAL] Hello handshake failed: {}", e);
            return;
        }
    };

    println!("[mTLS] Got peer certificate, saving as trusted...");

    if let Err(e) =
        crate::storage::save_peer_cert(&storage_dir_clone, "peer", &peer_cert_pem)
    {
        println!("[FATAL] Failed to save peer cert: {}", e);
        return;
    }

    let trusted_certs = match crate::storage::load_all_trusted_certs(&storage_dir_clone) {
        Ok(c) => c,
        Err(e) => {
            println!("[FATAL] Failed to load trusted certs: {}", e);
            return;
        }
    };

    let mtls_client = match build_mtls_client(&cert_pem, &key_pem, &trusted_certs) {
        Ok(c) => c,
        Err(e) => {
            println!("[FATAL] Failed to build mTLS client: {}", e);
            return;
        }
    };

    println!("[mTLS] mTLS client built, all subsequent requests are mutually authenticated");

    sync_tombstones(&mtls_client, &url, &db_clone, &sync_dir_clone, &event_tx).await;
    sync_files(
        &mtls_client,
        &url,
        &db_clone,
        &storage_dir_clone,
        &sync_dir_clone,
        &ignore_set,
        &event_tx,
    )
        .await;

    println!("[Sync] === Finished sync cycle with {} ===", url);
}

async fn sync_tombstones(
    client: &reqwest::Client,
    url: &str,
    db_clone: &Arc<Mutex<Database>>,
    sync_dir_clone: &str,
    event_tx: &broadcast::Sender<EngineEvent>,
) {
    let res = match client.get(format!("{}/tombstones", url)).send().await {
        Ok(r) => r,
        Err(e) => {
            println!("[FATAL] Tombstone request failed: {}", e);
            return;
        }
    };

    let text = match res.text().await {
        Ok(t) => t,
        Err(e) => {
            println!("[FATAL] Failed to read tombstone response: {}", e);
            return;
        }
    };

    let tombstones: Vec<crate::db::Tombstone> = match serde_json::from_str(&text) {
        Ok(t) => t,
        Err(e) => {
            println!("[FATAL] Failed to parse tombstone JSON: {}", e);
            return;
        }
    };

    println!("[Tombstone] Got {} tombstones from peer", tombstones.len());

    let db = db_clone.lock().unwrap();
    for tombstone in tombstones {
        if db.has_tombstone(&tombstone.file_id).unwrap_or(false) {
            continue;
        }

        if let Ok(Some(local_file)) = db.get_file_by_id(&tombstone.file_id) {
            if tombstone.version >= local_file.version {
                println!("[Tombstone] Deleting local file: {}", local_file.path);

                let full_path = format!("{}/{}", sync_dir_clone, local_file.path);
                if let Err(e) = std::fs::remove_file(&full_path) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        println!("[FATAL] Failed to delete file from disk: {}", e);
                    }
                }

                if let Err(e) = db.delete_file_with_tombstone(
                    &tombstone.file_id,
                    &tombstone.deleted_by,
                    tombstone.version,
                ) {
                    println!("[FATAL] Failed to apply tombstone to DB: {}", e);
                } else {
                    println!("[Tombstone] Applied tombstone for: {}", local_file.path);
                    let _ = event_tx.send(EngineEvent::FileDeleted { path: local_file.path.clone() });
                }
            } else {
                println!(
                    "[Tombstone] Ignoring tombstone for {} (local v{} > tombstone v{})",
                    local_file.path, local_file.version, tombstone.version
                );
            }
        } else {
            if let Err(e) = db.insert_tombstone(&tombstone) {
                println!("[FATAL] Failed to store tombstone: {}", e);
            }
        }
    }
}

async fn sync_files(
    client: &reqwest::Client,
    url: &str,
    db_clone: &Arc<Mutex<Database>>,
    storage_dir_clone: &str,
    sync_dir_clone: &str,
    ignore_set: &crate::ignore::IgnoreSet,
    event_tx: &broadcast::Sender<EngineEvent>,
) {
    let res = match client.get(format!("{}/metadata", url)).send().await {
        Ok(r) => r,
        Err(e) => {
            println!("[FATAL] Metadata request failed: {}", e);
            return;
        }
    };

    let text = match res.text().await {
        Ok(t) => t,
        Err(e) => {
            println!("[FATAL] Failed to read metadata body: {}", e);
            return;
        }
    };

    let files: Vec<crate::FileMetadata> = match serde_json::from_str(&text) {
        Ok(f) => f,
        Err(e) => {
            println!("[FATAL] Failed to parse metadata JSON: {} | raw: {}", e, text);
            return;
        }
    };

    println!("[Sync] Got {} files from peer", files.len());

    let files_to_sync = {
        let db = db_clone.lock().unwrap();
        let mut to_sync = Vec::new();
        for file in files {
            if db.has_tombstone(&file.id).unwrap_or(false) {
                println!("[Sync] Skipping {} — tombstoned locally", file.path);
                continue;
            }

            println!("[Sync] Saving peer file metadata: {}", file.path);
            if let Err(e) = db.upsert_file_from_peer(&file) {
                println!("[FATAL] upsert_file_from_peer failed: {}", e);
            }

            let local_blocks = db.get_blocks_for_file(&file.id).unwrap_or_default();
            let fully_present =
                !local_blocks.is_empty() && local_blocks.iter().all(|b| b.is_present == 1);

            println!(
                "[Sync] File {} -> blocks: {}, fully_present: {}",
                file.path,
                local_blocks.len(),
                fully_present
            );

            if !fully_present {
                to_sync.push(file);
            }
        }
        to_sync
    };

    println!("[Sync] {} files need syncing", files_to_sync.len());

    for file in files_to_sync {
        println!("[Sync] --- Syncing blocks for {} ---", file.path);

        let blocks_url = format!("{}/file_blocks/{}", url, file.id);
        let blocks_res = match client.get(&blocks_url).send().await {
            Ok(r) => r,
            Err(e) => {
                println!("[FATAL] file_blocks request failed: {}", e);
                continue;
            }
        };

        if !blocks_res.status().is_success() {
            println!("[FATAL] file_blocks non-success: {}", blocks_res.status());
            continue;
        }

        let blocks_text = match blocks_res.text().await {
            Ok(t) => t,
            Err(e) => {
                println!("[FATAL] Failed to read file_blocks body: {}", e);
                continue;
            }
        };

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
                if let Err(e) =
                    db.map_block_to_file(&file.id, &p_block.block_id, p_block.block_index)
                {
                    println!("[FATAL] map_block_to_file failed: {}", e);
                }
            }
        }

        for p_block in &peer_blocks {
            let block_path =
                crate::storage::get_block_path(storage_dir_clone, &p_block.block_id);
            if !block_path.exists() {
                let block_url = format!("{}/block/{}", url, p_block.block_id);
                println!("[Sync] Downloading block: {}", &p_block.block_id[..8]);

                match client.get(&block_url).send().await {
                    Ok(block_res) => match block_res.bytes().await {
                        Ok(block_bytes) => {
                            if let Err(e) = crate::storage::write_block(
                                storage_dir_clone,
                                &p_block.block_id,
                                &block_bytes,
                            ) {
                                println!("[FATAL] write_block failed: {}", e);
                            }
                            let db = db_clone.lock().unwrap();
                            if let Err(e) = db.set_block_present(&p_block.block_id, true) {
                                println!("[FATAL] set_block_present failed: {}", e);
                            }
                            println!("[Sync] Downloaded block {}", &p_block.block_id[..8]);
                        }
                        Err(e) => println!("[FATAL] Failed to read block bytes: {}", e),
                    },
                    Err(e) => println!("[FATAL] Block download failed: {}", e),
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

        println!(
            "[Sync] Final blocks for {}: {} total, all_present: {}",
            file.path,
            final_blocks.len(),
            !final_blocks.is_empty() && final_blocks.iter().all(|b| b.is_present == 1)
        );

        if !final_blocks.is_empty() && final_blocks.iter().all(|b| b.is_present == 1) {
            let output_path = format!("{}/{}", sync_dir_clone, file.path);

            if let Some(parent) = std::path::Path::new(&output_path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            crate::ignore::mark_ignored(ignore_set, &output_path);

            match crate::storage::assemble_file_from_blocks(
                storage_dir_clone,
                &output_path,
                &final_blocks,
            ) {
                Ok(_) => {
                    println!("[Sync] Assembled file into sync folder: {}", output_path);
                    let _ = event_tx.send(EngineEvent::FileDownloaded { path: file.path.clone() });
                }
                Err(e) => println!("[FATAL] assemble_file_from_blocks failed: {}", e),
            }

            let ignore_clone = ignore_set.clone();
            let path_clone = output_path.clone();
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                crate::ignore::unmark_ignored(&ignore_clone, &path_clone);
            });
        }
    }
}