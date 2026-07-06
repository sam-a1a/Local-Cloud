use engine::{Database, DeviceIdentity, start_discovery, server, FileMetadata};
use anyhow::Result;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    println!("Starting local-cloud-cli...");

    let database = Database::init("local-cloud.db")?;
    let identity = DeviceIdentity::generate()?;

    // Insert a dummy file to test metadata sync
    let dummy_file = FileMetadata {
        id: format!("{}/file1", identity.device_id),
        path: "test.txt".to_string(),
        size: 1024,
        modified_time: 1715000000,
        version: 1,
        created_by: identity.device_id.clone(),
    };
    database.insert_file(&dummy_file)?;

    println!("Device ID: {}", identity.device_id);

    let handle = tokio::runtime::Handle::current();

    let listener = TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    println!("Starting mDNS discovery on port {}...", port);
    let _daemon = start_discovery(identity.device_id.clone(), port, handle)?;

    println!("Starting HTTPS server...");
    server::start_server(listener, identity, database).await?;

    Ok(())
}