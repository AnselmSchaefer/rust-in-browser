//! The `.caprice` file format (JSON).
//!
//! Version 2 stores the document as one list of styled runs; page breaks are `\u{c}` characters
//! inside it. Version 1 files stored a list of pages, which are loaded as hard-separated pages.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::App;
use crate::model::{Character, Doc, Look, Flow, ImageData, Note, PAGE_BREAK, PageSetup, ParaAttrs, Scene, Style};
#[cfg(target_arch = "wasm32")]
use crate::web::rfd;

#[derive(Serialize, Deserialize)]
struct Run {
    text: String,
    font: String,
    size: f32,
    bold: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    italic: bool,
    underline: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<[u8; 3]>,
    /// Paragraph format; only present on runs holding paragraph marks.
    #[serde(default, skip_serializing_if = "is_default_para")]
    para: ParaAttrs,
    /// Picture id, on the placeholder character of a picture.
    #[serde(default, skip_serializing_if = "is_zero")]
    image: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_default_para(p: &ParaAttrs) -> bool {
    *p == ParaAttrs::default()
}

#[derive(Serialize, Deserialize)]
struct NoteFile {
    start: usize,
    end: usize,
    text: String,
    #[serde(default)]
    color: usize,
}

#[derive(Serialize, Deserialize)]
struct ImageFile {
    id: u32,
    format: String,
    /// The original file, base64 encoded.
    data: String,
    width_pt: f32,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    rotation: u8,
}

fn is_zero_u8(n: &u8) -> bool {
    *n == 0
}

/// A scene painted for the story, pinned to char `at`.
#[derive(Serialize, Deserialize)]
struct SceneEntry {
    at: usize,
    versions: Vec<String>,
    #[serde(default)]
    shown: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    subject: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    hidden: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    people: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    chosen: bool,
}

/// Someone in the story: their names, and how they look. Files from before looks could be kept
/// side by side hold one look in `look` and `sheet`.
#[derive(Serialize, Deserialize)]
struct CharacterEntry {
    name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    aliases: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    looks: Vec<LookEntry>,
    #[serde(default)]
    active: usize,
    #[serde(default, skip_serializing)]
    look: String,
    #[serde(default, skip_serializing)]
    sheet: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct LookEntry {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    look: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sheet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    peek: Option<String>,
}

/// A text box, pinned to char `at`; its text in one style.
#[derive(Serialize, Deserialize)]
struct BoxFile {
    at: usize,
    /// Top-left on the page and size (width, least height), in points.
    pos: [f32; 2],
    size: [f32; 2],
    text: String,
    /// Its style; `para` holds only its alignment.
    style: Run,
    #[serde(default)]
    look: crate::model::BoxLook,
    #[serde(default = "accent_ink")]
    color: [u8; 3],
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    behind: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    clear: bool,
}

fn accent_ink() -> [u8; 3] {
    crate::model::ACCENT_INK
}

#[derive(Serialize, Deserialize)]
pub struct DocFile {
    version: u32,
    #[serde(default)]
    setup: PageSetup,
    #[serde(default)]
    content: Vec<Run>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    notes: Vec<NoteFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    images: Vec<ImageFile>,
    /// Version 1 only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pages: Vec<Vec<Run>>,
    /// The scenes' description and whether they show; in older files also the one scene there was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<crate::backdrop::SceneFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    scenes: Vec<SceneEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    cast: Vec<CharacterEntry>,
    /// The cast peeks out now and then (`peek.rs`); on unless turned off.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    peeks: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    story_notes: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    scratchpad: String,
    /// Which story this is, to keep its versions together wherever the file goes (`versions.rs`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// When the story was begun, in seconds since 1970 (0 in older files: not known).
    #[serde(default, skip_serializing_if = "unknown")]
    pub created: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    boxes: Vec<BoxFile>,
    /// Where page numbering changes (`numbers.rs`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    numbers: Vec<crate::numbers::NumberMark>,
}

fn yes() -> bool {
    true
}

fn is_yes(b: &bool) -> bool {
    *b
}

fn unknown(when: &u64) -> bool {
    *when == 0
}

impl DocFile {
    /// How many words the story holds.
    pub fn words(&self) -> usize {
        self.content.iter().map(|r| r.text.as_str()).collect::<String>().split_whitespace().count()
    }
}

fn to_runs(text: &str, styles: &[Style]) -> Vec<Run> {
    let mut runs: Vec<(Style, String)> = Vec::new();
    for (ch, st) in text.chars().zip(styles) {
        match runs.last_mut() {
            Some((s, t)) if s == st => t.push(ch),
            _ => runs.push((st.clone(), ch.to_string())),
        }
    }
    runs.into_iter()
        .map(|(s, text)| Run {
            text,
            font: s.font.to_string(),
            size: s.size,
            bold: s.bold,
            italic: s.italic,
            underline: s.underline,
            color: s.color,
            para: s.para,
            image: s.image,
        })
        .collect()
}

fn run_style(r: &Run) -> Style {
    let (font, size, para, image) = (r.font.as_str().into(), r.size, r.para, r.image);
    Style { font, size, bold: r.bold, italic: r.italic, underline: r.underline, color: r.color, para, image }
}

fn from_runs(runs: Vec<Run>, text: &mut String, styles: &mut Vec<Style>) {
    for r in runs {
        styles.extend(std::iter::repeat_n(run_style(&r), r.text.chars().count()));
        text.push_str(&r.text);
    }
}

impl DocFile {
    pub fn from_doc(doc: &Doc) -> Self {
        let notes = doc
            .notes
            .iter()
            .map(|n| NoteFile { start: n.start, end: n.end, text: n.text.clone(), color: n.color })
            .collect();
        // Only pictures that are still in the text are kept.
        let used: BTreeSet<u32> = doc.flow.styles.iter().map(|s| s.image).filter(|&i| i != 0).collect();
        let images = doc
            .images
            .iter()
            .filter(|i| used.contains(&i.id))
            .map(|i| ImageFile { id: i.id, format: i.format.clone(), data: BASE64.encode(&i.bytes), width_pt: i.width_pt, rotation: i.rotation })
            .collect();
        // A scene whose first picture is still being painted has nothing to keep yet.
        let mut scenes: Vec<SceneEntry> = doc
            .scenes
            .iter()
            .filter(|s| !s.versions.is_empty())
            .map(|s| SceneEntry {
                at: s.at,
                versions: s.versions.clone(),
                shown: s.shown,
                subject: s.subject.clone(),
                hidden: s.hidden,
                people: s.people.clone(),
                chosen: s.chosen,
            })
            .collect();
        scenes.sort_by_key(|s| s.at);
        let boxes = doc
            .boxes
            .iter()
            .map(|b| BoxFile {
                at: b.at,
                pos: [b.pos.x, b.pos.y],
                size: [b.size.x, b.size.y],
                text: b.text.clone(),
                style: to_runs(" ", std::slice::from_ref(&b.style)).remove(0),
                look: b.look,
                color: b.color,
                behind: b.behind,
                clear: b.clear,
            })
            .collect();
        Self {
            version: 2,
            setup: doc.setup.clone(),
            content: to_runs(&doc.flow.text, &doc.flow.styles),
            notes,
            images,
            pages: Vec::new(),
            scene: None,
            scenes,
            cast: doc
                .cast
                .iter()
                .map(|c| CharacterEntry {
                    name: c.name.clone(),
                    aliases: c.aliases.clone(),
                    looks: c
                        .looks
                        .iter()
                        .map(|l| LookEntry { label: l.label.clone(), look: l.words.clone(), sheet: l.sheet.clone(), peek: l.peek.clone() })
                        .collect(),
                    active: c.active,
                    look: String::new(),
                    sheet: None,
                })
                .collect(),
            peeks: doc.peeks,
            story_notes: doc.story_notes.clone(),
            scratchpad: doc.scratchpad.clone(),
            id: String::new(),
            created: 0,
            boxes,
            numbers: doc.number_marks.clone(),
        }
    }

    pub fn into_doc(self) -> Doc {
        let mut text = String::new();
        let mut styles = Vec::new();
        from_runs(self.content, &mut text, &mut styles);
        for (k, page) in self.pages.into_iter().enumerate() {
            if k > 0 {
                let st = styles.last().cloned().unwrap_or_else(|| Style::new("Default"));
                text.push(PAGE_BREAK);
                styles.push(st);
            }
            from_runs(page, &mut text, &mut styles);
        }
        let mut doc = Doc::new();
        doc.flow = Flow { text, styles };
        doc.ensure_final_mark();
        doc.setup = self.setup;
        doc.setup.clamp_margins();
        // Before footers, page numbers were a setting of their own: they are a plain footer now.
        if std::mem::take(&mut doc.setup.page_numbers) && !doc.setup.footer.is_on() {
            doc.setup.footer = crate::model::Band::page_numbers();
        }
        doc.story_notes = self.story_notes;
        doc.scratchpad = self.scratchpad;
        doc.peeks = self.peeks;
        for mut c in self.cast {
            if c.looks.is_empty() {
                c.looks.push(LookEntry { label: String::new(), look: c.look, sheet: c.sheet, peek: None });
            }
            let id = doc.take_cast_id();
            let looks = c.looks.into_iter().map(|l| Look { id: doc.take_cast_id(), label: l.label, words: l.look, sheet: l.sheet, peek: l.peek });
            let looks = looks.collect::<Vec<_>>();
            let active = c.active.min(looks.len() - 1);
            doc.cast.push(Character { id, name: c.name, aliases: c.aliases, looks, active });
        }
        let removed = doc.take_out_contents_chars();
        for img in self.images {
            let Ok(bytes) = BASE64.decode(img.data.as_bytes()) else { continue };
            let Ok((format, px)) = crate::images::describe(&bytes) else { continue };
            doc.images.push(ImageData { id: img.id, format, bytes, px, width_pt: img.width_pt, rotation: img.rotation % 4 });
        }
        let total = doc.total_chars();
        let shift = |c: usize| c - removed.iter().filter(|&&r| r < c).count();
        for s in self.scenes.into_iter().filter(|s| !s.versions.is_empty()) {
            let id = doc.next_scene_id;
            doc.next_scene_id += 1;
            let shown = s.shown.min(s.versions.len() - 1);
            let at = shift(s.at).min(total - 1);
            doc.scenes.push(Scene { id, at, versions: s.versions, shown, subject: s.subject, hidden: s.hidden, people: s.people, chosen: s.chosen });
        }
        for b in self.boxes {
            let id = doc.next_box_id;
            doc.next_box_id += 1;
            let at = shift(b.at).min(total - 1);
            let size = egui::vec2(b.size[0], b.size[1]).max(egui::vec2(8.0, 8.0));
            let mut style = run_style(&b.style);
            style.image = 0;
            let pos = egui::vec2(b.pos[0], b.pos[1]);
            doc.boxes.push(crate::model::TextBox { id, at, pos, size, text: b.text, style, look: b.look, color: b.color, behind: b.behind, clear: b.clear });
        }
        for m in self.numbers {
            doc.number_marks.push(crate::numbers::NumberMark { at: shift(m.at).min(total - 1), ..m });
        }
        doc.number_marks.sort_by_key(|m| m.at);
        for n in self.notes {
            let (start, end) = (shift(n.start).min(total), shift(n.end).min(total));
            let id = doc.next_note_id;
            doc.next_note_id += 1;
            doc.notes.push(Note { id, start: start.min(end), end, text: n.text, color: n.color });
        }
        doc
    }
}

/// What an open file dialog is choosing a path for.
pub enum DialogFor {
    Open,
    Save,
    ExportDocx,
    PrintPdf,
    Picture,
    ScenePicture,
}

/// Something that would throw away unsaved changes, waiting for the user to say what to do.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Leaving {
    /// Opening another document.
    Open,
    /// Closing the window.
    Close,
}

/// A file dialog showing on its own thread.
pub struct PendingDialog {
    purpose: DialogFor,
    rx: Receiver<Option<PathBuf>>,
}

impl App {
    /// The document as saved, with the scene behind its pages.
    pub fn doc_file(&self) -> DocFile {
        DocFile { scene: self.backdrop.to_file(), id: self.story_id.clone(), created: self.created, ..DocFile::from_doc(&self.doc) }
    }

    /// A fingerprint of what saving would write, to tell whether anything changed since.
    pub fn content_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_string(&self.doc_file()).unwrap_or_default().hash(&mut h);
        h.finish()
    }

    /// Does the document hold anything not yet saved?
    pub fn unsaved(&self) -> bool {
        self.content_hash() != self.saved_hash
    }

    pub fn save(&mut self, ctx: &egui::Context, save_as: bool) {
        match (&self.path, save_as) {
            (Some(p), false) => self.save_to(p.clone()),
            _ => {
                // Named after its first chapter or words, until it has a name of its own.
                let name = match &self.path {
                    Some(p) => p.file_name().map_or("Untitled.caprice".into(), |n| n.to_string_lossy().into_owned()),
                    None => format!("{}.caprice", self.suggested_name()),
                };
                self.ask_path(ctx, DialogFor::Save, move || {
                    rfd::FileDialog::new().add_filter("Caprice document", &["caprice"]).set_file_name(name).save_file()
                });
            }
        }
    }

    pub fn save_to(&mut self, path: PathBuf) {
        let json = serde_json::to_string_pretty(&self.doc_file());
        let written = json.map_err(|e| e.to_string()).and_then(|j| std::fs::write(&path, j).map_err(|e| e.to_string()));
        self.status = match written {
            Ok(()) => {
                self.start.remember(&path, self.keep.now());
                self.path = Some(path);
                self.saved_hash = self.content_hash();
                self.as_saved = serde_json::to_string(&self.doc_file()).ok().map(|j| (self.saved_hash, j));
                self.saved_at = Some(self.keep.now());
                "- saved".into()
            }
            Err(e) => format!("- save failed: {e}"),
        };
    }

    /// Write the document as a Word file (`.docx`) next to wherever the user chooses.
    pub fn export_docx(&mut self, ctx: &egui::Context) {
        let stem = self
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map_or("Untitled".to_owned(), |s| s.to_string_lossy().into_owned());
        self.ask_path(ctx, DialogFor::ExportDocx, move || {
            rfd::FileDialog::new()
                .add_filter("Word document", &["docx"])
                .set_file_name(format!("{stem}.docx"))
                .save_file()
        });
    }

    pub(crate) fn export_docx_to(&mut self, path: PathBuf) {
        let written = crate::export::to_docx(&self.doc).and_then(|b| std::fs::write(&path, b).map_err(|e| e.to_string()));
        self.status = match written {
            Ok(()) => format!("- exported {}", path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned())),
            Err(e) => format!("- export failed: {e}"),
        };
    }

    /// Choose another document to open, first asking about unsaved changes.
    pub fn open(&mut self, ctx: &egui::Context) {
        self.save_before_leaving();
        if self.unsaved() || self.changed_since_saved() {
            self.leaving = Some(Leaving::Open);
        } else {
            self.choose_file_to_open(ctx);
        }
    }

    fn choose_file_to_open(&mut self, ctx: &egui::Context) {
        self.ask_path(ctx, DialogFor::Open, || {
            rfd::FileDialog::new().add_filter("Caprice document", &["caprice"]).pick_file()
        });
    }

    /// Show a file dialog without holding up the frames: the window keeps drawing (and answering
    /// the compositor) while it is open, and the chosen path is acted on in `poll_dialog`.
    pub fn ask_path(&mut self, ctx: &egui::Context, purpose: DialogFor, show: impl FnOnce() -> Option<PathBuf> + Send + 'static) {
        if self.dialog.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        crate::web::spawn(move || {
            let _ = tx.send(show());
            ctx.request_repaint();
        });
        self.dialog = Some(PendingDialog { purpose, rx });
    }

    /// Act on the path from a file dialog once the user has chosen it.
    pub fn poll_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.dialog else { return };
        let path = match pending.rx.try_recv() {
            Ok(path) => path,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => None,
        };
        let Some(PendingDialog { purpose, .. }) = self.dialog.take() else { return };
        let Some(path) = path else {
            self.after_save = None;
            return;
        };
        match purpose {
            DialogFor::Open => self.open_path(ctx, path),
            DialogFor::Save => {
                self.save_to(path);
                if let Some(next) = self.after_save.take()
                    && !self.unsaved()
                {
                    self.leave(ctx, next);
                }
            }
            DialogFor::ExportDocx => self.export_docx_to(path),
            DialogFor::PrintPdf => self.print_pdf_to(ctx, path),
            DialogFor::Picture => {
                self.status = match self.insert_image(ctx, &path) {
                    Ok(()) => "- picture added".into(),
                    Err(e) => format!("- picture failed: {e}"),
                };
            }
            DialogFor::ScenePicture => self.save_scene_picture_to(path),
        }
    }

    /// Go on with what was waiting on the unsaved-changes question.
    pub(crate) fn leave(&mut self, ctx: &egui::Context, leaving: Leaving) {
        match leaving {
            Leaving::Open => self.choose_file_to_open(ctx),
            Leaving::Close => {
                self.may_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// Hold the window open while it has unsaved changes, and ask instead.
    pub fn guard_close(&mut self, ctx: &egui::Context) {
        let closing = ctx.input(|i| i.viewport().close_requested());
        if closing {
            self.save_before_leaving();
        }
        if closing && !self.may_close && (self.unsaved() || self.changed_since_saved()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.leaving = Some(Leaving::Close);
        }
    }

    pub fn open_path(&mut self, ctx: &egui::Context, path: std::path::PathBuf) {
        let loaded = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<DocFile>(&s).map_err(|e| e.to_string()));
        match loaded {
            Ok(file) => {
                self.story_id = if file.id.is_empty() { crate::versions::new_id() } else { file.id.clone() };
                // Older files say nothing: the file's own date is the best guess (written with the next save).
                let meta = std::fs::metadata(&path).ok();
                let made = meta.and_then(|m| m.created().or_else(|_| m.modified()).ok());
                let made = made.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                self.created = if file.created > 0 { file.created } else { made };
                self.adopt(ctx, file);
                self.start.remember(&path, self.keep.now());
                self.start.leave(ctx.input(|i| i.time));
                self.path = Some(path);
                self.status = "- opened".into();
                self.saved_hash = self.content_hash();
                self.as_saved = serde_json::to_string(&self.doc_file()).ok().map(|j| (self.saved_hash, j));
                self.saved_at = Some(self.keep.now());
                self.keep.opened(&self.story_id);
            }
            Err(e) => self.status = format!("- open failed: {e}"),
        }
    }

    /// Show the story in `file` in place of the one shown, from its first page.
    pub fn adopt(&mut self, ctx: &egui::Context, mut file: DocFile) {
        let scene = file.scene.take();
        let mut doc = file.into_doc();
        doc.version = self.doc.version + 1;
        self.pads.clear();
        self.search.matches.clear();
        let fonts: BTreeSet<String> = doc.flow.styles.iter().map(|s| s.font.to_string()).collect();
        for f in fonts {
            self.fonts.ensure(ctx, &f, false);
        }
        if let Some(st) = doc.flow.styles.first() {
            self.typing = st.clone();
        }
        self.doc = doc;
        self.textures.clear();
        self.ensure_textures(ctx);
        self.doc.full_paginate(ctx, &self.typing);
        self.pos = 0.0;
        self.target = 0;
        self.set_caret(ctx, 0, false);
        self.fit = true;
        self.open_scene(scene);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_keeps_text_styles_breaks_and_setup() {
        let bold = Style { bold: true, ..Style::new("Foo") };
        let plain = Style::new("Foo");
        let mut doc = Doc::new();
        doc.flow.text = format!("hi{PAGE_BREAK}yo\n");
        doc.flow.styles = vec![bold.clone(), bold.clone(), plain.clone(), plain.clone(), plain.clone(), plain.clone()];
        doc.setup.margin_left = 40.0;
        doc.setup.footer = crate::model::Band { look: crate::model::BandLook::Filigree, text: "{page}".into(), color: [1, 2, 3] };
        doc.notes.push(Note { id: 7, start: 1, end: 4, text: "check this".into(), color: 2 });
        let json = serde_json::to_string(&DocFile::from_doc(&doc)).unwrap();
        let back = serde_json::from_str::<DocFile>(&json).unwrap().into_doc();
        assert_eq!(back.flow.text, doc.flow.text);
        assert_eq!(back.flow.styles, doc.flow.styles);
        assert_eq!(back.setup, doc.setup);
        assert_eq!(back.notes.len(), 1);
        assert_eq!((back.notes[0].start, back.notes[0].end, back.notes[0].color), (1, 4, 2));
        assert_eq!(back.notes[0].text, "check this");
    }

    #[test]
    fn chapter_titles_are_saved_and_older_files_have_none() {
        let plain = Style::new("Foo");
        let mut title = ParaAttrs::default();
        title.set_chapter_title(true);
        let mut doc = Doc::new();
        doc.flow.text = "One\ntext\n".into();
        doc.flow.styles = vec![plain.clone(); 9];
        doc.flow.styles[3] = plain.with_para(title);
        let json = serde_json::to_string(&DocFile::from_doc(&doc)).unwrap();
        assert_eq!(json.matches("ChapterTitle").count(), 1, "ordinary paragraphs say nothing about it: {json}");
        let back = serde_json::from_str::<DocFile>(&json).unwrap().into_doc();
        assert_eq!(back.flow.styles, doc.flow.styles);

        let old = json.replace(r#","kind":"ChapterTitle""#, "");
        let back = serde_json::from_str::<DocFile>(&old).unwrap().into_doc();
        assert!(!back.para_attrs_at(0).is_chapter_title());
    }

    #[test]
    fn files_with_the_old_contents_char_open_with_the_setting_and_post_its_and_scenes_in_place() {
        let st = Style::new("Foo");
        let mut doc = Doc::new();
        doc.flow.text = "\u{e000}\u{c}One\ntext\n".into();
        doc.flow.styles = vec![st; 11];
        doc.notes.push(Note { id: 1, start: 2, end: 5, text: "the title".into(), color: 0 });
        doc.scenes.push(Scene { id: 1, at: 6, versions: vec!["<svg/>".into()], shown: 0, subject: String::new(), hidden: false, people: Vec::new(), chosen: false });
        let json = serde_json::to_string(&DocFile::from_doc(&doc)).unwrap();
        let back = serde_json::from_str::<DocFile>(&json).unwrap().into_doc();
        assert_eq!(back.flow.text, "One\ntext\n");
        assert!(back.setup.contents);
        assert_eq!((back.notes[0].start, back.notes[0].end), (0, 3), "still on \"One\"");
        assert_eq!(back.scenes[0].at, 4, "the scene still on \"text\"");
        // Saved again, the setting stays, and the char does not come back.
        let json = serde_json::to_string(&DocFile::from_doc(&back)).unwrap();
        let again = serde_json::from_str::<DocFile>(&json).unwrap().into_doc();
        assert_eq!((again.flow.text.as_str(), again.setup.contents), ("One\ntext\n", true));
    }

    #[test]
    fn people_from_files_before_looks_open_with_their_one_look_in_use() {
        let json = r#"{"version":2,"content":[{"text":"Mara\n","font":"F","size":12.0,"bold":false,"underline":false}],
                       "cast":[{"name":"Mara","aliases":"Grandma","look":"grey braid","sheet":"<svg/>"}]}"#;
        let doc = serde_json::from_str::<DocFile>(json).unwrap().into_doc();
        let mara = &doc.cast[0];
        assert_eq!((mara.name.as_str(), mara.aliases.as_str(), mara.looks.len()), ("Mara", "Grandma", 1));
        assert_eq!((mara.look().words.as_str(), mara.look().sheet.as_deref()), ("grey braid", Some("<svg/>")));
        let saved = serde_json::to_string(&DocFile::from_doc(&doc)).unwrap();
        assert!(saved.contains(r#""looks":[{"look":"grey braid","sheet":"<svg/>"}],"active":0"#), "{saved}");
    }

    #[test]
    fn a_peek_and_whether_the_cast_peeks_are_saved_and_older_files_peek() {
        let json = r#"{"version":2,"content":[{"text":"Mara\n","font":"F","size":12.0,"bold":false,"underline":false}],
                       "cast":[{"name":"Mara","looks":[{"look":"grey braid","sheet":"<svg/>"}]}]}"#;
        let mut doc = serde_json::from_str::<DocFile>(json).unwrap().into_doc();
        assert!(doc.peeks && doc.cast[0].look().peek.is_none(), "older files: on, and no one drawn peeking yet");
        let saved = serde_json::to_string(&DocFile::from_doc(&doc)).unwrap();
        assert!(!saved.contains("peek"), "nothing written while unused: {saved}");

        (doc.cast[0].looks[0].peek, doc.peeks) = (Some("<svg id='peek'/>".into()), false);
        let saved = serde_json::to_string(&DocFile::from_doc(&doc)).unwrap();
        let again = serde_json::from_str::<DocFile>(&saved).unwrap().into_doc();
        assert_eq!(again.cast[0].look().peek.as_deref(), Some("<svg id='peek'/>"));
        assert!(!again.peeks, "turned off stays off");
    }

    #[test]
    fn version_1_files_still_load_as_separate_pages() {
        let json = r#"{"version":1,"pages":[[{"text":"one","font":"F","size":12.0,"bold":false,"underline":false}],
                                             [{"text":"two","font":"F","size":12.0,"bold":true,"underline":false}]]}"#;
        let doc = serde_json::from_str::<DocFile>(json).unwrap().into_doc();
        assert_eq!(doc.flow.text, format!("one{PAGE_BREAK}two\n"));
        assert_eq!(doc.flow.styles.len(), 8);
    }

    #[test]
    fn files_from_before_footers_keep_their_page_numbers_as_a_footer() {
        let json = r#"{"version":2,"setup":{"page_numbers":true},"content":[{"text":"one\n","font":"F","size":12.0,"bold":false,"underline":false}]}"#;
        let doc = serde_json::from_str::<DocFile>(json).unwrap().into_doc();
        assert_eq!(doc.setup.footer, crate::model::Band::page_numbers());
        assert!(!doc.setup.page_numbers);
        let saved = serde_json::to_string(&DocFile::from_doc(&doc)).unwrap();
        assert!(!saved.contains("page_numbers") && saved.contains(r#""text":"{page}""#), "{saved}");
        // Without them, no footer.
        let json = r#"{"version":2,"content":[{"text":"one\n","font":"F","size":12.0,"bold":false,"underline":false}]}"#;
        assert!(!serde_json::from_str::<DocFile>(json).unwrap().into_doc().setup.footer.is_on());
    }
}
