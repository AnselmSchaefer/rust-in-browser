//! The two docks: formatting on top, pages at the bottom.

use std::sync::Arc;

use eframe::egui::{self, Color32, FontId, Id, Rect, Sense, Stroke, UiBuilder, pos2, vec2};

use crate::{App, Appearance};
use crate::model::{Align, Band, BandLook, BoxLook, CM, ListKind, Orientation, PAGE_FIELD, Style, TextBox};
use crate::numbers::{Counting, Figures, Numbering};
use crate::theme::{Theme, pal};
#[cfg(target_arch = "wasm32")]
use crate::web::rfd;

pub const DOCK_H: f32 = 48.0;
pub const DOCK_GAP: f32 = 14.0;
/// Space at the top/bottom of the window taken by the two docks.
pub const RESERVED: f32 = DOCK_GAP + DOCK_H + 10.0;
/// The smallest the window gets: the docks then still hold File, "⋯" and a little of the name on
/// top, and Scratchpad, Fit, the zoom and "+" below.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub const MIN_WINDOW: [f32; 2] = [340.0, 400.0];
/// Widest the docks get.
pub const DOCK_MAX_W: f32 = 1180.0;
/// The toolbar's groups of tools, in the order they show; the last ones go into "⋯" first.
/// File, font, letters, paragraph, page and insert, note and find, Claude and scenes.
pub const GROUPS: usize = 7;
/// Room between two tools, or two groups.
const TOOL_GAP: f32 = 8.0;
/// The width of the font list.
const FONT_W: f32 = 128.0;
/// The width of the line before the paragraph and page groups.
const LINE_W: f32 = 5.0;
/// The "⋯" button's width.
const MORE_W: f32 = 30.0;
/// The height of a row in the second dock.
const MORE_LINE: f32 = 40.0;
/// Room between the tools and the document's name.
const NAME_GAP: f32 = 16.0;
/// How fast a name too long for its room runs past, in points a second.
const NAME_SPEED: f32 = 18.0;
/// How long a name too long for its room stands still between passes, in seconds.
const NAME_PAUSE: f32 = 3.0;
/// A paragraph-format change picked in the toolbar.
#[derive(Clone, Copy)]
enum ParaChange {
    Align(Align),
    Spacing(f32),
    List(ListKind),
    ChapterTitle(bool),
}

#[derive(Clone, Copy)]
enum Icon {
    Align(Align),
    Bullets,
    Numbers,
    Brush,
}

/// Draw an icon into a 30x30 square.
fn paint_icon(p: &egui::Painter, rect: Rect, icon: Icon, ink: Color32) {
    let stroke = Stroke::new(1.6, ink);
    let area = rect.shrink2(vec2(8.0, 8.0));
    match icon {
        Icon::Align(a) => {
            for (k, w) in [1.0, 0.62, 0.86, 0.5].into_iter().enumerate() {
                let w = if a == Align::Justify { 1.0 } else { w } * area.width();
                let x0 = match a {
                    Align::Left | Align::Justify => area.left(),
                    Align::Center => area.center().x - w / 2.0,
                    Align::Right => area.right() - w,
                };
                let y = area.top() + 1.0 + k as f32 * (area.height() - 2.0) / 3.0;
                p.line_segment([pos2(x0, y), pos2(x0 + w, y)], stroke);
            }
        }
        Icon::Bullets | Icon::Numbers => {
            for k in 0..3 {
                let y = area.top() + 2.0 + k as f32 * (area.height() - 4.0) / 2.0;
                if matches!(icon, Icon::Bullets) {
                    p.circle_filled(pos2(area.left() + 1.5, y), 1.7, ink);
                } else {
                    p.text(pos2(area.left() + 1.0, y), egui::Align2::LEFT_CENTER, (k + 1).to_string(), FontId::proportional(8.0), ink);
                }
                p.line_segment([pos2(area.left() + 6.5, y), pos2(area.right(), y)], stroke);
            }
        }
        Icon::Brush => {
            // A broad brush, bristles down: the head, its band, and the handle above.
            let (l, r, t, b) = (area.left() + 1.0, area.right() - 1.0, area.top(), area.bottom());
            let head = vec![pos2(l, b - 5.0), pos2(r, b - 5.0), pos2(r, b - 1.0), pos2(r - 2.0, b), pos2(l + 2.0, b), pos2(l, b - 1.0)];
            p.add(egui::Shape::convex_polygon(head, ink, Stroke::NONE));
            p.rect_stroke(Rect::from_min_max(pos2(l + 1.0, b - 8.5), pos2(r - 1.0, b - 6.0)), 1.0, stroke, egui::StrokeKind::Inside);
            let x = area.center().x;
            p.line_segment([pos2(x, t), pos2(x, b - 8.5)], Stroke::new(3.0, ink));
        }
    }
}

/// A square toolbar button with a drawn icon.
fn icon_button(ui: &mut egui::Ui, icon: Icon, selected: bool, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    let fill = if selected { pal().accent } else if resp.hovered() { pal().btn_hover } else { pal().btn };
    let ink = if selected { pal().on_accent } else { pal().text };
    ui.painter().rect_filled(rect, 8.0, fill);
    paint_icon(ui.painter(), rect, icon, ink);
    resp.on_hover_text(tip)
}

/// A toolbar button showing `icon` and a small arrow; clicking it opens a popup with more choices.
fn icon_dropdown(ui: &mut egui::Ui, icon: Icon, active: bool, tip: &str, popup: impl FnOnce(&mut egui::Ui)) {
    let (rect, resp) = ui.allocate_exact_size(vec2(46.0, 30.0), Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let fill = if active { pal().accent } else if resp.hovered() || open { pal().btn_hover } else { pal().btn };
    let ink = if active { pal().on_accent } else { pal().text };
    ui.painter().rect_filled(rect, 8.0, fill);
    paint_icon(ui.painter(), Rect::from_min_size(rect.min, vec2(30.0, 30.0)), icon, ink);
    let c = pos2(rect.right() - 9.0, rect.center().y);
    let s = Stroke::new(1.5, ink);
    ui.painter().line_segment([pos2(c.x - 3.5, c.y - 1.5), pos2(c.x, c.y + 2.0)], s);
    ui.painter().line_segment([pos2(c.x, c.y + 2.0), pos2(c.x + 3.5, c.y - 1.5)], s);
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(popup);
    if !open {
        resp.on_hover_text(tip);
    }
}

/// Text colours offered in the toolbar, named for their tooltips. Dark enough to read on paper.
/// Black is the usual ink (no colour of its own), to go back to it.
const TEXT_COLORS: [(&str, Option<[u8; 3]>); 11] = [
    ("Black", None),
    ("Dark red", Some([150, 28, 28])),
    ("Red", Some([205, 40, 40])),
    ("Orange", Some([215, 110, 15])),
    ("Gold", Some([180, 135, 0])),
    ("Green", Some([40, 125, 60])),
    ("Teal", Some([0, 120, 125])),
    ("Blue", Some([35, 90, 200])),
    ("Navy", Some([25, 40, 110])),
    ("Purple", Some([115, 50, 160])),
    ("Grey", Some([105, 105, 110])),
];

fn ink_of(color: Option<[u8; 3]>) -> Color32 {
    color.map_or(crate::theme::INK, |[r, g, b]| Color32::from_rgb(r, g, b))
}

/// The text colour button: an "A" over a bar in the colour of the text at the caret (light for
/// the usual ink, which would not show on the dock). Its popup sets a colour: `Some(None)` is the
/// usual ink again. Any other colour is picked in the popup itself: a picker opening its own popup
/// closed this one on the first click, as a click outside it.
fn color_dropdown(ui: &mut egui::Ui, color: Option<[u8; 3]>) -> Option<Option<[u8; 3]>> {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Text colour"));
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    ui.painter().rect_filled(rect, 8.0, if resp.hovered() || open { pal().btn_hover } else { pal().btn });
    let font = egui::FontId::proportional(15.0);
    ui.painter().text(rect.center() - vec2(0.0, 3.0), egui::Align2::CENTER_CENTER, "A", font, pal().text);
    let bar = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 7.0), vec2(16.0, 3.5));
    ui.painter().rect_filled(bar, 1.0, color.map_or(pal().text, |_| ink_of(color)));
    let mut pick = None;
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.set_max_width(6.0 * 26.0);
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for (name, c) in TEXT_COLORS {
                let (r, swatch) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                swatch.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
                ui.painter().rect_filled(r, 4.0, ink_of(c));
                if color == c || swatch.hovered() {
                    ui.painter().rect_stroke(r.expand(1.5), 5.0, Stroke::new(1.5, pal().text), egui::StrokeKind::Outside);
                }
                if swatch.on_hover_text(name).clicked() {
                    pick = Some(c);
                    ui.close();
                }
            }
        });
        egui::CollapsingHeader::new("Other colour").show(ui, |ui| {
            let mut other = ink_of(color);
            if egui::color_picker::color_picker_color32(ui, &mut other, egui::color_picker::Alpha::Opaque) {
                pick = Some(Some([other.r(), other.g(), other.b()]));
            }
        });
    });
    if !open {
        resp.on_hover_text("Text colour");
    }
    pick
}

/// The themes, each drawn as a little desk with its dock, a page and the accent; the one clicked
/// becomes `theme`.
fn theme_gallery(ui: &mut egui::Ui, theme: &mut Theme) {
    egui::Grid::new("theme_gallery").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
        for (k, t) in Theme::ALL.into_iter().enumerate() {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(t.name()).size(12.0).color(pal().text_dim));
                let (rect, resp) = ui.allocate_exact_size(vec2(150.0, 84.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t.name()));
                let c = t.palette();
                let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                p.rect_filled(rect, 6.0, c.desk);
                let glow = Rect::from_min_max(rect.min, pos2(rect.right(), rect.center().y));
                p.rect_filled(glow, egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, c.desk_top.gamma_multiply(0.7));
                let dock = Rect::from_min_size(rect.min + vec2(14.0, 7.0), vec2(rect.width() - 28.0, 14.0));
                p.rect_filled(dock, 5.0, c.dock);
                p.rect_stroke(dock, 5.0, Stroke::new(1.0, c.dock_edge), egui::StrokeKind::Inside);
                for (j, fill) in [c.btn, c.btn, c.accent, c.btn].into_iter().enumerate() {
                    let b = Rect::from_min_size(dock.min + vec2(4.0 + j as f32 * 13.0, 3.0), vec2(10.0, 8.0));
                    p.rect_filled(b, 2.0, fill);
                }
                let line = Rect::from_min_size(pos2(dock.right() - 34.0, dock.center().y - 1.0), vec2(28.0, 2.0));
                p.rect_filled(line, 1.0, c.text_dim);
                let page = Rect::from_min_size(pos2(rect.center().x - 22.0, dock.bottom() + 7.0), vec2(44.0, 52.0));
                p.rect_filled(page, 1.0, crate::theme::PAPER);
                for j in 0..6 {
                    let w = if j == 5 { 18.0 } else { 34.0 };
                    let y = page.top() + 7.0 + j as f32 * 6.0;
                    p.rect_filled(Rect::from_min_size(pos2(page.left() + 5.0, y), vec2(w, 1.5)), 0.0, Color32::from_gray(190));
                }
                p.circle_filled(pos2(page.right() + 12.0, page.bottom() - 6.0), 4.0, c.accent);
                p.circle_filled(pos2(page.left() - 12.0, page.top() + 10.0), 2.5, c.caret);
                if *theme == t {
                    ui.painter().rect_stroke(rect.expand(1.5), 7.0, Stroke::new(2.0, pal().accent), egui::StrokeKind::Outside);
                } else if resp.hovered() {
                    ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.0, pal().text_dim), egui::StrokeKind::Outside);
                }
                if resp.on_hover_text(t.about()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    *theme = t;
                }
            });
            if k % 2 == 1 {
                ui.end_row();
            }
        }
    });
}

/// Something picked in the Insert menu.
enum InsertAction {
    Picture,
    TextBox(BoxLook),
    /// A new header (true) or footer.
    Band(bool, Band),
    PageBreak,
}

/// The looks a text box can have, each drawn as a small box with a few words in it, as the
/// headers' gallery does; the one clicked.
fn box_gallery(ui: &mut egui::Ui, typing: &Style) -> Option<BoxLook> {
    let mut pick = None;
    egui::Grid::new("box_gallery").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
        for (k, look) in BoxLook::ALL.into_iter().enumerate() {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(look.name()).size(12.0).color(pal().text_dim));
                let (rect, resp) = ui.allocate_exact_size(vec2(170.0, 66.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, look.name()));
                let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                p.rect_filled(rect, 2.0, crate::theme::PAPER);
                let mut style = Style { size: 10.0, bold: false, italic: false, underline: false, color: None, ..typing.with_para(Default::default()) };
                style.para.align = if look == BoxLook::Quote { Align::Center } else { Align::Left };
                let text = "A few words set apart from the story.".into();
                let color = crate::textbox::first_color(look);
                let sample = TextBox { id: 0, at: 0, pos: vec2(12.0, 9.0), size: vec2(146.0, 46.0), text, style, look, color, behind: false, clear: false };
                for ink in crate::textbox::box_ink(ui.ctx(), &sample, 1.0, true) {
                    p.extend(ink.translate(rect.min.to_vec2()).shape());
                }
                if resp.hovered() {
                    ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.0, pal().text_dim), egui::StrokeKind::Outside);
                }
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    pick = Some(look);
                }
            });
            if k % 2 == 1 {
                ui.end_row();
            }
        }
    });
    pick
}

#[derive(Clone, Copy)]
enum FileAction {
    Open,
    Save,
    SaveAs,
    ExportDocx,
    Print,
    Versions,
    Revert,
    Info,
}

/// What was clicked in the tools this frame, done once they are all drawn.
#[derive(Default)]
struct Tools {
    file: Option<FileAction>,
    font: Option<String>,
    size: Option<f32>,
    bold: bool,
    italic: bool,
    underline: bool,
    color: Option<Option<[u8; 3]>>,
    brush: bool,
    para: Option<ParaChange>,
    setup: Option<crate::model::PageSetup>,
    /// Numbering set from a page on, or taken off it (`None`).
    numbering: Option<(usize, Option<Numbering>)>,
    /// A page to turn to.
    turn_to: Option<usize>,
    insert: Option<InsertAction>,
    add_note: bool,
    find: bool,
}

/// Whether group `g` has a line before it (see `tool_group`).
fn has_line(g: usize) -> bool {
    g == 3 || g == 4
}

/// The "⋯" button that opens the second row of tools.
fn more_button(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(MORE_W, 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "More tools"));
    let fill = if open { pal().accent } else if resp.hovered() { pal().btn_hover } else { pal().btn };
    let ink = if open { pal().on_accent } else { pal().text };
    ui.painter().rect_filled(rect, 8.0, fill);
    for k in [-1.0, 0.0, 1.0] {
        ui.painter().circle_filled(rect.center() + vec2(k * 5.5, 0.0), 1.7, ink);
    }
    resp.on_hover_text("More tools")
}

impl App {
    pub fn dock_rect(area: Rect, top: bool) -> Rect {
        let width = (area.width() - 48.0).clamp(0.0, DOCK_MAX_W);
        let y = if top { area.top() + DOCK_GAP + DOCK_H / 2.0 } else { area.bottom() - DOCK_GAP - DOCK_H / 2.0 };
        Rect::from_center_size(pos2(area.center().x, y), vec2(width, DOCK_H))
    }

    fn paint_dock(ui: &egui::Ui, rect: Rect) {
        // Too narrow to hold anything, it would be only a blob.
        if rect.width() < 40.0 {
            return;
        }
        let p = ui.painter();
        for k in 1..=5 {
            let g = rect.expand(k as f32 * 2.0).translate(vec2(0.0, 4.0));
            p.rect_filled(g, 16.0 + k as f32 * 2.0, Color32::from_black_alpha(20));
        }
        p.rect_filled(rect, 16.0, pal().dock);
        p.rect_stroke(rect, 16.0, Stroke::new(1.0, pal().dock_edge), egui::StrokeKind::Inside);
    }

    /// The formatting dock: one row of tools in groups, the document's name at its right end. Groups
    /// that do not fit, the last first, go into a second row that "⋯" opens under it.
    pub fn format_bar(&mut self, ui: &mut egui::Ui, area: Rect) {
        let ctx = ui.ctx().clone();
        let rect = Self::dock_rect(area, true);
        Self::paint_dock(ui, rect);
        let inner = rect.shrink2(vec2(14.0, 0.0));
        let name = self.doc_name();
        let galley = ui.painter().layout_no_wrap(name.clone(), FontId::proportional(13.0), pal().text_dim);
        // The name is never hidden: when the tools need the room, it keeps about four characters.
        let name_w = galley.size().x;
        let four = (name_w / name.chars().count().max(1) as f32 * 4.0).min(name_w);

        // Which groups fit, by their widths as last drawn.
        let w = self.tool_w;
        // A group's width with the room before it, which holds a line unless the group starts a row.
        let cost = |g: usize, first: bool| w[g] + if first { 0.0 } else { TOOL_GAP + if has_line(g) { LINE_W + TOOL_GAP } else { 0.0 } };
        let room = inner.width() - NAME_GAP - four;
        let all = (0..GROUPS).map(|g| cost(g, g == 0)).sum::<f32>();
        let shown = if all <= room + 0.5 {
            GROUPS
        } else {
            let mut used = MORE_W;
            (0..GROUPS).take_while(|&g| { used += cost(g, false); used <= room + 0.5 }).count()
        };
        // With no room even for "⋯", the tools are put away until the window is wider.
        let more_fits = shown == GROUPS || MORE_W <= room + 0.5;
        if shown == GROUPS || !more_fits {
            self.more_tools = false;
        }

        let mut tools = Tools::default();
        let mut end = inner.left();
        let mut more = None;
        ui.scope_builder(
            UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing = vec2(TOOL_GAP, 8.0);
                for g in 0..shown {
                    self.tool_group(ui, g, g == 0, &mut tools);
                }
                if shown < GROUPS && more_fits {
                    more = Some(more_button(ui, self.more_tools));
                }
                end = ui.min_rect().right();
            },
        );
        let more_rect = more.as_ref().map(|r| r.rect);
        if more.is_some_and(|r| r.clicked()) {
            self.more_tools = !self.more_tools;
        }

        // The rest, in a second dock under the first, in as many rows as they need.
        let mut bottom = rect.bottom();
        if self.more_tools {
            // Rows as wide as the toolbar, but a group wider than that has a row of its own reaching
            // nearly across the window; one wider still is left out until the window is wider.
            let width = rect.width() - 28.0;
            let (outer_l, outer_r) = (area.left() + 8.0, area.right() - 8.0);
            let mut lines: Vec<Vec<usize>> = vec![vec![]];
            let mut x = 0.0;
            for g in (shown..GROUPS).filter(|&g| w[g] <= outer_r - outer_l - 28.0) {
                if x > 0.0 && x + cost(g, false) > width {
                    lines.push(vec![]);
                    x = 0.0;
                }
                x += cost(g, x == 0.0);
                lines.last_mut().unwrap().push(g);
            }
            let line_w = |l: &Vec<usize>| l.iter().enumerate().map(|(j, &g)| cost(g, j == 0)).sum::<f32>();
            let widest = lines.iter().map(line_w).fold(0.0, f32::max);
            // Under "⋯", as far as the toolbar reaches, or the window if it must.
            let left = more_rect.map_or(rect.left(), |r| r.left() - 14.0).min(rect.right() - widest - 28.0).max(rect.left());
            let left = left.min(outer_r - widest - 28.0).max(outer_l);
            let panel = Rect::from_min_size(pos2(left, rect.bottom() + 8.0), vec2(widest + 28.0, lines.len() as f32 * MORE_LINE + 8.0));
            // On a layer of its own, like the windows, so the page under it takes no clicks (R16).
            egui::Area::new(Id::new("more_tools")).order(egui::Order::Middle).fixed_pos(panel.min).show(&ctx, |ui| {
                Self::paint_dock(ui, panel);
                ui.allocate_rect(panel, Sense::click_and_drag());
                for (k, line) in lines.iter().enumerate() {
                    let row = Rect::from_min_size(pos2(panel.left() + 14.0, panel.top() + 4.0 + k as f32 * MORE_LINE), vec2(widest, MORE_LINE));
                    ui.scope_builder(UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                        ui.spacing_mut().item_spacing = vec2(TOOL_GAP, 8.0);
                        for (j, &g) in line.iter().enumerate() {
                            self.tool_group(ui, g, j == 0, &mut tools);
                        }
                    });
                }
            });
            bottom = panel.bottom();
            // A click on the pages closes it; one in a menu opened from it does not.
            let pressed = ui.input(|i| if i.pointer.primary_pressed() { i.pointer.interact_pos() } else { None });
            if let Some(at) = pressed {
                let elsewhere = !panel.contains(at) && !rect.contains(at);
                if elsewhere && ctx.layer_id_at(at).is_none_or(|l| l.order == egui::Order::Background) {
                    self.more_tools = false;
                }
            }
        }
        self.tools_bottom = bottom;
        self.apply_tools(&ctx, tools);

        // The name takes the room right of the tools, no more than it needs; if that is too little
        // for all of it, it runs past slowly, over and over.
        let slot_left = more_rect.map_or(end, |r| r.right()) + NAME_GAP;
        let slot = Rect::from_min_max(pos2(slot_left.max(inner.right() - name_w), inner.top()), inner.max);
        let y = rect.center().y - galley.size().y / 2.0;
        let p = ui.painter().with_clip_rect(ui.clip_rect().intersect(slot.expand2(vec2(0.0, 6.0))));
        if slot.width() <= 0.0 {
            // No room for any of it.
        } else if name_w <= slot.width() + 0.5 {
            p.galley(pos2(inner.right() - name_w, y), galley, pal().text_dim);
        } else {
            // The gap between one pass and the next is narrower than the room, so the room is never empty.
            let lap = name_w + (NAME_GAP * 2.0).min(slot.width() * 0.6);
            // Each pass begins with the start of the name standing still for a while.
            let t = ui.input(|i| i.time) as f32 % (NAME_PAUSE + lap / NAME_SPEED);
            let x = slot.left() - (t - NAME_PAUSE).max(0.0) * NAME_SPEED;
            p.galley(pos2(x, y), galley.clone(), pal().text_dim);
            p.galley(pos2(x + lap, y), galley, pal().text_dim);
            let wait = if t < NAME_PAUSE { NAME_PAUSE - t } else { 0.033 };
            ctx.request_repaint_after(std::time::Duration::from_secs_f32(wait));
        }
    }

    pub fn page_bar(&mut self, ui: &mut egui::Ui, area: Rect) {
        let rect = Self::dock_rect(area, false);
        Self::paint_dock(ui, rect);
        self.scrollbar(ui, rect.shrink2(vec2(18.0, 0.0)));
    }

    /// Draw tool group `g` (see `GROUPS`), noting what was clicked in `out`, and measure it.
    fn tool_group(&mut self, ui: &mut egui::Ui, g: usize, first: bool, out: &mut Tools) {
        // The paragraph and page groups are set apart by a line, unless they start a row. It is not
        // measured with the group, so a group is as wide wherever it is; one whose width changed
        // with its place moved back and forth between two rows, every frame.
        if !first && has_line(g) {
            let (rect, _) = ui.allocate_exact_size(vec2(LINE_W, 30.0), Sense::hover());
            ui.painter().vline(rect.center().x, rect.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
        }
        let r = ui.scope(|ui| {
            match g {
                0 => self.file_tools(ui, out),
                1 => self.font_tools(ui, out),
                2 => self.letter_tools(ui, out),
                3 => self.paragraph_tools(ui, out),
                4 => {
                    out.setup = self.page_menu(ui, out);
                    self.insert_menu(ui, out);
                }
                5 => {
                    out.add_note |= ui.button("Note").on_hover_text("Attach a note to the selection or line (Ctrl+Alt+N)").clicked();
                    out.find |= ui.button("Find").on_hover_text("Search (Ctrl+F)").clicked();
                }
                _ => {
                    self.claude_menu(ui);
                    self.scene_menu(ui);
                }
            }
        });
        // Measured this frame, used the next; a changed width asks for one more frame.
        let width = r.response.rect.width();
        if (width - self.tool_w[g]).abs() > 0.5 {
            self.tool_w[g] = width;
            ui.ctx().request_repaint();
        }
    }

    fn file_tools(&mut self, ui: &mut egui::Ui, out: &mut Tools) {
        egui::containers::menu::MenuButton::new("File").ui(ui, |ui| {
            ui.set_min_width(210.0);
            let items = [
                ("Open…", "Ctrl+O", FileAction::Open),
                ("Save", "Ctrl+S", FileAction::Save),
                ("Save as…", "", FileAction::SaveAs),
                ("Export as Word (.docx)…", "", FileAction::ExportDocx),
                ("Print…", "Ctrl+P", FileAction::Print),
                ("Versions…", "", FileAction::Versions),
                ("Drop changes since saving", "", FileAction::Revert),
                ("Document info…", "", FileAction::Info),
            ];
            for (label, shortcut, action) in items {
                let button = egui::Button::new(label).shortcut_text(shortcut).frame(false);
                if ui.add(button).clicked() {
                    out.file = Some(action);
                }
            }
        });
    }

    /// The style the toolbar shows and changes: the selected text box's, else the typing style.
    fn tool_style(&self) -> Style {
        self.boxes.selected.and_then(|id| self.text_box(id)).map_or_else(|| self.typing.clone(), |b| b.style.clone())
    }

    fn font_tools(&mut self, ui: &mut egui::Ui, out: &mut Tools) {
        let shown = self.tool_style();
        egui::ComboBox::from_id_salt("font")
            .selected_text(shown.font.to_string())
            .width(FONT_W)
            .show_ui(ui, |ui| {
                for (f, _) in &self.fonts.families {
                    if ui.selectable_label(*f == *shown.font, f).clicked() {
                        out.font = Some(f.clone());
                    }
                }
            });
        let mut size = shown.size;
        ui.add_sized(
            [58.0, 30.0],
            egui::DragValue::new(&mut size).range(6.0..=96.0).speed(0.15).max_decimals(1).suffix(" pt"),
        );
        if size != shown.size {
            out.size = Some(size);
        }
    }

    /// The look of the letters, close together as one group.
    fn letter_tools(&mut self, ui: &mut egui::Ui, out: &mut Tools) {
        let t = &self.tool_style();
        let small = vec2(28.0, 30.0);
        ui.spacing_mut().item_spacing.x = 3.0;
        let b = egui::Button::new(egui::RichText::new("B").strong()).selected(t.bold).min_size(small);
        out.bold ^= ui.add(b).on_hover_text("Bold (Ctrl+B)").clicked();
        let i = egui::Button::new(egui::RichText::new("I").italics()).selected(t.italic).min_size(small);
        out.italic ^= ui.add(i).on_hover_text("Italic (Ctrl+I)").clicked();
        let u = egui::Button::new(egui::RichText::new("U").underline()).selected(t.underline).min_size(small);
        out.underline ^= ui.add(u).on_hover_text("Underline (Ctrl+U)").clicked();
        out.color = color_dropdown(ui, t.color).or(out.color);
        let tip = "Copy formatting: take up the look here, then select where it goes (Esc: put it down)";
        let brush = icon_button(ui, Icon::Brush, self.brush.is_some(), tip);
        brush.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Copy formatting"));
        out.brush ^= brush.clicked();
    }

    /// Paragraph format of the paragraph the caret is in. Alignment (with line spacing) and lists
    /// are one dropdown each, to save room.
    fn paragraph_tools(&mut self, ui: &mut egui::Ui, out: &mut Tools) {
        let mut attrs = self.doc.para_attrs_at(self.caret);
        if self.boxes.selected.is_some() {
            attrs.align = self.tool_style().para.align;
        }
        let para_change = &mut out.para;
        icon_dropdown(ui, Icon::Align(attrs.align), false, "Alignment and line spacing", |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let aligns = [
                    (Align::Left, "Align left (Ctrl+L)"),
                    (Align::Center, "Center (Ctrl+E)"),
                    (Align::Right, "Align right (Ctrl+R)"),
                    (Align::Justify, "Justify (Ctrl+J)"),
                ];
                for (a, tip) in aligns {
                    if icon_button(ui, Icon::Align(a), attrs.align == a, tip).clicked() {
                        *para_change = Some(ParaChange::Align(a));
                    }
                }
            });
            ui.add_space(2.0);
            ui.label(egui::RichText::new("Line spacing").size(12.0).color(pal().text_dim));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for s in [1.0_f32, 1.15, 1.5, 2.0, 3.0] {
                    let label = format!("{s:.2}").trim_end_matches('0').trim_end_matches('.').to_owned();
                    if ui.add(egui::Button::new(label).selected((attrs.spacing - s).abs() < 0.01).min_size(vec2(34.0, 28.0))).clicked() {
                        *para_change = Some(ParaChange::Spacing(s));
                    }
                }
            });
        });
        let list_icon = if attrs.list == ListKind::Numbered { Icon::Numbers } else { Icon::Bullets };
        icon_dropdown(ui, list_icon, attrs.list != ListKind::None, "Lists", |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if icon_button(ui, Icon::Bullets, attrs.list == ListKind::Bullet, "Bulleted list (Ctrl+Shift+L)").clicked() {
                    *para_change = Some(ParaChange::List(ListKind::Bullet));
                }
                if icon_button(ui, Icon::Numbers, attrs.list == ListKind::Numbered, "Numbered list").clicked() {
                    *para_change = Some(ParaChange::List(ListKind::Numbered));
                }
            });
        });

        let chapter = egui::Button::new("Chapter").selected(attrs.is_chapter_title()).min_size(vec2(0.0, 30.0));
        if ui.add(chapter).on_hover_text("Chapter title: starts a new chapter").clicked() {
            *para_change = Some(ParaChange::ChapterTitle(!attrs.is_chapter_title()));
        }
    }

    /// The Insert menu: a picture, a text box, a header or footer (each from a gallery of looks),
    /// a page break.
    fn insert_menu(&mut self, ui: &mut egui::Ui, out: &mut Tools) {
        egui::containers::menu::MenuButton::new("Insert")
            .config(egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside))
            .ui(ui, |ui| {
                ui.set_min_width(190.0);
                if ui.add(egui::Button::new("Picture…").frame(false)).clicked() {
                    out.insert = Some(InsertAction::Picture);
                    ui.close();
                }
                ui.menu_button("Text box", |ui| {
                    if let Some(look) = box_gallery(ui, &self.typing) {
                        out.insert = Some(InsertAction::TextBox(look));
                        ui.close();
                    }
                });
                for (label, top) in [("Header", true), ("Footer", false)] {
                    ui.menu_button(label, |ui| {
                        if let Some(band) = self.band_menu(ui, top) {
                            out.insert = Some(InsertAction::Band(top, band));
                        }
                    });
                }
                ui.separator();
                if ui.add(egui::Button::new("Page break").shortcut_text("Ctrl+Enter").frame(false)).clicked() {
                    out.insert = Some(InsertAction::PageBreak);
                    ui.close();
                }
            });
    }

    /// The gallery of header (`top`) or footer looks, and once there is one its text, fields,
    /// colour and a way to take it off. The band as changed, if it was.
    fn band_menu(&self, ui: &mut egui::Ui, top: bool) -> Option<Band> {
        let band = if top { &self.doc.setup.header } else { &self.doc.setup.footer };
        let font = self.doc.contents_style().font;
        let mut new = band.clone();
        egui::Grid::new(("band_gallery", top)).num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
            for (k, look) in BandLook::ALL.into_iter().enumerate() {
                // A look not in use is shown with its own first text, and the colour chosen.
                let text = if band.is_on() && !band.text.is_empty() { band.text.clone() } else { crate::bands::first_text(look, top).to_owned() };
                let shown = Band { look, text, color: band.color };
                ui.vertical(|ui| {
                    if crate::bands::band_tile(ui, &shown, top, band.look == look, &font).clicked() {
                        new = shown;
                    }
                });
                if k % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        if band.is_on() {
            ui.separator();
            let what = if top { "Header" } else { "Footer" };
            // It shows one thing: the page number, the chapter's title, or words of the writer's own.
            let (page, chapter) = (crate::model::PAGE_FIELD, crate::model::CHAPTER_FIELD);
            let own = band.text != page && band.text != chapter;
            ui.horizontal(|ui| {
                if ui.radio(band.text == page, "Page number").clicked() {
                    new.text = page.into();
                }
                if ui.radio(band.text == chapter, "Chapter title").on_hover_text("The chapter the page is in").clicked() {
                    new.text = chapter.into();
                }
                if ui.radio(own, "Own text").clicked() && !own {
                    new.text = String::new();
                }
            });
            if own {
                let mut text = band.text.clone();
                let field = egui::TextEdit::singleline(&mut text).hint_text(format!("{what} text")).desired_width(f32::INFINITY);
                if ui.add(field).changed() {
                    new.text = text;
                }
            }
            if let Some(c) = crate::bands::accent_row(ui, band.color) {
                new.color = c;
            }
            if ui.add(egui::Button::new(format!("Remove {}", what.to_lowercase())).frame(false)).clicked() {
                new = Band::default();
            }
        }
        (new != *band).then_some(new)
    }

    /// Do what was clicked in the tools, after they are all drawn.
    fn apply_tools(&mut self, ctx: &egui::Context, mut out: Tools) {
        if self.boxes.selected.is_some() {
            self.format_box(ctx, &mut out);
        }
        if let Some(f) = out.font {
            self.fonts.ensure(ctx, &f, false);
            let f: Arc<str> = f.into();
            self.apply_style(ctx, |s| s.font = f.clone());
        }
        if let Some(size) = out.size {
            self.apply_style(ctx, |s| s.size = size);
        }
        if out.bold {
            let bold = !self.typing.bold;
            self.apply_style(ctx, |s| s.bold = bold);
        }
        if out.italic {
            let italic = !self.typing.italic;
            self.apply_style(ctx, |s| s.italic = italic);
        }
        if out.underline {
            let underline = !self.typing.underline;
            self.apply_style(ctx, |s| s.underline = underline);
        }
        if let Some(color) = out.color {
            self.apply_style(ctx, |s| s.color = color);
        }
        if out.brush {
            self.toggle_brush();
        }
        match out.para {
            Some(ParaChange::Align(a)) => self.set_para(ctx, |p| p.align = a),
            Some(ParaChange::Spacing(s)) => self.set_para(ctx, |p| p.spacing = s),
            Some(ParaChange::List(kind)) => {
                // Clicking the active kind turns the list off.
                let kind = if self.doc.para_attrs_at(self.caret).list == kind { ListKind::None } else { kind };
                self.set_para(ctx, |p| p.list = kind);
            }
            Some(ParaChange::ChapterTitle(on)) => self.set_para(ctx, |p| p.set_chapter_title(on)),
            None => {}
        }
        if let Some(setup) = out.setup {
            self.set_setup(ctx, setup);
        }
        match out.numbering {
            Some((page, Some(n))) => self.doc.set_numbering(page, n),
            Some((page, None)) => self.doc.clear_numbering(page),
            None => {}
        }
        if let Some(page) = out.turn_to {
            self.turn_to(ctx, page, false);
        }
        if out.add_note {
            self.add_note(ctx);
        }
        if out.find {
            self.open_search();
        }
        match out.insert {
            Some(InsertAction::Picture) => {
                self.ask_path(ctx, crate::fileio::DialogFor::Picture, || {
                    rfd::FileDialog::new().add_filter("Pictures", &["png", "jpg", "jpeg"]).pick_file()
                });
            }
            Some(InsertAction::TextBox(look)) => self.insert_text_box(ctx, look),
            Some(InsertAction::Band(top, band)) => self.set_band(top, band),
            Some(InsertAction::PageBreak) => self.insert_page_break(ctx),
            None => {}
        }
        match out.file {
            Some(FileAction::Open) => self.open(ctx),
            Some(FileAction::Save) => self.save(ctx, false),
            Some(FileAction::SaveAs) => self.save(ctx, true),
            Some(FileAction::ExportDocx) => self.export_docx(ctx),
            Some(FileAction::Print) => self.open_print(ctx),
            Some(FileAction::Versions) => self.open_versions(),
            Some(FileAction::Revert) => self.drop_changes(ctx),
            Some(FileAction::Info) => self.open_info(),
            None => {}
        }
    }

    /// The letter tools and alignment act on the selected text box, all of its text: take them
    /// out of `out` and do them there.
    fn format_box(&mut self, ctx: &egui::Context, out: &mut Tools) {
        if let Some(f) = out.font.take() {
            self.fonts.ensure(ctx, &f, false);
            self.edit_selected_box(|b| b.style.font = f.into());
        }
        let toggles = [std::mem::take(&mut out.bold), std::mem::take(&mut out.italic), std::mem::take(&mut out.underline)];
        let size = out.size.take();
        let color = out.color.take();
        let align = match out.para {
            Some(ParaChange::Align(a)) => {
                out.para = None;
                Some(a)
            }
            _ => None,
        };
        if size.is_none() && color.is_none() && align.is_none() && !toggles.contains(&true) {
            return;
        }
        self.edit_selected_box(|b: &mut TextBox| {
            let st = &mut b.style;
            st.size = size.unwrap_or(st.size);
            st.color = color.unwrap_or(st.color);
            st.para.align = align.unwrap_or(st.para.align);
            st.bold ^= toggles[0];
            st.italic ^= toggles[1];
            st.underline ^= toggles[2];
        });
    }

    /// The Page menu: new page settings, if they were changed.
    fn page_menu(&mut self, ui: &mut egui::Ui, out: &mut Tools) -> Option<crate::model::PageSetup> {
        let mut setup = self.doc.setup.clone();
        let mut look = self.appearance;
        let mut theme = self.themes.theme;
        egui::containers::menu::MenuButton::new("Page")
            .config(
                egui::containers::menu::MenuConfig::new()
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
            )
            .ui(ui, |ui| {
                ui.set_min_width(250.0);
                ui.horizontal(|ui| {
                    ui.label("View");
                    ui.selectable_value(&mut look, Appearance::Book, "Book").on_hover_text("Pages turn over like a book's");
                    ui.selectable_value(&mut look, Appearance::Paperstack, "Paperstack").on_hover_text("Pages slide onto a stack");
                });
                ui.horizontal(|ui| {
                    ui.label("Theme");
                    ui.menu_button(theme.name(), |ui| theme_gallery(ui, &mut theme));
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Orientation");
                    ui.selectable_value(&mut setup.orientation, Orientation::Portrait, "Portrait");
                    ui.selectable_value(&mut setup.orientation, Orientation::Landscape, "Landscape");
                });
                ui.horizontal(|ui| {
                    ui.label("Paper");
                    for (name, w, h) in [("A4", 595.0, 842.0), ("Letter", 612.0, 792.0)] {
                        let on = (setup.paper_w - w).abs() < 1.0 && (setup.paper_h - h).abs() < 1.0;
                        if ui.selectable_label(on, name).clicked() {
                            setup.paper_w = w;
                            setup.paper_h = h;
                        }
                    }
                });
                ui.separator();
                let cm = |ui: &mut egui::Ui, label: &str, v: &mut f32| {
                    ui.label(label);
                    let mut x = *v / CM;
                    let drag = egui::DragValue::new(&mut x).range(0.0..=12.0).speed(0.02).max_decimals(2).suffix(" cm");
                    if ui.add(drag).changed() {
                        *v = x * CM;
                    }
                };
                egui::Grid::new("margins").num_columns(4).show(ui, |ui| {
                    cm(ui, "Top", &mut setup.margin_top);
                    cm(ui, "Bottom", &mut setup.margin_bottom);
                    ui.end_row();
                    cm(ui, "Left", &mut setup.margin_left);
                    cm(ui, "Right", &mut setup.margin_right);
                    ui.end_row();
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Chapter initials");
                    let tip = "The first letter of each chapter, enlarged to sink into its first lines";
                    ui.selectable_value(&mut setup.drop_cap_lines, 0, "Off").on_hover_text(tip);
                    ui.selectable_value(&mut setup.drop_cap_lines, 2, "2 lines").on_hover_text(tip);
                    ui.selectable_value(&mut setup.drop_cap_lines, 3, "3 lines").on_hover_text(tip);
                });
                ui.checkbox(&mut setup.contents, "Contents pages")
                    .on_hover_text("Pages before the story listing the chapter titles and their pages. Mark titles with the Chapter button.");
                ui.separator();
                ui.menu_button("Page numbers", |ui| self.numbers_menu(ui, out));
            });
        self.themes.choose(ui.ctx(), theme);
        if look != self.appearance {
            self.appearance = look;
            // Whatever was moving finishes the new way.
            (self.held, self.batch) = (0.0, None);
        }
        (setup != self.doc.setup).then_some(setup)
    }

    /// Page numbers, as the page shown has them: shown or not, its number (a new one starts the
    /// count again from it), figures or roman; then every page where that changes. All of it holds
    /// from the page shown until the next change (the contents' numbering, on a contents page).
    fn numbers_menu(&self, ui: &mut egui::Ui, out: &mut Tools) {
        ui.set_min_width(270.0);
        let page = self.target.min(self.last());
        let contents = self.doc.spans[page].contents.is_some();
        let now = self.doc.numbering_on(page);
        let number = self.doc.page_number(page);
        let dim = |t: &str| egui::RichText::new(t).size(12.0).color(pal().text_dim);

        // Without a header or footer showing it, no number is seen at all: say so first.
        let shown = [&self.doc.setup.header, &self.doc.setup.footer].iter().any(|b| b.is_on() && b.text.contains(PAGE_FIELD));
        if !shown {
            ui.label(dim("No header or footer shows the page number yet."));
            if ui.button("Add page numbers at the bottom").clicked() {
                out.insert = Some(InsertAction::Band(false, Band::page_numbers()));
            }
            ui.separator();
        }

        ui.label(dim(&if contents { "The contents pages".to_owned() } else { format!("Page {}", page + 1) }));
        let mut new = now;
        let mut on = now.counting != Counting::Off;
        if ui.checkbox(&mut on, "Show page numbers").clicked() {
            new.counting = if on { Counting::On } else { Counting::Off };
        }
        // A number of its own starts the count again here, and shows it.
        ui.horizontal(|ui| {
            ui.label("Number");
            let mut k = number.count;
            let roman = now.figures == Figures::Roman;
            let field = egui::DragValue::new(&mut k)
                .range(1..=9999)
                .speed(0.1)
                .custom_formatter(move |v, _| if roman { crate::numbers::roman(v as u32) } else { (v as u32).to_string() })
                .custom_parser(|t| crate::numbers::parse(t).map(f64::from));
            let resp = ui.add(field).on_hover_text("Change it to start counting again from here");
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, "Page number"));
            if resp.changed() && (k != number.count || now.counting == Counting::Off) {
                new.counting = Counting::From(k);
            }
            ui.add_space(8.0);
            ui.selectable_value(&mut new.figures, Figures::Arabic, "1, 2, 3");
            ui.selectable_value(&mut new.figures, Figures::Roman, "i, ii, iii");
        });
        ui.label(dim("From this page until the next change."));
        if new != now {
            out.numbering = Some((page, Some(new)));
        }

        let changes = self.doc.numbering_changes();
        if changes.is_empty() {
            return;
        }
        ui.separator();
        ui.label(dim("Changes"));
        let numbers = self.doc.page_numbers();
        for (at, n) in changes {
            let first = numbers.get(at).map_or(String::new(), |p| p.text());
            let what = match n.counting {
                Counting::Off => "hidden".to_owned(),
                Counting::From(_) => format!("starts again at {first}"),
                Counting::On if numbers.get(at).is_some_and(|p| p.figures == Figures::Roman) => format!("{first}, in roman"),
                Counting::On => format!("{first}, shown again"),
            };
            let place = if self.doc.spans[at].contents.is_some() { "Contents".to_owned() } else { format!("Page {}", at + 1) };
            ui.horizontal(|ui| {
                let go = ui.add(egui::Button::new(format!("{place}: {what}")).frame(false));
                if go.on_hover_text(format!("Turn to {}", place.to_lowercase())).clicked() {
                    out.turn_to = Some(at);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let undo = ui.small_button("✕").on_hover_text(format!("{place}: count on as before"));
                    undo.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Undo {place}")));
                    if undo.clicked() {
                        out.numbering = Some((at, None));
                    }
                });
            });
        }
    }

    pub fn scrollbar(&mut self, ui: &mut egui::Ui, r: Rect) {
        let n = self.doc.pages();
        let cy = r.center().y;

        // From the right: Scratchpad, Fit, the zoom, New page; the page count at the left, the track
        // between. When the bar is short, the page count goes first, then New page shrinks to "+",
        // then the track goes, then the zoom, Scratchpad, "+" and Fit, so nothing is drawn over
        // anything or past the bar however narrow the window is, and the track takes the room
        // left over rather than leaving it empty.
        let label_w = 92.0;
        let zoom_w = 68.0;
        let track_min = 60.0 + 20.0;
        let tools_w = |new_w: f32, zoom: bool| 100.0 + 8.0 + 48.0 + if zoom { zoom_w } else { 8.0 } + new_w;
        let show_label = r.width() >= label_w + 8.0 + track_min + tools_w(104.0, true);
        let new_w = if r.width() >= track_min + tools_w(104.0, true) { 104.0 } else { 30.0 };
        let show_track = r.width() >= track_min + tools_w(new_w, true);
        let show_zoom = r.width() >= tools_w(new_w, true);
        let show_scratch = r.width() >= tools_w(30.0, false);
        let show_new = r.width() >= 48.0 + 8.0 + 30.0;
        let show_fit = r.width() >= 48.0;
        if show_label {
            ui.painter().text(
                pos2(r.left(), cy),
                egui::Align2::LEFT_CENTER,
                format!("Page {} of {}", self.scrub_page().filter(|_| self.appearance == Appearance::Paperstack).unwrap_or(self.pos.round() as usize) + 1, n),
                FontId::proportional(13.0),
                pal().text_dim,
            );
        }
        let scratch_btn = Rect::from_min_max(pos2(r.right() - 100.0, cy - 15.0), pos2(r.right(), cy + 15.0));
        let fit_right = if show_scratch { scratch_btn.left() - 8.0 } else { r.right() };
        let fit_btn = Rect::from_min_max(pos2(fit_right - 48.0, cy - 15.0), pos2(fit_right, cy + 15.0));
        let new_right = fit_btn.left() - if show_zoom { zoom_w } else { 8.0 };
        let btn = Rect::from_min_max(pos2(new_right - new_w, cy - 15.0), pos2(new_right, cy + 15.0));
        let new_page = if new_w > 30.0 { egui::Button::new("+ New page") } else { egui::Button::new("+") };
        if show_new && ui.put(btn, new_page).on_hover_text("New page").clicked() {
            let cur = self.target;
            let ctx = ui.ctx().clone();
            self.insert_page_after(&ctx, cur);
        }
        // With the view's other controls: it is beside the pages, not on them.
        let scratch_tip = "Plain notes on the right, beside the pages, saved with the document";
        let scratch = egui::Button::new("Scratchpad").selected(self.scratchpad_open);
        if show_scratch && ui.put(scratch_btn, scratch).on_hover_text(scratch_tip).clicked() {
            self.toggle_scratchpad();
        }

        let fit_tip = "Show the full page (Ctrl+0). Ctrl+drag moves the page.";
        if show_fit && ui.put(fit_btn, egui::Button::new("Fit").selected(self.fit)).on_hover_text(fit_tip).clicked() {
            self.fit = true;
            self.fit_settling = true;
        }
        if show_zoom {
            ui.painter().text(
                pos2(fit_btn.left() - 8.0, cy),
                egui::Align2::RIGHT_CENTER,
                format!("{:.0}%", self.zoom * 100.0),
                FontId::proportional(13.0),
                pal().text_dim,
            );
        }

        let track_left = if show_label { r.left() + label_w + 8.0 } else { r.left() };
        let track = Rect::from_min_max(pos2(track_left, cy - 4.0), pos2(btn.left() - 20.0, cy + 4.0));
        if !show_track {
            return;
        }
        ui.painter().rect_filled(track, 4.0, pal().field);
        // Pages sliding in count only as they arrive, and a page sliding out until it is gone, so
        // the thumb glides instead of jumping when pages come and go.
        let arriving = if self.slide_in.is_some() { (self.target as f32 - self.pos).max(0.0) } else { 0.0 };
        let going = self.slide_out.map_or(0.0, |s| 1.0 - s);
        let last = (n - 1) as f32 - arriving + going;
        let thumb_w = (track.width() / (last + 1.0)).clamp(40.0, track.width());
        let travel = track.width() - thumb_w;
        if last > 0.0 && n <= 80 {
            for k in 0..=last.floor() as usize {
                let x = track.left() + thumb_w / 2.0 + travel * k as f32 / last;
                ui.painter().circle_filled(pos2(x, cy), 1.3, pal().text.gamma_multiply(0.18));
            }
        }

        // Dragging (or pressing) slides the pages half out one by one, following the thumb: right,
        // off the page; left, out of the pile behind. Letting go moves them all there together.
        // A book turns its pages along with the thumb instead.
        let resp = ui.interact(track.expand2(vec2(6.0, 12.0)), Id::new("scroll"), Sense::click_and_drag());
        self.last_track = (cy, track.left() + thumb_w / 2.0, track.right() - thumb_w / 2.0);
        let mut f = if last > 0.0 { (self.pos + going) / last } else { 0.0 };
        if n > 1 && (resp.dragged() || resp.drag_started() || resp.is_pointer_button_down_on()) {
            if let Some(p) = resp.interact_pointer_pos() {
                f = ((p.x - track.left() - thumb_w / 2.0) / travel).clamp(0.0, 1.0);
                self.scrub_to = Some(f * (n - 1) as f32);
                if self.appearance == Appearance::Book {
                    // In a book the pages turn along with the thumb.
                    self.pos = f * (n - 1) as f32;
                    let ctx = ui.ctx().clone();
                    self.turn_to(&ctx, (self.pos.round() as usize).min(self.last()), false);
                }
            }
        } else if let Some(to) = self.scrub_page() {
            self.scrub_to = None;
            if self.appearance == Appearance::Paperstack {
                let ctx = ui.ctx().clone();
                self.let_go_to(&ctx, to);
            }
        }

        let thumb = Rect::from_min_size(pos2(track.left() + travel * f, cy - 8.0), vec2(thumb_w, 16.0));
        let hot = resp.hovered() || self.scrub_to.is_some();
        ui.painter().rect_filled(thumb, 8.0, if hot { pal().accent } else { pal().thumb });
    }
}
