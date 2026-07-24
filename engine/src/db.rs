// engine/src/db.rs
use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use rusqlite::OptionalExtension;

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
    pub pinned_devices: Vec<String>, // NEW: Tracks which devices should hold the data
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlockMetadata {
    pub id: String,
    pub size: i64,
    pub is_present: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileBlock {
    pub block_id: String,
    pub block_index: i64,
    pub size: i64,
    pub is_present: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Tombstone {
    pub file_id: String,
    pub deleted_at: i64,
    pub deleted_by: String,
    pub version: i64,
}

fn file_from_row(row: &rusqlite::Row) -> rusqlite::Result<FileMetadata> {
    let pinned_str: String = row.get(6)?;
    let pinned_devices: Vec<String> = serde_json::from_str(&pinned_str).unwrap_or_default();
    Ok(FileMetadata {
        id: row.get(0)?,
        path: row.get(1)?,
        size: row.get(2)?,
        modified_time: row.get(3)?,
        version: row.get(4)?,
        created_by: row.get(5)?,
        pinned_devices,
    })
}

impl Database {
    pub fn init(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "
        PRAGMA journal_mode=WAL;
        PRAGMA foreign_keys=ON;

            CREATE TABLE IF NOT EXISTS devices (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, cert_pem TEXT NOT NULL,
                paired_at INTEGER NOT NULL, last_seen INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS files (
                id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, size INTEGER NOT NULL,
                modified_time INTEGER NOT NULL, version INTEGER NOT NULL, created_by TEXT NOT NULL,
                pinned_devices TEXT NOT NULL DEFAULT '[]'
            );
            CREATE TABLE IF NOT EXISTS blocks (
                id TEXT PRIMARY KEY, size INTEGER NOT NULL, is_present INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS file_blocks (
                file_id TEXT NOT NULL, block_id TEXT NOT NULL, block_index INTEGER NOT NULL,
                PRIMARY KEY (file_id, block_id),
                FOREIGN KEY (file_id) REFERENCES files(id),
                FOREIGN KEY (block_id) REFERENCES blocks(id)
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
        let pinned_str = serde_json::to_string(&file.pinned_devices)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO files (id, path, size, modified_time, version, created_by, pinned_devices) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![file.id, file.path, file.size, file.modified_time, file.version, file.created_by, pinned_str],
        )?;
        Ok(())
    }

    pub fn get_all_files(&self) -> Result<Vec<FileMetadata>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, size, modified_time, version, created_by, pinned_devices FROM files"
        )?;
        let files = stmt.query_map([], file_from_row)?;

        let mut result = Vec::new();
        for file in files {
            result.push(file?);
        }
        Ok(result)
    }

    pub fn get_file_by_id(&self, file_id: &str) -> Result<Option<FileMetadata>> {
        let result = self.conn.query_row(
            "SELECT id, path, size, modified_time, version, created_by, pinned_devices FROM files WHERE id = ?1",
            rusqlite::params![file_id],
            file_from_row,
        ).optional()?;
        Ok(result)
    }

    pub fn upsert_file_from_peer(&self, file: &FileMetadata) -> Result<()> {
        let pinned_str = serde_json::to_string(&file.pinned_devices)?;
        let existing_version: Option<i64> = self.conn.query_row(
            "SELECT version FROM files WHERE path = ?1",
            rusqlite::params![file.path],
            |row| row.get(0),
        ).optional()?;

        if let Some(existing_version) = existing_version {
            if file.version > existing_version {
                self.conn.execute(
                    "UPDATE files SET id = ?1, size = ?2, modified_time = ?3, version = ?4, created_by = ?5, pinned_devices = ?6 WHERE path = ?7",
                    rusqlite::params![file.id, file.size, file.modified_time, file.version, file.created_by, pinned_str, file.path],
                )?;
            }
        } else {
            self.conn.execute(
                "INSERT INTO files (id, path, size, modified_time, version, created_by, pinned_devices) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![file.id, file.path, file.size, file.modified_time, file.version, file.created_by, pinned_str],
            )?;
        }
        Ok(())
    }

    pub fn insert_block(&self, block: &BlockMetadata) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO blocks (id, size, is_present) VALUES (?1, ?2, ?3)",
            rusqlite::params![block.id, block.size, block.is_present],
        )?;
        Ok(())
    }

    pub fn map_block_to_file(&self, file_id: &str, block_id: &str, block_index: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO file_blocks (file_id, block_id, block_index) VALUES (?1, ?2, ?3)",
            rusqlite::params![file_id, block_id, block_index],
        )?;
        Ok(())
    }

    pub fn clear_blocks_for_file(&self, file_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM file_blocks WHERE file_id = ?1",
            rusqlite::params![file_id],
        )?;
        Ok(())
    }

    pub fn get_blocks_for_file(&self, file_id: &str) -> Result<Vec<FileBlock>> {
        let mut stmt = self.conn.prepare(
            "SELECT fb.block_id, fb.block_index, b.size, b.is_present
             FROM file_blocks fb
             JOIN blocks b ON fb.block_id = b.id
             WHERE fb.file_id = ?1 ORDER BY fb.block_index ASC"
        )?;
        let blocks = stmt.query_map(rusqlite::params![file_id], |row| {
            Ok(FileBlock {
                block_id: row.get(0)?,
                block_index: row.get(1)?,
                size: row.get(2)?,
                is_present: row.get(3)?,
            })
        })?;

        let mut result = Vec::new();
        for block in blocks {
            result.push(block?);
        }
        Ok(result)
    }

    pub fn set_block_present(&self, block_id: &str, is_present: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE blocks SET is_present = ?1 WHERE id = ?2",
            rusqlite::params![is_present as i64, block_id],
        )?;
        Ok(())
    }

    pub fn delete_file(&self, file_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM file_blocks WHERE file_id = ?1",
            rusqlite::params![file_id],
        )?;
        self.conn.execute(
            "DELETE FROM files WHERE id = ?1",
            rusqlite::params![file_id],
        )?;
        Ok(())
    }

    pub fn insert_tombstone(&self, tombstone: &Tombstone) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO tombstones (file_id, deleted_at, deleted_by, version) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![tombstone.file_id, tombstone.deleted_at, tombstone.deleted_by, tombstone.version],
        )?;
        Ok(())
    }

    pub fn get_all_tombstones(&self) -> Result<Vec<Tombstone>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_id, deleted_at, deleted_by, version FROM tombstones"
        )?;
        let tombstones = stmt.query_map([], |row| {
            Ok(Tombstone {
                file_id: row.get(0)?,
                deleted_at: row.get(1)?,
                deleted_by: row.get(2)?,
                version: row.get(3)?,
            })
        })?;

        let mut result = Vec::new();
        for tombstone in tombstones {
            result.push(tombstone?);
        }
        Ok(result)
    }

    pub fn has_tombstone(&self, file_id: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM tombstones WHERE file_id = ?1",
            rusqlite::params![file_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn delete_file_with_tombstone(
        &self,
        file_id: &str,
        deleted_by: &str,
        version: i64,
    ) -> Result<()> {
        let tombstone = Tombstone {
            file_id: file_id.to_string(),
            deleted_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs() as i64,
            deleted_by: deleted_by.to_string(),
            version,
        };
        self.insert_tombstone(&tombstone)?;
        self.delete_file(file_id)?;
        Ok(())
    }

    pub fn get_file_by_path(&self, path: &str) -> Result<Option<FileMetadata>> {
        let result = self.conn.query_row(
            "SELECT id, path, size, modified_time, version, created_by, pinned_devices FROM files WHERE path = ?1",
            rusqlite::params![path],
            file_from_row,
        ).optional()?;
        Ok(result)
    }
}