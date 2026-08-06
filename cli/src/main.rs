// cli/src/main.rs
use localcloud::{Engine, EngineEvent};
use anyhow::Result;
use std::sync::Arc;
use std::fs;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    println!("Starting local-cloud-cli...");

    let engine = Arc::new(Engine::new(".".to_string(), "./sync_folder".to_string()).map_err(|e| anyhow::anyhow!(e.to_string()))?);

    let short_id = engine.device_short_id();
    let sync_dir = engine.get_sync_dir();

    // Listen for events by polling in a background blocking thread
    let engine_events = engine.clone();
    tokio::spawn(async move {
        loop {
            let eng = engine_events.clone();
            let event = tokio::task::spawn_blocking(move || {
                eng.poll_event(500)
            }).await.unwrap();

            if let Some(event) = event {
                println!("[Event] {:?}", event);

                // Automatically synchronize metadata when a peer is discovered
                // Use a reference (&event) to avoid moving the values out
                if let EngineEvent::PeerDiscovered { device } = &event {
                    let engine_sync = engine_events.clone();
                    let peer_id = device.device_id.clone();
                    let addr = device.url.clone();
                    tokio::spawn(async move {
                        let _ = engine_sync.sync_with_peer(peer_id, addr);
                    });
                }

                // Now we can still use `event` here because we didn't move it above
                if let EngineEvent::EngineStopped = event {
                    break;
                }
            }
        }
    });

    engine.start().map_err(|e| anyhow::anyhow!(e.to_string()))?;

    // Drop a test file into the sync folder after 2 seconds
    let sync_dir_clone = sync_dir.clone();
    let short_id_clone = short_id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        let file_path = format!("{}/hello_from_{}.txt", sync_dir_clone, short_id_clone);
        if let Ok(mut f) = fs::File::create(&file_path) {
            for i in 0..500 {
                let _ = writeln!(f, "Line {} from device {}", i, short_id_clone);
            }
            println!("[Demo] Dropped test file into sync folder: {}", file_path);
        }
    });

    // Demo: Auto-pin the first file to the first peer after 5 seconds
    // This will force the peer to automatically download the data blocks
    let engine_pin = engine.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        let peers = engine_pin.get_known_peers();
        let files = engine_pin.get_local_files();
        if !peers.is_empty() && !files.is_empty() {
            let peer_id = peers.first().unwrap().device_id.clone();
            let file_id = files.first().unwrap().id.clone();
            println!("[Demo] Auto-pinning file {} to peer {}", file_id, peer_id);
            let _ = engine_pin.set_file_pinned_devices(file_id, vec![peer_id]);
        }
    });

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;
    println!("\nReceived Ctrl+C, shutting down gracefully...");

    engine.stop();
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    Ok(())
}