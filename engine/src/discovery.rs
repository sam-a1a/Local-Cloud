// engine/src/discovery.rs
use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use serde::Serialize;
use std::collections::HashMap;
use std::net::UdpSocket;
use std::sync::{Arc, Mutex, mpsc};
use crate::db::Database;
use crate::ignore::IgnoreSet;
use crate::tls::TrustStore;
use crate::EngineEvent;

const SERVICE_TYPE: &str = "_local-cloud._tcp.local.";

/// A device seen on the local network. Being discovered says nothing about
/// trust: unpaired devices appear here so they can be picked for pairing, and
/// get no catalog or data access until that completes.
#[derive(Clone, Debug, Serialize)]
pub struct DiscoveredDevice {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub url: String,
}

/// device_id -> most recently resolved advertisement for that device.
pub type PeerMap = Arc<Mutex<HashMap<String, DiscoveredDevice>>>;

fn get_local_ip() -> String {
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.connect("8.8.8.8:80").unwrap();
    socket.local_addr().unwrap().ip().to_string()
}

#[derive(Debug)]
struct TrustedPeerServerVerifier {
    trust: TrustStore,
}

impl TrustedPeerServerVerifier {
    fn new(trusted_cert_pems: &[String]) -> Self {
        Self {
            trust: TrustStore::from_pems(trusted_cert_pems),
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
        if self.trust.is_trusted(end_entity) {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "Server cert not in trusted peers list".into(),
            ))
        }
    }

    crate::impl_tls_verifier_methods!();
}

pub fn build_mtls_client(
    cert_pem: &str,
    key_pem: &str,
    trusted_cert_pems: &[String],
) -> Result<reqwest::Client> {
    let (certs, key) = crate::tls::load_certs_and_key(cert_pem, key_pem)?;

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
    device_name: String,
    port: u16,
    event_tx: mpsc::Sender<EngineEvent>,
    known_peers: PeerMap,
) -> Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new()?;

    let short_id = &device_id[..8];
    let host_name = format!("{}.local.", short_id);
    let mut properties = HashMap::new();
    properties.insert("device_id".to_string(), device_id.clone());
    properties.insert("name".to_string(), device_name);
    properties.insert("platform".to_string(), crate::crypto::platform_name().to_string());

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

                let peer_id = match info.get_property("device_id") {
                    Some(p) => p.val_str().to_string(),
                    None => continue,
                };

                // Fall back to the short id so a device with a malformed
                // advertisement is still selectable rather than nameless.
                let name = info
                    .get_property("name")
                    .map(|p| p.val_str().to_string())
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| peer_id.chars().take(8).collect());
                let platform = info
                    .get_property("platform")
                    .map(|p| p.val_str().to_string())
                    .unwrap_or_else(|| "unknown".to_string());

                let peer_port = info.get_port();

                if let Some(peer_ip) = info.get_addresses().iter().next() {
                    let device = DiscoveredDevice {
                        device_id: peer_id.clone(),
                        name,
                        platform,
                        url: format!("https://{}:{}", peer_ip, peer_port),
                    };

                    {
                        let mut peers = known_peers.lock().unwrap();
                        peers.insert(peer_id, device.clone());
                    }

                    let _ = event_tx.send(EngineEvent::PeerDiscovered { device });
                }
            }
        }
    });

    Ok(daemon)
}

async fn fetch_blocks_from_peer(
    client: &reqwest::Client,
    peer_url: &str,
    blocks: &[crate::db::FileBlock],
    db_clone: &Arc<Mutex<Database>>,
    storage_dir_clone: &str,
) -> bool {
    for b in blocks {
        let resp = match client.get(format!("{}/get_block/{}", peer_url, b.block_id)).send().await {
            Ok(r) => r,
            Err(_) => return false,
        };

        if resp.status() != 200 {
            return false;
        }

        if let Ok(data) = resp.bytes().await {
            let _ = crate::storage::write_block(storage_dir_clone, &b.block_id, &data);
            let db = db_clone.lock().unwrap();
            let _ = db.set_block_present(&b.block_id, true);
        } else {
            return false;
        }
    }
    true
}

pub async fn sync_with_peer(
    peer_url: String,
    peer_id: String,
    my_device_id: String,
    db_clone: Arc<Mutex<Database>>,
    storage_dir_clone: String,
    cert_pem: String,
    key_pem: String,
    event_tx: mpsc::Sender<EngineEvent>,
) {
    println!("[Sync] Starting metadata sync with {}", peer_id);

    // 1. Fetch peer cert and build mTLS client
    let untrusted_client = reqwest::Client::builder().danger_accept_invalid_certs(true).build().unwrap();
    let peer_cert_pem = match untrusted_client.get(format!("{}/hello", peer_url)).send().await {
        Ok(res) => match res.text().await { Ok(pem) => pem, Err(_) => return },
        Err(_) => return,
    };
    let _ = crate::storage::save_peer_cert(&storage_dir_clone, &peer_id, &peer_cert_pem);

    let trusted_certs = match crate::storage::load_all_trusted_certs(&storage_dir_clone) {
        Ok(c) => c,
        Err(_) => return,
    };
    let mtls_client = match build_mtls_client(&cert_pem, &key_pem, &trusted_certs) {
        Ok(c) => c,
        Err(_) => return,
    };

    // 2. Fetch peer's file list
    let remote_files: Vec<crate::FileMetadata> = match mtls_client.get(format!("{}/list_files", peer_url)).send().await {
        Ok(res) => match res.json().await { Ok(f) => f, Err(e) => { println!("[Sync] Failed to parse list_files: {}", e); return; } },
        Err(e) => { println!("[Sync] Failed to fetch list_files: {}", e); return; }
    };

    // 3. Upsert remote files and auto-download if we are pinned
    for remote_file in &remote_files {
        let (need_blocks, missing_blocks) = {
            let db = db_clone.lock().unwrap();
            let _ = db.upsert_file_from_peer(remote_file);

            // If we are pinned, check if we are missing blocks
            if remote_file.pinned_devices.contains(&my_device_id) {
                let local_blocks = db.get_blocks_for_file(&remote_file.id).unwrap_or_default();
                let missing: Vec<crate::db::FileBlock> = local_blocks.into_iter().filter(|b| b.is_present == 0).collect();
                (!missing.is_empty(), missing)
            } else {
                (false, vec![])
            }
        }; // db lock dropped

        if need_blocks {
            println!("[Sync] Auto-downloading pinned file: {}", remote_file.path);
            let success = fetch_blocks_from_peer(&mtls_client, &peer_url, &missing_blocks, &db_clone, &storage_dir_clone).await;

            if success {
                // Assemble the file locally so the user sees it
                let db = db_clone.lock().unwrap();
                let blocks = db.get_blocks_for_file(&remote_file.id).unwrap_or_default();
                let output_path = format!("/tmp/local_cloud_sync/{}", remote_file.path);
                let _ = crate::storage::assemble_file_from_blocks(&storage_dir_clone, &output_path, &blocks);

                let _ = event_tx.send(EngineEvent::FileDownloaded { path: remote_file.path.clone() });
            }
        }
    }

    // 4. Push our local files that the peer doesn't have or is outdated on
    let local_files = {
        let db = db_clone.lock().unwrap();
        db.get_all_files().unwrap_or_default()
    };

    for local_file in &local_files {
        let remote_version = remote_files.iter().find(|f| f.path == local_file.path).map(|f| f.version);

        let needs_push = match remote_version {
            Some(v) => local_file.version > v,
            None => true,
        };

        if needs_push {
            let (blocks, blocks_data_present) = {
                let db = db_clone.lock().unwrap();
                let blocks = db.get_blocks_for_file(&local_file.id).unwrap_or_default();
                let all_present = blocks.iter().all(|b| b.is_present == 1);
                (blocks, all_present)
            };

            let push_req = serde_json::json!({
                "file": local_file,
                "blocks": blocks.clone()
            });

            if let Err(e) = mtls_client.post(format!("{}/push_metadata", peer_url)).json(&push_req).send().await {
                println!("[Sync] Failed to push metadata: {}", e);
                continue;
            }

            // If we have the data and peer is pinned to it, push blocks automatically
            if blocks_data_present && local_file.pinned_devices.contains(&peer_id) {
                for b in &blocks {
                    if let Ok(data) = crate::storage::read_block(&storage_dir_clone, &b.block_id) {
                        let _ = mtls_client.post(format!("{}/push_block/{}", peer_url, b.block_id)).body(data).send().await;
                    }
                }
                let _ = mtls_client.post(format!("{}/finalize_file/{}", peer_url, local_file.id)).send().await;
            }
        }
    }

    println!("[Sync] Finished metadata sync with {}", peer_id);
}

pub async fn download_file_on_demand(
    file_id: String,
    db_clone: Arc<Mutex<Database>>,
    storage_dir_clone: String,
    sync_dir_clone: String,
    cert_pem: String,
    key_pem: String,
    known_peers: PeerMap,
    ignore_set: IgnoreSet,
    event_tx: mpsc::Sender<EngineEvent>,
) -> Result<(), String> {
    // 1. Check if we already have it locally
    let (file, blocks, missing) = {
        let db = db_clone.lock().unwrap();
        let file = db.get_file_by_id(&file_id).map_err(|e| e.to_string())?
            .ok_or("File not in DB")?;
        let blocks = db.get_blocks_for_file(&file_id).map_err(|e| e.to_string())?;
        let missing: Vec<crate::db::FileBlock> = blocks.iter().filter(|b| b.is_present == 0).cloned().collect();
        (file, blocks, missing)
    };

    if missing.is_empty() {
        let output_path = format!("{}/{}", sync_dir_clone, file.path);
        crate::storage::assemble_file_from_blocks(&storage_dir_clone, &output_path, &blocks).map_err(|e| e.to_string())?;
        return Ok(());
    }

    // 2. Find a peer who has the blocks
    let peers = known_peers.lock().unwrap().clone();
    if peers.is_empty() {
        return Err("No peers available".to_string());
    }

    let trusted_certs = crate::storage::load_all_trusted_certs(&storage_dir_clone).map_err(|e| e.to_string())?;
    let client = build_mtls_client(&cert_pem, &key_pem, &trusted_certs).map_err(|e| e.to_string())?;

    // Try peers until one works
    for (_peer_id, peer) in peers {
        let peer_url = peer.url;
        let success = fetch_blocks_from_peer(&client, &peer_url, &missing, &db_clone, &storage_dir_clone).await;

        if success {
            let output_path = format!("{}/{}", sync_dir_clone, file.path);
            if let Some(parent) = std::path::Path::new(&output_path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            crate::ignore::mark_ignored(&ignore_set, &output_path);
            crate::storage::assemble_file_from_blocks(&storage_dir_clone, &output_path, &blocks).map_err(|e| e.to_string())?;

            crate::ignore::schedule_unmark_ignored(ignore_set.clone(), output_path, 3);

            let _ = event_tx.send(EngineEvent::FileDownloaded { path: file.path.clone() });
            return Ok(());
        }
    }

    Err("Failed to download from all peers".to_string())
}