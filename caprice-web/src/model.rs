//! The document: one continuous flow of styled text plus page setup. Pages are *derived*
//! from it (see `paginate`), they are not stored.

use std::sync::Arc;

use eframe::egui::{Vec2, vec2};
use serde::{Deserialize, Serialize};

pub const FONT_SIZE: f32 = 12.0;
/// A hard page break inside the flow (form feed). Never shown in an editor.
pub const PAGE_BREAK: char = '\u{c}';
/// Points per centimetre.
pub const CM: f32 = 72.0 / 2.54;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum ListKind {
    #[default]
    None,
    Bullet,
    Numbered,
}

/// What a paragraph is in the story's structure.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum ParaKind {
    #[default]
    Body,
    /// Starts a chapter. Drawn larger and bold (see `CHAPTER_TITLE_SCALE`), on top of its own
    /// character formatting, so taking the title off gives the plain text back.
    ChapterTitle,
}

/// How much larger a chapter title is drawn than its characters' own size.
pub const CHAPTER_TITLE_SCALE: f32 = 1.6;

/// Paragraph formatting. It lives on the paragraph's terminating character (`\n` or `\f`), like
/// Word keeps it on the paragraph mark, so splitting, merging and undo handle it for free.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct ParaAttrs {
    pub align: Align,
    /// Line spacing as a multiple of the font's natural line height.
    pub spacing: f32,
    pub list: ListKind,
    #[serde(default, skip_serializing_if = "ParaKind::is_body")]
    pub kind: ParaKind,
}

impl Default for ParaAttrs {
    fn default() -> Self {
        Self { align: Align::Left, spacing: 1.0, list: ListKind::None, kind: ParaKind::Body }
    }
}

impl ParaKind {
    fn is_body(&self) -> bool {
        *self == ParaKind::Body
    }
}

impl ParaAttrs {
    pub fn is_chapter_title(&self) -> bool {
        self.kind == ParaKind::ChapterTitle
    }

    /// The paragraph made a chapter title (centered, no list), or back into ordinary text.
    pub fn set_chapter_title(&mut self, on: bool) {
        if on {
            *self = ParaAttrs { kind: ParaKind::ChapterTitle, align: Align::Center, list: ListKind::None, ..*self };
        } else {
            *self = ParaAttrs { kind: ParaKind::Body, align: Align::Left, ..*self };
        }
    }
}

/// Per-character formatting. `para` only means something on terminator characters.
#[derive(Clone, PartialEq, Debug)]
pub struct Style {
    pub font: Arc<str>,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// The text's colour (red, green, blue); none is the usual ink.
    pub color: Option<[u8; 3]>,
    pub para: ParaAttrs,
    /// Nonzero on the placeholder character of an inline picture: the id in `Doc::images`.
    pub image: u32,
}

impl Style {
    pub fn new(font: &str) -> Self {
        let para = ParaAttrs::default();
        Self { font: font.into(), size: FONT_SIZE, bold: false, italic: false, underline: false, color: None, para, image: 0 }
    }

    /// Same look of the glyph itself, ignoring paragraph formatting.
    pub fn same_char(&self, o: &Style) -> bool {
        let look = (self.bold, self.italic, self.underline, self.color) == (o.bold, o.italic, o.underline, o.color);
        self.font == o.font && self.size == o.size && look && self.image == o.image
    }

    pub fn with_para(&self, para: ParaAttrs) -> Style {
        Style { para, ..self.clone() }
    }

    /// The look of `from`'s letters, keeping this char's paragraph and picture. Copying formatting
    /// goes through here, so a new kind of character formatting is copied without more work.
    pub fn take_look(&mut self, from: &Style) {
        *self = Style { para: self.para, image: self.image, ..from.clone() };
    }
}

/// Stands in the text for a picture (which sits in a paragraph of its own).
pub const IMAGE_CHAR: char = '\u{fffc}';
/// Stood in the text for the contents in files saved before they became a page setting; taken
/// out when such a file is opened, and never let in again.
pub const CONTENTS_CHAR: char = '\u{e000}';

/// A picture of the document: the original file bytes, and how wide it is shown.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageData {
    pub id: u32,
    /// "png" or "jpeg".
    pub format: String,
    pub bytes: Vec<u8>,
    pub px: (u32, u32),
    /// Shown width in points (the height follows the aspect ratio).
    pub width_pt: f32,
    /// Clockwise quarter turns (0..=3). The stored bytes are never changed.
    pub rotation: u8,
}

impl ImageData {
    /// Shown height divided by shown width (quarter turns swap the sides).
    pub fn aspect(&self) -> f32 {
        let (w, h) = (self.px.0.max(1) as f32, self.px.1.max(1) as f32);
        if self.rotation % 2 == 1 { w / h } else { h / w }
    }
}

pub fn is_terminator(c: char) -> bool {
    c == '\n' || c == PAGE_BREAK
}

/// How a header or footer looks (drawn in `bands.rs`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum BandLook {
    /// There is none.
    #[default]
    None,
    /// The text, centred.
    Plain,
    /// The text over a hairline (a footer: under one) across the writing width.
    Rule,
    /// A hairline with a little fleuron in its middle, the text beyond it.
    Filigree,
    /// The text in white on a band of colour across the writing width.
    Banner,
    /// The text at the outer edge, left on even pages and right on odd ones, over a hairline.
    Facing,
    /// The text in white in a circle of colour (made for the page number).
    Circle,
}

impl BandLook {
    /// Every look there is, in the order the galleries show them.
    pub const ALL: [BandLook; 6] = [BandLook::Plain, BandLook::Rule, BandLook::Filigree, BandLook::Banner, BandLook::Facing, BandLook::Circle];

    pub fn name(self) -> &'static str {
        match self {
            BandLook::None => "None",
            BandLook::Plain => "Plain",
            BandLook::Rule => "Rule",
            BandLook::Filigree => "Filigree",
            BandLook::Banner => "Banner",
            BandLook::Facing => "Facing pages",
            BandLook::Circle => "Circle",
        }
    }
}

/// Stands in a header's or footer's text for the number of the page it is on.
pub const PAGE_FIELD: &str = "{page}";
/// For the title of the chapter the page is in: the first that starts on it, else the last before.
pub const CHAPTER_FIELD: &str = "{chapter}";

/// A header or footer: one line on every page, in the top or bottom margin. It shows the page
/// number (`PAGE_FIELD`), the chapter's title (`CHAPTER_FIELD`), filled in for each page, or text.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Band {
    pub look: BandLook,
    pub text: String,
    /// The colour of its lines, fleuron, band or circle.
    pub color: [u8; 3],
}

/// The colour headers, footers and text boxes start with: a quiet blue.
pub const ACCENT_INK: [u8; 3] = [86, 124, 164];

impl Default for Band {
    fn default() -> Self {
        Self { look: BandLook::None, text: String::new(), color: ACCENT_INK }
    }
}

impl Band {
    pub fn is_on(&self) -> bool {
        self.look != BandLook::None
    }

    /// The page number, centred: what "Page numbers" was before there were footers.
    pub fn page_numbers() -> Self {
        Self { look: BandLook::Plain, text: PAGE_FIELD.into(), ..Self::default() }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Orientation {
    #[default]
    Portrait,
    Landscape,
}

/// Paper, orientation, margins. All lengths in points.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PageSetup {
    pub orientation: Orientation,
    /// Portrait paper size.
    pub paper_w: f32,
    pub paper_h: f32,
    pub margin_top: f32,
    pub margin_bottom: f32,
    pub margin_left: f32,
    pub margin_right: f32,
    /// Files from before headers and footers: the page number at the foot of the page. Opening
    /// such a file makes it the footer (`DocFile::into_doc`); it is never set otherwise.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub page_numbers: bool,
    /// Drawn in the top and bottom margins of every page; they take no room from the text.
    pub header: Band,
    pub footer: Band,
    /// How many lines deep the first letter of a chapter sinks (0: no drop caps).
    pub drop_cap_lines: u8,
    /// Pages listing the chapters go before the story.
    pub contents: bool,
    /// How the contents pages are numbered (they hold no text to pin it to, `numbers.rs`).
    #[serde(skip_serializing_if = "crate::numbers::Numbering::is_usual")]
    pub contents_numbers: crate::numbers::Numbering,
}

impl Default for PageSetup {
    fn default() -> Self {
        Self {
            orientation: Orientation::Portrait,
            paper_w: 595.0,
            paper_h: 842.0,
            margin_top: 72.0,
            margin_bottom: 72.0,
            margin_left: 72.0,
            margin_right: 72.0,
            page_numbers: false,
            header: Band::default(),
            footer: Band::default(),
            drop_cap_lines: 0,
            contents: false,
            contents_numbers: Default::default(),
        }
    }
}

impl PageSetup {
    pub const MIN_CONTENT: f32 = 120.0;

    pub fn size(&self) -> Vec2 {
        match self.orientation {
            Orientation::Portrait => vec2(self.paper_w, self.paper_h),
            Orientation::Landscape => vec2(self.paper_h, self.paper_w),
        }
    }

    pub fn margin_origin(&self) -> Vec2 {
        vec2(self.margin_left, self.margin_top)
    }

    pub fn content_size(&self) -> Vec2 {
        self.size() - vec2(self.margin_left + self.margin_right, self.margin_top + self.margin_bottom)
    }

    /// Keep at least `MIN_CONTENT` of writing area on every page.
    pub fn clamp_margins(&mut self) {
        let s = self.size();
        let max_x = (s.x - Self::MIN_CONTENT).max(0.0);
        let max_y = (s.y - Self::MIN_CONTENT).max(0.0);
        self.margin_left = self.margin_left.clamp(0.0, max_x);
        self.margin_right = self.margin_right.clamp(0.0, max_x - self.margin_left);
        self.margin_top = self.margin_top.clamp(0.0, max_y);
        self.margin_bottom = self.margin_bottom.clamp(0.0, max_y - self.margin_top);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    /// Char and byte range of the page text inside the flow. A hard break after `end` is not part of it.
    pub start: usize,
    pub end: usize,
    pub bstart: usize,
    pub bend: usize,
    /// Does a hard page break follow (rather than the page ending because it is full)?
    pub hard: bool,
    /// On a page of the contents, which of them (0 for the first). These pages go before the
    /// story and hold none of its text: their span is the empty one at its start.
    pub contents: Option<usize>,
}

impl Span {
    pub fn shifted(self, dchars: isize, dbytes: isize) -> Span {
        Span {
            start: self.start.wrapping_add_signed(dchars),
            end: self.end.wrapping_add_signed(dchars),
            bstart: self.bstart.wrapping_add_signed(dbytes),
            bend: self.bend.wrapping_add_signed(dbytes),
            hard: self.hard,
            contents: self.contents,
        }
    }

    pub fn chars(&self) -> usize {
        self.end - self.start
    }
}

/// A sticky note attached to a stretch of text without being part of it. `start..end` are flow
/// positions (chars); `start == end` is a point note. The edit layer keeps them in place as the text changes.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub id: u64,
    pub start: usize,
    pub end: usize,
    pub text: String,
    /// Index into `NOTE_COLORS`.
    pub color: usize,
}

/// A scene Claude painted, shown faintly behind the pages from its place in the story on, until
/// the next scene. Like a note, it is a position outside the text.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub id: u64,
    /// The char it is pinned to: the start of the paragraph it was painted for.
    pub at: usize,
    /// Every drawing (SVG) painted for this place, oldest first. Repainting adds one.
    pub versions: Vec<String>,
    /// Which of them shows.
    pub shown: usize,
    /// What it was painted from: the description, or the passage.
    pub subject: String,
    pub hidden: bool,
    /// The people of the cast it was painted with, by name: those its subject names, or carried
    /// from the paragraphs before it, or those the writer chose (`chosen`).
    pub people: Vec<String>,
    /// The writer chose `people`, so painting it again keeps them rather than looking again.
    pub chosen: bool,
}

impl Scene {
    /// The drawing that shows, or none while the first is still being painted.
    pub fn svg(&self) -> Option<&str> {
        self.versions.get(self.shown).map(String::as_str)
    }
}

/// Someone in the story, as Claude should paint them in every scene that names them. Not pinned to
/// the text: they are found by name in what is painted.
#[derive(Clone, Debug, PartialEq)]
pub struct Character {
    pub id: u64,
    pub name: String,
    /// Other names the story calls them by, separated by commas.
    pub aliases: String,
    /// How they look: one look, or several kept to switch between (as a girl, after the shipwreck).
    /// Never empty.
    pub looks: Vec<Look>,
    /// The look scenes are painted with now.
    pub active: usize,
}

/// One way someone looks: in words, and their model sheet once drawn.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Look {
    pub id: u64,
    /// What the writer calls it ("as a girl"), to tell looks apart.
    pub label: String,
    pub words: String,
    /// The model sheet (SVG), once drawn: the figures scenes copy them from.
    pub sheet: Option<String>,
    /// Them peeking out from behind a page, once drawn: an SVG in parts to move (`peek.rs`).
    pub peek: Option<String>,
}

impl Character {
    /// Someone new, with one look yet to be described.
    pub fn new(id: u64, look_id: u64, name: &str) -> Self {
        Self { id, name: name.into(), aliases: String::new(), looks: vec![Look { id: look_id, ..Look::default() }], active: 0 }
    }

    /// The look scenes are painted with now.
    pub fn look(&self) -> &Look {
        &self.looks[self.active.min(self.looks.len() - 1)]
    }
}

/// How a text box looks (drawn in `textbox.rs`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum BoxLook {
    /// White, with a thin dark border.
    #[default]
    Simple,
    /// Tinted with its colour, no border.
    Shaded,
    /// A bar of its colour down the left side, lightly tinted.
    Sidebar,
    /// Larger italic text between two rules of its colour, centred, as a pull quote.
    Quote,
    /// Only the text.
    Bare,
}

impl BoxLook {
    pub const ALL: [BoxLook; 5] = [BoxLook::Simple, BoxLook::Shaded, BoxLook::Sidebar, BoxLook::Quote, BoxLook::Bare];

    pub fn name(self) -> &'static str {
        match self {
            BoxLook::Simple => "Simple",
            BoxLook::Shaded => "Shaded",
            BoxLook::Sidebar => "Sidebar",
            BoxLook::Quote => "Quote",
            BoxLook::Bare => "No border",
        }
    }
}

/// A box of text placed on a page, in front of the story's text or behind it, which does not flow
/// around it (as Word's "In front of text" and "Behind text"). Like a note it is pinned to a char
/// outside the text, and shows on that char's page, so it goes where the text it belongs with goes.
#[derive(Clone, Debug, PartialEq)]
pub struct TextBox {
    pub id: u64,
    /// The char it is pinned to.
    pub at: usize,
    /// Its top-left from the page's top-left, in page points.
    pub pos: Vec2,
    /// Its width, and the least height: it grows to hold its text.
    pub size: Vec2,
    pub text: String,
    /// Font, size, bold, italic, underline and colour of all its text; `para.align` aligns it.
    pub style: Style,
    pub look: BoxLook,
    /// The colour of its border, tint, bar or rules.
    pub color: [u8; 3],
    /// Lies behind the story's text and pictures, rather than in front of them.
    pub behind: bool,
    /// Its fill lets what is under it show through: none for a simple box, a see-through tint for the others.
    pub clear: bool,
}

pub const NOTE_COLORS: [(u8, u8, u8); 5] =
    [(255, 214, 90), (255, 150, 185), (130, 215, 140), (120, 195, 255), (255, 175, 100)];

pub struct Flow {
    pub text: String,
    /// One style per char of `text`.
    pub styles: Vec<Style>,
}

pub struct Doc {
    pub flow: Flow,
    pub setup: PageSetup,
    pub spans: Vec<Span>,
    /// Bumped on every change, so views can tell when they are stale.
    pub version: u64,
    pub notes: Vec<Note>,
    pub next_note_id: u64,
    pub images: Vec<ImageData>,
    pub history: crate::edit::History,
    /// Scenes painted for the story, in no particular order.
    pub scenes: Vec<Scene>,
    pub next_scene_id: u64,
    /// The people of the story, in the order the writer added them.
    pub cast: Vec<Character>,
    /// Those of them with a peek drawn peek out from behind the pages now and then.
    pub peeks: bool,
    /// The next id for someone of the cast or one of their looks.
    pub next_cast_id: u64,
    /// What the writer wants Claude to know about the story whenever it is asked about it.
    pub story_notes: String,
    /// The scratchpad's plain text, kept beside the story.
    pub scratchpad: String,
    /// Text boxes, bottom first: later ones lie on top of earlier ones in their layer.
    pub boxes: Vec<TextBox>,
    pub next_box_id: u64,
    /// Where page numbering changes, in the order of the story (`numbers.rs`).
    pub number_marks: Vec<crate::numbers::NumberMark>,
}

impl Doc {
    /// An empty document: just the final paragraph mark every document ends with.
    pub fn new() -> Self {
        Self {
            flow: Flow { text: "\n".into(), styles: vec![Style::new("Default")] },
            setup: PageSetup::default(),
            spans: vec![Span { start: 0, end: 1, bstart: 0, bend: 1, hard: false, contents: None }],
            version: 0,
            notes: Vec::new(),
            next_note_id: 1,
            images: Vec::new(),
            history: Default::default(),
            scenes: Vec::new(),
            next_scene_id: 1,
            cast: Vec::new(),
            peeks: true,
            next_cast_id: 1,
            story_notes: String::new(),
            scratchpad: String::new(),
            boxes: Vec::new(),
            next_box_id: 1,
            number_marks: Vec::new(),
        }
    }

    /// Make sure the text ends with a paragraph mark (it is the carrier of the last paragraph's format).
    pub fn ensure_final_mark(&mut self) {
        if !self.flow.text.ends_with('\n') {
            let st = self.flow.styles.last().cloned().unwrap_or_else(|| Style::new("Default"));
            self.flow.text.push('\n');
            self.flow.styles.push(st.with_para(ParaAttrs::default()));
        }
    }

    /// The text as the user sees it, without the final paragraph mark.
    pub fn visible_text(&self) -> &str {
        &self.flow.text[..self.flow.text.len().saturating_sub(1)]
    }

    pub fn image(&self, id: u32) -> Option<&ImageData> {
        self.images.iter().find(|i| i.id == id)
    }

    /// Size in points an image is drawn at, shrunk if needed to fit the writing area.
    pub fn image_size(&self, img: &ImageData) -> Vec2 {
        let content = self.setup.content_size();
        let mut w = img.width_pt.clamp(8.0, content.x);
        let mut h = w * img.aspect();
        if h > content.y {
            h = content.y;
            w = h / img.aspect();
        }
        vec2(w, h)
    }

    pub fn pages(&self) -> usize {
        self.spans.len()
    }

    pub fn page_text(&self, i: usize) -> &str {
        let s = self.spans[i];
        &self.flow.text[s.bstart..s.bend]
    }

    pub fn page_styles(&self, i: usize) -> &[Style] {
        let s = self.spans[i];
        &self.flow.styles[s.start..s.end]
    }

    pub fn char_to_byte(&self, c: usize) -> usize {
        self.flow.text.char_indices().nth(c).map_or(self.flow.text.len(), |(b, _)| b)
    }

    /// Chars in the flow, including the final paragraph mark. The caret can sit at `0..total_chars()`.
    pub fn total_chars(&self) -> usize {
        self.flow.styles.len()
    }

    pub fn char_at(&self, c: usize) -> Option<char> {
        self.flow.text.chars().nth(c)
    }

    /// The first paragraph terminator at or after (char `c`, byte `b`).
    pub fn term_from(&self, c: usize, b: usize) -> (usize, usize) {
        let rest = &self.flow.text[b..];
        let rel = rest.find(is_terminator).unwrap_or(rest.len().saturating_sub(1));
        (c + rest[..rel].chars().count(), b + rel)
    }

    /// Start (char, byte) of the paragraph containing char `c`.
    pub fn para_start(&self, c: usize) -> (usize, usize) {
        let b = self.char_to_byte(c);
        match self.flow.text[..b].rfind(is_terminator) {
            Some(t) => (self.flow.text[..t].chars().count() + 1, t + 1),
            None => (0, 0),
        }
    }

    /// Format of the paragraph containing char `c`.
    pub fn para_attrs_at(&self, c: usize) -> ParaAttrs {
        let b = self.char_to_byte(c.min(self.total_chars().saturating_sub(1)));
        let (tc, _) = self.term_from(c.min(self.total_chars().saturating_sub(1)), b);
        self.flow.styles[tc.min(self.flow.styles.len() - 1)].para
    }

    /// Which page the caret at flow position `c` is on. A position on a soft page boundary belongs
    /// to the page that starts there; the spot just before a hard break to the page it ends.
    pub fn page_of(&self, c: usize) -> usize {
        for (i, s) in self.spans.iter().enumerate() {
            if c < s.end || (s.hard && c == s.end) {
                return i;
            }
        }
        self.spans.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landscape_swaps_and_margins_are_kept_usable() {
        let mut s = PageSetup { orientation: Orientation::Landscape, margin_left: 900.0, ..Default::default() };
        assert_eq!(s.size(), vec2(842.0, 595.0));
        s.clamp_margins();
        assert!(s.content_size().x >= PageSetup::MIN_CONTENT - 0.01);
        let mut s = PageSetup { margin_left: 300.0, margin_right: 300.0, margin_top: 500.0, margin_bottom: 500.0, ..Default::default() };
        s.clamp_margins();
        assert_eq!(s.margin_left, 300.0);
        assert!((s.content_size().x - PageSetup::MIN_CONTENT).abs() < 0.01, "the right margin gives way");
        assert!((s.content_size().y - PageSetup::MIN_CONTENT).abs() < 0.01, "the bottom margin gives way");
    }

    #[test]
    fn a_centimetre_is_its_share_of_an_inch() {
        assert!((CM * 2.54 - 72.0).abs() < 1e-4);
    }

    #[test]
    fn same_char_compares_the_glyph_but_not_the_paragraph() {
        let a = Style::new("Serif");
        let list = ParaAttrs { list: ListKind::Bullet, ..Default::default() };
        assert!(a.same_char(&a.with_para(list)));
        assert!(!a.same_char(&Style::new("Sans")));
        assert!(!a.same_char(&Style { size: 14.0, ..a.clone() }));
        assert!(!a.same_char(&Style { bold: true, ..a.clone() }));
        assert!(!a.same_char(&Style { underline: true, ..a.clone() }));
        assert!(!a.same_char(&Style { image: 1, ..a.clone() }));
    }

    #[test]
    fn a_tall_picture_is_shrunk_to_fit_the_page() {
        let d = Doc::new();
        let content = d.setup.content_size();
        let img = ImageData { id: 1, format: "png".into(), bytes: vec![], px: (100, 1000), width_pt: content.x, rotation: 0 };
        let size = d.image_size(&img);
        assert!((size.y - content.y).abs() < 0.01);
        assert!((size.x - content.y / 10.0).abs() < 0.01, "keeps its proportions: {size:?}");
    }

    #[test]
    fn a_chapter_title_is_centered_without_a_list_and_turns_back_into_text() {
        let mut p = ParaAttrs { list: ListKind::Bullet, spacing: 1.5, ..Default::default() };
        p.set_chapter_title(true);
        assert!(p.is_chapter_title());
        assert_eq!((p.align, p.list, p.spacing), (Align::Center, ListKind::None, 1.5));
        p.set_chapter_title(false);
        assert_eq!(p, ParaAttrs { spacing: 1.5, ..Default::default() });
    }

    #[test]
    fn positions_past_the_end_belong_to_the_last_paragraph_and_page() {
        let mut d = Doc::new();
        let centered = ParaAttrs { align: Align::Center, ..Default::default() };
        d.flow.text = "a\nb\n".into();
        d.flow.styles = vec![Style::new("x"), Style::new("x"), Style::new("x"), Style::new("x").with_para(centered)];
        assert_eq!(d.para_attrs_at(0).align, Align::Left);
        assert_eq!(d.para_attrs_at(99).align, Align::Center);
        d.spans = vec![
            Span { start: 0, end: 2, bstart: 0, bend: 2, hard: false, contents: None },
            Span { start: 2, end: 4, bstart: 2, bend: 4, hard: false, contents: None },
        ];
        assert_eq!(d.spans[1].chars(), 2);
        assert_eq!(d.page_of(1), 0);
        assert_eq!(d.page_of(99), 1);
    }
}
