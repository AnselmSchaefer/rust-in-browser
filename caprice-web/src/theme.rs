//! Colours and egui visuals. The desk, the docks and everything around the pages come in a few
//! themes, picked in the Page menu and remembered in the data folder (`theme.json`). The page
//! itself (`PAPER`, `INK`) is the same in every theme: it is the story, and print draws it as shown.
//!
//! The palette in use is kept per thread, for the code that draws to ask with `pal()`: egui draws
//! on one thread, and each test runs on its own, so a test choosing a theme changes no other.

use std::cell::Cell;
use std::path::PathBuf;

use eframe::egui::{self, Color32, Painter, Rect, Stroke, vec2};
use serde::{Deserialize, Serialize};

pub const INK: Color32 = Color32::from_rgb(28, 28, 32);
pub const PAPER: Color32 = Color32::from_rgb(253, 252, 249);

/// How long a new theme takes to wash over the old one, in seconds.
const FADE: f64 = 0.6;

/// The colours of everything that is not the page.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Palette {
    /// The desk under the pages: `desk` at the bottom, `desk_top` at the top.
    pub desk: Color32,
    pub desk_top: Color32,
    pub dock: Color32,
    pub dock_edge: Color32,
    pub btn: Color32,
    pub btn_hover: Color32,
    pub text: Color32,
    /// Text under the pointer.
    pub text_hot: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    /// Text and icons on the accent.
    pub on_accent: Color32,
    /// Where something is typed in a window.
    pub field: Color32,
    pub caret: Color32,
    /// The scrollbar's thumb at rest (the accent when held).
    pub thumb: Color32,
    /// egui's light widgets underneath, rather than its dark ones.
    pub light: bool,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

thread_local! {
    static NOW: Cell<Palette> = const { Cell::new(Theme::Midnight.palette()) };
}

/// The palette in use.
pub fn pal() -> Palette {
    NOW.with(Cell::get)
}

/// The themes, each with a name and a few words about it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    Midnight,
    Lagoon,
    Marmalade,
    DiscoGrape,
    Bubblegum,
    Pistachio,
    Typewriter,
}

impl Theme {
    pub const ALL: [Theme; 7] =
        [Theme::Midnight, Theme::Lagoon, Theme::Marmalade, Theme::DiscoGrape, Theme::Bubblegum, Theme::Pistachio, Theme::Typewriter];

    pub fn name(self) -> &'static str {
        match self {
            Theme::Midnight => "Midnight Ink",
            Theme::Lagoon => "Lagoon",
            Theme::Marmalade => "Marmalade",
            Theme::DiscoGrape => "Disco Grape",
            Theme::Bubblegum => "Bubblegum",
            Theme::Pistachio => "Pistachio Sorbet",
            Theme::Typewriter => "Typewriter",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Theme::Midnight => "The quiet blue hour, when the words come",
            Theme::Lagoon => "Deep water, and a coral reef to steer by",
            Theme::Marmalade => "Toast, tea and a long chapter",
            Theme::DiscoGrape => "The mirror ball is on. Write like nobody's reading",
            Theme::Bubblegum => "Pink, sweet, and a little bit sticky",
            Theme::Pistachio => "A scoop of green on a summer afternoon",
            Theme::Typewriter => "Clack, clack, ding. The ribbon is red",
        }
    }

    pub const fn palette(self) -> Palette {
        match self {
            Theme::Midnight => Palette {
                desk: rgb(30, 32, 38),
                desk_top: rgb(40, 43, 53),
                dock: rgb(42, 45, 54),
                dock_edge: rgb(62, 66, 78),
                btn: rgb(55, 59, 71),
                btn_hover: rgb(68, 73, 88),
                text: rgb(222, 225, 232),
                text_hot: Color32::WHITE,
                text_dim: rgb(140, 146, 160),
                accent: rgb(143, 162, 255),
                on_accent: rgb(20, 22, 30),
                field: rgb(34, 36, 43),
                caret: rgb(70, 90, 220),
                thumb: rgb(112, 124, 196),
                light: false,
            },
            Theme::Lagoon => Palette {
                desk: rgb(10, 38, 48),
                desk_top: rgb(18, 66, 76),
                dock: rgb(16, 58, 68),
                dock_edge: rgb(36, 96, 108),
                btn: rgb(26, 78, 90),
                btn_hover: rgb(36, 102, 116),
                text: rgb(214, 246, 240),
                text_hot: Color32::WHITE,
                text_dim: rgb(118, 178, 178),
                accent: rgb(255, 128, 104),
                on_accent: rgb(40, 18, 14),
                field: rgb(12, 46, 56),
                caret: rgb(232, 88, 64),
                thumb: rgb(70, 160, 168),
                light: false,
            },
            Theme::Marmalade => Palette {
                desk: rgb(46, 26, 18),
                desk_top: rgb(84, 46, 24),
                dock: rgb(70, 40, 26),
                dock_edge: rgb(110, 66, 40),
                btn: rgb(92, 54, 32),
                btn_hover: rgb(118, 70, 42),
                text: rgb(255, 236, 214),
                text_hot: rgb(255, 250, 240),
                text_dim: rgb(206, 160, 122),
                accent: rgb(255, 172, 48),
                on_accent: rgb(52, 28, 8),
                field: rgb(54, 30, 20),
                caret: rgb(226, 120, 20),
                thumb: rgb(196, 120, 64),
                light: false,
            },
            Theme::DiscoGrape => Palette {
                desk: rgb(26, 10, 44),
                desk_top: rgb(70, 18, 92),
                dock: rgb(46, 18, 72),
                dock_edge: rgb(150, 60, 200),
                btn: rgb(66, 28, 100),
                btn_hover: rgb(96, 40, 140),
                text: rgb(252, 232, 255),
                text_hot: Color32::WHITE,
                text_dim: rgb(196, 150, 226),
                accent: rgb(0, 240, 220),
                on_accent: rgb(20, 8, 40),
                field: rgb(34, 12, 56),
                caret: rgb(255, 40, 170),
                thumb: rgb(255, 64, 180),
                light: false,
            },
            Theme::Bubblegum => Palette {
                desk: rgb(244, 190, 214),
                desk_top: rgb(255, 222, 236),
                dock: rgb(255, 238, 246),
                dock_edge: rgb(238, 172, 204),
                btn: rgb(252, 218, 234),
                btn_hover: rgb(246, 196, 222),
                text: rgb(96, 28, 72),
                text_hot: rgb(60, 10, 44),
                text_dim: rgb(172, 98, 142),
                accent: rgb(232, 62, 150),
                on_accent: Color32::WHITE,
                field: rgb(255, 246, 250),
                caret: rgb(214, 30, 130),
                thumb: rgb(240, 140, 190),
                light: true,
            },
            Theme::Pistachio => Palette {
                desk: rgb(190, 222, 172),
                desk_top: rgb(226, 244, 208),
                dock: rgb(240, 250, 230),
                dock_edge: rgb(170, 206, 150),
                btn: rgb(222, 240, 206),
                btn_hover: rgb(202, 230, 182),
                text: rgb(36, 70, 38),
                text_hot: rgb(16, 46, 20),
                text_dim: rgb(98, 134, 92),
                accent: rgb(228, 86, 112),
                on_accent: Color32::WHITE,
                field: rgb(248, 253, 242),
                caret: rgb(200, 50, 80),
                thumb: rgb(132, 186, 110),
                light: true,
            },
            Theme::Typewriter => Palette {
                desk: rgb(214, 204, 184),
                desk_top: rgb(234, 228, 212),
                dock: rgb(246, 242, 232),
                dock_edge: rgb(196, 186, 166),
                btn: rgb(234, 228, 214),
                btn_hover: rgb(220, 212, 194),
                text: rgb(48, 42, 36),
                text_hot: Color32::BLACK,
                text_dim: rgb(124, 112, 98),
                accent: rgb(196, 36, 40),
                on_accent: Color32::WHITE,
                field: rgb(252, 250, 244),
                caret: rgb(170, 20, 24),
                thumb: rgb(150, 138, 118),
                light: true,
            },
        }
    }
}

impl Palette {
    /// `self` turned `t` of the way into `to`.
    pub fn mix(self, to: Palette, t: f32) -> Palette {
        let m = |a: Color32, b: Color32| a.lerp_to_gamma(b, t);
        Palette {
            desk: m(self.desk, to.desk),
            desk_top: m(self.desk_top, to.desk_top),
            dock: m(self.dock, to.dock),
            dock_edge: m(self.dock_edge, to.dock_edge),
            btn: m(self.btn, to.btn),
            btn_hover: m(self.btn_hover, to.btn_hover),
            text: m(self.text, to.text),
            text_hot: m(self.text_hot, to.text_hot),
            text_dim: m(self.text_dim, to.text_dim),
            accent: m(self.accent, to.accent),
            on_accent: m(self.on_accent, to.on_accent),
            field: m(self.field, to.field),
            caret: m(self.caret, to.caret),
            thumb: m(self.thumb, to.thumb),
            // egui's own widgets switch halfway through.
            light: if t < 0.5 { self.light } else { to.light },
        }
    }
}

/// The desk, lighter (or brighter) towards the top.
pub fn paint_desk(painter: &Painter, area: Rect) {
    let p = pal();
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(area.left_top(), p.desk_top);
    mesh.colored_vertex(area.right_top(), p.desk_top);
    mesh.colored_vertex(area.left_bottom(), p.desk);
    mesh.colored_vertex(area.right_bottom(), p.desk);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(mesh);
}

/// The theme chosen, where it is kept, and the one it is fading in from.
pub struct Themes {
    pub theme: Theme,
    pub file: Option<PathBuf>,
    /// The palette shown when the theme was changed, and when.
    fade: Option<(Palette, f64)>,
}

impl Themes {
    /// The theme kept in the data folder, beside the versions.
    pub fn new() -> Self {
        Self::kept_in(crate::versions::default_dir().and_then(|d| d.parent().map(|p| p.join("theme.json"))))
    }

    pub fn kept_in(file: Option<PathBuf>) -> Self {
        let theme = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|j| serde_json::from_str(&j).ok());
        Self { theme: theme.unwrap_or_default(), file, fade: None }
    }

    /// Change to `theme`: it fades in over the one shown, and is kept for next time.
    pub fn choose(&mut self, ctx: &egui::Context, theme: Theme) {
        if theme == self.theme {
            return;
        }
        self.theme = theme;
        self.fade = Some((pal(), ctx.input(|i| i.time)));
        if let (Some(file), Ok(json)) = (&self.file, serde_json::to_string(&theme)) {
            let _ = file.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(file, json);
        }
    }

    /// Show the theme, partway through its fade if it is fading. Called at the start of each frame.
    pub fn show(&mut self, ctx: &egui::Context) {
        let to = self.theme.palette();
        let shown = match self.fade {
            Some((from, since)) => {
                let t = ((ctx.input(|i| i.time) - since) / FADE).clamp(0.0, 1.0) as f32;
                if t >= 1.0 {
                    self.fade = None;
                    to
                } else {
                    ctx.request_repaint();
                    from.mix(to, t * t * (3.0 - 2.0 * t))
                }
            }
            None => to,
        };
        if shown != pal() {
            apply_palette(ctx, shown);
        }
    }
}

/// The usual theme.
pub fn apply_theme(ctx: &egui::Context) {
    apply_palette(ctx, Theme::default().palette());
}

pub fn apply_palette(ctx: &egui::Context, p: Palette) {
    NOW.with(|n| n.set(p));
    // Caprice picks its own colours; don't let the OS's light/dark setting pick between them and
    // an unstyled default (that mismatch is what made menus look so rough).
    ctx.set_theme(if p.light { egui::ThemePreference::Light } else { egui::ThemePreference::Dark });
    let mut v = if p.light { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.panel_fill = p.desk;
    v.window_fill = p.dock;
    v.window_stroke = Stroke::new(1.0, p.dock_edge);
    v.window_corner_radius = egui::CornerRadius::same(12);
    v.menu_corner_radius = egui::CornerRadius::same(10);
    v.extreme_bg_color = p.field;
    v.faint_bg_color = p.btn;
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0, p.on_accent);
    v.hyperlink_color = p.accent;
    v.text_cursor.stroke = Stroke::new(1.5, p.caret);
    let r = egui::CornerRadius::same(8);
    let w = &mut v.widgets;
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text_dim);
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.dock_edge);
    w.noninteractive.bg_fill = p.dock;
    w.noninteractive.weak_bg_fill = p.dock;
    for (state, fill) in [(&mut w.inactive, p.btn), (&mut w.hovered, p.btn_hover), (&mut w.active, p.btn_hover), (&mut w.open, p.btn)] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::NONE;
        state.fg_stroke = Stroke::new(1.0, p.text);
        state.corner_radius = r;
        state.expansion = 0.0;
    }
    w.hovered.fg_stroke = Stroke::new(1.0, p.text_hot);
    ctx.set_visuals_of(if p.light { egui::Theme::Light } else { egui::Theme::Dark }, v);
    ctx.global_style_mut(|st| {
        st.spacing.button_padding = vec2(12.0, 5.0);
        st.spacing.item_spacing = vec2(8.0, 8.0);
        st.spacing.interact_size.y = 30.0;
        st.spacing.combo_height = 340.0;
    });
}
