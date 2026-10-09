//! Structure of the decompressed `dynamic` and `active` sections.
//!
//! Both are lists of sub chunks (`u32 name_hash, u32 size, payload`). Every
//! object inside is wrapped in a "blob":
//!
//! ```text
//! u32 size            (= 9 + payload length)
//! u32 0x4b11a930      (chunk magic)
//! u8  0xd4
//! u32 payload length
//! payload
//! ```
//!
//! `dynamic` (section 1): sub chunks `sub_chunk_dynamic`, each starting with an
//! 11 byte prefix, followed by entity slots `{ u16 id, blob | u32 0x80000000 }`
//! (0x80000000 marks a free id).
//!
//! `active` (section 2): sub chunks `sub_chunk_active` holding runtime records
//! `{ u16 id, u16 id2, blob }` (scripts states, animation state, AI, ...).

use anyhow::{ensure, Context, Result};

use crate::util::{get_u16, get_u32, wr_u16, wr_u32};

pub const CHUNK_MAGIC: u32 = 0x4b11_a930;
pub const CHUNK_TAG: u8 = 0xd4;
pub const EMPTY_SLOT: u32 = 0x8000_0000;
const BLOB_OVERHEAD: usize = 9;
const DYN_PREFIX_LEN: usize = 11;

fn read_blob(b: &[u8], pos: usize) -> Result<(Vec<u8>, usize)> {
    let size = get_u32(b, pos).context("truncated blob size")? as usize;
    let end = pos + 4 + size;
    ensure!(size >= BLOB_OVERHEAD && end <= b.len(), "bad blob size {size} at {pos:#x}");
    ensure!(get_u32(b, pos + 4) == Some(CHUNK_MAGIC) && b[pos + 8] == CHUNK_TAG, "missing chunk magic at {:#x}", pos + 4);
    let inner = get_u32(b, pos + 9).unwrap() as usize;
    ensure!(inner + BLOB_OVERHEAD == size, "inconsistent blob sizes at {pos:#x}");
    Ok((b[pos + 13..end].to_vec(), end))
}

fn write_blob(out: &mut Vec<u8>, data: &[u8]) {
    wr_u32(out, (data.len() + BLOB_OVERHEAD) as u32);
    wr_u32(out, CHUNK_MAGIC);
    out.push(CHUNK_TAG);
    wr_u32(out, data.len() as u32);
    out.extend_from_slice(data);
}

#[derive(Clone, Debug)]
pub struct Slot {
    pub id: u16,
    /// `None` for an unused id.
    pub data: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct DynSub {
    pub hash: u32,
    pub prefix: Vec<u8>,
    pub slots: Vec<Slot>,
}

#[derive(Clone, Debug)]
pub struct ActiveRec {
    pub id: u16,
    pub id2: u16,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ActiveSub {
    pub hash: u32,
    pub recs: Vec<ActiveRec>,
}

pub fn parse_dynamic(b: &[u8]) -> Result<Vec<DynSub>> {
    let mut subs = Vec::new();
    let mut p = 0;
    while p < b.len() {
        let hash = get_u32(b, p).context("truncated sub chunk")?;
        let size = get_u32(b, p + 4).context("truncated sub chunk")? as usize;
        let end = p + 8 + size;
        ensure!(end <= b.len() && size >= DYN_PREFIX_LEN, "bad dynamic sub chunk at {p:#x}");
        ensure!(get_u32(b, p + 8) == Some(CHUNK_MAGIC), "missing magic in dynamic sub chunk at {p:#x}");
        let prefix = b[p + 8..p + 8 + DYN_PREFIX_LEN].to_vec();
        let mut q = p + 8 + DYN_PREFIX_LEN;
        let mut slots = Vec::new();
        while q < end {
            let id = get_u16(b, q).context("truncated slot")?;
            if get_u32(b, q + 2) == Some(EMPTY_SLOT) {
                slots.push(Slot { id, data: None });
                q += 6;
            } else {
                let (data, next) = read_blob(b, q + 2).with_context(|| format!("entity slot {id:#06x}"))?;
                slots.push(Slot { id, data: Some(data) });
                q = next;
            }
        }
        ensure!(q == end, "dynamic sub chunk at {p:#x} overruns");
        subs.push(DynSub { hash, prefix, slots });
        p = end;
    }
    Ok(subs)
}

pub fn write_dynamic(subs: &[DynSub]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in subs {
        let mut body = s.prefix.clone();
        for slot in &s.slots {
            wr_u16(&mut body, slot.id);
            match &slot.data {
                Some(d) => write_blob(&mut body, d),
                None => wr_u32(&mut body, EMPTY_SLOT),
            }
        }
        wr_u32(&mut out, s.hash);
        wr_u32(&mut out, body.len() as u32);
        out.extend_from_slice(&body);
    }
    out
}

pub fn parse_active(b: &[u8]) -> Result<Vec<ActiveSub>> {
    let mut subs = Vec::new();
    let mut p = 0;
    while p < b.len() {
        let hash = get_u32(b, p).context("truncated sub chunk")?;
        let size = get_u32(b, p + 4).context("truncated sub chunk")? as usize;
        let end = p + 8 + size;
        ensure!(end <= b.len(), "bad active sub chunk at {p:#x}");
        let mut q = p + 8;
        let mut recs = Vec::new();
        while q < end {
            let id = get_u16(b, q).context("truncated record")?;
            let id2 = get_u16(b, q + 2).context("truncated record")?;
            let (data, next) = read_blob(b, q + 4).with_context(|| format!("active record {id:#06x}"))?;
            recs.push(ActiveRec { id, id2, data });
            q = next;
        }
        ensure!(q == end, "active sub chunk at {p:#x} overruns");
        subs.push(ActiveSub { hash, recs });
        p = end;
    }
    Ok(subs)
}

pub fn write_active(subs: &[ActiveSub]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in subs {
        let mut body = Vec::new();
        for r in &s.recs {
            wr_u16(&mut body, r.id);
            wr_u16(&mut body, r.id2);
            write_blob(&mut body, &r.data);
        }
        wr_u32(&mut out, s.hash);
        wr_u32(&mut out, body.len() as u32);
        out.extend_from_slice(&body);
    }
    out
}
