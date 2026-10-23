//! The cast: the people of the story, as Claude should paint them. The writer describes each one
//! once, and Claude can draw them a model sheet: the figure from the front, the side, behind and
//! sitting, as SVG. Whenever a scene is painted, everyone its passage names goes with it, their
//! look in words and their sheet to copy the figure from. So they look the same in every picture
//! of the book, though the story never says again how they look. People are found by name, or by
//! another name the writer gives them, as whole words (`Doc::named_in`). A passage that names no
//! one ("He climbed the ladder") is about those the paragraph just before it named, so they are
//! carried on (`App::carried_people`), until a break in the story. The list of scenes says who each
//! was painted with, and the writer can choose others and paint it again.
//!
//! Someone can have several looks (as a girl, after the shipwreck), each with its words and sheet.
//! One is in use at a time, chosen by the writer, and scenes painted from then on get that one.
//!
//! The cast is kept with the document and never exported, like the story notes.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui::{self, ColorImage, Id, TextureHandle, TextureOptions};

use crate::App;
use crate::backdrop::{MODEL, SYSTEM, extract_svg, rasterize};
use crate::claude::{Outcome, run_claude};
use crate::model::{Character, Doc, Look, PAGE_BREAK, ParaKind};
use crate::theme::pal;

/// How many model sheets go with one scene at most. Each is some 15 to 30 KB of SVG, and with more
/// people in one picture Claude starts to mix them up.
const MOST_SHEETS: usize = 3;
/// How many paragraphs back a passage that names no one looks for the people it is about.
const CARRY_BACK: usize = 3;
/// Width in pixels model sheets are rendered at, for the cast window, and the most they are shown high.
const SHEET_WIDTH: f32 = 600.0;
const SHEET_HEIGHT: f32 = 240.0;

pub const LOOK_HINT: &str = "How they look: age, build, face, hair, the clothes they always wear, what they carry\u{2026}";
/// The same passages painted with the passage alone, with a description, and with a model sheet.
const EXAMPLE: &[u8] = include_bytes!("../assets/cast-example.jpg");

impl Character {
    /// Every name the story may call them by: their name and the others the writer gave.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.name.as_str()).chain(self.aliases.split(',')).map(str::trim).filter(|n| !n.is_empty())
    }

    /// Where `text` first calls them by one of their names, if it does.
    pub(crate) fn first_named(&self, text: &str) -> Option<usize> {
        self.names().filter_map(|n| find_word(text, n)).min()
    }
}

/// Where `word` first stands in `text` as a whole word, not inside a longer one ("Mara" in
/// "Mara's" but not in "Maradona"). Case counts, so a name is not found in a common word ("Rose",
/// "rose"), except for the first letter, as a name may start a sentence ("the miller", "The miller").
fn find_word(text: &str, word: &str) -> Option<usize> {
    let mut first = word.chars();
    let capital: String = first.next().into_iter().flat_map(char::to_uppercase).chain(first).collect();
    [word, capital.as_str()]
        .into_iter()
        .flat_map(|w| text.match_indices(w).map(move |(i, _)| (i, w.len())))
        .filter(|&(i, len)| {
            let before = text[..i].chars().next_back();
            let after = text[i + len..].chars().next();
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
        })
        .map(|(i, _)| i)
        .min()
}

impl Doc {
    /// A new id for someone of the cast or one of their looks.
    pub fn take_cast_id(&mut self) -> u64 {
        self.next_cast_id += 1;
        self.next_cast_id - 1
    }

    /// The people of the cast that `text` names, in the order it first names them.
    pub fn named_in(&self, text: &str) -> Vec<&Character> {
        let mut named: Vec<(usize, &Character)> = self.cast.iter().filter_map(|c| Some((c.first_named(text)?, c))).collect();
        named.sort_by_key(|&(at, c)| (at, c.id));
        named.into_iter().map(|(_, c)| c).collect()
    }

    /// The people of the cast with these names, in this order. A name no one has any more (renamed,
    /// deleted) is left out.
    pub fn cast_by_names(&self, names: &[String]) -> Vec<&Character> {
        names.iter().filter_map(|n| self.cast.iter().find(|c| c.name.trim() == n)).collect()
    }
}

impl App {
    /// For a passage at char `at` that names no one: the people named in the nearest paragraph
    /// before it that names anyone, `CARRY_BACK` paragraphs back at most, starting with the
    /// sentences before it in its own (a scene may begin partway into one). A break in the story
    /// ends the search: a chapter title, a page break, or an empty or starred line ("* * *"),
    /// after which "he" may well be someone else.
    pub fn carried_people(&self, at: usize) -> Vec<String> {
        let mut end = self.doc.para_start(at).0;
        let named = self.doc.named_in(&self.selected_passage(end, at));
        if !named.is_empty() {
            return named.iter().map(|c| c.name.trim().to_owned()).collect();
        }
        for _ in 0..CARRY_BACK {
            if end == 0 || self.doc.char_at(end - 1) == Some(PAGE_BREAK) {
                break;
            }
            let start = self.doc.para_start(end - 1).0;
            let text = self.selected_passage(start, end - 1);
            if self.doc.para_attrs_at(start).kind == ParaKind::ChapterTitle || !text.chars().any(char::is_alphanumeric) {
                break;
            }
            let named = self.doc.named_in(&text);
            if !named.is_empty() {
                return named.iter().map(|c| c.name.trim().to_owned()).collect();
            }
            end = start;
        }
        Vec::new()
    }
}

/// What a scene's prompt says first about the people it names: how they look, and the model sheets
/// of the first few, with how to use them. Empty if it names no one the writer described.
pub fn cast_prompt(named: &[&Character]) -> String {
    let described: Vec<&&Character> = named.iter().filter(|c| !c.look().words.trim().is_empty() || c.look().sheet.is_some()).collect();
    if described.is_empty() {
        return String::new();
    }
    let mut out = String::from("<cast>\n");
    for c in &described {
        let others: Vec<&str> = c.names().skip(1).collect();
        let also = if others.is_empty() { String::new() } else { format!(" (also called {})", others.join(", ")) };
        out += &format!("{}{also}: {}\n", c.name.trim(), c.look().words.trim());
    }
    out += "</cast>\n\n";
    let sheets: Vec<&&Character> = described.iter().filter(|c| c.look().sheet.is_some()).take(MOST_SHEETS).copied().collect();
    for c in &sheets {
        out += &format!("<model_sheet name=\"{}\">\n{}\n</model_sheet>\n\n", c.name.trim(), c.look().sheet.as_deref().unwrap_or_default());
    }
    out += if sheets.is_empty() {
        "This is how these people look in every illustration of this book: draw them so.\n\n"
    } else {
        // Tried: told only to reuse the sheet, Claude copied a standing figure into a passage
        // where she sat. So the passage decides the pose.
        "This is how these people look in every illustration of this book. Draw those with a model \
         sheet from it: copy the figure of the view that fits, with its defs, into your SVG and place \
         it with transform (translate, scale, flip), keeping their shapes, proportions and colours. \
         The passage decides the pose: where it needs one the sheet does not show, redraw the figure \
         in that pose with the same shapes and colours. Light them to match the scene.\n\n"
    };
    out
}

/// The name as it starts the ids in a model sheet ("Old Tom" → "old-tom"), so that two sheets in
/// one scene do not share ids.
fn slug(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let s = s.trim_matches('-').to_owned();
    if s.is_empty() { "person".into() } else { s }
}

/// What Claude is asked to draw for the model sheet of `c` as they look in `look`.
fn sheet_prompt(c: &Character, look: &Look) -> String {
    let id = slug(&c.name);
    format!(
        "<character>\n{}: {}\n</character>\n\n\
        Draw a model sheet of this character for a book illustrator to reuse in many scenes.\n\
        - One standalone SVG with viewBox=\"0 0 800 500\", a plain light background.\n\
        - The character four times: standing seen from the front, walking seen from the side, \
          standing seen from behind, and sitting on a plain stool seen from the side.\n\
        - Each figure in its own <g> with id \"{id}-front\", \"{id}-side\", \"{id}-back\" and \
          \"{id}-sitting\", drawn around the origin with the feet at y=0 and about 400 units tall \
          standing, then placed with <use>. Gradients and filters in <defs>, their ids starting \"{id}-\".\n\
        - Realistic proportions; painterly but clean shapes, easy to copy into another drawing.\n\
        - No text, letters or numbers. No scripts, links, external references or embedded images.\n\
        - At most about 30 KB.\n\
        Reply with only the SVG code.",
        c.name.trim(),
        look.words.trim()
    )
}

/// Ask Claude for a model sheet (or, with `peek`, a peek) with `prompt`. Only a drawing that
/// renders is kept; a peek only with all its parts.
fn draw_sheet(prompt: &str, peek: bool, cancel: &AtomicBool) -> Result<Option<String>, String> {
    let mut reply = String::new();
    match run_claude(MODEL, SYSTEM, prompt, "medium", cancel, &mut |t| reply.push_str(t))? {
        Outcome::Cancelled => Ok(None),
        Outcome::Refused => Err("Claude declined to draw this character".into()),
        Outcome::Finished => {
            let svg = extract_svg(&reply).ok_or("Claude's reply held no drawing")?;
            if peek {
                crate::peek::cut(svg)?;
            } else {
                rasterize(svg, SHEET_WIDTH)?;
            }
            Ok(Some(svg.to_owned()))
        }
    }
}

/// What a look is called in the switch: its name, or its place among the person's looks.
fn look_title(l: &Look, k: usize) -> String {
    match l.label.trim() {
        "" => format!("Look {}", k + 1),
        label => label.to_owned(),
    }
}

fn hash_of(svg: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    svg.hash(&mut h);
    h.finish()
}

/// A model sheet rendered, by the hash of its drawing.
type Rendered = (u64, Result<ColorImage, String>);

/// A model sheet being drawn, for the look with id `look` of the character with id `character`.
struct SheetJob {
    rx: Receiver<Result<String, String>>,
    cancel: Arc<AtomicBool>,
    character: u64,
    look: u64,
    /// It is their peek being drawn (`peek.rs`), not their model sheet.
    peek: bool,
}

impl Drop for SheetJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The cast window's state; the cast itself is the document's (`Doc::cast`).
pub struct CastPanel {
    pub open: bool,
    /// The example picture shows.
    pub example: bool,
    job: Option<SheetJob>,
    /// What Claude was last asked to draw.
    asked: String,
    /// Model sheets rendered, by a hash of their drawing, and those being rendered or that could not be.
    sheets: HashMap<u64, TextureHandle>,
    rendering: HashSet<u64>,
    rendered: (Sender<Rendered>, Receiver<Rendered>),
    example_texture: Option<TextureHandle>,
}

impl Default for CastPanel {
    fn default() -> Self {
        Self {
            open: false,
            example: false,
            job: None,
            asked: String::new(),
            sheets: HashMap::new(),
            rendering: HashSet::new(),
            rendered: channel(),
            example_texture: None,
        }
    }
}

impl CastPanel {
    /// What Claude was last asked to draw.
    #[cfg(test)]
    pub fn asked(&self) -> &str {
        &self.asked
    }

    /// The model sheet `svg` has been rendered for the window.
    #[cfg(test)]
    pub fn has_sheet_shown(&self, svg: &str) -> bool {
        self.sheets.contains_key(&hash_of(svg))
    }

    /// The example picture has been read.
    #[cfg(test)]
    pub fn example_shown(&self) -> bool {
        self.example_texture.is_some()
    }

    /// Render the model sheet `svg` on a background thread, unless it is or has been already.
    fn render(&mut self, ctx: &egui::Context, svg: &str) {
        let key = hash_of(svg);
        if self.sheets.contains_key(&key) || !self.rendering.insert(key) {
            return;
        }
        let (tx, ctx, svg) = (self.rendered.0.clone(), ctx.clone(), svg.to_owned());
        crate::web::spawn(move || {
            let _ = tx.send((key, rasterize(&svg, SHEET_WIDTH)));
            ctx.request_repaint();
        });
    }

    /// The example picture, read the first time it is wanted.
    fn example_texture(&mut self, ctx: &egui::Context) -> Option<&TextureHandle> {
        if self.example_texture.is_none() {
            // It is taller than some graphics cards take a texture.
            let most = ctx.input(|i| i.max_texture_side) as u32;
            let rgba = image::load_from_memory(EXAMPLE).ok()?.resize(most, most, image::imageops::FilterType::Triangle).to_rgba8();
            let image = ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
            self.example_texture = Some(ctx.load_texture("cast_example", image, TextureOptions::LINEAR));
        }
        self.example_texture.as_ref()
    }
}

impl App {
    /// Start drawing the model sheet of look `look` of the character with id `id` (stopping one
    /// under way).
    fn start_sheet(&mut self, ctx: &egui::Context, id: u64, look: u64, peek: bool) {
        let Some(c) = self.doc.cast.iter().find(|c| c.id == id) else { return };
        let Some(l) = c.looks.iter().find(|l| l.id == look) else { return };
        let prompt = if peek { crate::peek::puppet_prompt(c, l) } else { sheet_prompt(c, l) };
        let (tx, cancel) = self.sheet_job(id, look, peek);
        self.cast_panel.asked = prompt.clone();
        let ctx = ctx.clone();
        crate::web::spawn(move || {
            if let Some(result) = draw_sheet(&prompt, peek, &cancel).transpose() {
                let _ = tx.send(result);
            }
            ctx.request_repaint();
        });
    }

    fn sheet_job(&mut self, id: u64, look: u64, peek: bool) -> (Sender<Result<String, String>>, Arc<AtomicBool>) {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cast_panel.job = Some(SheetJob { rx, cancel: cancel.clone(), character: id, look, peek });
        (tx, cancel)
    }

    /// A model sheet `svg` for the look in use of the character with id `id`, as if Claude had
    /// just drawn it.
    #[cfg(test)]
    pub fn fake_sheet(&mut self, id: u64, svg: &str) {
        let look = self.doc.cast.iter().find(|c| c.id == id).unwrap().look().id;
        self.sheet_job(id, look, false).0.send(Ok(svg.to_owned())).unwrap();
    }

    /// A peek `svg` for the look in use of the character with id `id`, as if Claude had just drawn it.
    #[cfg(test)]
    pub fn fake_peek(&mut self, id: u64, svg: &str) {
        let look = self.doc.cast.iter().find(|c| c.id == id).unwrap().look().id;
        self.sheet_job(id, look, true).0.send(Ok(svg.to_owned())).unwrap();
    }

    /// Take in a finished model sheet and the sheets rendered.
    fn update_cast(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.cast_panel.job {
            let (id, look, peek) = (job.character, job.look, job.peek);
            match job.rx.try_recv() {
                Ok(Ok(svg)) => {
                    self.cast_panel.job = None;
                    // Deleted meanwhile, they get none.
                    if let Some(c) = self.doc.cast.iter_mut().find(|c| c.id == id) {
                        let several = c.looks.len() > 1;
                        if let Some(l) = c.looks.iter_mut().find(|l| l.id == look) {
                            let label = if several && !l.label.trim().is_empty() { format!(" ({})", l.label.trim()) } else { String::new() };
                            if peek {
                                self.status = format!("- {}{label} can peek out now", c.name.trim());
                                l.peek = Some(svg);
                            } else {
                                self.status = format!("- {}{label} has a model sheet", c.name.trim());
                                l.sheet = Some(svg);
                            }
                        }
                    }
                }
                Ok(Err(e)) => {
                    self.cast_panel.job = None;
                    self.status = if peek { format!("- no peek: {e}") } else { format!("- no model sheet: {e}") };
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.cast_panel.job = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        while let Ok((key, image)) = self.cast_panel.rendered.1.try_recv() {
            match image {
                Ok(image) => {
                    self.cast_panel.rendering.remove(&key);
                    self.cast_panel.sheets.insert(key, ctx.load_texture("model_sheet", image, TextureOptions::LINEAR));
                }
                Err(e) => self.status = format!("- a model sheet could not be shown: {e}"),
            }
        }
    }

    /// Whether a model sheet is being drawn, for the Scene menu's title.
    pub fn drawing_sheet(&self) -> bool {
        self.cast_panel.job.is_some()
    }

    /// The cast window: everyone in the story, each with their names and their looks, each look with
    /// its words and model sheet, and with several, which of them is in use.
    pub fn cast_window(&mut self, ctx: &egui::Context) {
        self.update_cast(ctx);
        self.cast_example(ctx);
        if !self.cast_panel.open {
            return;
        }
        enum Do {
            Add,
            Draw(u64, u64),
            DrawPeek(u64, u64),
            PeekNow(u64),
            RemovePeek(u64, u64),
            Stop,
            RemoveSheet(u64, u64),
            AddLook(u64),
            Use(u64, usize),
            DeleteLook(u64, u64),
            Delete(u64),
            Example,
        }
        let mut action = None;
        let mut open = true;
        let sheets: Vec<String> = self.doc.cast.iter().flat_map(|c| c.looks.iter().filter_map(|l| l.sheet.clone())).collect();
        for svg in sheets {
            self.cast_panel.render(ctx, &svg);
        }
        let drawing = self.cast_panel.job.as_ref().map(|j| (j.look, j.peek));
        let panel = &self.cast_panel;
        egui::Window::new("Cast")
            .id(Id::new("cast"))
            .open(&mut open)
            .default_width(400.0)
            // Room for someone with a model sheet and all its buttons, without scrolling.
            .default_height(620.0)
            .collapsible(false)
            .show(ctx, |ui| {
                let about = "The people of the story. When a scene is painted, those its passage names are painted \
                             as described here, and copied from their model sheet if they have one, so they look \
                             the same in every picture.";
                ui.label(egui::RichText::new(about).size(12.0).color(pal().text_dim));
                ui.horizontal(|ui| {
                    if ui.button("Add someone").clicked() {
                        action = Some(Do::Add);
                    }
                    if ui.button("See an example").on_hover_text("The same passages painted with and without the cast").clicked() {
                        action = Some(Do::Example);
                    }
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                    for c in self.doc.cast.iter_mut() {
                        ui.group(|ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let name = egui::TextEdit::singleline(&mut c.name).id(Id::new(("cast_name", c.id))).hint_text("Name");
                                ui.add(name.desired_width(130.0));
                                let also = egui::TextEdit::singleline(&mut c.aliases)
                                    .id(Id::new(("cast_aliases", c.id)))
                                    .hint_text("Also called (Grandma, the old woman)");
                                ui.add(also.desired_width(f32::INFINITY));
                            });
                            let (several, can_draw) = (c.looks.len() > 1, !c.name.trim().is_empty());
                            // Which look is in use, by the name, so it is switched without scrolling.
                            if several {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(egui::RichText::new("In use:").color(pal().text_dim));
                                    for (k, l) in c.looks.iter().enumerate() {
                                        let tip = "Scenes painted from now on show them so";
                                        if ui.radio(k == c.active, look_title(l, k)).on_hover_text(tip).clicked() && k != c.active {
                                            action = Some(Do::Use(c.id, k));
                                        }
                                    }
                                });
                            }
                            for l in c.looks.iter_mut() {
                                if several {
                                    ui.add_space(2.0);
                                    // The button first, from the right, and the name in the room left, so the
                                    // row never asks for more than the window has (it would widen it each frame).
                                    ui.horizontal(|ui| {
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.small_button("Delete look").clicked() {
                                                action = Some(Do::DeleteLook(c.id, l.id));
                                            }
                                            let hint = "Name this look (as a girl)";
                                            let label = egui::TextEdit::singleline(&mut l.label).id(Id::new(("cast_label", l.id))).hint_text(hint);
                                            ui.add(label.desired_width(f32::INFINITY));
                                        });
                                    });
                                }
                                let words = egui::TextEdit::multiline(&mut l.words).id(Id::new(("cast_look", l.id))).hint_text(LOOK_HINT);
                                ui.add(words.desired_rows(3).desired_width(f32::INFINITY));
                                if let Some(t) = l.sheet.as_deref().and_then(|s| panel.sheets.get(&hash_of(s))) {
                                    // As wide as the window, unless a tall drawing would push the rest out of sight.
                                    let s = t.size_vec2();
                                    let size = s * (ui.available_width() / s.x).min(SHEET_HEIGHT / s.y);
                                    ui.add(egui::Image::new((t.id(), size)).bg_fill(egui::Color32::WHITE));
                                }
                                ui.horizontal(|ui| {
                                    if let Some((_, peek)) = drawing.filter(|(look, _)| *look == l.id) {
                                        ui.spinner();
                                        let doing = if peek { "Drawing them peeking out\u{2026}" } else { "Drawing the model sheet\u{2026}" };
                                        ui.label(egui::RichText::new(doing).color(pal().text_dim));
                                        if ui.small_button("Stop").clicked() {
                                            action = Some(Do::Stop);
                                        }
                                        return;
                                    }
                                    let can = can_draw && !l.words.trim().is_empty();
                                    let label = if l.sheet.is_some() { "Draw the model sheet again" } else { "Draw a model sheet" };
                                    let tip = if can {
                                        "Claude draws them from the front, the side, behind and sitting, for every scene to copy"
                                    } else {
                                        "Give a name and how they look first"
                                    };
                                    if ui.add_enabled(can, egui::Button::new(label).small()).on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
                                        action = Some(Do::Draw(c.id, l.id));
                                    }
                                    if l.sheet.is_some() && ui.small_button("Remove the sheet").clicked() {
                                        action = Some(Do::RemoveSheet(c.id, l.id));
                                    }
                                });
                                // Peeking out, drawn from the model sheet (`peek.rs`); one drawn shows without it too.
                                if (l.sheet.is_some() || l.peek.is_some()) && drawing.is_none_or(|(look, _)| look != l.id) {
                                    ui.horizontal(|ui| {
                                        let label = if l.peek.is_some() { "Draw the peek again" } else { "Draw them peeking out" };
                                        let tip = "Claude draws them from their model sheet, peeking, in parts to move";
                                        let draw = ui.add_enabled(l.sheet.is_some(), egui::Button::new(label).small());
                                        if draw.on_hover_text(tip).on_disabled_hover_text("Draw a model sheet first").clicked() {
                                            action = Some(Do::DrawPeek(c.id, l.id));
                                        }
                                        if l.peek.is_some() {
                                            if ui.small_button("Peek now").on_hover_text("See them peek out from behind the page").clicked() {
                                                action = Some(Do::PeekNow(c.id));
                                            }
                                            if ui.small_button("Remove the peek").clicked() {
                                                action = Some(Do::RemovePeek(c.id, l.id));
                                            }
                                        }
                                    });
                                }
                            }
                            ui.separator();
                            ui.horizontal(|ui| {
                                let tip = "Keep another way they look (older, in disguise), to switch to later";
                                if ui.small_button("Add a look").on_hover_text(tip).clicked() {
                                    action = Some(Do::AddLook(c.id));
                                }
                                if ui.small_button("Delete").on_hover_text("Take them out of the cast").clicked() {
                                    action = Some(Do::Delete(c.id));
                                }
                            });
                        });
                        ui.add_space(4.0);
                    }
                });
            });
        self.cast_panel.open = open;
        match action {
            Some(Do::Add) => {
                let (id, look) = (self.doc.take_cast_id(), self.doc.take_cast_id());
                self.doc.cast.push(Character::new(id, look, ""));
                ctx.memory_mut(|m| m.request_focus(Id::new(("cast_name", id))));
            }
            Some(Do::Draw(id, look)) => self.start_sheet(ctx, id, look, false),
            Some(Do::DrawPeek(id, look)) => self.start_sheet(ctx, id, look, true),
            Some(Do::PeekNow(id)) => self.peek_now(id),
            Some(Do::RemovePeek(id, look)) => {
                if let Some(l) = self.doc.cast.iter_mut().filter(|c| c.id == id).flat_map(|c| c.looks.iter_mut()).find(|l| l.id == look) {
                    l.peek = None;
                }
            }
            Some(Do::Stop) => self.cast_panel.job = None,
            Some(Do::RemoveSheet(id, look)) => {
                if let Some(l) = self.doc.cast.iter_mut().filter(|c| c.id == id).flat_map(|c| c.looks.iter_mut()).find(|l| l.id == look) {
                    l.sheet = None;
                }
            }
            Some(Do::AddLook(id)) => {
                let look = self.doc.take_cast_id();
                if let Some(c) = self.doc.cast.iter_mut().find(|c| c.id == id) {
                    // It starts from the words of the look in use, to change what is different.
                    let words = c.look().words.clone();
                    c.looks.push(Look { id: look, label: String::new(), words, sheet: None, peek: None });
                    ctx.memory_mut(|m| m.request_focus(Id::new(("cast_label", look))));
                }
            }
            Some(Do::Use(id, k)) => {
                if let Some(c) = self.doc.cast.iter_mut().find(|c| c.id == id) {
                    c.active = k.min(c.looks.len() - 1);
                }
            }
            Some(Do::DeleteLook(id, look)) => {
                if drawing.is_some_and(|(l, _)| l == look) {
                    self.cast_panel.job = None;
                }
                if let Some(c) = self.doc.cast.iter_mut().find(|c| c.id == id && c.looks.len() > 1) {
                    let k = c.looks.iter().position(|l| l.id == look).unwrap_or(c.looks.len());
                    if k < c.looks.len() {
                        c.looks.remove(k);
                        // The look in use stays in use; if it was this one, the one after it (or before) is.
                        if c.active > k {
                            c.active -= 1;
                        }
                        c.active = c.active.min(c.looks.len() - 1);
                    }
                }
            }
            Some(Do::Delete(id)) => {
                if self.cast_panel.job.as_ref().is_some_and(|j| j.character == id) {
                    self.cast_panel.job = None;
                }
                self.doc.cast.retain(|c| c.id != id);
            }
            Some(Do::Example) => self.cast_panel.example = true,
            None => {}
        }
    }

    /// The example: four passages that name Mara, painted from the passage alone, with her look in
    /// words, and with her model sheet.
    fn cast_example(&mut self, ctx: &egui::Context) {
        if !self.cast_panel.example {
            return;
        }
        let mut open = true;
        let Some(texture) = self.cast_panel.example_texture(ctx).cloned() else {
            self.cast_panel.example = false;
            return;
        };
        egui::Window::new("The cast at work")
            .id(Id::new("cast_example"))
            .open(&mut open)
            .default_width(560.0)
            .default_height(640.0)
            .collapsible(false)
            .show(ctx, |ui| {
                let about = "Four passages that only name Mara, painted three ways. From the passage alone she is \
                             someone else every time. Described in the cast, her clothes stay but her build and \
                             drawing change. With her model sheet she is the same figure throughout, posed for each \
                             scene. Poses the sheet does not show are the weak point: at the inn she sits in the \
                             story but stands in the picture.";
                ui.label(egui::RichText::new(about).size(12.0).color(pal().text_dim));
                ui.add_space(4.0);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let w = ui.available_width();
                    ui.add(egui::Image::new((texture.id(), texture.size_vec2() * (w / texture.size_vec2().x))));
                });
            });
        self.cast_panel.example = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn character(name: &str, aliases: &str, look: &str, sheet: Option<&str>) -> Character {
        let look = Look { id: 2, label: String::new(), words: look.into(), sheet: sheet.map(Into::into), peek: None };
        Character { id: 1, name: name.into(), aliases: aliases.into(), looks: vec![look], active: 0 }
    }

    #[test]
    fn people_are_found_by_any_of_their_names_as_whole_words() {
        let mara = character("Mara", "Grandma, the old woman", "", None);
        assert_eq!(mara.first_named("At the dock Mara argued."), Some(12));
        assert_eq!(mara.first_named("Mara's lantern swung."), Some(0), "with an apostrophe after it");
        assert_eq!(mara.first_named("Maradona scored."), None, "not inside a longer word");
        assert_eq!(mara.first_named("The boats knocked on the stones."), None);
        assert_eq!(mara.first_named("Then Grandma came in."), Some(5), "by another name");
        assert_eq!(mara.first_named("The old woman came in."), Some(0), "starting a sentence");
        let rose = character("Rose", "", "", None);
        assert_eq!(rose.first_named("A rose grew by the door."), None, "a name is not found in a common word");
    }

    #[test]
    fn a_scene_tells_how_the_people_it_names_look_and_sends_the_first_sheets() {
        assert_eq!(cast_prompt(&[]), "");
        let nobody = character("Tom", "", "  ", None);
        assert_eq!(cast_prompt(&[&nobody]), "", "someone not described adds nothing");

        let mara = character("Mara", "Grandma", "grey braid, red scarf", None);
        let words = cast_prompt(&[&mara]);
        assert!(words.starts_with("<cast>\nMara (also called Grandma): grey braid, red scarf\n</cast>"), "{words}");
        assert!(!words.contains("<model_sheet"));

        let sheets: Vec<Character> = (0..4).map(|k| character(&format!("P{k}"), "", "tall", Some(&format!("<svg id=\"{k}\"/>")))).collect();
        let all: Vec<&Character> = sheets.iter().collect();
        let prompt = cast_prompt(&all);
        assert_eq!(prompt.matches("<model_sheet").count(), MOST_SHEETS, "only the first few sheets are sent");
        assert!(prompt.contains("<model_sheet name=\"P0\">\n<svg id=\"0\"/>\n</model_sheet>"));
        assert!(prompt.contains("P3: tall"), "everyone's look is sent");
        assert!(prompt.contains("The passage decides the pose"));
    }

    #[test]
    fn a_model_sheet_is_asked_for_with_ids_from_the_name() {
        let tom = character("Old Tom", "", "a fisherman in yellow oilskins", None);
        let prompt = sheet_prompt(&tom, tom.look());
        assert!(prompt.starts_with("<character>\nOld Tom: a fisherman in yellow oilskins\n</character>"));
        assert!(prompt.contains("\"old-tom-sitting\""));
        assert_eq!(slug("  "), "person");
    }

    #[test]
    fn the_example_picture_can_be_read() {
        let img = image::load_from_memory(EXAMPLE).unwrap();
        assert!(img.width() > 800 && img.height() > img.width());
    }

    /// Asks the real Claude for a model sheet: `cargo test live_model_sheet -- --ignored`.
    #[test]
    #[ignore]
    fn live_model_sheet() {
        crate::claude::allow_live();
        let mara = character("Mara", "", "a woman of about sixty, tall and wiry, long grey braid, red wool scarf, \
                                         long dark-green oilskin coat, black boots, carries a brass lantern", None);
        let svg = draw_sheet(&sheet_prompt(&mara, mara.look()), false, &AtomicBool::new(false)).unwrap().unwrap();
        let path = std::env::temp_dir().join("caprice-live-sheet.svg");
        std::fs::write(&path, svg).unwrap();
        println!("model sheet saved to {}", path.display());
    }

    /// Claude draws Mara peeking, from a model sheet it drew first; the drawing has all its parts.
    /// `cargo test live_peek -- --ignored --nocapture`, then look at the SVG saved.
    #[test]
    #[ignore]
    fn live_peek() {
        crate::claude::allow_live();
        let mut mara = character("Mara", "", "a girl of ten, freckles, short red hair, a blue smock", None);
        let sheet = draw_sheet(&sheet_prompt(&mara, mara.look()), false, &AtomicBool::new(false)).unwrap().unwrap();
        mara.looks[0].sheet = Some(sheet);
        let svg = draw_sheet(&crate::peek::puppet_prompt(&mara, mara.look()), true, &AtomicBool::new(false)).unwrap().unwrap();
        assert!(crate::peek::cut(&svg).is_ok(), "all its parts are there");
        let path = std::env::temp_dir().join("caprice-live-peek.svg");
        std::fs::write(&path, svg).unwrap();
        println!("peek saved to {}", path.display());
    }
}
