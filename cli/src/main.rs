use engine::{Database, DeviceIdentity, start_discovery, start_watcher, server, storage, new_ignore_set};
use anyhow::Result;
use tokio::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::fs;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    println!("Starting local-cloud-cli...");

    let identity = DeviceIdentity::generate()?;
    let short_id = &identity.device_id[..8];

    let db_path = format!("local-cloud-{}.db", short_id);
    let database = Database::init(&db_path)?;

    let storage_dir = format!("./storage_{}", short_id);
    storage::ensure_storage_dir(&storage_dir)?;
    storage::ensure_trusted_peers_dir(&storage_dir)?;

    // Create the sync folder, then canonicalize it to an absolute path.
    // This MUST match the absolute path format that the `notify` crate
    // reports for file system events, or path comparisons will silently fail.
    let sync_dir_relative = format!("./sync_{}", short_id);
    fs::create_dir_all(&sync_dir_relative)?;
    let sync_dir = fs::canonicalize(&sync_dir_relative)?
        .to_string_lossy()
        .to_string();
    println!("Sync folder (canonical): {}", sync_dir);

    let db_state = Arc::new(Mutex::new(database));
    let ignore_set = new_ignore_set();

    println!("Device ID: {}", identity.device_id);

    let handle = tokio::runtime::Handle::current();

    let _watcher = start_watcher(
        sync_dir.clone(),
        storage_dir.clone(),
        identity.device_id.clone(),
        db_state.clone(),
        handle.clone(),
        ignore_set.clone(),
    )?;

    let sync_dir_clone = sync_dir.clone();
    let short_id_owned = short_id.to_string();
    handle.spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        let file_path = format!("{}/hello_from_{}.txt", sync_dir_clone, short_id_owned);
        if let Ok(mut f) = fs::File::create(&file_path) {
            for i in 0..500 {
                let _ = writeln!(f, "Line {} from device {}", i, short_id_owned);
            }
            println!("[Demo] Dropped test file into sync folder: {}", file_path);
        }
    });

    let listener = TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    println!("Starting mDNS discovery on port {}...", port);
    let _daemon = start_discovery(
        identity.device_id.clone(),
        port,
        handle.clone(),
        db_state.clone(),
        storage_dir.clone(),
        sync_dir.clone(),
        identity.cert_pem.clone(),
        identity.key_pem.clone(),
        ignore_set.clone(),
    )?;

    println!("Starting HTTPS server...");
    server::start_server(listener, identity, db_state, storage_dir).await?;

    Ok(())
}