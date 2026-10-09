use anyhow::{bail, Result};

pub fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

pub fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub fn rd_i32(b: &[u8], o: usize) -> i32 {
    rd_u32(b, o) as i32
}

pub fn rd_f32(b: &[u8], o: usize) -> f32 {
    f32::from_bits(rd_u32(b, o))
}

pub fn wr_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn wr_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Bounds-checked reads used by the parsers.
pub fn get_u16(b: &[u8], o: usize) -> Option<u16> {
    (o + 2 <= b.len()).then(|| rd_u16(b, o))
}

pub fn get_u32(b: &[u8], o: usize) -> Option<u32> {
    (o + 4 <= b.len()).then(|| rd_u32(b, o))
}

pub fn is_printable(c: u8) -> bool {
    (0x20..0x7f).contains(&c)
}

/// All NUL-terminated printable strings of at least `min` characters.
pub fn cstrings(b: &[u8], min: usize) -> Vec<(usize, String)> {
    let mut res = Vec::new();
    let mut start = None;
    for (i, &c) in b.iter().enumerate() {
        if is_printable(c) {
            start.get_or_insert(i);
        } else {
            if let Some(s) = start.take() {
                if c == 0 && i - s >= min {
                    res.push((s, String::from_utf8_lossy(&b[s..i]).into_owned()));
                }
            }
        }
    }
    res
}

pub fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 3);
    for (i, x) in b.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{x:02x}"));
    }
    s
}

pub fn parse_hex(s: &str) -> Result<Vec<u8>> {
    let clean: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ',' && *c != ':')
        .collect();
    let clean = clean.strip_prefix("0x").unwrap_or(&clean);
    if clean.len() % 2 != 0 {
        bail!("hex string has an odd number of digits");
    }
    (0..clean.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).map_err(|e| anyhow::anyhow!("bad hex: {e}")))
        .collect()
}

pub fn parse_num(s: &str) -> Result<usize> {
    let s = s.trim();
    Ok(match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => usize::from_str_radix(h, 16)?,
        None => s.parse()?,
    })
}

/// Classic 16-bytes-per-line hex dump.
pub fn hexdump(b: &[u8], base: usize) -> String {
    let mut s = String::new();
    for (row, chunk) in b.chunks(16).enumerate() {
        s.push_str(&format!("{:08x}  ", base + row * 16));
        for i in 0..16 {
            match chunk.get(i) {
                Some(x) => s.push_str(&format!("{x:02x} ")),
                None => s.push_str("   "),
            }
            if i == 7 {
                s.push(' ');
            }
        }
        s.push(' ');
        for &c in chunk {
            s.push(if is_printable(c) { c as char } else { '.' });
        }
        s.push('\n');
    }
    s
}
