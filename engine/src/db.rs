use anyhow::Result;
use rusqlite::Connection;

pub struct Database {
    pub conn: Connection,
}

impl Database {
    pub fn init(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        
        // execute_batch ignores returned rows, so it handles PRAGMA perfectly
        conn.execute_batch(
            "
            PRAGMA journal_mode=WAL;
            
            CREATE TABLE IF NOT EXISTS devices (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, cert_pem TEXT NOT NULL,
                paired_at INTEGER NOT NULL, last_seen INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS files (
                id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, size INTEGER NOT NULL,
                modified_time INTEGER NOT NULL, version INTEGER NOT NULL, created_by TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS blocks (
                id TEXT PRIMARY KEY, file_id TEXT NOT NULL, block_index INTEGER NOT NULL,
                size INTEGER NOT NULL, is_present INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (file_id) REFERENCES files(id)
            );
            CREATE TABLE IF NOT EXISTS tombstones (
                file_id TEXT PRIMARY KEY, deleted_at INTEGER NOT NULL,
                deleted_by TEXT NOT NULL, version INTEGER NOT NULL
            );
            "
        )?;
        
        Ok(Self { conn })
    }
}
