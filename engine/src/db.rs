use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

pub struct Database {
    pub conn: Connection,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileMetadata {
    pub id: String,
    pub path: String,
    pub size: i64,
    pub modified_time: i64,
    pub version: i64,
    pub created_by: String,
}

impl Database {
    pub fn init(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
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

    pub fn insert_file(&self, file: &FileMetadata) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO files (id, path, size, modified_time, version, created_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![file.id, file.path, file.size, file.modified_time, file.version, file.created_by],
        )?;
        Ok(())
    }

    pub fn get_all_files(&self) -> Result<Vec<FileMetadata>> {
        let mut stmt = self.conn.prepare("SELECT id, path, size, modified_time, version, created_by FROM files")?;
        let files = stmt.query_map([], |row| {
            Ok(FileMetadata {
                id: row.get(0)?,
                path: row.get(1)?,
                size: row.get(2)?,
                modified_time: row.get(3)?,
                version: row.get(4)?,
                created_by: row.get(5)?,
            })
        })?;

        let mut result = Vec::new();
        for file in files {
            result.push(file?);
        }
        Ok(result)
    }
}