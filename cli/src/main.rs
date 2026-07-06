use engine::{Database, DeviceIdentity, start_discovery, server, FileMetadata};
use anyhow::Result;
use tokio::net::TcpListener;
use std::sync::{Arc, Mutex};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    println!("Starting local-cloud-cli...");

    let database = Database::init("local-cloud.db")?;
    let identity = DeviceIdentity::generate()?;

    let short_id = &identity.device_id[..8];
    let dummy_file = FileMetadata {
        id: format!("{}/file1", identity.device_id),
        path: format!("test_{}.txt", short_id),
        size: 1024,
        modified_time: 1715000000,
        version: 1,
        created_by: identity.device_id.clone(),
    };
    database.insert_file(&dummy_file)?;

    let db_state = Arc::new(Mutex::new(database));

    println!("Device ID: {}", identity.device_id);

    let handle = tokio::runtime::Handle::current();

    let listener = TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    println!("Starting mDNS discovery on port {}...", port);
    let _daemon = start_discovery(identity.device_id.clone(), port, handle, db_state.clone())?;

    println!("Starting HTTPS server...");
    server::start_server(listener, identity, db_state).await?;

    Ok(())
}