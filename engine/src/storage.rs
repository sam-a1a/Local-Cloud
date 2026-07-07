use anyhow::Result;
use sha2::{Sha256, Digest};
use std::path::{Path, PathBuf};
use std::fs::{self, File};
use std::io::{Read, Write};
use crate::db::{Database, BlockMetadata, FileBlock};

const BLOCK_SIZE: usize = 4096;

pub fn ensure_storage_dir(base_dir: &str) -> Result<()> {
    fs::create_dir_all(base_dir)?;
    Ok(())
}

pub fn get_block_path(base_dir: &str, block_id: &str) -> PathBuf {
    Path::new(base_dir).join(block_id)
}

pub fn chunk_and_store_file(base_dir: &str, db: &Database, file_id: &str, file_path_on_disk: &str) -> Result<()> {
    let mut file = File::open(file_path_on_disk)?;
    let mut buffer = [0; BLOCK_SIZE];
    let mut index = 0;

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }

        let mut hasher = Sha256::new();
        hasher.update(&buffer[..bytes_read]);
        let hash = hasher.finalize();
        let block_id = hex::encode(hash);

        let block_path = get_block_path(base_dir, &block_id);
        if !block_path.exists() {
            let mut block_file = File::create(&block_path)?;
            block_file.write_all(&buffer[..bytes_read])?;
        }

        let block = BlockMetadata {
            id: block_id.clone(),
            size: bytes_read as i64,
            is_present: 1,
        };
        db.insert_block(&block)?;
        db.map_block_to_file(file_id, &block_id, index)?;

        index += 1;
    }
    Ok(())
}

pub fn read_block(base_dir: &str, block_id: &str) -> Result<Vec<u8>> {
    let path = get_block_path(base_dir, block_id);
    let mut file = File::open(path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    Ok(buffer)
}

pub fn write_block(base_dir: &str, block_id: &str, data: &[u8]) -> Result<()> {
    let path = get_block_path(base_dir, block_id);
    if !path.exists() {
        let mut file = File::create(&path)?;
        file.write_all(data)?;
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