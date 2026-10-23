//! Asking Claude about a passage. With the pen on, drawing a loop around text selects it and
//! opens a menu of commands; picking by cursor instead, letting go of a selection opens it (review, fix, grammar, summary, or a question of one's own). The answer
//! streams into a floating panel, from where a corrected passage can be put in place of the old one.
//!
//! Connecting scenes: the passage holds the end of one scene and the start of a later one, and
//! Claude finds the gap between them and suggests ways across. From there it is a conversation:
//! the writer replies, asks for a draft (the passage again with the gap filled), or pins an idea
//! as a post-it. Claude only ever sees the passage, what the writer types for it, and the story
//! notes, kept with the document, which say what the writer always wants it to know.
//!
//! The request goes through the installed Claude Code CLI (`claude -p`) on a background thread.
//! The CLI uses its own sign-in, which Caprice never sees, and streams the answer as it is written.

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;

use eframe::egui::{self, Color32, FontId, Id, Key, Pos2, Rect, Sense, Shape, Stroke, text::LayoutJob, vec2};
use serde_json::Value;

use crate::App;
use crate::model::{IMAGE_CHAR, PAGE_BREAK};
use crate::theme::pal;

/// The model that answers about a passage.
const MODEL: &str = "claude-opus-5-5";
const NO_CLI: &str = "Caprice could not find Claude Code. Install it and sign in by running `claude` once \
    in a terminal, then try again.";

pub const FOLLOW_UP: &str = "Answer the writer's last reply. Keep what has been settled in the conversation and build \
    on it. Be concise, and do not repeat what you already said.";

pub const DRAFT: &str = "Now fill the gap: write what happens there, so that the first scene leads into the \
    second, following what the writer settled in the conversation (if nothing was settled, the idea that fits \
    best). If you already wrote a draft, revise it as the writer asks. Match the story's language, voice, tense \
    and style, and how it sets out dialogue. Reply with the whole passage, word for word as it is, except that \
    your text stands in the gap and whatever marked the gap (a separator line, a note) is gone. Nothing before \
    or after it.";

const SYSTEM: &str = "You help a writer working in a word processor. They circled a passage of their \
    document and picked a command. Answer in the language of the passage. Write plain text only: no \
    Markdown, no headings, no bullet symbols other than simple dashes.";

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Review,
    Fix,
    Grammar,
    Summarize,
    Ask(String),
    /// Ideas for the gap between the two scenes in the passage, with the writer's note.
    Bridge(String),
}

impl Command {
    fn title(&self) -> &str {
        match self {
            Command::Review => "Review",
            Command::Fix => "Fix",
            Command::Grammar => "Grammar check",
            Command::Summarize => "Summary",
            Command::Ask(_) => "Answer",
            Command::Bridge(_) => "Connecting the scenes",
        }
    }

    fn instruction(&self) -> &str {
        match self {
            Command::Review => "Review this passage: what works, what does not, and concrete suggestions. Be concise.",
            Command::Fix => "Rewrite this passage to fix errors and awkward or unclear wording, keeping its meaning, \
                tone and voice. Reply with only the rewritten passage, nothing before or after it.",
            Command::Grammar => "Correct only spelling, grammar and punctuation in this passage; change nothing else. \
                Reply with only the corrected passage, nothing before or after it. If nothing needs \
                correcting, reply with the passage unchanged.",
            Command::Summarize => "Summarize this passage in a few sentences.",
            Command::Ask(q) => q,
            Command::Bridge(_) => "This passage holds the end of one scene and the start of a later one. Between them \
                is a gap: the writer does not yet know how to get from one to the other. It may be marked by a \
                separator line or a note, or not at all. Find the gap, then suggest three or four ways to connect \
                the two scenes that really differ from each other. For each, give a short name, then in a few \
                dashed lines what happens, and what it needs set up earlier or sets up for later. Keep to the \
                characters, facts and tone of the passage and to the writer's notes. Do not write the scene \
                itself. End with one or two questions whose answers would help the writer choose.",
        }
    }

    /// Is the answer a new version of the passage, that can replace it?
    fn rewrites(&self) -> bool {
        matches!(self, Command::Fix | Command::Grammar)
    }

    /// Is the answer about the story, so that the story notes help? Corrections need only the passage.
    fn knows_story(&self) -> bool {
        !self.rewrites()
    }
}

/// One exchange of a conversation with Claude: its answer, and what the writer replied.
struct Turn {
    answer: String,
    reply: String,
    /// The reply asked for a draft of the passage that fills the gap.
    draft: bool,
}

/// How passages are picked while asking Claude is on.
#[derive(Clone, Copy, PartialEq, Default)]
pub enum Picking {
    /// Drawing a loop around them with the pen. The page takes no other clicks.
    #[default]
    Circle,
    /// Selecting them as always, with the cursor, which can go on over several pages.
    Cursor,
}

/// The loop being drawn (or drawn) around text, in page points of page `page`.
#[derive(Default)]
pub struct Lasso {
    pub page: usize,
    pub points: Vec<Pos2>,
    /// The flow range it caught, and where on screen its menu opens, once the loop is closed.
    pub caught: Option<(usize, usize, Pos2)>,
    /// What the user types into the menu's "Ask" field, and whether the menu has been shown yet.
    pub question: String,
    pub menu_shown: bool,
    /// Connecting the scenes was picked: the menu asks what Claude should know (`note`) first.
    pub bridging: bool,
    pub note: String,
}

enum Msg {
    Text(String),
    Done,
    Failed(String),
    Refused,
}

#[derive(PartialEq)]
enum State {
    Waiting,
    Streaming,
    Done,
    Failed(String),
    Refused,
}

/// A question to Claude and its answer, as shown in the panel.
pub struct Answer {
    command: Command,
    /// What was sent first with the system prompt: the passage, any story around it, the command.
    first: String,
    /// The exchanges before the answer being written now; each reply sends them all again.
    turns: Vec<Turn>,
    /// The flow range asked about and its text then, to check it is unchanged before replacing it.
    range: (usize, usize),
    original: String,
    text: String,
    /// Being written in the reply field.
    reply: String,
    /// `text` is a draft of the passage, which can take its place.
    draft: bool,
    /// `text` has been pinned as a post-it.
    pinned: bool,
    state: State,
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    at: Pos2,
}

impl Answer {
    /// What is sent to Claude for the answer being written: the first prompt, then the conversation.
    fn sent(&self) -> String {
        let Some(last) = self.turns.last() else { return self.first.clone() };
        let mut s = format!("{}\n\n<conversation_so_far>\n", self.first);
        for t in &self.turns {
            s += &format!("<your_answer>\n{}\n</your_answer>\n", t.answer.trim());
            if !t.reply.trim().is_empty() {
                s += &format!("<writers_reply>\n{}\n</writers_reply>\n", t.reply.trim());
            }
        }
        s += "</conversation_so_far>\n\n";
        if !last.draft {
            return s + FOLLOW_UP;
        }
        // Named, the passage's ends are kept far more reliably than when only asked for in general.
        let (head, tail) = ends(&self.original);
        s + DRAFT + &format!(" Begin your reply with \"{head}\" and end it with \"{tail}\".")
    }

    /// Does `text` still hold the whole passage around what it adds? Claude sometimes answers a
    /// draft with only the new part, which would replace both scenes.
    fn keeps_ends(&self, text: &str) -> bool {
        let words = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        let (head, tail) = ends(&self.original);
        let text = words(text);
        text.starts_with(&words(&head)) && text.ends_with(&words(&tail))
    }

    /// Can the answer take the passage's place?
    fn rewrites(&self) -> bool {
        self.command.rewrites() || self.draft
    }
}

impl Drop for Answer {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Is `p` inside the polygon `poly` (even-odd rule)?
fn inside(poly: &[Pos2], p: Pos2) -> bool {
    let mut odd = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            odd = !odd;
        }
        j = i;
    }
    odd
}

/// How a run of `claude -p` ended.
pub enum Outcome {
    Finished,
    Refused,
    Cancelled,
}

/// The first and last few words of `passage`.
fn ends(passage: &str) -> (String, String) {
    let words: Vec<&str> = passage.split_whitespace().collect();
    let n = words.len().min(5);
    (words[..n].join(" "), words[words.len() - n..].join(" "))
}

/// Ask Claude `prompt` on a thread: the answer comes in on the receiver, and setting the flag stops it.
fn start(ctx: &egui::Context, prompt: String) -> (Receiver<Msg>, Arc<AtomicBool>) {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let (ctx, stop) = (ctx.clone(), cancel.clone());
    crate::web::spawn(move || {
        if let Err(e) = ask(&prompt, &tx, &ctx, &stop) {
            let _ = tx.send(Msg::Failed(e));
        }
        ctx.request_repaint();
    });
    (rx, cancel)
}

/// Stream Claude's answer to `prompt` into `tx`, until done or `cancel` is set.
fn ask(prompt: &str, tx: &Sender<Msg>, ctx: &egui::Context, cancel: &AtomicBool) -> Result<(), String> {
    let mut on_text = |t: &str| {
        let _ = tx.send(Msg::Text(t.to_owned()));
        ctx.request_repaint();
    };
    match run_claude(MODEL, SYSTEM, prompt, "medium", cancel, &mut on_text)? {
        Outcome::Finished => {
            let _ = tx.send(Msg::Done);
        }
        Outcome::Refused => {
            let _ = tx.send(Msg::Refused);
        }
        Outcome::Cancelled => {}
    }
    Ok(())
}

/// A whole reply from Claude, asked for on a thread, for windows that read it once it is all there
/// (the review, the ideas). Dropped, it stops the request.
pub struct Pending {
    rx: Receiver<Result<String, String>>,
    cancel: Arc<AtomicBool>,
}

impl Pending {
    pub fn start(ctx: &egui::Context, system: &'static str, prompt: String, effort: &'static str) -> Self {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (ctx, stop) = (ctx.clone(), cancel.clone());
        crate::web::spawn(move || {
            let mut reply = String::new();
            let result = match run_claude(MODEL, system, &prompt, effort, &stop, &mut |t| reply.push_str(t)) {
                Ok(Outcome::Finished) => Ok(reply),
                Ok(Outcome::Refused) => Err("Claude declined to answer this request.".into()),
                Ok(Outcome::Cancelled) => return,
                Err(e) => Err(e),
            };
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        Pending { rx, cancel }
    }

    /// The reply, once it has come (or why there is none).
    pub fn poll(&self) -> Option<Result<String, String>> {
        self.rx.try_recv().ok()
    }

    /// A reply that has already come, as if from Claude.
    #[cfg(test)]
    pub fn answered(reply: &str) -> Self {
        let (tx, rx) = channel();
        tx.send(Ok(reply.to_owned())).unwrap();
        Pending { rx, cancel: Arc::new(AtomicBool::new(false)) }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The JSON array in Claude's reply, which may come with words or a code fence around it.
pub fn json_list(reply: &str) -> Vec<Value> {
    let (Some(a), Some(b)) = (reply.find('['), reply.rfind(']')) else { return Vec::new() };
    serde_json::from_str::<Vec<Value>>(reply.get(a..=b).unwrap_or_default()).unwrap_or_default()
}

/// The Claude Code CLI: on the PATH, or where installers put it (the app launcher's PATH is often short).
fn find_cli() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let on_path = std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).map(|d| d.join("claude"));
    let usual = [".local/bin/claude", ".claude/local/claude", ".local/share/mise/shims/claude", ".local/share/mise/installs/claude/latest/claude"]
        .map(|p| home.join(p));
    on_path.chain(usual).find(|p| p.is_file())
}

// Tests never reach the real Claude, unless one says so with `allow_live` (the `live_*` tests).
#[cfg(test)]
thread_local! {
    static LIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub fn allow_live() {
    LIVE.with(|l| l.set(true));
}

/// Ask Claude (`model`) through `claude -p` (no tools, no project files, nothing saved) and pass
/// the answer to `on_text` as it is written, until it ends or `cancel` is set.
pub fn run_claude(model: &str, system: &str, prompt: &str, effort: &str, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str)) -> Result<Outcome, String> {
    use std::io::Write;
    use std::process::{Command as Process, Stdio};
    #[cfg(test)]
    if !LIVE.with(|l| l.get()) {
        return Err("Tests do not call Claude".into());
    }
    let cli = find_cli().ok_or(NO_CLI)?;
    let mut child = Process::new(&cli)
        .args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages"])
        // Keep user settings (API keys, custom providers like Bedrock/Vertex live there) but
        // drop project and local settings, which belong to whatever repo the writer has open.
        .args(["--tools", "", "--setting-sources", "user", "--strict-mcp-config", "--no-session-persistence"])
        .args(["--model", model, "--effort", effort, "--system-prompt", system])
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start Claude Code ({}): {e}", cli.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes()).map_err(|e| format!("Could not talk to Claude Code: {e}"))?;
    }
    let stdout = child.stdout.take().ok_or("Claude Code gave no output")?;
    let (mut failure, mut refused) = (None, false);
    for line in BufReader::new(stdout).lines() {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Outcome::Cancelled);
        }
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        match msg["type"].as_str() {
            // Events of the Messages API stream. After the message ends the CLI still sends its
            // closing summary, so keep reading.
            Some("stream_event") => {
                let event = &msg["event"];
                match event["type"].as_str() {
                    Some("content_block_delta") if event["delta"]["type"] == "text_delta" => on_text(event["delta"]["text"].as_str().unwrap_or_default()),
                    Some("message_delta") if event["delta"]["stop_reason"] == "refusal" => refused = true,
                    Some("error") => failure = Some(event["error"]["message"].as_str().unwrap_or("Unknown error").to_owned()),
                    _ => {}
                }
            }
            Some("result") if msg["is_error"] == true => {
                failure = Some(msg["result"].as_str().unwrap_or("Claude Code reported an error").to_owned());
            }
            _ => {}
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if refused {
        return Ok(Outcome::Refused);
    }
    if let Some(f) = failure {
        return Err(format!("Claude Code: {f}"));
    }
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut e, &mut err);
        }
        let err = err.trim();
        return Err(if err.is_empty() { format!("Claude Code stopped ({status})") } else { format!("Claude Code: {err}") });
    }
    Ok(Outcome::Finished)
}

/// Splits text into runs of word characters and runs of everything else, for comparing.
fn tokens(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut kind = None;
    for (b, c) in s.char_indices() {
        let k = c.is_alphanumeric();
        if kind.is_some_and(|w| w != k) {
            out.push(&s[start..b]);
            start = b;
        }
        kind = Some(k);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// The new text with what changed from the old one marked: removed words struck through in red,
/// added ones on green. None if the texts are too long to compare.
fn diff_job(old: &str, new: &str, font: FontId, ink: Color32) -> Option<LayoutJob> {
    let (a, b) = (tokens(old), tokens(new));
    if a.len() * b.len() > 4_000_000 {
        return None;
    }
    // Longest common subsequence table, filled from the end.
    let w = b.len() + 1;
    let mut lcs = vec![0u32; (a.len() + 1) * w];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i * w + j] = if a[i] == b[j] { lcs[(i + 1) * w + j + 1] + 1 } else { lcs[(i + 1) * w + j].max(lcs[i * w + j + 1]) };
        }
    }
    let same = egui::TextFormat { font_id: font.clone(), color: ink, ..Default::default() };
    let removed = egui::TextFormat {
        font_id: font.clone(),
        color: Color32::from_rgb(235, 110, 110),
        strikethrough: Stroke::new(1.0, Color32::from_rgb(235, 110, 110)),
        ..Default::default()
    };
    let added = egui::TextFormat { font_id: font, color: ink, background: Color32::from_rgba_unmultiplied(80, 190, 110, 90), ..Default::default() };
    let mut job = LayoutJob::default();
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            job.append(a[i], 0.0, same.clone());
            (i, j) = (i + 1, j + 1);
        } else if j < b.len() && (i == a.len() || lcs[i * w + j + 1] >= lcs[(i + 1) * w + j]) {
            job.append(b[j], 0.0, added.clone());
            j += 1;
        } else {
            job.append(a[i], 0.0, removed.clone());
            i += 1;
        }
    }
    Some(job)
}

impl App {
    /// The text of the flow chars `a..b`, as sent to Claude: pictures left out, page breaks as line breaks.
    pub fn passage(&self, a: usize, b: usize) -> String {
        let (ba, bb) = (self.doc.char_to_byte(a), self.doc.char_to_byte(b));
        self.doc.flow.text[ba..bb].chars().filter(|&c| c != IMAGE_CHAR).map(|c| if c == PAGE_BREAK { '\n' } else { c }).collect()
    }

    /// What is sent to Claude for `command` about the passage `a..b`: the passage and nothing else
    /// of the document, but the story notes if the command is about the story, and the writer's note.
    fn prompt(&self, a: usize, b: usize, command: &Command) -> String {
        let passage = self.passage(a, b);
        let story_notes = match self.doc.story_notes.trim() {
            n if n.is_empty() || !command.knows_story() => String::new(),
            n => format!("<story_notes>\n{n}\n</story_notes>\n\n"),
        };
        let note = match command {
            Command::Bridge(n) if !n.trim().is_empty() => format!("<writers_note>\n{}\n</writers_note>\n\n", n.trim()),
            _ => String::new(),
        };
        format!("{story_notes}<passage>\n{passage}\n</passage>\n\n{note}{}", command.instruction())
    }

    pub fn toggle_pen(&mut self) {
        self.pen = !self.pen;
        self.lasso = None;
    }

    /// Turn asking Claude on, picking passages by `picking`.
    pub fn pick_by(&mut self, picking: Picking) {
        (self.pen, self.picking) = (true, picking);
        self.lasso = None;
    }

    /// The toolbar's Claude menu: ask by circling passages with the pen, or by selecting them.
    pub fn claude_menu(&mut self, ui: &mut egui::Ui) {
        let (mut chosen, mut notes, mut review, mut ideas) = (None, false, false, false);
        let tip = "Ask Claude about a passage: circle it, or select it with the cursor (Ctrl+Shift+P, Esc to stop)";
        egui::containers::menu::MenuButton::from_button(egui::Button::new("Claude").selected(self.pen)).ui(ui, |ui| {
            ui.set_min_width(230.0);
            let choices = [
                (Some(Picking::Circle), "Circle text with the pen"),
                (Some(Picking::Cursor), "Select text with the cursor"),
                (None, "Off"),
            ];
            for (picking, label) in choices {
                let on = if let Some(p) = picking { self.pen && self.picking == p } else { !self.pen };
                if ui.selectable_label(on, label).clicked() {
                    chosen = Some(picking);
                    ui.close();
                }
            }
            ui.separator();
            let tip = "What Claude should always know about the story when asked about it";
            if ui.selectable_label(self.story_notes_open, "Story notes…").on_hover_text(tip).clicked() {
                notes = true;
                ui.close();
            }
            let tip = "A window on the left where Claude finds mistakes, clumsy wording, and places that want more \
                detail or say too much";
            if ui.selectable_label(self.review.open, "Review").on_hover_text(tip).clicked() {
                review = true;
                ui.close();
            }
            let tip = "A window on the left where Claude proposes ways the story could go on from the caret";
            if ui.selectable_label(self.ideas.open, "What next").on_hover_text(tip).clicked() {
                ideas = true;
                ui.close();
            }
        })
        .0
        .on_hover_text(tip);
        match chosen {
            _ if notes => self.story_notes_open = true,
            _ if review => self.toggle_review(),
            _ if ideas => self.toggle_ideas(),
            Some(Some(picking)) => self.pick_by(picking),
            Some(None) => (self.pen, self.lasso) = (false, None),
            None => {}
        }
    }

    /// The story notes, kept with the document and sent with every question about the story.
    pub fn story_notes_window(&mut self, ctx: &egui::Context) {
        let mut open = self.story_notes_open;
        egui::Window::new("Story notes").id(Id::new("story_notes")).open(&mut open).default_width(420.0).show(ctx, |ui| {
            let about = "Claude reads these whenever it reviews, answers about or connects parts of the story.";
            ui.label(egui::RichText::new(about).size(12.0).color(pal().text_dim));
            let hint = "Who is who, what has happened so far, what the reader must not learn yet…";
            let field = egui::TextEdit::multiline(&mut self.doc.story_notes).hint_text(hint).desired_width(f32::INFINITY);
            ui.add(field.desired_rows(14));
        });
        self.story_notes_open = open;
    }

    /// Loops are drawn on the page, which then takes no clicks for the text.
    pub fn circling(&self) -> bool {
        self.pen && self.picking == Picking::Circle
    }

    /// Picking by cursor, the selection was let go of at `at`: offer the commands for it.
    pub fn offer_commands(&mut self, at: Pos2) {
        let (a, b) = self.selection();
        let page = self.target;
        self.lasso = Some(Lasso { page, caught: Some((a, b, at)), ..Default::default() });
    }

    /// With the pen on, page `i` at `page_rect` takes drags as loops drawn around text.
    pub fn pen_surface(&mut self, ui: &mut egui::Ui, page_rect: Rect, i: usize) {
        if self.lasso.as_ref().is_some_and(|l| l.page != i) {
            self.lasso = None;
        }
        if self.circling() && !self.ctrl_down {
            let ctx = ui.ctx().clone();
            let sc = self.scale_of(page_rect);
            let resp = ui.interact(page_rect.expand(40.0), Id::new("pen"), Sense::drag());
            if resp.hovered() || resp.dragged() {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);
            }
            let to_page = |p: Pos2| ((p - page_rect.min) / sc).to_pos2();
            if resp.drag_started() {
                self.lasso = Some(Lasso { page: i, ..Default::default() });
            }
            let drawing = resp.dragged() || resp.drag_started();
            if let (Some(l), Some(p), true) = (&mut self.lasso, resp.interact_pointer_pos(), drawing) {
                let q = to_page(p);
                if l.caught.is_none() && l.points.last().is_none_or(|&last| (last - q).length() * sc > 2.0) {
                    l.points.push(q);
                }
            }
            if resp.drag_stopped() {
                self.close_lasso(&ctx, page_rect, i);
            }
        }
        self.paint_lasso(ui.painter(), page_rect);
        self.lasso_menu(ui.ctx());
    }

    /// The loop is finished: select the text inside it and offer the commands.
    fn close_lasso(&mut self, ctx: &egui::Context, page_rect: Rect, i: usize) {
        let Some(mut l) = self.lasso.take() else { return };
        let origin = self.doc.setup.margin_origin().to_pos2();
        let start = self.doc.spans[i].start;
        let caught: Vec<usize> = if l.points.len() < 3 {
            Vec::new()
        } else {
            let layout = self.doc.layout_page(ctx, i, 1.0, &[]);
            layout.char_centers().into_iter().filter(|&(_, c)| inside(&l.points, origin + c.to_vec2())).map(|(k, _)| start + k).collect()
        };
        let (Some(&a), Some(&b)) = (caught.iter().min(), caught.iter().max()) else {
            self.status = "- draw a loop around the text to ask Claude about".into();
            return;
        };
        let sc = self.scale_of(page_rect);
        let end = l.points.last().map_or(page_rect.center(), |&p| page_rect.min + p.to_vec2() * sc);
        l.caught = Some((a, b + 1, end));
        self.lasso = Some(l);
        self.anchor = a;
        self.set_caret(ctx, b + 1, true);
        self.target = i; // the end of the passage can be the start of the next page
    }

    fn paint_lasso(&self, painter: &egui::Painter, page_rect: Rect) {
        let Some(l) = self.lasso.as_ref().filter(|l| l.points.len() > 1) else { return }; // none by cursor
        let sc = self.scale_of(page_rect);
        let pts: Vec<Pos2> = l.points.iter().map(|&p| page_rect.min + p.to_vec2() * sc).collect();
        let stroke = Stroke::new(2.2, pal().accent.gamma_multiply(0.85));
        if l.caught.is_some() {
            painter.add(Shape::closed_line(pts, stroke));
        } else {
            painter.add(Shape::line(pts, stroke));
        }
    }

    /// The commands for a closed loop.
    fn lasso_menu(&mut self, ctx: &egui::Context) {
        let Some(l) = &mut self.lasso else { return };
        let Some((a, b, at)) = l.caught else { return };
        let (mut chosen, mut paint) = (None, false);
        let area = egui::Area::new(Id::new("lasso_menu")).order(egui::Order::Foreground).fixed_pos(at + vec2(10.0, 10.0)).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(230.0);
                if l.bridging {
                    let title = "Connect the story before this with the story after it";
                    ui.label(egui::RichText::new(title).size(12.0).color(pal().text_dim));
                    let hint = "What should Claude know? Who the people are, what has happened in between… (optional)";
                    let note = egui::TextEdit::multiline(&mut l.note).hint_text(hint).desired_width(220.0).desired_rows(5);
                    let field = ui.add(note);
                    if !l.menu_shown {
                        field.request_focus();
                        l.menu_shown = true;
                    }
                    let go = ui.add(egui::Button::new("Get ideas").min_size(vec2(220.0, 24.0))).on_hover_text("Ctrl+Enter");
                    if go.clicked() || (field.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && i.modifiers.command)) {
                        chosen = Some(Command::Bridge(l.note.trim().to_owned()));
                    }
                    return;
                }
                ui.label(egui::RichText::new("Ask Claude about this passage").size(12.0).color(pal().text_dim));
                for c in [Command::Review, Command::Fix, Command::Grammar, Command::Summarize] {
                    let label = match c {
                        Command::Summarize => "Sum it up",
                        _ => c.title(),
                    };
                    if ui.add(egui::Button::new(label).frame(false).min_size(vec2(220.0, 24.0))).clicked() {
                        chosen = Some(c);
                    }
                }
                let paint_tip = "Claude paints the scene this passage describes, behind its part of the story";
                if ui.add(egui::Button::new("Paint this scene").frame(false).min_size(vec2(220.0, 24.0))).on_hover_text(paint_tip).clicked() {
                    paint = true;
                }
                let bridge_tip = "This passage is a gap between two scenes: Claude suggests ways from the story before \
                    it to the story after it";
                let bridge = ui.add(egui::Button::new("Connect the scenes…").frame(false).min_size(vec2(220.0, 24.0)));
                if bridge.on_hover_text(bridge_tip).clicked() {
                    // The note field takes the typing next frame; the question field must not take it now.
                    (l.bridging, l.menu_shown) = (true, false);
                    return;
                }
                ui.separator();
                let field = ui.add(egui::TextEdit::singleline(&mut l.question).hint_text("Ask something else…").desired_width(220.0));
                // Typing goes into the question, never over the circled text.
                if !l.menu_shown {
                    field.request_focus();
                    l.menu_shown = true;
                }
                if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !l.question.trim().is_empty() {
                    chosen = Some(Command::Ask(l.question.trim().to_owned()));
                }
            });
        });
        let clicked_outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p)));
        let dismissed = clicked_outside || ctx.input(|i| i.key_pressed(Key::Escape));
        if let Some(command) = chosen {
            self.lasso = None;
            self.ask_claude(ctx, a, b, command, at);
        } else if paint {
            self.lasso = None;
            self.paint_passage(ctx, a, b);
        } else if dismissed {
            self.lasso = None;
        }
    }

    /// Send the passage `a..b` with `command`; the answer shows in the panel next to `at`.
    pub fn ask_claude(&mut self, ctx: &egui::Context, a: usize, b: usize, command: Command, at: Pos2) {
        let mut answer = self.new_answer(a, b, command, at);
        (answer.rx, answer.cancel) = start(ctx, answer.sent());
        self.answer = Some(answer);
    }

    /// An answer to `command` about `a..b` that is still to come.
    fn new_answer(&self, a: usize, b: usize, command: Command, at: Pos2) -> Answer {
        let first = self.prompt(a, b, &command);
        let original = self.doc.flow.text[self.doc.char_to_byte(a)..self.doc.char_to_byte(b)].to_owned();
        let (rx, cancel) = (channel().1, Arc::new(AtomicBool::new(false)));
        let (text, reply, state) = (String::new(), String::new(), State::Waiting);
        let (turns, range, draft, pinned) = (Vec::new(), (a, b), false, false);
        Answer { command, first, turns, range, original, text, reply, draft, pinned, state, rx, cancel, at }
    }

    /// Send the writer's reply to the answer shown, or ask for a draft of the gap (`draft`), with
    /// the conversation so far. A reply to a draft asks for it again, revised.
    pub fn follow_up(&mut self, ctx: &egui::Context, draft: bool) {
        let Some(ans) = &mut self.answer else { return };
        if ans.state != State::Done {
            return;
        }
        let draft = draft || ans.draft;
        let (answer, reply) = (std::mem::take(&mut ans.text), std::mem::take(&mut ans.reply).trim().to_owned());
        ans.turns.push(Turn { answer, reply, draft });
        (ans.draft, ans.pinned, ans.state) = (draft, false, State::Waiting);
        ans.cancel.store(true, Ordering::Relaxed);
        (ans.rx, ans.cancel) = start(ctx, ans.sent());
    }

    /// Put the answer shown on a post-it over the passage it is about.
    pub fn pin_answer(&mut self) -> bool {
        if self.unchanged().is_err() {
            return false;
        }
        let Some(ans) = &mut self.answer else { return false };
        if ans.state != State::Done || ans.text.trim().is_empty() || ans.pinned {
            return false;
        }
        let ((start, end), text) = (ans.range, ans.text.trim().to_owned());
        ans.pinned = true;
        let id = self.doc.next_note_id;
        self.doc.next_note_id += 1;
        self.doc.notes.push(crate::model::Note { id, start, end, text, color: 0 });
        self.status = "- pinned as a post-it".into();
        true
    }

    /// What the answer showing was asked with, as sent to Claude.
    #[cfg(test)]
    pub fn asked(&self) -> Option<(&Command, String)> {
        self.answer.as_ref().map(|a| (&a.command, a.sent()))
    }

    /// The floating panel with Claude's answer.
    pub fn answer_panel(&mut self, ctx: &egui::Context) {
        let Some(ans) = &mut self.answer else { return };
        while let Ok(msg) = ans.rx.try_recv() {
            match msg {
                Msg::Text(t) => {
                    ans.text.push_str(&t);
                    ans.state = State::Streaming;
                }
                Msg::Done if matches!(ans.state, State::Waiting | State::Streaming) => ans.state = State::Done,
                Msg::Done => {}
                Msg::Failed(e) => ans.state = State::Failed(e),
                Msg::Refused => ans.state = State::Refused,
            }
        }
        let applicable = self.applicable();
        let unchanged = self.unchanged();
        let Some(ans) = &mut self.answer else { return };
        let mut open = true;
        let (mut apply, mut pin, mut follow) = (false, false, None);
        let title = format!("Claude · {}", ans.command.title());
        egui::Window::new(title)
            .id(Id::new("claude_answer"))
            .open(&mut open)
            .default_pos(ans.at + vec2(16.0, 16.0))
            .default_width(400.0)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                let font = FontId::proportional(14.0);
                let ink = ui.visuals().text_color();
                egui::ScrollArea::vertical().max_height(420.0).stick_to_bottom(true).show(ui, |ui| {
                    // The conversation so far, then the answer being written.
                    for t in &ans.turns {
                        let job = LayoutJob::simple(t.answer.trim().to_owned(), font.clone(), pal().text_dim, f32::INFINITY);
                        ui.add(egui::Label::new(job).wrap().selectable(true));
                        ui.add_space(6.0);
                        let said = match (t.reply.as_str(), t.draft) {
                            ("", true) => "Draft it".to_owned(),
                            (r, true) => format!("Draft it: {r}"),
                            (r, false) => r.to_owned(),
                        };
                        ui.label(egui::RichText::new(format!("You: {said}")).color(pal().accent));
                        ui.add_space(6.0);
                    }
                    match &ans.state {
                        State::Waiting => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Claude is thinking…");
                            });
                        }
                        State::Failed(e) => {
                            ui.colored_label(Color32::from_rgb(235, 110, 110), e);
                        }
                        State::Refused => {
                            ui.colored_label(Color32::from_rgb(235, 110, 110), "Claude declined to answer this request.");
                        }
                        State::Streaming | State::Done => {}
                    }
                    if !ans.text.is_empty() && ans.state != State::Refused {
                        let job = (ans.rewrites() && ans.state == State::Done)
                            .then(|| diff_job(&ans.original, ans.text.trim(), font.clone(), ink))
                            .flatten()
                            .unwrap_or_else(|| LayoutJob::simple(ans.text.clone(), font.clone(), ink, f32::INFINITY));
                        ui.add(egui::Label::new(job).wrap().selectable(true));
                    }
                    if ans.state == State::Streaming {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new("writing…").color(pal().text_dim));
                        });
                    }
                });
                ui.add_space(4.0);
                let done = ans.state == State::Done && !ans.text.trim().is_empty();
                ui.horizontal(|ui| {
                    if ans.rewrites() {
                        let why = applicable.err().unwrap_or("Put this text in place of the marked passage (Ctrl+Z undoes it)");
                        let button = ui.add_enabled(applicable.is_ok(), egui::Button::new("Apply"));
                        apply = button.on_hover_text(why).on_disabled_hover_text(why).clicked();
                    }
                    if ui.add_enabled(!ans.text.is_empty(), egui::Button::new("Copy")).clicked() {
                        ui.ctx().copy_text(ans.text.trim().to_owned());
                    }
                    if ans.command.knows_story() {
                        let why = match unchanged {
                            _ if ans.pinned => "Pinned",
                            Err(e) => e,
                            Ok(()) => "Keep this on a post-it over the passage",
                        };
                        let button = ui.add_enabled(done && !ans.pinned && unchanged.is_ok(), egui::Button::new("Pin as note"));
                        pin = button.on_hover_text(why).on_disabled_hover_text(why).clicked();
                    }
                    if matches!(ans.command, Command::Bridge(_)) && !ans.draft {
                        let why = "Claude fills the gap in the passage, from what you settled; say how in the reply field \
                            first if you like";
                        if ui.add_enabled(done, egui::Button::new("Draft it")).on_hover_text(why).clicked() {
                            follow = Some(true);
                        }
                    }
                });
                if ans.command.knows_story() && ans.state == State::Done {
                    let hint = if ans.draft { "Ask for changes to the draft…" } else { "Reply to Claude…" };
                    let field = ui.add(egui::TextEdit::singleline(&mut ans.reply).hint_text(hint).desired_width(f32::INFINITY));
                    if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !ans.reply.trim().is_empty() {
                        follow = Some(false);
                    }
                }
            });
        if apply {
            self.apply_answer(ctx);
        } else if pin {
            self.pin_answer();
        } else if let Some(draft) = follow {
            self.follow_up(ctx, draft);
        } else if !open {
            self.answer = None;
        }
    }

    /// Can the answer replace the passage it is about? If not, why not.
    fn applicable(&self) -> Result<(), &'static str> {
        let Some(ans) = &self.answer else { return Err("No answer") };
        if !ans.rewrites() || ans.state != State::Done || ans.text.trim().is_empty() {
            return Err("Claude has not finished a new version of the passage yet");
        }
        if ans.original.contains([IMAGE_CHAR, PAGE_BREAK]) {
            return Err("The passage holds a picture or page break; copy the text instead");
        }
        if ans.draft && !ans.keeps_ends(&ans.text) {
            return Err("Claude's draft leaves out part of the marked scenes; copy it instead");
        }
        self.unchanged().map_err(|_| "The passage has been edited since; copy the text instead")
    }

    /// Is the passage asked about still where it was, as it was? The answer belongs to it only then.
    fn unchanged(&self) -> Result<(), &'static str> {
        let Some(ans) = &self.answer else { return Err("No answer") };
        let (a, b) = ans.range;
        let now = (b <= self.doc.total_chars()).then(|| &self.doc.flow.text[self.doc.char_to_byte(a)..self.doc.char_to_byte(b)]);
        if now != Some(ans.original.as_str()) {
            return Err("The passage has been edited since");
        }
        Ok(())
    }

    /// Put the answer in place of the passage it is about (one undo step), and close the panel.
    pub fn apply_answer(&mut self, ctx: &egui::Context) -> bool {
        if self.applicable().is_err() {
            return false;
        }
        let Some(ans) = self.answer.take() else { return false };
        let (a, b) = ans.range;
        self.set_caret(ctx, a, false);
        self.replace_range(ctx, a, b, ans.text.trim());
        true
    }

    /// An answer that has already arrived in full, as if from Claude.
    #[cfg(test)]
    pub fn fake_answer(&mut self, a: usize, b: usize, command: Command, text: &str) {
        self.answer = Some(self.new_answer(a, b, command, Pos2::ZERO));
        self.fake_text(text);
    }

    /// The answer being written arrives in full, as if from Claude, instead of whatever Claude
    /// would say (in tests, that it may not be called).
    #[cfg(test)]
    pub fn fake_text(&mut self, text: &str) {
        let Some(ans) = &mut self.answer else { return };
        let (tx, rx) = channel();
        tx.send(Msg::Text(text.to_owned())).unwrap();
        tx.send(Msg::Done).unwrap();
        (ans.rx, ans.text, ans.state) = (rx, String::new(), State::Waiting);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    /// Asks the real Claude through the Claude Code CLI: `cargo test live -- --ignored`.
    #[test]
    #[ignore]
    fn live_grammar_check() {
        allow_live();
        let (tx, rx) = channel();
        let ctx = egui::Context::default();
        let prompt = format!("<passage>\nTheir is a dog in the gardn.\n</passage>\n\n{}", Command::Grammar.instruction());
        ask(&prompt, &tx, &ctx, &AtomicBool::new(false)).unwrap();
        let text: String = rx.try_iter().filter_map(|m| if let Msg::Text(t) = m { Some(t) } else { None }).collect();
        println!("answer: {text}");
        assert!(text.contains("There") && text.contains("garden"));
    }

    #[test]
    #[ignore]
    fn live_bridge_ideas() {
        allow_live();
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        let (before, after) = ("fanden im Obergeschoss die Dachluke geöffnet.\n- Bist du noch da? fragte Edwin nach oben.\n\
            Doch keine Antwort.\n- Vielleicht ist er weg!\n- Aber wohin? wollte Mia wissen.\n", "\nNach dem Gottesdienst saßen \
            die beiden auf der niedrigen Mauer hinter der Kirche. Edwin sprach leise.\n- Er hat immer wieder dasselbe \
            gesagt: Em kalma rus. Keine Ahnung, was das bedeuten soll.\n- Em kalma rus, wiederholte ihr Großvater \
            langsam. Es bedeutet: Kein Weg zurück.");
        app.doc.flow.text = format!("{before}-----{after}\n"); // only the text is read to build the prompt
        let end = app.doc.flow.text.chars().count() - 1;
        let note = "Edwin und Mia verstecken einen Jungen aus einem fernen Land auf dem Dachboden; er wird gesucht.";
        let note = Command::Bridge(note.into());
        let said = |prompt: &str| {
            let (tx, rx) = channel();
            ask(prompt, &tx, &ctx, &AtomicBool::new(false)).unwrap();
            rx.try_iter().filter_map(|m| if let Msg::Text(t) = m { Some(t) } else { None }).collect::<String>()
        };
        let mut answer = app.new_answer(0, end, note, Pos2::ZERO);
        assert!(answer.first.contains("Dachluke") && answer.first.contains("Kein Weg zurück"));
        let ideas = said(&answer.sent());
        println!("ideas: {ideas}\n");
        assert!(ideas.contains("Großvater") || ideas.contains("Junge"));

        // Drafting the one the writer likes.
        let reply = "Die mit dem Zeitsprung, aber kurz: zwei, drei Absätze.".to_owned();
        answer.turns.push(Turn { answer: ideas, reply, draft: true });
        let draft = said(&answer.sent());
        println!("draft: {draft}");
        assert!(answer.keeps_ends(&draft), "the draft keeps both scenes around the gap");
    }

    #[test]
    fn tests_do_not_call_claude() {
        let r = run_claude(MODEL, "", "hi", "low", &AtomicBool::new(false), &mut |_| panic!("Claude answered"));
        assert_eq!(r.err().as_deref(), Some("Tests do not call Claude"));
    }

    #[test]
    fn points_inside_a_loop_are_found() {
        let square = [pos2(0.0, 0.0), pos2(10.0, 0.0), pos2(10.0, 10.0), pos2(0.0, 10.0)];
        assert!(inside(&square, pos2(5.0, 5.0)));
        assert!(!inside(&square, pos2(15.0, 5.0)));
        assert!(!inside(&square, pos2(5.0, -1.0)));
    }

    #[test]
    fn diff_marks_only_what_changed() {
        let job = diff_job("the qick fox", "the quick fox", FontId::proportional(10.0), Color32::BLACK).unwrap();
        let parts: Vec<(&str, bool)> = job
            .sections
            .iter()
            .map(|s| (&job.text[s.byte_range.start.0..s.byte_range.end.0], s.format.background != Color32::TRANSPARENT))
            .collect();
        let marked = |word: &str| parts.iter().find(|(t, _)| t.contains(word)).map(|&(_, m)| m);
        assert_eq!(marked("quick"), Some(true));
        assert_eq!((marked("the"), marked("fox")), (Some(false), Some(false)));
        assert!(job.text.contains("qick"), "the removed word is shown struck through");
    }
}
