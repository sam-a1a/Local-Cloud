use engine::Database;
use anyhow::Result;

fn main() -> Result<()> {
    println!("Starting local-cloud-cli...");
    let _database = Database::init("local-cloud.db")?;
    println!("Database initialized successfully.");
    Ok(())
}
