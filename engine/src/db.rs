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

/// One device's copy of one file.
///
/// `content_hash` records *which* content that device has, not merely that it
/// has something. Copies are snapshots and never update themselves, so a
/// holder whose hash differs from the file's current hash is simply behind.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileHolder {
    pub file_id: String,
    pub device_id: String,
    pub content_hash: String,
    pub received_at: i64,
}

/// A device this one has paired with. Its certificate is pinned, so it is the
/// only kind of device allowed to touch the catalog or block storage.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub cert_pem: String,
    pub paired_at: i64,
    pub last_seen: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Tombstone {
    pub file_id: String,
    pub deleted_at: i64,
    pub deleted_by: String,
    pub version: i64,
}

fn holder_from_row(row: &rusqlite::Row) -> rusqlite::Result<FileHolder> {
    Ok(FileHolder {
        file_id: row.get(0)?,
        device_id: row.get(1)?,
        content_hash: row.get(2)?,
        received_at: row.get(3)?,
    })
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
            CREATE TABLE IF NOT EXISTS file_holders (
                file_id TEXT NOT NULL,
                device_id TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                received_at INTEGER NOT NULL,
                PRIMARY KEY (file_id, device_id)
            );
            CREATE INDEX IF NOT EXISTS idx_file_holders_file ON file_holders(file_id);
            CREATE TABLE IF NOT EXISTS tombstones (
                file_id TEXT PRIMARY KEY, deleted_at INTEGER NOT NULL,
                deleted_by TEXT NOT NULL, version INTEGER NOT NULL
            );
            "
        )?;

        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    /// Additive schema changes for databases created by earlier versions.
    /// `CREATE TABLE IF NOT EXISTS` silently skips existing tables, so new
    /// columns have to be applied separately.
    fn migrate(&self) -> Result<()> {
        self.add_column_if_missing("devices", "platform", "TEXT NOT NULL DEFAULT ''")?;
        self.add_column_if_missing("files", "content_hash", "TEXT NOT NULL DEFAULT ''")?;
        Ok(())
    }

    fn add_column_if_missing(&self, table: &str, column: &str, definition: &str) -> Result<()> {
        let mut stmt = self.conn.prepare(&format!("PRAGMA table_info({})", table))?;
        let existing: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        if !existing.iter().any(|c| c == column) {
            self.conn.execute_batch(&format!(
                "ALTER TABLE {} ADD COLUMN {} {}",
                table, column, definition
            ))?;
        }
        Ok(())
    }

    /// Records a device as paired, or refreshes the details of one already
    /// paired. The certificate is what actually grants access; the name and
    /// platform are display only.
    pub fn upsert_paired_device(&self, device: &PairedDevice) -> Result<()> {
        self.conn.execute(
            "INSERT INTO devices (id, name, platform, cert_pem, paired_at, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                platform = excluded.platform,
                cert_pem = excluded.cert_pem,
                last_seen = excluded.last_seen",
            rusqlite::params![
                device.id,
                device.name,
                device.platform,
                device.cert_pem,
                device.paired_at,
                device.last_seen
            ],
        )?;
        Ok(())
    }

    pub fn get_paired_devices(&self) -> Result<Vec<PairedDevice>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, platform, cert_pem, paired_at, last_seen FROM devices ORDER BY name",
        )?;
        let devices = stmt.query_map([], |row| {
            Ok(PairedDevice {
                id: row.get(0)?,
                name: row.get(1)?,
                platform: row.get(2)?,
                cert_pem: row.get(3)?,
                paired_at: row.get(4)?,
                last_seen: row.get(5)?,
            })
        })?;

        let mut result = Vec::new();
        for device in devices {
            result.push(device?);
        }
        Ok(result)
    }

    pub fn is_paired(&self, device_id: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM devices WHERE id = ?1",
            rusqlite::params![device_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn remove_paired_device(&self, device_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM devices WHERE id = ?1",
            rusqlite::params![device_id],
        )?;
        Ok(())
    }

    pub fn touch_device(&self, device_id: &str, seen_at: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE devices SET last_seen = ?1 WHERE id = ?2",
            rusqlite::params![seen_at, device_id],
        )?;
        Ok(())
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

    // ---- Holder set ----
    //
    // Which devices actually have a file's bytes, and which content each one
    // has. Copies never update themselves, so a holder can legitimately be
    // behind; recording the hash per copy is what keeps the catalog honest
    // about that instead of implying every holder is current.
    //
    // A device only ever writes its own row. Removing another device's copy is
    // a request that device carries out and then publishes, so there is never
    // more than one writer per row and nothing needs merging.

    pub fn set_holder(&self, holder: &FileHolder) -> Result<()> {
        self.conn.execute(
            "INSERT INTO file_holders (file_id, device_id, content_hash, received_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(file_id, device_id) DO UPDATE SET
                content_hash = excluded.content_hash,
                received_at = excluded.received_at",
            rusqlite::params![
                holder.file_id,
                holder.device_id,
                holder.content_hash,
                holder.received_at
            ],
        )?;
        Ok(())
    }

    pub fn remove_holder(&self, file_id: &str, device_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM file_holders WHERE file_id = ?1 AND device_id = ?2",
            rusqlite::params![file_id, device_id],
        )?;
        Ok(())
    }

    pub fn get_holders(&self, file_id: &str) -> Result<Vec<FileHolder>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_id, device_id, content_hash, received_at
             FROM file_holders WHERE file_id = ?1 ORDER BY received_at",
        )?;
        let holders = stmt.query_map(rusqlite::params![file_id], holder_from_row)?;

        let mut result = Vec::new();
        for holder in holders {
            result.push(holder?);
        }
        Ok(result)
    }

    /// Every holder row known, for replicating the catalog to a peer.
    pub fn get_all_holders(&self) -> Result<Vec<FileHolder>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_id, device_id, content_hash, received_at FROM file_holders",
        )?;
        let holders = stmt.query_map([], holder_from_row)?;

        let mut result = Vec::new();
        for holder in holders {
            result.push(holder?);
        }
        Ok(result)
    }

    pub fn is_holder(&self, file_id: &str, device_id: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM file_holders WHERE file_id = ?1 AND device_id = ?2",
            rusqlite::params![file_id, device_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// How many devices still hold this file. Reaching zero is what sends an
    /// item to trash rather than freeing it outright.
    pub fn holder_count(&self, file_id: &str) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM file_holders WHERE file_id = ?1",
            rusqlite::params![file_id],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Drops every holder row for a file, used when an item leaves the catalog.
    pub fn clear_holders(&self, file_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM file_holders WHERE file_id = ?1",
            rusqlite::params![file_id],
        )?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Database {
        Database::init(":memory:").expect("in-memory database")
    }

    fn holder(file_id: &str, device_id: &str, hash: &str) -> FileHolder {
        FileHolder {
            file_id: file_id.to_string(),
            device_id: device_id.to_string(),
            content_hash: hash.to_string(),
            received_at: 1_700_000_000,
        }
    }

    #[test]
    fn sharing_adds_a_holder_without_removing_the_sender() {
        let db = db();
        db.set_holder(&holder("f1", "android", "hash-a")).unwrap();
        db.set_holder(&holder("f1", "macos", "hash-a")).unwrap();

        assert_eq!(db.holder_count("f1").unwrap(), 2);
        assert!(db.is_holder("f1", "android").unwrap());
        assert!(db.is_holder("f1", "macos").unwrap());
    }

    #[test]
    fn deleting_one_copy_leaves_the_others() {
        let db = db();
        db.set_holder(&holder("f1", "android", "hash-a")).unwrap();
        db.set_holder(&holder("f1", "macos", "hash-a")).unwrap();

        // The iPhone deletes the Mac's copy; Android is untouched.
        db.remove_holder("f1", "macos").unwrap();

        assert_eq!(db.holder_count("f1").unwrap(), 1);
        assert!(db.is_holder("f1", "android").unwrap());
        assert!(!db.is_holder("f1", "macos").unwrap());
    }

    #[test]
    fn removing_the_last_copy_empties_the_holder_set() {
        let db = db();
        db.set_holder(&holder("f1", "android", "hash-a")).unwrap();
        db.remove_holder("f1", "android").unwrap();

        // Zero holders is the signal to send an item to trash rather than
        // freeing it outright.
        assert_eq!(db.holder_count("f1").unwrap(), 0);
        assert!(db.get_holders("f1").unwrap().is_empty());
    }

    #[test]
    fn a_copy_that_was_never_resent_reads_as_behind() {
        let db = db();
        db.set_holder(&holder("f1", "android", "hash-v2")).unwrap();
        db.set_holder(&holder("f1", "macos", "hash-v1")).unwrap();

        let current = "hash-v2";
        let holders = db.get_holders("f1").unwrap();

        let stale: Vec<&str> = holders
            .iter()
            .filter(|h| h.content_hash != current)
            .map(|h| h.device_id.as_str())
            .collect();

        assert_eq!(stale, vec!["macos"]);
    }

    #[test]
    fn resending_updates_that_copy_in_place() {
        let db = db();
        db.set_holder(&holder("f1", "macos", "hash-v1")).unwrap();
        db.set_holder(&FileHolder {
            received_at: 1_700_000_500,
            ..holder("f1", "macos", "hash-v2")
        })
        .unwrap();

        // Overriding replaces the copy rather than adding a second one.
        assert_eq!(db.holder_count("f1").unwrap(), 1);
        let holders = db.get_holders("f1").unwrap();
        assert_eq!(holders[0].content_hash, "hash-v2");
        assert_eq!(holders[0].received_at, 1_700_000_500);
    }

    #[test]
    fn holder_sets_are_per_file() {
        let db = db();
        db.set_holder(&holder("f1", "android", "hash-a")).unwrap();
        db.set_holder(&holder("f2", "android", "hash-b")).unwrap();
        db.set_holder(&holder("f2", "macos", "hash-b")).unwrap();

        assert_eq!(db.holder_count("f1").unwrap(), 1);
        assert_eq!(db.holder_count("f2").unwrap(), 2);
        assert_eq!(db.get_all_holders().unwrap().len(), 3);

        db.clear_holders("f2").unwrap();
        assert_eq!(db.holder_count("f1").unwrap(), 1);
        assert_eq!(db.holder_count("f2").unwrap(), 0);
    }

    #[test]
    fn pairing_records_survive_a_round_trip() {
        let db = db();
        let device = PairedDevice {
            id: "abc".into(),
            name: "MacBook".into(),
            platform: "macOS".into(),
            cert_pem: "cert".into(),
            paired_at: 1,
            last_seen: 1,
        };
        db.upsert_paired_device(&device).unwrap();

        assert!(db.is_paired("abc").unwrap());
        assert_eq!(db.get_paired_devices().unwrap()[0].name, "MacBook");

        db.touch_device("abc", 99).unwrap();
        assert_eq!(db.get_paired_devices().unwrap()[0].last_seen, 99);

        db.remove_paired_device("abc").unwrap();
        assert!(!db.is_paired("abc").unwrap());
    }
}