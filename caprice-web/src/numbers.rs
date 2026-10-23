//! Page numbers: which pages show one, and which number. Counting runs through every page, from 1
//! on the first, and a change to it (`Numbering`) holds from the page it is set on until the next
//! change: start again at some number, go on counting, or show no numbers (the count still goes on
//! underneath, as Word does), each in figures or small roman numerals.
//!
//! A change is pinned to a char, the first of the page it was set on (`NumberMark`), like a text
//! box, and holds from the page that char is on: pages reflow, and the appendix that should have
//! no numbers is wherever its text has gone. The contents pages hold no text, so theirs is a page
//! setting (`PageSetup::contents_numbers`). Numbers change no page's size (R6): the contents keep
//! room for four figures whatever they show.

use serde::{Deserialize, Serialize};

use crate::model::Doc;

/// How counting goes from a page on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Counting {
    /// On from the page before, the numbers shown.
    #[default]
    On,
    /// Again from this number, shown.
    From(u32),
    /// No numbers shown; the count goes on.
    Off,
}

/// How a number is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Figures {
    #[default]
    Arabic,
    /// i, ii, iii, as front matter often is.
    Roman,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Numbering {
    pub counting: Counting,
    pub figures: Figures,
}

impl Numbering {
    pub fn is_usual(&self) -> bool {
        *self == Numbering::default()
    }
}

/// A change of numbering, pinned to a char of the story: it holds from that char's page on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NumberMark {
    pub at: usize,
    pub numbering: Numbering,
}

/// One page's number, and whether it is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageNumber {
    pub count: u32,
    pub shown: bool,
    pub figures: Figures,
}

impl PageNumber {
    /// The number as written, shown or not (the contents list a page's number either way).
    pub fn text(&self) -> String {
        match self.figures {
            Figures::Arabic => self.count.to_string(),
            Figures::Roman => roman(self.count),
        }
    }

    /// What a header or footer shows: nothing on a page without its number.
    pub fn label(&self) -> Option<String> {
        self.shown.then(|| self.text())
    }
}

/// `n` in small roman numerals (0 has none, and is written as a figure).
pub fn roman(mut n: u32) -> String {
    if n == 0 {
        return "0".into();
    }
    const DIGITS: [(u32, &str); 13] = [
        (1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"),
        (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i"),
    ];
    let mut s = String::new();
    for (v, d) in DIGITS {
        while n >= v {
            s.push_str(d);
            n -= v;
        }
    }
    s
}

/// A number written in figures or roman numerals, either case; `None` if it is neither.
pub fn parse(text: &str) -> Option<u32> {
    let t = text.trim().to_lowercase();
    if let Ok(k) = t.parse() {
        return Some(k);
    }
    let value = |c| match c {
        'i' => Some(1), 'v' => Some(5), 'x' => Some(10), 'l' => Some(50), 'c' => Some(100), 'd' => Some(500), 'm' => Some(1000),
        _ => None,
    };
    let digits: Vec<u32> = t.chars().map(value).collect::<Option<_>>()?;
    // A digit before a bigger one is taken away from it (iv, xc).
    let n = digits.iter().enumerate().map(|(k, &d)| if digits.get(k + 1).is_some_and(|&e| e > d) { -(d as i64) } else { d as i64 }).sum::<i64>();
    (n > 0 && roman(n as u32) == t).then_some(n as u32)
}

impl Doc {
    /// The number of every page, in order.
    pub fn page_numbers(&self) -> Vec<PageNumber> {
        let mut changes: Vec<(usize, Numbering)> = self.number_marks.iter().map(|m| (self.page_of(m.at), m.numbering)).collect();
        if self.spans.first().is_some_and(|s| s.contents.is_some()) {
            changes.push((0, self.setup.contents_numbers));
        }
        // On one page the change pinned last in the story counts; the contents' come before any.
        changes.sort_by_key(|&(page, _)| page);
        let mut out = Vec::with_capacity(self.spans.len());
        let mut now = PageNumber { count: 0, shown: true, figures: Figures::Arabic };
        let mut next = changes.iter().peekable();
        for i in 0..self.spans.len() {
            now.count += 1;
            while let Some((_, n)) = next.next_if(|(page, _)| *page == i) {
                now.figures = n.figures;
                match n.counting {
                    Counting::On => now.shown = true,
                    Counting::From(k) => (now.count, now.shown) = (k, true),
                    Counting::Off => now.shown = false,
                }
            }
            out.push(now);
        }
        out
    }

    pub fn page_number(&self, i: usize) -> PageNumber {
        self.page_numbers().get(i).copied().unwrap_or(PageNumber { count: i as u32 + 1, shown: true, figures: Figures::Arabic })
    }

    /// The numbering of page `i`: the contents' on a contents page, else the change pinned on that
    /// page, else the one in force there (counting on, or no numbers, in its figures).
    pub fn numbering_on(&self, i: usize) -> Numbering {
        if self.spans.get(i).is_some_and(|s| s.contents.is_some()) {
            return self.setup.contents_numbers;
        }
        let pinned = self.number_marks.iter().filter(|m| self.page_of(m.at) == i).max_by_key(|m| m.at);
        pinned.map_or_else(
            || {
                let n = self.page_number(i);
                Numbering { counting: if n.shown { Counting::On } else { Counting::Off }, figures: n.figures }
            },
            |m| m.numbering,
        )
    }

    /// Set how numbering goes from page `i` on. On the contents it is their setting; on a page of
    /// the story it is pinned to the page's first char, in place of any change on that page, and
    /// left out if it changes nothing. Not undoable, like the page settings (R1).
    pub fn set_numbering(&mut self, i: usize, numbering: Numbering) {
        let Some(span) = self.spans.get(i).copied() else { return };
        if span.contents.is_some() {
            self.setup.contents_numbers = numbering;
        } else {
            let marks = std::mem::take(&mut self.number_marks);
            let others = marks.into_iter().filter(|m| self.page_of(m.at) != i).collect();
            self.number_marks = others;
            let without = self.page_numbers();
            self.number_marks.push(NumberMark { at: span.start, numbering });
            self.number_marks.sort_by_key(|m| m.at);
            if self.page_numbers() == without {
                self.number_marks.retain(|m| m.at != span.start);
            }
        }
        self.version += 1;
    }

    /// Take off the change of numbering on page `i`: the pages go on as before it.
    pub fn clear_numbering(&mut self, i: usize) {
        if self.spans.get(i).is_some_and(|s| s.contents.is_some()) {
            self.setup.contents_numbers = Numbering::default();
        } else {
            let marks = std::mem::take(&mut self.number_marks);
            let others = marks.into_iter().filter(|m| self.page_of(m.at) != i).collect();
            self.number_marks = others;
        }
        self.version += 1;
    }

    /// The pages on which numbering changes, with the change: the contents', then the story's.
    pub fn numbering_changes(&self) -> Vec<(usize, Numbering)> {
        let mut out = Vec::new();
        if self.spans.first().is_some_and(|s| s.contents.is_some()) && !self.setup.contents_numbers.is_usual() {
            out.push((0, self.setup.contents_numbers));
        }
        out.extend(self.number_marks.iter().map(|m| (self.page_of(m.at), m.numbering)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{Edit, Piece};
    use crate::layout::with_ctx;
    use crate::model::{Flow, PAGE_BREAK, Style};

    /// The front matter, two chapters and an appendix, each on a page of its own after the contents.
    fn book(ctx: &eframe::egui::Context) -> Doc {
        let mut d = Doc::new();
        let text = format!("Front{PAGE_BREAK}One{PAGE_BREAK}Two{PAGE_BREAK}Appendix\n");
        d.flow = Flow { styles: vec![Style::new("x"); text.chars().count()], text };
        d.setup.contents = true;
        d.full_paginate(ctx, &Style::new("x"));
        d
    }

    fn shown(d: &Doc) -> Vec<Option<String>> {
        d.page_numbers().iter().map(|n| n.label()).collect()
    }

    #[test]
    fn numbering_holds_from_the_page_it_is_set_on_until_the_next_change() {
        with_ctx(|ctx| {
            let mut d = book(ctx);
            let s = |t: &str| Some(t.to_owned());
            assert_eq!(shown(&d), [s("1"), s("2"), s("3"), s("4"), s("5")]);
            // Front matter in roman numerals after the contents, the story from 1, no numbers on the appendix.
            d.set_numbering(0, Numbering { counting: Counting::Off, figures: Figures::Arabic });
            d.set_numbering(1, Numbering { counting: Counting::On, figures: Figures::Roman });
            d.set_numbering(2, Numbering { counting: Counting::From(1), figures: Figures::Arabic });
            d.set_numbering(4, Numbering { counting: Counting::Off, figures: Figures::Arabic });
            assert_eq!(shown(&d), [None, s("ii"), s("1"), s("2"), None]);
            assert_eq!(d.numbering_on(3), Numbering::default(), "counting on, as in force there");
            assert_eq!(d.numbering_on(4).counting, Counting::Off);

            // Set again where nothing changes, nothing is kept; a change is taken off again.
            let marks = d.number_marks.clone();
            d.set_numbering(3, Numbering::default());
            assert_eq!(d.number_marks, marks);
            // Without its own, a page goes on as the one before: here, as the contents, with none.
            d.clear_numbering(1);
            assert_eq!(shown(&d), [None, None, s("1"), s("2"), None]);

            // Changes go with their text: a page more before the chapters moves them along.
            d.apply(Edit::Replace { at: 2, old: Piece::default(), new: Piece { text: PAGE_BREAK.into(), styles: vec![Style::new("x")] } }, 0.0);
            d.full_paginate(ctx, &Style::new("x"));
            assert_eq!(shown(&d), [None, None, None, s("1"), s("2"), None]);
        });
    }

    #[test]
    fn a_number_is_read_in_figures_or_roman_numerals() {
        assert_eq!(parse("12"), Some(12));
        assert_eq!(parse(" xiv "), Some(14));
        assert_eq!(parse("MCMXC"), Some(1990));
        assert_eq!(parse("iiv"), None, "not how it is written");
        assert_eq!(parse("seven"), None);
    }

    #[test]
    fn roman_numerals_are_written_small() {
        let r: Vec<String> = [1, 4, 9, 14, 40, 90, 1999].into_iter().map(roman).collect();
        assert_eq!(r, ["i", "iv", "ix", "xiv", "xl", "xc", "mcmxcix"]);
    }
}
