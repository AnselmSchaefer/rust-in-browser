//! Peeking: now and then someone of the cast peeks out from behind the side of the page being
//! read, takes hold of its edge, looks at a line that names them, blinks, tilts their head, glances
//! at the writer and ducks away again.
//!
//! They are drawn as a puppet: Claude draws each look once more, from its model sheet, peeking,
//! in parts (`PARTS`): the shoulders, the head with the whites of the eyes, the irises, the closed
//! lids and a hand. Each part is rendered on its own (`cut`), and moved as a textured quad: the
//! head leans and tilts on the neck, the irises look about inside the eyes, the lids show for a
//! blink. All but the hand are drawn behind the page, so its edge hides what is behind it; the
//! hand is drawn in front, its fingers over the edge.
//!
//! Who peeks is someone named on the page, else in its chapter, else anyone of the cast who has a
//! peek drawn. They come out on the side nearer the line they look at, every minute or two, only
//! while the writer is not writing, and duck away as soon as they are.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui::{self, Color32, ColorImage, Event, Pos2, Rect, TextureHandle, TextureOptions, Vec2, pos2, vec2};

use crate::App;
use crate::model::{Character, Look};

/// The parts of a puppet, by the ids of their groups in its drawing, back to front. The body is
/// the only one it may lack.
pub const PARTS: [&str; 5] = ["peek-body", "peek-head", "peek-irises", "peek-lids", "peek-hand"];
const BODY: usize = 0;
const HEAD: usize = 1;
const IRISES: usize = 2;
const LIDS: usize = 3;
const HAND: usize = 4;

/// Pixels per unit of the drawing its parts are rendered at.
const RENDER_SCALE: f32 = 2.0;
/// The puppet's drawing (400 units high) as a share of the page's height.
const SIZE: f32 = 0.22;
/// How far it leans out round the page's edge, in radians.
const LEAN: f32 = 0.2;

/// Seconds: rising out, looking at the line (with two blinks), glancing at the writer, ducking.
const RISE: f32 = 0.9;
const LOOK_END: f32 = 4.6;
const GLANCE_END: f32 = 5.3;
const DUCK: f32 = 0.8;
const BLINKS: [f32; 3] = [2.0, 3.7, 4.95];
const BLINK: f32 = 0.13;
/// Seconds between peeks, at random within these; the first comes sooner.
const EVERY: (f64, f64) = (60.0, 120.0);
const FIRST: (f64, f64) = (20.0, 40.0);
/// Seconds the writer must have left the keys and mouse alone.
const QUIET: f64 = 3.0;

/// What Claude is asked to draw for `c` peeking out, as they look in `look`, from its model sheet.
pub fn puppet_prompt(c: &Character, look: &Look) -> String {
    let id: String = c.name.trim().chars().map(|ch| if ch.is_alphanumeric() { ch.to_ascii_lowercase() } else { '-' }).collect();
    format!(
        "<character>\n{}: {}\n</character>\n\n<model_sheet>\n{}\n</model_sheet>\n\n\
        Draw this character peeking out from behind a page, as a puppet whose parts will be moved \
        separately. Copy the face, hair, colours and style of the model sheet.\n\
        - One standalone SVG with viewBox=\"0 0 400 400\" and no background: only the character.\n\
        - Head and shoulders, facing the viewer, the head upright and centred around x=200, the eyes at \
          about y=170, the shoulders reaching the bottom edge.\n\
        - Exactly these groups, in this order, each a <g> with this id:\n\
          - \"peek-body\": the neck and shoulders, with their clothes.\n\
          - \"peek-head\": the head: hair, ears, face, eyebrows, nose, mouth, and the whites of both eyes as \
            open almond shapes, but no irises or pupils.\n\
          - \"peek-irises\": the irises with their pupils and a small highlight, centred in the whites of \
            the eyes, looking straight ahead. They are moved a little to make the character look about, \
            so keep them well inside the whites.\n\
          - \"peek-lids\": both eyes closed, as in a blink: skin-coloured lids covering all of each eye's \
            white, with a line of lashes. It is shown over the eyes only while blinking.\n\
          - \"peek-hand\": one hand on its own, at about x=330, y=280, its fingers curled over to the \
            left as if gripping the upright edge of a sheet of paper.\n\
        - Gradients and filters in <defs>, their ids starting \"{id}-peek-\".\n\
        - No text, letters or numbers. No scripts, links, external references or embedded images.\n\
        - At most about 20 KB.\n\
        Reply with only the SVG code.",
        c.name.trim(),
        look.words.trim(),
        look.sheet.as_deref().unwrap_or_default()
    )
}

/// A puppet's parts rendered: each one's image and where it lies in the drawing, and the
/// drawing's size.
pub struct Cut {
    pub size: Vec2,
    pub parts: Vec<Option<(ColorImage, Rect)>>,
}

/// Render each part of the puppet drawing `svg` on its own. Only a drawing with them all (the body
/// may be missing) is a puppet.
pub fn cut(svg: &str) -> Result<Cut, String> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).map_err(|e| format!("the drawing could not be read: {e}"))?;
    let size = vec2(tree.size().width(), tree.size().height());
    let mut parts = Vec::new();
    for (k, id) in PARTS.iter().enumerate() {
        let part = tree.node_by_id(id).and_then(|node| {
            let b = node.abs_layer_bounding_box()?;
            let (w, h) = ((b.width() * RENDER_SCALE).ceil() as u32, (b.height() * RENDER_SCALE).ceil() as u32);
            let mut pixmap = tiny_skia::Pixmap::new(w.max(1), h.max(1))?;
            resvg::render_node(node, tiny_skia::Transform::from_scale(RENDER_SCALE, RENDER_SCALE), &mut pixmap.as_mut())?;
            let image = ColorImage::from_rgba_premultiplied([pixmap.width() as usize, pixmap.height() as usize], pixmap.data());
            Some((image, Rect::from_min_size(pos2(b.x(), b.y()), vec2(b.width(), b.height()))))
        });
        if part.is_none() && k != BODY {
            return Err(format!("the drawing has no \u{201c}{id}\u{201d}"));
        }
        parts.push(part);
    }
    Ok(Cut { size, parts })
}

/// A puppet ready to draw: its parts' textures and places, and its drawing's size.
struct Puppet {
    size: Vec2,
    parts: Vec<Option<(TextureHandle, Rect)>>,
}

/// Where a peek is in its course: how far out (0 hidden, 1 out, a little more as it overshoots),
/// how much the eyes are on the line rather than on the writer, whether the lids are shut, how far
/// the hand has taken hold, and whether it is over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub out: f32,
    pub look: f32,
    pub blink: bool,
    pub hand: f32,
    pub done: bool,
}

fn smooth(s: f32) -> f32 {
    let s = s.clamp(0.0, 1.0);
    s * s * (3.0 - 2.0 * s)
}

/// Rising with a little overshoot, then settling.
fn ease_out_back(s: f32) -> f32 {
    let s = s.clamp(0.0, 1.0) - 1.0;
    1.0 + s * s * (2.7 * s + 1.7)
}

/// The pose `t` seconds into a peek; `duck` is when it began to duck early, if it did (the writer
/// went back to writing).
pub fn pose(t: f32, duck: Option<f32>) -> Pose {
    let d0 = duck.map_or(GLANCE_END, |d| d.min(GLANCE_END));
    let ducking = ((t - d0) / DUCK).clamp(0.0, 1.0);
    let out = if t < RISE { ease_out_back(t / RISE) } else { 1.0 } * (1.0 - smooth(ducking));
    // The eyes go to the line as it comes out, and to the writer before it goes.
    let to_line = smooth((t - RISE * 0.6) / 0.4);
    let to_writer = smooth((t - LOOK_END) / 0.3);
    let look = if duck.is_some() { to_line * (1.0 - smooth((t - d0) / 0.2)) } else { to_line * (1.0 - to_writer) };
    let blink = t < d0 && BLINKS.iter().any(|&b| t >= b && t < b + BLINK);
    let hand = smooth((t - 0.25) / 0.4) * (1.0 - smooth((t - d0) / 0.3));
    Pose { out, look, blink, hand, done: t >= d0 + DUCK }
}

/// A peek under way.
#[derive(Clone, Copy, Debug)]
struct Peek {
    /// The puppet drawing's hash.
    puppet: u64,
    /// The page it is on, and whether it comes out at its right side.
    page: usize,
    right: bool,
    /// What it looks at, in shares of the page's width and height.
    at: Pos2,
    /// When it began, and began to duck early (frame time).
    started: f64,
    duck: Option<f64>,
}

/// A puppet drawing rendered in parts, by its hash.
type CutDone = (u64, Result<Cut, String>);

/// Who peeks when, the puppets ready to draw, and the peek under way.
pub struct Peeks {
    /// The next peek is due (frame time); none until first looked at.
    next_at: Option<f64>,
    /// When the writer last typed or clicked (frame time).
    busy_at: f64,
    peek: Option<Peek>,
    /// Someone asked to peek now ("Peek now"), by their id.
    asked: Option<u64>,
    puppets: HashMap<u64, Puppet>,
    cutting: HashSet<u64>,
    cut: (Sender<CutDone>, Receiver<CutDone>),
    rng: u64,
    /// The parts drawn last frame and where: their quads' corners, from the drawing's top left
    /// clockwise (mirrored on the left side), for tests.
    pub drawn: Vec<(usize, [Pos2; 4])>,
}

impl Default for Peeks {
    fn default() -> Self {
        let seed = crate::web::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.as_nanos() as u64);
        Self {
            next_at: None,
            busy_at: f64::NEG_INFINITY,
            peek: None,
            asked: None,
            puppets: HashMap::new(),
            cutting: HashSet::new(),
            cut: channel(),
            rng: seed | 1,
            drawn: Vec::new(),
        }
    }
}

impl Peeks {
    /// A number from `lo` to `hi`, at random.
    fn between(&mut self, (lo, hi): (f64, f64)) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        lo + (hi - lo) * (self.rng % 10_000) as f64 / 10_000.0
    }

    /// Someone is peeking now.
    #[cfg(test)]
    pub fn peeking(&self) -> bool {
        self.peek.is_some()
    }

    /// It is the puppet drawing `svg` peeking.
    #[cfg(test)]
    pub fn peeking_with(&self, svg: &str) -> bool {
        self.peek.is_some_and(|p| p.puppet == hash_of(svg))
    }
}

fn hash_of(svg: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    svg.hash(&mut h);
    h.finish()
}

impl App {
    /// Ask the character with id `id` to peek out now (they look at where the caret is, unless the
    /// page names them).
    pub fn peek_now(&mut self, id: u64) {
        self.peeks.asked = Some(id);
        self.peeks.peek = None;
    }

    /// Render puppet drawings not yet ready on a background thread, and take in those done.
    fn take_puppets(&mut self, ctx: &egui::Context, wanted: &[&str]) {
        while let Ok((key, cut)) = self.peeks.cut.1.try_recv() {
            self.peeks.cutting.remove(&key);
            match cut {
                Ok(cut) => {
                    let parts = cut.parts.into_iter().map(|p| p.map(|(img, r)| (ctx.load_texture("peek_part", img, TextureOptions::LINEAR), r))).collect();
                    self.peeks.puppets.insert(key, Puppet { size: cut.size, parts });
                }
                Err(e) => self.status = format!("- a peek could not be shown: {e}"),
            }
        }
        for svg in wanted {
            let key = hash_of(svg);
            if !self.peeks.puppets.contains_key(&key) && self.peeks.cutting.insert(key) {
                let (tx, ctx, svg) = (self.peeks.cut.0.clone(), ctx.clone(), (*svg).to_owned());
                crate::web::spawn(move || {
                    let _ = tx.send((key, cut(&svg)));
                    ctx.request_repaint();
                });
            }
        }
    }

    /// Those who may peek out on page `i`: named on it, else in its chapter, else anyone; each with
    /// their puppet drawing and where on the page their name first stands (page-local char).
    fn peekers(&self, i: usize) -> Vec<(u64, String, Option<usize>)> {
        let sp = self.doc.spans[i];
        let page = &self.doc.flow.text[sp.bstart..sp.bend];
        let with_peek = |c: &Character| c.look().peek.clone().map(|svg| (c.id, svg));
        if let Some(id) = self.peeks.asked {
            let c = self.doc.cast.iter().find(|c| c.id == id);
            return c.and_then(|c| with_peek(c).map(|(id, svg)| (id, svg, c.first_named(page).map(|b| page[..b].chars().count())))).into_iter().collect();
        }
        let on_page: Vec<_> = self.doc.cast.iter().filter_map(|c| {
            let at = c.first_named(page)?;
            with_peek(c).map(|(id, svg)| (id, svg, Some(page[..at].chars().count())))
        }).collect();
        if !on_page.is_empty() {
            return on_page;
        }
        // The chapter the page is in: from its title to the next.
        let chapters = self.doc.chapters(false);
        let from = chapters.iter().rev().find(|e| e.at <= sp.start).map_or(0, |e| e.at);
        let to = chapters.iter().find(|e| e.at > sp.start).map_or(self.doc.total_chars(), |e| e.at);
        let (bf, bt) = (self.doc.char_to_byte(from), self.doc.char_to_byte(to));
        let chapter = &self.doc.flow.text[bf..bt];
        let in_chapter: Vec<_> = self.doc.cast.iter().filter(|c| c.first_named(chapter).is_some()).filter_map(|c| with_peek(c).map(|(id, svg)| (id, svg, None))).collect();
        if !in_chapter.is_empty() {
            return in_chapter;
        }
        self.doc.cast.iter().filter_map(|c| with_peek(c).map(|(id, svg)| (id, svg, None))).collect()
    }

    /// Once a frame on the page settled at `i`: start a peek when one is due and the writer is
    /// quiet, end it when it is over, and have it duck when they write again.
    pub fn update_peeks(&mut self, ctx: &egui::Context, i: usize) {
        let now = ctx.input(|inp| inp.time);
        let busy = ctx.input(|inp| {
            inp.events.iter().any(|e| matches!(e, Event::Key { .. } | Event::Text(_) | Event::PointerButton { .. } | Event::Paste(_) | Event::MouseWheel { .. }))
        });
        if busy {
            self.peeks.busy_at = now;
        }
        let first = self.peeks.between(FIRST);
        let next_at = *self.peeks.next_at.get_or_insert(now + first);

        if let Some(p) = &mut self.peeks.peek {
            if p.page != i || !self.doc.peeks && self.peeks.asked.is_none() {
                self.peeks.peek = None;
            } else {
                if busy && p.duck.is_none() {
                    p.duck = Some(now);
                }
                let duck = p.duck.map(|d| (d - p.started) as f32);
                if pose((now - p.started) as f32, duck).done {
                    self.peeks.peek = None;
                    let wait = self.peeks.between(EVERY);
                    self.peeks.next_at = Some(now + wait);
                } else {
                    ctx.request_repaint();
                }
            }
            return;
        }

        let asked = self.peeks.asked.is_some();
        let due = asked || (self.doc.peeks && now >= next_at && now - self.peeks.busy_at >= QUIET);
        if !due {
            if self.doc.peeks {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64((next_at - now).max(0.5)));
            }
            return;
        }
        let peekers = self.peekers(i);
        let svgs: Vec<&str> = peekers.iter().map(|(_, svg, _)| svg.as_str()).collect();
        self.take_puppets(ctx, &svgs);
        let ready: Vec<&(u64, String, Option<usize>)> = peekers.iter().filter(|(_, svg, _)| self.peeks.puppets.contains_key(&hash_of(svg))).collect();
        if ready.is_empty() {
            if peekers.is_empty() {
                // No one to peek: look again in a while.
                self.peeks.asked = None;
                let wait = self.peeks.between(EVERY);
                self.peeks.next_at = Some(now + wait);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
            return;
        }
        let k = (self.peeks.between((0.0, ready.len() as f64)) as usize).min(ready.len() - 1);
        let (_, svg, named) = ready[k].clone();
        let at = self.peek_target(ctx, i, named);
        self.peeks.asked = None;
        self.peeks.peek = Some(Peek { puppet: hash_of(&svg), page: i, right: at.x >= 0.5, at, started: now, duck: None });
        ctx.request_repaint();
    }

    /// What a peek on page `i` looks at, in shares of the page: the line that names them, else
    /// the caret if it is on the page, else the middle of the page.
    fn peek_target(&self, ctx: &egui::Context, i: usize, named: Option<usize>) -> Pos2 {
        let sp = self.doc.spans[i];
        let caret = (self.caret >= sp.start && self.caret <= sp.end).then(|| self.caret - sp.start);
        let Some(local) = named.or(caret) else { return pos2(0.5, 0.45) };
        let layout = self.doc.layout_page(ctx, i, 1.0, &[]);
        let r = layout.caret_rect(local, false);
        let setup = &self.doc.setup;
        let size = setup.size();
        pos2((setup.margin_left + r.center().x) / size.x, (setup.margin_top + r.center().y) / size.y)
    }

    /// The parts of the peek behind the page (`behind`), or the hand in front of it, on page `i`
    /// lying in `page`.
    pub fn draw_peek(&mut self, ui: &egui::Ui, page: Rect, i: usize, behind: bool) {
        if behind {
            self.peeks.drawn.clear();
        }
        let Some(p) = self.peeks.peek.filter(|p| p.page == i) else { return };
        let Some(puppet) = self.peeks.puppets.get(&p.puppet) else { return };
        let now = ui.ctx().input(|inp| inp.time);
        let t = (now - p.started) as f32;
        let pose = pose(t, p.duck.map(|d| (d - p.started) as f32));
        let k = page.height() * SIZE / puppet.size.y;
        let Some((_, head)) = &puppet.parts[HEAD] else { return };
        let Some((_, irises)) = &puppet.parts[IRISES] else { return };
        // Worked out as if on the right side; on the left everything is mirrored about the page's middle.
        let mirror = |q: Pos2| if p.right { q } else { pos2(2.0 * page.center().x - q.x, q.y) };
        let target = mirror(page.min + p.at.to_vec2() * page.size());
        let eye_y = (target.y).clamp(page.top() + page.height() * 0.2, page.bottom() - page.height() * 0.25);
        let edge = page.right();
        // Leaning out from behind the page's edge, as one peeks round a corner: the whole figure
        // turns on its hips (the bottom of the drawing), its top out away from the page; the head
        // tilts a little back towards the line it reads, on its neck, and breathes.
        let out = pose.out.min(1.1);
        let lean = LEAN * out;
        let hip_to_head = (puppet.size.y - head.center().y) * k;
        // Upright and hidden, the head's middle well behind the edge; leaning out, just past it.
        let hidden_x = edge - head.width() * k * 0.8;
        let out_x = edge + head.width() * k * 0.05 - LEAN.sin() * hip_to_head;
        let hip_x = hidden_x + (out_x - hidden_x) * out.clamp(0.0, 1.0);
        // The drawing's origin on screen, upright: the head's middle over the hips, the eyes at `eye_y`.
        let origin = pos2(hip_x - head.center().x * k, eye_y - irises.center().y * k);
        let hip = origin + vec2(head.center().x, puppet.size.y) * k;
        let neck = origin + vec2(head.center().x, head.bottom()) * k;
        let nod = -0.06 * pose.look + (t * 2.4).sin() * 0.012;
        let turn = |about: Pos2, angle: f32, q: Pos2| {
            let (s, c) = angle.sin_cos();
            let d = q - about;
            about + vec2(d.x * c - d.y * s, d.x * s + d.y * c)
        };
        // A point of the drawing on screen, the head's (`on_neck`) tilted on the neck as well.
        let at = |v: Vec2, on_neck: bool, shift: Vec2| {
            let q = origin + v * k + shift;
            mirror(turn(hip, lean, if on_neck { turn(neck, nod, q) } else { q }))
        };
        let place = |r: &Rect, on_neck: bool, shift: Vec2| -> [Pos2; 4] {
            [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()].map(|c| at(c.to_vec2(), on_neck, shift))
        };
        // The eyes move inside their whites: in the head's own frame, towards the line (all in the
        // right side's frame, so the mirrored point is mirrored back).
        let eyes = mirror(at(irises.center().to_vec2(), true, Vec2::ZERO));
        let to = (target - eyes).normalized();
        let (s, c) = (-(lean + nod)).sin_cos();
        let local = vec2(to.x * c - to.y * s, to.x * s + to.y * c);
        // As far as the whites leave room: sideways about half an iris, up and down less (the
        // whites are low), as Claude's drawings have them.
        let reach = irises.height() * k;
        let gaze = vec2(local.x * 0.5, local.y * 0.2) * reach * pose.look;

        let painter = ui.painter();
        let mut quad = |part: usize, corners: [Pos2; 4], alpha: f32| {
            let Some((tex, _)) = &puppet.parts[part] else { return };
            let mut mesh = egui::Mesh::with_texture(tex.id());
            let white = Color32::WHITE.gamma_multiply(alpha);
            for (q, uv) in corners.iter().zip([pos2(0.0, 0.0), pos2(1.0, 0.0), pos2(1.0, 1.0), pos2(0.0, 1.0)]) {
                mesh.vertices.push(egui::epaint::Vertex { pos: *q, uv, color: white });
            }
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            painter.add(egui::Shape::mesh(mesh));
            self.peeks.drawn.push((part, corners));
        };
        if behind {
            if let Some((_, r)) = &puppet.parts[BODY] {
                quad(BODY, place(r, false, Vec2::ZERO), 1.0);
            }
            quad(HEAD, place(head, true, Vec2::ZERO), 1.0);
            quad(IRISES, place(irises, true, gaze), 1.0);
            if pose.blink && let Some((_, r)) = &puppet.parts[LIDS] {
                quad(LIDS, place(r, true, Vec2::ZERO), 1.0);
            }
            ui.ctx().request_repaint();
        } else if pose.hand > 0.0 && let Some((_, r)) = &puppet.parts[HAND] {
            // The hand holds the edge at the shoulder's height, its fingers over the page.
            let size = r.size() * k;
            let centre = pos2(edge + size.x * 0.12 + size.x * 0.5 * (1.0 - pose.hand), eye_y + head.height() * k * 0.55);
            let rect = Rect::from_center_size(centre, size);
            let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()].map(mirror);
            quad(HAND, corners, pose.hand);
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A puppet drawn by hand, for tests: a round head with two eyes, shoulders and a hand.
    pub const PUPPET: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 400 400">
<g id="peek-body"><rect x="90" y="300" width="220" height="100" rx="40" fill="#556"/></g>
<g id="peek-head"><circle cx="200" cy="180" r="110" fill="#f2c9a0"/>
<ellipse cx="160" cy="170" rx="24" ry="16" fill="#fff"/><ellipse cx="240" cy="170" rx="24" ry="16" fill="#fff"/></g>
<g id="peek-irises"><circle cx="160" cy="170" r="10" fill="#335"/><circle cx="240" cy="170" r="10" fill="#335"/></g>
<g id="peek-lids"><ellipse cx="160" cy="170" rx="25" ry="17" fill="#e0b088"/><ellipse cx="240" cy="170" rx="25" ry="17" fill="#e0b088"/></g>
<g id="peek-hand"><rect x="310" y="250" width="50" height="64" rx="22" fill="#f2c9a0"/></g>
</svg>"##;

    #[test]
    fn a_puppet_is_cut_into_its_parts_each_where_it_lies() {
        let c = cut(PUPPET).unwrap();
        assert_eq!(c.size, vec2(400.0, 400.0));
        let (_, head) = c.parts[HEAD].as_ref().unwrap();
        let (_, irises) = c.parts[IRISES].as_ref().unwrap();
        assert!((head.center().x - 200.0).abs() < 2.0 && irises.top() > head.top() && irises.bottom() < head.bottom());
        let (img, hand) = c.parts[HAND].as_ref().unwrap();
        assert!(hand.left() >= 309.0 && img.size[0] >= 100, "rendered at twice its size: {:?}", img.size);
        let without = PUPPET.replace("peek-lids", "something-else");
        assert!(cut(&without).err().unwrap_or_default().contains("peek-lids"), "a drawing without lids is no puppet");
    }

    #[test]
    fn a_peek_rises_looks_blinks_glances_and_ducks() {
        let at = |t: f32| pose(t, None);
        assert!(at(0.0).out.abs() < 0.01 && !at(0.0).done, "it starts hidden");
        assert!(at(RISE * 0.8).out > 1.0, "it overshoots a little as it comes out");
        assert!((at(2.5).out - 1.0).abs() < 0.01 && at(2.5).look > 0.99, "out, looking at the line");
        assert!(at(BLINKS[0] + 0.05).blink && !at(BLINKS[0] + 0.3).blink, "it blinks");
        assert!(at(GLANCE_END - 0.1).look < 0.01, "it glances at the writer before it goes");
        assert!(at(GLANCE_END + DUCK).done, "and is gone");
        let early = pose(2.6, Some(2.5));
        assert!(early.out < 1.0 && early.look < 1.0, "written to, it ducks at once");
        assert!(pose(2.5 + DUCK, Some(2.5)).done);
    }
}
