//! The question asked before leaving a story (closing Caprice, or opening another) when it holds
//! changes: save them or not, or, for a story that saves itself, keep them or drop them.
//!
//! Both are one dialog: the story's first page and name, what is at stake (its words and chapters,
//! how much changed since it was last saved and when), and that a version is kept whichever is
//! chosen, then the way on, the way that throws away, and back to writing.

use eframe::egui::{self, Color32, CornerRadius, FontId, Id, RichText, Sense, Stroke, vec2};

use crate::App;
use crate::fileio::{DocFile, Leaving};
use crate::fonts::family_for;
use crate::start::{Kind, Sheet};
use crate::theme::pal;

/// The colour of the button that throws changes away.
const WARN: Color32 = Color32::from_rgb(236, 128, 116);

/// What the dialog tells about the story, worked out once for each state of it.
#[derive(Clone)]
pub struct About {
    words: usize,
    chapters: usize,
    /// Its words when it was last saved by hand (or opened), if it ever was.
    saved_words: Option<usize>,
    sheet: Sheet,
}

/// What was answered.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Answer {
    /// Save, or keep the changes.
    Go,
    /// Don't save, or drop the changes.
    Lose,
    Back,
}

/// "+312 words", "40 words fewer", for a change in words.
fn word_change(now: usize, then: usize) -> String {
    let n = |k: usize| format!("{} word{}", crate::words::grouped(k), if k == 1 { "" } else { "s" });
    match now.cmp(&then) {
        std::cmp::Ordering::Greater => format!("+{}", n(now - then)),
        std::cmp::Ordering::Less => format!("{} fewer", n(then - now)),
        std::cmp::Ordering::Equal => "Changed".to_owned(),
    }
}

impl App {
    /// A file name for a story never saved: its first chapter's title, else its first words.
    pub fn suggested_name(&self) -> String {
        let title = self.doc.chapters(false).into_iter().map(|e| e.title).find(|t| !t.trim().is_empty());
        let words = || self.doc.flow.text.split_whitespace().take(5).collect::<Vec<_>>().join(" ");
        let name = title.unwrap_or_else(words);
        // Characters no file system takes, and a name too long to read, are left out.
        let clean: String = name.chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control()).collect();
        let clean = clean.trim().trim_end_matches('.').chars().take(60).collect::<String>();
        if clean.trim().is_empty() { "Untitled".to_owned() } else { clean.trim().to_owned() }
    }

    fn about_story(&mut self) -> About {
        if let Some((v, about)) = &self.leave_about
            && *v == self.doc.version
        {
            return about.clone();
        }
        let saved_words = self.as_saved.as_ref().and_then(|(_, j)| serde_json::from_str::<DocFile>(j).ok()).map(|f| f.words());
        let seed = self.story_id.bytes().fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(b.into()));
        let about = About { words: self.story_words(), chapters: self.doc.chapters(false).len(), saved_words, sheet: Sheet::of_doc(&self.doc, seed) };
        self.leave_about = Some((self.doc.version, about.clone()));
        about
    }

    /// The "Save changes?" question, while something waits on it.
    pub fn unsaved_prompt(&mut self, ctx: &egui::Context) {
        let Some(leaving) = self.leaving else { return };
        if self.path.is_some() && !self.unsaved() {
            return self.keep_or_drop_prompt(ctx, leaving);
        }
        let about = self.about_story();
        let now = self.keep.now();
        let then = match leaving {
            Leaving::Open => "before opening another story",
            Leaving::Close => "before Caprice closes",
        };
        let (title, said, change) = match &self.path {
            None => (
                format!("Save \u{201c}{}\u{201d}?", self.suggested_name()),
                format!("This story has never been saved. Save it as a file {then}, or it is gone from your files."),
                format!("Begun {}", crate::versions::ago(self.created, now)),
            ),
            Some(_) => (
                format!("Save changes to \u{201c}{}\u{201d}?", self.story_name()),
                format!("Your changes are not in the file yet. Save them {then}, or they are lost from it."),
                self.change_since_saved(&about, now),
            ),
        };
        let primary = if self.path.is_some() { "Save" } else { "Save\u{2026}" };
        let kept = "Not saving still keeps this version under File \u{2192} Versions.";
        let answer = self.leave_dialog(ctx, "unsaved_prompt", &about, [&title, &said, &change, kept], [primary, "Don\u{2019}t save"]);
        match answer {
            Some(Answer::Go) => {
                self.leaving = None;
                match self.path.clone() {
                    Some(path) => {
                        self.save_to(path);
                        if !self.unsaved() {
                            self.leave(ctx, leaving);
                        }
                    }
                    None => {
                        self.after_save = Some(leaving);
                        self.save(ctx, true);
                    }
                }
            }
            Some(Answer::Lose) => {
                self.leaving = None;
                // Whatever was said, the story as it is stays among the versions.
                self.keep_now();
                self.leave(ctx, leaving);
            }
            Some(Answer::Back) => self.leaving = None,
            None => {}
        }
    }

    /// A story that saves itself, leaving with changes since it was saved by hand: keep them, or
    /// drop them back to that.
    pub fn keep_or_drop_prompt(&mut self, ctx: &egui::Context, leaving: Leaving) {
        let about = self.about_story();
        let now = self.keep.now();
        let title = format!("Keep the changes to \u{201c}{}\u{201d}?", self.story_name());
        let said = "They are saved in the file already. Dropping them puts the story back as you last saved it yourself.";
        let change = self.change_since_saved(&about, now);
        let kept = "Dropped changes stay under File \u{2192} Versions.";
        match self.leave_dialog(ctx, "keep_or_drop", &about, [&title, said, &change, kept], ["Keep changes", "Drop changes"]) {
            Some(answer @ (Answer::Go | Answer::Lose)) => {
                self.leaving = None;
                if answer == Answer::Lose {
                    self.drop_changes(ctx);
                }
                self.leave(ctx, leaving);
            }
            Some(Answer::Back) => self.leaving = None,
            None => {}
        }
    }

    /// "+312 words since you saved 12 minutes ago."
    fn change_since_saved(&self, about: &About, now: u64) -> String {
        let when = self.saved_at.map_or(String::new(), |t| format!(" {}", crate::versions::ago(t, now)));
        match about.saved_words {
            Some(then) => format!("{} since you saved it{when}", word_change(about.words, then)),
            None => format!("Changed since you saved it{when}"),
        }
    }

    /// The dialog: `lines` are its title, what it means, what changed, and what is kept either way.
    /// Enter takes the first of `buttons`, unless the keyboard is on another; Escape goes back.
    fn leave_dialog(&mut self, ctx: &egui::Context, id: &str, about: &About, lines: [&str; 4], buttons: [&str; 2]) -> Option<Answer> {
        let [title, said, change, kept] = lines;
        let serif = family_for(&Kind::Novel.font(&self.fonts), false, false).0;
        let versions = self.keep.dir.is_some();
        let frame = egui::Frame::new().fill(pal().dock).stroke(Stroke::new(1.0, pal().dock_edge)).corner_radius(CornerRadius::same(16)).inner_margin(24);
        let mut answer = None;
        let modal = egui::Modal::new(Id::new(id)).frame(frame).backdrop_color(Color32::from_black_alpha(150)).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.horizontal_top(|ui| {
                // The story's first page, as on the welcome page.
                let (r, _) = ui.allocate_exact_size(vec2(64.0, 88.0), Sense::hover());
                let w = about.sheet.width_for(r.height());
                let at = egui::Rect::from_center_size(r.center(), vec2(w, r.height()));
                about.sheet.draw(ui.painter(), at, 0.4, 1.0);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(RichText::new(title).font(FontId::new(21.0, serif)).color(pal().text)).wrap());
                    ui.add_space(2.0);
                    ui.add(egui::Label::new(RichText::new(said).color(pal().text_dim)).wrap());
                });
            });
            ui.add_space(14.0);

            // What is at stake.
            egui::Frame::new().fill(pal().desk.gamma_multiply(0.6)).corner_radius(CornerRadius::same(10)).inner_margin(12).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let mut holds = format!("{} words", crate::words::grouped(about.words));
                if about.chapters > 0 {
                    holds = format!("{holds}  \u{b7}  {} chapter{}", about.chapters, if about.chapters == 1 { "" } else { "s" });
                }
                let fact = |ui: &mut egui::Ui, dot: Color32, text: &str, color: Color32| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(8.0, 14.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 3.0, dot);
                        ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
                    });
                };
                fact(ui, pal().text_dim, &holds, pal().text);
                fact(ui, pal().accent, change, pal().text);
                if versions {
                    fact(ui, Color32::from_rgb(120, 190, 140), kept, pal().text_dim);
                }
            });
            ui.add_space(16.0);

            // Back to writing, the way that throws away, and the way on, the last on the right.
            // Built right to left, so the keyboard comes to the way on first.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let go = ui.add(egui::Button::new(RichText::new(buttons[0]).color(pal().desk).strong()).fill(pal().accent).min_size(vec2(96.0, 32.0)));
                let lose = ui.add(egui::Button::new(RichText::new(buttons[1]).color(WARN)).min_size(vec2(0.0, 32.0)));
                let back = ui.add(egui::Button::new("Cancel").frame(false).min_size(vec2(0.0, 32.0)));
                let other = lose.has_focus() || back.has_focus();
                let enter = !other && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                answer = if go.clicked() || enter {
                    Some(Answer::Go)
                } else if lose.clicked() {
                    Some(Answer::Lose)
                } else if back.clicked() {
                    Some(Answer::Back)
                } else {
                    None
                };
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let hint = format!("Enter: {}  \u{b7}  Esc: back to writing", buttons[0].trim_end_matches('\u{2026}').to_lowercase());
                    ui.label(RichText::new(hint).size(11.5).color(pal().text_dim.gamma_multiply(0.8)));
                });
            });
        });
        if answer.is_none() && modal.should_close() {
            answer = Some(Answer::Back);
        }
        answer
    }
}
