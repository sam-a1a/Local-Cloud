// engine/src/db.rs
use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use rusqlite::OptionalExtension;

pub struct Database {
    pub conn: Connection,
}

/// An item in the shared catalog.
///
/// Carries no version number and no list of devices. Copies never merge, so
/// there is nothing for a version to order; `content_hash` says what this item
/// currently is, and `file_holders` says who has which content.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileMetadata {
    pub id: String,
    pub path: String,
    pub size: i64,
    pub content_hash: String,
    pub modified_time: i64,
    pub created_by: String,
    /// Unix seconds, or 0 while the item is live.
    #[serde(default)]
    pub trashed_at: i64,
    #[serde(default)]
    pub trashed_by: String,
}

impl FileMetadata {
    pub fn is_trashed(&self) -> bool {
        self.trashed_at != 0
    }
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

/// A standing instruction for one device to drop its copy of an item.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct DeleteRequest {
    pub file_id: String,
    pub target_device: String,
    pub requested_by: String,
    pub requested_at: i64,
}

/// What happened when a peer's item was merged into the local catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    Applied,
    /// The item has a tombstone here, so it is not reintroduced.
    AlreadyDestroyed,
    /// A name conflict was settled by tie-break, moving one item aside.
    Renamed {
        file_id: String,
        from: String,
        to: String,
        /// The item that kept the contested name.
        conflicting_file_id: String,
    },
}

/// The whole shared namespace as one device knows it, as it travels between
/// devices.
///
/// This is the protocol's shape, not the API's: it carries tombstones and
/// outstanding delete requests because a peer needs them to converge. An
/// application does not, and `localcloud::Catalog` is what it sees instead.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CatalogPayload {
    pub files: Vec<FileMetadata>,
    pub holders: Vec<FileHolder>,
    #[serde(default)]
    pub delete_requests: Vec<DeleteRequest>,
    #[serde(default)]
    pub tombstones: Vec<Tombstone>,
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

/// A record that an item was destroyed for good.
///
/// Kept so a device that was away cannot reintroduce it from a stale catalog.
/// Carries no version: there is nothing to order, only the fact of deletion.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Tombstone {
    pub file_id: String,
    pub deleted_at: i64,
    pub deleted_by: String,
}

fn holder_from_row(row: &rusqlite::Row) -> rusqlite::Result<FileHolder> {
    Ok(FileHolder {
        file_id: row.get(0)?,
        device_id: row.get(1)?,
        content_hash: row.get(2)?,
        received_at: row.get(3)?,
    })
}

const FILE_COLUMNS: &str =
    "id, path, size, content_hash, modified_time, created_by, trashed_at, trashed_by";

fn file_from_row(row: &rusqlite::Row) -> rusqlite::Result<FileMetadata> {
    Ok(FileMetadata {
        id: row.get(0)?,
        path: row.get(1)?,
        size: row.get(2)?,
        content_hash: row.get(3)?,
        modified_time: row.get(4)?,
        created_by: row.get(5)?,
        trashed_at: row.get(6)?,
        trashed_by: row.get(7)?,
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
                id TEXT PRIMARY KEY, path TEXT NOT NULL, size INTEGER NOT NULL,
                content_hash TEXT NOT NULL DEFAULT '',
                modified_time INTEGER NOT NULL, created_by TEXT NOT NULL,
                trashed_at INTEGER NOT NULL DEFAULT 0,
                trashed_by TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS blocks (
                id TEXT PRIMARY KEY, size INTEGER NOT NULL, is_present INTEGER NOT NULL DEFAULT 0
            );
            -- Keyed by position, not by content. A manifest is an ordered list
            -- and the same block can legitimately appear at several places in
            -- it: any file with a run of zeros or repeated padding contains one
            -- block many times over. Keying on (file_id, block_id) made those
            -- repeats collapse into a single row, so the file reassembled short
            -- and corrupt on every device it was sent to.
            CREATE TABLE IF NOT EXISTS file_blocks (
                file_id TEXT NOT NULL, block_id TEXT NOT NULL, block_index INTEGER NOT NULL,
                PRIMARY KEY (file_id, block_index),
                FOREIGN KEY (file_id) REFERENCES files(id),
                FOREIGN KEY (block_id) REFERENCES blocks(id)
            );
            CREATE INDEX IF NOT EXISTS idx_file_blocks_block ON file_blocks(block_id);
            CREATE TABLE IF NOT EXISTS file_holders (
                file_id TEXT NOT NULL,
                device_id TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                received_at INTEGER NOT NULL,
                PRIMARY KEY (file_id, device_id)
            );
            CREATE INDEX IF NOT EXISTS idx_file_holders_file ON file_holders(file_id);
            -- A device can delete a copy it does not itself hold, but only the
            -- holder can erase its own disk. Requests are recorded and travel
            -- through the catalog, so an offline target carries them out when
            -- it comes back.
            CREATE TABLE IF NOT EXISTS delete_requests (
                file_id TEXT NOT NULL,
                target_device TEXT NOT NULL,
                requested_by TEXT NOT NULL,
                requested_at INTEGER NOT NULL,
                PRIMARY KEY (file_id, target_device)
            );
            CREATE TABLE IF NOT EXISTS tombstones (
                file_id TEXT PRIMARY KEY, deleted_at INTEGER NOT NULL,
                deleted_by TEXT NOT NULL
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

        // Older files tables need a full rebuild rather than added columns.
        // They declare path as inline UNIQUE, which would keep a trashed item
        // holding on to its name forever, and an inline constraint cannot be
        // dropped in place. The rebuild also sheds the merge-and-pin columns:
        // version ordered concurrent edits, which cannot happen now that copies
        // never propagate on their own, and pinned_devices recorded where data
        // *should* go, which file_holders replaces with where it actually is.
        if !self.has_column("files", "trashed_at")? {
            self.rebuild_files_table()?;
        }

        // Older databases key file_blocks on the block rather than its position,
        // so every repeat of a block within one file was silently dropped and
        // the file reassembled short wherever it was sent. The mappings that
        // were lost cannot be recovered here, but re-indexing rebuilds them,
        // and the move to larger blocks re-chunks every file on the next scan
        // regardless.
        if self.file_blocks_keyed_by_content()? {
            self.rebuild_file_blocks_table()?;
        }

        // Tombstones record that an item was destroyed, which has nothing to
        // order and so needs no version.
        self.drop_column_if_present("tombstones", "version")?;

        // Created here rather than alongside the table, because on an older
        // database the column it filters on does not exist until the rebuild
        // above has run.
        //
        // Names are unique among live items only: a trashed item keeps its path
        // for restore but stops owning the name, which is what lets Override
        // move the old content aside and reuse it.
        self.conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_files_live_path
                ON files(path) WHERE trashed_at = 0",
        )?;
        Ok(())
    }

    fn rebuild_files_table(&self) -> Result<()> {
        // Intermediate databases already have content_hash; the earliest ones
        // do not, and start with an empty hash until their files are reindexed.
        let content_hash = if self.has_column("files", "content_hash")? {
            "content_hash"
        } else {
            "''"
        };

        // file_blocks references files(id), so the old table cannot be dropped
        // with enforcement on. This has to sit outside the transaction: the
        // pragma is a no-op inside one.
        self.conn.execute_batch("PRAGMA foreign_keys=OFF")?;

        let result = (|| -> Result<()> {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(&format!(
                "CREATE TABLE files_rebuilt (
                    id TEXT PRIMARY KEY, path TEXT NOT NULL, size INTEGER NOT NULL,
                    content_hash TEXT NOT NULL DEFAULT '',
                    modified_time INTEGER NOT NULL, created_by TEXT NOT NULL,
                    trashed_at INTEGER NOT NULL DEFAULT 0,
                    trashed_by TEXT NOT NULL DEFAULT ''
                 );
                 INSERT INTO files_rebuilt
                    (id, path, size, content_hash, modified_time, created_by)
                    SELECT id, path, size, {}, modified_time, created_by FROM files;
                 DROP TABLE files;
                 ALTER TABLE files_rebuilt RENAME TO files;",
                content_hash
            ))?;
            tx.commit()?;
            Ok(())
        })();

        // Restore enforcement even if the rebuild failed, so a partial
        // migration cannot leave the connection silently unprotected.
        self.conn.execute_batch("PRAGMA foreign_keys=ON")?;
        result
    }

    /// Whether file_blocks still treats a file's manifest as a *set* of blocks
    /// rather than an ordered list of positions.
    fn file_blocks_keyed_by_content(&self) -> Result<bool> {
        let declaration: Option<String> = self
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'file_blocks'",
                [],
                |row| row.get(0),
            )
            .optional()?;

        Ok(declaration
            .map(|sql| sql.split_whitespace().collect::<String>())
            .is_some_and(|sql| sql.contains("PRIMARYKEY(file_id,block_id)")))
    }

    fn rebuild_file_blocks_table(&self) -> Result<()> {
        // As with the files rebuild: dropping a table other rows reference
        // needs enforcement off, and the pragma is a no-op inside a
        // transaction.
        self.conn.execute_batch("PRAGMA foreign_keys=OFF")?;

        let result = (|| -> Result<()> {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(
                "CREATE TABLE file_blocks_rebuilt (
                    file_id TEXT NOT NULL, block_id TEXT NOT NULL, block_index INTEGER NOT NULL,
                    PRIMARY KEY (file_id, block_index),
                    FOREIGN KEY (file_id) REFERENCES files(id),
                    FOREIGN KEY (block_id) REFERENCES blocks(id)
                 );
                 INSERT OR IGNORE INTO file_blocks_rebuilt (file_id, block_id, block_index)
                    SELECT file_id, block_id, block_index FROM file_blocks;
                 DROP TABLE file_blocks;
                 ALTER TABLE file_blocks_rebuilt RENAME TO file_blocks;
                 CREATE INDEX IF NOT EXISTS idx_file_blocks_block ON file_blocks(block_id);",
            )?;
            tx.commit()?;
            Ok(())
        })();

        self.conn.execute_batch("PRAGMA foreign_keys=ON")?;
        result
    }

    fn column_names(&self, table: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(&format!("PRAGMA table_info({})", table))?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(names)
    }

    fn has_column(&self, table: &str, column: &str) -> Result<bool> {
        Ok(self.column_names(table)?.iter().any(|c| c == column))
    }

    fn add_column_if_missing(&self, table: &str, column: &str, definition: &str) -> Result<()> {
        if !self.has_column(table, column)? {
            self.conn.execute_batch(&format!(
                "ALTER TABLE {} ADD COLUMN {} {}",
                table, column, definition
            ))?;
        }
        Ok(())
    }

    fn drop_column_if_present(&self, table: &str, column: &str) -> Result<()> {
        if self.has_column(table, column)? {
            self.conn
                .execute_batch(&format!("ALTER TABLE {} DROP COLUMN {}", table, column))?;
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
        self.conn.execute(
            "INSERT OR REPLACE INTO files
                (id, path, size, content_hash, modified_time, created_by, trashed_at, trashed_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                file.id,
                file.path,
                file.size,
                file.content_hash,
                file.modified_time,
                file.created_by,
                file.trashed_at,
                file.trashed_by
            ],
        )?;
        Ok(())
    }

    /// Live items only. This is the catalog as a person sees it.
    pub fn get_all_files(&self) -> Result<Vec<FileMetadata>> {
        self.query_files("SELECT {} FROM files WHERE trashed_at = 0")
    }

    /// Live and trashed together, for replicating to a peer. Trash state has to
    /// travel or a device that missed the deletion would keep offering the item.
    pub fn get_catalog_files(&self) -> Result<Vec<FileMetadata>> {
        self.query_files("SELECT {} FROM files")
    }

    pub fn get_trashed_files(&self) -> Result<Vec<FileMetadata>> {
        self.query_files("SELECT {} FROM files WHERE trashed_at != 0 ORDER BY trashed_at DESC")
    }

    fn query_files(&self, sql_template: &str) -> Result<Vec<FileMetadata>> {
        let mut stmt = self
            .conn
            .prepare(&sql_template.replace("{}", FILE_COLUMNS))?;
        let files = stmt.query_map([], file_from_row)?;

        let mut result = Vec::new();
        for file in files {
            result.push(file?);
        }
        Ok(result)
    }

    /// Moves an item aside without destroying it: its blocks and holder rows
    /// stay put, and its name becomes available again.
    pub fn trash_file(&self, file_id: &str, trashed_by: &str, trashed_at: i64) -> Result<()> {
        // 0 is what marks an item live, so accepting it here would silently
        // leave the item exactly as it was.
        if trashed_at <= 0 {
            anyhow::bail!("A trash timestamp must be positive; 0 means live");
        }

        let changed = self.conn.execute(
            "UPDATE files SET trashed_at = ?1, trashed_by = ?2 WHERE id = ?3 AND trashed_at = 0",
            rusqlite::params![trashed_at, trashed_by, file_id],
        )?;
        if changed == 0 {
            anyhow::bail!("No live item with that id");
        }
        Ok(())
    }

    /// The first numbered variant of `path` that no live item claims.
    /// A failed lookup counts as taken, so an error can never cause an
    /// overwrite.
    fn free_path(&self, path: &str) -> String {
        crate::collision::next_available_path(path, |candidate| {
            self.is_path_taken(candidate).unwrap_or(true)
        })
    }

    /// Merges one item from a peer's catalog, resolving name conflicts.
    ///
    /// Two devices that were apart can each create an item with the same name,
    /// and a sync cannot stop to ask which should win. The tie-break is the
    /// item id: the smaller one keeps the name. Both devices hold both ids, so
    /// each reaches the same answer alone and they converge without a round
    /// trip. The loser is placed under a numbered name, and stays there on
    /// later syncs rather than being bumped again.
    pub fn merge_catalog_file(&self, incoming: &FileMetadata) -> Result<MergeOutcome> {
        // A device that was away when an item was destroyed still has it in its
        // catalog, and would otherwise hand it straight back.
        if self.has_tombstone(&incoming.id)? {
            return Ok(MergeOutcome::AlreadyDestroyed);
        }

        // A trashed item does not own its name, so it never contends.
        if incoming.is_trashed() {
            self.upsert_file_from_catalog(incoming)?;
            return Ok(MergeOutcome::Applied);
        }

        let incumbent = match self.get_file_by_path(&incoming.path)? {
            Some(existing) if existing.id != incoming.id => existing,
            _ => {
                self.upsert_file_from_catalog(incoming)?;
                return Ok(MergeOutcome::Applied);
            }
        };

        if incumbent.id < incoming.id {
            // The incoming item yields. If a previous sync already placed it,
            // leave it there so the number does not creep upward.
            let target = match self.get_file_by_id(&incoming.id)? {
                Some(local) if !local.is_trashed() => local.path,
                _ => self.free_path(&incoming.path),
            };

            let mut placed = incoming.clone();
            placed.path = target.clone();
            self.upsert_file_from_catalog(&placed)?;

            Ok(MergeOutcome::Renamed {
                file_id: incoming.id.clone(),
                from: incoming.path.clone(),
                to: target,
                conflicting_file_id: incumbent.id,
            })
        } else {
            // The incoming item wins, so ours moves aside to free the name.
            let target = self.free_path(&incumbent.path);

            let tx = self.conn.unchecked_transaction()?;
            tx.execute(
                "UPDATE files SET path = ?1 WHERE id = ?2",
                rusqlite::params![target, incumbent.id],
            )?;
            tx.commit()?;

            self.upsert_file_from_catalog(incoming)?;

            Ok(MergeOutcome::Renamed {
                file_id: incumbent.id.clone(),
                from: incumbent.path,
                to: target,
                conflicting_file_id: incoming.id.clone(),
            })
        }
    }

    /// Hands a name from one item to another, atomically.
    ///
    /// Either the previous owner goes to trash and the incoming item takes the
    /// name, or nothing changes. Doing it in two steps could leave an item
    /// trashed for a name it never received.
    pub fn override_file(
        &self,
        existing_file_id: &str,
        incoming_file_id: &str,
        path: &str,
        trashed_by: &str,
        trashed_at: i64,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;

        let moved = tx.execute(
            "UPDATE files SET trashed_at = ?1, trashed_by = ?2 WHERE id = ?3 AND trashed_at = 0",
            rusqlite::params![trashed_at, trashed_by, existing_file_id],
        )?;
        if moved == 0 {
            anyhow::bail!("The item being overridden is no longer live");
        }

        // Rejected by the live-path index if anything else claimed the name in
        // the meantime, which rolls the trash back with it.
        tx.execute(
            "UPDATE files SET path = ?1 WHERE id = ?2",
            rusqlite::params![path, incoming_file_id],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Brings a trashed item back. Fails if something else took its name in the
    /// meantime, rather than silently restoring it under a different one.
    pub fn restore_file(&self, file_id: &str) -> Result<()> {
        let path: String = self.conn.query_row(
            "SELECT path FROM files WHERE id = ?1 AND trashed_at != 0",
            rusqlite::params![file_id],
            |row| row.get(0),
        )?;

        if self.is_path_taken(&path)? {
            anyhow::bail!("\"{}\" is in use by another item", path);
        }

        self.conn.execute(
            "UPDATE files SET trashed_at = 0, trashed_by = '' WHERE id = ?1",
            rusqlite::params![file_id],
        )?;
        Ok(())
    }

    /// Whether a live item already owns this name.
    pub fn is_path_taken(&self, path: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM files WHERE path = ?1 AND trashed_at = 0",
            rusqlite::params![path],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn get_file_by_id(&self, file_id: &str) -> Result<Option<FileMetadata>> {
        let result = self
            .conn
            .query_row(
                &format!("SELECT {} FROM files WHERE id = ?1", FILE_COLUMNS),
                rusqlite::params![file_id],
                file_from_row,
            )
            .optional()?;
        Ok(result)
    }

    /// Accepts an item as described by a peer's catalog.
    ///
    /// There is no version comparison, because there is nothing to order:
    /// copies never propagate on their own, so two devices cannot concurrently
    /// advance the same item. Items are keyed by id; a second item claiming an
    /// already-taken path is a name collision, which the sender resolves with
    /// the user at send time rather than being silently merged here.
    pub fn upsert_file_from_catalog(&self, file: &FileMetadata) -> Result<()> {
        self.conn.execute(
            "INSERT INTO files
                (id, path, size, content_hash, modified_time, created_by, trashed_at, trashed_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                path = excluded.path,
                size = excluded.size,
                content_hash = excluded.content_hash,
                modified_time = excluded.modified_time,
                created_by = excluded.created_by,
                trashed_at = excluded.trashed_at,
                trashed_by = excluded.trashed_by",
            rusqlite::params![
                file.id,
                file.path,
                file.size,
                file.content_hash,
                file.modified_time,
                file.created_by,
                file.trashed_at,
                file.trashed_by
            ],
        )?;
        Ok(())
    }

    /// Renames an item. Fails rather than clobbering if the name is taken.
    pub fn set_file_path(&self, file_id: &str, path: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE files SET path = ?1 WHERE id = ?2",
            rusqlite::params![path, file_id],
        )?;
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

    /// Blocks that only this file uses.
    ///
    /// Blocks are addressed by content hash and therefore shared: two identical
    /// files, or two revisions with an unchanged region, map to the same block.
    /// Deleting a file's blocks outright would take those away from whatever
    /// else still needs them, so only the exclusive ones may be removed.
    pub fn blocks_exclusive_to_file(&self, file_id: &str) -> Result<Vec<String>> {
        // DISTINCT because a block may fill several positions in the same file,
        // and callers release the bytes once per name they are given.
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT fb.block_id FROM file_blocks fb
             WHERE fb.file_id = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM file_blocks other
                   WHERE other.block_id = fb.block_id AND other.file_id != ?1
               )",
        )?;
        let blocks = stmt.query_map(rusqlite::params![file_id], |row| row.get::<_, String>(0))?;

        let mut result = Vec::new();
        for block in blocks {
            result.push(block?);
        }
        Ok(result)
    }

    pub fn clear_blocks_for_file(&self, file_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM file_blocks WHERE file_id = ?1",
            rusqlite::params![file_id],
        )?;
        Ok(())
    }

    /// Forgets a block that no file maps to any more, reporting whether it went.
    ///
    /// Blocks outlive the revision that introduced them: another item, or a
    /// later version of the same one, may map to the very same contents. Only
    /// once the last mapping is gone does the block belong to nobody, and the
    /// caller can release its bytes.
    pub fn forget_block_if_unreferenced(&self, block_id: &str) -> Result<bool> {
        let referenced: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM file_blocks WHERE block_id = ?1)",
            rusqlite::params![block_id],
            |row| row.get(0),
        )?;

        if referenced {
            return Ok(false);
        }

        self.conn.execute(
            "DELETE FROM blocks WHERE id = ?1",
            rusqlite::params![block_id],
        )?;
        Ok(true)
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

    /// Whether this device holds the block's contents, as opposed to merely
    /// knowing an item refers to it.
    pub fn block_is_present(&self, block_id: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT is_present FROM blocks WHERE id = ?1",
                rusqlite::params![block_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some_and(|present| present != 0))
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
            "INSERT OR REPLACE INTO tombstones (file_id, deleted_at, deleted_by)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![
                tombstone.file_id,
                tombstone.deleted_at,
                tombstone.deleted_by
            ],
        )?;
        Ok(())
    }

    pub fn get_all_tombstones(&self) -> Result<Vec<Tombstone>> {
        let mut stmt = self
            .conn
            .prepare("SELECT file_id, deleted_at, deleted_by FROM tombstones")?;
        let tombstones = stmt.query_map([], |row| {
            Ok(Tombstone {
                file_id: row.get(0)?,
                deleted_at: row.get(1)?,
                deleted_by: row.get(2)?,
            })
        })?;

        let mut result = Vec::new();
        for tombstone in tombstones {
            result.push(tombstone?);
        }
        Ok(result)
    }

    /// Destroys an item: its blocks mapping, holder rows, outstanding delete
    /// requests and catalog entry all go, and a tombstone takes their place.
    ///
    /// Stored blocks are not touched here - whether one can be removed depends
    /// on nothing else referencing it, which the caller establishes first.
    pub fn purge_file(&self, file_id: &str, deleted_by: &str, deleted_at: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for statement in [
            "DELETE FROM file_holders WHERE file_id = ?1",
            "DELETE FROM file_blocks WHERE file_id = ?1",
            "DELETE FROM delete_requests WHERE file_id = ?1",
            "DELETE FROM files WHERE id = ?1",
        ] {
            tx.execute(statement, rusqlite::params![file_id])?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO tombstones (file_id, deleted_at, deleted_by)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![file_id, deleted_at, deleted_by],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Trashed items whose retention has run out.
    pub fn expired_trash(&self, now: i64, retention_secs: i64) -> Result<Vec<FileMetadata>> {
        Ok(self
            .get_trashed_files()?
            .into_iter()
            .filter(|f| now.saturating_sub(f.trashed_at) >= retention_secs)
            .collect())
    }

    pub fn has_tombstone(&self, file_id: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM tombstones WHERE file_id = ?1",
            rusqlite::params![file_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
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

    /// Replaces everything known about one device's copies.
    ///
    /// Used when merging a peer's catalog: that peer is the only authority on
    /// what it holds, so its rows are taken wholesale rather than merged. This
    /// is what lets a deletion propagate - a copy the peer no longer reports is
    /// a copy it no longer has.
    pub fn replace_holders_for_device(
        &self,
        device_id: &str,
        holders: &[FileHolder],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM file_holders WHERE device_id = ?1",
            rusqlite::params![device_id],
        )?;
        for holder in holders {
            tx.execute(
                "INSERT OR REPLACE INTO file_holders (file_id, device_id, content_hash, received_at)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    holder.file_id,
                    holder.device_id,
                    holder.content_hash,
                    holder.received_at
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---- Delete requests ----

    pub fn record_delete_request(&self, request: &DeleteRequest) -> Result<()> {
        self.conn.execute(
            "INSERT INTO delete_requests (file_id, target_device, requested_by, requested_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(file_id, target_device) DO NOTHING",
            rusqlite::params![
                request.file_id,
                request.target_device,
                request.requested_by,
                request.requested_at
            ],
        )?;
        Ok(())
    }

    pub fn get_delete_requests(&self) -> Result<Vec<DeleteRequest>> {
        self.query_delete_requests(
            "SELECT file_id, target_device, requested_by, requested_at FROM delete_requests",
            [],
        )
    }

    pub fn get_delete_requests_for(&self, device_id: &str) -> Result<Vec<DeleteRequest>> {
        self.query_delete_requests(
            "SELECT file_id, target_device, requested_by, requested_at
             FROM delete_requests WHERE target_device = ?1",
            rusqlite::params![device_id],
        )
    }

    fn query_delete_requests(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<DeleteRequest>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params, |row| {
            Ok(DeleteRequest {
                file_id: row.get(0)?,
                target_device: row.get(1)?,
                requested_by: row.get(2)?,
                requested_at: row.get(3)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn clear_delete_request(&self, file_id: &str, target_device: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM delete_requests WHERE file_id = ?1 AND target_device = ?2",
            rusqlite::params![file_id, target_device],
        )?;
        Ok(())
    }

    /// Forgets requests that have been carried out.
    ///
    /// A request is satisfied once the target is no longer a live holder -
    /// either it dropped the copy, or the item went to trash because that was
    /// the last one. Deriving this from the holder set rather than tracking
    /// acknowledgements means no extra round trip and no way for a request to
    /// be applied twice.
    pub fn prune_satisfied_delete_requests(&self) -> Result<usize> {
        let removed = self.conn.execute(
            "DELETE FROM delete_requests
             WHERE NOT EXISTS (
                 SELECT 1 FROM file_holders h
                 JOIN files f ON f.id = h.file_id
                 WHERE h.file_id = delete_requests.file_id
                   AND h.device_id = delete_requests.target_device
                   AND f.trashed_at = 0
             )",
            [],
        )?;
        Ok(removed)
    }

    /// Drops every holder row for a file, used when an item leaves the catalog.
    pub fn clear_holders(&self, file_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM file_holders WHERE file_id = ?1",
            rusqlite::params![file_id],
        )?;
        Ok(())
    }

    /// The live item at this name, if any. Trashed items are excluded: they
    /// keep their path for restore but no longer own the name.
    pub fn get_file_by_path(&self, path: &str) -> Result<Option<FileMetadata>> {
        let result = self
            .conn
            .query_row(
                &format!(
                    "SELECT {} FROM files WHERE path = ?1 AND trashed_at = 0",
                    FILE_COLUMNS
                ),
                rusqlite::params![path],
                file_from_row,
            )
            .optional()?;
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

    fn file(id: &str, path: &str) -> FileMetadata {
        FileMetadata {
            id: id.to_string(),
            path: path.to_string(),
            size: 10,
            content_hash: format!("hash-{}", id),
            modified_time: 1_700_000_000,
            created_by: "android".to_string(),
            trashed_at: 0,
            trashed_by: String::new(),
        }
    }

    #[test]
    fn two_live_items_cannot_share_a_name() {
        let db = db();
        db.insert_file(&file("f1", "example.txt")).unwrap();

        assert!(db.is_path_taken("example.txt").unwrap());
        assert!(
            db.upsert_file_from_catalog(&file("f2", "example.txt")).is_err(),
            "a second item must not be able to take a live name"
        );
    }

    #[test]
    fn trashing_frees_the_name_without_destroying_the_item() {
        let db = db();
        db.insert_file(&file("f1", "example.txt")).unwrap();
        db.set_holder(&holder("f1", "android", "hash-f1")).unwrap();

        db.trash_file("f1", "iphone", 1_700_000_900).unwrap();

        // The name is available again, so Override can reuse it.
        assert!(!db.is_path_taken("example.txt").unwrap());
        db.upsert_file_from_catalog(&file("f2", "example.txt")).unwrap();

        // The old item is still there, still with its copy attached.
        assert!(db.get_file_by_path("example.txt").unwrap().unwrap().id == "f2");
        assert_eq!(db.get_trashed_files().unwrap().len(), 1);
        assert_eq!(db.holder_count("f1").unwrap(), 1);

        let trashed = &db.get_trashed_files().unwrap()[0];
        assert_eq!(trashed.id, "f1");
        assert_eq!(trashed.trashed_by, "iphone");
        assert!(trashed.is_trashed());
    }

    #[test]
    fn trashed_items_are_hidden_from_the_catalog_but_still_replicate() {
        let db = db();
        db.insert_file(&file("f1", "a.txt")).unwrap();
        db.insert_file(&file("f2", "b.txt")).unwrap();
        db.trash_file("f2", "macos", 1).unwrap();

        // A person sees only live items...
        let live = db.get_all_files().unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].id, "f1");

        // ...but peers must learn about the deletion.
        assert_eq!(db.get_catalog_files().unwrap().len(), 2);
    }

    #[test]
    fn restoring_returns_the_item_unless_its_name_was_taken() {
        let db = db();
        db.insert_file(&file("f1", "example.txt")).unwrap();
        db.trash_file("f1", "macos", 1).unwrap();

        db.restore_file("f1").unwrap();
        assert!(db.is_path_taken("example.txt").unwrap());
        assert!(db.get_trashed_files().unwrap().is_empty());

        // Trash it again, let something else claim the name, and restoring
        // must refuse rather than quietly renaming or clobbering.
        db.trash_file("f1", "macos", 2).unwrap();
        db.insert_file(&file("f2", "example.txt")).unwrap();
        assert!(db.restore_file("f1").is_err());
    }

    #[test]
    fn override_hands_the_name_over_and_keeps_the_old_item() {
        let db = db();

        // Another device's item owns the name, and holds the only copy.
        db.insert_file(&file("existing", "example.txt")).unwrap();
        db.set_holder(&holder("existing", "android", "hash-existing"))
            .unwrap();

        // Ours arrived and was kept under a free name.
        db.insert_file(&file("incoming", "example 1.txt")).unwrap();
        db.set_holder(&holder("incoming", "linux", "hash-incoming"))
            .unwrap();

        db.override_file("existing", "incoming", "example.txt", "linux", 1_700_000_900)
            .unwrap();

        let live = db.get_file_by_path("example.txt").unwrap().unwrap();
        assert_eq!(live.id, "incoming");
        assert_eq!(db.get_all_files().unwrap().len(), 1);

        // The overridden content is not gone: still trashed, still on Android,
        // so it can be restored.
        let trashed = db.get_trashed_files().unwrap();
        assert_eq!(trashed.len(), 1);
        assert_eq!(trashed[0].id, "existing");
        assert_eq!(db.holder_count("existing").unwrap(), 1);
    }

    #[test]
    fn a_failed_override_changes_nothing() {
        let db = db();
        db.insert_file(&file("existing", "old.txt")).unwrap();
        db.insert_file(&file("incoming", "example 1.txt")).unwrap();
        // Something else already owns the name the incoming item is aiming for.
        db.insert_file(&file("other", "example.txt")).unwrap();

        assert!(db
            .override_file("existing", "incoming", "example.txt", "linux", 2)
            .is_err());

        // The item being overridden must not be left trashed for a name it
        // never received.
        assert!(db.get_trashed_files().unwrap().is_empty());
        assert!(!db
            .get_file_by_path("old.txt")
            .unwrap()
            .unwrap()
            .is_trashed());
        assert_eq!(
            db.get_file_by_path("example 1.txt").unwrap().unwrap().id,
            "incoming"
        );
    }

    #[test]
    fn merging_a_new_item_just_applies_it() {
        let db = db();
        assert_eq!(
            db.merge_catalog_file(&file("f1", "notes.txt")).unwrap(),
            MergeOutcome::Applied
        );
        assert_eq!(db.get_all_files().unwrap().len(), 1);
    }

    #[test]
    fn the_smaller_id_keeps_a_contested_name() {
        // Device A already has item "aaa" at notes.txt and receives "bbb".
        let db = db();
        db.insert_file(&file("aaa", "notes.txt")).unwrap();

        let outcome = db.merge_catalog_file(&file("bbb", "notes.txt")).unwrap();
        assert_eq!(
            outcome,
            MergeOutcome::Renamed {
                file_id: "bbb".into(),
                from: "notes.txt".into(),
                to: "notes 1.txt".into(),
                conflicting_file_id: "aaa".into(),
            }
        );

        assert_eq!(db.get_file_by_path("notes.txt").unwrap().unwrap().id, "aaa");
        assert_eq!(
            db.get_file_by_path("notes 1.txt").unwrap().unwrap().id,
            "bbb"
        );
    }

    #[test]
    fn a_smaller_incoming_id_displaces_the_local_item() {
        // Device B has "bbb" at notes.txt and receives "aaa". It must move its
        // own item aside so both devices end up agreeing.
        let db = db();
        db.insert_file(&file("bbb", "notes.txt")).unwrap();

        let outcome = db.merge_catalog_file(&file("aaa", "notes.txt")).unwrap();
        assert_eq!(
            outcome,
            MergeOutcome::Renamed {
                file_id: "bbb".into(),
                from: "notes.txt".into(),
                to: "notes 1.txt".into(),
                conflicting_file_id: "aaa".into(),
            }
        );

        assert_eq!(db.get_file_by_path("notes.txt").unwrap().unwrap().id, "aaa");
        assert_eq!(
            db.get_file_by_path("notes 1.txt").unwrap().unwrap().id,
            "bbb"
        );
    }

    #[test]
    fn both_devices_reach_the_same_layout_without_talking() {
        // The whole point of the tie-break: each side decides alone.
        let device_a = db();
        device_a.insert_file(&file("aaa", "notes.txt")).unwrap();
        device_a.merge_catalog_file(&file("bbb", "notes.txt")).unwrap();

        let device_b = db();
        device_b.insert_file(&file("bbb", "notes.txt")).unwrap();
        device_b.merge_catalog_file(&file("aaa", "notes.txt")).unwrap();

        let layout = |db: &Database| {
            let mut rows: Vec<(String, String)> = db
                .get_all_files()
                .unwrap()
                .into_iter()
                .map(|f| (f.path, f.id))
                .collect();
            rows.sort();
            rows
        };

        assert_eq!(layout(&device_a), layout(&device_b));
    }

    #[test]
    fn resyncing_does_not_keep_bumping_the_number() {
        let db = db();
        db.insert_file(&file("aaa", "notes.txt")).unwrap();

        // The peer still calls it notes.txt until it learns about ours, so the
        // same record arrives repeatedly.
        for _ in 0..5 {
            db.merge_catalog_file(&file("bbb", "notes.txt")).unwrap();
        }

        assert_eq!(
            db.get_file_by_path("notes 1.txt").unwrap().unwrap().id,
            "bbb"
        );
        assert!(db.get_file_by_path("notes 2.txt").unwrap().is_none());
        assert_eq!(db.get_all_files().unwrap().len(), 2);
    }

    #[test]
    fn a_push_into_a_stale_catalog_is_accepted_rather_than_refused() {
        // The recipient has not synced yet, so it still believes an older item
        // owns the name. Delivery must not depend on that having caught up.
        let db = db();
        db.insert_file(&file("aaa", "photo.jpg")).unwrap();

        let incoming = file("bbb", "photo.jpg");
        assert!(db.merge_catalog_file(&incoming).is_ok());

        // Landed under a free name, and both items are present.
        assert_eq!(
            db.get_file_by_path("photo 1.jpg").unwrap().unwrap().id,
            "bbb"
        );
        assert_eq!(db.get_all_files().unwrap().len(), 2);
    }

    #[test]
    fn a_trashed_incoming_item_never_contends_for_a_name() {
        let db = db();
        db.insert_file(&file("aaa", "notes.txt")).unwrap();

        let mut deleted = file("bbb", "notes.txt");
        deleted.trashed_at = 1_700_000_900;
        deleted.trashed_by = "macos".into();

        assert_eq!(
            db.merge_catalog_file(&deleted).unwrap(),
            MergeOutcome::Applied
        );
        assert_eq!(db.get_file_by_path("notes.txt").unwrap().unwrap().id, "aaa");
        assert_eq!(db.get_trashed_files().unwrap().len(), 1);
    }

    #[test]
    fn trashing_is_not_repeatable() {
        let db = db();
        db.insert_file(&file("f1", "a.txt")).unwrap();
        db.trash_file("f1", "macos", 1).unwrap();
        assert!(
            db.trash_file("f1", "macos", 2).is_err(),
            "an already-trashed item has no live copy to move aside"
        );
    }

    #[test]
    fn an_old_database_migrates_without_losing_rows() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("old.db");
        let path = path.to_string_lossy().to_string();

        // The schema as it was under the merge-and-pin model.
        {
            let conn = Connection::open(&path).expect("open");
            conn.execute_batch(
                "CREATE TABLE files (
                    id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, size INTEGER NOT NULL,
                    modified_time INTEGER NOT NULL, version INTEGER NOT NULL,
                    created_by TEXT NOT NULL, pinned_devices TEXT NOT NULL DEFAULT '[]'
                 );
                 CREATE TABLE devices (
                    id TEXT PRIMARY KEY, name TEXT NOT NULL, cert_pem TEXT NOT NULL,
                    paired_at INTEGER NOT NULL, last_seen INTEGER NOT NULL
                 );
                 INSERT INTO files VALUES ('f1', 'notes.txt', 10, 111, 3, 'android', '[\"macos\"]');
                 INSERT INTO devices VALUES ('d1', 'MacBook', 'cert', 1, 2);",
            )
            .expect("seed old schema");
        }

        let db = Database::init(&path).expect("migrate");

        // Nothing was dropped on the way.
        let migrated = db.get_file_by_path("notes.txt").expect("query").expect("row");
        assert_eq!(migrated.id, "f1");
        assert_eq!(migrated.created_by, "android");
        assert_eq!(migrated.size, 10);
        assert!(db.is_paired("d1").expect("query"));
        assert_eq!(db.get_paired_devices().expect("query")[0].name, "MacBook");

        // The merge-and-pin columns are gone and the new ones are in place.
        let columns = db.column_names("files").expect("columns");
        assert!(!columns.iter().any(|c| c == "version"));
        assert!(!columns.iter().any(|c| c == "pinned_devices"));
        assert!(columns.iter().any(|c| c == "content_hash"));
        assert!(columns.iter().any(|c| c == "trashed_at"));

        // The inline UNIQUE(path) is gone, replaced by uniqueness among live
        // items only - otherwise a trashed item would own its name forever.
        db.trash_file("f1", "macos", 1).expect("trash");
        db.upsert_file_from_catalog(&file("f2", "notes.txt"))
            .expect("a trashed name must be reusable");

        // Blocks still reference files after the rebuild.
        db.insert_block(&BlockMetadata { id: "b1".into(), size: 1, is_present: 1 })
            .expect("block");
        db.map_block_to_file("f2", "b1", 0)
            .expect("foreign key survived the rebuild");

        // Migrating twice must be a no-op rather than an error.
        drop(db);
        Database::init(&path).expect("second migration is idempotent");
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

    #[test]
    fn a_database_keyed_on_block_content_is_rebuilt_to_key_on_position() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("old.db");
        let path = path.to_string_lossy().to_string();

        // The schema as it was when a manifest was a set of blocks. Note the
        // file that repeats a block: under the old key the second occurrence
        // overwrote the first, so the file lost a third of itself.
        {
            let conn = Connection::open(&path).expect("open");
            conn.execute_batch(
                "CREATE TABLE files (
                    id TEXT PRIMARY KEY, path TEXT NOT NULL, size INTEGER NOT NULL,
                    content_hash TEXT NOT NULL DEFAULT '',
                    modified_time INTEGER NOT NULL, created_by TEXT NOT NULL,
                    trashed_at INTEGER NOT NULL DEFAULT 0,
                    trashed_by TEXT NOT NULL DEFAULT ''
                 );
                 CREATE TABLE blocks (
                    id TEXT PRIMARY KEY, size INTEGER NOT NULL,
                    is_present INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE TABLE file_blocks (
                    file_id TEXT NOT NULL, block_id TEXT NOT NULL,
                    block_index INTEGER NOT NULL,
                    PRIMARY KEY (file_id, block_id)
                 );
                 INSERT INTO files (id, path, size, modified_time, created_by)
                    VALUES ('f1', 'padded.bin', 30, 1, 'laptop');
                 INSERT INTO blocks VALUES ('aaa', 10, 1), ('bbb', 10, 1);
                 INSERT INTO file_blocks VALUES ('f1', 'aaa', 0), ('f1', 'bbb', 1);",
            )
            .expect("seed old schema");
        }

        let db = Database::init(&path).expect("migrate");

        // The mappings that survived are carried over untouched.
        let blocks = db.get_blocks_for_file("f1").expect("query");
        assert_eq!(
            blocks.iter().map(|b| b.block_id.as_str()).collect::<Vec<_>>(),
            vec!["aaa", "bbb"]
        );

        // And the same block may now appear twice in one file, which is the
        // whole point of the rebuild.
        db.map_block_to_file("f1", "aaa", 2).expect("repeat a block");
        let blocks = db.get_blocks_for_file("f1").expect("query");
        assert_eq!(
            blocks.iter().map(|b| b.block_id.as_str()).collect::<Vec<_>>(),
            vec!["aaa", "bbb", "aaa"],
            "a repeated block must occupy its own position"
        );

        assert!(
            !db.file_blocks_keyed_by_content().expect("inspect schema"),
            "the old key must be gone, not merely worked around"
        );
    }
}
