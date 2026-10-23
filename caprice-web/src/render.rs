//! Painting pages: paper, page stacks, static pages and pages sliding to and from the pile behind.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, pos2, vec2};

use crate::{App, Appearance};
use crate::layout::Ink;
use crate::notes::{NOTE_OUT, NoteLook, mirror_note, note_color, note_rect};
use crate::theme::PAPER;

/// Which way, per sheet, the piles of pages ahead and behind spread out from under the page.
pub const AHEAD: egui::Vec2 = egui::vec2(1.0, 0.64);
pub const BEHIND: egui::Vec2 = egui::vec2(-1.0, 0.5);
/// Colour of the sheets in the pile ahead of the page.
const AHEAD_FILL: Color32 = Color32::from_rgb(232, 230, 224);
/// Colour of the sheets in the pile behind the page.
pub const BEHIND_FILL: Color32 = Color32::from_rgb(226, 224, 218);

impl App {
    /// Screen points per page point for a page drawn into `rect`.
    pub fn scale_of(&self, rect: Rect) -> f32 {
        rect.width() / self.doc.setup.size().x
    }

    pub fn paper(painter: &egui::Painter, rect: Rect) {
        for s in 1..=6u8 {
            let r = rect.expand(f32::from(s) * 2.5);
            painter.rect_filled(r.translate(vec2(0.0, 6.0)), 2.0, Color32::from_black_alpha(14));
        }
        painter.rect_filled(rect, 1.0, PAPER);
    }

    /// Page-point offset between neighbouring sheets for a document of `n` pages. The whole pile
    /// is `max * n / (n + STACK_KNEE)` thick: a few pages get thick, clearly separate sheets, and
    /// the thickness then grows ever more slowly towards a tenth of the page width.
    fn sheet_gap(&self, n: usize) -> f32 {
        const STACK_KNEE: f32 = 18.0;
        let max = self.doc.setup.size().x / 10.0;
        max / (n as f32 + STACK_KNEE)
    }

    /// Screen points between neighbouring sheets of the piles beside a page at `rect`.
    pub fn stack_gap(&self, ctx: &egui::Context, rect: Rect) -> f32 {
        let target = self.sheet_gap(self.doc.pages());
        // Ease the spacing when pages come and go, so the pile visibly thins or thickens.
        ctx.animate_value_with_time(egui::Id::new("page-stack-gap"), target, 0.3) * self.scale_of(rect)
    }

    /// The post-its of the pages in the piles beside the page at `rect`, deepest first: each one's
    /// id and page, and the part of it that sticks out from under the pages on top.
    pub fn pile_note_hits(&self, ctx: &egui::Context, rect: Rect, behind: usize, ahead: usize) -> Vec<(u64, usize, Rect)> {
        if self.doc.notes.is_empty() {
            return Vec::new();
        }
        let (gap, sc) = (self.stack_gap(ctx, rect), self.scale_of(rect));
        let places = self.note_places(ctx);
        let n = self.doc.pages();
        let mut hits = Vec::new();
        let mut add = |page: usize, visible: &dyn Fn(Rect) -> Rect, r: Rect| {
            for &(k, y) in places.get(&page).into_iter().flatten() {
                hits.push((self.doc.notes[k].id, page, visible(note_rect(r, sc, y))));
            }
        };
        for k in (1..=ahead).rev() {
            let above = rect.translate(AHEAD * (k - 1) as f32 * gap).right();
            let r = rect.translate(AHEAD * k as f32 * gap);
            add(n - ahead + k - 1, &|pr| Rect::from_min_max(pos2(pr.left().max(above), pr.top()), pr.max), r);
        }
        for k in (1..=behind).rev() {
            let above = rect.translate(BEHIND * (k - 1) as f32 * gap).left();
            let r = rect.translate(BEHIND * k as f32 * gap);
            add(k - 1, &|pr| {
                let pr = mirror_note(r, pr);
                Rect::from_min_max(pr.min, pos2(pr.right().min(above), pr.bottom()))
            }, r);
        }
        hits
    }

    /// Every how many sheets the piles beside a page at `rect` draw one, so they don't smear together.
    fn pile_step(&self, ctx: &egui::Context, rect: Rect) -> usize {
        let gap_px = self.stack_gap(ctx, rect) * ctx.pixels_per_point();
        (1.5 / gap_px).ceil().max(1.0) as usize
    }

    /// Stack hints: every sheet piled to the right (pages ahead) and left (pages behind). A page
    /// slides into the back of the left pile, so page `k - 1` lies `k` sheets out.
    pub fn stack(&self, painter: &egui::Painter, rect: Rect, behind: usize, ahead: usize) {
        let ctx = painter.ctx();
        let gap = self.stack_gap(ctx, rect);
        let gap_px = gap * ctx.pixels_per_point();
        // Sheets closer than ~1.5 px would smear into one dark edge: draw every `step`th one only,
        // and fade the edge lines as they crowd, leaving a soft blur for long documents.
        let step = self.pile_step(ctx, rect);
        let line = (gap_px / 3.0).clamp(0.25, 1.0);

        // Post-its of the pages in the pile peek out of it: to the right from pages ahead, and to
        // the left from pages behind (they moved over as their page slid there, or, in a book,
        // the page was turned over).
        let sc = self.scale_of(rect);
        let places = if self.doc.notes.is_empty() { Default::default() } else { self.note_places(ctx) };
        let n = self.doc.pages();

        let side = |count: usize, dir: egui::Vec2, fill: Color32, alpha: f32, page_at: &dyn Fn(usize) -> usize, left: bool| {
            for k in (1..=count).rev() {
                let r = rect.translate(dir * k as f32 * gap);
                for &(note, y) in places.get(&page_at(k)).into_iter().flatten() {
                    let n = &self.doc.notes[note];
                    let pr = note_rect(r, sc, y);
                    let pr = if left { mirror_note(r, pr) } else { pr };
                    if left && self.appearance == Appearance::Book {
                        // Turned over, a post-it shows its blank back.
                        painter.rect_filled(pr, 1.0, note_color(n.color).gamma_multiply(0.8).to_opaque());
                        painter.rect_stroke(pr, 1.0, Stroke::new(0.6, Color32::from_black_alpha(40)), egui::StrokeKind::Inside);
                    } else {
                        // It keeps its writing: the pages on top hide the rest.
                        let look = NoteLook { shade: 0.96, ..NoteLook::FLAT };
                        painter.extend(self.note_shapes(ctx, n, pr, sc, self.pad(n.id).sheet as f32, &look));
                    }
                }
                if k == count || k % step == 0 {
                    painter.rect_filled(r, 1.0, fill);
                    let edge = Color32::from_black_alpha((alpha * line) as u8);
                    painter.rect_stroke(r, 1.0, Stroke::new(0.6, edge), egui::StrokeKind::Inside);
                }
            }
        };
        side(ahead, AHEAD, AHEAD_FILL, 60.0, &|k| n - ahead + k - 1, false);
        side(behind, BEHIND, BEHIND_FILL, 50.0, &|k| k - 1, true);
    }

    /// What page `i` holds besides the story, at scale `sc` relative to the page's top-left: its
    /// header and footer, then the text boxes in front of the story on top. Every look and print
    /// draw it over the story's text and pictures, and `underlay` under them (R20, R24).
    /// `chapters` are `chapters(false)`, asked for only if a header or footer shows the chapter.
    pub fn furniture(&self, ctx: &egui::Context, i: usize, sc: f32, chapters: &dyn Fn() -> Vec<crate::contents::Entry>) -> Vec<Ink> {
        let mut ink = self.doc.bands_ink(ctx, i, sc, chapters);
        for b in self.doc.boxes_on(i).filter(|b| !b.behind) {
            ink.extend(crate::textbox::box_ink(ctx, b, sc, true));
        }
        ink
    }

    /// The text boxes of page `i` that lie behind the story's text and pictures, at scale `sc`
    /// relative to the page's top-left.
    pub fn underlay(&self, ctx: &egui::Context, i: usize, sc: f32) -> Vec<Ink> {
        self.doc.boxes_on(i).filter(|b| b.behind).flat_map(|b| crate::textbox::box_ink(ctx, b, sc, true)).collect()
    }

    /// Page `i`'s header, footer and text boxes in front, drawn on a page at `rect`.
    pub fn draw_furniture(&self, ui: &egui::Ui, rect: Rect, i: usize) {
        let ink = self.furniture(ui.ctx(), i, self.scale_of(rect), &|| self.doc.chapters(false));
        ui.painter().extend(ink.into_iter().filter_map(|k| k.translate(rect.min.to_vec2()).shape()));
    }

    /// Page `i`'s text boxes behind the story, drawn on a page at `rect`, before its text.
    pub fn draw_underlay(&self, ui: &egui::Ui, rect: Rect, i: usize) {
        let ink = self.underlay(ui.ctx(), i, self.scale_of(rect));
        ui.painter().extend(ink.into_iter().filter_map(|k| k.translate(rect.min.to_vec2()).shape()));
    }

    pub fn static_page(&self, ui: &egui::Ui, rect: Rect, i: usize) {
        Self::paper(ui.painter(), rect);
        ui.painter().extend(self.backdrop_shapes(ui.ctx(), i, rect, &|p| p, 1.0, 1.0));
        let sc = self.scale_of(rect);
        self.draw_underlay(ui, rect, i);
        let layout = self.page_layout(ui.ctx(), i, sc);
        self.paint_layout(ui.painter(), &layout, rect.min + self.doc.setup.margin_origin() * sc);
        self.draw_furniture(ui, rect, i);
        self.paint_notes(ui.ctx(), ui.painter(), rect, i, &NoteLook::FLAT);
    }

    /// Page `base` sliding off page `base + 1` into the back of the pile behind, `t` going from 0
    /// (lying on it) to 1 (at the back of the pile); going back, the other way round. It slides out
    /// to the left over everything until clear of the pile, its post-its staying put, then tucks
    /// in under the pile, its post-its moving over to its left edge as it goes in (nothing overlaps
    /// it out there, so it can go from over the pile to under it unseen). Coming `back` out of the
    /// pile, its post-its stay on its left edge until it slides over to the right onto the page,
    /// and move over to its right edge then. `under` draws what it lies on: page `base + 1`, or a
    /// page coming in or going out, with `ahead` pages in the pile ahead of that.
    #[allow(clippy::too_many_arguments)]
    pub fn slide_page(&self, ui: &egui::Ui, rect: Rect, base: usize, t: f32, back: bool, ahead: usize, under: &dyn Fn()) {
        let ease = |s: f32| s * s * (3.0 - 2.0 * s);
        let ctx = ui.ctx();
        let (gap, sc) = (self.stack_gap(ctx, rect), self.scale_of(rect));
        let pile = BEHIND * gap * (base + 1) as f32;
        // Out there its post-its sticking out to the right are left of the pile and the post-its
        // sticking out of it.
        let clear = vec2(-(rect.width() + gap * (base + 1) as f32 + (2.0 * NOTE_OUT + 12.0) * sc), pile.y);
        const OUT: f32 = 0.6;
        if t < OUT {
            let s = ease(t / OUT);
            self.stack(ui.painter(), rect, base, ahead);
            under();
            self.sheet_at(ui, rect, base, clear * s, 0.0, if back { s } else { 0.0 }, true);
        } else {
            let s = ease((t - OUT) / (1.0 - OUT));
            self.sheet_at(ui, rect, base, clear + (pile - clear) * s, s, if back { 1.0 } else { s }, true);
            self.stack(ui.painter(), rect, base, ahead);
            under();
        }
    }

    /// The paperstack's `book_slide_out`: the page after `i` was taken away, so it slides out
    /// `to_right` points to the right as `s` goes to 1, while page `i` comes back out of the pile
    /// over it as on any page back, its post-its with it. The page taken away is blank.
    pub fn paper_slide_out(&self, ui: &egui::Ui, rect: Rect, i: usize, s: f32, to_right: f32) {
        let ease = |s: f32| s * s * (3.0 - 2.0 * s);
        let leaving = || {
            let r = rect.translate(vec2(to_right * ease(s), 0.0));
            Self::paper(ui.painter(), r);
            ui.painter().extend(self.backdrop_shapes(ui.ctx(), i, r, &|p| p, 1.0, 1.0));
        };
        self.slide_page(ui, rect, i, 1.0 - s, true, self.last() - i, &leaving);
    }

    /// Where page `i` lies (an offset from the page) while the dragged scrollbar holds `held` pages'
    /// worth out from page `here` (see `App::held`). Ahead, the pages slide out over the page one
    /// after the other, up to half a page to the left, the ones out further a sheet apart. Back,
    /// they slide out of the back of the pile behind, up to half a page out to the left.
    pub fn held_offset(&self, ctx: &egui::Context, rect: Rect, i: usize, here: usize, held: f32) -> egui::Vec2 {
        let ease = |s: f32| s * s * (3.0 - 2.0 * s);
        let gap = self.stack_gap(ctx, rect);
        let half = vec2(-rect.width() / 2.0, 0.0);
        if i >= here {
            let j = (i - here) as f32;
            half * ease((held - j).clamp(0.0, 1.0)) - vec2(gap * (held - j - 1.0).max(0.0), 0.0)
        } else {
            let above = (here - 1 - i) as f32;
            let pile = BEHIND * gap * (i + 1) as f32;
            let out = half + BEHIND * gap * above;
            pile + (out - pile) * ease((-held - above).clamp(0.0, 1.0))
        }
    }

    /// The pages held out by the dragged scrollbar (see `held_offset`), and what lies around them.
    pub fn held_pages(&self, ui: &egui::Ui, rect: Rect, held: f32) {
        let (ctx, here, last) = (ui.ctx(), self.target, self.last());
        let k = held.abs().ceil() as usize;
        if held > 0.0 {
            // The page they uncover comes up from the pile ahead as they go.
            let under = (here + k).min(last);
            let shift = AHEAD * self.stack_gap(ctx, rect) * (k as f32 - held).max(0.0);
            self.stack(ui.painter(), rect, here, 0);
            self.stack(ui.painter(), rect.translate(shift), 0, last - under);
            self.static_page(ui, rect.translate(shift), under);
            let sheets: Vec<_> = (here..under).rev().map(|i| (i, self.held_offset(ctx, rect, i, here, held), 0.0)).collect();
            self.sheets(ui, rect, &sheets, 0.0);
        } else {
            let lo = here.saturating_sub(k);
            let sheets: Vec<_> = (lo..here).rev().map(|i| (i, self.held_offset(ctx, rect, i, here, held), 1.0)).collect();
            self.sheets(ui, rect, &sheets, 1.0);
            self.stack(ui.painter(), rect, lo, last - here);
            self.static_page(ui, rect, here);
        }
    }

    /// The pages let go of by the scrollbar going from page `from` to page `to` together, `s` going
    /// from 0 to 1, from where `held` held them (see `held_offset`). Ahead, they slide out to the
    /// left over everything until clear of the pile, then tuck in under it, their post-its moving
    /// over to their left edges as they go in. Back, they come out from under the pile until clear
    /// of it, their post-its staying on the left, then over it onto the page, their post-its
    /// moving back to the right as they go.
    pub fn batch_pages(&self, ui: &egui::Ui, rect: Rect, from: usize, to: usize, s: f32, held: f32) {
        let ease = |s: f32| s * s * (3.0 - 2.0 * s);
        let ctx = ui.ctx();
        let (gap, sc, last) = (self.stack_gap(ctx, rect), self.scale_of(rect), self.last());
        let (lo, hi) = (from.min(to), from.max(to));
        // Out there, their post-its sticking out to the right are left of the pile and its post-its.
        let clear_x = -(rect.width() + gap * hi as f32 + (2.0 * NOTE_OUT + 12.0) * sc);
        const OUT: f32 = 0.5;
        let (first, u) = if s < OUT { (true, ease(s / OUT)) } else { (false, ease((s - OUT) / (1.0 - OUT))) };
        if to > from {
            let at = |i: usize| {
                let pile = BEHIND * gap * (i + 1) as f32;
                let clear = vec2(clear_x - gap * (i - from) as f32, pile.y);
                if first {
                    let start = self.held_offset(ctx, rect, i, from, held);
                    (i, start + (clear - start) * u, 0.0)
                } else {
                    (i, clear + (pile - clear) * u, u)
                }
            };
            let sheets: Vec<_> = (from..to).rev().map(at).collect();
            // The page they uncover finishes coming up from the pile ahead.
            let rest = if first { (to - from) as f32 - held } else { 0.0 };
            let shift = AHEAD * gap * rest.max(0.0) * (1.0 - u);
            if !first {
                self.sheets(ui, rect, &sheets, u);
            }
            self.stack(ui.painter(), rect, from, 0);
            self.stack(ui.painter(), rect.translate(shift), 0, last - to);
            self.static_page(ui, rect.translate(shift), to);
            if first {
                self.sheets(ui, rect, &sheets, 0.0);
            }
        } else {
            let at = |i: usize| {
                let start = self.held_offset(ctx, rect, i, from, held);
                let clear = vec2(clear_x - gap * (from - 1 - i) as f32, start.y);
                if first {
                    (i, start + (clear - start) * u, 1.0)
                } else {
                    (i, clear + (AHEAD * gap * (i - to) as f32 - clear) * u, 1.0 - u)
                }
            };
            let sheets: Vec<_> = (to..from).rev().map(at).collect();
            // The page they land on goes into the pile ahead, under them.
            let shift = if first { egui::Vec2::ZERO } else { AHEAD * gap * (from - to) as f32 * u };
            if first {
                self.sheets(ui, rect, &sheets, 1.0 - u);
            }
            self.stack(ui.painter(), rect, lo, 0);
            self.stack(ui.painter(), rect.translate(shift), 0, last - hi);
            self.static_page(ui, rect.translate(shift), hi);
            if !first {
                self.sheets(ui, rect, &sheets, 0.0);
            }
        }
    }

    /// Draw pages lying apart (page, offset from `rect`, how far its post-its have moved over to
    /// the left), bottom one first. Only those with a good strip left uncovered show their text.
    fn sheets(&self, ui: &egui::Ui, rect: Rect, sheets: &[(usize, egui::Vec2, f32)], edge: f32) {
        for (m, &(i, offset, left)) in sheets.iter().enumerate() {
            let text = sheets.get(m + 1).is_none_or(|above| (above.1.x - offset.x).abs() > 20.0);
            self.sheet_at(ui, rect, i, offset, edge, left, text);
        }
    }

    /// Draw page `i` lying at `offset` from `rect`, its edge drawn in by `edge` (0 to 1), and its
    /// post-its moved over from its right edge to its left by `left` (0 to 1). Without `text`
    /// (covered anyway) just the paper and the post-its.
    #[allow(clippy::too_many_arguments)]
    fn sheet_at(&self, ui: &egui::Ui, rect: Rect, i: usize, offset: egui::Vec2, edge: f32, left: f32, text: bool) {
        let ctx = ui.ctx();
        let sheet = rect.translate(offset);
        Self::paper(ui.painter(), sheet);
        let sc = self.scale_of(sheet);
        if text {
            ui.painter().extend(self.backdrop_shapes(ctx, i, sheet, &|p| p, 1.0, 1.0));
            self.draw_underlay(ui, sheet, i);
            let layout = self.page_layout(ctx, i, sc);
            self.paint_layout(ui.painter(), &layout, sheet.min + self.doc.setup.margin_origin() * sc);
            self.draw_furniture(ui, sheet, i);
        }
        let r = note_rect(sheet, sc, 0.0);
        let shift = (mirror_note(sheet, r).left() - r.left()) * left;
        let map = move |p: Pos2| p + vec2(shift, 0.0);
        self.paint_notes(ctx, ui.painter(), sheet, i, &NoteLook { map: &map, ..NoteLook::FLAT });
        let line = (self.stack_gap(ctx, rect) * ctx.pixels_per_point() / 3.0).clamp(0.25, 1.0);
        let edge = Color32::from_black_alpha((50.0 * line * edge) as u8);
        ui.painter().rect_stroke(sheet, 1.0, Stroke::new(0.6, edge), egui::StrokeKind::Inside);
    }
}
