//! The review: a window on the left where Claude reads a stretch of the story and lists what it
//! finds, from spelling and grammar to style, places that want more detail and places that say too
//! much. Each finding quotes the words it is about, so a click finds them in the text however it
//! has been edited since, and one with a correction can be applied (one undo step).
//!
//! What is read is named on the button before it is sent: the selection, or else the chapter the
//! caret is in. The findings are not saved; they belong to the text as it was when read.

use eframe::egui::{self, Color32, Id, Rect, RichText, pos2, vec2};

use crate::App;
use crate::claude::{Pending, json_list};
use crate::theme::pal;

/// At most this many chars are read at once; a longer chapter is read around the caret.
const MOST: usize = 24_000;

const SYSTEM: &str = "You are a careful editor helping a writer in a word processor. You read their \
    text closely and point out only what is worth changing.";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Spelling,
    Grammar,
    Style,
    Detail,
    Verbose,
}

impl Kind {
    const ALL: [Kind; 5] = [Kind::Spelling, Kind::Grammar, Kind::Style, Kind::Detail, Kind::Verbose];

    fn name(self) -> &'static str {
        match self {
            Kind::Spelling => "Spelling",
            Kind::Grammar => "Grammar",
            Kind::Style => "Style",
            Kind::Detail => "More detail",
            Kind::Verbose => "Too wordy",
        }
    }

    fn from_key(key: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| format!("{k:?}").eq_ignore_ascii_case(key.trim()))
    }

    fn color(self) -> Color32 {
        match self {
            Kind::Spelling => Color32::from_rgb(235, 110, 110),
            Kind::Grammar => Color32::from_rgb(240, 170, 90),
            Kind::Style => Color32::from_rgb(143, 162, 255),
            Kind::Detail => Color32::from_rgb(110, 200, 150),
            Kind::Verbose => Color32::from_rgb(200, 150, 230),
        }
    }
}

/// One thing the review found.
pub struct Issue {
    pub kind: Kind,
    /// The words it is about, as they stand in the text.
    pub quote: String,
    /// What to put in their place; empty when the finding is advice only.
    pub fix: String,
    pub why: String,
}

#[derive(Default)]
pub struct Review {
    pub open: bool,
    pending: Option<Pending>,
    /// What the review last sent, and where the stretch it read began (to find quotes near it).
    pub asked: String,
    from: usize,
    pub issues: Vec<Issue>,
    /// The review has an answer to show (none found is an answer too).
    read: bool,
    error: Option<String>,
}

/// The findings in Claude's reply.
fn parse(reply: &str) -> Vec<Issue> {
    let field = |v: &serde_json::Value, k: &str| v[k].as_str().unwrap_or_default().trim().to_owned();
    json_list(reply)
        .iter()
        .filter_map(|v| {
            let kind = Kind::from_key(v["kind"].as_str()?)?;
            let quote = field(v, "quote");
            let fix = if kind == Kind::Detail { String::new() } else { field(v, "fix") };
            let fix = if fix == quote { String::new() } else { fix };
            (!quote.is_empty()).then(|| Issue { kind, quote, fix, why: field(v, "why") })
        })
        .collect()
}

fn instruction() -> &'static str {
    "Review the passage above. Look for:\n\
    - spelling: misspelt or mistyped words\n\
    - grammar: grammar and punctuation mistakes\n\
    - style: clumsy, unclear or repetitive wording, cliches, a slip in tense or voice\n\
    - detail: places that go by too fast or too thinly and want more detail, so the reader can see, \
      feel or follow them\n\
    - verbose: places that say too much, explain what is already clear, or drag\n\
    Leave alone what is a deliberate choice of the writer's voice, and dialect or slang in dialogue.\n\
    Reply with only a JSON array, most important first, at most 40 entries, each an object with:\n\
    \"kind\": one of \"spelling\", \"grammar\", \"style\", \"detail\", \"verbose\";\n\
    \"quote\": the words concerned, copied exactly as they stand in the passage, character for character, \
    as short as will still be found (a word, a phrase, at most one or two sentences);\n\
    \"fix\": the text to put in place of the quote, or \"\" if this is advice that the writer must carry out \
    (always \"\" for detail);\n\
    \"why\": one short sentence saying what is wrong or what is missing, in the language of the passage.\n\
    Reply [] if there is nothing worth changing."
}

/// Where a window docked on the left goes: the review above, the ideas below (`lower`).
pub fn left_dock(ctx: &egui::Context, lower: bool) -> Rect {
    let screen = ctx.content_rect();
    let tall = (screen.height() - 160.0).clamp(240.0, 720.0);
    let h = (tall - 16.0) / 2.0;
    let top = screen.top() + 70.0 + if lower { h + 16.0 } else { 0.0 };
    Rect::from_min_size(pos2(screen.left() + 24.0, top), vec2(340.0, h))
}

impl App {
    pub fn toggle_review(&mut self) {
        self.review.open = !self.review.open;
    }

    /// What the review would read now, and what to call it: the selection, or else the chapter the
    /// caret is in (the whole story if it has no chapters), at most `MOST` chars of it around the caret.
    pub fn review_stretch(&self) -> (usize, usize, &'static str) {
        if self.has_selection() {
            let (a, b) = self.selection();
            return (a, b.min(a + MOST), "the selection");
        }
        let starts: Vec<usize> = self.doc.chapters(false).iter().map(|e| e.at).collect();
        let end = self.doc.total_chars() - 1;
        let a = starts.iter().copied().rfind(|&s| s <= self.caret).unwrap_or(0);
        let b = starts.iter().copied().find(|&s| s > self.caret).unwrap_or(end);
        let what = if starts.is_empty() { "the story" } else { "this chapter" };
        if b - a <= MOST {
            return (a, b, what);
        }
        // Too long to read at once: whole paragraphs around the caret.
        let from = self.doc.para_start(self.caret.saturating_sub(MOST / 2).max(a)).0.max(a);
        let to = (from + MOST).min(b);
        let to = if to < b { self.doc.para_start(to).0.max(from + 1) } else { to };
        (from, to, "the pages around the caret")
    }

    /// Send the stretch the button names to be reviewed.
    pub fn start_review(&mut self, ctx: &egui::Context) {
        let (a, b, _) = self.review_stretch();
        let notes = match self.doc.story_notes.trim() {
            "" => String::new(),
            n => format!("<story_notes>\n{n}\n</story_notes>\n\n"),
        };
        let prompt = format!("{notes}<passage>\n{}\n</passage>\n\n{}", self.passage(a, b), instruction());
        self.review.pending = Some(Pending::start(ctx, SYSTEM, prompt.clone(), "medium"));
        (self.review.asked, self.review.from, self.review.error) = (prompt, a, None);
    }

    /// The review's answer arrives, as if from Claude.
    #[cfg(test)]
    pub fn fake_review(&mut self, reply: &str) {
        self.review.pending = Some(Pending::answered(reply));
    }

    /// Where `quote` is in the text now (chars), the place nearest `near` if it is there more than once.
    pub fn find_quote(&self, quote: &str, near: usize) -> Option<(usize, usize)> {
        let text = &self.doc.flow.text;
        let n = quote.chars().count();
        let mut c = 0; // chars before `last`
        let mut last = 0;
        let mut best: Option<usize> = None;
        for (b, _) in text.match_indices(quote) {
            c += text[last..b].chars().count();
            last = b;
            if best.is_none_or(|k| c.abs_diff(near) < k.abs_diff(near)) {
                best = Some(c);
            }
        }
        best.map(|a| (a, a + n))
    }

    /// The review window, if it is open: on the left, above the ideas.
    pub fn review_window(&mut self, ctx: &egui::Context) {
        if let Some(reply) = self.review.pending.as_ref().and_then(Pending::poll) {
            self.review.pending = None;
            self.review.read = true;
            match reply {
                Ok(r) => self.review.issues = parse(&r),
                Err(e) => (self.review.issues, self.review.error) = (Vec::new(), Some(e)),
            }
        }
        if !self.review.open {
            return;
        }
        let (_, _, what) = self.review_stretch();
        let mut open = true;
        let (mut check, mut show, mut apply, mut drop) = (false, None, None, None);
        egui::Window::new("Review")
            .id(Id::new("review"))
            .open(&mut open)
            .default_rect(left_dock(ctx, false))
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                let busy = self.review.pending.is_some();
                ui.horizontal(|ui| {
                    let tip = "Claude reads it for spelling, grammar and style, and for places that want more detail or say too much";
                    if ui.add_enabled(!busy, egui::Button::new(format!("Check {what}"))).on_hover_text(tip).clicked() {
                        check = true;
                    }
                    if busy {
                        ui.spinner();
                    }
                });
                if let Some(e) = &self.review.error {
                    ui.colored_label(Color32::from_rgb(235, 110, 110), e);
                }
                if self.review.read && self.review.issues.is_empty() && self.review.error.is_none() && !busy {
                    ui.label(RichText::new("Nothing to change.").color(pal().text_dim));
                }
                if !self.review.issues.is_empty() {
                    let counts: Vec<String> = Kind::ALL
                        .iter()
                        .filter_map(|&k| match self.review.issues.iter().filter(|i| i.kind == k).count() {
                            0 => None,
                            n => Some(format!("{n} {}", k.name().to_lowercase())),
                        })
                        .collect();
                    ui.label(RichText::new(counts.join(" · ")).size(12.0).color(pal().text_dim));
                }
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    for (k, issue) in self.review.issues.iter().enumerate() {
                        let found = self.find_quote(&issue.quote, self.review.from);
                        ui.push_id(k, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(issue.kind.name()).size(11.0).strong().color(issue.kind.color()));
                                // The quote is a link to its place in the text.
                                let quote = RichText::new(format!("“{}”", issue.quote)).italics();
                                let link = ui.add_enabled(found.is_some(), egui::Button::new(quote).frame(false).wrap());
                                let tip = if found.is_some() { "Show it in the text" } else { "No longer in the text" };
                                if link.on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
                                    show = found;
                                }
                            });
                            if !issue.fix.is_empty() {
                                ui.label(format!("→ {}", issue.fix));
                            }
                            if !issue.why.is_empty() {
                                ui.label(RichText::new(&issue.why).size(12.5).color(pal().text_dim));
                            }
                            ui.horizontal(|ui| {
                                if !issue.fix.is_empty() {
                                    let tip = "Put the correction in place of the quoted words (Ctrl+Z undoes it)";
                                    if ui.add_enabled(found.is_some(), egui::Button::new("Apply")).on_hover_text(tip).clicked() {
                                        apply = found.map(|f| (k, f));
                                    }
                                }
                                if ui.button("Dismiss").clicked() {
                                    drop = Some(k);
                                }
                            });
                        });
                        ui.add_space(8.0);
                    }
                });
            });
        self.review.open = open;
        if check {
            self.start_review(ctx);
        } else if let Some((a, b)) = show {
            self.set_caret(ctx, a, false);
            self.set_caret(ctx, b, true);
        } else if let Some((k, (a, b))) = apply {
            let fix = self.review.issues.remove(k).fix;
            self.set_caret(ctx, a, false);
            self.replace_range(ctx, a, b, &fix);
        } else if let Some(k) = drop {
            self.review.issues.remove(k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn findings_are_read_from_the_reply_whatever_is_around_it() {
        let reply = "Here you go:\n```json\n[{\"kind\": \"Spelling\", \"quote\": \"teh\", \"fix\": \"the\", \"why\": \"Typo.\"},\
            {\"kind\": \"detail\", \"quote\": \"They fought.\", \"fix\": \"They fought hard.\", \"why\": \"Too quick.\"},\
            {\"kind\": \"mood\", \"quote\": \"x\"}, {\"kind\": \"style\", \"quote\": \"\"},\
            {\"kind\": \"verbose\", \"quote\": \"very very\", \"fix\": \"very very\"}]\n```";
        let issues = parse(reply);
        let got: Vec<(Kind, &str, &str)> = issues.iter().map(|i| (i.kind, i.quote.as_str(), i.fix.as_str())).collect();
        // Detail is advice only, an unknown kind or empty quote is left out, a fix that changes nothing is none.
        assert_eq!(got, [(Kind::Spelling, "teh", "the"), (Kind::Detail, "They fought.", ""), (Kind::Verbose, "very very", "")]);
        assert!(parse("Nothing to change.").is_empty());
    }
}
