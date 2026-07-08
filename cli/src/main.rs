use engine::{Database, DeviceIdentity, start_discovery, server, FileMetadata, storage};
use anyhow::Result;
use tokio::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::fs::File;
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

    let local_file_name = format!("test_{}.txt", short_id);
    let mut file = File::create(&local_file_name)?;
    for i in 0..1000 {
        writeln!(file, "This is line {} in the local cloud test file, from device {}.", i, short_id)?;
    }
    file.flush()?;

    let file_size = std::fs::metadata(&local_file_name)?.len() as i64;

    let my_file = FileMetadata {
        id: format!("{}-file1", identity.device_id),
        path: local_file_name.clone(),
        size: file_size,
        modified_time: 1715000000,
        version: 1,
        created_by: identity.device_id.clone(),
    };
    database.insert_file(&my_file)?;

    storage::chunk_and_store_file(&storage_dir, &database, &my_file.id, &local_file_name)?;
    println!("Chunked file into blocks.");

    let db_state = Arc::new(Mutex::new(database));
    let db_for_deletion = db_state.clone();
    let my_file_id = my_file.id.clone();
    let my_device_id = identity.device_id.clone();
    let my_file_path = local_file_name.clone();
    let my_file_version = my_file.version;

    println!("Device ID: {}", identity.device_id);

    let handle = tokio::runtime::Handle::current();

    let listener = TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    println!("Starting mDNS discovery on port {}...", port);
    let _daemon = start_discovery(
        identity.device_id.clone(),
        port,
        handle.clone(),
        db_state.clone(),
        storage_dir.clone(),
    )?;
    handle.spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(15)).await;
        println!("[Demo] Deleting our own file to test tombstone sync...");

        if let Err(e) = std::fs::remove_file(&my_file_path) {
            println!("[Demo] Warning removing file from disk: {}", e);
        }

        let db = db_for_deletion.lock().unwrap();
        match db.delete_file_with_tombstone(&my_file_id, &my_device_id, my_file_version + 1) {
            Ok(_) => println!("[Demo] Tombstone written for {}", my_file_path),
            Err(e) => println!("[Demo] Failed to write tombstone: {}", e),
        }
    });

    println!("Starting HTTPS server...");
    server::start_server(listener, identity, db_state, storage_dir).await?;

    Ok(())
}