//! The welcome page: Caprice opened without a file shows it in place of the desk. It begins a new
//! story of one of a few kinds (a blank page, a novel, a manuscript, a journal: each with its own
//! paper, type and headers and footers), opens one found with the file dialog, or one of the
//! stories last opened, told about with what it holds. The sheet chosen then flies to where the
//! page lies on the desk while the welcome fades from over it.
//!
//! The stories opened or saved lately are a list of paths, with when each was last opened, in the
//! app's data folder beside the versions; the newest first. A file moved or deleted since is left
//! out of the page.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::text::{LayoutJob, TextWrapping};
use eframe::egui::{self, Color32, CornerRadius, FontId, Galley, Id, LayerId, Order, Rect, Sense, TextFormat, Ui, UiBuilder, Vec2, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::App;
use crate::fileio::DocFile;
use crate::fonts::{FontBook, family_for};
use crate::model::{Align, Band, BandLook, CHAPTER_FIELD, Doc, Flow, PAGE_FIELD, PageSetup, ParaAttrs, ParaKind, Style};
use crate::theme::{self, INK, PAPER, pal};

/// How many stories the list remembers.
const REMEMBER: usize = 8;
/// How long (seconds) the sheet chosen takes to fly to the desk.
pub const FLIGHT: f64 = 1.1;
/// The widest the page's column grows, and the room between the cards.
const COLUMN: f32 = 940.0;
const GAP: f32 = 18.0;

/// A story opened lately.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lately {
    pub path: PathBuf,
    /// When it was last opened (or first saved), in seconds since 1970.
    pub opened: u64,
}

/// What the welcome page tells about a story last opened, read from its file once.
#[derive(Clone, Debug)]
pub struct Story {
    pub path: PathBuf,
    pub name: String,
    pub opened: u64,
    /// When the file was last written, if the system says.
    pub changed: Option<u64>,
    pub words: usize,
    pub chapters: usize,
    pub notes: usize,
    /// The first words of its text, to know it by.
    pub excerpt: String,
    pub paper: String,
    sheet: Sheet,
}

/// The kinds of story the welcome page begins: each its own paper, type and furniture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Blank,
    Novel,
    Manuscript,
    Journal,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Blank, Kind::Novel, Kind::Manuscript, Kind::Journal];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Blank => "Blank page",
            Kind::Novel => "Novel",
            Kind::Manuscript => "Manuscript",
            Kind::Journal => "Journal",
        }
    }

    /// What it sets up, in a few words.
    fn about(self) -> &'static str {
        match self {
            Kind::Blank => "A4, as Caprice always began.",
            Kind::Novel => "Book pages, 6 \u{d7} 9 in. Justified, chapters with drop caps, numbered.",
            Kind::Manuscript => "A4 in typewriter type, double spaced, to send out.",
            Kind::Journal => "A5 with airy lines, today\u{2019}s date as the first title.",
        }
    }

    /// The fonts it is written in, best first: the first one offered is taken, else the usual.
    fn fonts(self) -> &'static [&'static str] {
        match self {
            Kind::Blank => &[],
            Kind::Novel => &[
                "Garamond", "Palatino Linotype", "Georgia", "Cambria", "Times New Roman", "Liberation Serif", "Noto Serif",
                "DejaVu Serif", "Tinos",
            ],
            Kind::Manuscript => &["Courier New", "Liberation Mono", "Cousine", "Nimbus Mono PS", "Times New Roman", "Liberation Serif"],
            Kind::Journal => &["Helvetica", "Arial", "Noto Sans", "Liberation Sans", "DejaVu Sans", "Arimo", "Verdana"],
        }
    }

    pub fn font(self, fonts: &FontBook) -> String {
        let offered = |f: &&&str| fonts.families.iter().any(|(n, _)| n == **f);
        self.fonts().iter().find(offered).map_or_else(|| fonts.preferred_default(), |f| (*f).to_owned())
    }

    pub fn setup(self) -> PageSetup {
        let band = |look, text: &str| Band { look, text: text.into(), ..Band::default() };
        match self {
            Kind::Blank => PageSetup::default(),
            Kind::Novel => PageSetup {
                paper_w: 432.0,
                paper_h: 648.0,
                margin_top: 58.0,
                margin_bottom: 58.0,
                margin_left: 54.0,
                margin_right: 54.0,
                header: band(BandLook::Facing, CHAPTER_FIELD),
                footer: band(BandLook::Plain, PAGE_FIELD),
                drop_cap_lines: 3,
                ..PageSetup::default()
            },
            Kind::Manuscript => PageSetup { header: band(BandLook::Plain, PAGE_FIELD), ..PageSetup::default() },
            Kind::Journal => PageSetup {
                paper_w: 420.0,
                paper_h: 595.0,
                margin_top: 50.0,
                margin_bottom: 56.0,
                margin_left: 50.0,
                margin_right: 50.0,
                footer: band(BandLook::Circle, PAGE_FIELD),
                ..PageSetup::default()
            },
        }
    }

    /// The look its story is written in.
    pub fn body(self, font: &str) -> Style {
        let (size, align, spacing) = match self {
            Kind::Blank => (crate::model::FONT_SIZE, Align::Left, 1.0),
            Kind::Novel => (11.0, Align::Justify, 1.15),
            Kind::Manuscript => (12.0, Align::Left, 2.0),
            Kind::Journal => (11.0, Align::Left, 1.35),
        };
        let para = ParaAttrs { align, spacing, ..ParaAttrs::default() };
        Style { size, para, ..Style::new(font) }
    }

    /// The chapter title it begins with, if any.
    pub fn title(self, now: u64) -> Option<String> {
        match self {
            Kind::Novel => Some("Chapter One".into()),
            Kind::Journal => crate::words::date(now).split(',').next().map(str::to_owned),
            Kind::Blank | Kind::Manuscript => None,
        }
    }

    fn sheet(self) -> Sheet {
        let body = self.body("").para;
        Sheet::of(&self.setup(), self.title(0).is_some(), &body, self as u64 + 1)
    }
}

/// What a small sheet shows of a page: its shape and how its text lies, not the words.
#[derive(Clone, Debug)]
pub(crate) struct Sheet {
    size: Vec2,
    /// Left, top, right, bottom, in points like the page.
    margins: [f32; 4],
    title: bool,
    drop_cap: bool,
    justify: bool,
    spacing: f32,
    header: BandLook,
    footer: BandLook,
    /// Where its paragraphs end, so two sheets do not look the same.
    seed: u64,
    /// How much of the page the text fills, 0 to 1: a story only begun shows a few lines.
    fill: f32,
}

impl Sheet {
    fn of(setup: &PageSetup, title: bool, body: &ParaAttrs, seed: u64) -> Self {
        Self {
            size: setup.size(),
            margins: [setup.margin_left, setup.margin_top, setup.margin_right, setup.margin_bottom],
            title,
            drop_cap: title && setup.drop_cap_lines > 0,
            justify: body.align == Align::Justify,
            spacing: body.spacing,
            header: setup.header.look,
            footer: setup.footer.look,
            seed,
            fill: 1.0,
        }
    }

    /// The sheet `h` points high, its width as the page's shape says.
    pub(crate) fn width_for(&self, h: f32) -> f32 {
        h * self.size.x / self.size.y
    }

    /// Draw it into `rect`, lifted `lift` (0 to 1) off the desk, its lines at `lines` strength.
    pub(crate) fn draw(&self, painter: &egui::Painter, rect: Rect, lift: f32, lines: f32) {
        let shadow = egui::epaint::Shadow {
            offset: [0, (3.0 + 5.0 * lift) as i8],
            blur: (10.0 + 12.0 * lift) as u8,
            spread: 0,
            color: Color32::from_black_alpha((70.0 + 50.0 * lift) as u8),
        };
        painter.add(shadow.as_shape(rect, CornerRadius::same(2)));
        painter.rect_filled(rect, CornerRadius::same(2), PAPER);
        if lines <= 0.0 {
            return;
        }
        let s = rect.width() / self.size.x;
        let [l, t, r, b] = self.margins.map(|m| m * s);
        let text = Rect::from_min_max(rect.min + vec2(l, t), rect.max - vec2(r, b));
        let ink = |a: f32| INK.gamma_multiply(a * lines);
        let pitch = (13.0 * self.spacing * s).max(2.6);
        let bar = (pitch / self.spacing * 0.45).clamp(0.7, 3.0);
        let line = |x0: f32, x1: f32, y: f32, a: f32| {
            painter.rect_filled(Rect::from_min_max(pos2(x0, y), pos2(x1, y + bar)), 0.0, ink(a));
        };

        // Headers and footers, in the margins.
        for (look, y) in [(self.header, rect.top() + t * 0.5), (self.footer, rect.bottom() - b * 0.5)] {
            match look {
                BandLook::None => {}
                BandLook::Circle => {
                    let accent = Color32::from_rgb(86, 124, 164).gamma_multiply(lines);
                    painter.circle_filled(pos2(text.center().x, y), (bar * 2.2).max(2.0), accent);
                }
                BandLook::Facing => line(text.right() - text.width() * 0.3, text.right(), y - bar, 0.35),
                _ => line(text.center().x - text.width() * 0.08, text.center().x + text.width() * 0.08, y - bar, 0.35),
            }
        }

        let mut y = text.top();
        let bottom = text.top() + text.height() * self.fill;
        if self.title {
            let h = (bar * 2.0).max(1.4);
            let w = text.width() * 0.42;
            let x = if self.justify { text.center().x - w / 2.0 } else { text.left() };
            painter.rect_filled(Rect::from_min_size(pos2(x, y), vec2(w, h)), 0.0, ink(0.75));
            y += h + pitch * 2.2;
        }
        // Paragraphs of a few lines each, their lengths from the seed.
        let mut rng = self.seed;
        let mut next = |n: u64| {
            rng = rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (rng >> 33) % n
        };
        let mut first = true;
        while y + bar < bottom {
            let rows = 3 + next(6) as usize;
            for k in 0..rows {
                if y + bar > bottom {
                    break;
                }
                let mut x0 = text.left();
                if first && self.drop_cap && k < 3 {
                    if k == 0 {
                        let cap = pitch * 2.0 + bar;
                        painter.rect_filled(Rect::from_min_size(pos2(x0, y), vec2(cap * 0.8, cap)), 0.0, ink(0.65));
                    }
                    x0 += pitch * 2.0 + bar;
                } else if k == 0 && !first {
                    x0 += pitch * 1.2;
                }
                let full = if self.justify { 1.0 } else { 0.86 + next(14) as f32 / 100.0 };
                let w = if k + 1 == rows { 0.25 + next(55) as f32 / 100.0 } else { full };
                line(x0, x0 + (text.right() - x0) * w, y, 0.32);
                y += pitch;
            }
            first = false;
        }
    }
}

/// The paper's usual name, or its size in millimetres.
fn paper_name(setup: &PageSetup) -> String {
    let named = [("A4", 595.0, 842.0), ("A5", 420.0, 595.0), ("Letter", 612.0, 792.0), ("6 \u{d7} 9 in", 432.0, 648.0)];
    let near = |a: f32, b: f32| (a - b).abs() < 2.0;
    let name = named.iter().find(|(_, w, h)| near(setup.paper_w, *w) && near(setup.paper_h, *h)).map(|(n, ..)| (*n).to_owned());
    let mm = |p: f32| (p * 25.4 / 72.0).round();
    let name = name.unwrap_or_else(|| format!("{} \u{d7} {} mm", mm(setup.paper_w), mm(setup.paper_h)));
    match setup.orientation {
        crate::model::Orientation::Portrait => name,
        crate::model::Orientation::Landscape => format!("{name}, landscape"),
    }
}

/// Read what the welcome page tells about a story from its file.
/// A story's paragraphs, each with its terminator's style; pictures left out.
fn paragraphs(doc: &Doc) -> Vec<(String, &Style)> {
    let mut paras = Vec::new();
    let mut words = String::new();
    for (c, st) in doc.flow.text.chars().zip(&doc.flow.styles) {
        if crate::model::is_terminator(c) {
            paras.push((std::mem::take(&mut words), st));
        } else if st.image == 0 {
            words.push(c);
        }
    }
    paras
}

/// Its first paragraph of story with words in it.
fn first_body<'a>(paras: &'a [(String, &'a Style)]) -> Option<&'a (String, &'a Style)> {
    paras.iter().find(|(t, st)| st.para.kind == ParaKind::Body && !t.trim().is_empty())
}

impl Sheet {
    /// The first page of `doc`, as much of it filled as its words would fill.
    pub(crate) fn of_doc(doc: &Doc, seed: u64) -> Self {
        let paras = paragraphs(doc);
        let title = paras.first().is_some_and(|(_, st)| st.para.kind == ParaKind::ChapterTitle);
        let body = first_body(&paras);
        let para = body.map_or(ParaAttrs::default(), |(_, st)| st.para);
        let words = doc.words_in(0, doc.total_chars());
        // About how many words a page of it holds: its lines, each of some six letters a word.
        let size = body.map_or(crate::model::FONT_SIZE, |(_, st)| st.size);
        let room = doc.setup.content_size();
        let per_page = (room.y / (size * 1.2 * para.spacing)) * (room.x / (size * 0.5 * 6.0));
        Sheet { fill: (words as f32 / per_page.max(1.0)).min(1.0), ..Sheet::of(&doc.setup, title, &para, seed) }
    }
}

/// Read what the welcome page tells about a story from its file.
fn read_story(l: &Lately) -> Option<Story> {
    let json = std::fs::read_to_string(&l.path).ok()?;
    let doc: Doc = serde_json::from_str::<DocFile>(&json).ok()?.into_doc();
    let changed = std::fs::metadata(&l.path).and_then(|m| m.modified()).ok();
    let changed = changed.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs());
    let paras = paragraphs(&doc);
    let excerpt = first_body(&paras).map_or(String::new(), |(t, _)| t.split_whitespace().take(60).collect::<Vec<_>>().join(" "));
    let seed = l.path.to_string_lossy().bytes().fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(b.into()));
    Some(Story {
        path: l.path.clone(),
        name: l.path.file_stem().map_or(String::new(), |n| n.to_string_lossy().into_owned()),
        opened: l.opened,
        changed,
        words: doc.words_in(0, doc.total_chars()),
        chapters: doc.chapters(false).len(),
        notes: doc.notes.len(),
        excerpt,
        paper: paper_name(&doc.setup),
        sheet: Sheet::of_doc(&doc, seed),
    })
}

/// The sheet chosen, flying from where it lay on the welcome page to the page on the desk.
pub struct Flight {
    /// Where it lay (None: opened from the dialog; the welcome only fades).
    from: Option<(Rect, Sheet)>,
    /// When it set off, in frame time.
    at: f64,
}

/// The welcome page, and the stories opened lately.
pub struct Start {
    /// The welcome page shows (Caprice was opened without a file).
    pub open: bool,
    /// Where the list is kept (None: only while Caprice runs).
    pub file: Option<PathBuf>,
    /// The stories opened or saved lately, newest first.
    pub recent: Vec<Lately>,
    /// What the page tells about them, read once when it first shows.
    pub stories: Option<Vec<Story>>,
    /// The welcome giving way to the desk.
    pub flight: Option<Flight>,
    /// When it first showed, in frame time (the emblem's clock).
    shown_at: Option<f64>,
}

impl Start {
    pub fn new() -> Self {
        Self::kept_in(crate::versions::default_dir().and_then(|d| d.parent().map(|p| p.join("recent.json"))))
    }

    /// The list kept in `file`.
    pub fn kept_in(file: Option<PathBuf>) -> Self {
        let recent = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|j| serde_json::from_str(&j).ok());
        Self { open: false, file, recent: recent.unwrap_or_default(), stories: None, flight: None, shown_at: None }
    }

    /// A story was opened or saved, at `now`: it goes to the top of the list.
    pub fn remember(&mut self, path: &Path, now: u64) {
        // The full path, so a story opened from the command line is the same as from the dialog.
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        // Autosave saves often: saving the story already on top changes nothing.
        if self.recent.first().is_some_and(|l| l.path == path) {
            return;
        }
        self.recent.retain(|l| l.path != path);
        self.recent.insert(0, Lately { path, opened: now });
        self.recent.truncate(REMEMBER);
        if let (Some(file), Ok(json)) = (&self.file, serde_json::to_string(&self.recent)) {
            let _ = file.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(file, json);
        }
    }

    /// The welcome is going: it fades from over the desk from `at` (frame time).
    pub fn leave(&mut self, at: f64) {
        if self.open {
            self.open = false;
            self.flight = Some(Flight { from: None, at });
        }
    }
}

/// "~/Writing" for a folder in the home folder, to read at a glance.
fn folder_name(path: &Path) -> String {
    let folder = path.parent().unwrap_or(path);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.as_ref().and_then(|h| folder.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => folder.display().to_string(),
    }
}

/// "Good morning" and so on, by the writer's clock.
fn greeting(now: u64) -> &'static str {
    let local = now as i64 + crate::words::offset();
    match local.rem_euclid(86_400) / 3600 {
        5..=11 => "Good morning.",
        12..=17 => "Good afternoon.",
        18..=22 => "Good evening.",
        _ => "Writing late?",
    }
}

fn ease(s: f32) -> f32 {
    s * s * (3.0 - 2.0 * s)
}

/// Text laid out `width` wide in at most `rows` rows, cut short with an ellipsis.
fn galley(ui: &Ui, text: impl Into<String>, font: FontId, color: Color32, width: f32, rows: usize, italics: bool) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(text.into(), TextFormat { font_id: font, color, italics, ..Default::default() });
    job.wrap = TextWrapping { max_width: width, max_rows: rows, break_anywhere: false, overflow_character: Some('\u{2026}') };
    ui.painter().layout_job(job)
}

/// "12,345 words · 8 chapters · 3 post-its · A5".
fn holds(st: &Story) -> String {
    let n = |k: usize, one: &str, many: &str| format!("{} {}", crate::words::grouped(k), if k == 1 { one } else { many });
    let mut parts = vec![n(st.words, "word", "words")];
    if st.chapters > 0 {
        parts.push(n(st.chapters, "chapter", "chapters"));
    }
    if st.notes > 0 {
        parts.push(n(st.notes, "post-it", "post-its"));
    }
    parts.push(st.paper.clone());
    parts.join("  \u{b7}  ")
}

/// What was chosen on the welcome page this frame.
enum Chosen {
    Begin(Kind, Rect),
    Browse,
    Story(PathBuf, Rect),
}

impl App {
    /// Begin a new story of `kind` in place of the one shown (an empty one, on the welcome page).
    pub fn begin(&mut self, ctx: &egui::Context, kind: Kind) {
        let now = self.keep.now();
        let font = kind.font(&self.fonts);
        let body = kind.body(&font);
        let (mut text, mut styles) = (String::new(), Vec::new());
        if let Some(title) = kind.title(now) {
            let align = if kind == Kind::Novel { Align::Center } else { Align::Left };
            let para = ParaAttrs { kind: ParaKind::ChapterTitle, align, ..body.para };
            text.push_str(&title);
            styles.extend(title.chars().map(|_| body.clone()));
            text.push('\n');
            styles.push(body.with_para(para));
        }
        text.push('\n');
        styles.push(body.clone());
        let mut doc = Doc::new();
        let caret = text.chars().count() - 1;
        (doc.flow, doc.setup) = (Flow { text, styles }, kind.setup());
        self.adopt(ctx, DocFile::from_doc(&doc));
        self.set_caret(ctx, caret, false);
        self.typing = body;
        (self.path, self.story_id, self.created, self.as_saved) = (None, crate::versions::new_id(), now, None);
        // What it begins with is no change to save.
        self.saved_hash = self.content_hash();
        self.keep.opened(&self.story_id);
    }

    /// The welcome page, in place of the desk.
    pub fn welcome(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if self.start.stories.is_none() {
            self.start.stories = Some(self.start.recent.iter().filter_map(read_story).collect());
        }
        let now = ctx.input(|i| i.time);
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.begin(&ctx, Kind::Blank);
            self.start.leave(now);
            return;
        }
        match self.welcome_page(ui, true) {
            Some(Chosen::Begin(kind, rect)) => {
                self.begin(&ctx, kind);
                self.start.open = false;
                self.start.flight = Some(Flight { from: Some((rect, kind.sheet())), at: now });
            }
            Some(Chosen::Browse) => self.open(&ctx),
            Some(Chosen::Story(path, rect)) => {
                // A story that cannot be read says so and leaves the page up to choose another.
                self.open_path(&ctx, path.clone());
                let sheet = self.start.stories.iter().flatten().find(|s| s.path == path).map(|s| s.sheet.clone());
                if let (Some(f), Some(sheet)) = (&mut self.start.flight, sheet) {
                    f.from = Some((rect, sheet));
                }
            }
            None => {}
        }
    }

    /// Draw the welcome page over `ui`; what was chosen, when `live`.
    fn welcome_page(&mut self, ui: &mut Ui, live: bool) -> Option<Chosen> {
        let area = ui.max_rect();
        theme::paint_desk(ui.painter(), area);
        let now = self.keep.now();
        // The name in the novel's type, once it is there (it comes the next frame, `FontBook::ensure`).
        let serif = Kind::Novel.font(&self.fonts);
        self.fonts.ensure(ui.ctx(), &serif, false);
        let serif = family_for(&serif, false, false).0;
        let words_today = self.words_today();
        let waiting = self.dialog.is_some() || !live;
        let stories = self.start.stories.clone().unwrap_or_default();
        let sense = if live { Sense::click() } else { Sense::hover() };
        let mut chosen = None;
        // The emblem's pencil draws from when the page first showed, and keeps the frames coming.
        let clock = ui.ctx().input(|i| i.time);
        let since = (clock - *self.start.shown_at.get_or_insert(clock)) as f32;
        ui.ctx().request_repaint();
        let mut child = ui.new_child(UiBuilder::new().max_rect(area));
        egui::ScrollArea::vertical().id_salt("welcome").auto_shrink(false).show(&mut child, |ui| {
            let full = ui.available_width();
            let col = (full - 64.0).min(COLUMN);
            let left = ui.max_rect().left() + (full - col) / 2.0;
            ui.add_space((area.height() * 0.07).clamp(24.0, 64.0) - 16.0);

            // Who we are, and good day.
            let name = galley(ui, "Caprice", FontId::new(46.0, serif.clone()), pal().text, col, 1, false);
            let mut hello = greeting(now).to_owned();
            if words_today > 0 {
                hello = format!("{hello}  {} words written today.", crate::words::grouped(words_today as usize));
            }
            let hello = galley(ui, hello, FontId::proportional(16.0), pal().text_dim, col, 1, false);
            // The emblem, then the name and the greeting beside it, centred on it.
            let side: f32 = 84.0;
            let text_h = name.size().y + hello.size().y + 6.0;
            let room = side * crate::emblem::HEADROOM;
            let (r, _) = ui.allocate_exact_size(vec2(full, room + side.max(text_h)), Sense::hover());
            let tile = Rect::from_min_size(pos2(left, r.top() + room), vec2(side, side));
            crate::emblem::draw(ui.painter(), tile, since);
            let (x, top) = (left + side + 26.0, tile.center().y - text_h / 2.0);
            let below = top + name.size().y + 6.0;
            ui.painter().galley(pos2(x, top), name, pal().text);
            ui.painter().galley(pos2(x + 2.0, below), hello, pal().text_dim);
            ui.add_space(34.0);

            // Something new.
            heading(ui, left, full, "Begin something new");
            let per_row = if col < 560.0 { 2 } else { 4 };
            let card_w = (col - GAP * (per_row - 1) as f32) / per_row as f32;
            let sheet_h = (card_w * 0.9).clamp(96.0, 168.0);
            for row in Kind::ALL.chunks(per_row) {
                let font = FontId::proportional(12.5);
                let abouts: Vec<_> = row.iter().map(|k| galley(ui, k.about(), font.clone(), pal().text_dim, card_w - 24.0, 3, false)).collect();
                let about_h = abouts.iter().map(|g| g.size().y).fold(0.0, f32::max);
                let card_h = 18.0 + sheet_h + 16.0 + 24.0 + about_h + 16.0;
                let (r, _) = ui.allocate_exact_size(vec2(full, card_h), Sense::hover());
                for (k, (kind, about)) in row.iter().zip(abouts).enumerate() {
                    let card = Rect::from_min_size(pos2(left + k as f32 * (card_w + GAP), r.top()), vec2(card_w, card_h));
                    let resp = ui.interact(card, Id::new(("welcome_kind", kind.name())), sense);
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, kind.name()));
                    let lift = ui.ctx().animate_bool_with_time(resp.id, live && resp.hovered(), 0.15);
                    let p = ui.painter();
                    p.rect_filled(card, CornerRadius::same(12), theme::pal().dock.gamma_multiply(0.35 + 0.65 * lift));
                    let sheet = kind.sheet();
                    let w = sheet.width_for(sheet_h);
                    let at = Rect::from_min_size(pos2(card.center().x - w / 2.0, card.top() + 18.0 - 5.0 * lift), vec2(w, sheet_h));
                    sheet.draw(p, at, lift, 1.0);
                    let y = card.top() + 18.0 + sheet_h + 16.0;
                    p.galley(pos2(card.left() + 12.0, y), galley(ui, kind.name(), FontId::proportional(15.5), pal().text, card_w - 24.0, 1, false), pal().text);
                    p.galley(pos2(card.left() + 12.0, y + 24.0), about, pal().text_dim);
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        chosen = Some(Chosen::Begin(*kind, at));
                    }
                }
                ui.add_space(GAP - ui.spacing().item_spacing.y);
            }
            ui.add_space(26.0);

            // The stories last opened.
            let r = heading(ui, left, full, "Last opened");
            let open_at = Rect::from_min_size(pos2(left + col - 160.0, r.center().y - 15.0), vec2(160.0, 30.0));
            let open = ui.put(open_at, egui::Button::new("Open another\u{2026}"));
            if open.on_hover_text("Find a story in your files (Ctrl+O)").clicked() && !waiting {
                chosen = Some(Chosen::Browse);
            }
            if stories.is_empty() {
                let say = "Stories you open or save will be here, to pick up where you left off.";
                let g = galley(ui, say, FontId::proportional(14.0), pal().text_dim, col, 2, false);
                let (r, _) = ui.allocate_exact_size(vec2(full, g.size().y + 8.0), Sense::hover());
                ui.painter().galley(pos2(left, r.top() + 4.0), g, pal().text_dim);
            }
            for st in &stories {
                if let Some(at) = story_row(ui, st, left, col, full, &serif, now, sense, live) {
                    chosen = Some(Chosen::Story(st.path.clone(), at));
                }
                ui.add_space(10.0 - ui.spacing().item_spacing.y);
            }
            ui.add_space(40.0);
        });
        chosen
    }

    /// While the welcome gives way: it fades from over the desk, and the sheet chosen flies to the page.
    pub fn welcome_leaving(&mut self, ui: &mut Ui) {
        let Some(flight) = &self.start.flight else { return };
        let ctx = ui.ctx().clone();
        let s = ((ctx.input(|i| i.time) - flight.at) / FLIGHT).clamp(0.0, 1.0) as f32;
        if s >= 1.0 {
            self.start.flight = None;
            return;
        }
        ctx.request_repaint();
        let from = flight.from.clone();
        let screen = ui.max_rect();
        // First the welcome fades, leaving the sheet chosen; it flies on alone and lands on the page.
        let flying = from.is_some();
        let gone = if flying { 0.4 } else { 0.7 };
        let land = ((s - 0.8) / 0.2).clamp(0.0, 1.0);
        if flying && land == 0.0 {
            // Until it lands, the page under it is desk: the sheet becomes the page, not a second one.
            // Landed, it lies just over the page, so it fades white on white (fading both went grey).
            let cover = self.last_page_rect.expand2(vec2(80.0, 30.0));
            ui.painter().rect_filled(cover, 0.0, pal().desk);
        }
        let layer = LayerId::new(Order::Foreground, Id::new("welcome_leaving"));
        let mut over = ui.new_child(UiBuilder::new().layer_id(layer).max_rect(screen));
        over.set_opacity(1.0 - ease((s / gone).min(1.0)));
        self.welcome_page(&mut over, false);
        if let Some((rect, sheet)) = from {
            let e = ease((s / 0.8).min(1.0));
            let to = self.last_page_rect;
            let at = Rect::from_min_max(rect.min.lerp(to.min, e), rect.max.lerp(to.max, e));
            // Paper over paper at the end: it fades into the page itself, which lies just there.
            let mut painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("welcome_sheet"))).with_clip_rect(screen);
            painter.multiply_opacity(1.0 - land);
            // It rises off the welcome, then settles onto the desk.
            let lift = (e * std::f32::consts::PI).sin();
            sheet.draw(&painter, at, lift, 1.0 - e);
        }
    }
}

/// One story last opened: its first page, what it is and holds, and when. Where its sheet lies, if clicked.
#[allow(clippy::too_many_arguments)]
fn story_row(ui: &mut Ui, st: &Story, left: f32, col: f32, full: f32, serif: &egui::FontFamily, now: u64, sense: Sense, live: bool) -> Option<Rect> {
    let row_h = 118.0;
    let (r, _) = ui.allocate_exact_size(vec2(full, row_h), Sense::hover());
    let row = Rect::from_min_size(pos2(left, r.top()), vec2(col, row_h));
    let resp = ui.interact(row, Id::new(("welcome_story", &st.path)), sense);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &st.name));
    let lift = ui.ctx().animate_bool_with_time(resp.id, live && resp.hovered(), 0.15);
    let p = ui.painter();
    p.rect_filled(row, CornerRadius::same(12), theme::pal().dock.gamma_multiply(0.25 + 0.75 * lift));
    let sheet_h = row_h - 30.0;
    let w = st.sheet.width_for(sheet_h);
    let at = Rect::from_min_size(pos2(row.left() + 16.0 + (64.0 - w) / 2.0, row.top() + 15.0 - 3.0 * lift), vec2(w, sheet_h));
    st.sheet.draw(p, at, lift, 1.0);

    // On the right, when; left of it, what it is and holds.
    let right_w = 190.0_f32.min(col * 0.3);
    let x = row.left() + 16.0 + 64.0 + 20.0;
    let text_w = row.right() - right_w - 24.0 - x;
    let mut whens = vec![format!("Opened {}", crate::versions::ago(st.opened, now))];
    if let Some(c) = st.changed {
        whens.push(format!("Changed {}", crate::versions::ago(c, now)));
    }
    whens.push(folder_name(&st.path));
    let mut y = row.top() + 18.0;
    for (k, when) in whens.into_iter().enumerate() {
        let color = if k == 0 { pal().text } else { pal().text_dim };
        let g = galley(ui, when, FontId::proportional(12.5), color, right_w, 1, false);
        p.galley(pos2(row.right() - 16.0 - g.size().x, y), g, color);
        y += 20.0;
    }
    let name = galley(ui, &st.name, FontId::new(20.0, serif.clone()), pal().text, text_w, 1, false);
    let excerpt = if st.excerpt.is_empty() { "Nothing written yet.".to_owned() } else { format!("\u{201c}{}\u{201d}", st.excerpt) };
    let excerpt = galley(ui, excerpt, FontId::proportional(13.0), pal().text.gamma_multiply(0.72), text_w, 2, true);
    let meta = galley(ui, holds(st), FontId::proportional(12.0), pal().text_dim, text_w, 1, false);
    let mut y = row.top() + 14.0;
    let (name_h, excerpt_h) = (name.size().y, excerpt.size().y);
    p.galley(pos2(x, y), name, pal().text);
    y += name_h + 4.0;
    p.galley(pos2(x, y), excerpt, pal().text);
    y += excerpt_h + 6.0;
    p.galley(pos2(x, y), meta, pal().text_dim);
    let clicked = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(st.path.display().to_string()).clicked();
    clicked.then_some(at)
}

/// A section's small heading, across the column; its row.
fn heading(ui: &mut Ui, left: f32, full: f32, text: &str) -> Rect {
    let g = galley(ui, text.to_uppercase(), FontId::proportional(12.0), pal().text_dim, full, 1, false);
    let (r, _) = ui.allocate_exact_size(vec2(full, 30.0), Sense::hover());
    ui.painter().galley(pos2(left, r.center().y - g.size().y / 2.0), g, pal().text_dim);
    ui.add_space(6.0);
    r
}
