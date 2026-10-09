//! egui front end.

use std::collections::BTreeSet;
use std::path::PathBuf;

use eframe::egui;
use metro_save::script::{describe, describe_state, Link};
use metro_save::util::{hex, hexdump, parse_hex, parse_num};
use metro_save::{Save, SectionKind};

pub fn run(path: Option<PathBuf>, select: Option<String>, names: Option<PathBuf>) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1500.0, 920.0])
            .with_title("Metro 2033 Redux game save editor"),
        ..Default::default()
    };
    eframe::run_native(
        "metro-2033-redux-save-editor",
        options,
        Box::new(move |_cc| {
            let mut app = App { extra_names: names, ..Default::default() };
            if let Some(p) = path {
                app.open(p);
            }
            if let Some(q) = select {
                match app.save.as_ref().map(|s| s.find(&q)) {
                    Some(Ok(id)) => app.select(id),
                    Some(Err(e)) => app.status = format!("{e:#}"),
                    None => {}
                }
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

#[derive(Clone)]
struct Row {
    id: u16,
    class: String,
    name: String,
    pos: Option<[f32; 3]>,
    script: bool,
}

#[derive(Default, PartialEq, Clone, Copy)]
enum Tab {
    #[default]
    Overview,
    Entity,
    Raw,
}

#[derive(Default)]
struct App {
    extra_names: Option<PathBuf>,
    path: Option<PathBuf>,
    save: Option<Save>,
    dirty: bool,
    backed_up: bool,
    status: String,

    rows: Vec<Row>,
    classes: Vec<String>,
    filter: String,
    class_filter: String,
    scripts_only: bool,
    near_player: bool,
    radius: f32,

    selected: Option<u16>,
    tab: Tab,
    pos_edit: [f32; 3],
    sel_block: Option<usize>,
    state_edit: String,
    param_edit: String,

    raw_section: Option<SectionKind>,
    raw_offset: String,
    raw_hex: String,
}

impl App {
    fn open(&mut self, path: PathBuf) {
        match Save::load(&path) {
            Ok(mut save) => {
                crate::load_extra_names(&mut save.names, self.extra_names.as_deref());
                self.status = format!("loaded {} ({})", path.display(), save.level_name());
                self.save = Some(save);
                self.path = Some(path);
                self.dirty = false;
                self.backed_up = false;
                self.selected = None;
                self.sel_block = None;
                self.radius = if self.radius == 0.0 { 15.0 } else { self.radius };
                self.rebuild_rows();
            }
            Err(e) => self.status = format!("error: {e:#}"),
        }
    }

    fn rebuild_rows(&mut self) {
        let Some(save) = &self.save else { return };
        self.rows = save
            .entity_ids()
            .into_iter()
            .map(|id| {
                let info = save.info(id);
                Row {
                    id,
                    class: info.as_ref().map(|i| i.class_name(&save.names)).unwrap_or_default(),
                    name: info.as_ref().map(|i| i.name.clone()).unwrap_or_default(),
                    pos: info.and_then(|i| i.position),
                    script: save.script(id).is_some(),
                }
            })
            .collect();
        let set: BTreeSet<String> = self.rows.iter().map(|r| r.class.clone()).collect();
        self.classes = set.into_iter().collect();
    }

    fn save_to(&mut self, target: Option<PathBuf>) {
        let Some(save) = &self.save else { return };
        let Some(path) = target.or_else(|| self.path.clone()) else { return };
        let in_place = Some(&path) == self.path.as_ref();
        if in_place && !self.backed_up && path.exists() {
            let backup = crate::backup_path(&path);
            if let Err(e) = std::fs::copy(&path, &backup) {
                self.status = format!("backup failed: {e}");
                return;
            }
            self.backed_up = true;
            self.status = format!("backup {}; ", backup.display());
        } else {
            self.status.clear();
        }
        match save.write(&path) {
            Ok(()) => {
                self.status.push_str(&format!("saved {}", path.display()));
                self.dirty = false;
                if !in_place {
                    self.path = Some(path);
                    self.backed_up = true;
                }
            }
            Err(e) => self.status = format!("save failed: {e:#}"),
        }
    }

    fn select(&mut self, id: u16) {
        self.selected = Some(id);
        self.sel_block = None;
        self.tab = Tab::Entity;
        if let Some(p) = self.save.as_ref().and_then(|s| s.info(id)).and_then(|i| i.position) {
            self.pos_edit = p;
        }
    }

    fn result(&mut self, what: &str, r: anyhow::Result<()>) {
        match r {
            Ok(()) => {
                self.dirty = true;
                self.status = format!("{what}: ok");
            }
            Err(e) => self.status = format!("{what}: {e:#}"),
        }
    }

    // ------------------------------------------------------------------ ui

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Open…").clicked() {
                if let Some(p) = rfd::FileDialog::new().set_title("Open Metro 2033 Redux save").pick_file() {
                    self.open(p);
                }
            }
            let has = self.save.is_some();
            if ui.add_enabled(has && self.dirty, egui::Button::new("Save")).on_hover_text("Overwrite the file (a .bak copy is made first)").clicked() {
                self.save_to(None);
            }
            if ui.add_enabled(has, egui::Button::new("Save as…")).clicked() {
                if let Some(p) = rfd::FileDialog::new().set_title("Save as").save_file() {
                    self.save_to(Some(p));
                }
            }
            if ui.add_enabled(has, egui::Button::new("Reload")).clicked() {
                if let Some(p) = self.path.clone() {
                    self.open(p);
                }
            }
            ui.separator();
            if let Some(p) = &self.path {
                ui.monospace(format!("{}{}", p.display(), if self.dirty { "  *modified*" } else { "" }));
            }
        });
        if !self.status.is_empty() {
            ui.label(egui::RichText::new(&self.status).weak());
        }
    }

    fn entity_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.text_edit_singleline(&mut self.filter);
        });
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("class")
                .width(170.0)
                .selected_text(if self.class_filter.is_empty() { "all classes" } else { &self.class_filter })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.class_filter, String::new(), "all classes");
                    for c in &self.classes {
                        ui.selectable_value(&mut self.class_filter, c.clone(), c);
                    }
                });
            ui.checkbox(&mut self.scripts_only, "has script");
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.near_player, "near player, radius");
            ui.add(egui::DragValue::new(&mut self.radius).range(0.5..=500.0).speed(0.5));
        });
        ui.separator();

        let player = self.rows.iter().find(|r| r.id == 0).and_then(|r| r.pos);
        let f = self.filter.to_lowercase();
        let visible: Vec<&Row> = self
            .rows
            .iter()
            .filter(|r| self.class_filter.is_empty() || r.class == self.class_filter)
            .filter(|r| !self.scripts_only || r.script)
            .filter(|r| f.is_empty() || r.name.to_lowercase().contains(&f) || format!("{:#06x}", r.id).contains(&f))
            .filter(|r| {
                !self.near_player
                    || matches!((r.pos, player), (Some(p), Some(c)) if dist(p, c) <= self.radius)
            })
            .collect();
        ui.label(format!("{} of {} entities", visible.len(), self.rows.len()));
        let mut clicked = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 18.0, visible.len(), |ui, range| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            for r in &visible[range] {
                let sel = self.selected == Some(r.id);
                let text = format!("{:#06x}  {:<18} {}", r.id, truncate(&r.class, 18), r.name);
                if ui.selectable_label(sel, egui::RichText::new(text).monospace()).clicked() {
                    clicked = Some(r.id);
                }
            }
        });
        if let Some(id) = clicked {
            self.select(id);
        }
    }

    fn overview(&mut self, ui: &mut egui::Ui) {
        let Some(save) = &self.save else { return };
        egui::Grid::new("overview").num_columns(2).striped(true).show(ui, |ui| {
            ui.label("Level");
            ui.monospace(save.level_name());
            ui.end_row();
            ui.label("Header");
            ui.label(save.header_strings().join("\n"));
            ui.end_row();
            for s in &save.container.sections {
                ui.label(format!("Section '{}'", save.names.name(s.id)));
                ui.monospace(format!("{} bytes in {} chunks", s.data.len(), s.chunk_sizes.len()));
                ui.end_row();
            }
            ui.label("Entities");
            ui.monospace(format!("{} ({} with scripts)", self.rows.len(), self.rows.iter().filter(|r| r.script).count()));
            ui.end_row();
            if let Some(p) = save.info(0).and_then(|i| i.position) {
                ui.label("Player position");
                ui.monospace(format!("{:.2} {:.2} {:.2}", p[0], p[1], p[2]));
                ui.end_row();
            }
        });
        ui.add_space(12.0);
        ui.label("Select an entity on the left. Visual scripts (class VISUALSCRIPT, or doors/traps with an embedded script) show their blocks and runtime state; triggers can be (de)activated and delays armed so they fire after loading.");
    }

    fn entity_view(&mut self, ui: &mut egui::Ui) {
        let (Some(save), Some(id)) = (&self.save, self.selected) else {
            ui.label("No entity selected.");
            return;
        };
        let Some(info) = save.info(id) else {
            ui.label("Entity header could not be decoded.");
            return;
        };
        let mut set_pos = false;
        egui::Grid::new("entity").num_columns(2).show(ui, |ui| {
            ui.label("Id");
            ui.monospace(format!("{id:#06x} ({id})"));
            ui.end_row();
            ui.label("Name");
            ui.monospace(&info.name);
            ui.end_row();
            ui.label("Class");
            ui.monospace(info.class_name(&save.names));
            ui.end_row();
            if !info.attach_bone.is_empty() || info.parent != 0xffff {
                ui.label("Attached");
                ui.monospace(format!("parent {:#06x} bone '{}'", info.parent, info.attach_bone));
                ui.end_row();
            }
            if info.position.is_some() {
                ui.label("Position");
                ui.horizontal(|ui| {
                    for v in &mut self.pos_edit {
                        ui.add(egui::DragValue::new(v).speed(0.05).max_decimals(3));
                    }
                    set_pos = ui.button("Apply").clicked();
                });
                ui.end_row();
            }
            ui.label("Records");
            ui.monospace(format!(
                "dynamic {} bytes, runtime {}",
                save.entity(id).map_or(0, <[u8]>::len),
                save.active_rec(id).map_or("none".into(), |r| format!("{} bytes", r.data.len()))
            ));
            ui.end_row();
        });
        if set_pos {
            let p = self.pos_edit;
            let r = self.save.as_mut().unwrap().set_position(id, p);
            self.result("set position", r);
            self.rebuild_rows();
        }

        ui.separator();
        self.script_view(ui, id);

        let save = self.save.as_ref().unwrap();
        egui::CollapsingHeader::new("Hex: dynamic record").id_salt(("dynhex", id)).show(ui, |ui| {
            hex_box(ui, save.entity(id).unwrap_or_default(), "dyn");
        });
        if let Some(r) = save.active_rec(id) {
            egui::CollapsingHeader::new("Hex: runtime record").id_salt(("acthex", id)).show(ui, |ui| {
                hex_box(ui, &r.data, "act");
            });
        }
    }

    fn script_view(&mut self, ui: &mut egui::Ui, id: u16) {
        let save = self.save.as_ref().unwrap();
        let Some(view) = save.script(id) else { return };
        let data = save.entity(id).unwrap().to_vec();
        let runtime = save.active_rec(id).map(|r| r.data.clone());
        let label = |eid: u16| save.entity_name(eid);

        ui.heading(format!(
            "Script: {} blocks, {} links{}",
            view.script.blocks.len(),
            view.script.links.len(),
            if view.states.is_some() { "" } else { " (runtime state not found)" }
        ));

        struct Line {
            kind: String,
            desc: String,
            state: Vec<u8>,
            state_desc: String,
        }
        let lines: Vec<Line> = view
            .script
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let state = match (&view.states, &runtime) {
                    (Some(s), Some(r)) => r[s.ranges[i].clone()].to_vec(),
                    _ => Vec::new(),
                };
                Line {
                    kind: b.kind.clone(),
                    desc: describe(b, &data, &save.names, &label),
                    state_desc: describe_state(b, &state),
                    state,
                }
            })
            .collect();

        let mut action: Option<(usize, Action)> = None;
        let mut pick = None;
        egui::ScrollArea::vertical().id_salt("blocks").max_height(ui.available_height() * 0.7).auto_shrink([false, true]).show_rows(
            ui,
            20.0,
            lines.len(),
            |ui, range| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                for i in range {
                    let l = &lines[i];
                    ui.horizontal(|ui| {
                        let sel = self.sel_block == Some(i);
                        if ui
                            .selectable_label(sel, egui::RichText::new(format!("{i:4} {:<34}", truncate(&l.kind, 34))).monospace())
                            .clicked()
                        {
                            pick = Some(i);
                        }
                        ui.allocate_ui_with_layout(
                            egui::vec2(380.0, 18.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_width(380.0);
                                ui.add(egui::Label::new(&l.desc).truncate()).on_hover_text(&l.desc);
                            },
                        );
                        ui.monospace(format!("[{}]", hex(&l.state)));
                        ui.label(&l.state_desc);
                        if l.kind.starts_with("triggers/") && !l.state.is_empty() {
                            if l.state[0] == 0 {
                                if ui.small_button("activate").clicked() {
                                    action = Some((i, Action::Trigger(true)));
                                }
                            } else if ui.small_button("deactivate").clicked() {
                                action = Some((i, Action::Trigger(false)));
                            }
                        }
                        if l.kind == "logic/delay" && !l.state.is_empty() && ui.small_button("arm").on_hover_text("activate and start the timer so it fires right after loading").clicked() {
                            action = Some((i, Action::ArmDelay));
                        }
                    });
                }
            },
        );
        if let Some(i) = pick {
            self.sel_block = Some(i);
            self.state_edit = hex(&lines[i].state);
            self.param_edit = hex(&data[view.script.blocks[i].params.clone()]);
        }

        if let Some(i) = self.sel_block.filter(|&i| i < lines.len()) {
            ui.separator();
            let b = &view.script.blocks[i];
            ui.label(egui::RichText::new(format!("Block {i}: {}", b.kind)).strong());
            ui.horizontal(|ui| {
                ui.label("params");
                ui.add(egui::TextEdit::singleline(&mut self.param_edit).font(egui::TextStyle::Monospace).desired_width(700.0));
                if ui.button("apply").clicked() {
                    action = Some((i, Action::Params));
                }
            });
            ui.horizontal(|ui| {
                ui.label("state ");
                ui.add(egui::TextEdit::singleline(&mut self.state_edit).font(egui::TextStyle::Monospace).desired_width(300.0));
                if ui.button("apply").clicked() {
                    action = Some((i, Action::State));
                }
            });
            let links = |pred: &dyn Fn(&Link) -> bool| -> Vec<Link> { view.script.links.iter().filter(|l| pred(l)).copied().collect() };
            ui.columns(2, |cols| {
                cols[0].label("incoming (from.out → in)");
                for l in links(&|l| l.to as usize == i) {
                    if cols[0].link(format!("{}:{} .{} → in {}", l.from, lines[l.from as usize].kind, l.out_pin, l.in_pin)).clicked() {
                        pick = Some(l.from as usize);
                    }
                }
                cols[1].label("outgoing (out → to.in)");
                for l in links(&|l| l.from as usize == i) {
                    if cols[1].link(format!("out {} → {}:{} .{}", l.out_pin, l.to, lines[l.to as usize].kind, l.in_pin)).clicked() {
                        pick = Some(l.to as usize);
                    }
                }
            });
            if let Some(j) = pick {
                self.sel_block = Some(j);
                self.state_edit = hex(&lines[j].state);
                self.param_edit = hex(&data[view.script.blocks[j].params.clone()]);
            }
        }

        if let Some((i, a)) = action {
            let save = self.save.as_mut().unwrap();
            let (what, r) = match a {
                Action::Trigger(on) => ("set trigger", save.set_trigger_active(id, i, on)),
                Action::ArmDelay => ("arm delay", save.arm_delay(id, i, 100)),
                Action::State => ("set state", parse_hex(&self.state_edit).and_then(|b| save.set_block_state(id, i, &b))),
                Action::Params => ("set params", parse_hex(&self.param_edit).and_then(|b| save.set_block_param(id, i, 0, &b))),
            };
            self.result(&format!("{what} (block {i})"), r);
            if let (Some(save), Some(sel)) = (&self.save, self.sel_block) {
                self.state_edit = hex(&save.block_state(id, sel).unwrap_or_default());
                if let Some(v) = save.script(id) {
                    self.param_edit = hex(&save.entity(id).unwrap()[v.script.blocks[sel].params.clone()]);
                }
            }
        }
        ui.separator();
    }

    fn raw_view(&mut self, ui: &mut egui::Ui) {
        let Some(save) = &self.save else { return };
        ui.label("Overwrite bytes of a decompressed section. The section is re-parsed afterwards, so edits that break its structure are rejected.");
        let kind = *self.raw_section.get_or_insert(SectionKind::Active);
        ui.horizontal(|ui| {
            for k in [SectionKind::Header, SectionKind::Dynamic, SectionKind::Active] {
                if ui.selectable_label(kind == k, k.name()).clicked() {
                    self.raw_section = Some(k);
                }
            }
            ui.label("offset");
            ui.add(egui::TextEdit::singleline(&mut self.raw_offset).desired_width(90.0));
            ui.label("bytes (hex)");
            ui.add(egui::TextEdit::singleline(&mut self.raw_hex).font(egui::TextStyle::Monospace).desired_width(420.0));
        });
        let offset = parse_num(if self.raw_offset.is_empty() { "0" } else { &self.raw_offset }).unwrap_or(0);
        let mut apply = false;
        ui.horizontal(|ui| {
            apply = ui.button("Apply patch").clicked();
        });
        let data = match kind {
            SectionKind::Header => save.section(kind).to_vec(),
            SectionKind::Dynamic => metro_save::sections::write_dynamic(&save.dynamic),
            SectionKind::Active => metro_save::sections::write_active(&save.active),
        };
        let start = offset.min(data.len()) & !0xf;
        let end = (start + 1024).min(data.len());
        ui.label(format!("section size {} bytes, showing {start:#x}..{end:#x}", data.len()));
        egui::ScrollArea::vertical().id_salt("rawhex").show(ui, |ui| {
            ui.monospace(hexdump(&data[start..end], start));
        });
        if apply {
            let r = parse_hex(&self.raw_hex).and_then(|b| self.save.as_mut().unwrap().patch_section(kind, offset, &b));
            self.result("patch", r);
            self.rebuild_rows();
        }
    }
}

#[derive(Clone, Copy)]
enum Action {
    Trigger(bool),
    ArmDelay,
    State,
    Params,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(2.0);
        });
        if self.save.is_none() {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label("Open a save (e.g. 2033_auto_save from the game folder's <steam id> directory).");
                });
            });
            return;
        }
        egui::Panel::left("entities").resizable(true).default_size(430.0).show(ui, |ui| self.entity_list(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Overview, "Overview");
                ui.selectable_value(&mut self.tab, Tab::Entity, "Entity");
                ui.selectable_value(&mut self.tab, Tab::Raw, "Raw sections");
            });
            ui.separator();
            match self.tab {
                Tab::Overview => self.overview(ui),
                Tab::Entity => {
                    egui::ScrollArea::vertical().id_salt("entity_scroll").auto_shrink([false, false]).show(ui, |ui| self.entity_view(ui));
                }
                Tab::Raw => self.raw_view(ui),
            }
        });
    }
}

fn hex_box(ui: &mut egui::Ui, data: &[u8], salt: &str) {
    egui::ScrollArea::vertical().id_salt(salt).max_height(300.0).show(ui, |ui| {
        ui.monospace(hexdump(data, 0));
    });
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}
