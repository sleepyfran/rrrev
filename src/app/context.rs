//! Reading past a hunk's edges: more of the file above or below it, or the
//! whole function it's in, straight from the file's text on disk.

use super::{App, Row};
use crate::diff::{Diff, DiffLine, Hunk, Side, scope};

/// Lines `[e` / `]e` add each time.
pub const EXPAND_STEP: usize = 20;

/// A hunk's extra context: lines shown above and below it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Expansion {
    pub above: usize,
    pub below: usize,
}

/// Line number on `side`, if the line has one there.
fn number(line: &DiffLine, side: Side) -> Option<u32> {
    match side {
        Side::New => line.new_no,
        Side::Old => line.old_no,
    }
}

/// The first and last line numbers a hunk covers on `side`. A hunk with no
/// lines on that side sits right after its start line.
pub(super) fn span(hunk: &Hunk, side: Side) -> (u32, u32) {
    let first = hunk.lines.iter().find_map(|l| number(l, side));
    let last = hunk.lines.iter().rev().find_map(|l| number(l, side));
    let start = match side {
        Side::New => hunk.new_start,
        Side::Old => hunk.old_start,
    };
    match (first, last) {
        (Some(first), Some(last)) => (first, last),
        _ => (start + 1, start),
    }
}

/// The old and new line numbers of context line `no` (on `side`) next to
/// `hunk`, before it when `above`.
pub fn context_numbers(hunk: &Hunk, side: Side, no: u32, above: bool) -> (Option<u32>, Option<u32>) {
    // Outside a hunk both sides run in step, offset by what the hunks so far
    // added or removed.
    let (old_start, new_start) = (hunk.old_start as i64, hunk.new_start as i64);
    let shift = if above {
        new_start - old_start
    } else {
        (new_start + hunk.new_lines as i64) - (old_start + hunk.old_lines as i64)
    };
    let other = match side {
        Side::New => no as i64 - shift,
        Side::Old => no as i64 + shift,
    };
    let other = u32::try_from(other).ok().filter(|&n| n > 0);
    match side {
        Side::New => (other, Some(no)),
        Side::Old => (Some(no), other),
    }
}

impl App {
    pub fn expansion(&self, file: usize, hunk: usize) -> Expansion {
        self.expanded.get(&(file, hunk)).copied().unwrap_or_default()
    }

    /// Context line numbers to show above and below a hunk, in order.
    pub(super) fn context_lines(&self, file: usize, hunk: usize) -> (Vec<u32>, Vec<u32>) {
        let Some((_, side)) = self.diff.context_source(file) else { return (Vec::new(), Vec::new()) };
        let (first, last) = span(self.hunk(file, hunk), side);
        let e = self.expansion(file, hunk);
        let above = (first.saturating_sub(e.above as u32).max(1)..first).collect();
        let below = (last + 1..=last + e.below as u32).collect();
        (above, below)
    }

    /// Lines between a hunk and its neighbour (or the file's edge) still
    /// hidden, above it and below it.
    pub fn hidden_around(&self, file: usize, hunk: usize) -> (usize, usize) {
        let Some((source, side)) = self.diff.context_source(file) else { return (0, 0) };
        let (above_room, below_room) = room(&self.diff, file, hunk, side, source.len());
        // A neighbour's own expansion uses up part of the gap too.
        let e = self.expansion(file, hunk);
        let previous = hunk.checked_sub(1).map_or(0, |h| self.expansion(file, h).below);
        let next = self.expansion(file, hunk + 1).above;
        (
            above_room.saturating_sub(e.above + previous),
            if hunk + 1 < self.diff.files[file].hunks.len() { 0 } else { below_room.saturating_sub(e.below + next) },
        )
    }

    /// `[e` / `]e`: `n` steps more context above or below the cursor's hunk.
    pub(super) fn expand(&mut self, steps: usize, above: bool) {
        let Some((file, hunk)) = self.cursor_hunk() else {
            self.message = Some("Move to a hunk to show more around it".to_string());
            return;
        };
        let Some((source, side)) = self.diff.context_source(file) else {
            self.message = Some("Can't read this file's text".to_string());
            return;
        };
        let (above_room, below_room) = room(&self.diff, file, hunk, side, source.len());
        let previous = hunk.checked_sub(1).map_or(0, |h| self.expansion(file, h).below);
        let next = self.expansion(file, hunk + 1).above;
        let mut e = self.expansion(file, hunk);
        let add = steps.saturating_mul(EXPAND_STEP);
        let before = e;
        if above {
            e.above = e.above.saturating_add(add).min(above_room.saturating_sub(previous));
        } else {
            e.below = e.below.saturating_add(add).min(below_room.saturating_sub(next));
        }
        if e == before {
            self.message = Some(format!("Nothing more {} this hunk", if above { "above" } else { "below" }));
            return;
        }
        self.set_expansion(file, hunk, e);
    }

    /// `ze`: shows the whole definition the hunk is in.
    pub(super) fn expand_scope(&mut self) {
        let Some((file, hunk)) = self.cursor_hunk() else {
            self.message = Some("Move to a hunk to show its function".to_string());
            return;
        };
        let Some((source, side)) = self.diff.context_source(file) else {
            self.message = Some("Can't read this file's text".to_string());
            return;
        };
        let (first, last) = span(self.hunk(file, hunk), side);
        // The innermost definition that holds the whole hunk.
        let scope = scope::enclosing(source, first).into_iter().rev().find(|s| s.end >= last);
        let Some(scope) = scope else {
            self.message = Some("This hunk isn't inside a function".to_string());
            return;
        };
        let (above_room, below_room) = room(&self.diff, file, hunk, side, source.len());
        let previous = hunk.checked_sub(1).map_or(0, |h| self.expansion(file, h).below);
        let next = self.expansion(file, hunk + 1).above;
        let e = self.expansion(file, hunk);
        let want = Expansion {
            above: (first.saturating_sub(scope.line) as usize).max(e.above).min(above_room.saturating_sub(previous)),
            below: (scope.end.saturating_sub(last) as usize).max(e.below).min(below_room.saturating_sub(next)),
        };
        self.message = Some(format!("Showing {} (lines {}-{})", scope.label, scope.line, scope.end));
        if want != e {
            self.set_expansion(file, hunk, want);
        }
    }

    /// `zE`: back to the hunks as git shows them.
    pub(super) fn reset_context(&mut self) {
        if self.expanded.is_empty() {
            self.message = Some("No extra context to hide".to_string());
            return;
        }
        // Off a context row first, so the cursor lands on its hunk.
        if let Some(Row::Context { file, hunk, .. }) = self.current_row()
            && let Some(idx) = self.rows.iter().position(|r| *r == Row::Hunk { file, hunk })
        {
            self.cursor = idx;
        }
        self.expanded.clear();
        self.rebuild_rows();
    }

    fn set_expansion(&mut self, file: usize, hunk: usize, e: Expansion) {
        if e == Expansion::default() {
            self.expanded.remove(&(file, hunk));
        } else {
            self.expanded.insert((file, hunk), e);
        }
        self.rebuild_rows();
    }

    /// The hunk the cursor is in or on.
    pub(super) fn cursor_hunk(&self) -> Option<(usize, usize)> {
        let row = self.current_row()?;
        Some((row.file()?, row.hunk()?))
    }
}

/// How many lines lie between a hunk and the hunk before it (or the top of
/// the file), and between it and the hunk after (or the end).
fn room(diff: &Diff, file: usize, hunk: usize, side: Side, len: usize) -> (usize, usize) {
    let hunks = &diff.files[file].hunks;
    let (first, last) = span(&hunks[hunk], side);
    let previous_last = hunk.checked_sub(1).map_or(0, |h| span(&hunks[h], side).1);
    let next_first = hunks.get(hunk + 1).map_or(len as u32 + 1, |h| span(h, side).0);
    let above = first.saturating_sub(previous_last + 1) as usize;
    let below = next_first.saturating_sub(last + 1) as usize;
    (above, below)
}

/// Whether context line `no` of a hunk's file sits above the hunk.
pub fn is_above(hunk: &Hunk, side: Side, no: u32) -> bool {
    no < span(hunk, side).0
}

/// A context line's text, if the file has it.
pub fn context_text(diff: &Diff, file: usize, no: u32) -> Option<&str> {
    let (source, _) = diff.context_source(file)?;
    source.get((no as usize).checked_sub(1)?).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::Action;
    use crate::diff::{Diff, FileDiff, FileStatus, HunkId, LineKind, Sources};

    /// A 60-line file `f.rs` with one function around lines 20-40 and two
    /// hunks changing lines 25 and 50.
    pub fn sourced_app() -> App {
        let mut text: Vec<String> = (1..=60).map(|i| format!("line{i}")).collect();
        text[19] = "fn work() {".into();
        for line in &mut text[20..39] {
            *line = format!("    {line}");
        }
        text[39] = "}".into();
        let changed = |no: u32| Hunk {
            id: HunkId::default(),
            header: format!("@@ -{no},1 +{no},1 @@"),
            old_start: no,
            old_lines: 1,
            new_start: no,
            new_lines: 1,
            lines: vec![
                DiffLine { kind: LineKind::Removed, old_no: Some(no), new_no: None, content: "old".into() },
                DiffLine { kind: LineKind::Added, old_no: None, new_no: Some(no), content: text[no as usize - 1].clone() },
            ],
        };
        let file = FileDiff {
            old_path: Some("f.rs".into()),
            new_path: Some("f.rs".into()),
            status: FileStatus::Modified,
            is_binary: false,
            hunks: vec![changed(25), changed(50)],
        };
        let sources = vec![Sources { old: None, new: Some(std::sync::Arc::new(text)) }];
        let diff = Diff::new(vec![file]).with_sources(sources);
        App::new(String::new(), diff, crate::review::ReviewState::default())
    }

    fn context_rows(app: &App) -> Vec<u32> {
        app.rows.iter().filter_map(|r| if let Row::Context { no, .. } = r { Some(*no) } else { None }).collect()
    }

    #[test]
    fn expands_above_and_below_without_overlapping_the_next_hunk() {
        let mut app = sourced_app();
        app.update(Action::NextHunk(1));
        assert_eq!(app.hidden_around(0, 0), (24, 0));
        app.update(Action::ExpandAbove(1));
        assert_eq!(context_rows(&app), (5..25).collect::<Vec<_>>());
        // Rows: header, hunk, 20 context lines, then the two changed lines.
        assert_eq!(app.rows[2], Row::Context { file: 0, hunk: 0, no: 5 });
        app.update(Action::ExpandAbove(5));
        assert_eq!(context_rows(&app).first(), Some(&1));
        app.update(Action::ExpandAbove(1));
        assert_eq!(app.message.as_deref(), Some("Nothing more above this hunk"));

        // Between the hunks: 24 lines, shared by one's below and the next's above.
        app.update(Action::ExpandBelow(2));
        assert_eq!(app.expansion(0, 0).below, 24);
        assert_eq!(app.hidden_around(0, 1).0, 0);
        assert_eq!(app.hidden_around(0, 1).1, 10);
    }

    #[test]
    fn shows_the_whole_function() {
        let mut app = sourced_app();
        app.update(Action::NextHunk(1));
        app.update(Action::ExpandScope);
        assert_eq!(app.expansion(0, 0), Expansion { above: 5, below: 15 });
        assert_eq!(app.message.as_deref(), Some("Showing fn work() (lines 20-40)"));
        let rows = context_rows(&app);
        assert_eq!((rows[0], *rows.last().unwrap()), (20, 40));

        // The second hunk isn't in a function.
        app.update(Action::NextHunk(1));
        app.update(Action::ExpandScope);
        assert_eq!(app.message.as_deref(), Some("This hunk isn't inside a function"));

        app.update(Action::ResetContext);
        assert!(context_rows(&app).is_empty());
    }

    #[test]
    fn context_rows_keep_the_hunk_reviewable() {
        let mut app = sourced_app();
        app.update(Action::NextHunk(1));
        app.update(Action::ExpandAbove(1));
        app.update(Action::Down(1));
        assert!(matches!(app.current_row(), Some(Row::Context { .. })));
        app.update(Action::ToggleReviewed);
        assert!(app.is_reviewed(0, 0));
        // Resetting puts the cursor back on the hunk's header.
        app.update(Action::ResetContext);
        assert_eq!(app.current_row(), Some(Row::Hunk { file: 0, hunk: 0 }));
    }

    #[test]
    fn files_without_text_say_so() {
        let mut app = sample_app();
        app.update(Action::Down(2));
        app.update(Action::ExpandAbove(1));
        assert_eq!(app.message.as_deref(), Some("Can't read this file's text"));
        app.update(Action::Top);
        app.update(Action::ExpandAbove(1));
        assert_eq!(app.message.as_deref(), Some("Move to a hunk to show more around it"));
    }

    #[test]
    fn numbers_both_sides_of_context_lines() {
        let hunk = Hunk {
            id: HunkId::default(),
            header: String::new(),
            old_start: 10,
            old_lines: 2,
            new_start: 12,
            new_lines: 5,
            lines: Vec::new(),
        };
        assert_eq!(context_numbers(&hunk, Side::New, 11, true), (Some(9), Some(11)));
        // Below: the hunk added three more lines than it removed.
        assert_eq!(context_numbers(&hunk, Side::New, 20, false), (Some(15), Some(20)));
        assert_eq!(context_numbers(&hunk, Side::Old, 15, false), (Some(15), Some(20)));
        assert_eq!(context_numbers(&hunk, Side::New, 1, true), (None, Some(1)));
    }
}
