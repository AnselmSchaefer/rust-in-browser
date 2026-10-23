//! Pictures: loading them into the document, their textures, and resizing.

use std::path::Path;

use eframe::egui::{self, Color32, ColorImage, Id, Pos2, Rect, TextureId, TextureOptions, Vec2, pos2, vec2};
use egui::epaint::{Mesh, Vertex};

use crate::App;
use crate::edit::{Edit, Piece};
use crate::model::{Align, IMAGE_CHAR, ImageData, ParaAttrs, is_terminator};

/// Largest side (in pixels) of the texture a picture is shown with.
const MAX_TEXTURE_SIDE: u32 = 2048;

/// Check that `bytes` is a picture we can show, and describe it. Other formats are not supported.
pub fn describe(bytes: &[u8]) -> Result<(String, (u32, u32)), String> {
    let format = match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => "png",
        Ok(image::ImageFormat::Jpeg) => "jpeg",
        _ => return Err("only PNG and JPEG pictures are supported".into()),
    };
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    Ok((format.to_owned(), (img.width(), img.height())))
}

fn texture_image(bytes: &[u8]) -> Option<ColorImage> {
    let mut img = image::load_from_memory(bytes).ok()?;
    if img.width().max(img.height()) > MAX_TEXTURE_SIDE {
        img = img.resize(MAX_TEXTURE_SIDE, MAX_TEXTURE_SIDE, image::imageops::FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

/// Texture coordinates for the point (`fx`, `fy`) of the on-screen rectangle (both 0..=1) when the
/// picture is turned clockwise by `turns` quarter turns.
pub fn rotated_uv(fx: f32, fy: f32, turns: u8) -> Pos2 {
    match turns % 4 {
        0 => pos2(fx, fy),
        1 => pos2(fy, 1.0 - fx),
        2 => pos2(1.0 - fx, 1.0 - fy),
        _ => pos2(1.0 - fy, fx),
    }
}

/// A textured rectangle showing a picture turned by `turns` quarter turns.
pub fn picture_mesh(tex: TextureId, rect: Rect, turns: u8, tint: Color32) -> Mesh {
    let mut mesh = Mesh::with_texture(tex);
    for (fx, fy) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let pos = pos2(rect.left() + fx * rect.width(), rect.top() + fy * rect.height());
        mesh.vertices.push(Vertex { pos, uv: rotated_uv(fx, fy, turns), color: tint });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    mesh
}

/// The bytes and file extension a picture gets in an exported document (turned pixels if rotated).
pub fn export_media(img: &ImageData) -> (Vec<u8>, &'static str) {
    let plain = (img.bytes.clone(), if img.format == "png" { "png" } else { "jpeg" });
    if img.rotation % 4 == 0 {
        return plain;
    }
    let Ok(decoded) = image::load_from_memory(&img.bytes) else { return plain };
    let turned = match img.rotation % 4 {
        1 => decoded.rotate90(),
        2 => decoded.rotate180(),
        _ => decoded.rotate270(),
    };
    let mut out = std::io::Cursor::new(Vec::new());
    match turned.write_to(&mut out, image::ImageFormat::Png) {
        Ok(()) => (out.into_inner(), "png"),
        Err(_) => plain,
    }
}

/// Which part of a selected picture a resize handle is: -1/0/1 along each side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Handle {
    pub kx: i8,
    pub ky: i8,
}

impl Handle {
    pub const ALL: [Handle; 8] = [
        Handle { kx: -1, ky: -1 },
        Handle { kx: 0, ky: -1 },
        Handle { kx: 1, ky: -1 },
        Handle { kx: 1, ky: 0 },
        Handle { kx: 1, ky: 1 },
        Handle { kx: 0, ky: 1 },
        Handle { kx: -1, ky: 1 },
        Handle { kx: -1, ky: 0 },
    ];

    pub fn pos(self, r: Rect) -> Pos2 {
        pos2(r.center().x + self.kx as f32 * r.width() / 2.0, r.center().y + self.ky as f32 * r.height() / 2.0)
    }

    pub fn cursor(self) -> egui::CursorIcon {
        match (self.kx, self.ky) {
            (0, _) => egui::CursorIcon::ResizeVertical,
            (_, 0) => egui::CursorIcon::ResizeHorizontal,
            (a, b) if a == b => egui::CursorIcon::ResizeNwSe,
            _ => egui::CursorIcon::ResizeNeSw,
        }
    }

    /// New width (in points) after dragging this handle by `drag` (in points) from `start_width`.
    /// The picture keeps its proportions, so a vertical drag counts via the aspect ratio.
    pub fn new_width(self, start_width: f32, aspect: f32, drag: Vec2) -> f32 {
        let along_x = self.kx as f32 * drag.x;
        let along_y = self.ky as f32 * drag.y / aspect.max(0.01);
        start_width + if along_x.abs() >= along_y.abs() { along_x } else { along_y }
    }
}

/// What the picture's context menu can do.
#[derive(Clone, Copy)]
pub enum PicAction {
    RotateRight,
    RotateLeft,
    Width(f32),
    OriginalSize,
    Align(Align),
    Delete,
}

/// Where a dragged picture is, and where it would land.
pub struct PicDrag {
    /// Flow position of the picture's placeholder character.
    pub from: usize,
    pub press: Pos2,
    /// Flow position of the paragraph start it would be dropped at, and the line to show there.
    pub drop: Option<(usize, f32)>,
}

/// A running resize by one of the handles.
pub struct ResizeDrag {
    pub id: u32,
    pub handle: Handle,
    pub start_width: f32,
    pub dragged: Vec2,
}

impl App {
    /// Make sure every picture of the document has a texture.
    pub fn ensure_textures(&mut self, ctx: &egui::Context) {
        for img in &self.doc.images {
            if !self.textures.contains_key(&img.id) {
                if let Some(ci) = texture_image(&img.bytes) {
                    let tex = ctx.load_texture(format!("picture-{}", img.id), ci, TextureOptions::LINEAR);
                    self.textures.insert(img.id, tex);
                }
            }
        }
    }

    /// Put the picture in the file at `path` into the text at the caret, in a paragraph of its own.
    pub fn insert_image(&mut self, ctx: &egui::Context, path: &Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        self.insert_image_bytes(ctx, bytes)
    }

    pub fn insert_image_bytes(&mut self, ctx: &egui::Context, bytes: Vec<u8>) -> Result<(), String> {
        let (format, px) = describe(&bytes)?;
        let id = self.doc.images.iter().map(|i| i.id).max().unwrap_or(0) + 1;
        // 96 dpi, but never wider than the writing area.
        let width_pt = (px.0 as f32 * 0.75).min(self.doc.setup.content_size().x);
        self.doc.images.push(ImageData { id, format, bytes, px, width_pt, rotation: 0 });
        self.ensure_textures(ctx);

        let (a, b) = self.selection();
        let (ps, _) = self.doc.para_start(a);
        let line = self.doc.para_attrs_at(a);
        let mark_attrs = ParaAttrs { list: crate::model::ListKind::None, ..line };
        let mut text = String::new();
        let mut styles = Vec::new();
        if ps != a {
            text.push('\n'); // the picture needs a paragraph of its own
            styles.push(self.typing.with_para(line));
        }
        text.push(IMAGE_CHAR);
        let mut pic = self.typing.with_para(ParaAttrs::default());
        pic.image = id;
        styles.push(pic);
        text.push('\n');
        styles.push(self.typing.with_para(mark_attrs));

        let max = self.doc.total_chars() - 1;
        let (a, b) = (a.min(max), b.min(max));
        let (ba, bb) = (self.doc.char_to_byte(a), self.doc.char_to_byte(b));
        let old = Piece { text: self.doc.flow.text[ba..bb].to_owned(), styles: self.doc.flow.styles[a..b].to_vec() };
        self.apply_edit(ctx, Edit::Replace { at: a, old, new: Piece { text, styles } }, b);
        Ok(())
    }

    /// Show a picture this wide (in points), keeping its proportions.
    pub fn set_image_width(&mut self, ctx: &egui::Context, id: u32, width_pt: f32) {
        let max = self.doc.setup.content_size().x;
        if let Some(img) = self.doc.images.iter_mut().find(|i| i.id == id) {
            img.width_pt = width_pt.clamp(16.0, max);
        }
        self.doc.version += 1;
        self.doc.full_paginate(ctx, &self.typing);
        ctx.request_repaint();
    }

    pub fn rotate_image(&mut self, ctx: &egui::Context, quarter_turns: i8) {
        let Some(id) = self.selected_image() else { return };
        if let Some(img) = self.doc.images.iter_mut().find(|i| i.id == id) {
            img.rotation = (img.rotation as i8 + quarter_turns).rem_euclid(4) as u8;
        }
        self.doc.version += 1;
        self.doc.full_paginate(ctx, &self.typing);
        ctx.request_repaint();
    }

    /// Is the selection exactly one picture? Then the range of its whole paragraph (picture and mark).
    pub fn picture_paragraph(&self) -> Option<(usize, usize)> {
        let (a, b) = self.selection();
        let is_pic = b == a + 1
            && self.doc.char_at(a) == Some(IMAGE_CHAR)
            && self.doc.flow.styles[a].image != 0
            && self.doc.char_at(b).is_some_and(is_terminator);
        is_pic.then_some((a, b + 1))
    }

    /// Move the picture paragraph starting at `from` so that it starts at the paragraph start `boundary`.
    pub fn move_picture(&mut self, ctx: &egui::Context, from: usize, boundary: usize) {
        let (to_end, max) = (from + 2, self.doc.total_chars() - 1);
        let valid = self.doc.char_at(from) == Some(IMAGE_CHAR) && self.doc.char_at(from + 1).is_some_and(is_terminator);
        if !valid || boundary == from || boundary == to_end || boundary > max {
            return;
        }
        let (bf, bt) = (self.doc.char_to_byte(from), self.doc.char_to_byte(to_end));
        let piece = Piece { text: self.doc.flow.text[bf..bt].to_owned(), styles: self.doc.flow.styles[from..to_end].to_vec() };
        let dest = if boundary > from { boundary - 2 } else { boundary };
        let remove = Edit::Replace { at: from, old: piece.clone(), new: Piece::default() };
        let insert = Edit::Replace { at: dest, old: Piece::default(), new: piece };
        self.doc.apply_group(vec![remove, insert]);
        self.doc.full_paginate(ctx, &self.typing);
        self.anchor = dest;
        self.set_caret(ctx, dest + 1, true);
    }

    /// Move the selected picture up or down by one paragraph (Alt+Up / Alt+Down).
    pub fn nudge_picture(&mut self, ctx: &egui::Context, up: bool) {
        let Some((from, _)) = self.picture_paragraph() else { return };
        let boundary = if up {
            if from == 0 {
                return;
            }
            self.doc.para_start(from - 1).0
        } else {
            let (tc, _) = self.doc.term_from(from + 2, self.doc.char_to_byte(from + 2));
            tc + 1
        };
        self.move_picture(ctx, from, boundary);
    }

    /// The right-click menu of a picture.
    pub fn picture_menu(&mut self, ctx: &egui::Context) {
        let Some(pos) = self.pic_menu else { return };
        if self.selected_image().is_none() {
            self.pic_menu = None;
            return;
        }
        let mut action = None;
        let area = egui::Area::new(Id::new("picture_menu")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_min_width(200.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                let mut item = |ui: &mut egui::Ui, label: &str, a: PicAction| {
                    if ui.add(egui::Button::new(label).frame(false).min_size(vec2(200.0, 24.0))).clicked() {
                        action = Some(a);
                    }
                };
                item(ui, "Rotate right 90\u{b0}", PicAction::RotateRight);
                item(ui, "Rotate left 90\u{b0}", PicAction::RotateLeft);
                ui.separator();
                item(ui, "Small  (25% of width)", PicAction::Width(0.25));
                item(ui, "Medium  (50%)", PicAction::Width(0.5));
                item(ui, "Large  (75%)", PicAction::Width(0.75));
                item(ui, "Full width", PicAction::Width(1.0));
                item(ui, "Original size", PicAction::OriginalSize);
                ui.separator();
                item(ui, "Align left", PicAction::Align(Align::Left));
                item(ui, "Center", PicAction::Align(Align::Center));
                item(ui, "Align right", PicAction::Align(Align::Right));
                ui.separator();
                item(ui, "Delete picture", PicAction::Delete);
            });
        });
        let fresh = std::mem::take(&mut self.pic_menu_fresh);
        let outside = !fresh && ctx.input(|i| i.pointer.any_pressed()) && !area.response.contains_pointer();
        if outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.pic_menu = None;
        }
        if let Some(a) = action {
            self.pic_menu = None;
            self.do_picture_action(ctx, a);
        }
    }

    pub fn do_picture_action(&mut self, ctx: &egui::Context, action: PicAction) {
        match action {
            PicAction::RotateRight => self.rotate_image(ctx, 1),
            PicAction::RotateLeft => self.rotate_image(ctx, -1),
            PicAction::Width(f) => self.set_image_width_fraction(ctx, f),
            PicAction::OriginalSize => {
                if let Some(id) = self.selected_image() {
                    if let Some(px) = self.doc.image(id).map(|i| i.px.0 as f32) {
                        self.set_image_width(ctx, id, px * 0.75);
                    }
                }
            }
            PicAction::Align(a) => self.set_para(ctx, |p| p.align = a),
            PicAction::Delete => {
                if let Some((a, b)) = self.picture_paragraph() {
                    self.replace_range(ctx, a, b, "");
                }
            }
        }
    }

    /// The picture the caret is on (the one in the caret's paragraph, if that paragraph is a picture).
    pub fn selected_image(&self) -> Option<u32> {
        let (ps, pb) = self.doc.para_start(self.caret);
        let (tc, _) = self.doc.term_from(ps, pb);
        (tc == ps + 1).then(|| self.doc.flow.styles[ps].image).filter(|&id| id != 0)
    }

    /// Show the selected picture at this share of the writing width.
    pub fn set_image_width_fraction(&mut self, ctx: &egui::Context, fraction: f32) {
        let Some(id) = self.selected_image() else { return };
        let full = self.doc.setup.content_size().x;
        if let Some(img) = self.doc.images.iter_mut().find(|i| i.id == id) {
            img.width_pt = full * fraction;
        }
        self.doc.version += 1;
        self.doc.full_paginate(ctx, &self.typing);
        let c = self.caret;
        self.set_caret(ctx, c, false);
    }
}

/// A small solid-colour PNG for tests.
#[cfg(test)]
pub fn test_png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_png_and_rejects_other_formats() {
        let (format, px) = describe(&test_png(40, 20, [200, 30, 30])).unwrap();
        assert_eq!((format.as_str(), px), ("png", (40, 20)));
        assert!(describe(b"GIF89a....").is_err());
        assert!(describe(b"not an image").is_err());
    }
}
