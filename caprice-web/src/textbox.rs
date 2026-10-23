//! Text boxes: text placed anywhere on a page, in front of the story or behind it, which does not
//! flow around them (egui cannot flow text around a box, R11; Word calls these "In front of text"
//! and "Behind text"). The story's pictures are in its text, so a box lies in front of them or
//! behind them too, and a box behind leaves the text and pictures over it to the story. A box is
//! pinned to a char outside the text, like a note or a scene (R7), and shows on that char's page,
//! so it goes along when the text it was put beside moves to another page. Where it is on the
//! page is its own: moved, it is pinned again to the char under its top-left corner.
//!
//! Boxes are drawn as `Ink` (`box_ink`), so every look of the pages and print show them. On the
//! page being edited they also take the pointer: a click writes in one, a drag on it (or its edge
//! while writing) moves it, and the handles of the one selected resize it. The toolbar then
//! formats the whole box. Like a picture's size, changes to a box are not undoable (R1).

use std::sync::Arc;

use eframe::egui::{self, Color32, Id, Key, Modifiers, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};

use crate::App;
use crate::bands::rgb;
use crate::images::Handle;
use crate::layout::{Ink, paragraph_job};
use crate::model::{Align, BoxLook, Doc, ParaAttrs, Style, TextBox};

/// Room between a box's edge and its text, in page points.
const PAD: Vec2 = vec2(7.0, 5.0);
/// How wide a sidebar's bar is, in page points.
const BAR: f32 = 4.0;
/// How much larger a quote's text is than its style says.
const QUOTE_SCALE: f32 = 1.3;
/// The smallest a box gets, in page points.
const MIN_SIZE: Vec2 = vec2(36.0, 22.0);
/// How far outside a box (in screen points) its edge still takes a drag.
const EDGE: f32 = 5.0;
/// The colour a new box of each look starts with: black lines for the plain one, else the accent.
pub fn first_color(look: BoxLook) -> [u8; 3] {
    if look == BoxLook::Simple { [40, 40, 44] } else { crate::model::ACCENT_INK }
}

/// The flag of a style that Ctrl and `key` turn on and off: bold, italic, underline.
fn flag_of(key: Key) -> Option<fn(&mut Style) -> &mut bool> {
    match key {
        Key::B => Some(|s| &mut s.bold),
        Key::I => Some(|s| &mut s.italic),
        Key::U => Some(|s| &mut s.underline),
        _ => None,
    }
}

/// White, tinted `t` of the way towards `c`. Clear, `c` that strong over whatever lies under it,
/// which on blank paper is the same colour.
fn tint([r, g, b]: [u8; 3], t: f32, clear: bool) -> Color32 {
    if clear {
        return Color32::from_rgba_unmultiplied(r, g, b, (255.0 * t).round() as u8);
    }
    let mix = |c: u8| (255.0 - (255.0 - f32::from(c)) * t).round() as u8;
    Color32::from_rgb(mix(r), mix(g), mix(b))
}

fn quad(r: Rect) -> Vec<Pos2> {
    vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()]
}

/// Where the text goes inside a box of this look, from its top-left, in page points: the left
/// and top inset, and how much narrower than the box it is.
fn inset(look: BoxLook) -> (Vec2, f32) {
    match look {
        BoxLook::Sidebar => (vec2(BAR + PAD.x + 2.0, PAD.y), BAR + 2.0 * PAD.x + 2.0),
        BoxLook::Quote => (vec2(PAD.x, PAD.y + 4.0), 2.0 * PAD.x),
        _ => (PAD, 2.0 * PAD.x),
    }
}

/// The style the box's text is drawn in: its own, made larger and italic for a quote.
pub fn shown_style(b: &TextBox) -> Style {
    let mut st = b.style.clone();
    if b.look == BoxLook::Quote {
        st.size *= QUOTE_SCALE;
        st.italic = true;
    }
    st
}

/// The box's text laid out at scale `sc`, from the top-left of the room for it.
pub fn box_galley(ctx: &egui::Context, b: &TextBox, sc: f32) -> Arc<egui::Galley> {
    let attrs = ParaAttrs { align: b.style.para.align, ..ParaAttrs::default() };
    let width = (b.size.x - inset(b.look).1).max(8.0) * sc;
    let mut job = paragraph_job(ctx, &b.text, &[], &shown_style(b), attrs, sc, width, &[]);
    // egui centres rows around the left edge of the galley; a field shows the galley from its
    // left. So rows are laid out from the left and then moved within the width.
    job.halign = egui::Align::LEFT;
    let g = ctx.fonts_mut(|f| f.layout_job(job));
    if !matches!(attrs.align, Align::Center | Align::Right) {
        return g;
    }
    let mut placed = (*g).clone();
    for row in &mut placed.rows {
        let free = (width - row.size.x).max(0.0);
        row.pos.x = if attrs.align == Align::Center { free / 2.0 } else { free };
    }
    Arc::new(placed)
}

/// How tall the box is, in page points: as tall as it was made, or as its text needs.
pub fn box_height(ctx: &egui::Context, b: &TextBox) -> f32 {
    let text = box_galley(ctx, b, 1.0).size().y;
    b.size.y.max(text + 2.0 * inset(b.look).0.y)
}

/// The box on its page, in page points from the page's top-left.
pub fn box_rect(ctx: &egui::Context, b: &TextBox) -> Rect {
    Rect::from_min_size(b.pos.to_pos2(), vec2(b.size.x, box_height(ctx, b)))
}

/// What the box draws at scale `sc`, relative to its page's top-left: its look, and its text
/// unless that is being written in a field drawn over it.
pub fn box_ink(ctx: &egui::Context, b: &TextBox, sc: f32, with_text: bool) -> Vec<Ink> {
    let r = Rect::from_min_size(b.pos.to_pos2() * sc, vec2(b.size.x, box_height(ctx, b)) * sc);
    let line = |y: f32| Ink::Line(vec![pos2(r.left(), y), pos2(r.right(), y)], 1.0 * sc, rgb(b.color));
    let mut ink = match b.look {
        BoxLook::Simple => {
            let mut edge = quad(r);
            edge.push(r.left_top());
            let line = Ink::Line(edge, 0.75 * sc, rgb(b.color));
            if b.clear { vec![line] } else { vec![Ink::Fill(quad(r), Color32::WHITE), line] }
        }
        BoxLook::Shaded => vec![Ink::Fill(quad(r), tint(b.color, 0.16, b.clear))],
        BoxLook::Sidebar => {
            let bar = Rect::from_min_size(r.min, vec2(BAR * sc, r.height()));
            vec![Ink::Fill(quad(r), tint(b.color, 0.07, b.clear)), Ink::Fill(quad(bar), rgb(b.color))]
        }
        BoxLook::Quote => vec![line(r.top()), line(r.bottom())],
        BoxLook::Bare => Vec::new(),
    };
    if with_text {
        ink.push(Ink::Text(box_galley(ctx, b, sc), r.min + inset(b.look).0 * sc));
    }
    ink
}

impl Doc {
    /// The text boxes shown on page `i`, bottom first.
    pub fn boxes_on(&self, i: usize) -> impl Iterator<Item = &TextBox> {
        self.boxes.iter().filter(move |b| self.spans[i].contents.is_none() && self.page_of(b.at) == i)
    }
}

/// A move or resize of a box by the pointer.
struct BoxDrag {
    id: u64,
    /// The box when it began, in page points; the handle dragged (none: the whole box moves).
    start: Rect,
    handle: Option<Handle>,
    dragged: Vec2,
}

/// The text boxes' state in the app: which is selected and being written in.
#[derive(Default)]
pub struct Boxes {
    /// Its handles show and the toolbar formats it.
    pub selected: Option<u64>,
    /// Its text is being written.
    pub editing: bool,
    /// Give the keyboard to the selected box's text on the next frame it is drawn.
    pub focus: bool,
    drag: Option<BoxDrag>,
}

impl App {
    /// Put a new text box in this look on the page shown, beside the caret if it is on it. It is
    /// selected, for its text to be typed.
    pub fn insert_text_box(&mut self, ctx: &egui::Context, look: BoxLook) {
        let caret_page = self.doc.page_of(self.caret);
        let shown = self.target.min(self.last());
        let i = if self.doc.spans[shown].contents.is_none() { shown } else { caret_page };
        let s = &self.doc.setup;
        let (origin, content) = (s.margin_origin(), s.content_size());
        // Level with the caret's line, if it is on this page; else at the top of the writing area.
        let (at, y) = if caret_page == i {
            let layout = self.doc.layout_page(ctx, i, 1.0, &[]);
            let row = layout.row_of(self.caret - self.doc.spans[i].start, self.prefer_next);
            (self.caret, row.map_or(0.0, |r| r.top))
        } else {
            (self.doc.spans[i].start, 0.0)
        };
        let w = (content.x * 0.55).clamp(MIN_SIZE.x, 260.0);
        let h = 60.0;
        let y = (origin.y + y).min(s.size().y - s.margin_bottom - h).max(0.0);
        let mut style = self.typing.with_para(ParaAttrs::default());
        style.para.align = if look == BoxLook::Quote { Align::Center } else { Align::Left };
        let id = self.doc.next_box_id;
        self.doc.next_box_id += 1;
        let pos = vec2(origin.x + (content.x - w) / 2.0, y);
        self.doc.boxes.push(TextBox { id, at, pos, size: vec2(w, h), text: String::new(), style, look, color: first_color(look), behind: false, clear: false });
        self.doc.version += 1;
        self.boxes = Boxes { selected: Some(id), editing: true, focus: true, drag: None };
        if self.target != i {
            self.turn_to(ctx, i, false);
        }
    }

    pub fn text_box(&self, id: u64) -> Option<&TextBox> {
        self.doc.boxes.iter().find(|b| b.id == id)
    }

    /// The selected box, to change it.
    fn selected_box_mut(&mut self) -> Option<&mut TextBox> {
        let id = self.boxes.selected?;
        self.doc.boxes.iter_mut().find(|b| b.id == id)
    }

    /// Change the selected box, if there is one; true if there was. Its text goes on being written.
    pub fn edit_selected_box(&mut self, edit: impl FnOnce(&mut TextBox)) -> bool {
        let Some(b) = self.selected_box_mut() else { return false };
        edit(b);
        self.doc.version += 1;
        if self.boxes.editing || self.boxes.focus {
            (self.boxes.editing, self.boxes.focus) = (true, true);
        }
        true
    }

    pub fn delete_text_box(&mut self, id: u64) {
        self.doc.boxes.retain(|b| b.id != id);
        self.doc.version += 1;
        if self.boxes.selected == Some(id) {
            self.boxes = Boxes::default();
        }
    }

    /// The boxes of page `i` drawn at `page_rect`, where they take the pointer: the editor under
    /// them must leave presses there alone.
    pub fn box_hits(&self, ctx: &egui::Context, page_rect: Rect, i: usize) -> Vec<Rect> {
        let sc = self.scale_of(page_rect);
        let grow = |r: Rect| r.expand(EDGE + if self.boxes.selected.is_some() { 4.0 } else { 0.0 });
        let on_text = self.pointer_on_text(ctx, page_rect, i);
        let boxes = self.doc.boxes_on(i).filter(|b| self.takes_pointer(b, on_text));
        boxes.map(|b| grow(box_rect(ctx, b) * sc).translate(page_rect.min.to_vec2())).collect()
    }

    /// Whether the pointer is on the story's text or a picture of page `i`, drawn at `page_rect`.
    /// Only asked for if a box lies behind the story there.
    fn pointer_on_text(&self, ctx: &egui::Context, page_rect: Rect, i: usize) -> bool {
        let Some(p) = ctx.input(|inp| inp.pointer.latest_pos()) else { return false };
        if !self.doc.boxes_on(i).any(|b| b.behind) {
            return false;
        }
        let sc = self.scale_of(page_rect);
        let layout = self.doc.layout_page(ctx, i, sc, &[]);
        layout.covers(p - page_rect.min - self.doc.setup.margin_origin() * sc)
    }

    /// Whether box `b` takes the pointer, `on_text` saying if it is on the story's text. A box
    /// behind the story leaves its text and pictures to it, unless it is selected, to be written
    /// in, moved or resized.
    fn takes_pointer(&self, b: &TextBox, on_text: bool) -> bool {
        !b.behind || !on_text || self.boxes.selected == Some(b.id)
    }

    /// Draw the text boxes of the page being edited that lie behind the story, at `page_rect`,
    /// before its text. `text_boxes` lets them be written in, moved and resized.
    pub fn text_boxes_behind(&self, ui: &egui::Ui, page_rect: Rect, i: usize) {
        let sc = self.scale_of(page_rect);
        for b in self.doc.boxes_on(i).filter(|b| b.behind) {
            let writing = self.boxes.selected == Some(b.id) && self.boxes.editing;
            for ink in box_ink(ui.ctx(), b, sc, !writing) {
                ui.painter().extend(ink.translate(page_rect.min.to_vec2()).shape());
            }
        }
    }

    /// Bring box `id` to the front of the story's text and pictures, over every other box, or
    /// send it to the back, behind them and every other box.
    pub fn stack_text_box(&mut self, id: u64, front: bool) {
        let Some(k) = self.doc.boxes.iter().position(|b| b.id == id) else { return };
        let mut b = self.doc.boxes.remove(k);
        b.behind = !front;
        if front { self.doc.boxes.push(b) } else { self.doc.boxes.insert(0, b) }
        self.doc.version += 1;
    }

    /// Keys for a box selected but not being written in: Delete or Backspace takes it away,
    /// Escape lets it go, and Ctrl+B, I and U format it. True if the key was the box's.
    pub fn box_key(&mut self, key: Key, m: Modifiers) -> bool {
        let Some(id) = self.boxes.selected.filter(|_| !self.boxes.editing) else { return false };
        if m.command {
            let Some(flag) = flag_of(key) else { return false };
            return self.edit_selected_box(|b| {
                let f = flag(&mut b.style);
                *f = !*f;
            });
        }
        match key {
            Key::Delete | Key::Backspace => self.delete_text_box(id),
            Key::Escape => self.boxes = Boxes::default(),
            _ => {
                // Anything else goes to the story again.
                self.boxes = Boxes::default();
                return false;
            }
        }
        true
    }

    /// Draw the text boxes of the page being edited, at `page_rect`, and let them be written in,
    /// moved and resized.
    pub fn text_boxes(&mut self, ui: &mut egui::Ui, page_rect: Rect, i: usize) {
        let ctx = ui.ctx().clone();
        let sc = self.scale_of(page_rect);
        let origin = page_rect.min.to_vec2();
        let on_text = self.pointer_on_text(&ctx, page_rect, i);
        // Those behind the story come first, as they are drawn: a box in front takes the pointer over them.
        let mut ids: Vec<&TextBox> = self.doc.boxes_on(i).collect();
        ids.sort_by_key(|b| !b.behind);
        let ids: Vec<u64> = ids.into_iter().map(|b| b.id).collect();
        for id in ids {
            let Some(b) = self.text_box(id).cloned() else { continue };
            let rect = (box_rect(&ctx, &b) * sc).translate(origin);
            let selected = self.boxes.selected == Some(id);
            let writing = selected && self.boxes.editing;
            // Those behind were drawn before the story (`text_boxes_behind`).
            if !b.behind {
                for ink in box_ink(&ctx, &b, sc, !writing) {
                    ui.painter().extend(ink.translate(origin).shape());
                }
            }
            let dragged = self.boxes.drag.as_ref().is_some_and(|d| d.id == id);
            if !self.takes_pointer(&b, on_text) && !dragged {
                continue;
            }

            // The box (while writing, only its edge) takes drags to move it, and clicks to write.
            let resp = ui.interact(rect.expand(EDGE), Id::new(("text_box", id)), Sense::click_and_drag());
            let label = if b.text.is_empty() { "Text box" } else { b.text.as_str() };
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
            // While it is written in, its text takes the pointer there, right clicks too.
            let field = writing.then(|| self.box_field(ui, &b, rect, sc));
            if resp.hovered() && self.boxes.drag.is_none() {
                ui.output_mut(|o| o.cursor_icon = if writing { egui::CursorIcon::Move } else { egui::CursorIcon::Text });
            }
            if resp.clicked() && !writing {
                self.boxes = Boxes { selected: Some(id), editing: true, focus: true, drag: None };
                // With formatting taken up, the click puts it down on the box: its text is one style.
                if let Some(brush) = self.brush.take() {
                    self.edit_selected_box(|b| {
                        let align = brush.para.align;
                        b.style.take_look(&brush);
                        b.style.para.align = align;
                    });
                }
            }
            if resp.drag_started() {
                self.boxes.selected = Some(id);
                self.boxes.drag = Some(BoxDrag { id, start: Rect::from_min_size(b.pos.to_pos2(), b.size), handle: None, dragged: Vec2::ZERO });
            }
            self.box_menu(&resp, id);
            if let Some(field) = field {
                self.box_menu(&field, id);
            }
            self.follow_box_drag(&ctx, &resp, i, sc);
            if selected {
                self.box_handles(ui, &ctx, &b, rect, i, sc);
                let edge = Stroke::new(1.0, crate::theme::pal().accent.gamma_multiply(0.8));
                ui.painter().rect_stroke(rect, 0.0, edge, egui::StrokeKind::Outside);
            }
        }
        // A box selected that is not on this page any more (its text moved) is let go.
        if self.boxes.selected.is_some_and(|id| !self.doc.boxes_on(i).any(|b| b.id == id)) {
            self.boxes = Boxes::default();
        }
    }

    /// The field the selected box's text is written in, over the box, in the box's own layout.
    fn box_field(&mut self, ui: &mut egui::Ui, b: &TextBox, rect: Rect, sc: f32) -> egui::Response {
        let (at, narrower) = inset(b.look);
        let area = Rect::from_min_size(rect.min + at * sc, vec2(b.size.x - narrower, rect.height() / sc - 2.0 * at.y) * sc);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
        let shape = b.clone();
        let mut layouter = move |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
            box_galley(ui.ctx(), &TextBox { text: buf.as_str().to_owned(), ..shape.clone() }, sc)
        };
        let mut edited = b.text.clone();
        let out = egui::TextEdit::multiline(&mut edited)
            .id(Id::new(("text_box_text", b.id)))
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .font(egui::FontId::new(shown_style(b).size * sc, crate::fonts::family_for(&b.style.font, b.style.bold, false).0))
            .desired_width(area.width())
            .min_size(area.size())
            .hint_text("Text box")
            .layouter(&mut layouter)
            .show(&mut child);
        let resp = out.response.response;
        if edited != b.text {
            if let Some(b) = self.selected_box_mut() {
                b.text = edited;
            }
            self.doc.version += 1;
        }
        if std::mem::take(&mut self.boxes.focus) {
            resp.request_focus();
        }
        // Bold, italic and underline keys reach the field, not the story: they format the box.
        if resp.has_focus() {
            for key in [Key::B, Key::I, Key::U] {
                let Some(flag) = flag_of(key) else { continue };
                if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, key)) {
                    self.edit_selected_box(|b| {
                        let f = flag(&mut b.style);
                        *f = !*f;
                    });
                }
            }
        }
        if resp.lost_focus() && !self.boxes.focus {
            // Escape, or a click elsewhere: the box stays selected, to be moved, formatted or deleted.
            self.boxes.editing = false;
        }
        resp
    }

    /// The right-click menu of a box: its look, its colour, deleting it.
    fn box_menu(&mut self, resp: &egui::Response, id: u64) {
        let Some(b) = self.text_box(id) else { return };
        let (look, color, was_clear) = (b.look, b.color, b.clear);
        let mut clear = was_clear;
        let (mut new_look, mut new_color) = (None, None);
        let (mut delete, mut stack) = (false, None);
        resp.context_menu(|ui| {
            for l in BoxLook::ALL {
                // Selectable, so the one in use gets the highlight its dark text is meant for.
                if ui.add(egui::Button::selectable(l == look, l.name())).clicked() {
                    new_look = Some(l);
                    ui.close();
                }
            }
            ui.separator();
            new_color = crate::bands::accent_row(ui, color);
            // A look without a fill is see-through already.
            if matches!(look, BoxLook::Simple | BoxLook::Shaded | BoxLook::Sidebar) && ui.checkbox(&mut clear, "Transparent").changed() {
                ui.close();
            }
            ui.separator();
            if ui.add(egui::Button::new("Bring to front").frame(false)).on_hover_text("In front of the text and pictures").clicked() {
                stack = Some(true);
                ui.close();
            }
            if ui.add(egui::Button::new("Send to back").frame(false)).on_hover_text("Behind the text and pictures").clicked() {
                stack = Some(false);
                ui.close();
            }
            ui.separator();
            if ui.add(egui::Button::new("Delete text box").frame(false)).clicked() {
                delete = true;
                ui.close();
            }
        });
        if delete {
            self.delete_text_box(id);
            return;
        }
        if let Some(front) = stack {
            self.stack_text_box(id, front);
        }
        if new_look.is_some() || new_color.is_some() || clear != was_clear {
            self.boxes.selected = Some(id);
            self.edit_selected_box(|b| {
                (b.look, b.color) = (new_look.unwrap_or(b.look), new_color.unwrap_or(b.color));
                b.clear = clear;
            });
        }
    }

    /// The eight handles of the selected box, which resize it.
    fn box_handles(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, b: &TextBox, rect: Rect, i: usize, sc: f32) {
        for h in Handle::ALL {
            let c = h.pos(rect);
            let sq = Rect::from_center_size(c, vec2(9.0, 9.0));
            ui.painter().rect_filled(sq, 2.0, Color32::WHITE);
            ui.painter().rect_stroke(sq, 2.0, Stroke::new(1.5, crate::theme::pal().accent), egui::StrokeKind::Outside);
            let r = ui.interact(Rect::from_center_size(c, vec2(13.0, 13.0)), Id::new(("box_handle", b.id, h.kx, h.ky)), Sense::drag());
            if r.hovered() || r.dragged() {
                ui.output_mut(|o| o.cursor_icon = h.cursor());
            }
            if r.drag_started() {
                self.boxes.drag = Some(BoxDrag { id: b.id, start: Rect::from_min_size(b.pos.to_pos2(), b.size), handle: Some(h), dragged: Vec2::ZERO });
            }
            self.follow_box_drag(ctx, &r, i, sc);
        }
    }

    /// Move or resize the box being dragged by `resp`; once let go, pin it to the char under its corner.
    fn follow_box_drag(&mut self, ctx: &egui::Context, resp: &egui::Response, i: usize, sc: f32) {
        let Some(d) = &mut self.boxes.drag else { return };
        if resp.dragged() {
            d.dragged += resp.drag_delta() / sc;
            let (id, start, handle, by) = (d.id, d.start, d.handle, d.dragged);
            let page = self.doc.setup.size();
            let Some(b) = self.doc.boxes.iter_mut().find(|b| b.id == id) else { return };
            match handle {
                None => {
                    let max = (page - start.size()).max(Vec2::ZERO);
                    b.pos = (start.min.to_vec2() + by).clamp(Vec2::ZERO, max);
                }
                Some(h) => {
                    // Each side the handle is on follows the pointer; the others stay.
                    let (mut min, mut max) = (start.min, start.max);
                    match h.kx {
                        -1 => min.x = (min.x + by.x).min(max.x - MIN_SIZE.x).max(0.0),
                        1 => max.x = (max.x + by.x).max(min.x + MIN_SIZE.x).min(page.x),
                        _ => {}
                    }
                    match h.ky {
                        -1 => min.y = (min.y + by.y).min(max.y - MIN_SIZE.y).max(0.0),
                        1 => max.y = (max.y + by.y).max(min.y + MIN_SIZE.y).min(page.y),
                        _ => {}
                    }
                    b.pos = min.to_vec2();
                    b.size = max - min;
                }
            }
            self.doc.version += 1;
        }
        if resp.drag_stopped() {
            let id = d.id;
            self.boxes.drag = None;
            self.pin_box(ctx, id, i);
        }
    }

    /// Pin a box to the char of page `i` nearest its top-left corner, so it stays on this page
    /// with the text it now lies over.
    fn pin_box(&mut self, ctx: &egui::Context, id: u64, i: usize) {
        let Some(b) = self.text_box(id) else { return };
        let layout = self.doc.layout_page(ctx, i, 1.0, &[]);
        let local = layout.hit(b.pos - self.doc.setup.margin_origin());
        let c = self.doc.spans[i].start + local;
        if self.doc.page_of(c) == i
            && let Some(b) = self.doc.boxes.iter_mut().find(|b| b.id == id)
        {
            b.at = c;
        }
    }
}
