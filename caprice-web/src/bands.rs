//! Headers and footers ("bands"): one line of text in the top or bottom margin of every page, in
//! one of a few looks (`BandLook`), like the galleries Word offers. They are page settings
//! (`PageSetup::header`, `footer`), saved with the story, and take no room from the text: pages
//! break where they would without them. A band is drawn as `Ink`, so the page shown, a page
//! turning and print all draw the same.
//!
//! A band shows one thing: the page number (`{page}`), the chapter's title (`{chapter}`), both
//! filled in for each page when it is drawn, or text of the writer's own. In Word the two are
//! fields too (`PAGE`, `STYLEREF`), see `export.rs`.

use std::f32::consts::PI;

use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};

use crate::App;
use crate::contents::Entry;
use crate::fonts::family_for;
use crate::layout::Ink;
use crate::model::{Band, BandLook, CHAPTER_FIELD, Doc, PAGE_FIELD, PageSetup};
use crate::theme::{PAPER, pal};

/// The size of a band's text, in page points.
pub const BAND_SIZE: f32 = 10.0;
/// The grey of a band's text when it is not on colour.
const BAND_INK: Color32 = Color32::from_gray(96);
/// How thick its hairlines are, in page points.
const HAIRLINE: f32 = 0.7;

pub fn rgb([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The band's text with its fields filled in: the page's `number` as written, its `chapter`.
pub fn filled(text: &str, number: &str, chapter: &str) -> String {
    text.replace(PAGE_FIELD, number).replace(CHAPTER_FIELD, chapter)
}

/// The text a band starts with when it is put on: the chapter on top, the page number below.
pub fn first_text(look: BandLook, top: bool) -> &'static str {
    if top && look != BandLook::Circle { CHAPTER_FIELD } else { PAGE_FIELD }
}

/// An outline like a leaf from `a` to `b`, `w` wide at its middle: two arcs meeting at both ends.
/// It is convex, as `Ink::Fill` needs.
fn leaf(a: Pos2, b: Pos2, w: f32) -> Vec<Pos2> {
    const N: usize = 10;
    let along = b - a;
    let across = vec2(-along.y, along.x).normalized() * w / 2.0;
    let side = |t: f32, k: f32| a + along * t + across * k * (PI * t).sin();
    let mut out: Vec<Pos2> = (0..=N).map(|j| side(j as f32 / N as f32, 1.0)).collect();
    out.extend((1..N).rev().map(|j| side(j as f32 / N as f32, -1.0)));
    out
}

/// A circle as an outline of many sides.
fn circle(c: Pos2, r: f32) -> Vec<Pos2> {
    (0..40).map(|k| c + Vec2::angled(k as f32 / 40.0 * 2.0 * PI) * r).collect()
}

/// A little fleuron hanging from the point `at` on a hairline, towards the text (`d`: 1 down, -1
/// up): a drop in the middle, a leaf curling out to each side and a bud at their root.
fn fleuron(at: Pos2, d: f32, sc: f32, color: Color32) -> Vec<Ink> {
    let p = |x: f32, y: f32| at + vec2(x, y * d) * sc;
    vec![
        Ink::Fill(leaf(p(0.0, 1.0), p(0.0, 10.0), 3.4 * sc), color),
        Ink::Fill(leaf(p(-0.8, 1.6), p(-8.0, 6.5), 3.0 * sc), color),
        Ink::Fill(leaf(p(0.8, 1.6), p(8.0, 6.5), 3.0 * sc), color),
        Ink::Fill(leaf(p(-3.5, 0.0), p(3.5, 0.0), 3.2 * sc), color),
    ]
}

/// Everything `band` draws on a page of `setup` at scale `sc`, relative to the page's top-left:
/// on top (`top`) a header, else a footer, showing `text` (its fields filled in) in `font`. Only
/// a band in the Facing look needs the page's `number`.
#[allow(clippy::too_many_arguments)]
pub fn band_ink(ctx: &egui::Context, band: &Band, top: bool, text: &str, number: usize, setup: &PageSetup, font: &str, sc: f32) -> Vec<Ink> {
    if !band.is_on() {
        return Vec::new();
    }
    let size = setup.size();
    let (left, right) = (setup.margin_left, size.x - setup.margin_right);
    // In the middle of the margin, as a footer always was; `d` points towards the text.
    let (cy, d) = if top { (setup.margin_top / 2.0, 1.0) } else { (size.y - setup.margin_bottom / 2.0, -1.0) };
    let accent = rgb(band.color);
    let on_colour = matches!(band.look, BandLook::Banner | BandLook::Circle);
    let color = if on_colour { Color32::WHITE } else { BAND_INK };
    let font = FontId::new(BAND_SIZE * sc, family_for(font, band.look == BandLook::Circle, false).0);
    let galley = ctx.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font, color));
    let (tw, th) = (galley.size().x / sc, galley.size().y / sc);
    let at = |x: f32, y: f32| pos2(x, y) * sc;
    let line = |y: f32, from: f32, to: f32| Ink::Line(vec![at(from, y), at(to, y)], HAIRLINE * sc, accent);
    let centred = |x: f32| Ink::Text(galley.clone(), at(x - tw / 2.0, cy - th / 2.0));
    let rule_y = cy + d * (th / 2.0 + 4.0);
    match band.look {
        BandLook::None => Vec::new(),
        BandLook::Plain => vec![centred((left + right) / 2.0)],
        BandLook::Rule => vec![centred((left + right) / 2.0), line(rule_y, left, right)],
        BandLook::Filigree => {
            let inset = (right - left) * 0.1;
            let mut ink = vec![centred((left + right) / 2.0), line(rule_y, left + inset, right - inset)];
            ink.extend(fleuron(at((left + right) / 2.0, rule_y), d, sc, accent));
            ink
        }
        BandLook::Banner => {
            let band = Rect::from_min_max(at(left, cy - th / 2.0 - 4.0), at(right, cy + th / 2.0 + 4.0));
            vec![Ink::Fill(vec![band.left_top(), band.right_top(), band.right_bottom(), band.left_bottom()], accent), centred((left + right) / 2.0)]
        }
        BandLook::Facing => {
            // Even pages are on the left of a spread: their text goes to the outer, left edge.
            let x = if number.is_multiple_of(2) { left } else { right - tw };
            vec![Ink::Text(galley.clone(), at(x, cy - th / 2.0)), line(rule_y, left, right)]
        }
        BandLook::Circle => {
            let mid = (left + right) / 2.0;
            let r = tw.max(th) / 2.0 + 5.0;
            vec![Ink::Fill(circle(at(mid, cy), r * sc), accent), centred(mid)]
        }
    }
}

impl Doc {
    /// The title of the chapter page `i` is in: the first that starts on it, else the last before
    /// it (as Word's `STYLEREF` finds it). `chapters` are `chapters(false)`.
    pub fn chapter_of_page(&self, chapters: &[Entry], i: usize) -> String {
        let sp = self.spans[i];
        if sp.contents.is_some() {
            return String::new();
        }
        let on = chapters.iter().find(|e| e.at >= sp.start && e.at < sp.end.max(sp.start + 1));
        on.or_else(|| chapters.iter().rev().find(|e| e.at < sp.start)).map_or(String::new(), |e| e.title.clone())
    }

    /// The header and footer of page `i`, at scale `sc`, relative to the page's top-left.
    /// `chapters` are `chapters(false)`, asked for only if a band shows the chapter.
    pub fn bands_ink(&self, ctx: &egui::Context, i: usize, sc: f32, chapters: &dyn Fn() -> Vec<Entry>) -> Vec<Ink> {
        let s = &self.setup;
        if !s.header.is_on() && !s.footer.is_on() {
            return Vec::new();
        }
        let wants_chapter = [&s.header, &s.footer].iter().any(|b| b.is_on() && b.text.contains(CHAPTER_FIELD));
        let chapter = if wants_chapter { self.chapter_of_page(&chapters(), i) } else { String::new() };
        let font = self.contents_style().font;
        let number = self.page_number(i);
        let mut ink = Vec::new();
        for (band, top) in [(&s.header, true), (&s.footer, false)] {
            // A page without its number shows none of a band showing it, not even its look.
            let Some(shown) = number.label().or_else(|| (!band.text.contains(PAGE_FIELD)).then(String::new)) else { continue };
            let text = filled(&band.text, &shown, &chapter);
            ink.extend(band_ink(ctx, band, top, &text, number.count as usize, s, &font, sc));
        }
        ink
    }
}

/// A gallery tile: the edge of a small page with `band` on it, its name above. Returns the click.
pub fn band_tile(ui: &mut egui::Ui, band: &Band, top: bool, chosen: bool, font: &str) -> egui::Response {
    const W: f32 = 230.0;
    // A narrow page, so the band is large enough to see in a small tile.
    let setup = PageSetup { paper_w: 300.0, paper_h: 300.0, margin_left: 26.0, margin_right: 26.0, margin_top: 44.0, margin_bottom: 44.0, ..PageSetup::default() };
    let sc = W / setup.size().x;
    let strip = setup.margin_top + 22.0;
    ui.label(egui::RichText::new(band.look.name()).size(12.0).color(pal().text_dim));
    let (rect, resp) = ui.allocate_exact_size(vec2(W, strip * sc), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, band.look.name()));
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    p.rect_filled(rect, 2.0, PAPER);
    // The page the strip is cut from: the top of it for a header, the foot for a footer.
    let page_top = if top { rect.top() } else { rect.bottom() - setup.size().y * sc };
    // A few lines of the story, to show where the band sits against it.
    let first = if top { setup.margin_top + 6.0 } else { setup.size().y - setup.margin_bottom - 18.0 };
    for k in 0..3 {
        let y = page_top + (first + k as f32 * 7.0) * sc;
        let r = Rect::from_min_max(pos2(rect.left() + setup.margin_left * sc, y), pos2(rect.right() - setup.margin_right * sc - k as f32 * 18.0, y + 2.5 * sc));
        p.rect_filled(r, 1.0, Color32::from_gray(222));
    }
    let sample = filled(if band.text.is_empty() { first_text(band.look, top) } else { &band.text }, "2", "Chapter One");
    for ink in band_ink(ui.ctx(), band, top, &sample, 2, &setup, font, sc) {
        p.extend(ink.translate(vec2(rect.left(), page_top)).shape());
    }
    let edge = if chosen { Stroke::new(2.0, crate::theme::pal().accent) } else if resp.hovered() { Stroke::new(1.0, pal().text_dim) } else { Stroke::NONE };
    ui.painter().rect_stroke(rect, 2.0, edge, egui::StrokeKind::Outside);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Colours offered for the lines and bands of headers, footers and text boxes.
pub const ACCENTS: [(&str, [u8; 3]); 8] = [
    ("Blue", crate::model::ACCENT_INK),
    ("Slate", [90, 96, 110]),
    ("Black", [30, 30, 34]),
    ("Dark red", [150, 40, 40]),
    ("Gold", [176, 136, 40]),
    ("Green", [60, 120, 80]),
    ("Teal", [30, 120, 125]),
    ("Plum", [112, 60, 130]),
];

/// A row of colour swatches; the one clicked, if any.
pub fn accent_row(ui: &mut egui::Ui, current: [u8; 3]) -> Option<[u8; 3]> {
    let mut pick = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (name, c) in ACCENTS {
            let (r, swatch) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
            swatch.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
            ui.painter().rect_filled(r, 4.0, rgb(c));
            if c == current || swatch.hovered() {
                ui.painter().rect_stroke(r.expand(1.5), 5.0, Stroke::new(1.5, crate::theme::pal().text), egui::StrokeKind::Outside);
            }
            if swatch.on_hover_text(name).clicked() {
                pick = Some(c);
            }
        }
    });
    pick
}

impl App {
    /// The header (`top`) or footer, new. Like the other page settings it is not undoable (R1),
    /// but unlike them it changes no page break, so nothing is paginated again.
    pub fn set_band(&mut self, top: bool, band: Band) {
        let slot = if top { &mut self.doc.setup.header } else { &mut self.doc.setup.footer };
        if *slot != band {
            *slot = band;
            self.doc.version += 1;
        }
    }

    /// The header and footer of page `i`, drawn on a page at `rect`.
    pub fn draw_bands(&self, ui: &egui::Ui, rect: Rect, i: usize) {
        let ink = self.doc.bands_ink(ui.ctx(), i, self.scale_of(rect), &|| self.doc.chapters(false));
        ui.painter().extend(ink.into_iter().filter_map(|k| k.translate(rect.min.to_vec2()).shape()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_filled_in_for_each_page() {
        assert_eq!(filled("{page}", "3", "The Mill"), "3");
        assert_eq!(filled("{chapter}", "iii", "The Mill"), "The Mill");
        assert_eq!(filled("no fields", "3", "x"), "no fields");
    }

    #[test]
    fn a_leaf_is_convex_and_reaches_from_end_to_end() {
        let pts = leaf(pos2(0.0, 0.0), pos2(10.0, 0.0), 4.0);
        assert!(pts.contains(&pos2(0.0, 0.0)) && pts.iter().any(|p| (p.x - 10.0).abs() < 1e-4));
        // Every turn goes the same way round.
        let turns: Vec<f32> = (0..pts.len())
            .map(|k| {
                let (a, b, c) = (pts[k], pts[(k + 1) % pts.len()], pts[(k + 2) % pts.len()]);
                (b - a).x * (c - b).y - (b - a).y * (c - b).x
            })
            .collect();
        assert!(turns.iter().all(|&t| t >= -1e-4) || turns.iter().all(|&t| t <= 1e-4), "{turns:?}");
    }
}
