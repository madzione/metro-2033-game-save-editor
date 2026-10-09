//! The outer `DEZP` container.
//!
//! ```text
//! "DEZP"
//! 3 x { u32 uncompressed_size, u32 section_id, u32 compressed_size }
//! for every section: chunks of
//!     { u32 chunk_size (incl. this 8 byte header), u32 uncompressed_size, lz4 block }
//! u32 crc32 (zlib/IEEE) of every byte before it
//! ```
//!
//! The LZ4 blocks of one section are "linked": a block may reference up to
//! 64 KiB of the previously decompressed output of the same section.
//! Writing independent blocks is a valid subset of that, so we do that.

use anyhow::{bail, ensure, Context, Result};

use crate::util::{rd_u32, wr_u32};

pub const MAGIC: &[u8; 4] = b"DEZP";
pub const SECTION_COUNT: usize = 3;
const HEADER_LEN: usize = 4 + SECTION_COUNT * 12;
const CHUNK_HEADER_LEN: usize = 8;
const LZ4_WINDOW: usize = 64 * 1024;
/// Chunk sizes used by the game when it writes a save.
const FIRST_CHUNK: usize = 0x30000;
const NEXT_CHUNK: usize = 0x20000;

#[derive(Clone, Debug)]
pub struct Section {
    /// CRC32 of the section name ("header", "dynamic", "active").
    pub id: u32,
    pub data: Vec<u8>,
    /// Uncompressed sizes of the chunks the section was stored in. Reused when
    /// repacking a section whose size did not change.
    pub chunk_sizes: Vec<usize>,
    /// Compressed size as found in the file (informational).
    pub stored_compressed_size: usize,
}

#[derive(Clone, Debug)]
pub struct Container {
    pub sections: Vec<Section>,
}

impl Container {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() >= HEADER_LEN + 4, "file too small to be a save");
        ensure!(&bytes[..4] == MAGIC, "missing DEZP magic, not a Metro 2033 Redux save");

        let body_len = bytes.len() - 4;
        let stored_crc = rd_u32(bytes, body_len);
        let calc_crc = crc32fast::hash(&bytes[..body_len]);
        ensure!(
            stored_crc == calc_crc,
            "checksum mismatch (stored {stored_crc:#010x}, calculated {calc_crc:#010x})"
        );

        let mut pos = HEADER_LEN;
        let mut sections = Vec::with_capacity(SECTION_COUNT);
        for i in 0..SECTION_COUNT {
            let usize_ = rd_u32(bytes, 4 + i * 12) as usize;
            let id = rd_u32(bytes, 8 + i * 12);
            let csize = rd_u32(bytes, 12 + i * 12) as usize;
            let end = pos
                .checked_add(csize)
                .filter(|&e| e <= body_len)
                .with_context(|| format!("section {i} exceeds file"))?;

            let mut data: Vec<u8> = Vec::with_capacity(usize_);
            let mut chunk_sizes = Vec::new();
            while pos < end {
                ensure!(pos + CHUNK_HEADER_LEN <= end, "truncated chunk header in section {i}");
                let chunk_len = rd_u32(bytes, pos) as usize;
                let out_len = rd_u32(bytes, pos + 4) as usize;
                ensure!(
                    chunk_len >= CHUNK_HEADER_LEN && pos + chunk_len <= end,
                    "bad chunk size {chunk_len} at {pos:#x}"
                );
                let src = &bytes[pos + CHUNK_HEADER_LEN..pos + chunk_len];
                let dict = &data[data.len().saturating_sub(LZ4_WINDOW)..];
                let out = lz4_flex::block::decompress_with_dict(src, out_len, dict)
                    .with_context(|| format!("LZ4 error in chunk at {pos:#x}"))?;
                ensure!(out.len() == out_len, "chunk at {pos:#x} decompressed to wrong size");
                data.extend_from_slice(&out);
                chunk_sizes.push(out_len);
                pos += chunk_len;
            }
            ensure!(
                data.len() == usize_,
                "section {i}: expected {usize_} bytes, got {}",
                data.len()
            );
            sections.push(Section { id, data, chunk_sizes, stored_compressed_size: csize });
        }
        if pos != body_len {
            bail!("{} unexpected bytes after the last section", body_len - pos);
        }
        Ok(Self { sections })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut compressed = Vec::with_capacity(self.sections.len());
        for s in &self.sections {
            let mut out = Vec::new();
            let mut off = 0;
            for len in chunk_layout(s) {
                let block = lz4_flex::block::compress(&s.data[off..off + len]);
                wr_u32(&mut out, (block.len() + CHUNK_HEADER_LEN) as u32);
                wr_u32(&mut out, len as u32);
                out.extend_from_slice(&block);
                off += len;
            }
            compressed.push(out);
        }

        let mut file = Vec::with_capacity(HEADER_LEN + compressed.iter().map(Vec::len).sum::<usize>() + 4);
        file.extend_from_slice(MAGIC);
        for (s, c) in self.sections.iter().zip(&compressed) {
            wr_u32(&mut file, s.data.len() as u32);
            wr_u32(&mut file, s.id);
            wr_u32(&mut file, c.len() as u32);
        }
        for c in &compressed {
            file.extend_from_slice(c);
        }
        let crc = crc32fast::hash(&file);
        wr_u32(&mut file, crc);
        file
    }
}

/// Keep the original chunking if the section size is unchanged, otherwise
/// split the way the game does (0x30000, then 0x20000 bytes).
fn chunk_layout(s: &Section) -> Vec<usize> {
    if !s.chunk_sizes.is_empty() && s.chunk_sizes.iter().sum::<usize>() == s.data.len() {
        return s.chunk_sizes.clone();
    }
    let mut sizes = Vec::new();
    let mut left = s.data.len();
    let mut next = FIRST_CHUNK;
    while left > 0 {
        let n = left.min(next);
        sizes.push(n);
        left -= n;
        next = NEXT_CHUNK;
    }
    sizes
}
