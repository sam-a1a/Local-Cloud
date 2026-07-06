use engine::{Database, DeviceIdentity, start_discovery, server};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    println!("Starting local-cloud-cli...");

    let _database = Database::init("local-cloud.db")?;
    let identity = DeviceIdentity::generate()?;

    println!("Device ID: {}", identity.device_id);
    println!("Starting mDNS discovery...");

    let _daemon = start_discovery(identity.device_id.clone(), 8080)?;

    println!("Starting HTTPS server on port 8080...");
    server::start_server(identity).await?;

    Ok(())
}