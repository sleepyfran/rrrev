//! Reading past a hunk's edges: more of the file above or below it, or the
//! whole function it's in, straight from the file's text on disk.

use super::{App, Row, View};
use crate::diff::{Diff, DiffLine, Hunk, LineKind, Side};

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

/// The first and last changed lines a hunk has on `side`, if it has any.
fn changed_span(hunk: &Hunk, side: Side) -> Option<(u32, u32)> {
    let mut changed = hunk.lines.iter().filter(|l| l.kind != LineKind::Context).filter_map(|l| number(l, side));
    let first = changed.next()?;
    Some((first, changed.next_back().unwrap_or(first)))
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
            // The gap to the next hunk is its to show, unless it's on another page.
            if hunk + 1 < self.diff.files[file].hunks.len() && self.neighbour_elsewhere(file, hunk, false).is_none() {
                0
            } else {
                below_room.saturating_sub(e.below + next)
            },
        )
    }

    /// The hunk next to `hunk` in its file, above or below, and the page it's
    /// on, when that's not the current page. It bounds the context `hunk` can
    /// show, so what's past that point is out of sight here.
    pub fn neighbour_elsewhere(&self, file: usize, hunk: usize, above: bool) -> Option<(usize, usize)> {
        if self.view == View::Files {
            return None;
        }
        let neighbour = if above { hunk.checked_sub(1)? } else { hunk + 1 };
        if neighbour >= self.diff.files[file].hunks.len() {
            return None;
        }
        let pages = self.pages();
        if pages.get(self.page).is_some_and(|p| p.hunks.contains(&(file, neighbour))) {
            return None;
        }
        let page = pages.iter().position(|p| p.hunks.contains(&(file, neighbour)))?;
        Some((neighbour, page))
    }

    /// Where the hunk past `hunk`'s context is, when it's on another page:
    /// `lines 121-135 in group 6 “Not in the plan”`.
    pub fn elsewhere(&self, file: usize, hunk: usize, above: bool) -> Option<String> {
        let (neighbour, page) = self.neighbour_elsewhere(file, hunk, above)?;
        let (_, side) = self.diff.context_source(file)?;
        let (first, last) = span(self.hunk(file, neighbour), side);
        let lines = if first >= last { format!("line {first}") } else { format!("lines {first}-{last}") };
        let place = match self.view {
            View::Plan => format!("group {} “{}”", page + 1, self.pages()[page].title),
            View::Stack | View::Files => format!("“{}”", self.page_label(page)),
        };
        Some(format!("{lines} in {place}"))
    }

    /// The status line's word on context cut short by a hunk on another page.
    fn wall_message(&self, file: usize, hunk: usize, above: bool) -> Option<String> {
        let at = self.elsewhere(file, hunk, above)?;
        let word = if above { "above" } else { "below" };
        Some(format!("Next {word}: {at} · gf shows it in the file"))
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
        let (wanted, room) = if above {
            (e.above.saturating_add(add), above_room.saturating_sub(previous))
        } else {
            (e.below.saturating_add(add), below_room.saturating_sub(next))
        };
        if above { e.above = wanted.min(room) } else { e.below = wanted.min(room) }
        let wall = (wanted > room).then(|| self.wall_message(file, hunk, above)).flatten();
        if e == before {
            self.message = Some(
                wall.unwrap_or_else(|| format!("Nothing more {} this hunk", if above { "above" } else { "below" })),
            );
            return;
        }
        self.message = wall;
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
        // The innermost definition that holds the hunk's changes: its context
        // lines can start above the definition, or run past its end.
        let (from, to) = changed_span(self.hunk(file, hunk), side).unwrap_or((first, last));
        let scope = self.diff.scopes_at(file, side, from).into_iter().rev().find(|s| s.end >= to);
        let Some(scope) = scope else {
            self.message = Some("This hunk isn't inside a function".to_string());
            return;
        };
        let (above_room, below_room) = room(&self.diff, file, hunk, side, source.len());
        let previous = hunk.checked_sub(1).map_or(0, |h| self.expansion(file, h).below);
        let next = self.expansion(file, hunk + 1).above;
        let e = self.expansion(file, hunk);
        let (up, down) = (first.saturating_sub(scope.line) as usize, scope.end.saturating_sub(last) as usize);
        let (up_room, down_room) = (above_room.saturating_sub(previous), below_room.saturating_sub(next));
        let want = Expansion { above: up.max(e.above).min(up_room), below: down.max(e.below).min(down_room) };
        let mut message = format!("Showing {} (lines {}-{})", scope.label, scope.line, scope.end);
        // Cut short by a hunk on another page: say where the rest is.
        let walls: Vec<String> = [(true, up > up_room), (false, down > down_room)]
            .into_iter()
            .filter(|&(_, cut)| cut)
            .filter_map(|(above, _)| self.elsewhere(file, hunk, above))
            .collect();
        if !walls.is_empty() {
            message += &format!(" · next: {} · gf shows it in the file", walls.join(" and "));
        }
        self.message = Some(message);
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

/// The sign a line gets when quoted in a comment or prompt.
pub fn sign(kind: LineKind) -> char {
    match kind {
        LineKind::Added => '+',
        LineKind::Removed => '-',
        LineKind::Context | LineKind::NoNewline => ' ',
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::Action;
    use crate::diff::{Diff, FileDiff, FileStatus, HunkId, Sources};

    /// A 60-line file `f.rs` with one function around lines 20-40 and two
    /// hunks changing lines 25 and 50.
    pub fn sourced_app() -> App {
        sourced_with(&[25, 50], crate::review::ReviewState::default())
    }

    /// [`sourced_app`]'s file with hunks changing `changes`.
    fn sourced_with(changes: &[u32], review: crate::review::ReviewState) -> App {
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
            hunks: changes.iter().map(|&no| changed(no)).collect(),
        };
        let sources = vec![Sources { old: None, new: Some(std::sync::Arc::new(text)) }];
        let diff = Diff::new(vec![file]).with_sources(sources);
        App::new(String::new(), diff, review)
    }

    /// [`sourced_with`], planned with the second hunk first and the first
    /// one in a later group, the cursor on the second's header.
    fn split_plan(changes: &[u32]) -> App {
        use crate::organizer::{Plan, PlanGroup};
        let app = sourced_with(changes, Default::default());
        let id = |h: usize| app.diff.files[0].hunks[h].id.clone();
        let group = |title: &str, h: usize| PlanGroup { title: title.into(), rationale: String::new(), hunks: vec![id(h)] };
        let mut review = crate::review::ReviewState::default();
        review.set_plan(Plan { groups: vec![group("Later", 1), group("Earlier", 0)], ..Plan::default() });
        let mut app = sourced_with(changes, review);
        assert_eq!(app.view, View::Plan);
        app.cursor = app.rows.iter().position(|r| *r == Row::Hunk { file: 0, hunk: 1 }).unwrap();
        app
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
    fn shows_the_function_when_the_hunk_starts_above_it() {
        // `def run(` on line 4 with its arguments split over lines, a body to
        // line 20, and a hunk changing line 5 whose context starts on line 2.
        let mut text: Vec<String> = vec!["x = 1".into(), String::new(), String::new(), "def run(".into()];
        text.extend(["    a,", "    b,", "):"].map(String::from));
        text.extend((8..=20).map(|i| format!("    y{i} = {i}")));
        text.extend([String::new(), "z = 3".into()]);
        let context = |no: u32| DiffLine {
            kind: LineKind::Context,
            old_no: Some(no),
            new_no: Some(no),
            content: text[no as usize - 1].clone(),
        };
        let mut lines: Vec<DiffLine> = (2..=4).map(context).collect();
        lines.push(DiffLine { kind: LineKind::Removed, old_no: Some(5), new_no: None, content: "    c,".into() });
        lines.push(DiffLine { kind: LineKind::Added, old_no: None, new_no: Some(5), content: text[4].clone() });
        lines.extend((6..=8).map(context));
        let hunk = Hunk {
            id: HunkId::default(),
            header: "@@ -2,7 +2,7 @@".into(),
            old_start: 2,
            old_lines: 7,
            new_start: 2,
            new_lines: 7,
            lines,
        };
        let file = FileDiff {
            old_path: Some("run.py".into()),
            new_path: Some("run.py".into()),
            status: FileStatus::Modified,
            is_binary: false,
            hunks: vec![hunk],
        };
        let sources = vec![Sources { old: None, new: Some(std::sync::Arc::new(text)) }];
        let diff = Diff::new(vec![file]).with_sources(sources);
        let mut app = App::new(String::new(), diff, crate::review::ReviewState::default());
        app.update(Action::NextHunk(1));
        app.update(Action::ExpandScope);
        assert_eq!(app.message.as_deref(), Some("Showing def run(a, b) (lines 4-20)"));
        assert_eq!(app.expansion(0, 0), Expansion { above: 0, below: 12 });
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

    #[test]
    fn context_cut_short_by_a_hunk_on_another_page_says_where_it_is() {
        let mut app = split_plan(&[25, 50]);
        app.update(Action::ExpandAbove(1));
        assert_eq!(app.message, None);
        app.update(Action::ExpandAbove(1));
        assert_eq!(app.expansion(0, 1).above, 24);
        let wall = "Next above: line 25 in group 2 “Earlier” · gf shows it in the file";
        assert_eq!(app.message.as_deref(), Some(wall));
        app.update(Action::ExpandAbove(1));
        assert_eq!(app.message.as_deref(), Some(wall));
        assert_eq!(app.hidden_around(0, 1).0, 0);
        assert_eq!(app.elsewhere(0, 1, true).as_deref(), Some("line 25 in group 2 “Earlier”"));

        // From the other page, the gap below is the earlier hunk's to show,
        // since the hunk past it isn't there.
        app.update(Action::NextFile(1));
        assert_eq!(app.page, 1);
        assert_eq!(app.hidden_around(0, 0), (24, 0));
        app.expanded.clear();
        assert_eq!(app.hidden_around(0, 0), (24, 24));

        // In the files view both are there, so nothing is said.
        app.update(Action::ShowView(View::Files));
        assert_eq!(app.elsewhere(0, 0, false), None);
    }

    #[test]
    fn ze_says_where_the_rest_of_the_function_is() {
        let mut app = split_plan(&[22, 30]);
        app.update(Action::ExpandScope);
        assert_eq!(app.expansion(0, 1).above, 7);
        let message = app.message.clone().unwrap();
        assert!(message.ends_with(" · next: line 22 in group 2 “Earlier” · gf shows it in the file"), "{message}");
    }
}
