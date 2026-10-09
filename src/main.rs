use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use clap::{Parser, Subcommand};

use metro_save::container::{Container, Section};
use metro_save::names::{extract_strings, Names};
use metro_save::script::{describe, describe_state};
use metro_save::util::{hex, hexdump, parse_hex, parse_num};
use metro_save::{Save, SectionKind};

#[cfg(feature = "gui")]
mod gui;

#[derive(Parser)]
#[command(name = "metro-2033-redux-save-editor", version, about = "Inspect, edit and repack Metro 2033 Redux save games.\nRun without a command to start the GUI.")]
struct Cli {
    /// Extra name dictionary (one name per line, see `dict`). `metro_names.txt`
    /// next to the executable or in the current directory is loaded automatically.
    #[arg(long, global = true)]
    names: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(clap::Args)]
struct Out {
    /// Output file. Without it the input is modified in place (a .bak copy is made first).
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(clap::Args)]
struct BlockRef {
    /// Script entity (name, #id or 0xid)
    #[arg(short, long)]
    script: String,
    /// Block index inside the script
    #[arg(short, long)]
    block: usize,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open the graphical editor
    Gui {
        save: Option<PathBuf>,
        /// Entity to show initially (name, #id or 0xid)
        #[arg(long)]
        select: Option<String>,
    },
    /// Show container and level information
    Info { save: PathBuf },
    /// Check the save can be decoded and re-encoded without changes
    Verify { save: PathBuf },
    /// Write the decompressed sections to a directory
    Unpack { save: PathBuf, dir: PathBuf },
    /// Build a save from an unpacked directory
    Pack { dir: PathBuf, output: PathBuf },
    /// List entities
    List {
        save: PathBuf,
        /// Filter by class (e.g. VISUALSCRIPT, O_ENTITY)
        #[arg(long)]
        class: Option<String>,
        /// Filter by name substring
        #[arg(long)]
        name: Option<String>,
        /// Only entities near "x,y,z" (or an entity name)
        #[arg(long)]
        near: Option<String>,
        #[arg(long, default_value_t = 10.0)]
        radius: f32,
    },
    /// Show one entity (header, script, raw data)
    Entity {
        save: PathBuf,
        entity: String,
        /// Include hex dumps of the entity and runtime records
        #[arg(long)]
        hex: bool,
    },
    /// Show a visual script: blocks, parameters, runtime state, links
    Script {
        save: PathBuf,
        entity: String,
        /// Print the link table
        #[arg(long)]
        links: bool,
    },
    /// Activate or deactivate a trigger block
    SetTrigger {
        save: PathBuf,
        #[command(flatten)]
        at: BlockRef,
        #[arg(long, action = clap::ArgAction::Set)]
        active: bool,
        #[command(flatten)]
        out: Out,
    },
    /// Activate a delay block and start its timer so it fires after loading
    ArmDelay {
        save: PathBuf,
        #[command(flatten)]
        at: BlockRef,
        /// Value written to the timer field
        #[arg(long, default_value_t = 100)]
        timer: i32,
        #[command(flatten)]
        out: Out,
    },
    /// Overwrite the runtime state bytes of a block
    SetState {
        save: PathBuf,
        #[command(flatten)]
        at: BlockRef,
        #[arg(long)]
        hex: String,
        #[command(flatten)]
        out: Out,
    },
    /// Overwrite parameter bytes of a block
    SetParam {
        save: PathBuf,
        #[command(flatten)]
        at: BlockRef,
        #[arg(long, default_value = "0")]
        offset: String,
        #[arg(long)]
        hex: String,
        #[command(flatten)]
        out: Out,
    },
    /// Move an entity
    #[command(allow_negative_numbers = true)]
    SetPos {
        save: PathBuf,
        entity: String,
        x: f32,
        y: f32,
        z: f32,
        #[command(flatten)]
        out: Out,
    },
    /// Overwrite raw bytes in a decompressed section
    Patch {
        save: PathBuf,
        /// header, dynamic or active
        #[arg(long)]
        section: String,
        #[arg(long)]
        offset: String,
        #[arg(long)]
        hex: String,
        #[command(flatten)]
        out: Out,
    },
    /// Extract a name dictionary from the game executable (improves name lookup)
    Dict { binary: PathBuf, output: PathBuf },
}

fn main() -> Result<()> {
    // Behave like other CLI tools when the output is piped into `head` etc.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    let extra_names = cli.names.clone();
    match cli.cmd {
        None => run_gui(None, None, extra_names),
        Some(Cmd::Gui { save, select }) => run_gui(save, select, extra_names),
        Some(cmd) => run(cmd, extra_names.as_deref()),
    }
}

#[cfg(feature = "gui")]
fn run_gui(save: Option<PathBuf>, select: Option<String>, names: Option<PathBuf>) -> Result<()> {
    gui::run(save, select, names)
}

#[cfg(not(feature = "gui"))]
fn run_gui(_: Option<PathBuf>, _: Option<String>, _: Option<PathBuf>) -> Result<()> {
    bail!("built without the `gui` feature; use one of the commands (see --help)")
}

pub fn load_save(path: &Path, extra_names: Option<&Path>) -> Result<Save> {
    let mut save = Save::load(path)?;
    load_extra_names(&mut save.names, extra_names);
    Ok(save)
}

pub fn load_extra_names(names: &mut Names, extra: Option<&Path>) {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = extra {
        candidates.push(p.to_path_buf());
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        candidates.push(dir.join("metro_names.txt"));
    }
    candidates.push(PathBuf::from("metro_names.txt"));
    for c in candidates {
        if c.is_file() {
            if let Err(e) = names.load_file(&c) {
                eprintln!("warning: could not read {}: {e}", c.display());
            }
        }
    }
}

/// Write the modified save; without an explicit output the input is replaced
/// after making a backup copy.
pub fn write_save(save: &Save, input: &Path, output: Option<&Path>) -> Result<PathBuf> {
    let target = match output {
        Some(o) => o.to_path_buf(),
        None => {
            let backup = backup_path(input);
            std::fs::copy(input, &backup).with_context(|| format!("creating backup {}", backup.display()))?;
            eprintln!("backup: {}", backup.display());
            input.to_path_buf()
        }
    };
    save.write(&target)?;
    eprintln!("written: {}", target.display());
    Ok(target)
}

pub fn backup_path(input: &Path) -> PathBuf {
    let base = input.as_os_str().to_owned();
    let mut n = 0;
    loop {
        let mut p = base.clone();
        p.push(if n == 0 { ".bak".to_string() } else { format!(".bak{n}") });
        let p = PathBuf::from(p);
        if !p.exists() {
            return p;
        }
        n += 1;
    }
}

fn run(cmd: Cmd, extra_names: Option<&Path>) -> Result<()> {
    match cmd {
        Cmd::Gui { .. } => unreachable!(),
        Cmd::Info { save: path } => {
            let bytes = std::fs::read(&path)?;
            let save = load_save(&path, extra_names)?;
            println!("file      {} ({} bytes)", path.display(), bytes.len());
            println!("level     {}", save.level_name());
            println!("header    {}", save.header_strings().join(" | "));
            for s in &save.container.sections {
                println!(
                    "section   {:8} {:>9} bytes, {:>8} compressed, {} chunks",
                    save.names.name(s.id),
                    s.data.len(),
                    s.stored_compressed_size,
                    s.chunk_sizes.len()
                );
            }
            let ids = save.entity_ids();
            let scripts = ids.iter().filter(|&&id| save.is_visual_script(id)).count();
            println!("entities  {} ({} visual scripts)", ids.len(), scripts);
            println!("runtime   {} records", save.active.iter().map(|s| s.recs.len()).sum::<usize>());
            if let Some(p) = save.info(0).and_then(|i| i.position) {
                println!("player    at {:.2} {:.2} {:.2}", p[0], p[1], p[2]);
            }
        }
        Cmd::Verify { save: path } => {
            let bytes = std::fs::read(&path)?;
            let save = Save::from_bytes(&bytes)?;
            let again = Save::from_bytes(&save.to_bytes())?;
            for (a, b) in save.container.sections.iter().zip(&again.container.sections) {
                ensure!(a.data == b.data, "section {:#x} differs after re-encoding", a.id);
            }
            let orig = Container::parse(&bytes)?;
            for (a, s) in orig.sections.iter().zip(&save.container.sections) {
                ensure!(a.data == s.data);
            }
            println!("OK: checksum valid, all sections decode, re-encoding is lossless");
        }
        Cmd::Unpack { save: path, dir } => {
            let save = load_save(&path, extra_names)?;
            std::fs::create_dir_all(&dir)?;
            let mut layout = String::new();
            for s in &save.container.sections {
                let name = save.names.name(s.id);
                std::fs::write(dir.join(format!("{name}.bin")), &s.data)?;
                let sizes: Vec<String> = s.chunk_sizes.iter().map(|c| c.to_string()).collect();
                layout.push_str(&format!("{name} {:#010x} {}\n", s.id, sizes.join(",")));
            }
            std::fs::write(dir.join("layout.txt"), layout)?;
            println!("unpacked {} sections to {}", save.container.sections.len(), dir.display());
        }
        Cmd::Pack { dir, output } => {
            let layout = std::fs::read_to_string(dir.join("layout.txt")).context("layout.txt missing")?;
            let mut sections = Vec::new();
            for line in layout.lines().filter(|l| !l.trim().is_empty()) {
                let mut parts = line.split_whitespace();
                let name = parts.next().context("bad layout.txt")?;
                let id = parse_num(parts.next().context("bad layout.txt")?)? as u32;
                let chunk_sizes = parts
                    .next()
                    .map(|c| c.split(',').filter(|x| !x.is_empty()).map(|x| x.parse()).collect::<Result<Vec<usize>, _>>())
                    .transpose()?
                    .unwrap_or_default();
                let data = std::fs::read(dir.join(format!("{name}.bin")))?;
                sections.push(Section { id, data, chunk_sizes, stored_compressed_size: 0 });
            }
            let bytes = Container { sections }.to_bytes();
            Save::from_bytes(&bytes).context("packed sections do not form a valid save")?;
            std::fs::write(&output, bytes)?;
            println!("packed {}", output.display());
        }
        Cmd::List { save: path, class, name, near, radius } => list_entities(path, class, name, near, radius, extra_names)?,
        Cmd::Entity { save: path, entity, hex: show_hex } => {
            let save = load_save(&path, extra_names)?;
            let id = save.find(&entity)?;
            print_entity(&save, id, show_hex);
        }
        Cmd::Script { save: path, entity, links } => {
            let save = load_save(&path, extra_names)?;
            let id = save.find(&entity)?;
            print_script(&save, id, links)?;
        }
        Cmd::SetTrigger { save: path, at, active, out } => {
            let mut save = load_save(&path, extra_names)?;
            let id = save.find(&at.script)?;
            save.set_trigger_active(id, at.block, active)?;
            report_block(&save, id, at.block);
            write_save(&save, &path, out.output.as_deref())?;
        }
        Cmd::ArmDelay { save: path, at, timer, out } => {
            let mut save = load_save(&path, extra_names)?;
            let id = save.find(&at.script)?;
            save.arm_delay(id, at.block, timer)?;
            report_block(&save, id, at.block);
            write_save(&save, &path, out.output.as_deref())?;
        }
        Cmd::SetState { save: path, at, hex: h, out } => {
            let mut save = load_save(&path, extra_names)?;
            let id = save.find(&at.script)?;
            save.set_block_state(id, at.block, &parse_hex(&h)?)?;
            report_block(&save, id, at.block);
            write_save(&save, &path, out.output.as_deref())?;
        }
        Cmd::SetParam { save: path, at, offset, hex: h, out } => {
            let mut save = load_save(&path, extra_names)?;
            let id = save.find(&at.script)?;
            save.set_block_param(id, at.block, parse_num(&offset)?, &parse_hex(&h)?)?;
            report_block(&save, id, at.block);
            write_save(&save, &path, out.output.as_deref())?;
        }
        Cmd::SetPos { save: path, entity, x, y, z, out } => {
            let mut save = load_save(&path, extra_names)?;
            let id = save.find(&entity)?;
            save.set_position(id, [x, y, z])?;
            write_save(&save, &path, out.output.as_deref())?;
        }
        Cmd::Patch { save: path, section, offset, hex: h, out } => {
            let mut save = load_save(&path, extra_names)?;
            save.patch_section(SectionKind::parse(&section)?, parse_num(&offset)?, &parse_hex(&h)?)?;
            write_save(&save, &path, out.output.as_deref())?;
        }
        Cmd::Dict { binary, output } => {
            let data = std::fs::read(&binary)?;
            let strings = extract_strings(&data);
            std::fs::write(&output, strings.join("\n") + "\n")?;
            println!("wrote {} names to {}", strings.len(), output.display());
        }
    }
    Ok(())
}

fn list_entities(
    path: PathBuf,
    class: Option<String>,
    name: Option<String>,
    near: Option<String>,
    radius: f32,
    extra_names: Option<&Path>,
) -> Result<()> {
    let save = load_save(&path, extra_names)?;
    let center = match &near {
        None => None,
        Some(s) => {
            let parts: Vec<&str> = s.split(',').collect();
            if parts.len() == 3 {
                Some([parts[0].trim().parse()?, parts[1].trim().parse()?, parts[2].trim().parse()?])
            } else {
                let id = save.find(s)?;
                Some(save.info(id).and_then(|i| i.position).context("entity has no position")?)
            }
        }
    };
    let name_lc = name.map(|n| n.to_lowercase());
    for id in save.entity_ids() {
        let Some(info) = save.info(id) else { continue };
        let cls = info.class_name(&save.names);
        if class.as_ref().is_some_and(|c| !cls.eq_ignore_ascii_case(c)) {
            continue;
        }
        if name_lc.as_ref().is_some_and(|n| !info.name.to_lowercase().contains(n)) {
            continue;
        }
        let mut dist = String::new();
        if let Some(c) = center {
            let Some(p) = info.position else { continue };
            let d = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt();
            if d > radius {
                continue;
            }
            dist = format!("  d={d:.1}");
        }
        let pos = info.position.map(|p| format!("{:8.2} {:8.2} {:8.2}", p[0], p[1], p[2])).unwrap_or_default();
        println!("#{id:#06x}  {cls:22} {:40} {pos}{dist}", info.name);
    }
    Ok(())
}

fn print_entity(save: &Save, id: u16, show_hex: bool) {
    let data = save.entity(id).unwrap();
    match save.info(id) {
        Some(info) => {
            println!("id        #{id:#06x}");
            println!("name      {}", info.name);
            println!("class     {}", info.class_name(&save.names));
            println!("parent    {:#06x}", info.parent);
            println!("flags     {:#06x}", info.flags);
            if let Some(p) = info.position {
                println!("position  {:.3} {:.3} {:.3}", p[0], p[1], p[2]);
            }
        }
        None => println!("id        #{id:#06x} (header not decoded)"),
    }
    println!("record    {} bytes", data.len());
    let strings: Vec<String> = metro_save::util::cstrings(data, 3).into_iter().map(|(_, s)| s).collect();
    println!("strings   {}", strings.join(" | "));
    if save.script(id).is_some() {
        println!("script    yes (see `script` command)");
    }
    match save.active_rec(id) {
        Some(r) => println!("runtime   {} bytes", r.data.len()),
        None => println!("runtime   none"),
    }
    if show_hex {
        println!("\n-- dynamic record --\n{}", hexdump(data, 0));
        if let Some(r) = save.active_rec(id) {
            println!("-- runtime record --\n{}", hexdump(&r.data, 0));
        }
    }
}

fn entity_label(save: &Save) -> impl Fn(u16) -> Option<String> + '_ {
    move |id| save.entity_name(id)
}

fn print_script(save: &Save, id: u16, links: bool) -> Result<()> {
    let data = save.entity(id).unwrap();
    let Some(view) = save.script(id) else { bail!("entity has no script") };
    let rt = save.active_rec(id).map(|r| r.data.as_slice());
    println!("{} blocks, {} links{}", view.script.blocks.len(), view.script.links.len(), if view.script.exact { "" } else { " (end guessed)" });
    if view.states.is_none() {
        println!("(runtime state not found)");
    }
    let label = entity_label(save);
    for (i, b) in view.script.blocks.iter().enumerate() {
        let desc = describe(b, data, &save.names, &label);
        let (st_hex, st_desc) = match (&view.states, rt) {
            (Some(s), Some(r)) => {
                let st = &r[s.ranges[i].clone()];
                (hex(st), describe_state(b, st))
            }
            _ => (String::new(), String::new()),
        };
        println!("{i:4}  {:36} {:50}  [{st_hex}] {st_desc}", b.kind, truncate(&desc, 50));
    }
    if links {
        println!("\nlinks (block.out_pin -> block.in_pin):");
        for l in &view.script.links {
            let short = |i: u16| {
                let b = &view.script.blocks[i as usize];
                format!("{i}:{}", b.kind.rsplit('/').next().unwrap_or(""))
            };
            println!("  {:>28}.{} -> {}.{}", short(l.from), l.out_pin, short(l.to), l.in_pin);
        }
    }
    Ok(())
}

fn report_block(save: &Save, id: u16, block: usize) {
    if let Some(view) = save.script(id) {
        let b = &view.script.blocks[block];
        let data = save.entity(id).unwrap();
        let st = save.block_state(id, block).unwrap_or_default();
        println!(
            "{} block {block} ({}): params [{}] state [{}] {}",
            save.entity_name(id).unwrap_or_default(),
            b.kind,
            hex(&data[b.params.clone()]),
            hex(&st),
            describe_state(b, &st)
        );
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n - 1).collect();
        t.push('…');
        t
    }
}
