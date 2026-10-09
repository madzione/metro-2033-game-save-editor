//! Visual scripts ("VS"): the level logic of the 4A engine.
//!
//! The *definition* of a script lives in the entity record (`dynamic`
//! section), either as a `VISUALSCRIPT` entity or embedded in another entity
//! (doors, traps, ...):
//!
//! ```text
//! 14 00 ff ff ff ff          marker
//! u32 block_count
//! block_count x { u32 class (crc32 of "triggers/use", "logic/delay", ...), u32 0, params }
//! u32 link_count
//! link_count x { u16 from_block, u16 out_pin, u16 to_block, u16 in_pin }
//! ```
//!
//! The *runtime state* of every block is stored in the entity's record in the
//! `active` section: `u32 block_count` followed by one state per block. Each
//! block type has a fixed state size, except entity references which store a
//! list of ids (`u32 count, u16 ids[count]`).
//!
//! Semantics (Exodus SDK docs + verified against saves):
//! * triggers: param byte 0 = "active", state byte 0 = currently active;
//!   input pin 0 = activate, pin 1 = deactivate.
//! * logic/delay: params `u32 min_ms, u32 max_ms, u8 flags` (0x02 = active);
//!   state `u8 active, i32 timer` (-2 = idle).
//! * logic/locker ("heavy"): inputs lock(0), unlock(1), in(2).
//! * logic/locker_base ("easy"): flag 0x08 = auto lock, state byte 0 = locked.

use std::ops::Range;

use crate::names::Names;
use crate::util::{cstrings, get_u16, get_u32, rd_f32, rd_i32, rd_u16, rd_u32};

pub const MARKER: [u8; 6] = [0x14, 0x00, 0xff, 0xff, 0xff, 0xff];
pub const ENTITY_REF: &str = "entities/entity ref";
const MAX_REFS: u32 = 64;

#[derive(Clone, Debug)]
pub struct Block {
    pub class: u32,
    pub kind: String,
    /// Offset of the class hash inside the entity record.
    pub start: usize,
    /// Parameter bytes inside the entity record.
    pub params: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    pub from: u16,
    pub out_pin: u16,
    pub to: u16,
    pub in_pin: u16,
}

#[derive(Clone, Debug)]
pub struct Script {
    pub blocks: Vec<Block>,
    pub links: Vec<Link>,
    pub links_offset: usize,
    /// `false` if the end of the script could only be guessed.
    pub exact: bool,
}

/// Size of the runtime state of a block type, `None` if unknown.
/// Entity references are variable (see [`ENTITY_REF`]).
pub fn state_size(kind: &str) -> Option<usize> {
    Some(match kind {
        "actions/engine/play_hud_particles" | "actions/engine/presence" | "actions/engine/quick_save"
        | "actions/engine/set environment" | "actions/action_set_interest" | "actions/action_set_interest_ex"
        | "actions/attach" | "actions/destroy" | "actions/detach" | "actions/engine/make_ghost" | "actions/explode"
        | "actions/freegun" | "actions/goto_target" | "actions/hit" | "actions/make immortal"
        | "actions/min mental state" | "actions/monster/watchman_get_reinforcements" | "actions/move"
        | "actions/movement type" | "actions/npc/anim_speed" | "actions/npc/cover group"
        | "actions/npc/obstacle" | "actions/npc/reload" | "actions/one shot" | "actions/p-force"
        | "actions/play camera-effect" | "actions/play color cube" | "actions/play coloranim"
        | "actions/play cover path" | "actions/play motion" | "actions/play motion ex"
        | "actions/play particles" | "actions/play patrol path" | "actions/play sound"
        | "actions/player/restrictor_obstacle" | "actions/player/way_point" | "actions/set enemy"
        | "actions/static_combat" | "actions/static_combat_anchor" | "actions/turn light"
        | "actions/update ik" | "fun/text splash" | "logic/and-2" | "logic/select-3" | "logic/switch-2"
        | "logic/switch_param" | "checker/npc/enemy" => 1,
        "actions/engine/set background music" | "actions/engine/signal" | "actions/entity/useful for player"
        | "actions/wear gasmask" | "covers/ref" | "logic/random" | "logic/random-2" | "logic/random-3"
        | "logic/random-4" | "logic/random-chooser" | "logic/switch-3" | "logic/adv/randomex"
        | "triggers/entity/orientation" | "entities/entity self" => 0,
        "actions/activate-deactivate" | "trade/take item" | "triggers/anim event" | "triggers/engine/signal"
        | "triggers/object_is_close" | "triggers/on use" | "triggers/pass cover" | "triggers/startgame" => 2,
        "triggers/die" | "triggers/hit" | "triggers/npc enemy is close" | "triggers/npc mental state"
        | "triggers/watchman group" | "triggers/zombie" => 3,
        "logic/locker" | "logic/locker_base" | "triggers/use" => 4,
        "logic/delay" => 5,
        "entities/entity player" | "triggers/generic" | "triggers/npc" | "triggers/object" | "triggers/player"
        | "triggers/player_special" => 6,
        "entities/entity level player" => 7,
        "actions/engine/clone" | "logic/counter" | "logic/counter ref" => 8,
        _ => return None,
    })
}

pub fn find_marker(data: &[u8]) -> Option<usize> {
    data.windows(MARKER.len()).position(|w| w == MARKER)
}

pub fn parse_script(data: &[u8], names: &Names) -> Option<Script> {
    let m = find_marker(data)?;
    let count = get_u32(data, m + 6)? as usize;
    if count > 10_000 {
        return None;
    }
    let first = m + 10;

    let mut starts = Vec::with_capacity(count);
    let mut o = first;
    while starts.len() < count && o + 8 <= data.len() {
        let h = rd_u32(data, o);
        if names.is_block(h) && rd_u32(data, o + 4) == 0 {
            starts.push(o);
            o += 8;
        } else {
            o += 1;
        }
    }
    if starts.len() != count || (count > 0 && starts[0] != first) {
        return None;
    }

    // Link table: right after the last block's parameters. Visual script
    // entities end with 3 trailer bytes, which pins down the exact position.
    let search_from = starts.last().map_or(first, |s| s + 8);
    let valid = |lo: usize| -> Option<Vec<Link>> {
        let cnt = get_u32(data, lo)? as usize;
        if cnt > 20_000 || lo + 4 + cnt * 8 > data.len() {
            return None;
        }
        let links: Vec<Link> = (0..cnt)
            .map(|i| {
                let b = lo + 4 + i * 8;
                Link { from: rd_u16(data, b), out_pin: rd_u16(data, b + 2), to: rd_u16(data, b + 4), in_pin: rd_u16(data, b + 6) }
            })
            .collect();
        let ok = links.iter().all(|l| (l.from as usize) < count && (l.to as usize) < count && l.out_pin < 64 && l.in_pin < 64);
        ok.then_some(links)
    };
    let mut found = None;
    for lo in search_from..data.len().saturating_sub(3) {
        let cnt = rd_u32(data, lo) as usize;
        if lo + 4 + cnt * 8 == data.len() - 3 {
            if let Some(l) = valid(lo) {
                found = Some((lo, l, true));
                break;
            }
        }
    }
    if found.is_none() {
        for lo in search_from..data.len().saturating_sub(4) {
            if get_u32(data, lo).is_some_and(|c| c > 0 || count == 0) {
                if let Some(l) = valid(lo) {
                    found = Some((lo, l, false));
                    break;
                }
            }
        }
    }
    let (links_offset, links, exact) = found?;

    let blocks = starts
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let end = starts.get(i + 1).copied().unwrap_or(links_offset);
            let class = rd_u32(data, s);
            Block { class, kind: names.name(class), start: s, params: s + 8..end }
        })
        .collect();
    Some(Script { blocks, links, links_offset, exact })
}

/// Where the state of every block lives inside an `active` record.
#[derive(Clone, Debug)]
pub struct StateLayout {
    pub count_offset: usize,
    pub ranges: Vec<Range<usize>>,
}

pub fn map_states(script: &Script, entity: &[u8], active: &[u8]) -> Option<StateLayout> {
    let n = script.blocks.len() as u32;
    let mut best = None;
    for start in 0..active.len().saturating_sub(3) {
        if rd_u32(active, start) != n {
            continue;
        }
        if let Some(ranges) = try_layout(script, entity, active, start + 4) {
            let end = ranges.last().map_or(start + 4, |r| r.end);
            if end == active.len() {
                return Some(StateLayout { count_offset: start, ranges });
            }
            best.get_or_insert(StateLayout { count_offset: start, ranges });
        }
    }
    best
}

fn try_layout(script: &Script, entity: &[u8], active: &[u8], mut o: usize) -> Option<Vec<Range<usize>>> {
    let mut ranges = Vec::with_capacity(script.blocks.len());
    for b in &script.blocks {
        let size = if b.kind == ENTITY_REF {
            let cnt = get_u32(active, o)?;
            if cnt > MAX_REFS {
                return None;
            }
            let param_id = get_u16(entity, b.params.start)?;
            if cnt >= 1 && param_id != 0xffff && get_u16(active, o + 4)? != param_id {
                return None;
            }
            4 + 2 * cnt as usize
        } else {
            state_size(&b.kind)?
        };
        if o + size > active.len() {
            return None;
        }
        ranges.push(o..o + size);
        o += size;
    }
    Some(ranges)
}

/// Short human readable description of a block's parameters.
pub fn describe(block: &Block, entity: &[u8], names: &Names, entity_name: &dyn Fn(u16) -> Option<String>) -> String {
    let p = &entity[block.params.clone()];
    let kind = block.kind.as_str();
    let strings = || cstrings(p, 2).into_iter().map(|(_, s)| s).collect::<Vec<_>>().join(" | ");
    match kind {
        ENTITY_REF | "entities/entity player" if p.len() >= 2 => {
            let id = rd_u16(p, 0);
            if id == 0xffff {
                "(none)".into()
            } else {
                format!("#{id:#06x} {}", entity_name(id).unwrap_or_default())
            }
        }
        "logic/delay" if p.len() >= 9 => {
            let flags = p[8];
            format!(
                "{} - {} ms{}",
                rd_u32(p, 0),
                rd_u32(p, 4),
                if flags & 2 == 0 { "  [inactive]" } else { "" }
            )
        }
        "logic/locker" | "logic/locker_base" if !p.is_empty() => format!("flags {:#04x}", p[0]),
        k if k.starts_with("triggers/") && !p.is_empty() => {
            // triggers/use: u8 active, u8 ?, f32 distance, char hint[]
            let s = if k == "triggers/use" && p.len() > 6 {
                cstrings(&p[6..], 1).into_iter().next().map(|(_, s)| s).unwrap_or_default()
            } else {
                strings()
            };
            let mut d = (if p[0] != 0 { "active" } else { "inactive" }).to_string();
            if k == "triggers/use" && p.len() >= 6 {
                d.push_str(&format!(" dist {:.1}", rd_f32(p, 2)));
            }
            if !s.is_empty() {
                d.push_str(&format!(" \"{s}\""));
            }
            d
        }
        _ => {
            let s = strings();
            if s.is_empty() && p.len() >= 4 && names.get(rd_u32(p, 0)).is_some() {
                names.name(rd_u32(p, 0))
            } else {
                s
            }
        }
    }
}

/// Short human readable description of a block's runtime state.
pub fn describe_state(block: &Block, state: &[u8]) -> String {
    let k = block.kind.as_str();
    match k {
        "logic/delay" if state.len() == 5 => {
            let t = rd_i32(state, 1);
            format!(
                "{}{}",
                if state[0] != 0 { "active" } else { "inactive" },
                if t == -2 { ", idle".to_string() } else { format!(", timer {t}") }
            )
        }
        ENTITY_REF if state.len() >= 4 => {
            let n = rd_u32(state, 0) as usize;
            let ids: Vec<String> = (0..n).filter_map(|i| get_u16(state, 4 + 2 * i)).map(|v| format!("{v:#06x}")).collect();
            ids.join(",")
        }
        _ if k.starts_with("triggers/") && !state.is_empty() => {
            if state[0] != 0 { "active".into() } else { "inactive".into() }
        }
        "logic/locker_base" if !state.is_empty() => {
            if state[0] != 0 { "locked".into() } else { "open".into() }
        }
        _ if k.starts_with("actions/") && state.len() == 1 => {
            if state[0] != 0 { "running/done".into() } else { "-".into() }
        }
        _ => String::new(),
    }
}
