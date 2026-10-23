//! The scratchpad: plain text kept with the document but outside the story, for notes, lists and
//! lines not yet placed. It floats on the right of the window, not on a page, so it stays as it is
//! while the pages turn. It is saved in the file, and never printed or exported.

use eframe::egui::{self, Id, Rect, pos2, vec2};

use crate::App;

const HINT: &str = "Notes, lists, lines not yet placed… Plain text, saved with the document.";

impl App {
    pub fn toggle_scratchpad(&mut self) {
        self.scratchpad_open = !self.scratchpad_open;
    }

    /// The scratchpad's window, if it is open: on the right, the size of most of the window's height.
    pub fn scratchpad(&mut self, ctx: &egui::Context) {
        if !self.scratchpad_open {
            return;
        }
        let screen = ctx.content_rect();
        let size = vec2(320.0, (screen.height() - 160.0).clamp(200.0, 680.0));
        let at = pos2(screen.right() - size.x - 24.0, screen.top() + 70.0);
        let mut open = true;
        egui::Window::new("Scratchpad")
            .id(Id::new("scratchpad"))
            .open(&mut open)
            .default_rect(Rect::from_min_size(at, size))
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    // The field fills the window, so a click anywhere in it starts writing.
                    let field = egui::TextEdit::multiline(&mut self.doc.scratchpad)
                        .id(Id::new("scratchpad_text"))
                        .hint_text(HINT)
                        .font(egui::TextStyle::Monospace)
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY)
                        .min_size(ui.available_size());
                    ui.add(field);
                });
            });
        self.scratchpad_open = open;
    }
}
