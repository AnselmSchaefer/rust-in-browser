//! What next: a window on the lower left where Claude lays out ways the story could go on from the
//! caret. It reads the story just before the caret, the story notes and, if the writer leaves it
//! ticked, the scratchpad, and offers a handful of developments that differ from each other, from
//! a quiet one to a wild card. An idea worth keeping goes into the scratchpad; "More like this"
//! asks again in its direction.
//!
//! What is read is named on the button before it is sent. The ideas are not saved: what is kept
//! is what the writer puts into the scratchpad.

use eframe::egui::{self, Color32, Id, RichText};

use crate::App;
use crate::claude::{Pending, json_list};
use crate::review::left_dock;
use crate::theme::pal;

/// At most this many chars of the story before the caret are read.
const BACK: usize = 16_000;

const SYSTEM: &str = "You are an imaginative story consultant working with a writer in a word processor. \
    You know how stories are built, and you love surprising but earned turns.";

/// One way the story could go on.
pub struct Idea {
    pub title: String,
    pub what: String,
    /// What it opens up, or why it would grip the reader.
    pub hook: String,
}

pub struct Ideas {
    pub open: bool,
    pending: Option<Pending>,
    /// What the ideas were last asked with.
    pub asked: String,
    pub ideas: Vec<Idea>,
    /// Where the writer would like the story to go, sent with the request.
    pub steer: String,
    /// The scratchpad is sent with the request.
    pub use_scratchpad: bool,
    error: Option<String>,
}

impl Default for Ideas {
    fn default() -> Self {
        let (pending, asked, ideas, steer, error) = (None, String::new(), Vec::new(), String::new(), None);
        Ideas { open: false, pending, asked, ideas, steer, use_scratchpad: true, error }
    }
}

/// The ideas in Claude's reply.
fn parse(reply: &str) -> Vec<Idea> {
    let field = |v: &serde_json::Value, k: &str| v[k].as_str().unwrap_or_default().trim().to_owned();
    json_list(reply)
        .iter()
        .map(|v| Idea { title: field(v, "title"), what: field(v, "what"), hook: field(v, "hook") })
        .filter(|i| !i.title.is_empty() || !i.what.is_empty())
        .collect()
}

fn instruction() -> &'static str {
    "The story so far ends where the writer's caret is. Propose five ways it could go on from there: \
    developments for the next scenes, not a plan of the whole book. Make them really differ from each \
    other: one quiet and inward, one that raises the stakes, one that deepens or tests a character, one \
    that pays off or twists something already set up, and one wild card that is bold but could still \
    belong to this story. Build on the characters, threads and open questions in the text, and on the \
    writer's notes and scratchpad if there are any; follow the writer's wish if there is one. Be \
    concrete and vivid: name who does what, and where. Do not write the scenes themselves.\n\
    Reply with only a JSON array of objects with:\n\
    \"title\": a few evocative words;\n\
    \"what\": two to four sentences on what happens;\n\
    \"hook\": one sentence on what it opens up or why the reader will want to know more.\n\
    Write them in the language of the story."
}

impl App {
    pub fn toggle_ideas(&mut self) {
        self.ideas.open = !self.ideas.open;
    }

    /// Where the story read for ideas begins: whole paragraphs, at most `BACK` chars before the caret.
    pub fn ideas_from(&self) -> usize {
        match self.caret.checked_sub(BACK) {
            None => 0,
            // The next paragraph, so as not to go over.
            Some(c) => (self.doc.term_from(c, self.doc.char_to_byte(c)).0 + 1).min(self.caret),
        }
    }

    /// Ask for ideas for what comes after the caret.
    pub fn start_ideas(&mut self, ctx: &egui::Context) {
        let story = self.passage(self.ideas_from(), self.caret);
        let tagged = |tag: &str, s: &str| match s.trim() {
            "" => String::new(),
            s => format!("<{tag}>\n{s}\n</{tag}>\n\n"),
        };
        let pad = if self.ideas.use_scratchpad { tagged("writers_scratchpad", &self.doc.scratchpad) } else { String::new() };
        let prompt = format!(
            "{}{pad}<story_so_far>\n{}\n</story_so_far>\n\n{}{}",
            tagged("story_notes", &self.doc.story_notes),
            story.trim_end(),
            tagged("writers_wish", &self.ideas.steer),
            instruction()
        );
        self.ideas.pending = Some(Pending::start(ctx, SYSTEM, prompt.clone(), "medium"));
        (self.ideas.asked, self.ideas.error) = (prompt, None);
    }

    /// The ideas arrive, as if from Claude.
    #[cfg(test)]
    pub fn fake_ideas(&mut self, reply: &str) {
        self.ideas.pending = Some(Pending::answered(reply));
    }

    /// Put an idea at the end of the scratchpad.
    fn keep_idea(&mut self, k: usize) {
        let Some(idea) = self.ideas.ideas.get(k) else { return };
        let pad = &mut self.doc.scratchpad;
        let gap = match pad.trim_end().len() {
            0 => "",
            _ if pad.ends_with("\n\n") => "",
            _ if pad.ends_with('\n') => "\n",
            _ => "\n\n",
        };
        *pad += &format!("{gap}Idea: {}\n{}\n", idea.title, idea.what);
        self.status = "- kept in the scratchpad".into();
    }

    /// The window of ideas, if it is open: on the left, below the review.
    pub fn ideas_window(&mut self, ctx: &egui::Context) {
        if let Some(reply) = self.ideas.pending.as_ref().and_then(Pending::poll) {
            self.ideas.pending = None;
            match reply.map(|r| parse(&r)) {
                Ok(ideas) if ideas.is_empty() => self.ideas.error = Some("Claude's reply held no ideas; try again.".into()),
                Ok(ideas) => self.ideas.ideas = ideas,
                Err(e) => self.ideas.error = Some(e),
            }
        }
        if !self.ideas.open {
            return;
        }
        let words = self.passage(self.ideas_from(), self.caret).split_whitespace().count();
        let mut open = true;
        let (mut ask, mut keep, mut more) = (false, None, None);
        egui::Window::new("What next")
            .id(Id::new("ideas"))
            .open(&mut open)
            .default_rect(left_dock(ctx, true))
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                let busy = self.ideas.pending.is_some();
                let hint = "Where should it go? (optional: darker, more about Mia, a twist…)";
                ui.add(egui::TextEdit::singleline(&mut self.ideas.steer).hint_text(hint).desired_width(f32::INFINITY));
                ui.horizontal(|ui| {
                    let tip = format!("Claude reads the story before the caret ({words} words) and the story notes");
                    if ui.add_enabled(!busy && words > 0, egui::Button::new("Ideas for what comes next")).on_hover_text(tip).clicked() {
                        ask = true;
                    }
                    if busy {
                        ui.spinner();
                    }
                });
                let tip = "Send the scratchpad with it, so the ideas can use what you collected there";
                ui.checkbox(&mut self.ideas.use_scratchpad, "Use the scratchpad").on_hover_text(tip);
                if let Some(e) = &self.ideas.error {
                    ui.colored_label(Color32::from_rgb(235, 110, 110), e);
                }
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    for (k, idea) in self.ideas.ideas.iter().enumerate() {
                        ui.push_id(k, |ui| {
                            ui.label(RichText::new(&idea.title).strong().size(15.0).color(pal().accent));
                            ui.label(&idea.what);
                            if !idea.hook.is_empty() {
                                ui.label(RichText::new(&idea.hook).italics().size(12.5).color(pal().text_dim));
                            }
                            ui.horizontal(|ui| {
                                if ui.button("To scratchpad").on_hover_text("Keep this idea at the end of the scratchpad").clicked() {
                                    keep = Some(k);
                                }
                                let tip = "Ask again for ideas that go further this way";
                                if ui.add_enabled(!busy, egui::Button::new("More like this")).on_hover_text(tip).clicked() {
                                    more = Some(k);
                                }
                            });
                        });
                        ui.add_space(10.0);
                    }
                });
            });
        self.ideas.open = open;
        if ask {
            self.start_ideas(ctx);
        } else if let Some(k) = keep {
            self.keep_idea(k);
        } else if let Some(k) = more {
            let idea = &self.ideas.ideas[k];
            self.ideas.steer = format!("More in this direction: {} ({})", idea.title, idea.what);
            self.start_ideas(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ideas_are_read_from_the_reply_whatever_is_around_it() {
        let reply = "Sure!\n[{\"title\": \"The hatch\", \"what\": \"Mia climbs up.\", \"hook\": \"Who left it open?\"},\
            {\"title\": \"\", \"what\": \"\"}, {\"what\": \"Edwin lies.\"}]";
        let ideas = parse(reply);
        let got: Vec<(&str, &str, &str)> = ideas.iter().map(|i| (i.title.as_str(), i.what.as_str(), i.hook.as_str())).collect();
        assert_eq!(got, [("The hatch", "Mia climbs up.", "Who left it open?"), ("", "Edwin lies.", "")]);
        assert!(parse("I can't think of anything.").is_empty());
    }
}
