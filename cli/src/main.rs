// cli/src/main.rs
use localcloud::{Engine, EngineEvent, EventListener};
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
    let sync_dir = engine.sync_dir();

    // Everything this consumer does with events. Registered before `start`, so
    // nothing is missed - including EngineStarted.
    //
    // Syncing on discovery used to be done here. The engine does that itself
    // now, and keeps at it on an interval, so a consumer that only watches
    // still ends up with a true catalog.
    struct Printer;
    impl EventListener for Printer {
        fn on_event(&self, event: EngineEvent) {
            println!("[Event] {:?}", event);
        }
    }
    engine.set_event_listener(Arc::new(Printer));

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

    // Demo: share the first file with the first paired device after 5 seconds.
    // Nothing is transferred unless a person asks for it, and only paired
    // devices can receive anything, so this does nothing until pairing has run.
    let engine_share = engine.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        let devices = engine_share.paired_devices();
        let files = engine_share.local_files();

        match (devices.first(), files.first()) {
            (Some(device), Some(file)) => {
                println!("[Demo] Sharing {} with {}", file.path, device.name);
                if let Err(e) = engine_share.share_to(file.id.clone(), vec![device.id.clone()]) {
                    println!("[Demo] Share failed: {}", e);
                }
            }
            (None, _) => println!("[Demo] No paired devices yet - pair one first"),
            (_, None) => println!("[Demo] No files indexed yet"),
        }
    });

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;
    println!("\nReceived Ctrl+C, shutting down gracefully...");

    engine.stop();
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    Ok(())
}