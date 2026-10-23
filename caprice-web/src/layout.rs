//! Laying text out: one egui text layout per paragraph, so every paragraph can have its own
//! alignment, line spacing and list indent. A page is a stack of paragraph pieces.

use std::ops::Range;
use std::sync::Arc;

use eframe::egui::{self, Color32, FontId, Rect, Stroke, TextFormat, Vec2, pos2, text::LayoutJob, vec2};
use egui::text::CCursor;

use crate::contents::{ContentsBlock, Entry, layout_contents};
use crate::dropcap::{Float, layout_beside, layout_with_cap};
use crate::fonts::family_for;
use crate::model::{Align, CHAPTER_TITLE_SCALE, Doc, IMAGE_CHAR, ListKind, PAGE_BREAK, ParaAttrs, Style, is_terminator};
use crate::theme::INK;

/// Width reserved for the bullet or number of a list item, in page points.
pub const LIST_INDENT: f32 = 28.0;

/// A background highlight over a range of chars (notes, search results).
#[derive(Clone, Debug)]
pub struct Mark {
    pub range: Range<usize>,
    pub color: Color32,
}

pub fn run_format(ctx: &egui::Context, st: &Style, scale: f32, background: Color32, attrs: ParaAttrs) -> TextFormat {
    let title = attrs.is_chapter_title();
    let size = if title { st.size * CHAPTER_TITLE_SCALE } else { st.size };
    let (family, slant) = family_for(&st.font, st.bold || title, st.italic);
    let font_id = FontId::new(size * scale, family);
    let spacing = attrs.spacing;
    let line_height = ((spacing - 1.0).abs() > 0.001).then(|| ctx.fonts_mut(|f| f.row_height(&font_id)) * spacing);
    let color = st.color.map_or(INK, |[r, g, b]| Color32::from_rgb(r, g, b));
    TextFormat {
        background,
        line_height,
        font_id,
        color,
        italics: slant,
        underline: if st.underline { Stroke::new(scale.max(1.0), color) } else { Stroke::NONE },
        ..Default::default()
    }
}

/// Layout job for one paragraph (or the part of it that is on this page).
/// `styles[k]` is the style of the k-th char of `text`. Later `marks` paint over earlier ones.
pub fn paragraph_job(
    ctx: &egui::Context,
    text: &str,
    styles: &[Style],
    term: &Style,
    attrs: ParaAttrs,
    scale: f32,
    wrap_width: f32,
    marks: &[Mark],
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.halign = match attrs.align {
        Align::Left | Align::Justify => egui::Align::LEFT,
        Align::Center => egui::Align::Center,
        Align::Right => egui::Align::RIGHT,
    };
    job.justify = attrs.align == Align::Justify;
    if text.is_empty() {
        // An empty paragraph still has a height: that of the mark that ends it.
        job.append("", 0.0, run_format(ctx, term, scale, Color32::TRANSPARENT, attrs));
        return job;
    }
    let style_at = |k: usize| styles.get(k).or(styles.last()).unwrap_or(term);
    let mark_at = |k: usize| marks.iter().rev().find(|m| m.range.contains(&k)).map_or(Color32::TRANSPARENT, |m| m.color);
    let (mut start, mut run, mut run_mark) = (0, 0, mark_at(0));
    for (k, (b, _)) in text.char_indices().enumerate() {
        let mark = mark_at(k);
        if k > run && (!style_at(k).same_char(style_at(run)) || mark != run_mark) {
            job.append(&text[start..b], 0.0, run_format(ctx, style_at(run), scale, run_mark, attrs));
            start = b;
            run = k;
            run_mark = mark;
        }
    }
    job.append(&text[start..], 0.0, run_format(ctx, style_at(run), scale, run_mark, attrs));
    job
}

/// Where a picture is drawn, relative to the top-left of its paragraph.
#[derive(Clone, Copy, Debug)]
pub struct ImageBox {
    pub id: u32,
    pub rect: Rect,
    /// Clockwise quarter turns.
    pub rotation: u8,
}

/// A row of a paragraph: how many chars it holds and where it is, relative to the paragraph's top.
#[derive(Clone, Copy, Debug)]
pub struct RowGeom {
    pub chars: usize,
    pub y: f32,
    pub height: f32,
}

/// One paragraph (or the page's share of it), laid out.
pub struct ParaLayout {
    /// Page-local char range of the text; `end` is where the terminator sits (or the page ends).
    pub start: usize,
    pub end: usize,
    /// Top-left relative to the page's writing area.
    pub y: f32,
    pub x: f32,
    pub height: f32,
    pub galley: Arc<egui::Galley>,
    /// Bullet or number, with its position relative to the paragraph's own top-left (before `x`).
    pub marker: Option<(Arc<egui::Galley>, Vec2)>,
    pub attrs: ParaAttrs,
    /// Set when the paragraph is a picture.
    pub image: Option<ImageBox>,
    /// Does this piece begin its paragraph (rather than continue one from the previous page)?
    pub starts_paragraph: bool,
    /// Is this piece a drop cap (see `dropcap`)?
    pub cap: bool,
    /// Does it sit beside the piece before it (the lines next to a drop cap) rather than below?
    pub beside: bool,
    /// Set when the paragraph is the contents.
    pub contents: Option<ContentsBlock>,
}

impl ParaLayout {
    /// A picture or the contents: one block that the caret goes before or after, relative to the
    /// top-left of the piece.
    pub fn block(&self) -> Option<Rect> {
        self.image.map(|img| img.rect).or_else(|| self.contents.as_ref().map(|c| Rect::from_min_size(pos2(0.0, 0.0), c.size)))
    }

    pub fn row_table(&self) -> Vec<RowGeom> {
        if self.block().is_some() || self.cap {
            return vec![RowGeom { chars: self.end - self.start, y: 0.0, height: self.height }];
        }
        self.galley
            .rows
            .iter()
            .map(|r| RowGeom { chars: usize::from(r.row.char_count_excluding_newline()), y: r.pos.y, height: r.size.y })
            .collect()
    }
}

pub struct PageLayout {
    pub paras: Vec<ParaLayout>,
    pub height: f32,
    pub scale: f32,
}

/// One thing drawn on a page: text with its top-left, a picture in its box (on the page), or a
/// shape of a header, footer or text box.
pub enum Ink {
    Text(Arc<egui::Galley>, egui::Pos2),
    Picture(ImageBox),
    /// An outline filled with a colour. It must be convex: egui fills no other kind.
    Fill(Vec<egui::Pos2>, Color32),
    /// A line through the points, this wide.
    Line(Vec<egui::Pos2>, f32, Color32),
}

impl Ink {
    /// What egui draws for it on screen; a picture needs its texture, so it has none.
    pub fn shape(self) -> Option<egui::Shape> {
        match self {
            Ink::Text(g, at) => Some(egui::Shape::galley(at, g, INK)),
            Ink::Picture(_) => None,
            Ink::Fill(points, color) => Some(egui::Shape::convex_polygon(points, color, Stroke::NONE)),
            Ink::Line(points, width, color) => Some(egui::Shape::line(points, Stroke::new(width, color))),
        }
    }

    /// The same, moved by `by`.
    pub fn translate(self, by: Vec2) -> Ink {
        match self {
            Ink::Text(g, at) => Ink::Text(g, at + by),
            Ink::Picture(img) => Ink::Picture(ImageBox { rect: img.rect.translate(by), ..img }),
            Ink::Fill(points, color) => Ink::Fill(points.into_iter().map(|p| p + by).collect(), color),
            Ink::Line(points, width, color) => Ink::Line(points.into_iter().map(|p| p + by).collect(), width, color),
        }
    }
}

impl PageLayout {
    /// Everything drawn on the page, its writing area's top-left at `origin`, bottom first. The
    /// screen (`paint_layout`) and print (`print.rs`) both draw from this, so they show the same.
    pub fn ink(&self, origin: egui::Pos2) -> Vec<Ink> {
        let mut ink = Vec::new();
        for p in &self.paras {
            if let Some(img) = p.image {
                ink.push(Ink::Picture(ImageBox { rect: img.rect.translate(origin.to_vec2() + vec2(0.0, p.y)), ..img }));
            }
            if let Some((g, at)) = &p.marker {
                ink.push(Ink::Text(g.clone(), origin + vec2(at.x, p.y + at.y)));
            }
            if p.block().is_none() {
                ink.push(Ink::Text(p.galley.clone(), origin + vec2(p.x, p.y)));
            }
            for (g, at) in p.contents.iter().flat_map(|c| &c.texts) {
                ink.push(Ink::Text(g.clone(), origin + vec2(p.x + at.x, p.y + at.y)));
            }
        }
        ink
    }

    /// Whether a point, relative to the writing area, is on a row of text or a picture (the
    /// room beside a short line, or between paragraphs, is not).
    pub fn covers(&self, pos: Vec2) -> bool {
        let at = pos.to_pos2();
        self.ink(egui::Pos2::ZERO).iter().any(|ink| match ink {
            Ink::Text(g, origin) => g.rows.iter().any(|r| !r.row.glyphs.is_empty() && r.rect().translate(origin.to_vec2()).contains(at)),
            Ink::Picture(img) => img.rect.contains(at),
            _ => false,
        })
    }
}

/// What the layout of a piece is built from.
pub struct PieceSpec<'a> {
    pub text: &'a str,
    pub styles: &'a [Style],
    pub term: &'a Style,
    pub attrs: ParaAttrs,
    pub scale: f32,
    pub content_width: f32,
    pub marks: &'a [Mark],
    /// The text before a page break that has none: takes up no room.
    pub invisible: bool,
    /// Text of the bullet/number, if this piece starts a list item.
    pub marker: Option<String>,
    /// The picture this piece is: its id, its size in page points and its rotation.
    pub image: Option<(u32, Vec2, u8)>,
    /// The contents, if this piece is them: the chapters, the height of a page, and which of the
    /// contents' pages this is.
    pub contents: Option<(&'a [Entry], f32, usize)>,
}

pub fn layout_piece(ctx: &egui::Context, spec: &PieceSpec, local_start: usize, local_end: usize, y: f32) -> ParaLayout {
    let indent = if spec.attrs.list == ListKind::None { 0.0 } else { LIST_INDENT * spec.scale };
    let wrap = (spec.content_width - indent).max(10.0);
    let job = paragraph_job(ctx, spec.text, spec.styles, spec.term, spec.attrs, spec.scale, wrap, spec.marks);
    let mut galley = ctx.fonts_mut(|f| f.layout_job(job));
    if spec.attrs.align == Align::Justify {
        galley = justified_edges(galley);
    }
    if !spec.marks.is_empty() {
        galley = tight_highlights(galley, ctx.pixels_per_point());
    }
    if let Some((id, size, rotation)) = spec.image {
        let size = size * spec.scale;
        let x = match spec.attrs.align {
            Align::Left | Align::Justify => 0.0,
            Align::Center => (spec.content_width - size.x) / 2.0,
            Align::Right => spec.content_width - size.x,
        };
        return ParaLayout {
            start: local_start,
            end: local_end,
            y,
            x: 0.0,
            height: size.y,
            galley,
            marker: None,
            attrs: spec.attrs,
            starts_paragraph: false,
            cap: false,
            beside: false,
            contents: None,
            image: Some(ImageBox { id, rect: Rect::from_min_size(pos2(x, 0.0), size), rotation }),
        };
    }
    if let Some((entries, max_height, part)) = spec.contents {
        let st = spec.styles.first().unwrap_or(spec.term);
        let block = layout_contents(ctx, entries, st, spec.content_width / spec.scale, max_height, spec.scale, part);
        return ParaLayout {
            start: local_start,
            end: local_end,
            y,
            x: 0.0,
            height: block.size.y,
            galley,
            marker: None,
            attrs: spec.attrs,
            starts_paragraph: false,
            cap: false,
            beside: false,
            contents: Some(block),
            image: None,
        };
    }
    let galley = true_to_scale(ctx, spec, galley, wrap);
    let height = if spec.invisible { 0.0 } else { galley.size().y };
    let marker = spec.marker.as_ref().map(|text| {
        let st = spec.styles.first().unwrap_or(spec.term);
        let font = FontId::new(st.size * spec.scale, family_for(&st.font, false, false).0);
        let g = ctx.fonts_mut(|f| f.layout(text.clone(), font, INK, f32::INFINITY));
        let row = galley.rows.first().map_or((0.0, g.size().y), |r| (r.pos.y, r.size.y));
        let at = vec2(
            (indent - 6.0 * spec.scale - g.size().x).max(0.0),
            row.0 + (row.1 - g.size().y) / 2.0,
        );
        (g, at)
    });
    ParaLayout {
        start: local_start,
        end: local_end,
        y,
        // egui anchors centered and right-aligned text at its middle / right edge.
        x: indent + match spec.attrs.align {
            Align::Center => wrap / 2.0,
            Align::Right => wrap,
            Align::Left | Align::Justify => 0.0,
        },
        height,
        galley,
        marker,
        attrs: spec.attrs,
        starts_paragraph: false,
        cap: false,
        beside: false,
        contents: None,
        image: None,
    }
}

/// egui lines up every row of justified text by its first and last letters, leaving out the
/// spaces and tabs before and after them: a tab at the start of a row (an indent, or typed after a
/// drop cap) went out to the left of the text, and a tab or space typed at the end of a paragraph
/// left the caret where it was until a letter came after it. Put them back: the whitespace in
/// front takes its room again (on a stretched row, the gaps between words give it up), and the
/// last row is as wide as all its chars.
fn justified_edges(mut galley: Arc<egui::Galley>) -> Arc<egui::Galley> {
    let edged = |r: &egui::epaint::text::PlacedRow| {
        let g = &r.row.glyphs;
        g.first().is_some_and(|f| f.chr.is_whitespace()) || g.last().is_some_and(|l| l.chr.is_whitespace())
    };
    if !galley.rows.iter().any(edged) {
        return galley;
    }
    let g = Arc::make_mut(&mut galley);
    let wrap = g.job.wrap.max_width;
    let last_row = g.rows.len().saturating_sub(1);
    for (i, placed) in g.rows.iter_mut().enumerate() {
        let stretched = !placed.ends_with_newline && i < last_row;
        let row = Arc::make_mut(&mut placed.row);
        let lead = row.glyphs.iter().take_while(|gl| gl.chr.is_whitespace()).count();
        if lead == row.glyphs.len() {
            continue; // only whitespace: egui keeps it as it is
        }
        let room: f32 = row.glyphs[..lead].iter().map(|gl| gl.advance_width).sum();
        // The first letter belongs after the whitespace in front; on a stretched row the last one
        // ends at the margin. egui put them elsewhere (with whitespace in front, even past the
        // margin), so each glyph moves by a share of both: the gaps between words before it say how much.
        let end = row.glyphs.len() - row.glyphs.iter().rev().take_while(|gl| gl.chr.is_whitespace()).count();
        let first_by = room - row.glyphs[lead].pos.x;
        let last = &row.glyphs[end - 1];
        let last_by = if stretched { wrap - (last.pos.x + last.advance_width) } else { first_by };
        if lead > 0 && (first_by.abs() > 0.01 || last_by.abs() > 0.01) {
            let spaces = row.glyphs[lead..end].iter().filter(|gl| gl.chr.is_whitespace()).count();
            let mut seen = 0usize;
            let n = row.glyphs.len();
            for k in 0..n {
                let t = if k < lead {
                    0.0
                } else if k >= end {
                    1.0
                } else if spaces > 0 {
                    seen as f32 / spaces as f32
                } else {
                    (k - lead) as f32 / (end - lead - 1).max(1) as f32
                };
                if k >= lead && row.glyphs[k].chr.is_whitespace() {
                    seen += 1;
                }
                let by = first_by + (last_by - first_by) * t;
                let gl = &mut row.glyphs[k];
                gl.pos.x += by;
                if !gl.uv_rect.is_nothing() {
                    let v = gl.first_vertex as usize;
                    for vert in row.visuals.mesh.vertices.iter_mut().skip(v).take(4) {
                        vert.pos.x += by;
                    }
                }
            }
        }
        if !stretched && let Some(end) = row.glyphs.last().map(|gl| gl.pos.x + gl.advance_width) {
            row.size.x = row.size.x.max(end);
        }
    }
    galley
}

/// egui paints a highlight over the whole line height, so with line spacing above 1 it reaches far
/// below the letters. Cut each highlight down to the height of the font it lies behind.
fn tight_highlights(mut galley: Arc<egui::Galley>, ppp: f32) -> Arc<egui::Galley> {
    // How far egui grows a highlight past the letters (`TextFormat::expand_bg`, left at its default).
    const EXPAND: f32 = 1.0;
    let g = Arc::make_mut(&mut galley);
    for placed in &mut g.rows {
        let row = Arc::make_mut(&mut placed.row);
        let bg = row.visuals.glyph_vertex_range.start;
        for quad in row.visuals.mesh.vertices[..bg].as_chunks_mut::<4>().0 {
            let top = quad.iter().map(|v| v.pos.y).fold(f32::INFINITY, f32::min);
            let x = quad.iter().map(|v| v.pos.x).sum::<f32>() / 4.0;
            let Some(glyph) = row.glyphs.iter().find(|g| g.max_x() > x).or(row.glyphs.last()) else { continue };
            let bottom = ((top + glyph.font_height + 2.0 * EXPAND) * ppp).round() / ppp;
            for v in quad.iter_mut().filter(|v| v.pos.y > top) {
                v.pos.y = bottom;
            }
        }
        row.visuals.mesh_bounds = row.visuals.mesh.calc_bounds();
    }
    galley
}

/// egui rounds every line's height to whole pixels, so zoomed text comes out a little shorter or
/// taller than at page size, where the page breaks are worked out. Over a page that adds up to a
/// line or more. Give the lines their page-size height times the zoom instead, spreading them to
/// match (each placed on a whole pixel, so the text stays crisp).
///
/// Zoomed, a word at the edge can wrap differently than at page size, so the line counts can
/// differ: what is matched is the height per line, not the paragraph's, or the lines would
/// squeeze together or spread apart until typing moves the word again.
fn true_to_scale(ctx: &egui::Context, spec: &PieceSpec, galley: Arc<egui::Galley>, wrap: f32) -> Arc<egui::Galley> {
    let sc = spec.scale;
    if (sc - 1.0).abs() < 1e-4 || galley.size().y <= 0.0 {
        return galley;
    }
    let job = paragraph_job(ctx, spec.text, spec.styles, spec.term, spec.attrs, 1.0, wrap / sc, &[]);
    let page = ctx.fonts_mut(|f| f.layout_job(job));
    let k = (page.size().y / page.rows.len().max(1) as f32 * sc) / (galley.size().y / galley.rows.len().max(1) as f32);
    let height = galley.size().y * k;
    if (k - 1.0).abs() < 1e-4 {
        return galley;
    }
    let ppp = ctx.pixels_per_point();
    let mut g = (*galley).clone();
    for row in &mut g.rows {
        row.pos.y = (row.pos.y * k * ppp).round() / ppp;
    }
    g.rect.max.y = g.rect.min.y + height;
    g.mesh_bounds.max.y += height - galley.size().y;
    Arc::new(g)
}

/// Number of the list item that starts at (char `c`, byte `b`) among consecutive numbered paragraphs.
pub fn list_number(doc: &Doc, mut c: usize, mut b: usize) -> usize {
    let mut n = 1;
    while c > 0 {
        let prev_term = c - 1;
        if doc.flow.styles[prev_term].para.list != ListKind::Numbered {
            break;
        }
        n += 1;
        // Step back to the start of the previous paragraph.
        match doc.flow.text[..b - 1].rfind(is_terminator) {
            Some(t) => {
                c = doc.flow.text[..t].chars().count() + 1;
                b = t + 1;
            }
            None => {
                c = 0;
                b = 0;
            }
        }
    }
    n
}

impl Doc {
    /// If chars `c..ec` are exactly one picture placeholder, its id and size.
    pub fn picture_in(&self, c: usize, ec: usize) -> Option<(u32, Vec2, u8)> {
        if ec != c + 1 {
            return None;
        }
        let st = &self.flow.styles[c];
        let img = (st.image != 0).then(|| self.image(st.image)).flatten()?;
        (self.char_at(c) == Some(IMAGE_CHAR)).then(|| (img.id, self.image_size(img), img.rotation))
    }

    /// Page `part` of the contents: one block, with no text of the story's.
    fn layout_contents_page(&self, ctx: &egui::Context, part: usize, scale: f32) -> PageLayout {
        let st = self.contents_style();
        let chapters = self.chapters(true);
        let spec = PieceSpec {
            text: "",
            styles: &[],
            term: &st,
            attrs: ParaAttrs::default(),
            scale,
            content_width: self.setup.content_size().x * scale,
            marks: &[],
            invisible: false,
            marker: None,
            image: None,
            contents: Some((&chapters, self.setup.content_size().y, part)),
        };
        let p = layout_piece(ctx, &spec, 0, 0, 0.0);
        PageLayout { height: p.height, paras: vec![p], scale }
    }

    pub fn hard_end(&self, page: usize) -> bool {
        self.spans[page].hard
    }

    /// Lay out page `i`. `marks` are page-local highlights.
    pub fn layout_page(&self, ctx: &egui::Context, i: usize, scale: f32, marks: &[Mark]) -> PageLayout {
        let sp = self.spans[i];
        if let Some(part) = sp.contents {
            return self.layout_contents_page(ctx, part, scale);
        }
        let hard_end = sp.hard;
        let content_width = self.setup.content_size().x * scale;
        let mut paras = Vec::new();
        let mut y = 0.0;
        let (mut c, mut b) = (sp.start, sp.bstart);
        let mut number: Option<usize> = None; // the number of the last numbered paragraph seen
        // The room beside a drop cap whose paragraph ended before the letter did (page points, as
        // pagination works it out: `y / scale` is where we are at page size).
        let mut float: Option<Float> = None;

        while c < sp.end || (c == sp.end && hard_end) {
            let (tc, tb) = self.term_from(c, b);
            let (pe_c, pe_b) = if tc <= sp.end { (tc, tb) } else { (sp.end, sp.bend) };
            let term = &self.flow.styles[tc];
            let attrs = term.para;
            let at_para_start = c == 0 || self.flow.text.as_bytes()[b - 1] == b'\n' || self.flow.text.as_bytes()[b - 1] == 0x0c;

            // Which number this item has (counting from the start of its list if it began on an earlier page).
            if attrs.list == ListKind::Numbered {
                number = Some(match number {
                    Some(n) if at_para_start => n + 1,
                    Some(n) => n,
                    None => {
                        let (pc, pb) = if at_para_start { (c, b) } else { self.para_start(c) };
                        list_number(self, pc, pb)
                    }
                });
            } else {
                number = None;
            }
            let is_picture = self.picture_in(c, pe_c).is_some();
            let marker = match (attrs.list, at_para_start && !is_picture) {
                (ListKind::Bullet, true) => Some("\u{2022}".to_owned()),
                (ListKind::Numbered, true) => Some(format!("{}.", number.unwrap_or(1))),
                _ => None,
            };

            let local = |x: usize| x - sp.start;
            let piece_marks: Vec<Mark> = marks
                .iter()
                .filter(|m| m.range.end > local(c) && m.range.start < local(pe_c))
                .map(|m| Mark { range: m.range.start.saturating_sub(local(c))..m.range.end.min(local(pe_c)) - local(c), color: m.color })
                .collect();
            let spec = PieceSpec {
                text: &self.flow.text[b..pe_b],
                styles: &self.flow.styles[c..pe_c],
                term,
                attrs,
                scale,
                content_width,
                marks: &piece_marks,
                invisible: pe_c == c && self.flow.text[tb..].starts_with(PAGE_BREAK) && tc == pe_c,
                marker,
                image: self.picture_in(c, pe_c),
                contents: None,
            };
            let cap = self.drop_cap(ctx, c, b).filter(|_| at_para_start);
            let beside = float.take().and_then(|f| {
                let here = y / scale;
                if here < f.bottom - 0.5 && cap.is_none() && !is_picture {
                    Some((f, self.beside_cap(ctx, (c, b), (pe_c, pe_b), f.bottom - here, f.indent)))
                } else {
                    y = y.max(f.bottom * scale);
                    None
                }
            });
            match (cap, beside) {
                (_, Some((f, (chars, h)))) => {
                    let mut pieces = layout_beside(ctx, &spec, chars, f.indent, local(c), y);
                    // The lines beside take the height worked out at page size (R4).
                    y += h * scale;
                    for p in pieces.iter_mut().filter(|p| !p.beside) {
                        p.y = y;
                        y += p.height;
                    }
                    if c + chars >= pe_c {
                        float = Some(f);
                    }
                    paras.extend(pieces);
                }
                (Some(cap), None) => {
                    let top = y;
                    let pieces = layout_with_cap(ctx, &spec, cap, local(c), y);
                    y = pieces.iter().map(|p| p.y + p.height).fold(y, f32::max);
                    if c + 1 + cap.beside >= pe_c && let Some(f) = Float::after(&cap, top / scale) {
                        y = top + cap.beside_height * scale;
                        float = Some(f);
                    }
                    paras.extend(pieces);
                }
                (None, None) => {
                    let mut p = layout_piece(ctx, &spec, local(c), local(pe_c), y);
                    p.starts_paragraph = at_para_start;
                    y += p.height;
                    paras.push(p);
                }
            }

            if tc >= sp.end {
                break;
            }
            c = tc + 1;
            b = tb + 1;
        }
        PageLayout { paras, height: y, scale }
    }
}

/// A visual row of a page.
#[derive(Clone, Copy, Debug)]
pub struct RowRef {
    pub para: usize,
    /// Page-local range of chars on this row.
    pub start: usize,
    pub end: usize,
    /// Top and height relative to the writing area.
    pub top: f32,
    pub height: f32,
}

impl PageLayout {
    /// Index of the paragraph piece the page-local position belongs to. Where one piece of a
    /// paragraph ends and the next begins, that is the next one if `prefer_next_row` (as at a
    /// line wrap), and always the lines beside a drop cap rather than the cap.
    pub fn piece_of(&self, local: usize, prefer_next_row: bool) -> Option<usize> {
        let k = self
            .paras
            .iter()
            .position(|p| local >= p.start && local <= p.end)
            .or_else(|| self.paras.len().checked_sub(1))?;
        let next = self.paras.get(k + 1).filter(|n| n.start == local && (n.beside || prefer_next_row));
        Some(if next.is_some() { k + 1 } else { k })
    }

    /// The caret rectangle (a thin vertical bar) relative to the writing area.
    pub fn caret_rect(&self, local: usize, prefer_next_row: bool) -> Rect {
        let Some(k) = self.piece_of(local, prefer_next_row) else {
            return Rect::from_min_size(pos2(0.0, 0.0), vec2(1.0, 14.0 * self.scale));
        };
        let p = &self.paras[k];
        if let Some(block) = p.block() {
            // Before a picture or the contents: its left edge; after it: its right edge. The
            // contents keep a normal, single-line caret rather than growing with the chapter list.
            let x = if local <= p.start { block.left() } else { block.right() };
            let height = if p.contents.is_some() { p.galley.size().y } else { p.height };
            return Rect::from_min_max(pos2(x, p.y), pos2(x + 1.0, p.y + height));
        }
        let idx = local.clamp(p.start, p.end) - p.start;
        let r = p.galley.pos_from_cursor(CCursor { index: idx.into(), prefer_next_row }).translate(vec2(p.x, p.y));
        if p.cap {
            // The cap's line reaches high above the letter: keep the caret beside it.
            return Rect::from_x_y_ranges(r.x_range(), p.y..=p.y + p.height);
        }
        r
    }

    /// Where a dragged picture would land for a pointer at height `y`: the paragraph start nearest
    /// to it, as (page-local position, height of the line to draw).
    pub fn drop_boundary(&self, y: f32) -> Option<(usize, f32)> {
        self.paras
            .iter()
            .filter(|p| p.starts_paragraph)
            .map(|p| (p.start, p.y))
            .min_by(|a, b| (a.1 - y).abs().total_cmp(&(b.1 - y).abs()))
    }

    /// The picture paragraph under a point relative to the writing area.
    pub fn image_at(&self, pos: Vec2) -> Option<&ParaLayout> {
        self.paras.iter().find(|p| {
            p.image.is_some_and(|img| img.rect.translate(vec2(0.0, p.y)).contains(pos.to_pos2()))
        })
    }

    /// Page-local position closest to a point relative to the writing area.
    pub fn hit(&self, pos: Vec2) -> usize {
        let Some(mut k) = self.paras.iter().position(|p| pos.y < p.y + p.height).or(self.paras.len().checked_sub(1)) else {
            return 0;
        };
        // Right of a drop cap are the lines beside it: of its own paragraph, or of the ones after
        // it there, whichever are at that height.
        if self.paras[k].cap {
            let beside = self.paras.iter().enumerate().skip(k + 1).take_while(|(_, n)| n.beside);
            if let Some((j, _)) = beside.filter(|(_, n)| pos.x >= n.x && n.y <= pos.y).last() {
                k = j;
            }
        }
        let p = &self.paras[k];
        if let Some(block) = p.block() {
            return if pos.x < block.center().x { p.start } else { p.end };
        }
        let idx = usize::from(p.galley.cursor_from_pos(vec2(pos.x - p.x, pos.y - p.y)).index);
        p.start + idx.min(p.end - p.start)
    }

    /// The page a contents line under a point (relative to the writing area) leads to.
    pub fn link_at(&self, pos: Vec2) -> Option<usize> {
        let p = self.paras.iter().find(|p| p.contents.is_some() && pos.y >= p.y && pos.y < p.y + p.height)?;
        let at = (pos - vec2(p.x, p.y)).to_pos2();
        p.contents.as_ref()?.links.iter().find(|(r, _)| r.contains(at)).map(|&(_, page)| page)
    }

    pub fn rows(&self) -> Vec<RowRef> {
        let mut out = Vec::new();
        for (k, p) in self.paras.iter().enumerate() {
            let mut start = p.start;
            for r in p.row_table() {
                out.push(RowRef { para: k, start, end: start + r.chars, top: p.y + r.y, height: r.height });
                start += r.chars;
            }
        }
        out
    }

    /// The visual row containing the page-local position (the later one at a wrap point).
    pub fn row_of(&self, local: usize, prefer_next_row: bool) -> Option<RowRef> {
        let rows = self.rows();
        let k = self.piece_of(local, prefer_next_row)?;
        let in_piece: Vec<&RowRef> = rows.iter().filter(|r| r.para == k).collect();
        let last = in_piece.len().checked_sub(1)?;
        // A row ends where the next one starts, so at a wrap point the caret is on this row only
        // if it prefers the end of it to the start of the next.
        for (n, r) in in_piece.iter().enumerate() {
            let on_end = local == r.end && (n == last || !prefer_next_row);
            if local >= r.start && (local < r.end || on_end) {
                return Some(**r);
            }
        }
        in_piece.last().map(|r| **r)
    }

    /// The middle of every char on the page (a picture counts as one char), relative to the
    /// writing area, with its page-local position.
    pub fn char_centers(&self) -> Vec<(usize, egui::Pos2)> {
        let mut out = Vec::new();
        for r in self.rows() {
            let p = &self.paras[r.para];
            if let Some(block) = p.block() {
                out.push((p.start, block.translate(vec2(0.0, p.y)).center()));
                continue;
            }
            let x = |c: usize, next: bool| p.galley.pos_from_cursor(CCursor { index: (c - p.start).into(), prefer_next_row: next }).left();
            for c in r.start..r.end {
                out.push((c, pos2(p.x + (x(c, true) + x(c + 1, false)) / 2.0, r.top + r.height / 2.0)));
            }
        }
        out
    }

    /// Rectangles (relative to the writing area) covering page-local `a..b`, one per row piece.
    pub fn selection_rects(&self, a: usize, b: usize) -> Vec<Rect> {
        let mut out = Vec::new();
        for r in self.rows() {
            let p = &self.paras[r.para];
            if let Some(block) = p.block() {
                if a < p.end && b > p.start {
                    out.push(block.translate(vec2(0.0, p.y)));
                }
                continue;
            }
            let (s, e) = (a.max(r.start), b.min(r.end));
            let includes_end = b > p.end && r.end == p.end && p.galley.rows.last().is_some();
            if s >= e && !(includes_end && a <= r.end) {
                continue;
            }
            let (s, e) = (s.min(e), e.max(s));
            let x0 = p.galley.pos_from_cursor(CCursor { index: (s - p.start).into(), prefer_next_row: true }).left();
            let x1 = p.galley.pos_from_cursor(CCursor { index: (e - p.start).into(), prefer_next_row: false }).left();
            let tail = if includes_end { 5.0 * self.scale } else { 0.0 };
            out.push(Rect::from_min_max(
                pos2(p.x + x0, r.top),
                pos2(p.x + x1.max(x0) + tail, r.top + r.height),
            ));
        }
        out
    }
}

/// Run `f` inside an egui frame, so fonts exist (tests only).
#[cfg(test)]
pub fn with_ctx(f: impl FnOnce(&egui::Context)) {
    let ctx = egui::Context::default();
    let mut f = Some(f);
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        if let Some(f) = f.take() {
            f(ui.ctx());
        }
    });
    out.textures_delta.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with(text: &str, attrs: ParaAttrs) -> Doc {
        let mut d = Doc::new();
        let st = Style::new("x");
        d.flow.text = format!("{text}\n");
        d.flow.styles = vec![st.clone(); text.chars().count()];
        d.flow.styles.push(st.with_para(attrs));
        d
    }

    #[test]
    fn alignment_moves_the_text_within_the_writing_width() {
        with_ctx(|ctx| {
            let width = 300.0;
            let x_of = |align| {
                let mut d = doc_with("hello", ParaAttrs { align, ..Default::default() });
                d.setup.margin_left = 0.0;
                d.setup.margin_right = d.setup.size().x - width;
                d.full_paginate(ctx, &Style::new("x"));
                let l = d.layout_page(ctx, 0, 1.0, &[]);
                (l.caret_rect(0, true).left(), l.caret_rect(5, true).left())
            };
            let left = x_of(Align::Left);
            let center = x_of(Align::Center);
            let right = x_of(Align::Right);
            assert!(left.0 < 1.0, "left aligned starts at 0: {left:?}");
            let text_w = left.1 - left.0;
            assert!((center.0 - (width - text_w) / 2.0).abs() < 2.0, "centered: {center:?} text_w {text_w}");
            assert!((right.1 - width).abs() < 2.0, "right aligned ends at the edge: {right:?}");
        });
    }

    #[test]
    fn justified_text_keeps_a_tab_before_it_and_after_it() {
        with_ctx(|ctx| {
            let justified = ParaAttrs { align: Align::Justify, ..Default::default() };
            let plain = ParaAttrs::default();
            let long = format!("\t{}", "an indented paragraph long enough to wrap onto a few rows. ".repeat(4).trim_end());
            let row0 = |text: &str, attrs| {
                let mut d = doc_with(text, attrs);
                d.full_paginate(ctx, &Style::new("x"));
                let l = d.layout_page(ctx, 0, 1.0, &[]);
                let r = &l.paras[0].galley.rows[0];
                let letter = &r.row.glyphs[1];
                let right = r.row.glyphs.iter().rev().find(|g| !g.chr.is_whitespace()).map(|g| r.pos.x + g.pos.x + g.advance_width);
                (r.pos.x + letter.pos.x, right.unwrap(), d.setup.content_size().x, l)
            };
            let (indent, _, _, _) = row0(&long, plain);
            let (first, right, width, _) = row0(&long, justified);
            assert!(indent > 5.0 && (first - indent).abs() < 0.5, "indented by the tab, like left aligned text: {first} vs {indent}");
            assert!((right - width).abs() < 1.5, "and still out to the margin: {right} of {width}");

            // At the end, the caret goes past a tab or a space as soon as it is typed.
            for end in ['\t', ' '] {
                let (_, _, _, l) = row0(&format!("Short{end}"), justified);
                let (_, _, _, without) = row0("Short", justified);
                assert!(l.caret_rect(6, true).left() > without.caret_rect(5, true).left() + 2.0, "{end:?} at the end");
            }
        });
    }

    #[test]
    fn text_fills_the_page_the_same_at_every_zoom() {
        with_ctx(|ctx| {
            let long = "a paragraph long enough to wrap onto a few rows of the page. ".repeat(4);
            let text: String = (0..60).map(|k| if k % 5 == 0 { format!("{long}\n") } else { "qwe\n".into() }).collect();
            let st = Style::new("x");
            let mut d = Doc::new();
            d.flow.text = text.clone();
            d.flow.styles = vec![st.clone(); text.chars().count()];
            d.full_paginate(ctx, &st);
            let at_page_size = d.layout_page(ctx, 0, 1.0, &[]).height;
            for sc in [0.8f32, 0.9, 0.94, 1.2, 1.37] {
                let h = d.layout_page(ctx, 0, sc, &[]).height / sc;
                assert!((h - at_page_size).abs() < 0.01, "at zoom {sc} the text is {h}pt tall instead of {at_page_size}pt");
            }
        });
    }

    #[test]
    fn a_chapter_title_is_drawn_larger_and_bold() {
        with_ctx(|ctx| {
            let mut title = ParaAttrs::default();
            title.set_chapter_title(true);
            let laid_out = |attrs| {
                let mut d = doc_with("Chapter One", attrs);
                d.full_paginate(ctx, &Style::new("x"));
                let l = d.layout_page(ctx, 0, 1.0, &[]);
                (l.height, l.paras[0].galley.size().x)
            };
            let (body, title) = (laid_out(ParaAttrs::default()), laid_out(title));
            assert!(title.0 > body.0 * 1.4 && title.0 < body.0 * 1.8, "taller: {body:?} {title:?}");
            assert!(title.1 > body.1 * 1.5, "wider, from the size and the bold: {body:?} {title:?}");
        });
    }

    #[test]
    fn line_spacing_makes_lines_taller_and_lists_indent() {
        with_ctx(|ctx| {
            let long = "word ".repeat(60);
            let height = |spacing: f32, list| {
                let mut d = doc_with(&long, ParaAttrs { spacing, list, ..Default::default() });
                d.full_paginate(ctx, &Style::new("x"));
                let l = d.layout_page(ctx, 0, 1.0, &[]);
                (l.height, l.paras[0].x, l.paras[0].marker.is_some())
            };
            let single = height(1.0, ListKind::None);
            let double = height(2.0, ListKind::None);
            assert!(double.0 > single.0 * 1.8 && double.0 < single.0 * 2.2, "{single:?} {double:?}");
            let bullet = height(1.0, ListKind::Bullet);
            assert!(bullet.1 > 20.0 && bullet.2, "bullet items are indented and have a marker: {bullet:?}");
            assert!(bullet.0 >= single.0, "narrower text wraps into at least as many rows");
        });
    }

    #[test]
    fn lines_keep_their_spacing_when_zoomed_text_wraps_differently() {
        with_ctx(|ctx| {
            let st = Style::new("x");
            let text = "Edwin konnte die ganze Nacht nicht schlafen, er starrte and die Decke und stellte sich vor, dass sie sich ploetzlich oeffnet und der gestreifte Junge mit langen Hoernern und gefletschten Zaehnen auf ihn herabblicken wuerde. Warum hatten sie ihn im Dachstuhl versteckt?";
            let mut seen = 0;
            for n in (150..=text.len()).filter(|&n| text.is_char_boundary(n)) {
                let mut d = doc_with(&text[..n], ParaAttrs::default());
                d.full_paginate(ctx, &st);
                let page_rows = d.layout_page(ctx, 0, 1.0, &[]).paras[0].galley.rows.len();
                for sc in [0.7237f32, 0.85, 1.13, 1.37, 1.6] {
                    let g = d.layout_page(ctx, 0, sc, &[]).paras[0].galley.clone();
                    if g.rows.len() < 2 {
                        continue;
                    }
                    if g.rows.len() != page_rows {
                        seen += 1;
                    }
                    let pitch = g.rows[1].pos.y - g.rows[0].pos.y;
                    for w in g.rows.windows(2) {
                        let step = w[1].pos.y - w[0].pos.y;
                        assert!((step - pitch).abs() <= 1.0, "{n} chars at zoom {sc}: rows {} pt apart, not {pitch}", step);
                    }
                    let font = egui::FontId::new(st.size * sc, family_for(&st.font, false, false).0);
                    let normal = ctx.fonts_mut(|f| f.row_height(&font));
                    assert!((pitch - normal).abs() <= 1.0, "{n} chars at zoom {sc} ({} rows, {page_rows} at page size): rows {pitch} apart, not {normal}", g.rows.len());
                }
            }
            assert!(seen > 0, "some zoom wraps the text differently from page size");
        });
    }
}
