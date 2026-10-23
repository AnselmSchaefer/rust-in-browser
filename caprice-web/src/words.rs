//! Document info: what the story is, when it was made and saved, what it holds (words of the
//! story, the caret's chapter and the selection, and more), and how many words were written today
//! towards a daily goal. In a window of its own (File > Document info), not always in sight.
//!
//! The goal is the writer's, not a story's: it and each story's count at the start of the day are
//! kept in the app's data folder (`progress.json`), never in the story's file, so counting does
//! not make a story unsaved. Today's words are those written in every story since each was first
//! seen today, as last counted (words deleted count against them).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use eframe::egui::{self, Color32, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::App;
use crate::model::Doc;
use crate::theme::pal;

/// How often (seconds) today's words are counted and kept.
const TRACK_EVERY: f64 = 5.0;

/// Words in `text`: runs between spaces with a letter or a digit in them (a dash alone is none).
pub fn count(text: &str) -> usize {
    text.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count()
}

impl Doc {
    /// Words in chars `a..b`.
    pub fn words_in(&self, a: usize, b: usize) -> usize {
        let (ba, bb) = (self.char_to_byte(a), self.char_to_byte(b.max(a)));
        count(&self.flow.text[ba..bb])
    }

    /// The caret's chapter, from its title to the next (None in a story without chapters).
    pub fn chapter_around(&self, c: usize) -> Option<(usize, usize)> {
        let starts: Vec<usize> = self.chapters(false).iter().map(|e| e.at).collect();
        starts.first()?;
        // Before the first title, the chapter is what comes before it.
        let a = starts.iter().copied().rfind(|&s| s <= c).unwrap_or(0);
        let b = starts.iter().copied().find(|&s| s > c).unwrap_or(self.total_chars());
        Some((a, b))
    }
}

/// The clock's offset from UTC in seconds, where the writer is, asked of the system once.
pub(crate) fn offset() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        let out = std::process::Command::new("date").arg("+%z").output().ok();
        let z = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default();
        utc_offset(&z).unwrap_or(0)
    })
}

/// Days since 1970 where the writer is.
pub fn today(unix: u64) -> i64 {
    (unix as i64 + offset()).div_euclid(86_400)
}

/// Seconds east of UTC in `date +%z` output ("+0200", "-0530").
fn utc_offset(z: &str) -> Option<i64> {
    let (sign, digits) = match z.as_bytes().first()? {
        b'+' => (1, &z[1..]),
        b'-' => (-1, &z[1..]),
        _ => return None,
    };
    if digits.len() != 4 {
        return None;
    }
    let (h, m): (i64, i64) = (digits[..2].parse().ok()?, digits[2..].parse().ok()?);
    Some(sign * (h * 3600 + m * 60))
}

/// A story's words today: its count when first seen today, and when last counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Day {
    pub day: i64,
    pub start: usize,
    pub last: usize,
}

/// The daily goal and each story's day, as kept in the data folder.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    /// Words a day to aim for (0: none).
    pub goal: usize,
    pub stories: HashMap<String, Day>,
}

impl Progress {
    /// Words written today in every story.
    pub fn today(&self, day: i64) -> isize {
        self.stories.values().filter(|d| d.day == day).map(|d| d.last as isize - d.start as isize).sum()
    }

    /// Count `words` for story `id` on `day`; whether anything changed.
    pub fn track(&mut self, id: &str, day: i64, words: usize) -> bool {
        let entry = self.stories.entry(id.to_owned()).or_default();
        let next = if entry.day == day { Day { last: words, ..*entry } } else { Day { day, start: words, last: words } };
        let changed = next != *entry;
        *entry = next;
        // Days gone by are of no more use.
        self.stories.retain(|_, d| d.day == day);
        changed
    }
}

/// Counting words for the page bar, and keeping today's.
pub struct Words {
    /// Where the goal and today's counts are kept (None: only while Caprice runs).
    pub file: Option<PathBuf>,
    pub progress: Progress,
    /// The time in seconds since 1970, when a test says so; else the clock's.
    pub clock: Option<u64>,
    /// The story's words, and the document version they were counted for.
    story: Option<(u64, usize)>,
    tracked_at: f64,
    /// The Document info window shows.
    pub open: bool,
}

impl Words {
    pub fn new() -> Self {
        let file = crate::versions::default_dir().and_then(|d| d.parent().map(|p| p.join("progress.json")));
        let progress = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|j| serde_json::from_str(&j).ok());
        Self { file, progress: progress.unwrap_or_default(), clock: None, story: None, tracked_at: f64::NEG_INFINITY, open: false }
    }

    pub fn day(&self) -> i64 {
        let now = self.clock.unwrap_or_else(|| crate::web::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()));
        today(now)
    }

    fn save(&self) {
        if let (Some(file), Ok(json)) = (&self.file, serde_json::to_string(&self.progress)) {
            let _ = file.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(file, json);
        }
    }
}

/// "12,345": thousands apart, to read at a glance.
pub fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (k, c) in digits.chars().enumerate() {
        if k > 0 && (digits.len() - k).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl App {
    /// The story's words (counted again only when the text changed).
    pub fn story_words(&mut self) -> usize {
        match self.words.story {
            Some((v, n)) if v == self.doc.version => n,
            _ => {
                let n = self.doc.words_in(0, self.doc.total_chars());
                self.words.story = Some((self.doc.version, n));
                n
            }
        }
    }

    /// Words written today, in every story.
    pub fn words_today(&self) -> isize {
        self.words.progress.today(self.words.day())
    }

    /// Now and then: count this story's words towards today's, and keep them.
    pub fn track_words(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if now - self.words.tracked_at < TRACK_EVERY {
            return;
        }
        self.words.tracked_at = now;
        let (n, day, id) = (self.story_words(), self.words.day(), self.story_id.clone());
        if self.words.progress.track(&id, day, n) {
            self.words.save();
        }
    }

    /// Open the Document info window.
    pub fn open_info(&mut self) {
        self.words.open = true;
    }

    /// The Document info window, if open: what the story is, when it was made and saved, what it
    /// holds, and today's words towards the daily goal.
    pub fn info_window(&mut self, ctx: &egui::Context) {
        if !self.words.open {
            return;
        }
        let mut open = true;
        let mut goal = self.words.progress.goal;
        let mut versions = false;
        let screen = ctx.content_rect();
        let saved_at = self.path.as_ref().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok()).and_then(secs);
        let kept: Vec<u64> = self.keep.dir.as_ref().map_or(Vec::new(), |d| crate::versions::times_kept(&d.join(&self.story_id)));
        egui::Window::new("Document info")
            .id(egui::Id::new("document_info"))
            .open(&mut open)
            .default_pos(pos2(screen.center().x - 170.0, screen.top() + 90.0))
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let row = |ui: &mut egui::Ui, what: &str, value: String| {
                    ui.label(egui::RichText::new(what).color(pal().text_dim));
                    ui.label(value);
                    ui.end_row();
                };
                let grid = |ui: &mut egui::Ui, id: &str, rows: &mut dyn FnMut(&mut egui::Ui)| {
                    egui::Grid::new(id).num_columns(2).min_col_width(110.0).spacing(vec2(16.0, 6.0)).show(ui, rows);
                };
                grid(ui, "info_story", &mut |ui| {
                    row(ui, "Name", self.story_name());
                    let file = self.path.as_ref().map_or("Not saved yet".to_owned(), |p| p.display().to_string());
                    row(ui, "File", file);
                    row(ui, "Created", if self.created > 0 { date(self.created) } else { "Not known".into() });
                    row(ui, "Last saved", saved_at.map_or("Not saved yet".into(), date));
                    let versions_kept = match (kept.first(), kept.len()) {
                        (_, 0) => "None yet".to_owned(),
                        (Some(&oldest), n) => format!("{n}, the oldest {}", date(oldest)),
                        _ => String::new(),
                    };
                    ui.label(egui::RichText::new("Versions").color(pal().text_dim));
                    ui.horizontal(|ui| {
                        ui.label(versions_kept);
                        versions = ui.small_button("Show…").clicked();
                    });
                    ui.end_row();
                });
                ui.separator();
                let story = self.story_words();
                grid(ui, "info_text", &mut |ui| {
                    row(ui, "Words", grouped(story));
                    if let Some((a, b)) = self.doc.chapter_around(self.caret) {
                        row(ui, "This chapter", format!("{} words", grouped(self.doc.words_in(a, b))));
                    }
                    if self.has_selection() {
                        let (a, b) = self.selection();
                        row(ui, "Selection", format!("{} words", grouped(self.doc.words_in(a, b))));
                    }
                    let text = &self.doc.flow.text;
                    let chars = text.chars().filter(|&c| !crate::model::is_terminator(c) && c != crate::model::IMAGE_CHAR).count();
                    row(ui, "Characters", grouped(chars));
                    let paras = text.split(crate::model::is_terminator).filter(|p| !p.trim().is_empty()).count();
                    row(ui, "Paragraphs", grouped(paras));
                    row(ui, "Pages", grouped(self.doc.pages()));
                    let chapters = self.doc.chapters(false).len();
                    if chapters > 0 {
                        row(ui, "Chapters", grouped(chapters));
                    }
                    let d = &self.doc;
                    let around = [("Post-its", d.notes.len()), ("Scenes", d.scenes.len()), ("Pictures", d.images.len()), ("People in the cast", d.cast.len())];
                    for (what, n) in around {
                        if n > 0 {
                            row(ui, what, grouped(n));
                        }
                    }
                });
                ui.separator();
                grid(ui, "info_today", &mut |ui| {
                    let today = self.words_today();
                    let sign = if today < 0 { "\u{2212}" } else { "" };
                    let of = if goal > 0 { format!(" of {}", grouped(goal)) } else { String::new() };
                    row(ui, "Today", format!("{sign}{}{of} words", grouped(today.unsigned_abs())));
                    ui.label(egui::RichText::new("Daily goal").color(pal().text_dim));
                    ui.add(egui::DragValue::new(&mut goal).range(0..=100_000).speed(10.0).suffix(" words"))
                        .on_hover_text("0 for none. Today counts the words written in every story.");
                    ui.end_row();
                });
                if goal > 0 {
                    let done = (self.words_today().max(0) as f32 / goal as f32).min(1.0);
                    let reached = if done >= 1.0 { Color32::from_rgb(110, 190, 120) } else { pal().accent };
                    ui.add(egui::ProgressBar::new(done).fill(reached).desired_height(6.0));
                }
            });
        self.words.open = open;
        if goal != self.words.progress.goal {
            self.words.progress.goal = goal;
            self.words.save();
        }
        if versions {
            self.open_versions();
        }
    }
}

/// Seconds since 1970 of a file time.
fn secs(t: std::time::SystemTime) -> Option<u64> {
    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// "18 Oct 2026, 17:53", in the writer's time.
pub fn date(unix: u64) -> String {
    let local = unix as i64 + offset();
    let (days, rest) = (local.div_euclid(86_400), local.rem_euclid(86_400));
    let (y, m, d) = civil(days);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{d} {} {y}, {:02}:{:02}", MONTHS[(m - 1) as usize], rest / 3600, rest % 3600 / 60)
}

/// The year, month and day `days` after 1 January 1970 (Howard Hinnant's `civil_from_days`).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_runs_with_a_letter_or_digit_in_them() {
        assert_eq!(count("It was nearly midnight \u{2014} and 12 o'clock."), 7);
        assert_eq!(count("  \n\t "), 0);
        assert_eq!(count("Mara's"), 1);
    }

    #[test]
    fn the_carets_chapter_runs_from_its_title_to_the_next() {
        use crate::model::{ParaAttrs, Style, is_terminator};
        let mut d = Doc::new();
        let mut title = ParaAttrs::default();
        title.set_chapter_title(true);
        let plain = Style::new("x");
        d.flow.text.clear();
        d.flow.styles.clear();
        for (t, attrs) in [("Before\n", ParaAttrs::default()), ("One\n", title), ("a b c\n", ParaAttrs::default()), ("Two\n", title), ("d\n", ParaAttrs::default())] {
            d.flow.text.push_str(t);
            d.flow.styles.extend(t.chars().map(|c| if is_terminator(c) { plain.with_para(attrs) } else { plain.clone() }));
        }
        assert_eq!(d.chapter_around(9), Some((7, 17)), "One, a b c");
        assert_eq!(d.words_in(7, 17), 4);
        assert_eq!(d.chapter_around(2), Some((0, 7)), "before the first title");
        assert_eq!(d.chapter_around(18).map(|(a, _)| a), Some(17));
    }

    #[test]
    fn dates_are_told_as_day_month_year() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_744), (2026, 10, 18));
        assert_eq!(civil(11_016), (2000, 2, 29));
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1234), "1,234");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn the_clocks_offset_is_read_from_date() {
        assert_eq!(utc_offset("+0200"), Some(7200));
        assert_eq!(utc_offset("-0530"), Some(-19_800));
        assert_eq!(utc_offset("UTC"), None);
    }

    #[test]
    fn todays_words_count_from_each_storys_first_sight_today_and_start_again_tomorrow() {
        let mut p = Progress::default();
        assert!(p.track("a", 10, 1000));
        p.track("a", 10, 1250);
        p.track("b", 10, 300);
        p.track("b", 10, 260);
        assert_eq!(p.today(10), 250 - 40);
        assert!(!p.track("a", 10, 1250), "nothing new");
        p.track("a", 11, 1250);
        assert_eq!(p.today(11), 0, "a new day starts from where the story is");
        assert!(!p.stories.contains_key("b"), "yesterday's are let go");
    }
}
