//! Printing: the pages as Caprice lays them out, made into a PDF, then saved or sent to a printer.
//!
//! The PDF is drawn from the same page layouts as the screen (`PageLayout::ink` at page size, R4),
//! so it breaks and wraps where the pages shown do, in the same fonts, which it carries inside.
//! Only the pages are printed: not the scene behind them, the post-its or any highlight.
//!
//! A printer gets the PDF through CUPS (`lp`), which macOS and Linux both have. Setting a printer
//! up is the system's business: the window only lists those there are and offers the way to the
//! system's printer settings.

use std::collections::HashMap;
use std::ops::{Range, RangeInclusive};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui::{self, Color32, FontFamily, Galley, Id, Pos2, Rect, pos2, vec2};
use krilla::Document;
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Size, Transform};
use krilla::image::Image;
use krilla::metadata::Metadata;
use krilla::num::NormalizedF32;
use krilla::paint::Fill;
use krilla::surface::Surface;
use krilla::text::{Font, GlyphId, KrillaGlyph};

use crate::App;
use crate::fileio::DialogFor;
use crate::layout::Ink;
use crate::theme::INK;
#[cfg(target_arch = "wasm32")]
use crate::web::rfd;

/// The printers the system has, and the one it prints to unless told otherwise.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Printers {
    pub names: Vec<String>,
    pub default: Option<String>,
}

/// One print job, as the spooler is asked for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub printer: String,
    pub copies: u32,
    pub two_sided: bool,
    pub title: String,
    /// The paper's name when it is one printers know by name (A4, Letter).
    pub media: Option<&'static str>,
}

/// Where print jobs go: CUPS for real, something that only keeps them in tests.
pub trait Spooler: Send + Sync {
    fn printers(&self) -> Result<Printers, String>;
    /// Send the PDF at `file` to be printed; what the spooler said, or why it would not.
    fn print(&self, job: &Job, file: &Path) -> Result<String, String>;
}

/// CUPS, through its command-line tools, the same on macOS and Linux.
pub struct Cups;

/// Run a CUPS tool in the C locale, so its answers can be read.
fn cups(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program).args(args).env("LC_ALL", "C").output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("CUPS is not installed ({program} is missing)")
        } else {
            format!("{program}: {e}")
        }
    })?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(text)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(if err.is_empty() { format!("{program} failed") } else { err })
    }
}

/// The printers in `lpstat -p` output ("printer Office is idle.  enabled since …").
fn printers_listed(lpstat_p: &str) -> Vec<String> {
    lpstat_p.lines().filter_map(|l| l.strip_prefix("printer ")?.split_whitespace().next()).map(str::to_owned).collect()
}

/// The default printer in `lpstat -d` output ("system default destination: Office").
fn default_listed(lpstat_d: &str) -> Option<String> {
    lpstat_d.lines().find_map(|l| l.split_once("default destination:")).map(|(_, name)| name.trim().to_owned())
}

impl Spooler for Cups {
    fn printers(&self) -> Result<Printers, String> {
        // With no printer set up, lpstat says so on stderr and fails: that is no printers, not an error.
        let names = match cups("lpstat", &["-p"]) {
            Ok(text) => printers_listed(&text),
            Err(e) if e.contains("No destinations") => Vec::new(),
            Err(e) => return Err(e),
        };
        let default = cups("lpstat", &["-d"]).ok().and_then(|t| default_listed(&t)).filter(|d| names.contains(d));
        Ok(Printers { names, default })
    }

    fn print(&self, job: &Job, file: &Path) -> Result<String, String> {
        let copies = job.copies.to_string();
        let mut args = vec!["-d", &job.printer, "-n", &copies, "-t", &job.title];
        let media = job.media.map(|m| format!("media={m}"));
        if let Some(m) = &media {
            args.extend(["-o", m]);
        }
        args.extend(["-o", if job.two_sided { "sides=two-sided-long-edge" } else { "sides=one-sided" }]);
        let file = file.to_string_lossy();
        args.push(&file);
        cups("lp", &args).map(|said| said.trim().to_owned())
    }
}

/// Open the system's printer settings, where printers are added.
fn open_printer_settings() -> Result<(), String> {
    let tried: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("open", &["x-apple.systempreferences:com.apple.Printers-Scanners-Settings.extension"])]
    } else {
        &[("system-config-printer", &[]), ("xdg-open", &["http://localhost:631/admin"])]
    };
    for (program, args) in tried {
        if Command::new(program).args(*args).spawn().is_ok() {
            return Ok(());
        }
    }
    Err("no printer settings found: install system-config-printer, or open http://localhost:631".into())
}

/// Which pages are printed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Which {
    All,
    /// The page shown.
    Shown,
    /// From `Print::from` to `Print::upto`.
    Some,
}

/// The Print window and what it has chosen.
pub struct Print {
    pub open: bool,
    pub spooler: Arc<dyn Spooler>,
    /// The system's printers, once asked (each time the window opens, as one may have been added).
    printers: Option<Result<Printers, String>>,
    asking: Option<Receiver<Result<Printers, String>>>,
    /// The printer chosen, or None for a PDF file.
    pub to: Option<String>,
    /// Has the writer chosen where it goes? Until then it goes to the system's default printer.
    chosen: bool,
    pub which: Which,
    /// The first and last page printed with `Which::Some`, counting from 1 as the page bar does.
    pub from: usize,
    pub upto: usize,
    pub copies: u32,
    pub two_sided: bool,
    sending: Option<Receiver<Result<String, String>>>,
}

impl Print {
    pub fn new() -> Self {
        Self {
            open: false,
            spooler: Arc::new(Cups),
            printers: None,
            asking: None,
            to: None,
            chosen: false,
            which: Which::All,
            from: 1,
            upto: 1,
            copies: 1,
            two_sided: false,
            sending: None,
        }
    }

    /// The pages printed (from 0), with `shown` the page shown and `n` pages in all.
    pub fn pages(&self, shown: usize, n: usize) -> RangeInclusive<usize> {
        let last = n.max(1) - 1;
        match self.which {
            Which::All => 0..=last,
            Which::Shown => shown.min(last)..=shown.min(last),
            Which::Some => {
                let a = self.from.clamp(1, last + 1) - 1;
                a..=(self.upto.clamp(1, last + 1) - 1).max(a)
            }
        }
    }

    /// Has the system said which printers it has, since it was last asked?
    pub fn asking_done(&self) -> bool {
        self.asking.is_none() && self.printers.is_some()
    }

    fn ask_printers(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let (spooler, ctx) = (self.spooler.clone(), ctx.clone());
        crate::web::spawn(move || {
            let _ = tx.send(spooler.printers());
            ctx.request_repaint();
        });
        self.asking = Some(rx);
    }

    fn take_printers(&mut self) {
        let Some(rx) = &self.asking else { return };
        let answer = match rx.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("could not ask for the printers".into()),
        };
        self.asking = None;
        if let Ok(p) = &answer {
            let gone = self.to.as_ref().is_some_and(|t| !p.names.contains(t));
            if !self.chosen || gone {
                self.to = p.default.clone().or_else(|| p.names.first().cloned());
            }
        }
        self.printers = Some(answer);
    }
}

/// What printers call a paper of `size` points, for the common sizes.
fn media_name(size: egui::Vec2) -> Option<&'static str> {
    let (w, h) = (size.x.min(size.y), size.x.max(size.y));
    let near = |a: f32, b: f32| (w - a).abs() < 1.5 && (h - b).abs() < 1.5;
    [("A4", 595.0, 842.0), ("Letter", 612.0, 792.0), ("A5", 420.0, 595.0), ("Legal", 612.0, 1008.0)]
        .into_iter()
        .find(|&(_, a, b)| near(a, b))
        .map(|(name, _, _)| name)
}

/// A printer's queue name as people read it ("Office_Laser" → "Office Laser").
fn shown_name(queue: &str) -> String {
    queue.replace('_', " ")
}

impl App {
    /// Open the Print window, asking the system for its printers again.
    pub fn open_print(&mut self, ctx: &egui::Context) {
        let n = self.doc.pages();
        let p = &mut self.print;
        p.open = true;
        if p.from > n || p.upto > n || p.upto == 0 {
            (p.from, p.upto) = (1, n);
        }
        p.ask_printers(ctx);
    }

    /// The document's name, for a print job or a PDF.
    fn print_title(&self) -> String {
        self.path.as_ref().and_then(|p| p.file_stem()).map_or("Untitled".to_owned(), |s| s.to_string_lossy().into_owned())
    }

    /// The Print window, if open, and the answer of a print job sent.
    pub fn print_window(&mut self, ctx: &egui::Context) {
        self.take_print_answer();
        if !self.print.open {
            return;
        }
        self.print.take_printers();
        let n = self.doc.pages();
        let shown = self.target.min(n - 1);
        let mut open = true;
        let (mut go, mut settings, mut cancel) = (false, false, false);
        let screen = ctx.content_rect();
        egui::Window::new("Print")
            .id(Id::new("print"))
            .open(&mut open)
            .default_pos(pos2(screen.center().x - 170.0, screen.top() + 110.0))
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let p = &mut self.print;
                egui::Grid::new("print_grid").num_columns(2).spacing(vec2(12.0, 10.0)).show(ui, |ui| {
                    ui.label("Printer");
                    ui.horizontal(|ui| {
                        let names = match &p.printers {
                            Some(Ok(list)) => list.names.clone(),
                            _ => Vec::new(),
                        };
                        let current = p.to.as_deref().map_or("PDF file".to_owned(), shown_name);
                        egui::ComboBox::from_id_salt("print_to").selected_text(current).width(200.0).show_ui(ui, |ui| {
                            for name in &names {
                                if ui.selectable_label(p.to.as_ref() == Some(name), shown_name(name)).clicked() {
                                    (p.to, p.chosen) = (Some(name.clone()), true);
                                }
                            }
                            if ui.selectable_label(p.to.is_none(), "PDF file").clicked() {
                                (p.to, p.chosen) = (None, true);
                            }
                        });
                        if p.asking.is_some() {
                            ui.spinner();
                        }
                    });
                    ui.end_row();

                    match &p.printers {
                        Some(Ok(list)) if list.names.is_empty() => {
                            ui.label("");
                            ui.vertical(|ui| {
                                ui.label("No printer is set up.");
                                settings = ui.button("Printer settings…").on_hover_text("Add a printer in the system's settings").clicked();
                            });
                            ui.end_row();
                        }
                        Some(Err(e)) => {
                            ui.label("");
                            ui.vertical(|ui| {
                                ui.colored_label(ui.visuals().warn_fg_color, format!("No printers: {e}"));
                                ui.label("Saving a PDF still works.");
                            });
                            ui.end_row();
                        }
                        _ => {}
                    }

                    ui.label("Pages");
                    ui.vertical(|ui| {
                        ui.radio_value(&mut p.which, Which::All, format!("All ({n})"));
                        ui.radio_value(&mut p.which, Which::Shown, format!("This page ({})", shown + 1));
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut p.which, Which::Some, "From");
                            let a = ui.add(egui::DragValue::new(&mut p.from).range(1..=n).speed(0.1));
                            ui.label("to");
                            let b = ui.add(egui::DragValue::new(&mut p.upto).range(1..=n).speed(0.1));
                            if a.changed() || b.changed() {
                                p.which = Which::Some;
                            }
                        });
                    });
                    ui.end_row();

                    if p.to.is_some() {
                        ui.label("Copies");
                        ui.add(egui::DragValue::new(&mut p.copies).range(1..=99).speed(0.1));
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut p.two_sided, "Both sides of the paper");
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
                // Inside `horizontal`, so the row is as wide as the window and no wider (R16).
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if p.to.is_some() { "Print" } else { "Save PDF…" };
                        go = ui.add_enabled(p.sending.is_none(), egui::Button::new(label)).clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            });
        self.print.open = open && !cancel && !go;
        if settings && let Err(e) = open_printer_settings() {
            self.status = format!("- {e}");
        }
        if go {
            self.print_now(ctx);
        }
    }

    /// Print the chosen pages to the chosen printer, or ask where to save them as a PDF.
    fn print_now(&mut self, ctx: &egui::Context) {
        let Some(printer) = self.print.to.clone() else {
            let name = format!("{}.pdf", self.print_title());
            self.ask_path(ctx, DialogFor::PrintPdf, move || {
                rfd::FileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(name).save_file()
            });
            return;
        };
        let pages = self.print.pages(self.target, self.doc.pages());
        let pdf = match self.pdf(ctx, pages) {
            Ok(pdf) => pdf,
            Err(e) => {
                self.status = format!("- printing failed: {e}");
                return;
            }
        };
        let job = Job {
            printer,
            copies: self.print.copies.max(1),
            two_sided: self.print.two_sided,
            title: self.print_title(),
            media: media_name(self.doc.setup.size()),
        };
        // lp reads the file before it answers, so it can go as soon as lp is done with it.
        let file = crate::web::temp_dir().join(format!("caprice-print-{}-{}.pdf", crate::web::pid(), self.doc.version));
        if let Err(e) = std::fs::write(&file, pdf) {
            self.status = format!("- printing failed: {e}");
            return;
        }
        let (tx, rx) = channel();
        let (spooler, ctx) = (self.print.spooler.clone(), ctx.clone());
        crate::web::spawn(move || {
            let said = spooler.print(&job, &file).map(|_| format!("- sent to {}", shown_name(&job.printer)));
            let _ = std::fs::remove_file(&file);
            let _ = tx.send(said);
            ctx.request_repaint();
        });
        self.print.sending = Some(rx);
        self.status = "- printing…".into();
    }

    fn take_print_answer(&mut self) {
        let Some(rx) = &self.print.sending else { return };
        let said = match rx.try_recv() {
            Ok(said) => said,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the printer did not answer".into()),
        };
        self.print.sending = None;
        self.status = said.unwrap_or_else(|e| format!("- printing failed: {e}"));
    }

    /// Save the chosen pages as a PDF at `path`.
    pub(crate) fn print_pdf_to(&mut self, ctx: &egui::Context, path: PathBuf) {
        let pages = self.print.pages(self.target, self.doc.pages());
        let written = self.pdf(ctx, pages).and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string()));
        self.status = match written {
            Ok(()) => format!("- saved {}", path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned())),
            Err(e) => format!("- PDF failed: {e}"),
        };
    }

    /// Pages `pages` (from 0) as a PDF, each at the paper's size, drawn as the screen draws them.
    pub fn pdf(&self, ctx: &egui::Context, pages: RangeInclusive<usize>) -> Result<Vec<u8>, String> {
        let size = self.doc.setup.size();
        let mut document = Document::new();
        document.set_metadata(Metadata::new().title(self.print_title()).creator("Caprice".into()));
        let mut faces = Faces::new(self.fonts.defs());
        let mut images: HashMap<u32, Option<Image>> = HashMap::new();
        // Headers showing the chapter need the chapters, found once for all the pages.
        let chapters = std::cell::OnceCell::new();
        let chapters = || chapters.get_or_init(|| self.doc.chapters(false)).clone();
        for i in pages.filter(|&i| i < self.doc.pages()) {
            let settings = krilla::page::PageSettings::from_wh(size.x, size.y).ok_or("the paper has no size")?;
            let mut page = document.start_page_with(settings);
            let mut surface = page.surface();
            let layout = self.doc.layout_page(ctx, i, 1.0, &[]);
            let mut ink = self.underlay(ctx, i, 1.0);
            ink.extend(layout.ink(self.doc.setup.margin_origin().to_pos2()));
            ink.extend(self.furniture(ctx, i, 1.0, &chapters));
            for ink in ink {
                match ink {
                    Ink::Text(galley, at) => draw_galley(&mut surface, &mut faces, &galley, at),
                    Ink::Fill(points, color) => draw_shape(&mut surface, &points, true, Some(color), None),
                    Ink::Line(points, width, color) => draw_shape(&mut surface, &points, false, None, Some((width, color))),
                    Ink::Picture(img) => {
                        let picture = images.entry(img.id).or_insert_with(|| {
                            let data = self.doc.images.iter().find(|d| d.id == img.id)?;
                            let (bytes, kind) = crate::images::export_media(data);
                            let bytes = krilla::Data::from(bytes);
                            if kind == "png" { Image::from_png(bytes, true) } else { Image::from_jpeg(bytes, true) }.ok()
                        });
                        if let (Some(picture), Some(at)) = (picture.clone(), Size::from_wh(img.rect.width(), img.rect.height())) {
                            surface.push_transform(&Transform::from_translate(img.rect.left(), img.rect.top()));
                            surface.draw_image(picture, at);
                            surface.pop();
                        }
                    }
                }
            }
            surface.finish();
            page.finish();
        }
        document.finish().map_err(|e| format!("{e:?}"))
    }
}

/// The font files egui draws with, read for their glyphs and made into PDF fonts as they are needed.
struct Faces<'a> {
    defs: &'a egui::FontDefinitions,
    read: HashMap<&'a str, Option<(ttf_parser::Face<'a>, Font)>>,
}

impl<'a> Faces<'a> {
    fn new(defs: &'a egui::FontDefinitions) -> Self {
        Self { defs, read: HashMap::new() }
    }

    fn face(&mut self, key: &'a str) -> Option<&(ttf_parser::Face<'a>, Font)> {
        let defs = self.defs;
        self.read
            .entry(key)
            .or_insert_with(|| {
                let data = defs.font_data.get(key)?;
                let face = ttf_parser::Face::parse(&data.font, data.index).ok()?;
                let bytes = match &data.font {
                    std::borrow::Cow::Borrowed(b) => krilla::Data::from(*b),
                    std::borrow::Cow::Owned(v) => krilla::Data::from(v.clone()),
                };
                Some((face, Font::new(bytes, data.index)?))
            })
            .as_ref()
    }

    /// The face that draws `c` in `family`, and its glyph: the first in the family's chain that
    /// has it, as egui chooses.
    fn glyph(&mut self, family: &FontFamily, c: char) -> Option<(&'a str, u16)> {
        let defs = self.defs;
        for key in defs.families.get(family)? {
            if let Some((face, _)) = self.face(key)
                && let Some(g) = face.glyph_index(c)
            {
                return Some((key, g.0));
            }
        }
        None
    }
}

/// Glyphs of one row drawn in one format and face, each at its place on the page.
#[derive(Debug)]
pub struct Run<'a> {
    pub section: usize,
    pub face: &'a str,
    pub text: String,
    /// Where an underline goes on the page, as egui draws it: along the bottom of the line.
    pub underline_y: f32,
    /// Per glyph: its id, its baseline's start on the page, its width, and its bytes in `text`.
    pub glyphs: Vec<(u16, Pos2, f32, Range<usize>)>,
}

/// The runs of `galley` drawn with its top-left at `at`: a new one wherever the format or the face
/// changes, or a char has no glyph in any face (it is left out, as egui leaves it).
fn runs<'a>(faces: &mut Faces<'a>, galley: &Galley, at: Pos2) -> Vec<Run<'a>> {
    let job = &galley.job;
    let mut bytes = job.text.char_indices().map(|(b, _)| b);
    let mut section = 0;
    let mut out: Vec<Run> = Vec::new();
    for placed in &galley.rows {
        let mut open = false;
        for g in &placed.row.glyphs {
            let b = bytes.next().unwrap_or(job.text.len());
            while section + 1 < job.sections.len() && !job.sections[section].byte_range.contains(&egui::text::ByteIndex(b)) {
                section += 1;
            }
            let Some(sec) = job.sections.get(section) else { continue };
            let Some((face, id)) = faces.glyph(&sec.format.font_id.family, g.chr) else {
                open = false;
                continue;
            };
            let pos = at + placed.pos.to_vec2() + g.pos.to_vec2();
            let run = match out.last_mut() {
                Some(r) if open && r.section == section && r.face == face => r,
                _ => {
                    let underline_y = pos.y - g.font_ascent + g.line_height;
                    out.push(Run { section, face, text: String::new(), underline_y, glyphs: Vec::new() });
                    out.last_mut().expect("just pushed")
                }
            };
            let start = run.text.len();
            run.text.push(g.chr);
            run.glyphs.push((id, pos, g.advance_width, start..run.text.len()));
            open = true;
        }
        if placed.ends_with_newline {
            bytes.next();
        }
    }
    out
}

/// Where each glyph of `run` starts, and where the last one ends. egui puts every glyph on a whole
/// pixel, which at page size is a whole point: printed, letters and words would stand unevenly
/// apart. So the glyphs follow each other by their own widths, as egui worked them out before
/// rounding, and only a justified line's spaces widen, to end where egui's does. The chars egui
/// joined into one glyph (a ligature) share its width, as print draws them one by one.
pub fn places(run: &Run) -> Vec<f32> {
    let chars: Vec<char> = run.text.chars().collect();
    let gs = &run.glyphs;
    let (mut xs, mut spaces) = (Vec::with_capacity(gs.len() + 1), Vec::with_capacity(gs.len()));
    let mut next = gs.first().map_or(0.0, |g| g.1.x);
    let (mut k, mut seen) = (0, 0);
    while k < gs.len() {
        // The chars after a ligature's first come with no width of their own.
        let joined = (k + 1..gs.len()).take_while(|&j| gs[j].2 == 0.0 && !chars[j].is_whitespace()).count();
        for j in 0..=joined {
            xs.push(next + gs[k].2 * j as f32 / (joined + 1) as f32);
            spaces.push(seen);
        }
        next += gs[k].2;
        seen += usize::from(chars[k].is_whitespace());
        k += joined + 1;
    }
    xs.push(next);
    spaces.push(seen);
    // A justified line ends further on than its glyphs reach: the difference goes to its spaces.
    let last = gs.len().saturating_sub(1);
    let short = gs.last().map_or(0.0, |g| g.1.x - xs[last]);
    if short > 1.0 && spaces[last] > 0 {
        let per_space = short / spaces[last] as f32;
        for (x, n) in xs.iter_mut().zip(&spaces) {
            *x += per_space * *n as f32;
        }
    }
    xs
}

/// Draw `galley` with its top-left at `at`, as text a PDF reader can search and copy.
fn paint_of(color: Color32) -> (krilla::paint::Paint, NormalizedF32) {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    (rgb::Color::new(r, g, b).into(), NormalizedF32::new(f32::from(a) / 255.0).unwrap_or(NormalizedF32::ONE))
}

/// An outline through `points` (`closed` back to the first), filled and stroked as given.
fn draw_shape(surface: &mut Surface, points: &[Pos2], closed: bool, fill: Option<Color32>, stroke: Option<(f32, Color32)>) {
    let mut path = PathBuilder::new();
    for (k, p) in points.iter().enumerate() {
        if k == 0 { path.move_to(p.x, p.y) } else { path.line_to(p.x, p.y) }
    }
    if closed {
        path.close();
    }
    let Some(path) = path.finish() else { return };
    surface.set_fill(fill.map(|c| {
        let (paint, opacity) = paint_of(c);
        Fill { paint, opacity, ..Default::default() }
    }));
    surface.set_stroke(stroke.map(|(width, c)| {
        let (paint, opacity) = paint_of(c);
        krilla::paint::Stroke { paint, width, opacity, ..Default::default() }
    }));
    surface.draw_path(&path);
    surface.set_stroke(None);
}

fn draw_galley(surface: &mut Surface, faces: &mut Faces, galley: &Galley, at: Pos2) {
    for run in runs(faces, galley, at) {
        let format = &galley.job.sections[run.section].format;
        let Some(tweak) = faces.defs.font_data.get(run.face).map(|d| d.tweak.clone()) else { continue };
        let Some(font) = faces.face(run.face).map(|(_, f)| f.clone()) else { continue };
        let size = format.font_id.size * tweak.scale;
        let lift = format.font_id.size * tweak.y_offset_factor + tweak.y_offset;
        let color = if format.color == Color32::PLACEHOLDER { INK } else { format.color };
        let [r, g, b, a] = color.to_srgba_unmultiplied();
        let fill = |surface: &mut Surface| {
            surface.set_fill(Some(Fill {
                paint: rgb::Color::new(r, g, b).into(),
                opacity: NormalizedF32::new(f32::from(a) / 255.0).unwrap_or(NormalizedF32::ONE),
                ..Default::default()
            }));
        };
        fill(surface);
        let first = run.glyphs[0].1;
        let xs = places(&run);
        let glyphs: Vec<KrillaGlyph> = run
            .glyphs
            .iter()
            .enumerate()
            .map(|(k, (id, _, _, range))| {
                let advance = xs[k + 1] - xs[k];
                KrillaGlyph::new(GlyphId::new(u32::from(*id)), advance / size, 0.0, 0.0, 0.0, range.clone(), None)
            })
            .collect();
        let start = Point::from_xy(first.x, first.y + lift);
        // egui slants text with no italic face of its own by a quarter of its height; so does print.
        if format.italics {
            surface.push_transform(&Transform::from_row(1.0, 0.0, -0.25, 1.0, 0.25 * start.y, 0.0));
        }
        surface.draw_glyphs(start, &glyphs, font, &run.text, size, false);
        if format.italics {
            surface.pop();
        }
        if format.underline.width > 0.0 {
            let (y, w) = (run.underline_y, format.underline.width);
            let line = Rect::from_min_max(pos2(first.x, y - w / 2.0), pos2(xs[xs.len() - 1], y + w / 2.0));
            let [r, g, b, a] = format.underline.color.to_srgba_unmultiplied();
            surface.set_fill(Some(Fill {
                paint: rgb::Color::new(r, g, b).into(),
                opacity: NormalizedF32::new(f32::from(a) / 255.0).unwrap_or(NormalizedF32::ONE),
                ..Default::default()
            }));
            let mut path = PathBuilder::new();
            if let Some(rect) = krilla::geom::Rect::from_xywh(line.left(), line.top(), line.width(), line.height()) {
                path.push_rect(rect);
            }
            if let Some(path) = path.finish() {
                surface.draw_path(&path);
            }
        }
    }
}

#[cfg(test)]
impl App {
    /// The text print puts on page `i`, run after run, as a PDF reader finds it.
    pub fn printed_text(&self, ctx: &egui::Context, i: usize) -> String {
        let mut faces = Faces::new(self.fonts.defs());
        let layout = self.doc.layout_page(ctx, i, 1.0, &[]);
        let mut ink = self.underlay(ctx, i, 1.0);
        ink.extend(layout.ink(self.doc.setup.margin_origin().to_pos2()));
        ink.extend(self.furniture(ctx, i, 1.0, &|| self.doc.chapters(false)));
        let texts = ink.iter().filter_map(|k| if let Ink::Text(g, at) = k { Some((g, *at)) } else { None });
        texts.flat_map(|(g, at)| runs(&mut faces, g, at)).map(|r| r.text).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printers_and_the_default_are_read_from_lpstat() {
        let listed = "printer Office_Laser is idle.  enabled since Mon 12 Oct 2026\n\
                      printer Home disabled since Tue 13 Oct 2026 -\n\treason unknown\n";
        assert_eq!(printers_listed(listed), ["Office_Laser", "Home"]);
        assert_eq!(default_listed("system default destination: Home\n").as_deref(), Some("Home"));
        assert_eq!(default_listed("no system default destination\n"), None);
        assert_eq!(shown_name("Office_Laser"), "Office Laser");
    }

    #[test]
    fn the_paper_goes_to_the_printer_by_name_when_it_has_one() {
        assert_eq!(media_name(vec2(595.0, 842.0)), Some("A4"));
        assert_eq!(media_name(vec2(792.0, 612.0)), Some("Letter"), "landscape is the same paper");
        assert_eq!(media_name(vec2(500.0, 700.0)), None);
    }

    fn run(text: &str, glyphs: &[(f32, f32)]) -> Run<'static> {
        let mut at = 0;
        let glyphs = text
            .chars()
            .zip(glyphs)
            .map(|(c, &(x, w))| {
                at += c.len_utf8();
                (1, pos2(x, 10.0), w, at - c.len_utf8()..at)
            })
            .collect();
        Run { section: 0, face: "f", text: text.into(), underline_y: 12.0, glyphs }
    }

    #[test]
    fn letters_follow_their_own_widths_and_not_the_whole_points_egui_put_them_on() {
        // Three letters 5.4 wide, rounded by egui to 10, 15 and 21.
        let xs = places(&run("abc", &[(10.0, 5.4), (15.0, 5.4), (21.0, 5.4)]));
        let want = [10.0, 15.4, 20.8, 26.2];
        assert!(xs.iter().zip(want).all(|(x, w)| (x - w).abs() < 1e-4), "{xs:?}");
    }

    #[test]
    fn a_justified_lines_spaces_take_up_what_it_is_wider_by() {
        // "a b c", each glyph 5 wide, egui spreading the words to end at 40 rather than 20.
        let xs = places(&run("a b c", &[(0.0, 5.0), (5.0, 5.0), (18.0, 5.0), (23.0, 5.0), (40.0, 5.0)]));
        assert_eq!(xs, [0.0, 5.0, 20.0, 25.0, 40.0, 45.0]);
    }

    #[test]
    fn chars_joined_into_one_glyph_share_its_width() {
        // "fit": egui drew "fi" as one glyph 9 wide, the "i" coming with no width.
        let xs = places(&run("fit", &[(0.0, 9.0), (9.0, 0.0), (9.0, 3.0)]));
        assert_eq!(xs, [0.0, 4.5, 9.0, 12.0]);
    }
}
