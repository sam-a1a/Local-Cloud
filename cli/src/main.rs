use engine::{Database, DeviceIdentity, start_discovery};
use anyhow::Result;
use std::time::Duration;

fn main() -> Result<()> {
    println!("Starting local-cloud-cli...");
    
    let _database = Database::init("local-cloud.db")?;
    let identity = DeviceIdentity::generate()?;
    
    println!("Device ID: {}", identity.device_id);
    println!("Starting mDNS discovery...");
    
    let _daemon = start_discovery(identity.device_id, 8080)?;
    
    loop {
        std::thread::sleep(Duration::from_secs(5));
    }
}
