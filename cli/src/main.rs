use engine::{Database, DeviceIdentity};
use anyhow::Result;

fn main() -> Result<()> {
    println!("Starting local-cloud-cli...");
    
    let _database = Database::init("local-cloud.db")?;
    println!("Database initialized successfully.");

    println!("Generating device identity...");
    let identity = DeviceIdentity::generate()?;
    
    println!("Success! Device ID: {}", identity.device_id);
    println!("TLS Cert generated (first 40 chars): {}...", &identity.cert_pem[..40]);

    Ok(())
}
