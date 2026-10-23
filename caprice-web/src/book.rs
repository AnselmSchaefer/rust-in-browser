//! The "Book" appearance: pages turn over like the pages of a book, hinged on their left edge,
//! and lie turned over in the pile behind, their post-its showing their blank backs.

use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke, pos2, vec2};
use egui::epaint::{Mesh, TessellationOptions, Tessellator, WHITE_UV};

use crate::App;
use crate::layout::Ink;
use crate::notes::NoteLook;
use crate::render::BEHIND_FILL;
use crate::theme::{INK, PAPER};

/// Share of a page's turn spent turning it over; the rest slides it onto the pile.
pub const TURN: f32 = 0.75;
/// Pages a second while a new page slides in, or one taken away slides out, in a book or on a
/// paperstack: unhurried, just under a second, slower than turning through pages already there.
pub const NEW_PAGE_PACE: f32 = 1.05;

impl App {
    /// Mid-turn: page `base` turns over, revealing `base + 1`, then slides into the back of the
    /// pile behind; going back, the other way round.
    pub fn book_turn(&self, ui: &egui::Ui, rect: Rect, base: usize, t: f32) {
        let ease = |s: f32| s * s * (3.0 - 2.0 * s);
        let ahead = self.last() - base - 1;
        if t < TURN {
            // Turning, it is on top of everything: coming down on the left, it covers the pile
            // and the post-its sticking out of it until it lies there turned over completely.
            self.stack(ui.painter(), rect, base, ahead);
            self.static_page(ui, rect, base + 1);
            self.flipping_page(ui, rect, base, ease(t / TURN) * std::f32::consts::PI);
        } else {
            // Then it slides into the back of the pile, under it.
            let from = vec2(-rect.width(), 0.0);
            self.turned_to_pile(ui, rect, base, from, ease((t - TURN) / (1.0 - TURN)));
            self.stack(ui.painter(), rect, base, ahead);
            self.static_page(ui, rect, base + 1);
        }
    }

    /// A new page `base + 1` comes in from `from_right` points to the right while page `base` turns
    /// over onto the pile, its post-its going with it; the new page arrives as the old one lies down.
    pub fn book_slide_in(&self, ui: &egui::Ui, rect: Rect, base: usize, t: f32, from_right: f32) {
        let ease = |s: f32| s * s * (3.0 - 2.0 * s);
        let ahead = self.last() - base - 1;
        if t < TURN {
            let incoming = rect.translate(vec2(from_right * (1.0 - ease(t / TURN)), 0.0));
            self.stack(ui.painter(), rect, base, ahead);
            self.static_page(ui, incoming, base + 1);
            self.flipping_page(ui, rect, base, ease(t / TURN) * std::f32::consts::PI);
        } else {
            self.book_turn(ui, rect, base, t);
        }
    }

    /// The other way round: the page after `i` was taken away, so it slides out `to_right` points to
    /// the right as `s` goes to 1, while page `i` comes out of the pile and turns back over it, its
    /// post-its with it. The page taken away is blank, since its text is gone.
    pub fn book_slide_out(&self, ui: &egui::Ui, rect: Rect, i: usize, s: f32, to_right: f32) {
        let ease = |s: f32| s * s * (3.0 - 2.0 * s);
        let t = 1.0 - s;
        let ahead = self.last() - i;
        let leaving = |r: Rect| {
            Self::paper(ui.painter(), r);
            ui.painter().extend(self.backdrop_shapes(ui.ctx(), i, r, &|p| p, 1.0, 1.0));
        };
        if t < TURN {
            self.stack(ui.painter(), rect, i, ahead);
            leaving(rect.translate(vec2(to_right * (1.0 - ease(t / TURN)), 0.0)));
            self.flipping_page(ui, rect, i, ease(t / TURN) * std::f32::consts::PI);
        } else {
            let from = vec2(-rect.width(), 0.0);
            self.turned_to_pile(ui, rect, i, from, ease((t - TURN) / (1.0 - TURN)));
            self.stack(ui.painter(), rect, i, ahead);
            leaving(rect);
        }
    }

    /// Draw page `i` turned over, sliding by `s` from `from` (an offset from `rect`) into its place
    /// at the back of the pile behind it. Drawn before the pile and the page at `rect`, so it passes
    /// under them.
    pub fn turned_to_pile(&self, ui: &egui::Ui, rect: Rect, i: usize, from: egui::Vec2, s: f32) {
        let ctx = ui.ctx();
        let gap = self.stack_gap(ctx, rect);
        let sheet = rect.translate(from + (crate::render::BEHIND * gap * (i + 1) as f32 - from) * s);
        // `flipping_page` lays a page turned all the way over to the left of its hinge.
        self.flipping_page(ui, sheet.translate(vec2(sheet.width(), 0.0)), i, std::f32::consts::PI);
        // The pile's edge line, coming in as the sheet arrives.
        let line = (gap * ctx.pixels_per_point() / 3.0).clamp(0.25, 1.0);
        let edge = Color32::from_black_alpha((50.0 * line * s) as u8);
        ui.painter().rect_stroke(sheet, 1.0, Stroke::new(0.6, edge), egui::StrokeKind::Inside);
    }

    /// Draw page `i` hinged on its left edge, turned by `theta` (0 = flat, PI = fully flipped).
    pub fn flipping_page(&self, ui: &egui::Ui, rect: Rect, i: usize, theta: f32) {
        let painter = ui.painter();
        let (sin, cos) = theta.sin_cos();
        let pw = rect.width();
        let sc = self.scale_of(rect);
        let spine = rect.left();
        let cy = rect.center().y;
        let lift = 0.07 * sin; // free edge swells towards the viewer
        let map = |u: f32, y: f32| -> Pos2 {
            let s = 1.0 + lift * (u / pw);
            pos2(spine + u * cos, cy + (y - cy) * s)
        };
        let front = cos >= 0.0;

        // Paper, as vertical strips so shading can vary across the curl.
        const STRIPS: usize = 28;
        let mut mesh = Mesh::default();
        for k in 0..=STRIPS {
            let f = k as f32 / STRIPS as f32;
            let u = f * pw;
            // Darker towards the free edge and as the page turns edge-on; back side a touch dimmer.
            let mut shade = 1.0 - 0.28 * sin * f - 0.10 * sin;
            if !front {
                shade -= 0.06;
            }
            let mut c = Color32::from_rgb(
                (f32::from(PAPER.r()) * shade) as u8,
                (f32::from(PAPER.g()) * shade) as u8,
                (f32::from(PAPER.b()) * shade) as u8,
            );
            if !front {
                // Lying down, the back takes on the colour of the pile it joins.
                c = c.lerp_to_gamma(BEHIND_FILL, -cos);
            }
            mesh.colored_vertex(map(u, rect.top()), c);
            mesh.colored_vertex(map(u, rect.bottom()), c);
            if k > 0 {
                let b = (k * 2) as u32;
                mesh.add_triangle(b - 2, b - 1, b);
                mesh.add_triangle(b - 1, b + 1, b);
            }
        }
        debug_assert_eq!(mesh.vertices[0].uv, WHITE_UV);

        // Soft shadow on the page underneath, just beyond the moving edge.
        if front {
            let edge = spine + pw * cos;
            let reach = 70.0 * sc * sin;
            let mut sh = Mesh::default();
            let a = (90.0 * sin) as u8;
            for (x, al) in [(edge, a), (edge + reach, 0)] {
                sh.colored_vertex(pos2(x, rect.top()), Color32::from_black_alpha(al));
                sh.colored_vertex(pos2(x, rect.bottom()), Color32::from_black_alpha(al));
            }
            sh.add_triangle(0, 1, 2);
            sh.add_triangle(1, 3, 2);
            painter.add(Shape::mesh(sh));
        }
        // Seen from behind, the post-its are under the paper: only what sticks out shows.
        let on_page = |p: Pos2| map(p.x - spine, p.y);
        let shade = 1.0 - 0.28 * sin * 0.5 - 0.10 * sin;
        if !front {
            let look = NoteLook { map: &on_page, shade, back: true, ..NoteLook::FLAT };
            self.paint_notes(ui.ctx(), painter, rect, i, &look);
        }
        painter.add(Shape::mesh(mesh));

        if front {
            painter.extend(self.backdrop_shapes(ui.ctx(), i, rect, &on_page, shade, 1.0));
            // Text: tessellate the galleys, then squash/lift the vertices with the paper.
            let ctx = ui.ctx();
            let page = self.page_layout(ctx, i, sc);
            let font_tex = ctx.fonts(|f| f.font_image_size());
            let mut tess = Tessellator::new(ctx.pixels_per_point(), TessellationOptions::default(), font_tex, vec![]);
            let mut text_mesh = Mesh::default();
            let origin = self.doc.setup.margin_origin() * sc;
            // The header, footer and text boxes bend with the paper too, those behind the story under it.
            for shape in self.underlay(ctx, i, sc).into_iter().filter_map(Ink::shape) {
                tess.tessellate_shape(shape, &mut text_mesh);
            }
            for p in &page.paras {
                if let Some((g, at)) = &p.marker {
                    tess.tessellate_shape(Shape::galley(pos2(origin.x + at.x, origin.y + p.y + at.y), g.clone(), INK), &mut text_mesh);
                }
                if p.block().is_none() {
                    tess.tessellate_shape(Shape::galley(pos2(origin.x + p.x, origin.y + p.y), p.galley.clone(), INK), &mut text_mesh);
                }
                for (g, at) in p.contents.iter().flat_map(|c| &c.texts) {
                    tess.tessellate_shape(Shape::galley(pos2(origin.x + p.x + at.x, origin.y + p.y + at.y), g.clone(), INK), &mut text_mesh);
                }
            }
            // Those in front go over the pictures, so they get a mesh of their own, drawn after them.
            let mut front_mesh = Mesh::default();
            for shape in self.furniture(ctx, i, sc, &|| self.doc.chapters(false)).into_iter().filter_map(Ink::shape) {
                tess.tessellate_shape(shape, &mut front_mesh);
            }
            let bend = |mut mesh: Mesh| {
                for v in &mut mesh.vertices {
                    v.pos = map(v.pos.x, rect.top() + v.pos.y);
                    let c = v.color;
                    v.color = Color32::from_rgba_premultiplied(
                        (f32::from(c.r()) * shade) as u8,
                        (f32::from(c.g()) * shade) as u8,
                        (f32::from(c.b()) * shade) as u8,
                        c.a(),
                    );
                }
                Shape::mesh(mesh)
            };
            painter.add(bend(text_mesh));

            // Pictures are textured quads that follow the paper the same way.
            for p in &page.paras {
                let (Some(img), Some(tex)) = (p.image, p.image.and_then(|i| self.textures.get(&i.id))) else { continue };
                let r = img.rect.translate(vec2(origin.x, origin.y + p.y));
                let tint = Color32::from_rgb(
                    (255.0 * shade) as u8,
                    (255.0 * shade) as u8,
                    (255.0 * shade) as u8,
                );
                let mut quad = Mesh::with_texture(tex.id());
                const N: usize = 8; // a few strips so it bends with the page
                for k in 0..=N {
                    let f = k as f32 / N as f32;
                    let x = r.left() + f * r.width();
                    let u = f;
                    for (y, v) in [(r.top(), 0.0), (r.bottom(), 1.0)] {
                        let pos = map(x, rect.top() + y);
                        quad.vertices.push(egui::epaint::Vertex { pos, uv: crate::images::rotated_uv(u, v, img.rotation), color: tint });
                    }
                    if k > 0 {
                        let b = (k * 2) as u32;
                        quad.add_triangle(b - 2, b - 1, b);
                        quad.add_triangle(b - 1, b + 1, b);
                    }
                }
                painter.add(Shape::mesh(quad));
            }
            painter.add(bend(front_mesh));

            let look = NoteLook { map: &on_page, shade, ..NoteLook::FLAT };
            self.paint_notes(ctx, painter, rect, i, &look);
        }
    }
}
