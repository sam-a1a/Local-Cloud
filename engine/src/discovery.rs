use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::collections::HashMap;
use std::net::UdpSocket;

const SERVICE_TYPE: &str = "_local-cloud._tcp.local.";

fn get_local_ip() -> String {
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.connect("8.8.8.8:80").unwrap();
    socket.local_addr().unwrap().ip().to_string()
}

pub fn start_discovery(device_id: String, port: u16) -> Result<ServiceDaemon> {
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
                println!("[Discovery] Found peer: {}", peer_name);
            }
        }
    });

    Ok(daemon)
}