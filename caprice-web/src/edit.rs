//! The edit layer. Every change to the document goes through here as an `Edit`, so one place
//! keeps notes anchored, remembers how to undo, and tells the rest of the app what changed.

use crate::model::{Doc, Note, Style};

/// A run of text together with its per-char styles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Piece {
    pub text: String,
    pub styles: Vec<Style>,
}

impl Piece {
    pub fn chars(&self) -> usize {
        self.styles.len()
    }

    pub fn plain(text: &str, style: &Style) -> Self {
        Self { text: text.to_owned(), styles: vec![style.clone(); text.chars().count()] }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// Replace `old` at char `at` with `new` (insert: `old` empty, delete: `new` empty).
    Replace { at: usize, old: Piece, new: Piece },
    /// Change styles only; both vectors have the same length.
    Restyle { at: usize, old: Vec<Style>, new: Vec<Style> },
}

impl Edit {
    pub fn insert(at: usize, text: &str, style: &Style) -> Self {
        Edit::Replace { at, old: Piece::default(), new: Piece::plain(text, style) }
    }

    pub fn inverse(&self) -> Edit {
        match self {
            Edit::Replace { at, old, new } => Edit::Replace { at: *at, old: new.clone(), new: old.clone() },
            Edit::Restyle { at, old, new } => Edit::Restyle { at: *at, old: new.clone(), new: old.clone() },
        }
    }

    /// Where the caret belongs after this edit has been applied.
    pub fn caret_after(&self) -> usize {
        match self {
            Edit::Replace { at, new, .. } => at + new.chars(),
            Edit::Restyle { at, new, .. } => at + new.len(),
        }
    }

    /// Change in (chars, bytes) of the document length.
    pub fn delta(&self) -> (isize, isize) {
        match self {
            Edit::Replace { old, new, .. } => (
                new.chars() as isize - old.chars() as isize,
                new.text.len() as isize - old.text.len() as isize,
            ),
            Edit::Restyle { .. } => (0, 0),
        }
    }

    /// Would `self` followed by `next` read as one continuous act of typing or deleting?
    fn continues_with(&self, next: &Edit) -> bool {
        match (self, next) {
            (Edit::Replace { at: a1, old: o1, new: n1 }, Edit::Replace { at: a2, old: o2, new: n2 }) => {
                let typing = o1.chars() == 0 && o2.chars() == 0 && *a2 == a1 + n1.chars();
                let deleting = n1.chars() == 0 && n2.chars() == 0 && (*a2 + o2.chars() == *a1 || a2 == a1);
                typing || deleting
            }
            // The same text recoloured or restyled again and again, as while dragging in a picker.
            (Edit::Restyle { at: a1, new: n1, .. }, Edit::Restyle { at: a2, new: n2, .. }) => a1 == a2 && n1.len() == n2.len(),
            _ => false,
        }
    }
}

pub struct History {
    undo: Vec<Vec<Edit>>,
    redo: Vec<Vec<Edit>>,
    last_edit_time: f64,
}

impl Default for History {
    fn default() -> Self {
        Self { undo: Vec::new(), redo: Vec::new(), last_edit_time: f64::NEG_INFINITY }
    }
}

impl History {
    const MAX: usize = 1000;
    /// Edits closer together than this, and contiguous, undo as one step.
    const GROUP_GAP: f64 = 1.0;

    fn record(&mut self, edit: Edit, now: f64) {
        self.redo.clear();
        let joins = now - self.last_edit_time < Self::GROUP_GAP;
        self.last_edit_time = now;
        if joins {
            if let Some(group) = self.undo.last_mut() {
                if group.last().is_some_and(|last| last.continues_with(&edit)) {
                    group.push(edit);
                    return;
                }
            }
        }
        self.undo.push(vec![edit]);
        if self.undo.len() > Self::MAX {
            self.undo.remove(0);
        }
    }

    /// Several edits that undo as one step (never merged with typing before or after).
    fn record_group(&mut self, edits: Vec<Edit>) {
        self.redo.clear();
        self.last_edit_time = f64::NEG_INFINITY;
        self.undo.push(edits);
        if self.undo.len() > Self::MAX {
            self.undo.remove(0);
        }
    }

    /// Where the last edit began (the last one typed, not the run of typing it undoes with).
    pub fn last_at(&self) -> Option<usize> {
        match self.undo.last()?.last()? {
            Edit::Replace { at, .. } | Edit::Restyle { at, .. } => Some(*at),
        }
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.last_edit_time = f64::NEG_INFINITY;
    }
}

impl Doc {
    /// Apply an edit and remember it for undo. `now` is in seconds and drives grouping.
    pub fn apply(&mut self, edit: Edit, now: f64) {
        self.apply_raw(&edit);
        self.history.record(edit, now);
    }

    /// Apply several edits in order; undo reverts them all at once.
    pub fn apply_group(&mut self, edits: Vec<Edit>) {
        for edit in &edits {
            self.apply_raw(edit);
        }
        self.history.record_group(edits);
    }

    /// Undo the last step; returns where the caret should go.
    pub fn undo(&mut self) -> Option<usize> {
        let group = self.history.undo.pop()?;
        let mut caret = 0;
        for edit in group.iter().rev() {
            let inv = edit.inverse();
            self.apply_raw(&inv);
            caret = inv.caret_after();
        }
        self.history.redo.push(group);
        self.history.last_edit_time = f64::NEG_INFINITY;
        Some(caret)
    }

    pub fn redo(&mut self) -> Option<usize> {
        let group = self.history.redo.pop()?;
        let mut caret = 0;
        for edit in &group {
            self.apply_raw(edit);
            caret = edit.caret_after();
        }
        self.history.undo.push(group);
        self.history.last_edit_time = f64::NEG_INFINITY;
        Some(caret)
    }

    fn apply_raw(&mut self, edit: &Edit) {
        match edit {
            Edit::Replace { at, old, new } => {
                let b = self.char_to_byte(*at);
                debug_assert_eq!(&self.flow.text[b..b + old.text.len()], old.text);
                self.flow.text.replace_range(b..b + old.text.len(), &new.text);
                self.flow.styles.splice(*at..*at + old.chars(), new.styles.iter().cloned());
                rebase_notes(&mut self.notes, *at, old.chars(), new.chars());
                rebase_scenes(&mut self.scenes, *at, old.chars(), new.chars());
                rebase_boxes(&mut self.boxes, *at, old.chars(), new.chars());
                rebase_number_marks(&mut self.number_marks, *at, old.chars(), new.chars());
            }
            Edit::Restyle { at, new, .. } => {
                self.flow.styles[*at..*at + new.len()].clone_from_slice(new);
            }
        }
        self.version += 1;
    }
}

/// Move note anchors to follow a replacement of `a` chars at `at` by `b` chars. Text typed right
/// at the edge of a note does not join it; a note whose text is deleted shrinks to a point.
fn rebase_notes(notes: &mut [Note], at: usize, a: usize, b: usize) {
    let start = |p: usize| if p < at { p } else if p >= at + a { p + b - a } else { at + b };
    let end = |p: usize| if p <= at { p } else if p >= at + a { p + b - a } else { at };
    for n in notes {
        let (s, e) = (start(n.start), end(n.end));
        (n.start, n.end) = if s > e { (e, e) } else { (s, e) };
    }
}

/// Move scene pins with the same replacement. Text typed right at a pin goes after it, so a
/// scene pinned to a paragraph's start still covers what is typed there; a pin whose text is
/// deleted stays where the text was.
fn rebase_scenes(scenes: &mut [crate::model::Scene], at: usize, a: usize, b: usize) {
    for s in scenes {
        s.at = if s.at <= at { s.at } else if s.at >= at + a { s.at + b - a } else { at };
    }
}

/// Move text boxes' pins with the same replacement, as scene pins move: a box stays with the char
/// it is pinned to, and one whose char is deleted stays where the text was.
fn rebase_boxes(boxes: &mut [crate::model::TextBox], at: usize, a: usize, b: usize) {
    for t in boxes {
        t.at = if t.at <= at { t.at } else if t.at >= at + a { t.at + b - a } else { at };
    }
}

/// Move where page numbering changes the same way: text typed at the start of the page it was
/// set on goes after it, so that page keeps the change.
fn rebase_number_marks(marks: &mut [crate::numbers::NumberMark], at: usize, a: usize, b: usize) {
    for m in marks {
        m.at = if m.at <= at { m.at } else if m.at >= at + a { m.at + b - a } else { at };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> (Doc, Style) {
        let st = Style::new("x");
        let mut d = Doc::new();
        d.flow.text = text.to_owned();
        d.flow.styles = vec![st.clone(); text.chars().count()];
        (d, st)
    }

    fn note(start: usize, end: usize) -> Note {
        Note { id: 1, start, end, text: String::new(), color: 0 }
    }

    #[test]
    fn undo_redo_restore_text_and_styles() {
        let (mut d, st) = doc("hello world");
        let bold = Style { bold: true, ..st.clone() };
        let old = Piece { text: "world".into(), styles: vec![st.clone(); 5] };
        d.apply(Edit::Replace { at: 6, old, new: Piece::plain("there", &bold) }, 0.0);
        assert_eq!(d.flow.text, "hello there");
        assert_eq!(d.flow.styles[6], bold);
        assert_eq!(d.undo(), Some(11)); // caret ends up after the restored text
        assert_eq!(d.flow.text, "hello world");
        assert_eq!(d.flow.styles[6], st);
        assert_eq!(d.redo(), Some(11));
        assert_eq!(d.flow.text, "hello there");
        assert!(d.redo().is_none());
    }

    #[test]
    fn typing_groups_into_one_undo_step_until_a_pause() {
        let (mut d, st) = doc("");
        for (k, c) in "abc".chars().enumerate() {
            d.apply(Edit::insert(k, &c.to_string(), &st), 0.1 * k as f64);
        }
        d.apply(Edit::insert(3, "d", &st), 5.0); // after a pause: its own step
        assert_eq!(d.flow.text, "abcd");
        d.undo();
        assert_eq!(d.flow.text, "abc");
        d.undo();
        assert_eq!(d.flow.text, "");
        assert!(d.undo().is_none());
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let (mut d, st) = doc("");
        d.apply(Edit::insert(0, "a", &st), 0.0);
        d.undo();
        d.apply(Edit::insert(0, "b", &st), 9.0);
        assert!(d.redo().is_none());
    }

    #[test]
    fn text_boxes_stay_with_their_char_and_where_it_was_once_it_is_deleted() {
        let (mut d, st) = doc("one two three");
        let style = st.clone();
        let pinned = |at| crate::model::TextBox {
            id: 1, at, pos: Default::default(), size: Default::default(), text: String::new(), style: style.clone(),
            look: Default::default(), color: [0, 0, 0], behind: false, clear: false,
        };
        d.boxes = vec![pinned(4), pinned(8)];
        d.apply(Edit::Replace { at: 0, old: Piece::default(), new: Piece::plain("so ", &st) }, 0.0);
        assert_eq!((d.boxes[0].at, d.boxes[1].at), (7, 11), "moved along with their text");
        let old = Piece::plain("two ", &st);
        d.apply(Edit::Replace { at: 7, old, new: Piece::default() }, 1.0);
        assert_eq!((d.boxes[0].at, d.boxes[1].at), (7, 7), "the one whose char went stays where it was");
    }

    #[test]
    fn notes_follow_edits() {
        let (mut d, st) = doc("one two three");
        d.notes.push(note(4, 7)); // "two"
        d.apply(Edit::insert(0, "zero ", &st), 0.0); // before it
        assert_eq!((d.notes[0].start, d.notes[0].end), (9, 12));
        d.apply(Edit::insert(10, "X", &st), 1.0); // inside it
        assert_eq!((d.notes[0].start, d.notes[0].end), (9, 13));
        d.apply(Edit::insert(13, "!", &st), 2.0); // right at its end: not included
        assert_eq!((d.notes[0].start, d.notes[0].end), (9, 13));
        d.apply(Edit::insert(9, "?", &st), 3.0); // right at its start: not included
        assert_eq!((d.notes[0].start, d.notes[0].end), (10, 14));
    }

    #[test]
    fn deleting_a_notes_text_leaves_a_point_note() {
        let (mut d, _) = doc("one two three");
        d.notes.push(note(4, 7));
        let old = Piece { text: "two".into(), styles: d.flow.styles[4..7].to_vec() };
        d.apply(Edit::Replace { at: 4, old, new: Piece::default() }, 0.0);
        assert_eq!((d.notes[0].start, d.notes[0].end), (4, 4));
    }

    #[test]
    fn delta_counts_chars_and_bytes() {
        let old = Piece { text: "ab".into(), styles: vec![Style::new("x"); 2] };
        let e = Edit::Replace { at: 0, old, new: Piece::plain("äöü", &Style::new("x")) };
        assert_eq!(e.delta(), (1, 4));
        assert_eq!(e.inverse().delta(), (-1, -4));
        assert_eq!(Edit::Restyle { at: 3, old: vec![], new: vec![] }.delta(), (0, 0));
    }

    #[test]
    fn undoing_a_format_change_puts_the_caret_after_it() {
        let (mut d, st) = doc("hello world");
        let bold = Style { bold: true, ..st.clone() };
        d.apply(Edit::Restyle { at: 6, old: vec![st.clone(); 5], new: vec![bold; 5] }, 0.0);
        assert!(d.flow.styles[6].bold);
        assert_eq!(d.undo(), Some(11));
        assert!(!d.flow.styles[6].bold);
        assert_eq!(d.redo(), Some(11));
    }

    #[test]
    fn deleting_groups_backwards_and_forwards_but_not_with_typing() {
        let (mut d, st) = doc("abcdef");
        let del = |d: &Doc, at: usize| {
            let b = d.char_to_byte(at);
            let old = Piece { text: d.flow.text[b..b + 1].into(), styles: vec![d.flow.styles[at].clone()] };
            Edit::Replace { at, old, new: Piece::default() }
        };
        // Backspace twice from the end, then delete forward twice at the front: two steps.
        d.apply(del(&d, 5), 0.0);
        d.apply(del(&d, 4), 0.1);
        d.apply(del(&d, 0), 0.2);
        d.apply(del(&d, 0), 0.3);
        assert_eq!(d.flow.text, "cd");
        d.undo();
        assert_eq!(d.flow.text, "abcd");
        d.undo();
        assert_eq!(d.flow.text, "abcdef");
        // Typing right after deleting, or typing somewhere else, starts a new step.
        d.apply(del(&d, 5), 1.0);
        d.apply(Edit::insert(5, "X", &st), 1.1);
        d.apply(Edit::insert(0, "Y", &st), 1.2);
        d.undo();
        assert_eq!(d.flow.text, "abcdeX");
        d.undo();
        assert_eq!(d.flow.text, "abcde");
        d.undo();
        assert_eq!(d.flow.text, "abcdef");
    }

    #[test]
    fn grouping_needs_edits_less_than_a_second_apart() {
        let (mut d, st) = doc("");
        d.apply(Edit::insert(0, "a", &st), 2.0);
        d.apply(Edit::insert(1, "b", &st), 2.5); // late in the session, still quick: same step
        d.apply(Edit::insert(2, "c", &st), 3.5); // exactly a second later: a new step
        d.undo();
        assert_eq!(d.flow.text, "ab");
        d.undo();
        assert_eq!(d.flow.text, "");
    }

    #[test]
    fn grouped_edits_undo_together_and_each_group_is_kept() {
        let (mut d, st) = doc("");
        d.apply_group(vec![Edit::insert(0, "a", &st), Edit::insert(1, "b", &st)]);
        d.apply_group(vec![Edit::insert(2, "c", &st)]);
        d.undo();
        assert_eq!(d.flow.text, "ab");
        d.undo();
        assert_eq!(d.flow.text, "");
    }

    #[test]
    fn replacing_across_a_notes_edges() {
        let (mut d, st) = doc("one two three");
        d.notes.push(note(4, 7)); // "two"
        let replace = |d: &mut Doc, a: usize, b: usize, new: &str| {
            let (ba, bb) = (d.char_to_byte(a), d.char_to_byte(b));
            let old = Piece { text: d.flow.text[ba..bb].into(), styles: d.flow.styles[a..b].to_vec() };
            d.apply(Edit::Replace { at: a, old, new: Piece::plain(new, &st) }, 0.0);
        };
        replace(&mut d, 0, 5, "ONE-T!"); // "one t" -> the note's start was replaced, its end shifts
        assert_eq!(d.flow.text, "ONE-T!wo three");
        assert_eq!((d.notes[0].start, d.notes[0].end), (6, 8));
        replace(&mut d, 7, 10, "XY"); // "o t" -> the note's end was replaced
        assert_eq!(d.flow.text, "ONE-T!wXYhree");
        assert_eq!((d.notes[0].start, d.notes[0].end), (6, 7));
        d.notes[0] = note(5, 6);
        replace(&mut d, 4, 8, "Z"); // the whole note was inside the replaced text: a point note
        assert_eq!((d.notes[0].start, d.notes[0].end), (4, 4));
    }
}
