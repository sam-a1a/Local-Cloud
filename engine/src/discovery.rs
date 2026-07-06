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

pub fn start_discovery(device_id: String, port: u16, handle: Handle, db: Arc<Mutex<Database>>) -> Result<ServiceDaemon> {
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

                        let url = format!("https://{}:{}/metadata", peer_ip, peer_port);
                        let db_clone = db.clone();

                        handle.spawn(async move {
                            let client = reqwest::Client::builder()
                                .danger_accept_invalid_certs(true)
                                .build()
                                .unwrap();

                            if let Ok(res) = client.get(&url).send().await {
                                if let Ok(text) = res.text().await {
                                    if let Ok(files) = serde_json::from_str::<Vec<crate::FileMetadata>>(&text) {
                                        let db = db_clone.lock().unwrap();
                                        for file in files {
                                            println!("[Sync] Saving peer file metadata: {}", file.path);
                                            let _ = db.upsert_file_from_peer(&file);
                                        }
                                    }
                                }
                            }
                        });
                    }
                }
            }
        }
    });

    Ok(daemon)
}