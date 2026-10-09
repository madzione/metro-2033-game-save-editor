//! Names are stored as CRC32 hashes. This module turns them back into text.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::util::{cstrings, is_printable};

const BLOCK_NAMES: &str = include_str!("../data/block_names.txt");
const CLASS_NAMES: &str = include_str!("../data/class_names.txt");

pub fn crc(s: &str) -> u32 {
    crc32fast::hash(s.as_bytes())
}

#[derive(Clone, Default)]
pub struct Names {
    map: HashMap<u32, String>,
    blocks: HashSet<u32>,
}

impl Names {
    pub fn builtin() -> Self {
        let mut n = Self::default();
        for line in BLOCK_NAMES.lines().filter(|l| !l.is_empty()) {
            let h = crc(line);
            n.blocks.insert(h);
            n.map.insert(h, line.to_string());
        }
        for line in CLASS_NAMES.lines().filter(|l| !l.is_empty()) {
            n.add(line);
        }
        n
    }

    pub fn add(&mut self, s: &str) {
        self.map.entry(crc(s)).or_insert_with(|| s.to_string());
    }

    /// Learn every string stored in a buffer (entity names, animation names, ...).
    pub fn learn_from(&mut self, data: &[u8]) {
        for (_, s) in cstrings(data, 2) {
            self.add(&s);
        }
    }

    /// Load an extra dictionary, one name per line (see the `dict` command).
    pub fn load_file(&mut self, path: &Path) -> std::io::Result<usize> {
        let text = std::fs::read(path)?;
        let mut count = 0;
        for line in text.split(|&c| c == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.len() >= 2 && line.iter().all(|&c| is_printable(c)) {
                let s = String::from_utf8_lossy(line);
                self.add(&s);
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn get(&self, h: u32) -> Option<&str> {
        self.map.get(&h).map(String::as_str)
    }

    pub fn name(&self, h: u32) -> String {
        self.get(h).map(str::to_string).unwrap_or_else(|| format!("{h:#010x}"))
    }

    pub fn is_block(&self, h: u32) -> bool {
        self.blocks.contains(&h)
    }
}

/// Extract all printable strings (like `strings -n 3`) from a game binary,
/// for use with [`Names::load_file`].
pub fn extract_strings(binary: &[u8]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut res = Vec::new();
    let mut start = None;
    for (i, &c) in binary.iter().chain(std::iter::once(&0u8)).enumerate() {
        if is_printable(c) {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            let len = i - s;
            if (3..=120).contains(&len) {
                let st = String::from_utf8_lossy(&binary[s..i]).into_owned();
                if seen.insert(st.clone()) {
                    res.push(st);
                }
            }
        }
    }
    res
}
