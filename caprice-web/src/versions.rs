//! Keeping the writing safe: a story with a file saves itself soon after every change, and every
//! story, saved or not, keeps earlier versions of itself, to go back to.
//!
//! Versions are whole `.caprice` files in the app's data folder, one folder per story, named by
//! the story's id (saved in its file), so they stay together when the file is moved or renamed,
//! and a story never saved has them too. One is kept when a story is opened, every few minutes
//! while it changes, before going back to another, and when Caprice closes: so even "Don't save"
//! on a story never saved leaves it in the versions.

use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use eframe::egui::{self, Id, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::App;
use crate::fileio::DocFile;

/// Changes this old (seconds) are saved, in a story with a file.
pub const SAVE_AFTER: f64 = 30.0;
/// How often (seconds) the story is looked at for changes: it means serializing all of it.
const CHECK_EVERY: f64 = 5.0;
/// A version is kept at most this often (seconds) while the story changes.
pub const KEEP_EVERY: f64 = 300.0;
/// The newest versions are all kept; of older ones, the last of each day, for `KEEP_DAYS` days.
const KEEP_NEWEST: usize = 20;
const KEEP_DAYS: u64 = 30;
const DAY: u64 = 86_400;

/// Where the versions go: the app's data folder, as each system has it.
pub fn default_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join("Library/Application Support/Caprice/Versions"));
    }
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute());
    data.or_else(|| home.map(|h| h.join(".local/share"))).map(|d| d.join("caprice/versions"))
}

/// A new story's id: unlikely ever to be another's.
pub fn new_id() -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    crate::web::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos().hash(&mut h);
    crate::web::pid().hash(&mut h);
    format!("{:016x}", h.finish())
}

fn hash_of(json: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    json.hash(&mut h);
    h.finish()
}

/// What a story's folder says about it, for the list of stories never saved.
#[derive(Serialize, Deserialize, Default)]
struct About {
    name: String,
    path: Option<PathBuf>,
}

/// One version kept.
#[derive(Clone, Debug)]
pub struct Kept {
    pub file: PathBuf,
    /// When, in seconds since 1970.
    pub at: u64,
    pub words: usize,
    /// The story's name, and its id.
    pub name: String,
    pub id: String,
}

/// Autosaving and the versions kept, and the window that lists them.
pub struct Keeper {
    /// Save a story with a file by itself (and before opening another or closing).
    pub autosave: bool,
    /// Where versions go (None: none are kept).
    pub dir: Option<PathBuf>,
    /// The time in seconds since 1970, when a test says so; else the clock's.
    pub clock: Option<u64>,
    checked_at: f64,
    /// Since when (frame time) the story has changes not saved.
    changed_since: Option<f64>,
    kept_at: f64,
    /// What the newest version holds, so the same is not kept twice.
    kept_hash: u64,
    pub open: bool,
    /// This story's versions, newest first, and the newest of each story never saved.
    pub mine: Vec<Kept>,
    pub others: Vec<Kept>,
}

impl Keeper {
    pub fn new() -> Self {
        Self {
            autosave: true,
            dir: default_dir(),
            clock: None,
            checked_at: f64::NEG_INFINITY,
            changed_since: None,
            kept_at: f64::NEG_INFINITY,
            kept_hash: 0,
            open: false,
            mine: Vec::new(),
            others: Vec::new(),
        }
    }

    pub fn now(&self) -> u64 {
        self.clock.unwrap_or_else(|| crate::web::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()))
    }

    /// Another story is shown: its newest version is what it holds, unless it has changed since.
    pub fn opened(&mut self, id: &str) {
        let newest = self.dir.as_ref().and_then(|d| versions_in(&d.join(id)).into_iter().next_back());
        self.kept_hash = newest.and_then(|(_, f)| std::fs::read_to_string(f).ok()).map_or(0, |j| hash_of(&j));
        (self.kept_at, self.changed_since, self.checked_at) = (f64::NEG_INFINITY, None, f64::NEG_INFINITY);
    }
}

/// When the versions in a story's folder were kept, oldest first.
pub fn times_kept(dir: &Path) -> Vec<u64> {
    versions_in(dir).into_iter().map(|(t, _)| t).collect()
}

/// The versions in a story's folder, oldest first: (when, file).
fn versions_in(dir: &Path) -> Vec<(u64, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<(u64, PathBuf)> = entries
        .filter_map(|e| {
            let path = e.ok()?.path();
            let at = path.file_name()?.to_str()?.strip_suffix(".caprice")?.parse().ok()?;
            Some((at, path))
        })
        .collect();
    found.sort();
    found
}

/// Which of the versions made at `times` (oldest first) to keep at `now`: the newest
/// `KEEP_NEWEST`, and of the others the last of each day, if no older than `KEEP_DAYS`.
pub fn kept(times: &[u64], now: u64) -> Vec<bool> {
    let newest = times.len().saturating_sub(KEEP_NEWEST);
    times
        .iter()
        .enumerate()
        .map(|(k, &t)| {
            let last_of_day = times.get(k + 1).is_none_or(|&next| next / DAY != t / DAY);
            k >= newest || (last_of_day && now.saturating_sub(t) < KEEP_DAYS * DAY)
        })
        .collect()
}

/// How long ago `at` was, as people say it.
pub fn ago(at: u64, now: u64) -> String {
    let s = now.saturating_sub(at);
    let n = |k: u64, one: &str| format!("{k} {one}{} ago", if k == 1 { "" } else { "s" });
    match s {
        0..60 => "just now".into(),
        60..3600 => n(s / 60, "minute"),
        3600..DAY => n(s / 3600, "hour"),
        _ if s < 2 * DAY => "yesterday".into(),
        _ => n(s / DAY, "day"),
    }
}

impl App {
    /// The story's name, as files and versions show it.
    pub fn story_name(&self) -> String {
        self.path.as_ref().and_then(|p| p.file_stem()).map_or("Untitled".to_owned(), |s| s.to_string_lossy().into_owned())
    }

    /// Once in a while: save a story with a file whose changes are `SAVE_AFTER` old, and keep a
    /// version if the story changed and none was kept for `KEEP_EVERY`.
    pub fn keep_safe(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if now - self.keep.checked_at < CHECK_EVERY {
            return;
        }
        self.keep.checked_at = now;
        let json = serde_json::to_string(&self.doc_file()).unwrap_or_default();
        let hash = hash_of(&json);
        if hash == self.saved_hash {
            self.keep.changed_since = None;
        } else {
            let since = *self.keep.changed_since.get_or_insert(now);
            let free = self.dialog.is_none() && self.leaving.is_none();
            if self.keep.autosave && free && now - since >= SAVE_AFTER {
                self.save_quietly();
            }
        }
        // A fresh story with nothing in it is not worth keeping.
        let blank = self.path.is_none() && hash == self.saved_hash;
        let due = now - self.keep.kept_at >= KEEP_EVERY;
        if self.keep.dir.is_some() && hash != self.keep.kept_hash && !blank && due {
            self.keep_version(&json);
            self.keep.kept_at = now;
        }
        // Nothing may happen on screen until then: come back to save or keep.
        let pending = self.keep.changed_since.is_some() || (self.keep.dir.is_some() && hash != self.keep.kept_hash && !blank);
        if pending {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(CHECK_EVERY));
        }
    }

    /// Save to the story's file without saying so: it happens on its own.
    pub fn save_quietly(&mut self) {
        let Some(path) = self.path.clone() else { return };
        let (shown, as_saved) = (std::mem::take(&mut self.status), self.as_saved.take());
        self.save_to(path);
        self.as_saved = as_saved;
        if self.status == "- saved" {
            self.status = shown;
        }
    }

    /// Has a story with a file changed since it was opened or saved by hand? Autosave does not
    /// count: it is what the writer may still want to drop on leaving.
    pub fn changed_since_saved(&self) -> bool {
        let saved = self.as_saved.as_ref().map(|(h, _)| *h);
        self.keep.autosave && self.path.is_some() && saved.is_some_and(|h| h != self.content_hash())
    }

    /// Put the story back as it was opened or last saved by hand, its file too. What it held is
    /// kept as a version first, so dropping it can be undone from the Versions window.
    pub fn drop_changes(&mut self, ctx: &egui::Context) {
        let Some(file) = self.as_saved.as_ref().and_then(|(_, j)| serde_json::from_str::<DocFile>(j).ok()) else {
            self.status = "- not saved yet: nothing to go back to".into();
            return;
        };
        if self.as_saved.as_ref().is_some_and(|(h, _)| *h == self.content_hash()) {
            self.status = "- no changes since saving".into();
            return;
        }
        self.keep_now();
        self.adopt(ctx, file);
        self.save_quietly();
        self.status = "- back to as saved".into();
    }

    /// Before opening another story or closing, a story with a file is saved, rather than asked about.
    pub fn save_before_leaving(&mut self) {
        if self.keep.autosave && self.path.is_some() && self.unsaved() {
            self.save_quietly();
        }
    }

    /// Keep a version of the story as it is now, unless the newest already holds it.
    pub fn keep_now(&mut self) {
        let json = serde_json::to_string(&self.doc_file()).unwrap_or_default();
        let blank = self.path.is_none() && !self.unsaved();
        if self.keep.dir.is_some() && hash_of(&json) != self.keep.kept_hash && !blank {
            self.keep_version(&json);
        }
    }

    fn keep_version(&mut self, json: &str) {
        let Some(dir) = self.keep.dir.as_ref().map(|d| d.join(&self.story_id)) else { return };
        // Two in one second (going back, then opening the list) must not write over each other.
        let mut now = self.keep.now();
        while dir.join(format!("{now}.caprice")).exists() {
            now += 1;
        }
        let about = About { name: self.story_name(), path: self.path.clone() };
        let written = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(dir.join(format!("{now}.caprice")), json))
            .and_then(|()| std::fs::write(dir.join("story.json"), serde_json::to_string(&about).unwrap_or_default()));
        match written {
            Ok(()) => {
                self.keep.kept_hash = hash_of(json);
                let versions = versions_in(&dir);
                let times: Vec<u64> = versions.iter().map(|(t, _)| *t).collect();
                for ((_, file), keep) in versions.iter().zip(kept(&times, now)) {
                    if !keep {
                        let _ = std::fs::remove_file(file);
                    }
                }
            }
            Err(e) => self.status = format!("- keeping a version failed: {e}"),
        }
    }

    /// Open the Versions window, reading what is kept.
    pub fn open_versions(&mut self) {
        self.keep_now();
        self.keep.open = true;
        self.read_versions();
    }

    fn read_versions(&mut self) {
        let Some(dir) = self.keep.dir.clone() else { return };
        let read = |id: &str, (at, file): (u64, PathBuf)| -> Option<Kept> {
            let about: About = std::fs::read_to_string(dir.join(id).join("story.json")).ok().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
            let story: DocFile = serde_json::from_str(&std::fs::read_to_string(&file).ok()?).ok()?;
            Some(Kept { file, at, words: story.words(), name: about.name, id: id.to_owned() })
        };
        self.keep.mine = versions_in(&dir.join(&self.story_id)).into_iter().rev().filter_map(|v| read(&self.story_id, v)).collect();
        let mut others = Vec::new();
        let ids: BTreeSet<String> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|id| *id != self.story_id)
            .collect();
        for id in ids {
            let about: Option<About> = std::fs::read_to_string(dir.join(&id).join("story.json")).ok().and_then(|j| serde_json::from_str(&j).ok());
            if about.is_some_and(|a| a.path.is_none())
                && let Some(newest) = versions_in(&dir.join(&id)).pop()
                && let Some(k) = read(&id, newest)
            {
                others.push(k);
            }
        }
        others.sort_by_key(|k| std::cmp::Reverse(k.at));
        self.keep.others = others;
    }

    /// Show the version `k` in place of the story shown, which is kept first. A version of this
    /// story keeps its file, to be saved over; a story never saved stays without one.
    pub fn go_back_to(&mut self, ctx: &egui::Context, k: &Kept) {
        let Some(file) = std::fs::read_to_string(&k.file).ok().and_then(|j| serde_json::from_str::<DocFile>(&j).ok()) else {
            self.status = "- that version cannot be read".into();
            return;
        };
        self.keep_now();
        let when = ago(k.at, self.keep.now());
        if k.id != self.story_id {
            (self.path, self.story_id) = (None, k.id.clone());
            self.keep.opened(&k.id);
            self.created = file.created;
        }
        self.adopt(ctx, file);
        self.status = format!("- back to {when}");
        self.read_versions();
    }

    /// The Versions window, if open.
    pub fn versions_window(&mut self, ctx: &egui::Context) {
        if !self.keep.open {
            return;
        }
        let now = self.keep.now();
        let mut open = true;
        let mut chosen: Option<Kept> = None;
        let screen = ctx.content_rect();
        egui::Window::new("Versions")
            .id(Id::new("versions"))
            .open(&mut open)
            .default_pos(pos2(screen.center().x - 160.0, screen.top() + 90.0))
            .default_width(320.0)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                if self.keep.dir.is_none() {
                    ui.label("Versions cannot be kept: there is no folder for them.");
                    return;
                }
                ui.label(format!("Earlier versions of \u{201c}{}\u{201d}, kept as you write:", self.story_name()));
                ui.add_space(4.0);
                egui::ScrollArea::vertical().id_salt("versions_mine").max_height(320.0).show(ui, |ui| {
                    if self.keep.mine.is_empty() {
                        ui.weak("None yet: one is kept every few minutes while the story changes.");
                    }
                    for k in &self.keep.mine {
                        ui.horizontal(|ui| {
                            if ui.button(ago(k.at, now)).on_hover_text("Go back to this version (the story as it is now is kept)").clicked() {
                                chosen = Some(k.clone());
                            }
                            ui.weak(format!("{} words", k.words));
                        });
                    }
                });
                if !self.keep.others.is_empty() {
                    ui.add_space(6.0);
                    egui::CollapsingHeader::new("Stories never saved").id_salt("versions_others").show(ui, |ui| {
                        for k in &self.keep.others {
                            ui.horizontal(|ui| {
                                if ui.button(format!("{}, {}", k.name, ago(k.at, now))).on_hover_text("Open this story").clicked() {
                                    chosen = Some(k.clone());
                                }
                                ui.weak(format!("{} words", k.words));
                            });
                        }
                    });
                }
                ui.add_space(2.0);
                ui.allocate_space(vec2(ui.available_width(), 0.0));
            });
        self.keep.open = open;
        if let Some(k) = chosen {
            self.go_back_to(ctx, &k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_versions_are_kept_and_of_older_ones_the_last_of_each_day_for_a_month() {
        let now = 100 * DAY;
        // Three a day for forty days, then twenty-five today.
        let mut times: Vec<u64> = (60..100).flat_map(|d| [d * DAY + 10, d * DAY + 20, d * DAY + 30]).collect();
        times.extend((0..25).map(|k| now + k));
        let keep = kept(&times, now + 30);
        let kept_times: Vec<u64> = times.iter().zip(&keep).filter(|(_, k)| **k).map(|(t, _)| *t).collect();
        assert!(times[times.len() - 20..].iter().all(|t| kept_times.contains(t)), "the newest twenty");
        assert!(kept_times.contains(&(99 * DAY + 30)) && !kept_times.contains(&(99 * DAY + 20)), "the last of a day");
        assert!(!kept_times.contains(&(60 * DAY + 30)), "not older than a month");
        assert!(kept_times.contains(&(71 * DAY + 30)));
    }

    #[test]
    fn times_are_told_as_how_long_ago() {
        assert_eq!(ago(1000, 1030), "just now");
        assert_eq!(ago(1000, 1000 + 60), "1 minute ago");
        assert_eq!(ago(1000, 1000 + 900), "15 minutes ago");
        assert_eq!(ago(0, 3 * 3600 + 5), "3 hours ago");
        assert_eq!(ago(0, DAY + 5), "yesterday");
        assert_eq!(ago(0, 4 * DAY), "4 days ago");
    }
}
