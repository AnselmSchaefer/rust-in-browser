//! The contents: pages before the story listing the chapter titles and the pages they start on,
//! when `PageSetup::contents` asks for them. The list is never stored; it is made from the titles
//! every time a page of it is laid out, so it is always up to date. Its pages hold none of the
//! story's text, so the caret is never on them.
//!
//! How many pages it takes depends only on the titles, not on their page numbers, so pagination
//! never goes in circles: the pages are worked out first, and the numbers filled in when drawn.

use std::ops::Range;
use std::sync::Arc;

use eframe::egui::{self, Color32, Pos2, Rect, Vec2, pos2, vec2};

use crate::layout::run_format;
use crate::model::{CHAPTER_TITLE_SCALE, CONTENTS_CHAR, Doc, IMAGE_CHAR, ParaAttrs, Style, is_terminator};

/// One line of the contents.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub title: String,
    /// Where the title starts in the flow.
    pub at: usize,
    /// The page it is on (counting from 0), once the pages are known.
    pub page: Option<usize>,
    /// That page's number as written (`numbers.rs`), once the pages are known; shown even where
    /// headers and footers show none.
    pub number: String,
}

/// The contents laid out, relative to the top-left of its paragraph.
pub struct ContentsBlock {
    pub size: Vec2,
    pub texts: Vec<(Arc<egui::Galley>, Pos2)>,
    /// Each entry's area, and the page it leads to.
    pub links: Vec<(Rect, usize)>,
}

impl Doc {
    /// The chapter titles, in order. Their pages are only filled in if `with_pages`.
    pub fn chapters(&self, with_pages: bool) -> Vec<Entry> {
        let mut out = Vec::new();
        let numbers = if with_pages { self.page_numbers() } else { Vec::new() };
        let (mut pc, mut pb) = (0, 0);
        for (c, (b, ch)) in self.flow.text.char_indices().enumerate() {
            if !is_terminator(ch) {
                continue;
            }
            if self.flow.styles[c].para.is_chapter_title() {
                let title: String = self.flow.text[pb..b].chars().filter(|&k| k != IMAGE_CHAR).collect();
                let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
                if !title.is_empty() {
                    let page = (with_pages && !self.spans.is_empty()).then(|| self.page_of(pc));
                    let number = page.and_then(|p| numbers.get(p)).map_or_else(String::new, |n| n.text());
                    out.push(Entry { title, at: pc, page, number });
                }
            }
            (pc, pb) = (c + 1, b + ch.len_utf8());
        }
        out
    }

    /// Files saved before the contents became a page setting held a char for them, in a
    /// paragraph of its own: take those out (with their paragraph end) and turn the setting on.
    /// Returns where the removed chars were, in the text as it was, in order.
    pub fn take_out_contents_chars(&mut self) -> Vec<usize> {
        let mut removed = Vec::new();
        let (mut text, mut styles) = (String::new(), Vec::new());
        let chars: Vec<char> = self.flow.text.chars().collect();
        let mut k = 0;
        while k < chars.len() {
            if chars[k] == CONTENTS_CHAR {
                removed.push(k);
                let alone = (k == 0 || is_terminator(chars[k - 1])) && chars.get(k + 1).is_some_and(|&t| is_terminator(t));
                if alone && k + 2 < chars.len() {
                    removed.push(k + 1);
                    k += 1;
                }
            } else {
                text.push(chars[k]);
                styles.push(self.flow.styles[k].clone());
            }
            k += 1;
        }
        if !removed.is_empty() {
            self.flow.text = text;
            self.flow.styles = styles;
            self.setup.contents = true;
        }
        removed
    }

    /// The look of the contents: the font and size most of the text (looked at up to a point)
    /// has, plain.
    pub fn contents_style(&self) -> Style {
        let mut counts: Vec<(&Style, usize)> = Vec::new();
        for (ch, st) in self.flow.text.chars().zip(&self.flow.styles).take(20_000) {
            if is_terminator(ch) || ch == IMAGE_CHAR {
                continue;
            }
            match counts.iter_mut().find(|(s, _)| s.font == st.font && s.size == st.size) {
                Some((_, n)) => *n += 1,
                None => counts.push((st, 1)),
            }
        }
        let most = counts.iter().max_by_key(|(_, n)| *n).map(|(st, _)| *st);
        let st = most.or(self.flow.styles.first()).cloned().unwrap_or_else(|| Style::new("Default"));
        Style { size: st.size, ..Style::new(&st.font) }
    }

    /// How many pages the contents take up (none if the setup has no contents).
    pub fn contents_pages(&self, ctx: &egui::Context) -> usize {
        if !self.setup.contents {
            return 0;
        }
        let content = self.setup.content_size();
        split(ctx, &self.chapters(false), &self.contents_style(), content.x, content.y).len()
    }
}

fn text(ctx: &egui::Context, s: &str, st: &Style, scale: f32, wrap: f32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap;
    job.append(s, 0.0, run_format(ctx, st, scale, Color32::TRANSPARENT, ParaAttrs::default()));
    ctx.fonts_mut(|f| f.layout_job(job))
}

/// Heights in page points of the contents: their heading, the gap between lines, and the line
/// for each entry.
struct Metrics {
    heading: Arc<egui::Galley>,
    line: f32,
    title_w: f32,
}

fn metrics(ctx: &egui::Context, st: &Style, width: f32) -> Metrics {
    let heading_st = Style { size: st.size * CHAPTER_TITLE_SCALE, bold: true, ..st.clone() };
    let number_w = text(ctx, "0000", st, 1.0, f32::INFINITY).size().x;
    Metrics {
        heading: text(ctx, "Contents", &heading_st, 1.0, f32::INFINITY),
        line: text(ctx, " ", st, 1.0, f32::INFINITY).size().y,
        title_w: (width - number_w - st.size).max(width * 0.5),
    }
}

/// Which entries go on each page of the contents, `max_height` tall: the heading and as many as
/// fit on the first, then on as many more as it takes. Every page holds at least one entry.
fn split(ctx: &egui::Context, entries: &[Entry], st: &Style, width: f32, max_height: f32) -> Vec<Range<usize>> {
    let m = metrics(ctx, st, width);
    let mut pages = Vec::new();
    let (mut from, mut y) = (0, m.heading.size().y + m.line);
    for (k, e) in entries.iter().enumerate() {
        let h = text(ctx, &e.title, st, 1.0, m.title_w).size().y;
        if y + h > max_height && k > from {
            pages.push(from..k);
            (from, y) = (k, 0.0);
        }
        y += h + m.line * 0.4;
    }
    pages.push(from..entries.len());
    pages
}

/// Lay out page `part` of the contents in the style `st` (that of its placeholder char), `width`
/// wide (page points) on pages `max_height` tall. Heights and positions come from page size, so the
/// block is equally tall, relative to the page, at every zoom.
pub fn layout_contents(ctx: &egui::Context, entries: &[Entry], st: &Style, width: f32, max_height: f32, scale: f32, part: usize) -> ContentsBlock {
    let m = metrics(ctx, st, width);
    let range = split(ctx, entries, st, width, max_height).get(part).cloned().unwrap_or_default();
    let mut texts = Vec::new();
    let mut links = Vec::new();
    let (mut y, mut bottom) = (0.0, 0.0);
    if part == 0 {
        let heading_st = Style { size: st.size * CHAPTER_TITLE_SCALE, bold: true, ..st.clone() };
        let heading = text(ctx, "Contents", &heading_st, scale, f32::INFINITY);
        texts.push((heading.clone(), pos2((width * scale - heading.size().x) / 2.0, 0.0)));
        bottom = m.heading.size().y;
        y = bottom + m.line;
    }

    for e in &entries[range] {
        let h = text(ctx, &e.title, st, 1.0, m.title_w).size().y;
        bottom = y + h;
        let title = text(ctx, &e.title, st, scale, m.title_w * scale);
        let top = y * scale;
        if let Some(page) = e.page {
            // The page number at the right, on the title's last line, with dots leading to it.
            let last = title.rows.last().map_or(Rect::ZERO, |r| r.rect());
            let number = text(ctx, &e.number, st, scale, f32::INFINITY);
            let nx = width * scale - number.size().x;
            let dot = text(ctx, " .", st, scale, f32::INFINITY).size().x.max(1.0);
            let room = nx - last.right() - st.size * scale * 0.6;
            let dots = text(ctx, &" .".repeat((room / dot).max(0.0) as usize), st, scale, f32::INFINITY);
            texts.push((dots.clone(), pos2(nx - dots.size().x - st.size * scale * 0.3, top + last.top())));
            texts.push((number, pos2(nx, top + last.top())));
            links.push((Rect::from_min_size(pos2(0.0, top), vec2(width * scale, h * scale)), page));
        }
        texts.push((title, pos2(0.0, top)));
        y += h + m.line * 0.4;
    }
    ContentsBlock { size: vec2(width, bottom) * scale, texts, links }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::with_ctx;
    use crate::model::PAGE_BREAK;

    /// A document of paragraphs; `true` marks a chapter title. One ending in a page break ends
    /// with that instead of a line break.
    fn doc(paras: &[(&str, bool)]) -> Doc {
        let mut d = Doc::new();
        let st = Style::new("x");
        let mut title = ParaAttrs::default();
        title.set_chapter_title(true);
        d.flow.text.clear();
        d.flow.styles.clear();
        for &(text, is_title) in paras {
            d.flow.text.push_str(text);
            d.flow.styles.extend(std::iter::repeat_n(st.clone(), text.chars().count()));
            let end = if text.ends_with(PAGE_BREAK) { d.flow.text.pop(); d.flow.styles.pop(); PAGE_BREAK } else { '\n' };
            d.flow.text.push(end);
            d.flow.styles.push(st.with_para(if is_title { title } else { ParaAttrs::default() }));
        }
        d
    }

    /// A story of `chapters` chapters of `paras_each` long paragraphs, with the contents.
    fn story(chapters: usize, paras_each: usize) -> Doc {
        let mut paras = Vec::new();
        for n in 1..=chapters {
            paras.push((format!("Chapter {n}"), true));
            paras.extend(std::iter::repeat_n(("It was nearly midnight and the Prime Minister sat alone in his office. ".repeat(8), false), paras_each));
        }
        let mut d = doc(&paras.iter().map(|(s, t)| (s.as_str(), *t)).collect::<Vec<_>>());
        d.setup.contents = true;
        d
    }

    #[test]
    fn the_chapters_are_the_titles_with_text_and_their_pages() {
        with_ctx(|ctx| {
            let mut d = doc(&[("Intro", false), ("  The   Other\tMinister ", true), ("", true), ("Text", false), ("Next\u{c}", false), ("Hagrid?", true)]);
            d.full_paginate(ctx, &Style::new("x"));
            let ch = d.chapters(true);
            let titles: Vec<&str> = ch.iter().map(|e| e.title.as_str()).collect();
            assert_eq!(titles, ["The Other Minister", "Hagrid?"], "empty titles are left out, spaces tidied");
            assert_eq!((ch[0].at, ch[0].page, ch[1].page), (6, Some(0), Some(1)));
            assert!(d.chapters(false).iter().all(|e| e.page.is_none()));
        });
    }

    #[test]
    fn the_contents_list_each_chapter_and_lead_to_its_page() {
        with_ctx(|ctx| {
            let mut d = story(4, 6);
            d.full_paginate(ctx, &Style::new("x"));
            assert!(d.pages() >= 3);
            assert!(d.chapters(true).windows(2).any(|w| w[0].page != w[1].page), "chapters on different pages");
            assert_eq!(d.spans[0], crate::model::Span { contents: Some(0), ..Default::default() }, "a page holding no text");
            assert_eq!((d.spans[1].start, d.spans[1].contents), (0, None), "the story starts on the next");
            let l = d.layout_page(ctx, 0, 1.0, &[]);
            let block = l.paras[0].contents.as_ref().expect("the first page is the contents");
            assert_eq!(block.links.len(), 4);
            let shown: Vec<String> = block.texts.iter().map(|(g, _)| g.text().to_owned()).collect();
            for (n, e) in d.chapters(true).iter().enumerate() {
                assert!(shown.contains(&e.title));
                assert!(shown.contains(&(e.page.unwrap() + 1).to_string()), "page of {}: {shown:?}", e.title);
                let (r, page) = block.links[n];
                assert_eq!(l.link_at(r.center().to_vec2()), Some(page));
                assert_eq!(page, e.page.unwrap());
            }
            assert_eq!(l.link_at(vec2(1.0, 1.0)), None, "the heading leads nowhere");
            // No spot in the story is on it.
            assert_eq!(d.page_of(0), 1);
        });
    }

    #[test]
    fn without_the_setting_there_are_no_contents_pages() {
        with_ctx(|ctx| {
            let mut d = story(4, 1);
            d.setup.contents = false;
            d.full_paginate(ctx, &Style::new("x"));
            assert_eq!(d.contents_pages(ctx), 0);
            assert!(d.spans.iter().all(|s| s.contents.is_none()));
            assert_eq!(d.page_of(0), 0);
        });
    }

    #[test]
    fn the_contents_take_the_same_room_at_every_zoom() {
        with_ctx(|ctx| {
            let mut d = story(3, 1);
            d.full_paginate(ctx, &Style::new("x"));
            let h = d.layout_page(ctx, 0, 1.0, &[]).height;
            for sc in [0.8f32, 1.37] {
                assert!((d.layout_page(ctx, 0, sc, &[]).height / sc - h).abs() < 0.01);
            }
        });
    }

    #[test]
    fn long_contents_go_on_over_as_many_pages_as_they_take() {
        with_ctx(|ctx| {
            let mut d = story(120, 0);
            d.full_paginate(ctx, &Style::new("x"));
            let n = d.contents_pages(ctx);
            assert!(n >= 3, "{n} pages");
            for (k, sp) in d.spans[..n].iter().enumerate() {
                assert_eq!((sp.start, sp.end, sp.contents), (0, 0, Some(k)));
            }
            assert_eq!((d.spans[n].start, d.spans[n].contents), (0, None), "the story goes on after them");

            let mut shown = Vec::new();
            for k in 0..n {
                let l = d.layout_page(ctx, k, 1.0, &[]);
                assert!(l.height <= d.setup.content_size().y + 0.5, "page {k}: {} of {}", l.height, d.setup.content_size().y);
                let block = l.paras[0].contents.as_ref().unwrap();
                let texts: Vec<&str> = block.texts.iter().map(|(g, _)| g.text()).collect();
                assert_eq!(texts.contains(&"Contents"), k == 0, "the heading only on the first");
                shown.extend(block.links.iter().map(|&(_, page)| page));
            }
            let pages: Vec<usize> = d.chapters(true).iter().map(|e| e.page.unwrap()).collect();
            assert_eq!(shown, pages, "every chapter, once, in order");
            assert_eq!(pages[0], n, "the first chapter is on the page after the contents");
        });
    }

    #[test]
    fn chapters_added_or_taken_away_change_the_contents_pages_like_from_scratch() {
        with_ctx(|ctx| {
            let st = Style::new("x");
            let mut d = story(30, 0);
            d.full_paginate(ctx, &st);
            let before = d.contents_pages(ctx);
            let mut title = ParaAttrs::default();
            title.set_chapter_title(true);
            let check = |d: &mut Doc| {
                let incremental = d.spans.clone();
                d.full_paginate(ctx, &st);
                assert_eq!(incremental, d.spans);
            };
            while d.contents_pages(ctx) == before {
                let at = d.total_chars() - 1;
                let new = crate::edit::Piece { text: "More\n".into(), styles: [vec![st.clone(); 4], vec![st.with_para(title)]].concat() };
                d.apply(crate::edit::Edit::Replace { at, old: crate::edit::Piece::default(), new }, 0.0);
                d.paginate_after(ctx, &st, at, at, 5, 5);
                check(&mut d);
            }
            // Taking them all away again, from the end.
            while d.contents_pages(ctx) > before {
                let at = d.total_chars() - 6;
                let old = crate::edit::Piece { text: "More\n".into(), styles: d.flow.styles[at..at + 5].to_vec() };
                d.apply(crate::edit::Edit::Replace { at, old, new: crate::edit::Piece::default() }, 0.0);
                d.paginate_after(ctx, &st, at, at + 5, -5, -5);
                check(&mut d);
            }
        });
    }

    #[test]
    fn a_new_chapter_far_on_reflows_like_from_scratch() {
        with_ctx(|ctx| {
            let st = Style::new("x");
            // A title made of a paragraph begun pages before.
            let mut d = story(3, 5);
            d.full_paginate(ctx, &st);
            let at = d.flow.text.rfind("It was").unwrap();
            let at = d.flow.text[..at].chars().count();
            let mut title = ParaAttrs::default();
            title.set_chapter_title(true);
            let new = crate::edit::Piece { text: "A new chapter\n".into(), styles: [vec![st.clone(); 13], vec![st.with_para(title)]].concat() };
            d.apply(crate::edit::Edit::Replace { at, old: crate::edit::Piece::default(), new }, 0.0);
            d.paginate_after(ctx, &st, at, at, 14, 14);
            let incremental = d.spans.clone();
            d.full_paginate(ctx, &st);
            assert_eq!(incremental, d.spans);
        });
    }

    #[test]
    fn the_contents_look_like_most_of_the_text_plain() {
        let mut d = doc(&[("Heading", false), ("Body text, most of it.", false)]);
        let body = Style { size: 11.0, underline: true, ..Style::new("Body") };
        for st in &mut d.flow.styles[..7] {
            *st = Style { size: 20.0, bold: true, ..Style::new("Head") };
        }
        for st in &mut d.flow.styles[8..30] {
            *st = body.clone();
        }
        assert_eq!(d.contents_style(), Style { size: 11.0, ..Style::new("Body") });
    }

    #[test]
    fn files_with_a_contents_char_open_with_the_setting_instead() {
        let mut d = doc(&[("\u{e000}\u{c}", false), ("Chapter One", true), ("Text", false)]);
        assert_eq!(d.take_out_contents_chars(), [0, 1]);
        assert_eq!(d.flow.text, "Chapter One\nText\n");
        assert_eq!(d.flow.styles.len(), d.flow.text.chars().count());
        assert!(d.setup.contents);
        // In the middle of a paragraph, only the char goes; without one, nothing changes.
        let mut d = doc(&[("Fore\u{e000}word", false)]);
        assert_eq!(d.take_out_contents_chars(), [4]);
        assert_eq!(d.flow.text, "Foreword\n");
        let mut d = doc(&[("Plain", false)]);
        assert!(d.take_out_contents_chars().is_empty() && !d.setup.contents);
    }
}
