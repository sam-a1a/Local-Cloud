use engine::Engine;
use anyhow::Result;
use std::fs;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    println!("Starting local-cloud-cli...");

    let handle = tokio::runtime::Handle::current();

    // Engine will create ./identity.json, ./local-cloud-<id>.db, ./sync_<id>, etc.
    let mut engine = Engine::new(".", handle)?;

    let short_id = engine.device_short_id().to_string();
    let sync_dir = engine.get_sync_dir().to_string();

    // Subscribe to engine events
    let mut rx = engine.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            println!("[Event] {:?}", event);
        }
    });

    engine.start().await?;

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

    Ok(())
}