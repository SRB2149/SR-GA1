//! GUI application shell: state, undo/redo, file handling, toolbar, keyboard
//! navigation, autosave, and the frame loop. The fabric canvas, inspectors
//! and simulation panel live in the sibling modules.

mod canvas;
pub mod colors;
mod inspector;
mod simpanel;

use eframe::egui;
use fpga_core::bitstream;
use fpga_core::config::{BlockId, CellConfig, Design};
use fpga_core::designfile::{load_design, save_design, DesignFile};
use fpga_core::drc::{self, DrcItem, Severity};
use fpga_core::fabric::Fabric;
use fpga_core::naming::{resolve, Namer, NetOrigin, Netlist};
use fpga_core::sim::{Settled, SimState};
use fpga_core::trace::Tracer;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub fn run(open: Option<String>) {
    let shell = match boot(open) {
        Ok(app) => Shell::Ready(Box::new(app)),
        Err(msg) => Shell::Failed(msg),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 920.0])
            .with_title("fpgatool — SR-GA1"),
        ..Default::default()
    };
    let _ = eframe::run_native("fpgatool", options, Box::new(move |_cc| Ok(Box::new(shell))));
}

fn boot(open: Option<String>) -> Result<App, String> {
    let mut fabric_path = PathBuf::from("fabric.toml");
    if !fabric_path.exists() {
        if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)) {
            let candidate = dir.join("fabric.toml");
            if candidate.exists() {
                fabric_path = candidate;
            }
        }
    }
    let fabric = Fabric::load_file(&fabric_path).map_err(|e| e.to_string())?;
    let mut app = App::new(fabric);
    if let Some(path) = open {
        app.open_path(PathBuf::from(path));
    }
    Ok(app)
}

/// Wrapper so a broken fabric.toml still shows a readable window.
enum Shell {
    Ready(Box<App>),
    Failed(String),
}

impl eframe::App for Shell {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        match self {
            Shell::Ready(app) => app.update(ctx, frame),
            Shell::Failed(msg) => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.heading("Could not load the fabric description");
                    ui.label(msg.as_str());
                    ui.label("Fix fabric.toml and restart fpgatool.");
                });
            }
        }
    }
}

pub enum Copied {
    Clb(CellConfig),
    Csb(CellConfig),
}

pub struct Camera {
    pub pan: egui::Vec2,
    pub zoom: f32,
}

pub struct SimRuntime {
    pub state: SimState,
    pub settled: Option<Settled>,
    pub tracer: Tracer,
    /// Tick at which the tracer's history starts.
    pub trace_start: u64,
    pub running: bool,
    pub tps: f32,
    pub accum: f32,
    pub error: Option<String>,
}

pub struct App {
    pub fabric: Fabric,
    pub file: DesignFile,
    pub file_path: Option<PathBuf>,
    pub dirty: bool,

    pub nets: Netlist,
    pub drc_items: Vec<DrcItem>,
    pub drc_dismissed: HashSet<String>,
    pub show_drc: bool,
    pub show_history: bool,
    pub show_waves: bool,

    pub selected: Option<BlockId>,
    pub io_selected: Option<(bool, usize)>,
    pub multi: Vec<BlockId>,

    pub undo_stack: Vec<(String, Design)>,
    pub redo_stack: Vec<(String, Design)>,

    pub sim: Option<SimRuntime>,

    pub cam: Camera,
    pub fit_requested: bool,
    pub center_on: Option<BlockId>,
    pub search: String,
    pub flash: Option<(BlockId, f64)>,

    pub copied: Option<Copied>,
    pub toasts: Vec<(String, f64)>,
    pub last_autosave: f64,

    pub rename_buf: String,
    pub rename_err: Option<String>,
    pub stim_bufs: HashMap<String, String>,
    pub wrap_export: bool,
}

impl App {
    fn new(fabric: Fabric) -> Self {
        let file = DesignFile::new(&fabric, "untitled");
        let nets = resolve(&fabric, &file.design);
        let drc_items = drc::check(&fabric, &file.design);
        App {
            fabric,
            file,
            file_path: None,
            dirty: false,
            nets,
            drc_items,
            drc_dismissed: HashSet::new(),
            show_drc: false,
            show_history: false,
            show_waves: false,
            selected: None,
            io_selected: None,
            multi: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            sim: None,
            cam: Camera { pan: egui::Vec2::ZERO, zoom: 1.0 },
            fit_requested: true,
            center_on: None,
            search: String::new(),
            flash: None,
            copied: None,
            toasts: Vec::new(),
            last_autosave: 0.0,
            rename_buf: String::new(),
            rename_err: None,
            stim_bufs: HashMap::new(),
            wrap_export: false,
        }
    }

    // ------------------------------------------------------------------
    // Naming helpers

    pub fn block_label(&self, b: BlockId) -> String {
        Namer::new(&self.fabric, &self.file.design).block_name(b)
    }

    pub fn is_pinned(&self, b: BlockId) -> bool {
        self.file.design.pinned_name(b).is_some()
    }

    pub fn net_label(&self, o: NetOrigin) -> String {
        Namer::new(&self.fabric, &self.file.design).net_name(o)
    }

    pub fn clock_label(&self, col: usize) -> String {
        Namer::new(&self.fabric, &self.file.design).clock_name(&self.nets.clocks[col])
    }

    // ------------------------------------------------------------------
    // Mutation with undo

    pub fn apply(&mut self, desc: &str, f: impl FnOnce(&mut Design, &Fabric)) {
        self.undo_stack.push((desc.to_string(), self.file.design.clone()));
        if self.undo_stack.len() > 300 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
        let fabric = self.fabric.clone();
        f(&mut self.file.design, &fabric);
        self.after_design_change();
    }

    pub fn undo(&mut self) {
        if let Some((desc, design)) = self.undo_stack.pop() {
            self.redo_stack.push((desc.clone(), self.file.design.clone()));
            self.file.design = design;
            self.after_design_change();
            self.toast(format!("undid: {}", desc));
        }
    }

    pub fn redo(&mut self) {
        if let Some((desc, design)) = self.redo_stack.pop() {
            self.undo_stack.push((desc.clone(), self.file.design.clone()));
            self.file.design = design;
            self.after_design_change();
            self.toast(format!("redid: {}", desc));
        }
    }

    pub fn after_design_change(&mut self) {
        self.nets = resolve(&self.fabric, &self.file.design);
        self.drc_items = drc::check(&self.fabric, &self.file.design);
        self.dirty = true;
        if let Some(sim) = &mut self.sim {
            match sim.state.view(&self.fabric, &self.file.design, &self.file.stimulus) {
                Ok(s) => sim.settled = Some(s),
                Err(e) => {
                    sim.error = Some(e.to_string());
                    sim.running = false;
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Selection, copy/paste

    pub fn select(&mut self, b: Option<BlockId>) {
        self.selected = b;
        self.io_selected = None;
        self.multi.clear();
        self.rename_err = None;
        self.rename_buf = b.map(|b| self.block_label(b)).unwrap_or_default();
    }

    /// Pin a manual name on a block, surfacing collisions inline.
    pub fn try_rename(&mut self, b: BlockId, name: &str) {
        if name == self.block_label(b) {
            self.rename_err = None;
            return;
        }
        let mut candidate = self.file.design.clone();
        match candidate.rename(&self.fabric, b, name) {
            Ok(()) => {
                self.undo_stack.push((format!("rename to \"{}\"", name), self.file.design.clone()));
                self.redo_stack.clear();
                self.file.design = candidate;
                self.rename_err = None;
                self.after_design_change();
            }
            Err(e) => self.rename_err = Some(e.to_string()),
        }
    }

    /// Write one mux select slice of a CLB with undo.
    pub fn set_slice_clb(&mut self, col: usize, row: usize, slice: fpga_core::fabric::FieldSlice, value: u64, desc: &str) {
        let width = self.fabric.clb_fields[slice.field].width;
        self.apply(desc, move |design, _| {
            let old = design.clb(col, row).get(slice.field);
            let new = match slice.bit {
                Some(b) => (old & !(1 << b)) | ((value & 1) << b),
                None => value,
            };
            design.clb_mut(col, row).set(slice.field, width, new);
        });
    }

    pub fn set_slice_csb(&mut self, col: usize, slice: fpga_core::fabric::FieldSlice, value: u64, desc: &str) {
        let width = self.fabric.csb_fields[slice.field].width;
        self.apply(desc, move |design, _| {
            let old = design.csb(col).get(slice.field);
            let new = match slice.bit {
                Some(b) => (old & !(1 << b)) | ((value & 1) << b),
                None => value,
            };
            design.csb_mut(col).set(slice.field, width, new);
        });
    }

    /// Flag or unflag a signal for tracing (id format: see `trace.rs`).
    pub fn toggle_trace(&mut self, id: String) {
        if let Some(i) = self.file.traces.iter().position(|t| *t == id) {
            self.file.traces.remove(i);
        } else {
            self.file.traces.push(id);
        }
        self.dirty = true;
        simpanel::rebuild_tracer(self);
    }

    pub fn is_traced(&self, id: &str) -> bool {
        self.file.traces.iter().any(|t| t == id)
    }

    pub fn copy_selected(&mut self) {
        match self.selected {
            Some(BlockId::Clb { col, row }) => {
                self.copied = Some(Copied::Clb(self.file.design.clb(col, row).clone()));
                self.toast("copied CLB configuration".to_string());
            }
            Some(BlockId::Csb { col }) => {
                self.copied = Some(Copied::Csb(self.file.design.csb(col).clone()));
                self.toast("copied CSB configuration".to_string());
            }
            None => {}
        }
    }

    pub fn paste_selected(&mut self) {
        let mut targets: Vec<BlockId> = self.multi.clone();
        if let Some(sel) = self.selected {
            if !targets.contains(&sel) {
                targets.push(sel);
            }
        }
        if targets.is_empty() {
            return;
        }
        enum Src {
            Clb(CellConfig),
            Csb(CellConfig),
        }
        let src = match &self.copied {
            Some(Copied::Clb(c)) => Src::Clb(c.clone()),
            Some(Copied::Csb(c)) => Src::Csb(c.clone()),
            None => return,
        };
        let mut count = 0usize;
        self.apply("paste configuration", |design, _fabric| {
            for t in targets {
                match (&src, t) {
                    (Src::Clb(c), BlockId::Clb { col, row }) => {
                        *design.clb_mut(col, row) = c.clone();
                        count += 1;
                    }
                    (Src::Csb(c), BlockId::Csb { col }) => {
                        *design.csb_mut(col) = c.clone();
                        count += 1;
                    }
                    _ => {}
                }
            }
        });
        self.toast(format!("pasted into {} block(s)", count));
    }

    // ------------------------------------------------------------------
    // Files

    pub fn toast(&mut self, msg: String) {
        self.toasts.push((msg, 0.0));
    }

    fn title_name(&self) -> String {
        match &self.file_path {
            Some(p) => p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
            None => "untitled".to_string(),
        }
    }

    pub fn open_path(&mut self, path: PathBuf) {
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                self.toast(format!("cannot read {}: {}", path.display(), e));
                return;
            }
        };
        match load_design(&self.fabric, &src) {
            Ok((file, warnings)) => {
                for w in warnings {
                    self.toast(w);
                }
                self.file = file;
                self.file_path = Some(path);
                self.dirty = false;
                self.undo_stack.clear();
                self.redo_stack.clear();
                self.sim = None;
                self.stim_bufs.clear();
                self.select(None);
                self.after_design_change();
                self.dirty = false;
                self.fit_requested = true;
            }
            Err(e) => self.toast(format!("{}: {}", path.display(), e)),
        }
    }

    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new().add_filter("design", &["json"]).pick_file() {
            self.open_path(path);
        }
    }

    fn save(&mut self) {
        match &self.file_path {
            Some(path) => {
                let path = path.clone();
                self.save_to(path);
            }
            None => self.save_as(),
        }
    }

    fn save_as(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("design", &["json"])
            .set_file_name(format!("{}.json", self.file.name))
            .save_file()
        {
            self.file_path = Some(path.clone());
            self.save_to(path);
        }
    }

    fn save_to(&mut self, path: PathBuf) {
        if let Some(sim) = &self.sim {
            self.file.tick = sim.state.tick;
        }
        match std::fs::write(&path, save_design(&self.fabric, &self.file)) {
            Ok(()) => {
                self.dirty = false;
                self.toast(format!("saved {}", path.display()));
            }
            Err(e) => self.toast(format!("cannot write {}: {}", path.display(), e)),
        }
    }

    fn export_bitstream(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("bitstream", &["txt"])
            .set_file_name(format!("{}.txt", self.file.name))
            .save_file()
        {
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let wrap = if self.wrap_export { Some(80) } else { None };
            let text = bitstream::format_text(&self.fabric, &self.file.design, &self.file.name, &timestamp, false, wrap);
            match std::fs::write(&path, text) {
                Ok(()) => self.toast(format!("exported {} bits", self.fabric.total_bits())),
                Err(e) => self.toast(format!("cannot write {}: {}", path.display(), e)),
            }
        }
    }

    fn import_bitstream(&mut self) {
        if let Some(path) = rfd::FileDialog::new().add_filter("bitstream", &["txt"]).pick_file() {
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    self.toast(format!("cannot read {}: {}", path.display(), e));
                    return;
                }
            };
            match bitstream::parse_text(&text).and_then(|bits| bitstream::import_bits(&self.fabric, &bits)) {
                Ok(design) => {
                    self.apply("import bitstream", move |d, _| *d = design);
                    self.toast("bitstream imported".to_string());
                }
                Err(e) => self.toast(format!("{}: {}", path.display(), e)),
            }
        }
    }

    fn new_design(&mut self) {
        self.file = DesignFile::new(&self.fabric, "untitled");
        self.file_path = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.sim = None;
        self.stim_bufs.clear();
        self.select(None);
        self.after_design_change();
        self.dirty = false;
    }

    fn autosave(&mut self, now: f64) {
        if !self.dirty || now - self.last_autosave < 60.0 {
            return;
        }
        self.last_autosave = now;
        let path = match &self.file_path {
            Some(p) => p.with_extension("autosave.json"),
            None => PathBuf::from("untitled.autosave.json"),
        };
        let _ = std::fs::write(&path, save_design(&self.fabric, &self.file));
    }

    // ------------------------------------------------------------------
    // Search

    fn run_search(&mut self, now: f64) {
        let query = self.search.trim().to_lowercase();
        if query.is_empty() {
            return;
        }
        let namer = Namer::new(&self.fabric, &self.file.design);
        let mut blocks = self.file.design.blocks();
        // Exact match first, then substring.
        blocks.sort_by_key(|b| namer.block_name(*b).to_lowercase() != query);
        let hit = blocks
            .iter()
            .find(|b| {
                let name = namer.block_name(**b).to_lowercase();
                name == query
                    || name.contains(&query)
                    || matches!(**b, BlockId::Clb { .. })
                        && [&self.fabric.naming.suffix_op, &self.fabric.naming.suffix_reg, &self.fabric.naming.suffix_carry]
                            .iter()
                            .any(|s| format!("{}{}", name, s.to_lowercase()) == query)
            })
            .copied();
        match hit {
            Some(b) => {
                self.select(Some(b));
                self.center_on = Some(b);
                self.flash = Some((b, now + 2.0));
            }
            None => self.toast(format!("nothing named \"{}\"", self.search.trim())),
        }
    }

    // ------------------------------------------------------------------
    // Keyboard

    fn keyboard(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused().is_some()) {
            return; // a text field owns the keyboard
        }
        let (mut dx, mut dy) = (0i32, 0i32);
        ctx.input(|i| {
            if i.key_pressed(egui::Key::ArrowLeft) {
                dx -= 1;
            }
            if i.key_pressed(egui::Key::ArrowRight) {
                dx += 1;
            }
            if i.key_pressed(egui::Key::ArrowUp) {
                dy += 1; // row 0 is at the bottom
            }
            if i.key_pressed(egui::Key::ArrowDown) {
                dy -= 1;
            }
        });
        if dx != 0 || dy != 0 {
            let next = match self.selected {
                None => Some(BlockId::Clb { col: 0, row: 0 }),
                Some(BlockId::Clb { col, row }) => {
                    let col = (col as i32 + dx).clamp(0, self.fabric.columns as i32 - 1) as usize;
                    let row = row as i32 + dy;
                    if row < 0 {
                        Some(BlockId::Csb { col })
                    } else {
                        Some(BlockId::Clb { col, row: (row.min(self.fabric.rows as i32 - 1)) as usize })
                    }
                }
                Some(BlockId::Csb { col }) => {
                    if dy > 0 {
                        Some(BlockId::Clb { col, row: 0 })
                    } else {
                        let col = (col as i32 + dx).clamp(0, self.fabric.columns as i32 - 1) as usize;
                        Some(BlockId::Csb { col })
                    }
                }
            };
            self.select(next);
            self.center_on = next;
        }
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Escape) {
                // handled below (needs &mut self outside closure)
            }
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.select(None);
        }
        let ctrl = ctx.input(|i| i.modifiers.command);
        if ctrl && ctx.input(|i| i.key_pressed(egui::Key::Z)) {
            self.undo();
        }
        if ctrl && ctx.input(|i| i.key_pressed(egui::Key::Y)) {
            self.redo();
        }
        if ctrl && ctx.input(|i| i.key_pressed(egui::Key::C)) {
            self.copy_selected();
        }
        if ctrl && ctx.input(|i| i.key_pressed(egui::Key::V)) {
            self.paste_selected();
        }
        if ctrl && ctx.input(|i| i.key_pressed(egui::Key::S)) {
            self.save();
        }
    }

    // ------------------------------------------------------------------
    // Frame

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time);
        self.keyboard(ctx);
        self.autosave(now);
        simpanel::advance_sim(self, ctx);

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            self.toolbar(ui, now);
        });
        egui::TopBottomPanel::bottom("simpanel").resizable(true).show(ctx, |ui| {
            simpanel::show(self, ui);
        });
        if self.selected.is_some() || self.io_selected.is_some() {
            egui::SidePanel::right("inspector")
                .resizable(true)
                .default_width(360.0)
                .show(ctx, |ui| {
                    inspector::show(self, ui);
                });
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            canvas::show(self, ui, now);
        });

        self.drc_window(ctx);
        self.history_window(ctx);
        self.toast_overlay(ctx, now);

        if self.sim.as_ref().is_some_and(|s| s.running) {
            ctx.request_repaint();
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, now: f64) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New").clicked() {
                    self.new_design();
                    ui.close_menu();
                }
                if ui.button("Open…").clicked() {
                    self.open_dialog();
                    ui.close_menu();
                }
                if ui.button("Save").clicked() {
                    self.save();
                    ui.close_menu();
                }
                if ui.button("Save as…").clicked() {
                    self.save_as();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Export bitstream…").clicked() {
                    self.export_bitstream();
                    ui.close_menu();
                }
                ui.checkbox(&mut self.wrap_export, "wrap exported bits at 80 columns");
                if ui.button("Import bitstream…").clicked() {
                    self.import_bitstream();
                    ui.close_menu();
                }
            });
            ui.menu_button("Edit", |ui| {
                let undo_label = self
                    .undo_stack
                    .last()
                    .map(|(d, _)| format!("Undo {}", d))
                    .unwrap_or_else(|| "Undo".to_string());
                if ui.add_enabled(!self.undo_stack.is_empty(), egui::Button::new(undo_label)).clicked() {
                    self.undo();
                    ui.close_menu();
                }
                if ui.add_enabled(!self.redo_stack.is_empty(), egui::Button::new("Redo")).clicked() {
                    self.redo();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Copy block config").clicked() {
                    self.copy_selected();
                    ui.close_menu();
                }
                if ui.button("Paste block config").clicked() {
                    self.paste_selected();
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("History…").clicked() {
                    self.show_history = true;
                    ui.close_menu();
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Zoom to fit").clicked() {
                    self.fit_requested = true;
                    ui.close_menu();
                }
                if ui.button("Design rule checks…").clicked() {
                    self.show_drc = true;
                    ui.close_menu();
                }
                ui.checkbox(&mut self.show_waves, "Waveform panel");
            });
            ui.separator();
            let name_edit = egui::TextEdit::singleline(&mut self.file.name)
                .desired_width(140.0)
                .hint_text("design name");
            ui.add(name_edit);
            ui.separator();
            let search = egui::TextEdit::singleline(&mut self.search)
                .desired_width(180.0)
                .hint_text("search block or net (Enter)");
            let resp = ui.add(search);
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.run_search(now);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let errors = self.drc_items.iter().filter(|i| i.severity == Severity::Error).count();
                let label = if errors > 0 {
                    egui::RichText::new(format!("DRC: {} error(s)", errors)).color(egui::Color32::LIGHT_RED)
                } else {
                    egui::RichText::new("DRC ok").color(egui::Color32::from_gray(160))
                };
                if ui.button(label).clicked() {
                    self.show_drc = true;
                }
                ui.label(
                    egui::RichText::new(format!("{}{}", self.title_name(), if self.dirty { " *" } else { "" }))
                        .color(egui::Color32::from_gray(160)),
                );
            });
        });
    }

    fn drc_window(&mut self, ctx: &egui::Context) {
        if !self.show_drc {
            return;
        }
        let mut open = self.show_drc;
        let mut select: Option<BlockId> = None;
        let mut dismiss: Option<String> = None;
        egui::Window::new("Design rule checks")
            .open(&mut open)
            .default_width(520.0)
            .show(ctx, |ui| {
                if ui.button("Re-run").clicked() {
                    self.drc_items = drc::check(&self.fabric, &self.file.design);
                    self.drc_dismissed.clear();
                }
                ui.separator();
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    for item in &self.drc_items {
                        if self.drc_dismissed.contains(&item.message) {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            let (tag, color) = match item.severity {
                                Severity::Error => ("error", egui::Color32::LIGHT_RED),
                                Severity::Warning => ("warn", egui::Color32::GOLD),
                                Severity::Info => ("info", egui::Color32::from_gray(150)),
                            };
                            ui.label(egui::RichText::new(tag).color(color).monospace());
                            let resp = ui.add(egui::Label::new(&item.message).sense(egui::Sense::click()).wrap());
                            if resp.clicked() {
                                select = item.block;
                            }
                            if ui.small_button("dismiss").clicked() {
                                dismiss = Some(item.message.clone());
                            }
                        });
                    }
                });
            });
        self.show_drc = open;
        if let Some(msg) = dismiss {
            self.drc_dismissed.insert(msg);
        }
        if let Some(b) = select {
            self.select(Some(b));
            self.center_on = Some(b);
        }
    }

    fn history_window(&mut self, ctx: &egui::Context) {
        if !self.show_history {
            return;
        }
        let mut open = self.show_history;
        let mut undo_to: Option<usize> = None;
        egui::Window::new("History").open(&mut open).default_width(320.0).show(ctx, |ui| {
            ui.label(format!("{} change(s); click to undo back to a point", self.undo_stack.len()));
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for (i, (desc, _)) in self.undo_stack.iter().enumerate().rev() {
                    if ui.button(format!("{}: {}", i + 1, desc)).clicked() {
                        undo_to = Some(i);
                    }
                }
            });
        });
        self.show_history = open;
        if let Some(target) = undo_to {
            while self.undo_stack.len() > target {
                self.undo();
            }
        }
    }

    fn toast_overlay(&mut self, ctx: &egui::Context, now: f64) {
        for t in &mut self.toasts {
            if t.1 == 0.0 {
                t.1 = now + 4.0;
            }
        }
        self.toasts.retain(|t| t.1 > now);
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::LEFT_BOTTOM, [12.0, -12.0])
            .show(ctx, |ui| {
                for (msg, _) in &self.toasts {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(msg);
                    });
                }
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(300));
    }
}
