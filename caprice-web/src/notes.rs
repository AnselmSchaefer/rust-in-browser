//! Sticky notes: post-its anchored to text without being part of it. The text shows a highlight;
//! the post-it sits at the page's right edge, sticking out past it, at the height of its line, and
//! follows that line from page to page. Post-its have one fixed size: text that does not fit
//! continues on further sheets of the same pad, which flip up like a real pad of post-its.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Color32, FontId, Galley, Id, Pos2, Rect, Sense, Shape, Stroke, TextEdit, pos2, vec2};
use egui::epaint::{Mesh, TessellationOptions, Tessellator, text::LayoutJob};

use crate::App;
use crate::layout::Mark;
use crate::model::{NOTE_COLORS, Note};
use crate::theme::INK;

pub const SEARCH_FIELD: &str = "search_field";

/// A post-it is a square this many page points wide, and sticks out this far past the page's right edge.
pub const NOTE_SIZE: f32 = 116.0;
pub const NOTE_OUT: f32 = 74.0;
const NOTE_PAD: f32 = 8.0;
const NOTE_FONT: f32 = 9.5;
/// Room at the bottom of a sheet for the "‹ 2/3 ›" sheet counter.
const NOTE_FOOTER: f32 = 11.0;
/// Space kept between post-its that would otherwise overlap.
const NOTE_GAP: f32 = 6.0;

pub fn note_color(k: usize) -> Color32 {
    let (r, g, b) = NOTE_COLORS[k % NOTE_COLORS.len()];
    Color32::from_rgb(r, g, b)
}

fn shaded(c: Color32, shade: f32, alpha: f32) -> Color32 {
    let f = |v: u8| (f32::from(v) * shade).clamp(0.0, 255.0) as u8;
    Color32::from_rgba_premultiplied(f(c.r()), f(c.g()), f(c.b()), c.a()).gamma_multiply(alpha)
}

/// Which sheet of a note's pad is on top, and the animated position flipping towards it.
#[derive(Clone, Copy, Default)]
pub struct Pad {
    pub sheet: usize,
    pub pos: f32,
}

/// How a note's text, laid out at one scale, breaks into sheets.
struct Sheets {
    /// Where each sheet starts: char index into the text, and y in the galley.
    starts: Vec<(usize, f32)>,
}

impl Sheets {
    fn count(&self) -> usize {
        self.starts.len()
    }

    /// The sheet holding the galley row at height `y`.
    fn at_y(&self, y: f32) -> usize {
        self.starts.iter().rposition(|&(_, sy)| sy <= y + 0.5).unwrap_or(0)
    }
}

fn note_job(text: &str, sc: f32) -> LayoutJob {
    LayoutJob::simple(text.to_owned(), FontId::proportional(NOTE_FONT * sc), INK, (NOTE_SIZE - 2.0 * NOTE_PAD) * sc)
}

fn sheets(ctx: &egui::Context, text: &str, sc: f32) -> Sheets {
    let galley = ctx.fonts_mut(|f| f.layout_job(note_job(text, sc)));
    let row_h = galley.rows.first().map_or(NOTE_FONT * sc, |r| r.rect().height()).max(1.0);
    let per = (((NOTE_SIZE - 2.0 * NOTE_PAD - NOTE_FOOTER) * sc / row_h).floor() as usize).max(1);
    let mut starts = vec![(0, 0.0)];
    let mut c = 0;
    for (k, row) in galley.rows.iter().enumerate() {
        if k > 0 && k % per == 0 {
            starts.push((c, row.pos.y));
        }
        c += row.char_count_including_newline().0;
    }
    Sheets { starts }
}

/// How many sheets a note's text takes at scale `sc`.
#[cfg(test)]
pub fn sheet_count(ctx: &egui::Context, text: &str, sc: f32) -> usize {
    sheets(ctx, text, sc).count()
}

/// The text written on sheet `k`, laid out on its own.
fn sheet_galley(ctx: &egui::Context, text: &str, s: &Sheets, k: usize, sc: f32) -> Arc<Galley> {
    let from = s.starts[k].0;
    let to = s.starts.get(k + 1).map_or(usize::MAX, |&(c, _)| c);
    let part: String = text.chars().skip(from).take(to - from).collect();
    ctx.fonts_mut(|f| f.layout_job(note_job(part.trim_end_matches('\n'), sc)))
}

/// Where a note's post-it goes on a page drawn at `page`: its top is `y` page points down.
pub fn note_rect(page: Rect, sc: f32, y: f32) -> Rect {
    Rect::from_min_size(pos2(page.right() - (NOTE_SIZE - NOTE_OUT) * sc, page.top() + y * sc), vec2(NOTE_SIZE, NOTE_SIZE) * sc)
}

/// A post-it at `r` on a page at `page`, moved to stick out of the page's left edge instead.
pub fn mirror_note(page: Rect, r: Rect) -> Rect {
    r.translate(vec2(page.left() + page.right() - r.left() - r.right(), 0.0))
}

/// The text area of a post-it at `r`, and its footer line below it.
fn note_areas(r: Rect, sc: f32) -> (Rect, Rect) {
    let inner = r.shrink(NOTE_PAD * sc);
    let text = Rect::from_min_max(inner.min, pos2(inner.right(), inner.bottom() - NOTE_FOOTER * sc));
    let footer = Rect::from_min_max(pos2(inner.left(), text.bottom()), inner.max);
    (text, footer)
}

/// The sheet counter ("‹  2/3  ›") of a post-it at `r` on sheet `k` of `n`, and where it is drawn:
/// at the right of the footer. Clicking left of its middle turns back, right of it forward.
pub fn sheet_counter(ctx: &egui::Context, r: Rect, sc: f32, k: usize, n: usize) -> (Arc<Galley>, Rect) {
    let (_, footer) = note_areas(r, sc);
    let label = format!("\u{2039}  {}/{}  \u{203a}", k + 1, n);
    let g = ctx.fonts_mut(|f| f.layout_no_wrap(label, FontId::proportional(NOTE_FONT * 0.8 * sc), INK.gamma_multiply(0.55)));
    let at = Rect::from_min_size(pos2(footer.right() - g.size().x, footer.bottom() - g.size().y), g.size());
    (g, at)
}

/// A flat quad through `map`, in strips so it can bend with a turning page.
fn quad_mesh(r: Rect, color: Color32, map: &dyn Fn(Pos2) -> Pos2) -> Mesh {
    const STRIPS: usize = 6;
    let mut mesh = Mesh::default();
    for k in 0..=STRIPS {
        let x = r.left() + r.width() * k as f32 / STRIPS as f32;
        mesh.colored_vertex(map(pos2(x, r.top())), color);
        mesh.colored_vertex(map(pos2(x, r.bottom())), color);
        if k > 0 {
            let b = (k * 2) as u32;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    mesh
}

/// Galleys tessellated into one mesh, moved through `map` and tinted.
fn text_mesh(ctx: &egui::Context, galleys: &[(Pos2, Arc<Galley>)], map: &dyn Fn(Pos2) -> Pos2, shade: f32, alpha: f32) -> Mesh {
    let font_tex = ctx.fonts(|f| f.font_image_size());
    let mut tess = Tessellator::new(ctx.pixels_per_point(), TessellationOptions::default(), font_tex, vec![]);
    let mut mesh = Mesh::default();
    for (at, g) in galleys {
        tess.tessellate_shape(Shape::galley(*at, g.clone(), INK), &mut mesh);
    }
    for v in &mut mesh.vertices {
        v.pos = map(v.pos);
        v.color = shaded(v.color, shade, alpha);
    }
    mesh
}

/// How a post-it is to be drawn.
pub struct NoteLook<'a> {
    /// Takes a point of the flat page on screen to where it is drawn (the identity on a page at rest).
    pub map: &'a dyn Fn(Pos2) -> Pos2,
    pub shade: f32,
    pub alpha: f32,
    /// Seen from behind (on the back of a turning page): blank, and darker.
    pub back: bool,
    /// Leave the top sheet's text out (a text field draws it instead).
    pub no_text: bool,
}

impl NoteLook<'_> {
    pub const FLAT: NoteLook<'static> = NoteLook { map: &|p| p, shade: 1.0, alpha: 1.0, back: false, no_text: false };
}

impl App {
    /// Highlights for page `i`, relative to the page text: notes first, then search results on top.
    pub fn page_marks(&self, i: usize) -> Vec<Mark> {
        let sp = self.doc.spans[i];
        let local = |s: usize, e: usize| s.saturating_sub(sp.start)..e.min(sp.end).saturating_sub(sp.start);
        let mut marks = Vec::new();
        for n in &self.doc.notes {
            if n.end > n.start && n.end > sp.start && n.start < sp.end {
                marks.push(Mark { range: local(n.start, n.end), color: note_color(n.color).gamma_multiply(0.38) });
            }
        }
        if self.search.open {
            for (k, &(s, e)) in self.search.matches.iter().enumerate() {
                if e > sp.start && s < sp.end {
                    let color = if k == self.search.current {
                        Color32::from_rgba_unmultiplied(255, 140, 0, 190)
                    } else {
                        Color32::from_rgba_unmultiplied(255, 220, 0, 130)
                    };
                    marks.push(Mark { range: local(s, e), color });
                }
            }
        }
        marks
    }

    /// Attach a new note to the selection, or to the line the cursor is on, and start writing on it.
    pub fn add_note(&mut self, ctx: &egui::Context) {
        let (mut start, mut end) = self.selection();
        if start == end {
            // No selection: take the line the caret is on.
            let page = self.doc.page_of(self.caret);
            let layout = self.doc.layout_page(ctx, page, 1.0, &[]);
            let first = self.doc.spans[page].start;
            if let Some(row) = layout.row_of(self.caret - first, self.prefer_next) {
                (start, end) = (first + row.start, first + row.end);
            }
        }
        let id = self.doc.next_note_id;
        self.doc.next_note_id += 1;
        self.doc.notes.push(Note { id, start, end, text: String::new(), color: 0 });
        self.note_focus = Some(id);
        ctx.request_repaint();
    }

    /// Go to a note's page, the pages in between moving together, with the caret at its text, and
    /// start writing on it there.
    pub fn go_to_note(&mut self, ctx: &egui::Context, id: u64) {
        if let Some(start) = self.doc.notes.iter().find(|n| n.id == id).map(|n| n.start) {
            if self.appearance == crate::Appearance::Book {
                self.set_caret(ctx, start, false);
            } else {
                self.batch_to_caret(ctx, start);
            }
            self.note_focus = Some(id);
        }
    }

    /// Notes by the page they are on, each with the top of its post-it in page points: level with
    /// its line, pushed apart where they would overlap, and never above or below the page.
    pub fn note_places(&self, ctx: &egui::Context) -> HashMap<usize, Vec<(usize, f32)>> {
        let mut by_page: HashMap<usize, Vec<(usize, f32)>> = HashMap::new();
        for (k, n) in self.doc.notes.iter().enumerate() {
            by_page.entry(self.doc.page_of(n.start)).or_default().push((k, 0.0));
        }
        let max = (self.doc.setup.size().y - NOTE_SIZE).max(0.0);
        for (&page, places) in &mut by_page {
            let layout = self.doc.layout_page(ctx, page, 1.0, &[]);
            let (top, first) = (self.doc.setup.margin_origin().y, self.doc.spans[page].start);
            for (k, y) in places.iter_mut() {
                let line = layout.caret_rect(self.doc.notes[*k].start.saturating_sub(first), true);
                *y = top + line.top() - NOTE_PAD;
            }
            places.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
            let mut floor = 0.0f32;
            for (_, y) in places.iter_mut() {
                *y = y.max(floor);
                floor = *y + NOTE_SIZE + NOTE_GAP;
            }
            let mut ceil = max;
            for (_, y) in places.iter_mut().rev() {
                *y = y.min(ceil).max(0.0);
                ceil = *y - NOTE_SIZE - NOTE_GAP;
            }
        }
        by_page
    }

    pub fn pad(&self, id: u64) -> Pad {
        self.pads.get(&id).copied().unwrap_or_default()
    }

    /// Ease every pad towards its chosen sheet.
    pub fn animate_pads(&mut self, ui: &egui::Ui) {
        let dt = ui.input(|i| i.stable_dt).min(0.05);
        let notes = &self.doc.notes;
        self.pads.retain(|id, _| notes.iter().any(|n| n.id == *id));
        let mut moving = false;
        for p in self.pads.values_mut() {
            let diff = p.sheet as f32 - p.pos;
            let step = 4.5 * dt;
            p.pos = if diff.abs() <= step { p.sheet as f32 } else { p.pos + step * diff.signum() };
            moving |= p.pos != p.sheet as f32;
        }
        if moving {
            ui.ctx().request_repaint();
        }
    }

    /// The shapes of one note's post-it at `r`, with its pad showing sheet position `pos`
    /// (2.4: sheet 2 is lifting away, 40% of the way, uncovering sheet 3).
    pub fn note_shapes(&self, ctx: &egui::Context, note: &Note, r: Rect, sc: f32, pos: f32, look: &NoteLook) -> Vec<Shape> {
        let map = look.map;
        let color = note_color(note.color);
        if look.back {
            return vec![Shape::mesh(quad_mesh(r, shaded(color, look.shade * 0.8, look.alpha), map))];
        }
        let s = sheets(ctx, &note.text, sc);
        let n = s.count();
        let pos = pos.clamp(0.0, (n - 1) as f32);
        let base = pos.floor() as usize;
        let t = pos - base as f32;
        let mut out = vec![Shape::mesh(quad_mesh(r.translate(vec2(0.8, 1.8) * sc), Color32::from_black_alpha((40.0 * look.alpha) as u8), map))];

        // The sheets underneath show as edges below the top one.
        let under = (n - 1 - base).min(3);
        for j in (1..=under).rev() {
            let e = r.translate(vec2(0.5, 1.4) * sc * j as f32);
            out.push(Shape::mesh(quad_mesh(e, shaded(color, look.shade * (0.94 - 0.03 * j as f32), look.alpha), map)));
        }

        let (text_area, _) = note_areas(r, sc);
        let sheet = |k: usize, with_text: bool, map: &dyn Fn(Pos2) -> Pos2, shade: f32, out: &mut Vec<Shape>| {
            out.push(Shape::mesh(quad_mesh(r, shaded(color, shade, look.alpha), map)));
            // A faint band where the glue is.
            let glue = Rect::from_min_size(r.min, vec2(r.width(), 9.0 * sc));
            out.push(Shape::mesh(quad_mesh(glue, shaded(color, shade * 0.97, look.alpha), map)));
            let mut galleys = Vec::new();
            if with_text {
                galleys.push((text_area.min, sheet_galley(ctx, &note.text, &s, k, sc)));
            }
            if n > 1 {
                let (g, at) = sheet_counter(ctx, r, sc, k, n);
                galleys.push((at.min, g));
            }
            out.push(Shape::mesh(text_mesh(ctx, &galleys, map, shade, look.alpha)));
        };

        if t > 0.0 && base + 1 < n {
            // Sheet `base` swings up about its glued top edge, uncovering the next one.
            sheet(base + 1, true, map, look.shade, &mut out);
            let angle = t * std::f32::consts::FRAC_PI_2;
            let lift = move |p: Pos2| map(pos2(p.x, r.top() + (p.y - r.top()) * angle.cos()));
            sheet(base, true, &lift, look.shade * (1.0 - 0.3 * angle.sin()), &mut out);
        } else {
            sheet(base, !look.no_text, map, look.shade, &mut out);
        }
        out
    }

    /// The post-its of page `i` drawn on a page at `rect`, not interactive.
    pub fn paint_notes(&self, ctx: &egui::Context, painter: &egui::Painter, rect: Rect, i: usize, look: &NoteLook) {
        let sc = self.scale_of(rect);
        let places = self.note_places(ctx);
        for &(k, y) in places.get(&i).into_iter().flatten() {
            let note = &self.doc.notes[k];
            let pos = self.pad(note.id).sheet as f32;
            painter.extend(self.note_shapes(ctx, note, note_rect(rect, sc, y), sc, pos, look));
        }
    }

    /// The post-its of the page at rest: written on directly, with a menu for colour and deleting.
    pub fn draw_notes(&mut self, ui: &mut egui::Ui, rect: Rect, i: usize) {
        let ctx = ui.ctx().clone();
        let sc = self.scale_of(rect);
        let places = self.note_places(&ctx).remove(&i).unwrap_or_default();
        let mut delete = None;
        for (k, y) in places {
            let id = self.doc.notes[k].id;
            let r = note_rect(rect, sc, y);
            let mut pad = self.pad(id);
            let s = sheets(&ctx, &self.doc.notes[k].text, sc);
            pad.sheet = pad.sheet.min(s.count() - 1);
            pad.pos = pad.pos.min((s.count() - 1) as f32);

            let look = NoteLook { no_text: true, ..NoteLook::FLAT };
            ui.painter().extend(self.note_shapes(&ctx, &self.doc.notes[k], r, sc, pad.sheet as f32, &look));

            // The whole text is one field, shifted up so the top sheet's rows show through the sheet.
            let (area, footer) = note_areas(r, sc);
            let shift = s.starts[pad.sheet].1;
            let field_rect = Rect::from_min_size(area.min - vec2(0.0, shift), vec2(area.width(), shift + area.height()));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field_rect));
            // Only this sheet's rows: the area has room for part of one more, which would show cut in half.
            let next = s.starts.get(pad.sheet + 1).map_or(f32::INFINITY, |&(_, y)| y);
            let rows = Rect::from_min_size(area.min, vec2(area.width(), (next - shift).min(area.height())));
            child.set_clip_rect(rows.intersect(ui.clip_rect()));
            let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| ui.fonts_mut(|f| f.layout_job(note_job(buf.as_str(), sc)));
            let out = TextEdit::multiline(&mut self.doc.notes[k].text)
                .id(Id::new(("note_text", id)))
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::ZERO)
                // The caret is at least a row of this font tall, so it must be the post-it's own.
                .font(FontId::proportional(NOTE_FONT * sc))
                .desired_width(area.width())
                .min_size(field_rect.size())
                .text_color(INK)
                .layouter(&mut layouter)
                .show(&mut child);
            let resp = out.response.response;
            if self.note_focus == Some(id) {
                resp.request_focus();
                self.note_focus = None;
            }

            // Writing past the end of a sheet flips the pad to the sheet the cursor is on.
            if resp.has_focus() {
                if let Some(cr) = out.cursor_range {
                    let s = sheets(&ctx, &self.doc.notes[k].text, sc);
                    pad.sheet = s.at_y(out.galley.pos_from_cursor(cr.primary).center().y).min(s.count() - 1);
                }
            }
            if s.count() > 1 {
                // Split where the arrows are, not in the footer's middle: the counter is at its right.
                let (_, counter) = sheet_counter(&ctx, r, sc, pad.sheet, s.count());
                let (prev, next) = footer.split_left_right_at_x(counter.center().x);
                if ui.interact(prev, Id::new(("note_prev", id)), Sense::click()).clicked() {
                    pad.sheet = pad.sheet.saturating_sub(1);
                }
                if ui.interact(next, Id::new(("note_next", id)), Sense::click()).clicked() {
                    pad.sheet = (pad.sheet + 1).min(s.count() - 1);
                }
            }

            let note = &mut self.doc.notes[k];
            resp.context_menu(|ui| {
                ui.horizontal(|ui| {
                    for c in 0..NOTE_COLORS.len() {
                        let (sw, r) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                        ui.painter().rect_filled(sw, 5.0, note_color(c));
                        if c == note.color {
                            ui.painter().rect_stroke(sw.expand(2.0), 6.0, Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Outside);
                        }
                        if r.clicked() {
                            note.color = c;
                        }
                    }
                });
                if ui.button("Delete note").clicked() {
                    delete = Some(id);
                    ui.close();
                }
            });

            // While the pad flips, draw the moving sheets over the field.
            if pad.pos != pad.sheet as f32 {
                ui.painter().extend(self.note_shapes(&ctx, &self.doc.notes[k], r, sc, pad.pos, &NoteLook::FLAT));
            }

            // A small × in the top right corner, while the pointer is over the post-it or it is being written on.
            let close = Rect::from_center_size(pos2(r.right() - 7.0 * sc, r.top() + 7.0 * sc), vec2(14.0, 14.0) * sc.max(0.8));
            let close_resp = ui.interact(close, Id::new(("note_delete", id)), Sense::click()).on_hover_text("Delete note");
            if ui.rect_contains_pointer(r) || resp.has_focus() {
                let p = ui.painter();
                if close_resp.hovered() {
                    p.circle_filled(close.center(), close.width() / 2.0, Color32::from_black_alpha(40));
                }
                let (c, d) = (close.center(), close.width() * 0.2);
                let stroke = Stroke::new((1.3 * sc).max(1.0), INK.gamma_multiply(if close_resp.hovered() { 0.9 } else { 0.5 }));
                p.line_segment([c - vec2(d, d), c + vec2(d, d)], stroke);
                p.line_segment([c + vec2(-d, d), c + vec2(d, -d)], stroke);
            }
            if close_resp.clicked() {
                delete = Some(id);
            }
            self.pads.insert(id, pad);
        }
        if let Some(id) = delete {
            self.doc.notes.retain(|n| n.id != id);
        }
    }
}
