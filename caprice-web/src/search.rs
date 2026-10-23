//! Find and replace: highlights every match, and the page flips to the current one. Unfolded,
//! the bar replaces the current match or all of them (one undo step), and finds case and whole
//! words only, as renaming someone in the story needs.

use eframe::egui::{self, Color32, Id, Key, Modifiers, Rect, Sense, Stroke, TextEdit, UiBuilder, pos2, vec2};

use crate::App;
use crate::edit::{Edit, Piece};
use crate::model::{PAGE_BREAK, ParaAttrs, is_terminator};
use crate::notes::SEARCH_FIELD;
use crate::theme::pal;
use crate::ui::{DOCK_GAP, DOCK_H};

/// The side of the bar's square buttons, and the field's height.
const BAR_BTN: f32 = 30.0;
/// Room for "No results" or "12 / 340".
const COUNT_W: f32 = 64.0;
/// The height of the bar's first row, and of each row unfolded under it.
const ROW_H: f32 = 42.0;
const MORE_H: f32 = 38.0;
/// The hint of the field to replace with (for tests to find it).
pub const REPLACE_FIELD: &str = "Replace with";

#[derive(Clone, Copy)]
enum Glyph {
    Previous,
    Next,
    Close,
    /// Unfold (or fold) replacing and the options.
    More(bool),
}

/// A square button with a drawn glyph: the font has no ✕, and its ‹ › are too small to hit.
fn bar_button(ui: &mut egui::Ui, glyph: Glyph, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(BAR_BTN, BAR_BTN), Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tip));
    let p = ui.painter();
    p.rect_filled(rect, 8.0, if resp.hovered() && enabled { pal().btn_hover } else { pal().btn });
    let ink = if !enabled { pal().text_dim.gamma_multiply(0.6) } else if resp.hovered() { pal().text_hot } else { pal().text };
    let s = Stroke::new(1.6, ink);
    let c = rect.center();
    match glyph {
        Glyph::Previous | Glyph::Next => {
            let dx = if matches!(glyph, Glyph::Next) { 1.0 } else { -1.0 };
            p.line_segment([pos2(c.x - 2.5 * dx, c.y - 5.0), pos2(c.x + 2.5 * dx, c.y)], s);
            p.line_segment([pos2(c.x + 2.5 * dx, c.y), pos2(c.x - 2.5 * dx, c.y + 5.0)], s);
        }
        Glyph::More(open) => {
            let dy = if open { -1.0 } else { 1.0 };
            p.line_segment([pos2(c.x - 5.0, c.y - 2.5 * dy), pos2(c.x, c.y + 2.5 * dy)], s);
            p.line_segment([pos2(c.x, c.y + 2.5 * dy), pos2(c.x + 5.0, c.y - 2.5 * dy)], s);
        }
        Glyph::Close => {
            let d = 4.5;
            p.line_segment([c - vec2(d, d), c + vec2(d, d)], s);
            p.line_segment([c + vec2(-d, d), c + vec2(d, -d)], s);
        }
    }
    resp.on_hover_text(tip)
}

#[derive(Default)]
pub struct Search {
    pub open: bool,
    pub query: String,
    /// Flow ranges (chars) of every match.
    pub matches: Vec<(usize, usize)>,
    pub current: usize,
    /// (document version, query, match case, whole words) the matches were computed for.
    stamp: Option<(u64, String, bool, bool)>,
    want_focus: bool,
    /// Replacing and the options show, under the field.
    pub more: bool,
    pub replace: String,
    /// Only matches with the same capitals; only whole words.
    pub case: bool,
    pub words: bool,
    /// After replacing one, the next match is the first from here (the text replaced may match too).
    resume_at: Option<usize>,
    want_replace_focus: bool,
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// All non-overlapping occurrences of `needle` in `text`, as char ranges: whatever their capitals
/// unless `case`, and only where neither end is inside a word if `words` ("Ann" is not in "Anne").
pub fn find_all(text: &str, needle: &str, case: bool, words: bool) -> Vec<(usize, usize)> {
    let raw: Vec<char> = text.chars().collect();
    let hay: Vec<char> = if case { raw.clone() } else { raw.iter().map(|&c| fold(c)).collect() };
    let pat: Vec<char> = needle.chars().map(|c| if case { c } else { fold(c) }).collect();
    let wordy = |k: usize| raw.get(k).is_some_and(|c| c.is_alphanumeric() || *c == '_');
    let whole = |a: usize, b: usize| !words || (!(a > 0 && wordy(a - 1) && wordy(a)) && !(wordy(b - 1) && wordy(b)));
    let mut out = Vec::new();
    if pat.is_empty() || pat.len() > hay.len() {
        return out;
    }
    let mut k = 0;
    while k + pat.len() <= hay.len() {
        if hay[k..k + pat.len()] == pat[..] && whole(k, k + pat.len()) {
            out.push((k, k + pat.len()));
            k += pat.len();
        } else {
            k += 1;
        }
    }
    out
}

impl App {
    pub fn open_search(&mut self) {
        self.search.open = true;
        self.search.want_focus = true;
    }

    /// Open the bar unfolded, to replace: in the field to replace with once there is something to find.
    pub fn open_replace(&mut self) {
        self.search.open = true;
        self.search.more = true;
        if self.search.query.is_empty() {
            self.search.want_focus = true;
        } else {
            self.search.want_replace_focus = true;
        }
    }

    /// The edit putting the replacement in for chars `a..b`, in the style of the first of them,
    /// so a bold name stays bold.
    fn replacement(&self, a: usize, b: usize) -> Edit {
        let text: String = self.search.replace.chars().filter(|&c| c != '\r' && c != PAGE_BREAK && !is_terminator(c)).collect();
        let style = self.doc.flow.styles[a].with_para(ParaAttrs::default());
        let (ba, bb) = (self.doc.char_to_byte(a), self.doc.char_to_byte(b));
        let old = Piece { text: self.doc.flow.text[ba..bb].to_owned(), styles: self.doc.flow.styles[a..b].to_vec() };
        Edit::Replace { at: a, old, new: Piece::plain(&text, &style) }
    }

    /// Replace the current match, and go on to the next.
    pub fn replace_current(&mut self, ctx: &egui::Context) {
        self.refresh_matches();
        let Some(&(a, b)) = self.search.matches.get(self.search.current) else { return };
        let edit = self.replacement(a, b);
        let after = edit.caret_after();
        self.apply_edit(ctx, edit, b);
        self.search.resume_at = Some(after);
        self.refresh_matches();
    }

    /// Replace every match, as one undo step.
    pub fn replace_all(&mut self, ctx: &egui::Context) {
        self.refresh_matches();
        let found = self.search.matches.clone();
        let Some(&(first, _)) = found.first() else { return };
        // From the last, so the places of those before stay as found.
        let edits: Vec<Edit> = found.iter().rev().map(|&(a, b)| self.replacement(a, b)).collect();
        let caret = edits.last().map_or(first, Edit::caret_after);
        self.doc.apply_group(edits);
        self.doc.full_paginate(ctx, &self.typing);
        self.set_caret(ctx, caret, false);
        self.status = format!("- replaced {} time{}", found.len(), if found.len() == 1 { "" } else { "s" });
        self.refresh_matches();
    }

    fn close_search(&mut self, ctx: &egui::Context) {
        if let Some(&(s, _)) = self.search.matches.get(self.search.current) {
            self.set_caret(ctx, s, false);
        }
        self.search.open = false;
        self.search.matches.clear();
        self.search.stamp = None;
        ctx.request_repaint();
    }

    fn refresh_matches(&mut self) {
        let s = &self.search;
        let stamp = (self.doc.version, s.query.clone(), s.case, s.words);
        if s.stamp.as_ref() == Some(&stamp) {
            return;
        }
        let query_changed = s.stamp.as_ref().is_none_or(|old| (&old.1, old.2, old.3) != (&stamp.1, stamp.2, stamp.3));
        self.search.matches = find_all(&self.doc.flow.text, &self.search.query, self.search.case, self.search.words);
        self.search.stamp = Some(stamp);
        let n = self.search.matches.len();
        if let Some(from) = self.search.resume_at.take().filter(|_| n > 0) {
            let next = self.search.matches.iter().position(|&(s, _)| s >= from).unwrap_or(0);
            self.jump_to_match(next);
        } else if n == 0 {
            self.search.current = 0;
        } else if query_changed {
            let caret = self.caret;
            let first = self.search.matches.iter().position(|&(s, _)| s >= caret).unwrap_or(0);
            self.jump_to_match(first);
        } else {
            self.search.current = self.search.current.min(n - 1);
        }
    }

    /// Make match `k` the current one and flip to its page.
    fn jump_to_match(&mut self, k: usize) {
        let Some(&(s, _)) = self.search.matches.get(k) else { return };
        self.search.current = k;
        self.target = self.doc.page_of(s);
    }

    fn step_match(&mut self, forward: bool) {
        let n = self.search.matches.len();
        if n > 0 {
            let k = if forward { (self.search.current + 1) % n } else { (self.search.current + n - 1) % n };
            self.jump_to_match(k);
        }
    }

    pub fn search_bar(&mut self, ui: &mut egui::Ui, area: Rect) {
        if !self.search.open {
            return;
        }
        let ctx = ui.ctx().clone();
        let dock_w = (area.width() - 48.0).min(crate::ui::DOCK_MAX_W);
        let width = 430.0_f32.min(dock_w);
        let rect = Rect::from_min_size(
            pos2(area.center().x + dock_w / 2.0 - width, self.tools_bottom.max(area.top() + DOCK_GAP + DOCK_H) + 8.0),
            vec2(width, ROW_H + if self.search.more { 2.0 * MORE_H } else { 0.0 }),
        );
        let p = ui.painter();
        for k in 1..=4 {
            p.rect_filled(rect.expand(k as f32 * 2.0).translate(vec2(0.0, 3.0)), 14.0 + k as f32 * 2.0, Color32::from_black_alpha(20));
        }
        p.rect_filled(rect, 14.0, pal().dock);
        p.rect_stroke(rect, 14.0, Stroke::new(1.0, pal().dock_edge), egui::StrokeKind::Inside);

        let (mut forward, mut backward, mut close, mut more) = (false, false, false, false);
        let (mut replace, mut replace_all) = (false, false);
        let row = Rect::from_min_size(rect.min, vec2(rect.width(), ROW_H)).shrink2(vec2(10.0, 0.0));
        ui.scope_builder(
            UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                // The field takes what the count and the four buttons leave, and is as tall as they are.
                let field_w = (ui.available_width() - COUNT_W - 4.0 * BAR_BTN - 5.0 * 6.0).max(40.0);
                let field = TextEdit::singleline(&mut self.search.query)
                    .id(Id::new(SEARCH_FIELD))
                    .hint_text("Find in document")
                    .vertical_align(egui::Align::Center)
                    .desired_width(f32::INFINITY);
                let resp = ui.add_sized(vec2(field_w, BAR_BTN), field);
                if std::mem::take(&mut self.search.want_focus) {
                    resp.request_focus();
                }
                if resp.changed() {
                    ctx.request_repaint();
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    if ui.input(|i| i.modifiers.shift) {
                        backward = true;
                    } else {
                        forward = true;
                    }
                    resp.request_focus();
                }
                if resp.has_focus() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                    close = true;
                }
                let n = self.search.matches.len();
                let label = match (self.search.query.is_empty(), n) {
                    (true, _) => String::new(),
                    (false, 0) => "No results".to_owned(),
                    (false, _) => format!("{} / {}", self.search.current + 1, n),
                };
                // A slot of its own, so the buttons stay put whatever it says.
                ui.allocate_ui_with_layout(vec2(COUNT_W, BAR_BTN), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.set_min_size(vec2(COUNT_W, BAR_BTN));
                    ui.label(egui::RichText::new(label).size(12.5).color(pal().text_dim));
                });
                ui.add_enabled_ui(n > 0, |ui| {
                    backward |= bar_button(ui, Glyph::Previous, "Previous (Shift+Enter)").clicked();
                    forward |= bar_button(ui, Glyph::Next, "Next (Enter)").clicked();
                });
                more = bar_button(ui, Glyph::More(self.search.more), "Replace, and match case or whole words (Ctrl+H)").clicked();
                close |= bar_button(ui, Glyph::Close, "Close (Esc)").clicked();
            },
        );
        if self.search.more {
            let n = self.search.matches.len();
            let row = Rect::from_min_size(rect.min + vec2(0.0, ROW_H - 4.0), vec2(rect.width(), MORE_H)).shrink2(vec2(10.0, 0.0));
            ui.scope_builder(UiBuilder::new().max_rect(row).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_enabled_ui(n > 0, |ui| {
                    replace_all = ui.add_sized(vec2(44.0, BAR_BTN), egui::Button::new("All")).on_hover_text("Replace every match").clicked();
                    replace = ui.add_sized(vec2(72.0, BAR_BTN), egui::Button::new("Replace")).on_hover_text("Replace this match").clicked();
                });
                let field = TextEdit::singleline(&mut self.search.replace)
                    .id(Id::new("replace_field"))
                    .hint_text(REPLACE_FIELD)
                    .vertical_align(egui::Align::Center)
                    .desired_width(f32::INFINITY);
                let resp = ui.add_sized(vec2(ui.available_width(), BAR_BTN), field);
                if std::mem::take(&mut self.search.want_replace_focus) {
                    resp.request_focus();
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    replace = n > 0;
                    resp.request_focus();
                }
                if resp.has_focus() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                    close = true;
                }
            });
            let row = row.translate(vec2(0.0, MORE_H - 6.0));
            ui.scope_builder(UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                ui.checkbox(&mut self.search.case, "Match case");
                ui.checkbox(&mut self.search.words, "Whole words");
            });
        }
        self.refresh_matches();
        if more {
            self.search.more = !self.search.more;
        }
        if replace {
            self.replace_current(&ctx);
        }
        if replace_all {
            self.replace_all(&ctx);
        }
        if forward {
            self.step_match(true);
        }
        if backward {
            self.step_match(false);
        }
        if close {
            self.close_search(&ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_case_insensitively_without_overlap() {
        assert_eq!(find_all("Abc abc ABC", "abc", false, false), vec![(0, 3), (4, 7), (8, 11)]);
        assert_eq!(find_all("aaaa", "aa", false, false), vec![(0, 2), (2, 4)]);
        assert!(find_all("abc", "", false, false).is_empty());
        assert!(find_all("abc", "abcd", false, false).is_empty());
    }

    #[test]
    fn finds_with_the_same_capitals_or_whole_words_only_when_asked() {
        let text = "Ann met Anne and ann's dog, Ann.";
        assert_eq!(find_all(text, "Ann", false, false).len(), 4);
        assert_eq!(find_all(text, "Ann", true, false), vec![(0, 3), (8, 11), (28, 31)]);
        assert_eq!(find_all(text, "Ann", false, true), vec![(0, 3), (17, 20), (28, 31)], "\"ann's\" names her, \"Anne\" does not");
        assert_eq!(find_all(text, "Ann", true, true), vec![(0, 3), (28, 31)]);
    }

    #[test]
    fn positions_are_chars_not_bytes() {
        assert_eq!(find_all("ünï x", "x", false, false), vec![(4, 5)]);
    }
}
