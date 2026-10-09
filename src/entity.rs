//! Common header of an entity record in the `dynamic` section.
//!
//! ```text
//! u32 class (crc32 of e.g. "O_ENTITY", "VISUALSCRIPT", "NPC_FX")
//! u32 static data key
//! char attach_bone[]   (NUL terminated, usually empty)
//! u16 id
//! u16 parent id (0xffff = none)
//! ...
//! char name[]          (NUL terminated)
//! u16 flags
//! f32 transform[12]    (3x3 rotation, then x y z)
//! class specific data
//! ```

use crate::names::Names;
use crate::util::{is_printable, rd_f32, rd_u16, rd_u32};

#[derive(Clone, Debug)]
pub struct EntityInfo {
    pub class: u32,
    pub static_key: u32,
    pub attach_bone: String,
    pub id: u16,
    pub parent: u16,
    pub name: String,
    pub name_offset: usize,
    pub flags: u16,
    /// Offset of the 12 transform floats inside the record, if present.
    pub transform_offset: Option<usize>,
    pub position: Option<[f32; 3]>,
}

impl EntityInfo {
    pub fn class_name(&self, names: &Names) -> String {
        names.name(self.class)
    }
}

/// Runtime id stored inside the record (the slot key in the `dynamic`
/// section is the id of the template for cloned entities).
pub fn runtime_id(data: &[u8]) -> Option<u16> {
    let bone_end = 8 + data.get(8..data.len().min(8 + 128))?.iter().position(|&c| c == 0)?;
    (bone_end + 3 <= data.len()).then(|| rd_u16(data, bone_end + 1))
}

pub fn parse_info(data: &[u8]) -> Option<EntityInfo> {
    if data.len() < 13 {
        return None;
    }
    let class = rd_u32(data, 0);
    let static_key = rd_u32(data, 4);
    let bone_end = 8 + data[8..data.len().min(8 + 128)].iter().position(|&c| c == 0)?;
    let attach_bone = String::from_utf8_lossy(&data[8..bone_end]).into_owned();
    let id_off = bone_end + 1;
    if id_off + 4 > data.len() {
        return None;
    }
    let id = rd_u16(data, id_off);
    let parent = rd_u16(data, id_off + 2);

    let mut base = EntityInfo {
        class,
        static_key,
        attach_bone,
        id,
        parent,
        name: String::new(),
        name_offset: 0,
        flags: 0,
        transform_offset: None,
        position: None,
    };
    // The name follows a variable amount of class data; accept the first
    // string that is followed by flags and a sane transform.
    let mut from = id_off + 4;
    while let Some((off, name)) = next_string(data, from) {
        let after = off + name.len() + 1;
        let t = after + 2;
        if t + 48 <= data.len() && plausible_transform(data, t) {
            let pos = [rd_f32(data, t + 36), rd_f32(data, t + 40), rd_f32(data, t + 44)];
            // Bytes of a preceding float can look like text ("h=>eff_x").
            let cut = name
                .char_indices()
                .filter(|(_, c)| !(c.is_ascii_alphanumeric() || "_@.-$?\\ ".contains(*c)))
                .map(|(i, c)| i + c.len_utf8())
                .last()
                .unwrap_or(0);
            base.name = name[cut..].to_string();
            base.name_offset = off + cut;
            base.flags = rd_u16(data, after);
            base.transform_offset = Some(t);
            base.position = Some(pos);
            return Some(base);
        }
        from = after;
    }
    // No transform found: fall back to the first string.
    if let Some((off, name)) = next_string(data, id_off + 4) {
        base.name = name;
        base.name_offset = off;
    }
    Some(base)
}

fn plausible_transform(data: &[u8], t: usize) -> bool {
    (0..9).all(|i| {
        let v = rd_f32(data, t + i * 4);
        v.is_finite() && v.abs() <= 1.5
    }) && (9..12).all(|i| {
        let v = rd_f32(data, t + i * 4);
        v.is_finite() && v.abs() < 1.0e6
    })
}

/// Next NUL-terminated string of at least 2 printable characters.
fn next_string(data: &[u8], from: usize) -> Option<(usize, String)> {
    let mut run_start = None;
    for i in from..data.len() {
        let c = data[i];
        if is_printable(c) {
            run_start.get_or_insert(i);
        } else {
            if let Some(s) = run_start.take() {
                if c == 0 && i - s >= 2 {
                    return Some((s, String::from_utf8_lossy(&data[s..i]).into_owned()));
                }
            }
        }
    }
    None
}
