//! Drop caps: the first letter of a chapter, enlarged to sink a few lines deep, with the
//! paragraph's first lines wrapped narrower beside it.
//!
//! egui lays a paragraph out at one wrap width and cannot flow text around a box, so a paragraph
//! with a drop cap is laid out as three pieces: the cap, the lines beside it, and the rest at full
//! width. Where they go is worked out at page size, so that pagination and drawing at any zoom
//! agree on which chars sit beside the cap and how tall that block is.

use std::sync::Arc;

use eframe::egui::{self, Color32, vec2};

use crate::layout::{Mark, ParaLayout, PieceSpec, layout_piece, paragraph_job, run_format};
use crate::model::{Align, Doc, ListKind, ParaAttrs, ParaKind, Style};

/// Chars of a paragraph looked at to find the lines beside the cap (far more than they hold).
const WINDOW: usize = 4000;

/// Where a paragraph's drop cap goes, in page points relative to the paragraph's top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropCap {
    /// Font size of the cap.
    pub size: f32,
    /// How far the cap's galley is moved down so its baseline is that of the last line beside it.
    pub dy: f32,
    /// Left edge of the lines beside the cap.
    pub indent: f32,
    /// Chars after the cap on the lines beside it.
    pub beside: usize,
    /// Height of the cap and the lines beside it together.
    pub height: f32,
    /// Where the letter's ink ends below the paragraph's top: until there, the paragraphs after
    /// a short one go on beside it (`Float`).
    pub bottom: f32,
    /// Where the lines of its own paragraph beside it end.
    pub beside_height: f32,
}

/// The room left beside a drop cap once its paragraph has ended, in page points from the top
/// and left of the writing area: the paragraphs after it go on there, narrower, until `bottom`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Float {
    pub bottom: f32,
    pub indent: f32,
}

impl Float {
    /// The room beside the cap of a paragraph at `top`, if its own text leaves some.
    pub fn after(cap: &DropCap, top: f32) -> Option<Float> {
        (cap.beside_height < cap.bottom - 0.5).then_some(Float { bottom: top + cap.bottom, indent: cap.indent })
    }
}

/// Is the byte a paragraph terminator? (`\n` and the page break are both one byte.)
fn ends_paragraph(byte: u8) -> bool {
    byte == b'\n' || byte == 0x0c
}

fn cap_style(first: &Style, size: f32) -> Style {
    Style { size, ..first.clone() }
}

/// The one glyph of a galley holding a single char.
fn glyph(galley: &egui::Galley) -> Option<&egui::epaint::text::Glyph> {
    galley.rows.first()?.row.glyphs.first()
}

/// Baseline of each of the first `n` rows, relative to the galley's top. Rows the paragraph does
/// not have are where they would be, a line further down each.
fn baselines(galley: &egui::Galley, n: usize) -> Vec<f32> {
    let row_base = |r: &egui::epaint::text::PlacedRow| r.pos.y + r.row.glyphs.first().map_or(r.size.y * 0.8, |g| g.pos.y);
    let Some(last) = galley.rows.last() else { return vec![0.0; n] };
    let pitch = galley.rows.first().map_or(last.size.y, |r| r.size.y);
    (0..n)
        .map(|k| match galley.rows.get(k) {
            Some(r) => row_base(r),
            None => row_base(last) + (k + 1 - galley.rows.len()) as f32 * pitch,
        })
        .collect()
}

impl Doc {
    /// Is the paragraph that starts at (char `c`, byte `b`) the first of a chapter? That is the
    /// first paragraph with text after a chapter title; empty lines between them are skipped.
    pub fn starts_chapter(&self, c: usize, b: usize) -> bool {
        let bytes = self.flow.text.as_bytes();
        let (mut pc, mut pb) = (c, b);
        loop {
            if pc == 0 {
                return false;
            }
            if self.flow.styles[pc - 1].para.is_chapter_title() {
                return true;
            }
            let before_empty = pc == 1 || ends_paragraph(bytes[pb - 2]);
            if !before_empty {
                return false;
            }
            (pc, pb) = (pc - 1, pb - 1);
        }
    }

    /// Does the paragraph that starts at (char `c`, byte `b`) get a drop cap? It does if drop caps
    /// are turned on, and it is ordinary left-aligned or justified text, beginning with a letter
    /// or digit, that starts a chapter.
    pub fn has_drop_cap(&self, c: usize, b: usize) -> bool {
        if self.setup.drop_cap_lines < 2 {
            return false;
        }
        let (tc, _) = self.term_from(c, b);
        let attrs = self.flow.styles[tc].para;
        let plain = attrs.kind == ParaKind::Body && attrs.list == ListKind::None && matches!(attrs.align, Align::Left | Align::Justify);
        let first = self.flow.text[b..].chars().next().filter(|ch| ch.is_alphanumeric());
        plain && tc > c && first.is_some() && self.flow.styles[c].image == 0 && self.starts_chapter(c, b)
    }

    /// Where the drop cap of the paragraph that starts at (char `c`, byte `b`) goes, if it has one.
    pub fn drop_cap(&self, ctx: &egui::Context, c: usize, b: usize) -> Option<DropCap> {
        if !self.has_drop_cap(c, b) {
            return None;
        }
        let lines = usize::from(self.setup.drop_cap_lines);
        let (tc, _) = self.term_from(c, b);
        let term = &self.flow.styles[tc];
        let attrs = term.para;
        let first = self.flow.text[b..].chars().next()?;
        let first_st = &self.flow.styles[c];
        let (cb, end) = (b + first.len_utf8(), tc.min(c + 1 + WINDOW));
        let eb = cb + self.flow.text[cb..].char_indices().nth(end - c - 1).map_or(self.flow.text.len() - cb, |(o, _)| o);
        let (text, styles) = (&self.flow.text[cb..eb], &self.flow.styles[c + 1..end]);
        // Nothing after the letter yet: its lines are measured with a stand-in in its style, as the
        // first one typed will be, or it sat a little lower until then (an empty line has no
        // baseline to measure).
        let stand_in = [first_st.clone()];
        let empty = text.is_empty();
        let (text, styles) = if empty { ("x", &stand_in[..]) } else { (text, styles) };
        let width = self.setup.content_size().x;
        // The job is built before taking the fonts: with line spacing it asks them for a row's height.
        let layout = |w: f32| {
            let job = paragraph_job(ctx, text, styles, term, attrs, 1.0, w, &[]);
            ctx.fonts_mut(|f| f.layout_job(job))
        };
        let letter = |size: f32| {
            let mut job = egui::text::LayoutJob::default();
            job.append(&first.to_string(), 0.0, run_format(ctx, &cap_style(first_st, size), 1.0, Color32::TRANSPARENT, ParaAttrs::default()));
            ctx.fonts_mut(|f| f.layout_job(job))
        };

        // Scale the letter so it reaches from the top of the first line's capitals down to the
        // baseline of the last line it sinks into. A letter that hangs below its baseline (j, p, q,
        // y) is made smaller instead, so its tail ends with the last line rather than reaching into
        // the next: it fits the lines set, top to tail.
        let whole = layout(width);
        let full = baselines(&whole, lines);
        let below = whole.rows.first().map_or(0.0, |r| r.pos.y + r.size.y - full[0]);
        // Measured large: at text size its ink is a few whole pixels, too coarse to scale from.
        const REF: f32 = 200.0;
        let large = letter(REF);
        let g0 = glyph(&large)?;
        let (rise, descent) = (-g0.uv_rect.offset.y / REF, (g0.uv_rect.offset.y + g0.uv_rect.size.y).max(0.0) / REF);
        if rise * first_st.size < 1.0 {
            return None;
        }
        // In units of the letter's size: from its top on the first line to the last line's baseline.
        let span = full[lines - 1] - full[0] + rise * first_st.size;
        let size = (span / rise).min((span + below) / (rise + descent));
        let (k, rise) = (size / first_st.size, rise * first_st.size);
        let cap = letter(size);
        let g = glyph(&cap)?;
        let gap = first_st.size * 0.35;
        let indent = g.pos.x + g.uv_rect.offset.x + g.uv_rect.size.x + gap;
        if width - indent < width * 0.3 {
            return None; // a huge letter on a narrow page: leave the text be
        }

        // Its top where the first line's letters' tops are; its baseline follows from its size
        // (the last line's, unless it was made smaller to keep its tail in).
        let narrow = layout(width - indent);
        let base = baselines(&narrow, lines)[0] - rise + k * rise;
        let dy = base - (cap.rows[0].pos.y + g.pos.y);
        let cap_bottom = base + g.uv_rect.offset.y + g.uv_rect.size.y;
        // Should a tail still reach into the next line (a font whose letters run deeper than its
        // lines), that line goes beside it too, rather than the text below starting under the
        // tail and leaving a line empty beside it.
        let reached = narrow.rows.iter().take_while(|r| r.pos.y < cap_bottom - 1.0).count();
        let n = reached.max(lines).min(narrow.rows.len());
        let beside: usize = if empty { 0 } else { narrow.rows[..n].iter().map(|r| usize::from(r.row.char_count_excluding_newline())).sum() };
        let beside_bottom = if beside == 0 { 0.0 } else { narrow.rows[n - 1].pos.y + narrow.rows[n - 1].size.y };
        // A tail ending within a point of the last line is in it (its ink is rounded to pixels).
        let cap_bottom = if cap_bottom <= beside_bottom + 1.0 { cap_bottom.min(beside_bottom) } else { cap_bottom };
        let height = cap_bottom.max(beside_bottom);
        Some(DropCap { size, dy, indent, beside, height, bottom: cap_bottom, beside_height: beside_bottom })
    }

    /// How much of the paragraph from (char `c`, byte `b`), on a page until `end` (char, byte),
    /// goes beside a cap whose room reaches `room` below where it starts: its chars on the rows
    /// that start there, and their height. Worked out at page size, for pagination and drawing alike.
    pub fn beside_cap(&self, ctx: &egui::Context, (c, b): (usize, usize), end: (usize, usize), room: f32, indent: f32) -> (usize, f32) {
        let (tc, _) = self.term_from(c, b);
        let term = &self.flow.styles[tc];
        let ec = end.0.min(c + WINDOW);
        let eb = b + self.flow.text[b..].char_indices().nth(ec - c).map_or(end.1 - b, |(o, _)| o);
        let spec = PieceSpec {
            text: &self.flow.text[b..eb],
            styles: &self.flow.styles[c..ec],
            term,
            attrs: term.para,
            scale: 1.0,
            content_width: self.setup.content_size().x - indent,
            marks: &[],
            invisible: false,
            marker: None,
            image: None,
            contents: None,
        };
        let rows = layout_piece(ctx, &spec, 0, ec - c, 0.0).row_table();
        let n = rows.iter().take_while(|r| r.y < room - 0.5).count().max(1).min(rows.len());
        let chars = rows[..n].iter().map(|r| r.chars).sum();
        (chars, rows.get(n - 1).map_or(0.0, |r| r.y + r.height))
    }
}

/// Keep only the rows of a laid-out piece that hold its first `n` chars.
fn keep_chars(p: &mut ParaLayout, n: usize) {
    let mut seen = 0;
    let rows = p.galley.rows.iter().take_while(|r| {
        let before = seen;
        seen += usize::from(r.row.char_count_excluding_newline());
        before < n
    });
    let keep = rows.count().max(1);
    if keep == p.galley.rows.len() {
        return;
    }
    let g = Arc::make_mut(&mut p.galley);
    g.rows.truncate(keep);
    let bottom = g.rows.last().map_or(0.0, |r| r.pos.y + r.size.y);
    g.rect.max.y = g.rect.min.y + bottom;
    g.mesh_bounds.max.y = g.mesh_bounds.max.y.min(g.rect.max.y);
    p.height = bottom;
}

/// The chars `from..to` of a piece, `width` wide, without its list marker.
fn sub<'a>(spec: &PieceSpec<'a>, from: usize, to: usize, marks: &'a [Mark], width: f32) -> PieceSpec<'a> {
    let byte_at = |k: usize| spec.text.char_indices().nth(k).map_or(spec.text.len(), |(o, _)| o);
    PieceSpec {
        text: &spec.text[byte_at(from)..byte_at(to)],
        styles: &spec.styles[from..to],
        content_width: width,
        marks,
        invisible: false,
        marker: None,
        image: None,
        contents: None,
        ..*spec
    }
}

/// The marks of a piece that fall on its chars `from..to`, counted from `from`.
fn marks_in(spec: &PieceSpec, from: usize, to: usize) -> Vec<Mark> {
    spec.marks
        .iter()
        .filter(|m| m.range.end > from && m.range.start < to)
        .map(|m| Mark { range: m.range.start.max(from) - from..m.range.end.min(to) - from, color: m.color })
        .collect()
}

/// The first `chars` of a piece laid out narrow, right of `indent` (page points), and the rest
/// below them at full width, from `y`. The narrow lines are justified like the paragraph's
/// others, though egui never justifies a layout's last line: a little of the rest is laid out with
/// them, then dropped again.
fn narrow_then_rest(ctx: &egui::Context, spec: &PieceSpec, (from, chars): (usize, usize), indent: f32, local_start: usize, y: f32) -> Vec<ParaLayout> {
    let sc = spec.scale;
    let total = spec.styles.len();
    let split = (from + chars).min(total);
    let mut pieces = Vec::new();
    let mut below = y;
    // An empty paragraph beside a cap still has its line there, for the caret.
    if split > from || (from == 0 && total == 0) {
        let more = if spec.attrs.align == Align::Justify && split < total { (split + 200).min(total) } else { split };
        let marks = marks_in(spec, from, more);
        let narrow = PieceSpec { marker: if from == 0 { spec.marker.clone() } else { None }, ..sub(spec, from, more, &marks, spec.content_width - indent * sc) };
        let mut p = layout_piece(ctx, &narrow, local_start + from, local_start + split, y);
        keep_chars(&mut p, split - from);
        p.x += indent * sc;
        p.beside = true;
        below = y + p.height;
        pieces.push(p);
    }
    if split < total {
        let marks = marks_in(spec, split, total);
        pieces.push(layout_piece(ctx, &sub(spec, split, total, &marks, spec.content_width), local_start + split, local_start + total, below));
    }
    pieces
}

/// Lay out a paragraph that starts beside a drop cap above it (`Float`): its first `chars` beside
/// the cap, the rest below at full width. `local_start` is where it begins on the page, `y` its top.
pub fn layout_beside(ctx: &egui::Context, spec: &PieceSpec, chars: usize, indent: f32, local_start: usize, y: f32) -> Vec<ParaLayout> {
    let mut pieces = narrow_then_rest(ctx, spec, (0, chars), indent, local_start, y);
    if let Some(p) = pieces.first_mut() {
        p.starts_paragraph = true;
    }
    pieces
}

/// Lay out a paragraph with a drop cap: the cap, the lines beside it and the rest (of what is on
/// this page). `spec` holds the paragraph's text on this page, which starts with the cap.
/// `local_start` is the page-local char where it begins, `y` its top.
pub fn layout_with_cap(ctx: &egui::Context, spec: &PieceSpec, cap: DropCap, local_start: usize, y: f32) -> Vec<ParaLayout> {
    let sc = spec.scale;
    // The cap: one big letter, moved down onto the baseline of the last line beside it.
    let first = spec.text.chars().next().unwrap_or(' ');
    let st = cap_style(&spec.styles[0], cap.size);
    let color = marks_in(spec, 0, 1).last().map_or(Color32::TRANSPARENT, |m| m.color);
    let mut job = egui::text::LayoutJob::default();
    job.append(&first.to_string(), 0.0, run_format(ctx, &st, sc, color, ParaAttrs::default()));
    let mut galley = ctx.fonts_mut(|f| f.layout_job(job));
    let ppp = ctx.pixels_per_point();
    let dy = (cap.dy * sc * ppp).round() / ppp;
    let g = Arc::make_mut(&mut galley);
    for row in &mut g.rows {
        row.pos.y += dy;
    }
    g.rect = g.rect.translate(vec2(0.0, dy));
    g.mesh_bounds = g.mesh_bounds.translate(vec2(0.0, dy));
    let height = cap.height * sc;
    let mut pieces = vec![ParaLayout {
        start: local_start,
        end: local_start + 1,
        y,
        x: 0.0,
        height,
        galley,
        marker: None,
        attrs: spec.attrs,
        image: None,
        starts_paragraph: true,
        beside: false,
        cap: true,
        contents: None,
    }];

    // The lines beside it, then the rest at full width below the cap.
    let mut rest = narrow_then_rest(ctx, spec, (1, cap.beside), cap.indent, local_start, y);
    if let Some(r) = rest.last_mut().filter(|r| !r.beside) {
        r.y = y + height;
    }
    pieces.extend(rest);
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::with_ctx;

    const STORY: &str = "It was nearly midnight and the Prime Minister was sitting alone in his office, reading a long \
memo that was slipping through his brain without leaving the slightest trace of meaning behind. He was waiting for a call \
from the President of a far distant country, and between wondering when the wretched man would telephone, and trying to \
suppress unpleasant memories of what had been a very long, tiring and difficult week, there was not much space in his head \
for anything else.";

    /// A document of paragraphs, each with its format.
    fn doc(paras: &[(&str, ParaAttrs)], lines: u8) -> Doc {
        let mut d = Doc::new();
        let st = Style::new("x");
        d.flow.text.clear();
        d.flow.styles.clear();
        for (text, attrs) in paras {
            d.flow.text.push_str(text);
            d.flow.text.push('\n');
            d.flow.styles.extend(std::iter::repeat_n(st.clone(), text.chars().count()));
            d.flow.styles.push(st.with_para(*attrs));
        }
        d.setup.drop_cap_lines = lines;
        d
    }

    fn title() -> ParaAttrs {
        let mut t = ParaAttrs::default();
        t.set_chapter_title(true);
        t
    }

    fn body() -> ParaAttrs {
        ParaAttrs::default()
    }

    #[test]
    fn the_first_paragraph_with_text_after_a_title_starts_the_chapter() {
        let d = doc(&[("Before", body()), ("One", title()), ("", body()), ("", body()), ("Text", body()), ("More", body())], 3);
        let starts: Vec<bool> = [0, 7, 11, 12, 13, 18].iter().map(|&c| d.starts_chapter(c, d.char_to_byte(c))).collect();
        assert_eq!(starts, [false, false, true, true, true, false]);
    }

    #[test]
    fn only_plain_text_starting_with_a_letter_gets_a_cap_and_only_when_turned_on() {
        with_ctx(|ctx| {
            let cap = |first: &str, attrs: ParaAttrs, lines: u8| {
                let d = doc(&[("One", title()), (first, attrs)], lines);
                d.drop_cap(ctx, 4, 4).is_some()
            };
            assert!(cap(STORY, body(), 3) && cap(STORY, body(), 2));
            assert!(!cap(STORY, body(), 0), "turned off");
            assert!(!cap("\u{ab} Hello \u{bb}, she said.", body(), 3), "starts with a quote");
            assert!(!cap(STORY, ParaAttrs { align: Align::Center, ..body() }, 3));
            assert!(!cap(STORY, ParaAttrs { list: ListKind::Bullet, ..body() }, 3));
            assert!(!cap(STORY, title(), 3), "a second title line");
            assert!(cap(STORY, ParaAttrs { align: Align::Justify, ..body() }, 3));
        });
    }

    #[test]
    fn the_cap_sinks_as_many_lines_deep_as_set() {
        with_ctx(|ctx| {
            let line = {
                let d = doc(&[(STORY, body())], 0);
                let l = d.layout_page(ctx, 0, 1.0, &[]);
                l.paras[0].galley.rows[0].size.y
            };
            for lines in [2u8, 3] {
                let mut d = doc(&[("One", title()), (STORY, ParaAttrs { align: Align::Justify, ..body() })], lines);
                d.full_paginate(ctx, &Style::new("x"));
                let cap = d.drop_cap(ctx, 4, 4).unwrap();
                let depth = cap.height / line;
                assert!(depth > f32::from(lines) - 0.5 && depth < f32::from(lines) + 0.5, "{lines} lines: {cap:?}, line {line}");

                let l = d.layout_page(ctx, 0, 1.0, &[]);
                let (head, cap_p, beside, rest) = (&l.paras[0], &l.paras[1], &l.paras[2], &l.paras[3]);
                assert!(cap_p.cap && cap_p.starts_paragraph && (cap_p.start, cap_p.end) == (4, 5));
                assert!(beside.beside && beside.y == cap_p.y && beside.x >= cap.indent - 0.01);
                assert_eq!(beside.galley.rows.len(), usize::from(lines), "the lines beside the cap");
                let widths: Vec<f32> = beside.galley.rows.iter().map(|r| r.rect().width()).collect();
                assert!(widths.iter().all(|w| (w - widths[0]).abs() < 1.0), "all justified to the same width: {widths:?}");
                assert_eq!((beside.start, beside.end, rest.start), (5, 5 + cap.beside, 5 + cap.beside));
                assert!((rest.y - (head.height + cap.height)).abs() < 0.01 && rest.x == 0.0, "the rest goes on below the cap");
                assert_eq!(rest.end, 4 + STORY.chars().count(), "and ends the paragraph");
            }
        });
    }

    #[test]
    fn a_cap_hanging_below_its_line_is_made_smaller_to_keep_its_tail_in_the_lines_set() {
        with_ctx(|ctx| {
            let story = format!("p{}", &STORY[1..]);
            for text in [story.as_str(), STORY] {
                let mut d = doc(&[("One", title()), (text, ParaAttrs { align: Align::Justify, ..body() })], 3);
                d.full_paginate(ctx, &Style::new("x"));
                let cap = d.drop_cap(ctx, 4, 4).unwrap();
                let l = d.layout_page(ctx, 0, 1.0, &[]);
                let (cap_p, beside, rest) = (&l.paras[1], &l.paras[2], &l.paras[3]);
                let tail = cap_p.y + cap_p.galley.mesh_bounds.max.y;
                let first = &text[..1];
                assert_eq!(beside.galley.rows.len(), 3, "{first:?}: three lines beside the cap, as set");
                assert!((rest.y - (beside.y + beside.height)).abs() < 0.01, "{first:?}: the rest goes on at once");
                assert!(tail <= rest.y + 1.0, "{first:?}: its tail within the three lines: {tail} > {}", rest.y);
                assert_eq!(rest.start, 5 + cap.beside);
            }
        });
    }

    #[test]
    fn the_cap_stays_put_when_the_first_letter_after_it_is_typed() {
        with_ctx(|ctx| {
            let spaced = ParaAttrs { align: Align::Justify, spacing: 1.15, ..body() };
            for attrs in [body(), spaced] {
                let cap_of = |text: &str| {
                    let mut d = doc(&[("One", title()), (text, attrs)], 3);
                    d.full_paginate(ctx, &Style::new("x"));
                    d.drop_cap(ctx, 4, 4).unwrap()
                };
                let (alone, typed) = (cap_of("I"), cap_of("It"));
                assert!((alone.dy - typed.dy).abs() < 0.01 && (alone.size - typed.size).abs() < 0.01, "{alone:?} then {typed:?}");
                assert_eq!(alone.beside, 0, "nothing beside it yet");
            }
        });
    }

    #[test]
    fn clicks_and_the_caret_find_the_text_beside_the_cap() {
        with_ctx(|ctx| {
            let mut d = doc(&[("One", title()), (STORY, body())], 3);
            d.full_paginate(ctx, &Style::new("x"));
            let l = d.layout_page(ctx, 0, 1.0, &[]);
            let (cap, beside) = (&l.paras[1], &l.paras[2]);
            let row2 = beside.galley.rows[1].pos.y + beside.galley.rows[1].size.y / 2.0;
            let hit = l.hit(vec2(beside.x + 2.0, beside.y + row2));
            let second_row_start = beside.start + usize::from(beside.galley.rows[0].row.char_count_excluding_newline());
            assert_eq!(hit, second_row_start, "the start of the second line beside the cap");
            assert!(l.hit(vec2(2.0, cap.y + cap.height / 2.0)) <= cap.end, "on the cap");

            let after_cap = l.caret_rect(5, false);
            assert!((after_cap.left() - beside.x).abs() < 1.0, "after the cap the caret starts the line beside it: {after_cap:?}");
            let on_cap = l.caret_rect(4, false);
            assert!(on_cap.top() >= cap.y - 0.01 && on_cap.bottom() <= cap.y + cap.height + 0.01);
            let rows = l.rows();
            assert_eq!(rows.iter().filter(|r| (r.top - cap.y).abs() < 0.01).count(), 2, "the cap and the first line beside it");
        });
    }

    #[test]
    fn a_chapter_fills_the_page_the_same_at_every_zoom() {
        with_ctx(|ctx| {
            let paras: Vec<(&str, ParaAttrs)> = (0..8).flat_map(|_| [("One", title()), (STORY, body()), (STORY, body())]).collect();
            let mut d = doc(&paras, 3);
            d.full_paginate(ctx, &Style::new("x"));
            assert!(d.pages() >= 2);
            let limit = d.setup.content_size().y + 0.5;
            for page in 0..d.pages() {
                let at_page_size = d.layout_page(ctx, page, 1.0, &[]).height;
                assert!(at_page_size <= limit, "page {page} holds {at_page_size}pt of {limit}pt");
                for sc in [0.8f32, 1.37] {
                    let h = d.layout_page(ctx, page, sc, &[]).height / sc;
                    assert!((h - at_page_size).abs() < 0.05, "page {page} at zoom {sc}: {h} instead of {at_page_size}");
                }
            }
        });
    }

    #[test]
    fn a_cap_and_its_lines_never_split_over_two_pages() {
        with_ctx(|ctx| {
            let mut d = doc(&[("One", title()), (STORY, body())], 3);
            let line = 14.0;
            // Move the chapter down until its cap no longer fits under the title.
            let fits = d.setup.content_size().y;
            let filler = ((fits / line) as usize).saturating_sub(4);
            let mut paras: Vec<(&str, ParaAttrs)> = vec![("x", body()); filler];
            paras.extend([("One", title()), (STORY, body())]);
            for extra in 0..6 {
                let mut p = paras.clone();
                p.splice(0..0, std::iter::repeat_n(("x", body()), extra));
                d = doc(&p, 3);
                d.full_paginate(ctx, &Style::new("x"));
                let c = d.flow.text.find(STORY).unwrap();
                let c = d.flow.text[..c].chars().count();
                let page = d.page_of(c);
                let cap = d.drop_cap(ctx, c, d.char_to_byte(c)).unwrap();
                assert!(d.page_of(c + cap.beside) == page, "with {extra} more lines the cap's lines moved apart");
                if d.spans[page].start == c {
                    return; // moved to the next page whole
                }
            }
            panic!("the chapter never reached the bottom of a page");
        });
    }

    #[test]
    fn editing_a_title_repaginates_the_chapter_after_it_like_from_scratch() {
        with_ctx(|ctx| {
            let st = Style::new("x");
            // The chapter's text starts a page of its own, so the page before it ends where it did.
            let mut paras: Vec<(&str, ParaAttrs)> = vec![("Opening", body()), ("One", title())];
            paras.extend(std::iter::repeat_n((STORY, body()), 12));
            let mut d = doc(&paras, 3);
            let at = d.flow.text.find("One\n").unwrap() + 3;
            d.flow.text.replace_range(at..at + 1, "\u{c}");
            d.full_paginate(ctx, &st);
            assert!(d.pages() >= 3 && d.spans[1].start == at + 1);
            for on in [false, true] {
                let old = vec![d.flow.styles[at].clone()];
                let mut attrs = old[0].para;
                attrs.set_chapter_title(on);
                let new = vec![old[0].with_para(attrs)];
                d.apply(crate::edit::Edit::Restyle { at, old, new }, 0.0);
                assert_eq!(d.drop_cap(ctx, at + 1, at + 1).is_some(), on);
                d.paginate_after(ctx, &st, at, at + 1, 0, 0);
                let incremental = d.spans.clone();
                d.full_paginate(ctx, &st);
                assert_eq!(incremental, d.spans, "with the title {}", if on { "back" } else { "taken off" });
            }
        });
    }
}
