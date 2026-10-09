//! Metro 2033 Redux save game library.
//!
//! Layers, outside in:
//! * [`container`]: `DEZP` file, LZ4 chunks, CRC32
//! * [`sections`]: entity slots (`dynamic`) and runtime records (`active`)
//! * [`entity`]: common entity header (class, name, transform)
//! * [`script`]: visual scripts (blocks, links, per-block runtime state)
//! * [`save`]: everything together plus editing operations

pub mod container;
pub mod entity;
pub mod names;
pub mod save;
pub mod script;
pub mod sections;
pub mod util;

pub use save::{Save, SectionKind};
