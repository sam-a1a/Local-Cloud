use engine::{Database, DeviceIdentity, start_discovery, server};
use anyhow::Result;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    // Explicitly install the ring crypto provider for rustls 0.23
    let _ = rustls::crypto::ring::default_provider().install_default();

    println!("Starting local-cloud-cli...");

    let _database = Database::init("local-cloud.db")?;
    let identity = DeviceIdentity::generate()?;

    println!("Device ID: {}", identity.device_id);

    let handle = tokio::runtime::Handle::current();

    // Bind to port 0 to let the OS pick an available port
    let listener = TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    println!("Starting mDNS discovery on port {}...", port);
    let _daemon = start_discovery(identity.device_id.clone(), port, handle)?;

    println!("Starting HTTPS server...");
    server::start_server(listener, identity).await?;

    Ok(())
}