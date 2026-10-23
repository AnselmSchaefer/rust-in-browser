//! Export to Word's `.docx` (Office Open XML, ISO/IEC 29500), which Word, LibreOffice, Google Docs
//! and Pages all open. Mapping:
//!
//! * paragraphs = paragraphs of the flow; alignment, line spacing, bullets and numbering = paragraph properties
//! * hard page breaks = "page break before" on the paragraph that follows
//! * font, size, bold, italic, underline, colour = run properties
//! * page setup = section properties (paper, orientation, margins)
//! * header and footer = Word's header and footer parts, their fields (`{page}`, `{chapter}`) =
//!   `PAGE` and `STYLEREF "Heading 1"` fields; their looks = paragraph
//!   borders (Rule, Facing), shading (Banner), a "❦" under a border (Filigree), an ellipse shape
//!   holding the text (Circle). Facing pages = different odd and even headers (`evenAndOddHeaders`).
//! * page numbering changes = sections from the change's paragraph: `pgNumType` (start, roman), and
//!   for pages without numbers an empty part in place of a header or footer showing them
//! * text boxes = text box shapes anchored in the paragraph of the char they are pinned to, placed
//!   on the page and in front of the text (`wrapNone`), as they are in Caprice
//! * notes = Word comments anchored to the same text
//! * chapter titles = the built-in "Heading 1" style; drop caps = Word's own drop caps (a framed
//!   paragraph holding the letter); the contents = a `TOC` field over the headings, filled in with
//!   Caprice's pages and brought up to date by Word when the file is opened

use std::fmt::Write as _;
use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;

use crate::contents::Entry;
use eframe::egui;

use crate::model::{Align, Band, BandLook, CHAPTER_TITLE_SCALE, Doc, IMAGE_CHAR, ListKind, Note, PAGE_BREAK, PAGE_FIELD, ParaAttrs, Style, is_terminator};
use crate::numbers::{Counting, Figures, Numbering};

const NS_W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const NS_WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_PIC: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
const NS_WPS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";
const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const CT: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml";

/// Fonts that are only look-alikes of the usual Office fonts are written under the Office name,
/// so the document looks the same in Word (LibreOffice maps them back automatically).
fn office_font(name: &str) -> &str {
    match name {
        "Liberation Serif" | "Tinos" | "Nimbus Roman" => "Times New Roman",
        "Liberation Sans" | "Arimo" | "Nimbus Sans" => "Arial",
        "Liberation Mono" | "Cousine" | "Nimbus Mono PS" => "Courier New",
        "Carlito" => "Calibri",
        "Caladea" => "Cambria",
        "P052" => "Palatino Linotype",
        other => other,
    }
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {} // not allowed in XML
            c => out.push(c),
        }
    }
    out
}

fn twips(points: f32) -> i32 {
    (points * 20.0).round() as i32
}

fn run_props(st: &Style) -> String {
    let font = esc(office_font(&st.font));
    let half_points = (st.size * 2.0).round().max(2.0) as i32;
    let mut s = format!("<w:rFonts w:ascii=\"{font}\" w:hAnsi=\"{font}\" w:cs=\"{font}\"/>");
    if st.bold {
        s.push_str("<w:b/><w:bCs/>");
    }
    if st.italic {
        s.push_str("<w:i/><w:iCs/>");
    }
    if let Some([r, g, b]) = st.color {
        let _ = write!(s, "<w:color w:val=\"{r:02X}{g:02X}{b:02X}\"/>");
    }
    let _ = write!(s, "<w:sz w:val=\"{half_points}\"/><w:szCs w:val=\"{half_points}\"/>");
    if st.underline {
        s.push_str("<w:u w:val=\"single\"/>");
    }
    s
}

fn emu(points: f32) -> i64 {
    (points * 12700.0).round() as i64
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("{r:02X}{g:02X}{b:02X}")
}

/// The runs of a header's, footer's or text box's text, in run properties `rpr`: the text, and a
/// field for each `{page}` and `{chapter}`, its result filled in with `results`
/// until Word works it out.
fn field_runs(text: &str, rpr: &str, results: [&str; 2]) -> String {
    use crate::model::{CHAPTER_FIELD, PAGE_FIELD};
    let fields = [(PAGE_FIELD, " PAGE "), (CHAPTER_FIELD, " STYLEREF \"Heading 1\" ")];
    let plain = |t: &str| {
        // A line break within the text is a break in the run.
        let lines: Vec<String> = t.split('\n').map(|l| format!("<w:t xml:space=\"preserve\">{}</w:t>", esc(l))).collect();
        if t.is_empty() { String::new() } else { format!("<w:r><w:rPr>{rpr}</w:rPr>{}</w:r>", lines.join("<w:br/>")) }
    };
    let mut out = String::new();
    let mut rest = text;
    loop {
        let next = fields.iter().zip(results).filter_map(|((f, code), result)| rest.find(f).map(|at| (at, *f, *code, result))).min_by_key(|x| x.0);
        let Some((at, f, code, result)) = next else {
            out.push_str(&plain(rest));
            return out;
        };
        out.push_str(&plain(&rest[..at]));
        let run = |inner: String| format!("<w:r><w:rPr>{rpr}</w:rPr>{inner}</w:r>");
        out.push_str(&run("<w:fldChar w:fldCharType=\"begin\"/>".into()));
        out.push_str(&run(format!("<w:instrText xml:space=\"preserve\">{}</w:instrText>", esc(code))));
        out.push_str(&run("<w:fldChar w:fldCharType=\"separate\"/>".into()));
        out.push_str(&run(format!("<w:t xml:space=\"preserve\">{}</w:t>", esc(result))));
        out.push_str(&run("<w:fldChar w:fldCharType=\"end\"/>".into()));
        rest = &rest[at + f.len()..];
    }
}

/// How a shape is drawn: its outline (`rect`, `ellipse`), fill and how opaque it is (0 to 1), line
/// (width in points), the room inside it around its text, in points (left, top, right, bottom),
/// and whether it lies behind the text rather than in front.
struct ShapeLook<'a> {
    geom: &'a str,
    fill: Option<([u8; 3], f32)>,
    line: Option<(f32, [u8; 3])>,
    inset: [f32; 4],
    behind: bool,
}

/// A shape holding `paragraphs`, placed at `rect` (points from the page's top-left) in front of
/// the text or behind it, which does not wrap around it. Shapes later in the document lie on top.
/// Word 2010 and later, and LibreOffice, read it.
fn anchored_shape(id: u32, rect: egui::Rect, look: &ShapeLook, paragraphs: &str) -> String {
    let (cx, cy) = (emu(rect.width()), emu(rect.height()));
    let fill = look.fill.map_or("<a:noFill/>".to_owned(), |(c, a)| {
        let alpha = if a < 1.0 { format!("<a:alpha val=\"{}\"/>", (a * 100_000.0).round()) } else { String::new() };
        format!("<a:solidFill><a:srgbClr val=\"{}\">{alpha}</a:srgbClr></a:solidFill>", hex(c))
    });
    let line = look.line.map_or("<a:ln><a:noFill/></a:ln>".to_owned(), |(w, c)| {
        format!("<a:ln w=\"{}\"><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill></a:ln>", emu(w), hex(c))
    });
    let [l, t, r, b] = look.inset.map(emu);
    format!(
        "<w:r><w:drawing><wp:anchor distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\" simplePos=\"0\" relativeHeight=\"{}\" behindDoc=\"{}\" \
locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\"><wp:simplePos x=\"0\" y=\"0\"/>\
<wp:positionH relativeFrom=\"page\"><wp:posOffset>{}</wp:posOffset></wp:positionH>\
<wp:positionV relativeFrom=\"page\"><wp:posOffset>{}</wp:posOffset></wp:positionV>\
<wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/><wp:wrapNone/>\
<wp:docPr id=\"{id}\" name=\"Text Box {id}\"/><wp:cNvGraphicFramePr/>\
<a:graphic><a:graphicData uri=\"{NS_WPS}\"><wps:wsp><wps:cNvSpPr txBox=\"1\"/>\
<wps:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"{}\"><a:avLst/></a:prstGeom>{fill}{line}</wps:spPr>\
<wps:txbx><w:txbxContent>{paragraphs}</w:txbxContent></wps:txbx>\
<wps:bodyPr rot=\"0\" vert=\"horz\" wrap=\"square\" lIns=\"{l}\" tIns=\"{t}\" rIns=\"{r}\" bIns=\"{b}\" anchor=\"{}\" anchorCtr=\"0\"><a:noAutofit/></wps:bodyPr>\
</wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r>",
        251_659_264 + id,
        u8::from(look.behind),
        emu(rect.left()),
        emu(rect.top()),
        look.geom,
        if look.geom == "ellipse" { "ctr" } else { "t" },
    )
}

/// A text box as Word's: a shape with its look, its text in paragraphs of its style.
fn text_box_xml(b: &crate::model::TextBox, id: u32) -> String {
    use crate::model::BoxLook;
    let st = crate::textbox::shown_style(b);
    let rpr = run_props(&st);
    let jc = match b.style.para.align {
        Align::Left => "",
        Align::Center => "<w:jc w:val=\"center\"/>",
        Align::Right => "<w:jc w:val=\"right\"/>",
        Align::Justify => "<w:jc w:val=\"both\"/>",
    };
    let lines: Vec<&str> = b.text.split('\n').collect();
    let rule = |side: &str| format!("<w:{side} w:val=\"single\" w:sz=\"8\" w:space=\"4\" w:color=\"{}\"/>", hex(b.color));
    let mut paragraphs = String::new();
    for (k, line) in lines.iter().enumerate() {
        // A quote's rules are the borders of its first and last paragraph; a sidebar's bar runs
        // down the left of every one.
        let border = match b.look {
            BoxLook::Quote => [(k == 0).then(|| rule("top")), (k + 1 == lines.len()).then(|| rule("bottom"))].into_iter().flatten().collect(),
            BoxLook::Sidebar => format!("<w:left w:val=\"single\" w:sz=\"{}\" w:space=\"6\" w:color=\"{}\"/>", 32, hex(b.color)),
            _ => String::new(),
        };
        let border = if border.is_empty() { border } else { format!("<w:pBdr>{border}</w:pBdr>") };
        let _ = write!(paragraphs, "<w:p><w:pPr>{border}{jc}<w:rPr>{rpr}</w:rPr></w:pPr>{}</w:p>", field_runs(line, &rpr, ["1", ""]));
    }
    // A tint is the colour mixed into white, or, transparent, the colour that strong over what is under it.
    let tint = |t: f32| {
        if b.clear { (b.color, t) } else { (b.color.map(|c| (255.0 - (255.0 - f32::from(c)) * t).round() as u8), 1.0) }
    };
    let (fill, line, inset) = match b.look {
        BoxLook::Simple => ((!b.clear).then_some(([255, 255, 255], 1.0)), Some((0.75, b.color)), [7.0, 5.0, 7.0, 5.0]),
        BoxLook::Shaded => (Some(tint(0.16)), None, [7.0, 5.0, 7.0, 5.0]),
        BoxLook::Sidebar => (Some(tint(0.07)), None, [2.0, 5.0, 7.0, 5.0]),
        BoxLook::Quote | BoxLook::Bare => (None, None, [7.0, 5.0, 7.0, 5.0]),
    };
    let look = ShapeLook { geom: "rect", fill, line, inset, behind: b.behind };
    // Word grows the shape to hold its text (`spAutoFit`), as Caprice does.
    let rect = egui::Rect::from_min_size(b.pos.to_pos2(), b.size);
    anchored_shape(id, rect, &look, &paragraphs).replace("<a:noAutofit/>", "<a:spAutoFit/>")
}

/// A header (`top`) or footer part showing `band`; `even` for the even pages of facing pages.
/// Its text is in `font`.
fn band_xml(band: &Band, top: bool, even: bool, font: &str, setup: &crate::model::PageSetup) -> String {
    use crate::bands::BAND_SIZE;
    let on_colour = matches!(band.look, BandLook::Banner | BandLook::Circle);
    let st = Style {
        size: BAND_SIZE,
        color: Some(if on_colour { [255, 255, 255] } else { [96, 96, 96] }),
        bold: band.look == BandLook::Circle,
        ..Style::new(font)
    };
    let rpr = run_props(&st);
    let runs = field_runs(&band.text, &rpr, ["1", ""]);
    let color = hex(band.color);
    let side = if top { "bottom" } else { "top" };
    let rule = format!("<w:pBdr><w:{side} w:val=\"single\" w:sz=\"4\" w:space=\"4\" w:color=\"{color}\"/></w:pBdr>");
    let centre = "<w:jc w:val=\"center\"/>";
    let para = |ppr: &str, runs: &str| format!("<w:p><w:pPr>{ppr}</w:pPr>{runs}</w:p>");
    let body = match band.look {
        BandLook::None | BandLook::Plain => para(centre, &runs),
        BandLook::Rule => para(&format!("{rule}{centre}"), &runs),
        BandLook::Filigree => {
            let inset = twips(setup.content_size().x * 0.1);
            let text = para(&format!("{rule}<w:ind w:left=\"{inset}\" w:right=\"{inset}\"/>{centre}"), &runs);
            let fleuron = Style { font: "Segoe UI Symbol".into(), size: 12.0, color: Some(band.color), bold: false, ..st.clone() };
            let fleuron = para(centre, &format!("<w:r><w:rPr>{}</w:rPr><w:t>\u{2766}</w:t></w:r>", run_props(&fleuron)));
            // It hangs from the line, on the side of the text.
            if top { text + &fleuron } else { fleuron + &text }
        }
        BandLook::Banner => para(&format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{color}\"/>{centre}"), &runs),
        BandLook::Facing => para(&format!("{rule}<w:jc w:val=\"{}\"/>", if even { "left" } else { "right" }), &runs),
        BandLook::Circle => {
            let size = setup.size();
            let cy = if top { setup.margin_top / 2.0 } else { size.y - setup.margin_bottom / 2.0 };
            let mid = (setup.margin_left + size.x - setup.margin_right) / 2.0;
            // Word works out the text's width itself: the circle is made for two or three digits.
            let shown = crate::bands::filled(&band.text, "99", "");
            let r = (shown.chars().count() as f32 * BAND_SIZE * 0.3).max(BAND_SIZE * 0.6) + 5.0;
            let rect = egui::Rect::from_center_size(egui::pos2(mid, cy), egui::vec2(2.0 * r, 2.0 * r));
            let look = ShapeLook { geom: "ellipse", fill: Some((band.color, 1.0)), line: None, inset: [0.0; 4], behind: false };
            para("", &anchored_shape(3000 + u32::from(top), rect, &look, &para(centre, &runs)))
        }
    };
    let tag = if top { "hdr" } else { "ftr" };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:{tag} xmlns:w=\"{NS_W}\" xmlns:r=\"{NS_R}\" xmlns:wp=\"{NS_WP}\" xmlns:a=\"{NS_A}\" xmlns:wps=\"{NS_WPS}\" xmlns:mc=\"{NS_MC}\" mc:Ignorable=\"wps\">{body}</w:{tag}>"
    )
}

/// Builds `<w:body>` content. A paragraph's properties are only known at its end, so its runs are
/// collected first.
struct Body {
    /// Finished paragraphs.
    xml: String,
    /// Runs and comment markers of the open paragraph.
    para: String,
    run: Option<(Style, String)>,
    /// Does the open paragraph have anything in it (text, a tab or a comment marker)?
    has_content: bool,
    /// Is the next paragraph the first on its page?
    at_page_start: bool,
    /// A hard page break came just before.
    page_break_before: bool,
    last_list: ListKind,
    /// How many separate numbered lists there have been (each restarts at 1).
    numbered_lists: u32,
    /// Pictures: id -> (width, height) in EMU (1 pt = 12700 EMU), and the ids used.
    pics: std::collections::HashMap<u32, (i64, i64, String)>,
    used_pics: Vec<u32>,
    /// Is the open paragraph a chapter title (whose text is drawn larger and bold)?
    title: bool,
    /// Where the properties of the last paragraph written end in `xml`: a section ends there.
    last_ppr_end: Option<usize>,
}

impl Body {
    fn new() -> Self {
        Self {
            xml: String::new(),
            para: String::new(),
            run: None,
            has_content: false,
            at_page_start: true,
            page_break_before: false,
            last_list: ListKind::None,
            numbered_lists: 0,
            pics: Default::default(),
            used_pics: Vec::new(),
            title: false,
            last_ppr_end: None,
        }
    }

    /// How text of style `st` looks in the open paragraph.
    fn shown(&self, st: &Style) -> Style {
        if self.title { Style { size: st.size * CHAPTER_TITLE_SCALE, bold: true, ..st.clone() } } else { st.clone() }
    }

    /// Properties that only the first paragraph after a page break gets.
    fn take_page_break(&mut self) -> &'static str {
        self.at_page_start = false;
        if std::mem::take(&mut self.page_break_before) { "<w:pageBreakBefore/>" } else { "" }
    }

    /// A drop cap: Word wants the letter in a paragraph of its own, framed `lines` deep into the
    /// paragraph that follows. Its size is the one Word itself picks for a drop cap.
    fn push_drop_cap(&mut self, c: char, st: &Style, lines: u8, spacing: f32) {
        self.flush_run();
        let cap = Style { size: st.size * f32::from(lines) * 1.15 * spacing, ..st.clone() };
        let page_break = self.take_page_break();
        let ppr = format!(
            "<w:keepNext/>{page_break}<w:framePr w:dropCap=\"drop\" w:lines=\"{lines}\" w:wrap=\"around\" w:vAnchor=\"text\" w:hAnchor=\"text\"/>\
<w:spacing w:line=\"{}\" w:lineRule=\"exact\"/><w:textAlignment w:val=\"baseline\"/>",
            twips(cap.size)
        );
        let _ = write!(
            self.xml,
            "<w:p><w:pPr>{ppr}</w:pPr>{}<w:r><w:rPr>{}</w:rPr><w:t>{}</w:t></w:r></w:p>",
            std::mem::take(&mut self.para),
            run_props(&cap),
            esc(&c.to_string())
        );
        self.has_content = false;
    }

    /// The contents, before the story: a heading, then a `TOC` field over the chapter titles,
    /// filled in with `entries` until Word updates it. `tab` is where the page numbers end, in
    /// twips. The story starts on a new page after them.
    fn push_contents(&mut self, entries: &[Entry], st: &Style, tab: i32) {
        let heading = Style { size: st.size * CHAPTER_TITLE_SCALE, bold: true, ..st.clone() };
        let _ = write!(
            self.xml,
            "<w:p><w:pPr><w:spacing w:after=\"{}\"/><w:jc w:val=\"center\"/></w:pPr><w:r><w:rPr>{}</w:rPr><w:t>Contents</w:t></w:r></w:p>",
            twips(st.size * 1.2),
            run_props(&heading)
        );
        let props = run_props(st);
        let field = |kind: &str| format!("<w:r><w:rPr>{props}</w:rPr><w:fldChar w:fldCharType=\"{kind}\"/></w:r>");
        let begin = format!(
            "{}<w:r><w:rPr>{props}</w:rPr><w:instrText xml:space=\"preserve\"> TOC \\o \"1-1\" \\h \\z </w:instrText></w:r>{}",
            field("begin"),
            field("separate")
        );
        let ppr = format!("<w:pStyle w:val=\"TOC1\"/><w:tabs><w:tab w:val=\"right\" w:leader=\"dot\" w:pos=\"{tab}\"/></w:tabs>");
        let text = |t: &str| format!("<w:r><w:rPr>{props}</w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>", esc(t));
        if entries.is_empty() {
            let _ = write!(self.xml, "<w:p><w:pPr>{ppr}</w:pPr>{begin}{}</w:p>", field("end"));
        }
        for (k, e) in entries.iter().enumerate() {
            let mut runs = if k == 0 { begin.clone() } else { String::new() };
            runs.push_str(&text(&e.title));
            if e.page.is_some() {
                let _ = write!(runs, "<w:r><w:rPr>{props}</w:rPr><w:tab/></w:r>{}", text(&e.number));
            }
            if k + 1 == entries.len() {
                runs.push_str(&field("end"));
            }
            let _ = write!(self.xml, "<w:p><w:pPr>{ppr}</w:pPr>{runs}</w:p>");
        }
        self.page_break_before = true;
    }

    /// End a section with the last paragraph written (`sect` is its `sectPr`), or with an empty
    /// one if there is none. The contents always get an empty one: Word rewrites a `TOC` field's
    /// paragraphs when it updates it, and would take the section's end with them.
    fn end_section(&mut self, sect: &str, own_paragraph: bool) {
        match self.last_ppr_end.filter(|_| !own_paragraph) {
            Some(at) => self.xml.insert_str(at, sect),
            None => {
                let _ = write!(self.xml, "<w:p><w:pPr>{sect}</w:pPr></w:p>");
            }
        }
        self.last_ppr_end = None;
    }

    fn flush_run(&mut self) {
        if let Some((st, text)) = self.run.take() {
            let _ = write!(
                self.para,
                "<w:r><w:rPr>{}</w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>",
                run_props(&self.shown(&st)),
                esc(&text)
            );
            self.has_content = true;
        }
    }

    fn push_picture(&mut self, id: u32) {
        let Some((cx, cy, ext)) = self.pics.get(&id).cloned() else { return };
        self.flush_run();
        if !self.used_pics.contains(&id) {
            self.used_pics.push(id);
        }
        let n = self.used_pics.iter().position(|&p| p == id).unwrap_or(0) + 1;
        let _ = write!(
            self.para,
            "<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
<wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{n}\" name=\"Picture {n}\"/>\
<wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect=\"1\"/></wp:cNvGraphicFramePr>\
<a:graphic><a:graphicData uri=\"{NS_PIC}\"><pic:pic>\
<pic:nvPicPr><pic:cNvPr id=\"{n}\" name=\"image{id}.{ext}\"/><pic:cNvPicPr/></pic:nvPicPr>\
<pic:blipFill><a:blip r:embed=\"rIdImg{id}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
<pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>\
</wp:inline></w:drawing></w:r>"
        );
        self.has_content = true;
    }

    /// A shape anchored in the open paragraph (a text box).
    fn push_shape(&mut self, xml: &str) {
        self.flush_run();
        self.para.push_str(xml);
        self.has_content = true;
    }

    fn push_char(&mut self, c: char, st: &Style) {
        if c == IMAGE_CHAR && st.image != 0 {
            return self.push_picture(st.image);
        }
        match c {
            '\t' => {
                self.flush_run();
                let _ = write!(self.para, "<w:r><w:rPr>{}</w:rPr><w:tab/></w:r>", run_props(&self.shown(st)));
                self.has_content = true;
            }
            c => match &mut self.run {
                Some((cur, text)) if cur.same_char(st) => text.push(c),
                _ => {
                    self.flush_run();
                    self.run = Some((st.clone(), c.to_string()));
                }
            },
        }
    }

    fn comment_start(&mut self, id: usize) {
        self.flush_run();
        let _ = write!(self.para, "<w:commentRangeStart w:id=\"{id}\"/>");
        self.has_content = true;
    }

    fn comment_end(&mut self, id: usize) {
        self.flush_run();
        let _ = write!(
            self.para,
            "<w:commentRangeEnd w:id=\"{id}\"/><w:r><w:commentReference w:id=\"{id}\"/></w:r>"
        );
        self.has_content = true;
    }

    /// The paragraph ends with `term` (`\n`, or a page break). `\n` always makes a paragraph; a page
    /// break only does if it has text or it is what holds an otherwise empty page.
    fn end_paragraph(&mut self, term: char, st: &Style) {
        self.flush_run();
        let is_break = term == PAGE_BREAK;
        if !is_break || self.has_content || self.at_page_start {
            self.emit(st.para, st);
        }
        if is_break {
            self.page_break_before = true;
            self.at_page_start = true;
        }
    }

    fn emit(&mut self, attrs: ParaAttrs, term: &Style) {
        let mut ppr = String::new();
        if attrs.is_chapter_title() {
            ppr.push_str("<w:pStyle w:val=\"Heading1\"/>");
        }
        if self.page_break_before {
            ppr.push_str("<w:pageBreakBefore/>");
        }
        match attrs.list {
            ListKind::None => {}
            kind => {
                if kind == ListKind::Numbered && self.last_list != ListKind::Numbered {
                    self.numbered_lists += 1; // a new list: numbering starts over
                }
                let num_id = if kind == ListKind::Bullet { 1 } else { 1 + self.numbered_lists };
                let _ = write!(ppr, "<w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"{num_id}\"/></w:numPr>");
            }
        }
        if (attrs.spacing - 1.0).abs() > 0.001 {
            let _ = write!(ppr, "<w:spacing w:line=\"{}\" w:lineRule=\"auto\"/>", (attrs.spacing * 240.0).round() as i32);
        }
        if attrs.list != ListKind::None {
            ppr.push_str("<w:ind w:left=\"560\" w:hanging=\"360\"/>");
        }
        match attrs.align {
            Align::Left => {}
            Align::Center => ppr.push_str("<w:jc w:val=\"center\"/>"),
            Align::Right => ppr.push_str("<w:jc w:val=\"right\"/>"),
            Align::Justify => ppr.push_str("<w:jc w:val=\"both\"/>"),
        }
        // The paragraph mark's own size keeps empty lines the right height.
        let _ = write!(ppr, "<w:rPr>{}</w:rPr>", run_props(&self.shown(term)));
        self.last_ppr_end = Some(self.xml.len() + "<w:p><w:pPr>".len() + ppr.len());
        let _ = write!(self.xml, "<w:p><w:pPr>{ppr}</w:pPr>{}</w:p>", std::mem::take(&mut self.para));
        self.has_content = false;
        self.at_page_start = false;
        self.page_break_before = false;
        self.last_list = attrs.list;
    }
}

/// The body XML, how many separate numbered lists it uses, and the pictures it refers to.
/// `sect` gives a section's properties from its page numbering and whether it goes on from the
/// page before (continuous) rather than starting a page.
fn document_xml(doc: &Doc, sect: &dyn Fn(Numbering, bool) -> String, media: &std::collections::HashMap<u32, (Vec<u8>, &'static str)>) -> (String, u32, Vec<u32>) {
    let mut body = Body::new();
    for img in &doc.images {
        let size = doc.image_size(img);
        let ext = media.get(&img.id).map_or("png", |m| m.1);
        body.pics.insert(img.id, ((size.x * 12700.0) as i64, (size.y * 12700.0) as i64, ext.to_owned()));
    }
    let notes: &[Note] = &doc.notes;
    let fallback = Style::new("Times New Roman");
    let mut chars = doc.flow.text.char_indices().zip(doc.flow.styles.iter().chain(std::iter::repeat(&fallback)));
    // Each change of page numbering starts a section at its paragraph (`numbers.rs`); the last
    // change pinned in a paragraph counts.
    let mut changes: Vec<(usize, Numbering)> = Vec::new();
    for m in &doc.number_marks {
        let p = doc.para_start(m.at.min(doc.total_chars().saturating_sub(1))).0;
        match changes.last_mut() {
            Some(last) if last.0 == p => last.1 = m.numbering,
            _ => changes.push((p, m.numbering)),
        }
    }
    let mut changes = changes.into_iter().peekable();
    // The section being written: its numbering, and whether it went on from the page before.
    let mut section = (Numbering::default(), false);
    if doc.setup.contents {
        body.push_contents(&doc.chapters(true), &doc.contents_style(), twips(doc.setup.content_size().x));
        if !doc.setup.contents_numbers.is_usual() || !doc.number_marks.is_empty() {
            body.end_section(&sect(doc.setup.contents_numbers, false), true);
            body.page_break_before = false; // the story's section starts the page
        }
    }
    // A change at the story's start is the numbering of its first section, with no break before.
    if let Some((_, n)) = changes.next_if(|&(p, _)| p == 0) {
        section.0 = n;
    }
    let mut para_start = true;

    let n = doc.total_chars();
    for k in 0..=n {
        for (j, b) in doc.boxes.iter().enumerate().filter(|(_, b)| b.at == k) {
            body.push_shape(&text_box_xml(b, 1000 + j as u32));
        }
        for (id, note) in notes.iter().enumerate() {
            if note.start == k {
                body.comment_start(id);
            }
        }
        for (id, note) in notes.iter().enumerate() {
            if note.end == k {
                body.comment_end(id);
            }
        }
        if let Some(((b, c), st)) = chars.next() {
            if let Some((_, n)) = changes.next_if(|&(p, _)| para_start && p == k) {
                body.flush_run();
                body.end_section(&sect(section.0, section.1), false);
                // After a hard break the section starts the page; elsewhere it goes on from it.
                section = (n, !std::mem::take(&mut body.page_break_before));
            }
            // At a paragraph start, look at how the paragraph is formatted (that is on its mark).
            let cap = para_start.then(|| {
                let (tc, _) = doc.term_from(k, b);
                let attrs = doc.flow.styles[tc].para;
                body.title = attrs.is_chapter_title();
                doc.has_drop_cap(k, b).then_some(attrs.spacing)
            });
            if let Some(Some(spacing)) = cap {
                body.push_drop_cap(c, st, doc.setup.drop_cap_lines, spacing);
            } else if is_terminator(c) {
                body.end_paragraph(c, st);
            } else {
                body.push_char(c, st);
            }
            para_start = is_terminator(c);
        }
    }
    body.flush_run();
    if body.has_content || !body.para.is_empty() {
        body.emit(ParaAttrs::default(), &fallback); // markers after the final paragraph mark
    }

    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:document xmlns:w=\"{NS_W}\" xmlns:r=\"{NS_R}\" xmlns:wp=\"{NS_WP}\" xmlns:a=\"{NS_A}\" xmlns:pic=\"{NS_PIC}\" \
xmlns:wps=\"{NS_WPS}\" xmlns:mc=\"{NS_MC}\" mc:Ignorable=\"wps\"><w:body>{}{}</w:body></w:document>",
        body.xml,
        sect(section.0, section.1),
    );
    (xml, body.numbered_lists, body.used_pics)
}

/// A section's properties: the page setup, headers and footers (`refs`), how it starts and how
/// its pages are numbered.
fn sect_xml(s: &crate::model::PageSetup, refs: &str, numbering: Numbering, continuous: bool) -> String {
    let size = s.size();
    let orient = if size.x > size.y { " w:orient=\"landscape\"" } else { "" };
    let kind = if continuous { "<w:type w:val=\"continuous\"/>" } else { "" };
    let start = match numbering.counting {
        Counting::From(k) => format!(" w:start=\"{k}\""),
        _ => String::new(),
    };
    let fmt = if numbering.figures == Figures::Roman { " w:fmt=\"lowerRoman\"" } else { "" };
    let numbers = if start.is_empty() && fmt.is_empty() { String::new() } else { format!("<w:pgNumType{fmt}{start}/>") };
    format!(
        "<w:sectPr>{refs}{kind}<w:pgSz w:w=\"{}\" w:h=\"{}\"{orient}/>\
<w:pgMar w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" w:left=\"{}\" w:header=\"{}\" w:footer=\"{}\" w:gutter=\"0\"/>{numbers}</w:sectPr>",
        twips(size.x),
        twips(size.y),
        twips(s.margin_top),
        twips(s.margin_right),
        twips(s.margin_bottom),
        twips(s.margin_left),
        twips(s.margin_top / 2.0),
        twips(s.margin_bottom / 2.0),
    )
}

/// Bullet and numbering definitions: one bullet list, and one numbered list per run of numbered paragraphs.
fn numbering_xml(numbered_lists: u32) -> String {
    let level = |fmt: &str, text: &str| {
        format!(
            "<w:lvl w:ilvl=\"0\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{fmt}\"/><w:lvlText w:val=\"{text}\"/>\
<w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"560\" w:hanging=\"360\"/></w:pPr></w:lvl>"
        )
    };
    let mut s = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:numbering xmlns:w=\"{NS_W}\">");
    let _ = write!(s, "<w:abstractNum w:abstractNumId=\"0\"><w:multiLevelType w:val=\"singleLevel\"/>{}</w:abstractNum>", level("bullet", "\u{2022}"));
    let _ = write!(s, "<w:abstractNum w:abstractNumId=\"1\"><w:multiLevelType w:val=\"singleLevel\"/>{}</w:abstractNum>", level("decimal", "%1."));
    s.push_str("<w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>");
    for k in 0..numbered_lists {
        let _ = write!(
            s,
            "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"1\"/><w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"1\"/></w:lvlOverride></w:num>",
            k + 2
        );
    }
    s.push_str("</w:numbering>");
    s
}

/// The default look, plus Word's built-in "heading 1" (chapter titles, which the contents list) and
/// "toc 1" (lines of the contents, with dots leading to the page number at `tab` twips).
fn styles_xml(default: &Style, tab: i32) -> String {
    let font = esc(office_font(&default.font));
    let half_points = (default.size * 2.0).round() as i32;
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:styles xmlns:w=\"{NS_W}\"><w:docDefaults><w:rPrDefault><w:rPr>\
<w:rFonts w:ascii=\"{font}\" w:hAnsi=\"{font}\" w:cs=\"{font}\"/>\
<w:sz w:val=\"{half_points}\"/><w:szCs w:val=\"{half_points}\"/></w:rPr></w:rPrDefault>\
<w:pPrDefault><w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault>\
</w:docDefaults><w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/>\
<w:uiPriority w:val=\"9\"/><w:qFormat/><w:pPr><w:keepNext/><w:outlineLvl w:val=\"0\"/></w:pPr><w:rPr><w:b/><w:bCs/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"TOC1\"><w:name w:val=\"toc 1\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/>\
<w:uiPriority w:val=\"39\"/><w:pPr><w:tabs><w:tab w:val=\"right\" w:leader=\"dot\" w:pos=\"{tab}\"/></w:tabs><w:spacing w:after=\"100\"/></w:pPr></w:style>\
</w:styles>"
    )
}

/// Current UTC time as `2026-10-02T17:00:00Z` (days-to-date after Howard Hinnant's algorithm).
fn timestamp_now() -> String {
    let secs = crate::web::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    iso8601(secs)
}

fn iso8601(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn comments_xml(notes: &[Note]) -> String {
    let now = timestamp_now();
    let mut s = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:comments xmlns:w=\"{NS_W}\">");
    for (id, note) in notes.iter().enumerate() {
        let _ = write!(s, "<w:comment w:id=\"{id}\" w:author=\"Caprice\" w:date=\"{now}\" w:initials=\"C\">");
        for line in note.text.split('\n') {
            let _ = write!(s, "<w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>", esc(line));
        }
        s.push_str("</w:comment>");
    }
    s.push_str("</w:comments>");
    s
}

/// The whole `.docx` file as bytes.
pub fn to_docx(doc: &Doc) -> Result<Vec<u8>, String> {
    // Each header and footer: its part, its id, and which pages it is for.
    let font = doc.contents_style().font;
    let mut bands: Vec<(String, String, &str, String)> = Vec::new();
    for (band, top) in [(&doc.setup.header, true), (&doc.setup.footer, false)].into_iter().filter(|(b, _)| b.is_on()) {
        let kind = if top { "header" } else { "footer" };
        bands.push((format!("{kind}1.xml"), format!("rId{kind}"), "default", band_xml(band, top, false, &font, &doc.setup)));
        if band.look == BandLook::Facing {
            bands.push((format!("{kind}2.xml"), format!("rId{kind}Even"), "even", band_xml(band, top, true, &font, &doc.setup)));
        }
    }
    let facing = [&doc.setup.header, &doc.setup.footer].iter().any(|b| b.look == BandLook::Facing);
    // On pages without numbers, a band showing them is an empty part (none would carry on the
    // section's before).
    let numbered = |top: bool| if top { &doc.setup.header } else { &doc.setup.footer }.text.contains(PAGE_FIELD);
    let any_off = doc.number_marks.iter().any(|m| m.numbering.counting == Counting::Off)
        || (doc.setup.contents && doc.setup.contents_numbers.counting == Counting::Off);
    let refs = |off: bool| -> String {
        bands
            .iter()
            .map(|(file, id, pages, _)| {
                let kind = if file.starts_with("header") { "header" } else { "footer" };
                let id = if off && numbered(kind == "header") { format!("rId{kind}Blank") } else { id.clone() };
                format!("<w:{kind}Reference w:type=\"{pages}\" r:id=\"{id}\"/>")
            })
            .collect()
    };
    let (on, off) = (refs(false), refs(true));
    for top in [true, false].into_iter().filter(|&top| any_off && numbered(top) && if top { &doc.setup.header } else { &doc.setup.footer }.is_on()) {
        let (kind, tag) = if top { ("header", "hdr") } else { ("footer", "ftr") };
        let blank = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:{tag} xmlns:w=\"{NS_W}\"><w:p/></w:{tag}>"
        );
        bands.push((format!("{kind}3.xml"), format!("rId{kind}Blank"), "", blank));
    }
    let sect = |n: Numbering, continuous: bool| sect_xml(&doc.setup, if n.counting == Counting::Off { &off } else { &on }, n, continuous);
    let has_comments = !doc.notes.is_empty();
    let default = doc.flow.styles.first().cloned().unwrap_or_else(|| Style::new("Times New Roman"));
    // Rotated pictures are written with their pixels turned.
    let media: std::collections::HashMap<u32, (Vec<u8>, &'static str)> =
        doc.images.iter().map(|i| (i.id, crate::images::export_media(i))).collect();
    let (document, numbered_lists, used_pics) = document_xml(doc, &sect, &media);
    let has_numbering = document.contains("<w:numPr>");

    let mut content_types = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"{CT}.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"{CT}.styles+xml\"/>"
    );
    let mut rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rIdStyles\" Type=\"{REL}/styles\" Target=\"styles.xml\"/>"
    );
    if has_comments {
        let _ = write!(content_types, "<Override PartName=\"/word/comments.xml\" ContentType=\"{CT}.comments+xml\"/>");
        let _ = write!(rels, "<Relationship Id=\"rIdComments\" Type=\"{REL}/comments\" Target=\"comments.xml\"/>");
    }
    let pic_ext = |id: u32| -> &'static str { media.get(&id).map_or("png", |m| m.1) };
    if used_pics.iter().any(|&id| pic_ext(id) == "png") {
        content_types.push_str("<Default Extension=\"png\" ContentType=\"image/png\"/>");
    }
    if used_pics.iter().any(|&id| pic_ext(id) == "jpeg") {
        content_types.push_str("<Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>");
    }
    for &id in &used_pics {
        let _ = write!(rels, "<Relationship Id=\"rIdImg{id}\" Type=\"{REL}/image\" Target=\"media/image{id}.{}\"/>", pic_ext(id));
    }
    if has_numbering {
        let _ = write!(content_types, "<Override PartName=\"/word/numbering.xml\" ContentType=\"{CT}.numbering+xml\"/>");
        let _ = write!(rels, "<Relationship Id=\"rIdNumbering\" Type=\"{REL}/numbering\" Target=\"numbering.xml\"/>");
    }
    // Word fills in the contents' page numbers by its own layout when it opens the file.
    let has_contents = doc.setup.contents;
    // Facing pages need Word told that odd and even pages have headers of their own.
    if has_contents || facing {
        let _ = write!(content_types, "<Override PartName=\"/word/settings.xml\" ContentType=\"{CT}.settings+xml\"/>");
        let _ = write!(rels, "<Relationship Id=\"rIdSettings\" Type=\"{REL}/settings\" Target=\"settings.xml\"/>");
    }
    for (file, id, _, _) in &bands {
        let kind = if file.starts_with("header") { "header" } else { "footer" };
        let _ = write!(content_types, "<Override PartName=\"/word/{file}\" ContentType=\"{CT}.{kind}+xml\"/>");
        let _ = write!(rels, "<Relationship Id=\"{id}\" Type=\"{REL}/{kind}\" Target=\"{file}\"/>");
    }
    content_types.push_str("</Types>");
    rels.push_str("</Relationships>");

    let root_rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"{REL}/officeDocument\" Target=\"word/document.xml\"/></Relationships>"
    );

    let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
    let mut text_parts: Vec<(&str, String)> = vec![
        ("[Content_Types].xml", content_types),
        ("_rels/.rels", root_rels),
        ("word/document.xml", document),
        ("word/_rels/document.xml.rels", rels),
        ("word/styles.xml", styles_xml(&default, twips(doc.setup.content_size().x))),
    ];
    if has_comments {
        text_parts.push(("word/comments.xml", comments_xml(&doc.notes)));
    }
    if has_numbering {
        text_parts.push(("word/numbering.xml", numbering_xml(numbered_lists)));
    }
    if has_contents || facing {
        let even = if facing { "<w:evenAndOddHeaders/>" } else { "" };
        let update = if has_contents { "<w:updateFields w:val=\"true\"/>" } else { "" };
        let settings = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:settings xmlns:w=\"{NS_W}\">{even}{update}</w:settings>"
        );
        text_parts.push(("word/settings.xml", settings));
    }
    parts.extend(text_parts.into_iter().map(|(n, s)| (n.to_owned(), s.into_bytes())));
    parts.extend(bands.into_iter().map(|(file, _, _, xml)| (format!("word/{file}"), xml.into_bytes())));
    for &id in &used_pics {
        if let Some((bytes, ext)) = media.get(&id) {
            parts.push((format!("word/media/image{id}.{ext}"), bytes.clone()));
        }
    }

    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in parts {
        zip.start_file(name, options).map_err(|e| e.to_string())?;
        zip.write_all(&data).map_err(|e| e.to_string())?;
    }
    Ok(zip.finish().map_err(|e| e.to_string())?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Orientation, TextBox};
    use std::io::Read;

    fn test_box(at: usize, text: &str, look: crate::model::BoxLook) -> TextBox {
        let style = Style::new("Liberation Serif");
        TextBox { id: 1, at, pos: egui::vec2(200.0, 300.0), size: egui::vec2(150.0, 40.0), text: text.into(), style, look, color: [10, 20, 30], behind: false, clear: false }
    }

    fn read_part(bytes: &[u8], name: &str) -> Option<String> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut file = zip.by_name(name).ok()?;
        let mut s = String::new();
        file.read_to_string(&mut s).unwrap();
        Some(s)
    }

    /// A document with this text (which must end in the final paragraph mark), all in `style`.
    fn doc_with(text: &str, style: Style) -> Doc {
        assert!(text.ends_with('\n'));
        let mut d = Doc::new();
        d.flow.text = text.to_owned();
        d.flow.styles = vec![style; text.chars().count()];
        d
    }

    fn count(xml: &str, tag: &str) -> usize {
        roxmltree::Document::parse(xml).unwrap().descendants().filter(|n| n.tag_name().name() == tag).count()
    }

    #[test]
    fn every_part_is_well_formed_xml() {
        let st = Style::new("Liberation Serif");
        let mut d = doc_with("a < b & c\n\u{c}tab\there\nitem\n", st.clone());
        d.setup.header = Band { look: BandLook::Facing, text: "{chapter} & <the rest>".into(), ..Band::default() };
        d.setup.footer = Band { look: BandLook::Circle, text: "{page}".into(), ..Band::default() };
        d.boxes.push(test_box(2, "a < b\nand {page}", crate::model::BoxLook::Quote));
        let n = d.flow.styles.len();
        d.flow.styles[n - 1].para = ParaAttrs { list: ListKind::Numbered, ..Default::default() };
        d.notes.push(Note { id: 1, start: 0, end: 5, text: "x & <y>\nsecond".into(), color: 0 });
        let bytes = to_docx(&d).unwrap();
        for name in [
            "[Content_Types].xml",
            "_rels/.rels",
            "word/document.xml",
            "word/_rels/document.xml.rels",
            "word/styles.xml",
            "word/comments.xml",
            "word/header1.xml",
            "word/header2.xml",
            "word/footer1.xml",
            "word/settings.xml",
            "word/numbering.xml",
        ] {
            let xml = read_part(&bytes, name).unwrap_or_else(|| panic!("missing {name}"));
            roxmltree::Document::parse(&xml).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    #[test]
    fn text_styles_breaks_and_setup_are_written() {
        let plain = Style::new("Liberation Serif");
        let bold = Style { bold: true, underline: true, size: 14.0, ..plain.clone() };
        let mut d = doc_with("hi\n\u{c}x\n", plain.clone());
        d.flow.styles[0] = bold.clone();
        d.flow.styles[1] = bold.clone();
        d.setup.orientation = Orientation::Landscape;
        let bytes = to_docx(&d).unwrap();
        let xml = read_part(&bytes, "word/document.xml").unwrap();
        assert_eq!(count(&xml, "p"), 2, "a paragraph for each of the two lines, none for the bare break");
        assert_eq!(count(&xml, "pageBreakBefore"), 1, "the paragraph after the break starts a page");
        assert!(xml.contains("w:ascii=\"Times New Roman\""), "look-alike font gets its Office name");
        assert!(xml.contains("<w:b/>") && xml.contains("<w:u w:val=\"single\"/>"));
        assert!(xml.contains("w:val=\"28\""), "14pt = 28 half-points");
        assert!(xml.contains("w:orient=\"landscape\""));
        assert!(xml.contains("w:w=\"16840\" w:h=\"11900\""), "A4 landscape in twips");
        assert!(xml.contains("w:top=\"1440\"") && xml.contains("w:left=\"1440\""), "1 inch margins");
        assert!(read_part(&bytes, "word/footer1.xml").is_none(), "no footer without page numbers");
        assert!(read_part(&bytes, "word/numbering.xml").is_none(), "no numbering part without lists");
    }

    #[test]
    fn an_empty_page_between_breaks_is_kept() {
        let st = Style::new("Arial");
        let d = doc_with("a\u{c}\u{c}b\n", st);
        let xml = read_part(&to_docx(&d).unwrap(), "word/document.xml").unwrap();
        assert_eq!(count(&xml, "p"), 3, "a, an empty page, b");
        assert_eq!(count(&xml, "pageBreakBefore"), 2);
    }

    #[test]
    fn paragraph_formats_are_written() {
        let st = Style::new("Arial");
        let mut d = doc_with("one\ntwo\nthree\nfour\nfive\n", st);
        let para = |d: &mut Doc, end: usize, p: ParaAttrs| d.flow.styles[end].para = p;
        para(&mut d, 3, ParaAttrs { align: Align::Center, ..Default::default() });
        para(&mut d, 7, ParaAttrs { align: Align::Justify, spacing: 1.5, ..Default::default() });
        para(&mut d, 13, ParaAttrs { list: ListKind::Bullet, ..Default::default() });
        para(&mut d, 18, ParaAttrs { list: ListKind::Numbered, ..Default::default() });
        para(&mut d, 23, ParaAttrs { list: ListKind::Numbered, ..Default::default() });
        let bytes = to_docx(&d).unwrap();
        let xml = read_part(&bytes, "word/document.xml").unwrap();
        assert!(xml.contains("<w:jc w:val=\"center\"/>") && xml.contains("<w:jc w:val=\"both\"/>"));
        assert!(xml.contains("w:line=\"360\""), "1.5 line spacing");
        assert_eq!(count(&xml, "numPr"), 3);
        let numbering = read_part(&bytes, "word/numbering.xml").unwrap();
        assert!(numbering.contains("w:numFmt w:val=\"bullet\"") && numbering.contains("w:numFmt w:val=\"decimal\""));
        assert_eq!(count(&numbering, "num"), 2, "one bullet list and one numbered list");
    }

    #[test]
    fn two_separate_numbered_lists_each_restart() {
        let st = Style::new("Arial");
        let mut d = doc_with("a\nb\nbreak\nc\n", st);
        let numbered = ParaAttrs { list: ListKind::Numbered, ..Default::default() };
        for end in [1, 3, 11] {
            d.flow.styles[end].para = numbered;
        }
        let bytes = to_docx(&d).unwrap();
        let numbering = read_part(&bytes, "word/numbering.xml").unwrap();
        assert_eq!(numbering.matches("startOverride").count(), 2);
        let xml = read_part(&bytes, "word/document.xml").unwrap();
        assert!(xml.contains("<w:numId w:val=\"2\"/>") && xml.contains("<w:numId w:val=\"3\"/>"));
    }

    #[test]
    fn notes_become_comments_over_the_same_text() {
        let st = Style::new("Arial");
        let mut d = doc_with("one two three\n", st);
        d.notes.push(Note { id: 1, start: 4, end: 7, text: "check".into(), color: 0 });
        let bytes = to_docx(&d).unwrap();
        let xml = read_part(&bytes, "word/document.xml").unwrap();
        let start = xml.find("commentRangeStart").unwrap();
        let end = xml.find("commentRangeEnd").unwrap();
        let between = &xml[start..end];
        assert!(between.contains(">two<"), "the comment covers exactly the noted word: {between}");
        assert!(!between.contains("one") && !between.contains("three"));
        assert!(read_part(&bytes, "word/comments.xml").unwrap().contains("check"));
    }

    #[test]
    fn pictures_are_embedded() {
        let st = Style::new("Arial");
        let mut d = doc_with("text\n\u{fffc}\nmore\n", st.clone());
        d.flow.styles[5].image = 1;
        d.images.push(crate::model::ImageData {
            id: 1,
            format: "png".into(),
            bytes: crate::images::test_png(20, 10, [1, 2, 3]),
            px: (20, 10),
            width_pt: 100.0,
            rotation: 0,
        });
        let bytes = to_docx(&d).unwrap();
        for name in ["word/document.xml", "word/_rels/document.xml.rels", "[Content_Types].xml"] {
            roxmltree::Document::parse(&read_part(&bytes, name).unwrap()).unwrap();
        }
        let xml = read_part(&bytes, "word/document.xml").unwrap();
        assert_eq!(count(&xml, "drawing"), 1);
        assert!(xml.contains("cx=\"1270000\" cy=\"635000\""), "100 x 50 pt in EMU");
        assert!(xml.contains("r:embed=\"rIdImg1\""));
        assert!(read_part(&bytes, "word/_rels/document.xml.rels").unwrap().contains("media/image1.png"));
        let mut zip = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let mut media = Vec::new();
        zip.by_name("word/media/image1.png").unwrap().read_to_end(&mut media).unwrap();
        assert_eq!(media, d.images[0].bytes);
    }

    #[test]
    fn timestamps_are_iso_8601() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(951_782_400), "2000-02-29T00:00:00Z"); // leap day
        assert_eq!(iso8601(1_790_960_400), "2026-10-02T17:00:00Z");
    }

    #[test]
    fn an_empty_document_still_has_a_paragraph() {
        let bytes = to_docx(&Doc::new()).unwrap();
        let xml = read_part(&bytes, "word/document.xml").unwrap();
        assert_eq!(count(&xml, "p"), 1);
    }

    /// The body's paragraphs: their text, and their properties as XML.
    fn paragraphs(xml: &str) -> Vec<(String, String)> {
        let d = roxmltree::Document::parse(xml).unwrap();
        let body = d.descendants().find(|n| n.tag_name().name() == "body").unwrap();
        body.children()
            .filter(|n| n.tag_name().name() == "p")
            .map(|p| {
                let text = p.descendants().filter(|n| n.tag_name().name() == "t").filter_map(|n| n.text()).collect();
                let ppr = p.children().find(|n| n.tag_name().name() == "pPr").map_or(String::new(), |n| xml[n.range()].to_owned());
                (text, ppr)
            })
            .collect()
    }

    /// A story with a contents page and a chapter, laid out (so pages are known).
    fn chapter_doc(lines: u8) -> Doc {
        let st = Style::new("Liberation Serif");
        let mut d = doc_with("Chapter One\nIt was a dark night.\nMore.\n", st);
        d.flow.styles[11].para.set_chapter_title(true);
        d.setup.drop_cap_lines = lines;
        d.setup.contents = true;
        crate::layout::with_ctx(|ctx| d.full_paginate(ctx, &Style::new("x")));
        d
    }

    #[test]
    fn chapters_become_headings_with_drop_caps_and_a_contents_field() {
        let d = chapter_doc(3);
        let bytes = to_docx(&d).unwrap();
        let xml = read_part(&bytes, "word/document.xml").unwrap();
        let paras = paragraphs(&xml);
        let texts: Vec<&str> = paras.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(texts, ["Contents", "Chapter One2", "Chapter One", "I", "t was a dark night.", "More."]);

        // The contents: a field over the headings, shown with Caprice's page until Word updates it.
        assert!(xml.contains(r#"<w:instrText xml:space="preserve"> TOC \o "1-1" \h \z </w:instrText>"#));
        assert_eq!((count(&xml, "fldChar"), count(&xml, "tab")), (3, 2), "begin, separate, end; the entry's tab stop and tab");
        assert!(paras[1].1.contains(r#"<w:pStyle w:val="TOC1"/>"#) && paras[1].1.contains(r#"w:leader="dot""#));
        let settings = read_part(&bytes, "word/settings.xml").expect("Word is asked to update the field");
        assert!(settings.contains(r#"<w:updateFields w:val="true"/>"#));
        assert!(read_part(&bytes, "word/_rels/document.xml.rels").unwrap().contains("settings.xml"));

        // The title: Heading 1, after the page break, drawn larger and bold as in Caprice.
        assert!(paras[2].1.starts_with(r#"<w:pPr><w:pStyle w:val="Heading1"/><w:pageBreakBefore/>"#), "{}", paras[2].1);
        assert!(xml.contains(r#"<w:b/><w:bCs/><w:sz w:val="38"/>"#), "12pt × 1.6, bold");
        let styles = read_part(&bytes, "word/styles.xml").unwrap();
        assert!(styles.contains(r#"<w:name w:val="heading 1"/>"#) && styles.contains(r#"<w:outlineLvl w:val="0"/>"#));

        // The drop cap: Word's own, three lines deep, then the rest of the paragraph.
        assert!(paras[3].1.contains(r#"<w:framePr w:dropCap="drop" w:lines="3""#), "{}", paras[3].1);
        assert!(!paras[4].1.contains("framePr") && !paras[5].1.contains("framePr"), "only the chapter's first paragraph");

        for name in ["word/document.xml", "word/styles.xml", "word/settings.xml"] {
            roxmltree::Document::parse(&read_part(&bytes, name).unwrap()).unwrap();
        }
    }

    #[test]
    fn without_contents_or_drop_caps_word_gets_neither() {
        let mut d = chapter_doc(0);
        d.setup.contents = false;
        let bytes = to_docx(&d).unwrap();
        let xml = read_part(&bytes, "word/document.xml").unwrap();
        let texts: Vec<String> = paragraphs(&xml).into_iter().map(|(t, _)| t).collect();
        assert_eq!(texts, ["Chapter One", "It was a dark night.", "More."]);
        assert!(!xml.contains("framePr") && !xml.contains("fldChar"));
        assert!(read_part(&bytes, "word/settings.xml").is_none());
    }
}

#[cfg(test)]
pub(crate) mod sample {
    use super::*;
    use crate::model::{BoxLook, TextBox, is_terminator};

    /// Writes a sample document for checking in real office suites:
    /// `CAPRICE_SAMPLE_OUT=/some/file.docx cargo test sample_docx`
    #[test]
    fn sample_docx() {
        let Some(out) = std::env::var_os("CAPRICE_SAMPLE_OUT") else { return };
        let mut d = sample_doc();
        // Laid out for the contents' page numbers, in a context that has its font: one that does not
        // panics as soon as any other test has loaded it.
        let ctx = egui::Context::default();
        let mut fonts = crate::fonts::FontBook::new();
        fonts.ensure(&ctx, "Liberation Serif", true);
        for frame in 0..2 {
            // Fonts set on a context are there from its next frame.
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                if frame == 1 {
                    d.full_paginate(ui.ctx(), &Style::new("Liberation Serif"));
                }
            });
            out.textures_delta.clear();
        }
        std::fs::write(out, to_docx(&d).unwrap()).unwrap();
    }

    /// Something of every kind of formatting, over a few pages, not yet laid out.
    pub(crate) fn sample_doc() -> Doc {
        let plain = Style::new("Liberation Serif");
        let bold = Style { bold: true, size: 20.0, ..plain.clone() };
        let under = Style { underline: true, ..plain.clone() };
        let italic_red = Style { italic: true, color: Some([205, 40, 40]), ..plain.clone() };
        let centered = ParaAttrs { align: Align::Center, ..Default::default() };
        let bullet = ParaAttrs { list: ListKind::Bullet, ..Default::default() };
        let numbered = ParaAttrs { list: ListKind::Numbered, ..Default::default() };
        let double = ParaAttrs { spacing: 2.0, align: Align::Justify, ..Default::default() };
        let mut title = ParaAttrs::default();
        title.set_chapter_title(true);
        let justified = ParaAttrs { align: Align::Justify, ..Default::default() };
        let story = "It was nearly midnight and the Prime Minister was sitting alone in his office, reading a long memo \
that was slipping through his brain without leaving the slightest trace of meaning behind. He was waiting for a call \
from the President of a far distant country.";
        let mut d = Doc::new();
        d.flow.text.clear();
        d.flow.styles.clear();
        let parts: Vec<(&str, &Style, ParaAttrs)> = vec![
            ("A centered title", &bold, ParaAttrs::default()),
            ("\n", &bold, centered),
            ("Plain text, then ", &plain, ParaAttrs::default()),
            ("underlined words", &under, ParaAttrs::default()),
            (", ", &plain, ParaAttrs::default()),
            ("red italic words", &italic_red, ParaAttrs::default()),
            (" and more plain text.", &plain, ParaAttrs::default()),
            ("\n", &plain, ParaAttrs::default()),
            ("first bullet", &plain, ParaAttrs::default()),
            ("\n", &plain, bullet),
            ("second bullet", &plain, ParaAttrs::default()),
            ("\n", &plain, bullet),
            ("step one", &plain, ParaAttrs::default()),
            ("\n", &plain, numbered),
            ("step two", &plain, ParaAttrs::default()),
            ("\n", &plain, numbered),
            ("A justified paragraph with double line spacing that is long enough to wrap onto several lines so that the spacing can be seen clearly.", &plain, ParaAttrs::default()),
            ("\n", &plain, double),
            ("\u{c}", &plain, ParaAttrs::default()),
            ("Second page starts here.", &plain, ParaAttrs::default()),
            ("\n", &plain, ParaAttrs::default()),
            ("The Other Minister", &plain, ParaAttrs::default()),
            ("\n", &plain, title),
            (story, &plain, ParaAttrs::default()),
            ("\n", &plain, justified),
            ("A second paragraph without a drop cap.", &plain, ParaAttrs::default()),
            ("\u{c}", &plain, justified),
            ("Hagrid?", &plain, ParaAttrs::default()),
            ("\n", &plain, title),
            (story, &plain, ParaAttrs::default()),
            ("\n", &plain, justified),
        ];
        for (t, s, p) in parts {
            d.flow.text.push_str(t);
            for ch in t.chars() {
                d.flow.styles.push(if is_terminator(ch) { s.with_para(p) } else { s.clone() });
            }
        }
        d.setup.header = Band { look: BandLook::Filigree, text: "{chapter}".into(), ..Band::default() };
        d.setup.footer = Band { look: BandLook::Circle, text: "{page}".into(), ..Band::default() };
        let style = Style { size: 11.0, ..plain.clone() };
        let text = "Set apart: a few words in a box\nof their own.".into();
        let (pos, size) = (egui::vec2(330.0, 420.0), egui::vec2(180.0, 50.0));
        d.boxes.push(TextBox { id: 1, at: 60, pos, size, text, style, look: BoxLook::Sidebar, color: crate::model::ACCENT_INK, behind: false, clear: false });
        // One behind the text, see-through, over the lines of the story.
        let (text, pos, size) = ("BEHIND THE TEXT".into(), egui::vec2(140.0, 236.0), egui::vec2(260.0, 40.0));
        let style = Style { size: 22.0, bold: true, ..plain.clone() };
        d.boxes.push(TextBox { id: 2, at: 60, pos, size, text, style, look: BoxLook::Shaded, color: crate::model::ACCENT_INK, behind: true, clear: true });
        d.setup.margin_left = 100.0;
        d.setup.drop_cap_lines = 3;
        d.setup.contents = true;
        // The contents in roman numerals, the story from 1, the last chapter without numbers.
        use crate::numbers::{Counting, Figures, NumberMark, Numbering};
        d.setup.contents_numbers = Numbering { counting: Counting::On, figures: Figures::Roman };
        d.number_marks.push(NumberMark { at: 0, numbering: Numbering { counting: Counting::From(1), figures: Figures::Arabic } });
        let last = d.flow.text[..d.flow.text.find("Hagrid?").unwrap()].chars().count();
        d.number_marks.push(NumberMark { at: last, numbering: Numbering { counting: Counting::Off, figures: Figures::Arabic } });
        d.notes.push(Note { id: 1, start: 26, end: 42, text: "Remember to rephrase this.".into(), color: 1 });
        d
    }

}
