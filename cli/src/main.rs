use localcloud::{Engine, EngineEvent};
use anyhow::Result;
use std::sync::Arc;
use std::fs;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    println!("Starting local-cloud-cli...");

    let engine = Arc::new(Engine::new(".".to_string()).map_err(|e| anyhow::anyhow!(e))?);

    let short_id = engine.device_short_id();
    let sync_dir = engine.get_sync_dir();

    // Listen for events using UniFFI's async next_event method
    let engine_events = engine.clone();
    tokio::spawn(async move {
        loop {
            let event = engine_events.next_event().await;
            println!("[Event] {:?}", event);
            if let EngineEvent::EngineStopped = event {
                break;
            }
        }
    });

    engine.start().await.map_err(|e| anyhow::anyhow!(e))?;

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

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;
    println!("\nReceived Ctrl+C, shutting down gracefully...");

    engine.stop();
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    Ok(())
}