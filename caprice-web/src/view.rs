//! The camera: fit-to-window, pinch/keyboard zoom, scroll and Ctrl+drag panning, and swiping through pages.

use eframe::egui::{self, Id, Key, Modifiers, Rect, Sense, pos2, vec2};

use crate::App;
use crate::ui::RESERVED;

pub const MIN_ZOOM: f32 = 0.3;
pub const MAX_ZOOM: f32 = 4.0;
/// How far two fingers swipe sideways to flip one page, in points.
const SWIPE_PAGE: f32 = 50.0;

impl App {
    /// The area between the two docks.
    fn view_area(area: Rect) -> Rect {
        Rect::from_min_max(pos2(area.left(), area.top() + RESERVED), pos2(area.right(), area.bottom() - RESERVED))
    }

    /// Hold Ctrl and drag to move the page around.
    pub fn pan_with_ctrl(&mut self, ui: &mut egui::Ui, area: Rect) {
        // The reported modifier state sometimes says "Ctrl released" while it is still held
        // (seen on Hyprland), but the key press/release events are reliable. Track those instead.
        let now = ui.input(|i| i.time);
        let debug = std::env::var_os("CAPRICE_DEBUG").is_some();
        let was = self.ctrl_down;
        ui.input(|i| {
            for e in &i.events {
                match e {
                    egui::Event::Key { key: Key::ControlLeft | Key::ControlRight, pressed, .. } => {
                        self.ctrl_down = *pressed;
                        self.ctrl_via_key = *pressed;
                    }
                    // Without key events (e.g. virtual keyboards) the modifier state is all there is.
                    egui::Event::ModifiersChanged(m) if m.ctrl => self.ctrl_down = true,
                    egui::Event::ModifiersChanged(m) if !self.ctrl_via_key => self.ctrl_down = m.ctrl,
                    egui::Event::WindowFocused(false) => {
                        self.ctrl_down = false;
                        self.ctrl_via_key = false;
                    }
                    _ => {}
                }
            }
        });
        if debug && was != self.ctrl_down {
            eprintln!("[{now:.3}] ctrl {}", if self.ctrl_down { "down" } else { "up" });
        }
        if !self.ctrl_down {
            return;
        }
        let view = Self::view_area(area);
        // Registered after the text editor, so it wins the drag.
        let resp = ui.interact(view, Id::new("pan"), Sense::drag());
        // Don't rely on `resp.hovered()`: it can be false for a frame right after a drag ends.
        let over = ui.input(|i| i.pointer.latest_pos()).is_some_and(|p| view.contains(p));
        if over || resp.dragged() {
            ui.output_mut(|o| {
                o.cursor_icon = if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab }
            });
        }
        if resp.dragged() {
            self.origin += resp.drag_delta();
            self.fit = false;
            ui.ctx().request_repaint();
        }
    }

    /// Two-finger swipe sideways over the page flips through the pages, one per `SWIPE_PAGE` points.
    pub fn swipe_pages(&mut self, ctx: &egui::Context, area: Rect) {
        let (d, hover, now) = ctx.input(|i| (i.smooth_scroll_delta, i.pointer.hover_pos(), i.time));
        let over = hover.is_some_and(|p| {
            Self::view_area(area).contains(p) && ctx.layer_id_at(p).is_none_or(|l| l.order == egui::Order::Background)
        });
        if !over || d.x.abs() <= d.y.abs() {
            return;
        }
        // A pause starts a new swipe, so leftovers of the last one don't add up to a flip.
        if now - self.swipe_at > 0.3 {
            self.swipe = 0.0;
        }
        self.swipe_at = now;
        // Fingers moving left (content moving left) go forward, like turning a page.
        self.swipe -= d.x;
        let pages = (self.swipe / SWIPE_PAGE).trunc();
        if pages != 0.0 {
            self.swipe -= pages * SWIPE_PAGE;
            let to = (self.target as i64 + pages as i64).clamp(0, self.last() as i64) as usize;
            if to != self.target {
                self.turn_to(ctx, to, false);
                self.swiped = true;
            }
        }
    }

    /// Pinch / Ctrl+scroll to zoom around the pointer, two-finger scroll up and down to pan. Returns the page rectangle.
    pub fn page_rect(&mut self, ctx: &egui::Context, area: Rect) -> Rect {
        let view = Self::view_area(area);
        let size = self.doc.setup.size();
        // Leave room on both sides for post-its sticking out of the page and the stacks.
        let fit = ((view.height() - 16.0) / size.y).min((view.width() - 48.0) / (size.x + 2.0 * crate::notes::NOTE_OUT)).clamp(MIN_ZOOM, 3.0);
        let (pinch, scroll, hover) = ctx.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.pointer.hover_pos()));
        let keys = ctx.input_mut(|i| {
            let mut z = 1.0;
            if i.consume_key(Modifiers::COMMAND, Key::Equals) || i.consume_key(Modifiers::COMMAND, Key::Plus) {
                z *= 1.1;
            }
            if i.consume_key(Modifiers::COMMAND, Key::Minus) {
                z /= 1.1;
            }
            z
        });
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Num0)) {
            self.fit = true;
            self.fit_settling = true;
        }
        if self.fit {
            let centered = pos2(view.center().x - size.x * fit / 2.0, view.center().y - size.y * fit / 2.0);
            if self.fit_settling {
                // Asking for "fit" glides the page to the middle instead of jumping there.
                let dt = ctx.input(|i| i.stable_dt).min(0.05);
                let k = 1.0 - (-12.0 * dt).exp();
                self.zoom += (fit - self.zoom) * k;
                self.origin += (centered - self.origin) * k;
                if (self.origin - centered).length() < 0.5 && (self.zoom - fit).abs() < 1e-3 {
                    self.fit_settling = false;
                }
                ctx.request_repaint();
            }
            if !self.fit_settling {
                // Settled: follow the window size exactly.
                self.zoom = fit;
                self.origin = centered;
            }
        }
        let z = pinch * keys;
        if (z - 1.0).abs() > 1e-4 {
            let anchor = hover.filter(|p| view.contains(*p)).unwrap_or(view.center());
            let mut new = (self.zoom * z).clamp(MIN_ZOOM, MAX_ZOOM);
            if (new - fit).abs() < 0.02 {
                new = fit; // stop at the fitting size on the way through
            }
            self.origin = anchor + (self.origin - anchor) * (new / self.zoom);
            self.zoom = new;
            // Zooming never moves the page to the middle by itself: only a page that is already
            // centred at the fitting size counts as fitted (and then follows the window size).
            let centered = pos2(view.center().x - size.x * fit / 2.0, view.center().y - size.y * fit / 2.0);
            self.fit = new == fit && (self.origin - centered).length() < 1.0;
            self.fit_settling = false;
        }
        // Sideways swipes flip pages instead (see `swipe_pages`); Ctrl+drag still pans sideways.
        // Scrolling over a window (the scratchpad, Claude's answer) scrolls that, not the pages.
        let over_pages = hover.is_some_and(|p| ctx.layer_id_at(p).is_none_or(|l| l.order == egui::Order::Background));
        if !self.fit && over_pages && scroll.y.abs() >= scroll.x.abs() {
            self.origin.y += scroll.y;
        }

        // The page can be moved anywhere as long as a good part of it stays in view.
        let (w, h) = (size.x * self.zoom, size.y * self.zoom);
        let keep = 80.0;
        self.origin = pos2(
            self.origin.x.clamp(view.left() + keep - w, view.right() - keep),
            self.origin.y.clamp(view.top() + keep - h, view.bottom() - keep),
        );
        Rect::from_min_size(self.origin, vec2(w, h))
    }
}
