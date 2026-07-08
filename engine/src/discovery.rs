use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use std::collections::HashMap;
use std::io::Cursor;
use std::net::UdpSocket;
use std::sync::{Arc, Mutex, mpsc};
use crate::db::Database;
use crate::tls::TrustedCerts;
use crate::EngineEvent;
use crate::ignore::IgnoreSet;

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
    event_tx: mpsc::Sender<EngineEvent>,
    known_peers: Arc<Mutex<HashMap<String, String>>>,
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

                        {
                            let mut peers = known_peers.lock().unwrap();
                            peers.insert(pid.clone(), url.clone());
                        }

                        let _ = event_tx.send(EngineEvent::PeerDiscovered { peer_id: pid, addr: url });
                    }
                }
            }
        }
    });

    Ok(daemon)
}

pub async fn push_file_to_peer(
    peer_url: String,
    peer_id: String,
    file_id: String,
    db_clone: Arc<Mutex<Database>>,
    storage_dir_clone: String,
    _sync_dir_clone: String,
    cert_pem: String,
    key_pem: String,
    _ignore_set: IgnoreSet,
    event_tx: mpsc::Sender<EngineEvent>,
) {
    println!("[Push] === Starting push of {} to {} ===", file_id, peer_id);

    // 1. mTLS Handshake
    let untrusted_client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();

    let peer_cert_pem = match untrusted_client.get(format!("{}/hello", peer_url)).send().await {
        Ok(res) => match res.text().await {
            Ok(pem) => pem,
            Err(e) => {
                let _ = event_tx.send(EngineEvent::ErrorEvent { message: format!("Push failed: {}", e) });
                return;
            }
        },
        Err(e) => {
            let _ = event_tx.send(EngineEvent::ErrorEvent { message: format!("Push handshake failed: {}", e) });
            return;
        }
    };

    // Use peer_id for the cert filename to support multiple devices
    let peer_cert_filename = format!("{}.pem", peer_id);
    if let Err(e) = crate::storage::save_peer_cert(&storage_dir_clone, &peer_cert_filename, &peer_cert_pem) {
        let _ = event_tx.send(EngineEvent::ErrorEvent { message: format!("Failed to save peer cert: {}", e) });
        return;
    }

    let trusted_certs = match crate::storage::load_all_trusted_certs(&storage_dir_clone) {
        Ok(c) => c,
        Err(e) => {
            let _ = event_tx.send(EngineEvent::ErrorEvent { message: format!("Failed to load trusted certs: {}", e) });
            return;
        }
    };

    let mtls_client = match build_mtls_client(&cert_pem, &key_pem, &trusted_certs) {
        Ok(c) => c,
        Err(e) => {
            let _ = event_tx.send(EngineEvent::ErrorEvent { message: format!("Failed to build mTLS client: {}", e) });
            return;
        }
    };

    // 2. Fetch File & Blocks from DB
    let (file, blocks) = {
        let db = db_clone.lock().unwrap();
        let file = match db.get_file_by_id(&file_id) {
            Ok(Some(f)) => f,
            Ok(None) => {
                let _ = event_tx.send(EngineEvent::ErrorEvent { message: "File not found locally".to_string() });
                return;
            }
            Err(e) => {
                let _ = event_tx.send(EngineEvent::ErrorEvent { message: format!("DB error: {}", e) });
                return;
            }
        };
        let blocks = match db.get_blocks_for_file(&file_id) {
            Ok(b) => b,
            Err(e) => {
                let _ = event_tx.send(EngineEvent::ErrorEvent { message: format!("DB block error: {}", e) });
                return;
            }
        };
        (file, blocks)
    };

    // 3. Push Metadata (Clone file and blocks so we can use them later)
    let push_req = serde_json::json!({
        "file": file.clone(),
        "blocks": blocks.clone()
    });

    if let Err(e) = mtls_client.post(format!("{}/push_metadata", peer_url)).json(&push_req).send().await {
        let _ = event_tx.send(EngineEvent::ErrorEvent { message: format!("Push metadata failed: {}", e) });
        return;
    }

    // 4. Push Blocks
    for b in &blocks {
        let data = match crate::storage::read_block(&storage_dir_clone, &b.block_id) {
            Ok(d) => d,
            Err(e) => {
                let _ = event_tx.send(EngineEvent::ErrorEvent { message: format!("Failed to read block: {}", e) });
                return;
            }
        };

        if let Err(e) = mtls_client.post(format!("{}/push_block/{}", peer_url, b.block_id)).body(data).send().await {
            let _ = event_tx.send(EngineEvent::ErrorEvent { message: format!("Push block failed: {}", e) });
            return;
        }
    }

    // 5. Finalize File on Peer
    if let Err(e) = mtls_client.post(format!("{}/finalize_file/{}", peer_url, file_id)).send().await {
        let _ = event_tx.send(EngineEvent::ErrorEvent { message: format!("Finalize file failed: {}", e) });
        return;
    }

    let _ = event_tx.send(EngineEvent::FileSent { path: file.path.clone() });
    println!("[Push] === Finished push of {} to {} ===", file_id, peer_id);
}