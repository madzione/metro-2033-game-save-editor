//! A fully decoded save game plus the editing operations.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

use crate::container::Container;
use crate::entity::{parse_info, runtime_id, EntityInfo};
use crate::names::{crc, Names};
use crate::script::{map_states, parse_script, Script, StateLayout};
use crate::sections::{parse_active, parse_dynamic, write_active, write_dynamic, ActiveRec, ActiveSub, DynSub};
use crate::util::{cstrings, parse_num, rd_u32};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    Header,
    Dynamic,
    Active,
}

impl SectionKind {
    pub fn name(self) -> &'static str {
        match self {
            SectionKind::Header => "header",
            SectionKind::Dynamic => "dynamic",
            SectionKind::Active => "active",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "0" | "header" => SectionKind::Header,
            "1" | "dynamic" => SectionKind::Dynamic,
            "2" | "active" => SectionKind::Active,
            _ => bail!("unknown section '{s}' (use header, dynamic or active)"),
        })
    }
}

#[derive(Clone)]
pub struct Save {
    pub container: Container,
    pub dynamic: Vec<DynSub>,
    pub active: Vec<ActiveSub>,
    pub names: Names,
    dyn_index: HashMap<u16, (usize, usize)>,
    act_index: HashMap<u16, (usize, usize)>,
}

/// Script definition + where its runtime state lives.
pub struct ScriptView {
    pub script: Script,
    pub states: Option<StateLayout>,
}

impl Save {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let container = Container::parse(bytes)?;
        let dynamic = parse_dynamic(&container.sections[section_index(&container, SectionKind::Dynamic)?].data)
            .context("parsing dynamic section")?;
        let active = parse_active(&container.sections[section_index(&container, SectionKind::Active)?].data)
            .context("parsing active section")?;
        let mut names = Names::builtin();
        for s in &container.sections {
            names.learn_from(&s.data);
        }
        let mut save = Self { container, dynamic, active, names, dyn_index: HashMap::new(), act_index: HashMap::new() };
        save.reindex();
        Ok(save)
    }

    fn reindex(&mut self) {
        self.dyn_index.clear();
        for (si, s) in self.dynamic.iter().enumerate() {
            for (ei, slot) in s.slots.iter().enumerate() {
                if let Some(d) = &slot.data {
                    let id = runtime_id(d).unwrap_or(slot.id);
                    self.dyn_index.entry(id).or_insert((si, ei));
                }
            }
        }
        self.act_index.clear();
        for (si, s) in self.active.iter().enumerate() {
            for (ri, r) in s.recs.iter().enumerate() {
                self.act_index.entry(r.id).or_insert((si, ri));
            }
        }
    }

    /// Serialize the (possibly modified) save.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut c = self.container.clone();
        if let Ok(i) = section_index(&c, SectionKind::Dynamic) {
            c.sections[i].data = write_dynamic(&self.dynamic);
        }
        if let Ok(i) = section_index(&c, SectionKind::Active) {
            c.sections[i].data = write_active(&self.active);
        }
        c.to_bytes()
    }

    /// Write to `path`, then load the result again to make sure it is valid.
    pub fn write(&self, path: &Path) -> Result<()> {
        let bytes = self.to_bytes();
        let check = Save::from_bytes(&bytes).context("internal error: written save does not parse")?;
        ensure!(check.dyn_index.len() == self.dyn_index.len(), "internal error: entity count changed");
        std::fs::write(path, &bytes).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    pub fn section(&self, kind: SectionKind) -> &[u8] {
        match section_index(&self.container, kind) {
            Ok(i) => &self.container.sections[i].data,
            Err(_) => &[],
        }
    }

    /// Level name and version strings from the `header` section.
    pub fn header_strings(&self) -> Vec<String> {
        let clean = |s: &str| {
            s.len() >= 4 && s.chars().all(|c| c.is_ascii_alphanumeric() || "_\\#.-".contains(c))
        };
        cstrings(self.section(SectionKind::Header), 4)
            .into_iter()
            .map(|(_, s)| s)
            .take_while(|s| clean(s))
            .collect()
    }

    pub fn level_name(&self) -> String {
        self.header_strings().into_iter().next().unwrap_or_default()
    }

    pub fn entity_ids(&self) -> Vec<u16> {
        let mut ids: Vec<(usize, usize, u16)> = self.dyn_index.iter().map(|(&id, &(s, e))| (s, e, id)).collect();
        ids.sort_unstable();
        ids.into_iter().map(|(_, _, id)| id).collect()
    }

    pub fn entity(&self, id: u16) -> Option<&[u8]> {
        let &(s, e) = self.dyn_index.get(&id)?;
        self.dynamic[s].slots[e].data.as_deref()
    }

    pub fn entity_mut(&mut self, id: u16) -> Option<&mut Vec<u8>> {
        let &(s, e) = self.dyn_index.get(&id)?;
        self.dynamic[s].slots[e].data.as_mut()
    }

    pub fn info(&self, id: u16) -> Option<EntityInfo> {
        parse_info(self.entity(id)?)
    }

    pub fn entity_name(&self, id: u16) -> Option<String> {
        self.info(id).map(|i| i.name)
    }

    pub fn active_rec(&self, id: u16) -> Option<&ActiveRec> {
        let &(s, r) = self.act_index.get(&id)?;
        Some(&self.active[s].recs[r])
    }

    pub fn active_rec_mut(&mut self, id: u16) -> Option<&mut ActiveRec> {
        let &(s, r) = self.act_index.get(&id)?;
        Some(&mut self.active[s].recs[r])
    }

    /// Accepts `#123`, `0x7b`, a plain number or an entity name.
    pub fn find(&self, query: &str) -> Result<u16> {
        let q = query.trim();
        let as_id = q.strip_prefix('#').map(parse_num).or_else(|| {
            (q.starts_with("0x") || q.chars().all(|c| c.is_ascii_digit())).then(|| parse_num(q))
        });
        if let Some(Ok(id)) = as_id {
            let id = u16::try_from(id).context("id out of range")?;
            ensure!(self.entity(id).is_some(), "no entity with id {id:#06x}");
            return Ok(id);
        }
        let matches: Vec<u16> =
            self.entity_ids().into_iter().filter(|&id| self.entity_name(id).as_deref() == Some(q)).collect();
        match matches.as_slice() {
            [id] => Ok(*id),
            [] => bail!("no entity named '{q}'"),
            many => bail!("'{q}' is ambiguous, ids: {many:x?}"),
        }
    }

    pub fn script(&self, id: u16) -> Option<ScriptView> {
        let data = self.entity(id)?;
        let script = parse_script(data, &self.names)?;
        let states = self.active_rec(id).and_then(|r| map_states(&script, data, &r.data));
        Some(ScriptView { script, states })
    }

    pub fn is_visual_script(&self, id: u16) -> bool {
        self.entity(id).is_some_and(|d| d.len() >= 4 && rd_u32(d, 0) == crc("VISUALSCRIPT"))
    }

    // ---------------------------------------------------------------- edits

    pub fn set_block_state(&mut self, id: u16, block: usize, bytes: &[u8]) -> Result<()> {
        let view = self.script(id).context("entity has no script")?;
        let states = view.states.context("runtime state of this script not found")?;
        let range = states.ranges.get(block).context("block index out of range")?.clone();
        ensure!(
            range.len() == bytes.len(),
            "state of block {block} is {} bytes, got {}",
            range.len(),
            bytes.len()
        );
        self.active_rec_mut(id).unwrap().data[range].copy_from_slice(bytes);
        Ok(())
    }

    pub fn set_block_param(&mut self, id: u16, block: usize, offset: usize, bytes: &[u8]) -> Result<()> {
        let view = self.script(id).context("entity has no script")?;
        let b = view.script.blocks.get(block).context("block index out of range")?;
        let start = b.params.start + offset;
        ensure!(
            start + bytes.len() <= b.params.end,
            "write exceeds the {} parameter bytes of block {block}",
            b.params.len()
        );
        self.entity_mut(id).unwrap()[start..start + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }

    pub fn block_state(&self, id: u16, block: usize) -> Result<Vec<u8>> {
        let view = self.script(id).context("entity has no script")?;
        let states = view.states.context("runtime state of this script not found")?;
        let range = states.ranges.get(block).context("block index out of range")?.clone();
        Ok(self.active_rec(id).unwrap().data[range].to_vec())
    }

    fn block_kind(&self, id: u16, block: usize) -> Result<String> {
        let view = self.script(id).context("entity has no script")?;
        Ok(view.script.blocks.get(block).context("block index out of range")?.kind.clone())
    }

    /// Switch a trigger block on or off (both its "active" property and its runtime flag).
    pub fn set_trigger_active(&mut self, id: u16, block: usize, active: bool) -> Result<()> {
        let kind = self.block_kind(id, block)?;
        ensure!(kind.starts_with("triggers/"), "block {block} is {kind}, not a trigger");
        self.set_block_param(id, block, 0, &[active as u8])?;
        let mut st = self.block_state(id, block)?;
        ensure!(!st.is_empty(), "trigger has no runtime state");
        st[0] = active as u8;
        self.set_block_state(id, block, &st)
    }

    /// Make a delay block active and start its timer so it fires right after
    /// the save is loaded (as if its input had just been triggered).
    pub fn arm_delay(&mut self, id: u16, block: usize, timer: i32) -> Result<()> {
        let kind = self.block_kind(id, block)?;
        ensure!(kind == "logic/delay", "block {block} is {kind}, not logic/delay");
        let view = self.script(id).unwrap();
        let p = view.script.blocks[block].params.clone();
        ensure!(p.len() >= 9, "unexpected delay parameters");
        let flags = self.entity(id).unwrap()[p.start + 8] | 0x02;
        self.set_block_param(id, block, 8, &[flags])?;
        let mut st = vec![1u8];
        st.extend_from_slice(&timer.to_le_bytes());
        self.set_block_state(id, block, &st)
    }

    /// Move an entity. NPCs keep a second copy of their position in the runtime
    /// record (same x and z, y may differ slightly); that copy is moved too.
    pub fn set_position(&mut self, id: u16, pos: [f32; 3]) -> Result<()> {
        let info = self.info(id).context("unknown entity")?;
        let t = info.transform_offset.context("entity has no transform")?;
        let data = self.entity_mut(id).unwrap();
        let old_x: [u8; 4] = data[t + 36..t + 40].try_into().unwrap();
        let old_z: [u8; 4] = data[t + 44..t + 48].try_into().unwrap();
        let new: Vec<u8> = pos.iter().flat_map(|v| v.to_le_bytes()).collect();
        data[t + 36..t + 48].copy_from_slice(&new);
        if let Some(rec) = self.active_rec_mut(id) {
            let d = &mut rec.data;
            let mut i = 0;
            while i + 12 <= d.len() {
                if d[i..i + 4] == old_x && d[i + 8..i + 12] == old_z {
                    d[i..i + 12].copy_from_slice(&new);
                    i += 12;
                } else {
                    i += 1;
                }
            }
        }
        Ok(())
    }

    /// Overwrite raw bytes of a decompressed section (same length). The
    /// result is re-parsed, so structural damage is rejected.
    pub fn patch_section(&mut self, kind: SectionKind, offset: usize, bytes: &[u8]) -> Result<()> {
        let mut data = match kind {
            SectionKind::Header => self.section(kind).to_vec(),
            SectionKind::Dynamic => write_dynamic(&self.dynamic),
            SectionKind::Active => write_active(&self.active),
        };
        ensure!(offset + bytes.len() <= data.len(), "patch beyond end of section ({} bytes)", data.len());
        data[offset..offset + bytes.len()].copy_from_slice(bytes);
        match kind {
            SectionKind::Header => {
                let i = section_index(&self.container, kind)?;
                self.container.sections[i].data = data;
            }
            SectionKind::Dynamic => self.dynamic = parse_dynamic(&data).context("patch breaks section structure")?,
            SectionKind::Active => self.active = parse_active(&data).context("patch breaks section structure")?,
        }
        self.reindex();
        Ok(())
    }

    /// Replace a whole decompressed section (used by `pack`).
    pub fn replace_section(&mut self, kind: SectionKind, data: Vec<u8>) -> Result<()> {
        match kind {
            SectionKind::Header => {}
            SectionKind::Dynamic => self.dynamic = parse_dynamic(&data)?,
            SectionKind::Active => self.active = parse_active(&data)?,
        }
        let i = section_index(&self.container, kind)?;
        self.container.sections[i].data = data;
        self.reindex();
        Ok(())
    }
}

pub fn section_index(c: &Container, kind: SectionKind) -> Result<usize> {
    let id = crc(kind.name());
    c.sections
        .iter()
        .position(|s| s.id == id)
        .or(match kind {
            SectionKind::Header => Some(0),
            SectionKind::Dynamic => Some(1),
            SectionKind::Active => Some(2),
        })
        .filter(|&i| i < c.sections.len())
        .with_context(|| format!("section {} not found", kind.name()))
}
