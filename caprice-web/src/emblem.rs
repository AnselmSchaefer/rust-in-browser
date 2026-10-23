//! The emblem beside the name on the welcome page: the app icon come alive. On its purple tile
//! lies a page, and a pencil draws the icon's coral loop across it, sets the orange dot at its end,
//! lifts, and after a while the loop fades and it draws it again.
//!
//! Everything is drawn from shapes (egui fills only convex ones, R11), in the tile's own units
//! from 0 to 1, so it can be any size.

use std::f32::consts::PI;

use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};

/// Seconds the pencil takes to draw the loop, the loop then rests drawn, and fades.
const DRAW: f32 = 2.4;
const REST: f32 = 2.2;
const FADE: f32 = 0.6;
/// One whole round.
pub const ROUND: f32 = DRAW + REST + FADE;
/// How far above the tile the pencil reaches, in tile sides.
pub const HEADROOM: f32 = 0.25;

const TILE_TOP: Color32 = Color32::from_rgb(150, 140, 255);
const TILE_BOTTOM: Color32 = Color32::from_rgb(92, 70, 220);
const LOOP: Color32 = Color32::from_rgb(244, 96, 122);
const DOT: Color32 = Color32::from_rgb(250, 168, 76);
const PENCIL: Color32 = Color32::from_rgb(248, 198, 72);
const PENCIL_SHADE: Color32 = Color32::from_rgb(226, 166, 44);
const WOOD: Color32 = Color32::from_rgb(240, 206, 164);
const LEAD: Color32 = Color32::from_rgb(58, 56, 70);
const BAND: Color32 = Color32::from_rgb(204, 208, 220);
const ERASER: Color32 = Color32::from_rgb(240, 138, 160);

/// The loop, as the icon has it: up from the lower left, round once, and off to the upper right.
const CURVE: [[(f32, f32); 4]; 4] = [
    [(0.14, 0.80), (0.26, 0.62), (0.46, 0.68), (0.48, 0.53)],
    [(0.48, 0.53), (0.50, 0.40), (0.35, 0.38), (0.36, 0.51)],
    [(0.36, 0.51), (0.37, 0.64), (0.56, 0.66), (0.66, 0.51)],
    [(0.66, 0.51), (0.73, 0.40), (0.77, 0.30), (0.83, 0.22)],
];

/// Points along the loop, evenly in the curves' own steps, in tile units.
fn loop_points() -> Vec<Pos2> {
    let mut out = Vec::new();
    for c in CURVE {
        let [a, b, cc, d] = c.map(|(x, y)| vec2(x, y));
        let steps = 40;
        for k in 0..steps {
            let t = k as f32 / steps as f32;
            let u = 1.0 - t;
            let p = a * (u * u * u) + b * (3.0 * u * u * t) + cc * (3.0 * u * t * t) + d * (t * t * t);
            out.push(p.to_pos2());
        }
    }
    let (x, y) = CURVE[3][3];
    out.push(pos2(x, y));
    out
}

/// How far the loop is drawn (0 to 1) and how strongly it shows, `t` seconds into a round.
pub fn drawn_at(t: f32) -> (f32, f32) {
    let t = t.rem_euclid(ROUND);
    if t < DRAW {
        // Slow at the start and the end, like a hand.
        let s = t / DRAW;
        (s * s * (3.0 - 2.0 * s), 1.0)
    } else if t < DRAW + REST {
        (1.0, 1.0)
    } else {
        (1.0, 1.0 - (t - DRAW - REST) / FADE)
    }
}

/// The point the loop has reached when `drawn` of it is, in tile units.
pub fn tip_at(drawn: f32) -> Pos2 {
    let pts = loop_points();
    let at = drawn.clamp(0.0, 1.0) * (pts.len() - 1) as f32;
    let k = (at.floor() as usize).min(pts.len() - 2);
    pts[k].lerp(pts[k + 1], at - k as f32)
}

/// A rounded square filled from `top` at its upper left to `bottom` at its lower right.
fn gradient_tile(rect: Rect, radius: f32, top: Color32, bottom: Color32) -> Shape {
    let mut mesh = egui::Mesh::default();
    let colour = |p: Pos2| {
        let t = ((p.x - rect.left()) + (p.y - rect.top())) / (rect.width() + rect.height());
        top.lerp_to_gamma(bottom, t.clamp(0.0, 1.0))
    };
    mesh.colored_vertex(rect.center(), colour(rect.center()));
    let corners = [
        (rect.right_bottom() - vec2(radius, radius), 0.0),
        (rect.left_bottom() + vec2(radius, -radius), 0.5 * PI),
        (rect.left_top() + vec2(radius, radius), PI),
        (rect.right_top() + vec2(-radius, radius), 1.5 * PI),
    ];
    for (c, start) in corners {
        for k in 0..=8 {
            let a = start + k as f32 / 8.0 * 0.5 * PI;
            let p = c + vec2(a.cos(), a.sin()) * radius;
            mesh.colored_vertex(p, colour(p));
        }
    }
    let n = mesh.vertices.len() as u32;
    for k in 1..n {
        mesh.add_triangle(0, k, if k + 1 < n { k + 1 } else { 1 });
    }
    Shape::mesh(mesh)
}

/// The corners of a rectangle `size` big, its centre at `c`, turned by `angle`.
fn turned(c: Pos2, size: Vec2, angle: f32) -> Vec<Pos2> {
    let (s, co) = angle.sin_cos();
    let h = size / 2.0;
    [vec2(-h.x, -h.y), vec2(h.x, -h.y), vec2(h.x, h.y), vec2(-h.x, h.y)]
        .into_iter()
        .map(|v| c + vec2(v.x * co - v.y * s, v.x * s + v.y * co))
        .collect()
}

/// Draw the emblem into `rect` (a square), `t` seconds after the welcome page first showed. The
/// pencil reaches out of it, up to `HEADROOM` of its side above it.
pub fn draw(painter: &egui::Painter, rect: Rect, t: f32) {
    let side = rect.width();
    let at = |p: Pos2| rect.min + p.to_vec2() * side;

    // A soft glow of the tile's colour behind it.
    for (grow, alpha) in [(0.32, 0.05), (0.2, 0.08), (0.1, 0.12)] {
        painter.circle_filled(rect.center(), side * (0.5 + grow), TILE_BOTTOM.gamma_multiply(alpha));
    }
    painter.add(gradient_tile(rect, side * 0.22, TILE_TOP, TILE_BOTTOM));

    // The page, a little askew, with a shadow and lines of writing.
    let tilt = -0.12;
    let page_c = at(pos2(0.47, 0.50));
    let page = vec2(0.50, 0.64) * side;
    painter.add(Shape::convex_polygon(turned(page_c + vec2(0.02, 0.03) * side, page, tilt), Color32::from_black_alpha(50), Stroke::NONE));
    painter.add(Shape::convex_polygon(turned(page_c, page, tilt), Color32::from_rgb(250, 249, 252), Stroke::NONE));
    for (k, w) in [0.30, 0.26, 0.18].into_iter().enumerate() {
        let y = -0.20 + k as f32 * 0.075;
        let (s, co) = tilt.sin_cos();
        let off = vec2(-0.13 + w / 2.0, y) * side;
        let c = page_c + vec2(off.x * co - off.y * s, off.x * s + off.y * co);
        painter.add(Shape::convex_polygon(turned(c, vec2(w, 0.028) * side, tilt), Color32::from_rgb(196, 200, 226), Stroke::NONE));
    }

    // The loop, as far as it is drawn.
    let (drawn, shown) = drawn_at(t);
    let pts = loop_points();
    let upto = ((pts.len() - 1) as f32 * drawn) as usize;
    let mut line: Vec<Pos2> = pts[..=upto].iter().map(|p| at(*p)).collect();
    line.push(at(tip_at(drawn)));
    if line.len() > 1 && shown > 0.0 {
        painter.add(Shape::line(line, Stroke::new(side * 0.06, LOOP.gamma_multiply(shown))));
        // Round ends, where the line begins and where the pencil is.
        painter.circle_filled(at(pts[0]), side * 0.03, LOOP.gamma_multiply(shown));
        painter.circle_filled(at(tip_at(drawn)), side * 0.03, LOOP.gamma_multiply(shown));
    }
    if drawn >= 1.0 {
        painter.circle_filled(at(pts[pts.len() - 1]), side * 0.055, DOT.gamma_multiply(shown));
    }

    // The pencil: its point on the line while drawing, lifted off and away once done.
    let round = t.rem_euclid(ROUND);
    let lift = ((round - DRAW) / 0.4).clamp(0.0, 1.0) * (1.0 - ((round - ROUND + 0.5) / 0.5).clamp(0.0, 1.0));
    let tip = at(tip_at(drawn)) + vec2(0.10, -0.12) * side * lift;
    // Leaning to the right, as a right hand holds it, swaying a little as it writes.
    let angle = 0.62 + (t * 7.0).sin() * 0.04 * (1.0 - lift);
    let dir = vec2(angle.sin(), -angle.cos()); // from the point up along the pencil
    let across = vec2(-dir.y, dir.x);
    let w = side * 0.1;
    let seg = |from: f32, to: f32, wa: f32, wb: f32| {
        let (a, b) = (tip + dir * (from * side), tip + dir * (to * side));
        vec![a - across * wa / 2.0, a + across * wa / 2.0, b + across * wb / 2.0, b - across * wb / 2.0]
    };
    // A shadow on the page, then the lead, the wood, the body (with a shaded side), the band, the eraser.
    let shadow: Vec<Pos2> = seg(0.0, 0.50, 0.0, w).into_iter().map(|p| p + vec2(0.05, 0.05) * side).collect();
    painter.add(Shape::convex_polygon(shadow, Color32::from_black_alpha(40), Stroke::NONE));
    painter.add(Shape::convex_polygon(seg(0.0, 0.04, 0.0, w * 0.3), LEAD, Stroke::NONE));
    painter.add(Shape::convex_polygon(seg(0.04, 0.11, w * 0.3, w), WOOD, Stroke::NONE));
    painter.add(Shape::convex_polygon(seg(0.11, 0.39, w, w), PENCIL, Stroke::NONE));
    let shade: Vec<Pos2> = seg(0.11, 0.39, w * 0.34, w * 0.34).into_iter().map(|p| p + across * w * 0.33).collect();
    painter.add(Shape::convex_polygon(shade, PENCIL_SHADE, Stroke::NONE));
    painter.add(Shape::convex_polygon(seg(0.39, 0.44, w * 1.04, w * 1.04), BAND, Stroke::NONE));
    painter.add(Shape::convex_polygon(seg(0.44, 0.50, w, w), ERASER, Stroke::NONE));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pencil_draws_the_loop_then_it_rests_fades_and_starts_again() {
        assert_eq!(drawn_at(0.0), (0.0, 1.0), "it begins with nothing drawn");
        let (half, _) = drawn_at(DRAW / 2.0);
        assert!((half - 0.5).abs() < 0.01, "halfway through the time, halfway along: {half}");
        assert_eq!(drawn_at(DRAW + 1.0), (1.0, 1.0), "drawn, it rests");
        let (_, fading) = drawn_at(DRAW + REST + FADE / 2.0);
        assert!(fading > 0.3 && fading < 0.7, "then fades: {fading}");
        assert_eq!(drawn_at(ROUND + 0.0), drawn_at(0.0), "and starts again");

        // The point follows the loop, from its start at the lower left to its end at the upper right.
        let (start, end) = (tip_at(0.0), tip_at(1.0));
        assert!(start.x < 0.2 && start.y > 0.7 && end.x > 0.8 && end.y < 0.3, "{start:?} to {end:?}");
        let mid = tip_at(drawn_at(DRAW * 0.5).0);
        assert!(mid != start && mid != end);
    }
}
