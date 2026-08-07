use anyhow::{bail, Result};
use sha2::{Sha256, Digest};
use std::path::{Path, PathBuf};
use std::fs::{self, File};
use std::io::{BufReader, ErrorKind, Read, Write};
use crate::db::{Database, BlockMetadata, FileBlock};

/// How much of a file goes into one block.
///
/// This is a transfer decision far more than a storage one. Every block costs a
/// separate HTTP request, so at the original 4 KiB a 1 GB file meant 262,144
/// round-trips, and no amount of bandwidth could make that finish in a sensible
/// time. A megabyte brings the same file down to 1,024, which a LAN can
/// actually saturate.
///
/// Little is given up. Chunking is fixed-size, so blocks only ever dedup
/// against byte-identical, identically-aligned content; small blocks bought
/// finer sharing only in the narrow case of a file edited without any bytes
/// shifting. Against that, every block carries two database rows, a file in the
/// storage directory, and a request of its own.
pub const BLOCK_SIZE: usize = 1024 * 1024;

pub fn ensure_storage_dir(base_dir: &str) -> Result<()> {
    fs::create_dir_all(base_dir)?;
    Ok(())
}

pub fn get_block_path(base_dir: &str, block_id: &str) -> PathBuf {
    Path::new(base_dir).join(block_id)
}

/// The id a piece of content is stored under.
///
/// Blocks are content-addressed: the id *is* the SHA-256 of the bytes, which is
/// what lets a receiver check that what arrived is what it asked for.
pub fn block_id_for(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Whether a string could name a block at all.
///
/// Ids arrive from other devices in a URL path and get joined onto the storage
/// directory. Without this check `../../../.ssh/authorized_keys` is a block id,
/// and a paired device could read or overwrite any file this process can reach.
/// Only a lowercase hex SHA-256 digest is a block id.
pub fn is_valid_block_id(block_id: &str) -> bool {
    block_id.len() == 64 && block_id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn chunk_and_store_file(base_dir: &str, db: &Database, file_id: &str, file_path_on_disk: &str) -> Result<()> {
    let mut file = BufReader::new(File::open(file_path_on_disk)?);
    // On the heap: a block is a megabyte now, far too much for a stack frame.
    let mut buffer = vec![0u8; BLOCK_SIZE];
    let mut index = 0;

    loop {
        let bytes_read = fill(&mut file, &mut buffer)?;
        if bytes_read == 0 {
            break;
        }

        let block_id = block_id_for(&buffer[..bytes_read]);
        store_block(base_dir, &block_id, &buffer[..bytes_read])?;

        db.insert_block(&BlockMetadata {
            id: block_id.clone(),
            size: bytes_read as i64,
            is_present: 1,
        })?;
        db.map_block_to_file(file_id, &block_id, index)?;

        index += 1;
    }
    Ok(())
}

/// Reads until the buffer is full or the file ends, and reports how much of it
/// was filled.
///
/// `Read::read` is free to return less than it was asked for at any time, and
/// at a megabyte a call it regularly does. Taking whatever a single call
/// returned would make block boundaries depend on how the read happened to be
/// serviced, so two devices could split identical files differently, share no
/// blocks between them, and disagree about the content hash that decides
/// whether their copies match.
fn fill(reader: &mut impl Read, buffer: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(filled)
}

/// Puts content into storage under an id already known to be its hash.
///
/// The bytes go to a temporary name and are renamed into place, so a write cut
/// short leaves nothing behind rather than a truncated block. That matters now
/// that a block is a megabyte: storing treats an existing path as already done,
/// so a short block would be served and assembled forever with nothing to
/// repair it. Rename covers a crash or an error part-way; surviving power loss
/// would need an fsync per block, which is not worth a thousand of them per
/// gigabyte for data a peer can simply send again.
fn store_block(base_dir: &str, block_id: &str, data: &[u8]) -> std::io::Result<()> {
    let path = get_block_path(base_dir, block_id);
    if path.exists() {
        return Ok(());
    }

    // Unique per attempt, so two transfers racing on the same block cannot end
    // up sharing one half-written temporary.
    let temp = Path::new(base_dir).join(format!(".{}.{}.partial", block_id, uuid::Uuid::new_v4()));

    let written = File::create(&temp)
        .and_then(|mut file| file.write_all(data))
        .and_then(|_| fs::rename(&temp, &path));

    if let Err(e) = written {
        let _ = fs::remove_file(&temp);
        return Err(e);
    }
    Ok(())
}

/// Identifies the exact content of a file as an ordered manifest of its blocks.
///
/// Block ids are already SHA-256 of block contents, so hashing them in order
/// pins both the bytes and their arrangement. Two copies of a file are the same
/// content precisely when this matches, which is what lets the catalog say
/// which devices hold a current copy and which are behind.
pub fn content_hash(blocks: &[FileBlock]) -> String {
    let mut hasher = Sha256::new();
    for block in blocks {
        hasher.update(block.block_id.as_bytes());
        hasher.update(b"\n");
    }
    hex::encode(hasher.finalize())
}

pub fn read_block(base_dir: &str, block_id: &str) -> Result<Vec<u8>> {
    if !is_valid_block_id(block_id) {
        bail!("{} is not a block id", block_id);
    }
    let path = get_block_path(base_dir, block_id);
    let mut file = File::open(path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    Ok(buffer)
}

/// Why a block offered by another device was not stored.
///
/// Separated from a plain error so a caller can tell whose fault it was: the
/// first two mean the sender is wrong and no retry will help, the third means
/// this device is.
#[derive(Debug, thiserror::Error)]
pub enum BlockError {
    #[error("{0} is not a block id")]
    NotABlockId(String),
    #[error("contents do not hash to {0}")]
    ContentMismatch(String),
    #[error("could not store the block: {0}")]
    Storage(#[from] std::io::Error),
}

/// Stores a block that came from another device.
///
/// Everything arriving over the network lands here, so this is where the
/// content-addressing invariant is enforced: an id that is not a hash, or
/// contents that do not hash to the id they were sent under, are refused. A
/// paired device could otherwise serve any bytes it liked and have them
/// assembled into a file unnoticed, or name a block in a way that escapes the
/// storage directory entirely.
pub fn write_block(base_dir: &str, block_id: &str, data: &[u8]) -> Result<(), BlockError> {
    if !is_valid_block_id(block_id) {
        return Err(BlockError::NotABlockId(block_id.to_string()));
    }
    if block_id_for(data) != block_id {
        return Err(BlockError::ContentMismatch(block_id.to_string()));
    }
    Ok(store_block(base_dir, block_id, data)?)
}

/// Deletes a stored block. Callers must have established that nothing else
/// references it - see `Database::blocks_exclusive_to_file`.
pub fn remove_block(base_dir: &str, block_id: &str) -> Result<()> {
    if !is_valid_block_id(block_id) {
        bail!("{} is not a block id", block_id);
    }
    let path = get_block_path(base_dir, block_id);
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn assemble_file_from_blocks(base_dir: &str, output_path: &str, blocks: &[FileBlock]) -> Result<()> {
    let mut file = File::create(output_path)?;
    for block in blocks {
        let data = read_block(base_dir, &block.block_id)?;
        file.write_all(&data)?;
    }
    file.flush()?;
    Ok(())
}

pub fn get_trusted_peers_dir(base_dir: &str) -> PathBuf {
    Path::new(base_dir).join("trusted_peers")
}

pub fn ensure_trusted_peers_dir(base_dir: &str) -> Result<()> {
    fs::create_dir_all(get_trusted_peers_dir(base_dir))?;
    Ok(())
}

pub fn save_peer_cert(base_dir: &str, peer_id: &str, cert_pem: &str) -> Result<()> {
    let path = get_trusted_peers_dir(base_dir).join(format!("{}.pem", peer_id));
    let mut file = File::create(path)?;
    file.write_all(cert_pem.as_bytes())?;
    Ok(())
}

/// Drops a pinned certificate. The trust store must be reloaded afterwards for
/// the revocation to take effect.
pub fn remove_peer_cert(base_dir: &str, peer_id: &str) -> Result<()> {
    let path = get_trusted_peers_dir(base_dir).join(format!("{}.pem", peer_id));
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn load_all_trusted_certs(base_dir: &str) -> Result<Vec<String>> {
    let dir = get_trusted_peers_dir(base_dir);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut certs = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.path().extension().and_then(|s| s.to_str()) == Some("pem") {
            let pem = fs::read_to_string(entry.path())?;
            certs.push(pem);
        }
    }
    Ok(certs)
}
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Content that differs from block to block, so a test cannot pass by
    /// accident on every block hashing to the same id.
    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    struct Fixture {
        _dir: TempDir,
        base: String,
        db: Database,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = TempDir::new().expect("temp dir");
            let base = dir.path().to_string_lossy().to_string();
            let db = Database::init(&format!("{}/test.db", base)).expect("db");
            Self { _dir: dir, base, db }
        }

        fn chunk(&self, name: &str, file_id: &str, contents: &[u8]) -> Vec<FileBlock> {
            let path = format!("{}/{}", self.base, name);
            fs::write(&path, contents).expect("write source file");
            self.db
                .insert_file(&crate::db::FileMetadata {
                    id: file_id.to_string(),
                    path: name.to_string(),
                    size: contents.len() as i64,
                    content_hash: String::new(),
                    modified_time: 1,
                    created_by: "device".into(),
                    trashed_at: 0,
                    trashed_by: String::new(),
                })
                .expect("record file");
            chunk_and_store_file(&self.base, &self.db, file_id, &path).expect("chunk");
            self.db.get_blocks_for_file(file_id).expect("blocks")
        }
    }

    /// A reader that hands back less than it was asked for, the way a socket or
    /// a large read from a real file is entitled to.
    struct Reluctant<'a> {
        data: &'a [u8],
        most_at_once: usize,
    }

    impl Read for Reluctant<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.data.len().min(buf.len()).min(self.most_at_once);
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    #[test]
    fn a_short_read_does_not_end_a_block_early() {
        // The whole of chunking rests on this: if a block ended wherever a read
        // happened to stop, two devices would split the same file differently,
        // share no blocks, and disagree on the content hash that decides
        // whether their copies match.
        let data = pattern(BLOCK_SIZE);
        let mut reader = Reluctant { data: &data, most_at_once: 7 };
        let mut buffer = vec![0u8; BLOCK_SIZE];

        assert_eq!(fill(&mut reader, &mut buffer).expect("fill"), BLOCK_SIZE);
        assert_eq!(buffer, data);
        assert_eq!(fill(&mut reader, &mut buffer).expect("fill"), 0, "the file has ended");
    }

    #[test]
    fn a_file_larger_than_one_block_splits_on_block_boundaries() {
        let f = Fixture::new();
        let contents = pattern(BLOCK_SIZE * 2 + 512);
        let blocks = f.chunk("big.bin", "file-1", &contents);

        assert_eq!(blocks.len(), 3, "two full blocks and a remainder");
        assert_eq!(
            blocks.iter().map(|b| b.size).collect::<Vec<_>>(),
            vec![BLOCK_SIZE as i64, BLOCK_SIZE as i64, 512],
            "only the last block may be short"
        );
        assert!(blocks.iter().all(|b| b.is_present == 1));
    }

    #[test]
    fn a_chunked_file_reassembles_byte_for_byte() {
        let f = Fixture::new();
        let contents = pattern(BLOCK_SIZE * 3 + 9_001);
        let blocks = f.chunk("big.bin", "file-1", &contents);

        let out = format!("{}/rebuilt.bin", f.base);
        assemble_file_from_blocks(&f.base, &out, &blocks).expect("assemble");

        assert_eq!(fs::read(&out).expect("read rebuilt"), contents);
    }

    #[test]
    fn two_files_sharing_a_leading_region_share_its_blocks() {
        // Fixed-size chunking only dedups content that is identical and
        // identically aligned, which is exactly what a shared prefix is.
        let f = Fixture::new();
        let prefix = pattern(BLOCK_SIZE * 2);

        let mut a = prefix.clone();
        a.extend_from_slice(b"ending one");
        let mut b = prefix.clone();
        b.extend_from_slice(b"a different ending");

        let a_blocks = f.chunk("a.bin", "file-a", &a);
        let b_blocks = f.chunk("b.bin", "file-b", &b);

        let ids = |blocks: &[FileBlock]| -> Vec<String> {
            blocks.iter().map(|b| b.block_id.clone()).collect()
        };
        assert_eq!(
            ids(&a_blocks[..2]),
            ids(&b_blocks[..2]),
            "the shared region is one set of blocks"
        );
        assert_ne!(a_blocks[2].block_id, b_blocks[2].block_id);
    }

    #[test]
    fn a_block_whose_contents_do_not_match_its_id_is_refused() {
        let f = Fixture::new();
        let honest = block_id_for(b"the block that was asked for");

        let refused = write_block(&f.base, &honest, b"something else entirely");

        assert!(matches!(refused, Err(BlockError::ContentMismatch(_))));
        assert!(
            !get_block_path(&f.base, &honest).exists(),
            "nothing may be stored under an id it does not hash to"
        );
    }

    #[test]
    fn a_block_id_that_is_really_a_path_is_refused() {
        // Ids arrive from other devices and get joined onto the storage
        // directory, so anything that is not a bare hash must not reach the
        // filesystem at all.
        let f = Fixture::new();
        let escape = "../../../etc/passwd";

        assert!(!is_valid_block_id(escape));
        assert!(matches!(
            write_block(&f.base, escape, b"payload"),
            Err(BlockError::NotABlockId(_))
        ));
        assert!(read_block(&f.base, escape).is_err());
        assert!(remove_block(&f.base, escape).is_err());
    }

    #[test]
    fn an_interrupted_write_leaves_no_block_behind() {
        // A truncated block would be indistinguishable from a stored one and
        // would be served and assembled forever, so the write must be all or
        // nothing. Storing into a directory that does not exist stands in for
        // any failure part-way through.
        let f = Fixture::new();
        let missing = format!("{}/nowhere", f.base);
        let id = block_id_for(b"contents");

        assert!(write_block(&missing, &id, b"contents").is_err());
        assert!(
            !Path::new(&missing).exists(),
            "a failed write must not leave a partial file"
        );
    }
}
