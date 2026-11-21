//! The file view: a whole file, read-only, with the same cursor as the diff
//! and the diff's changes marked in its gutter. Enter on a code line opens
//! it there; `q` or `Esc` goes back to the diff where it was.

use super::column::Caret;
use super::{Action, App, Row};
use crate::app::find_matches;
use crate::diff::{FileDiff, LineKind, Side, Source};

/// How a line of the file took part in the diff, for its gutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// Added on the new side, or removed on the old, with nothing in its place.
    Changed,
    /// Replaces lines on the other side.
    Modified,
    /// Lines on the other side went away just above this one.
    Gap,
}

#[derive(Debug, Clone)]
pub struct FileView {
    pub path: String,
    pub side: Side,
    pub lines: Source,
    /// The diff's file this is, for its scopes; `None` for a file outside the diff.
    pub file: Option<usize>,
    /// Per line, how it took part in the diff.
    pub marks: Vec<Option<Mark>>,
    pub caret: Caret,
    /// Index of the first line shown.
    pub scroll: usize,
}

impl FileView {
    pub fn new(path: String, side: Side, lines: Source, file: Option<usize>, marks: Vec<Option<Mark>>) -> Self {
        Self { path, side, lines, file, marks, caret: Caret::default(), scroll: 0 }
    }

    /// The character the cursor is on, in its line.
    pub fn cursor_column(&self) -> usize {
        self.caret.on(&self.lines[..]).unwrap_or(0)
    }

    /// The 1-based line number the cursor is on.
    pub fn line_no(&self) -> u32 {
        self.caret.line as u32 + 1
    }

    /// Where each run of marked lines starts, for `]c` / `[c`.
    fn change_starts(&self) -> Vec<usize> {
        (0..self.marks.len())
            .filter(|&i| self.marks[i].is_some() && (i == 0 || self.marks[i - 1].is_none()))
            .collect()
    }

    fn last(&self) -> usize {
        self.lines.len().saturating_sub(1)
    }

    /// Keeps the cursor's line on screen, `height` lines tall.
    pub fn keep_in_view(&mut self, height: usize) {
        if self.caret.line < self.scroll {
            self.scroll = self.caret.line;
        } else if height > 0 && self.caret.line >= self.scroll + height {
            self.scroll = self.caret.line + 1 - height;
        }
    }

    /// The cursor's line `above` lines from the top of a `height`-line pane.
    fn scroll_with_above(&mut self, above: usize) {
        self.scroll = self.caret.line.saturating_sub(above);
    }

    /// `n` / `N`: the `n`th line with a match after (or before) the
    /// cursor's, wrapping around. Returns whether it wrapped, or `None` when
    /// nothing matches.
    fn search(&mut self, query: &str, n: usize, forward: bool) -> Option<bool> {
        let count = self.lines.len();
        let hits: Vec<usize> = (0..count).filter(|&i| !find_matches(&self.lines[i], query).is_empty()).collect();
        if hits.is_empty() {
            return None;
        }
        let here = self.caret.line;
        let mut wrapped = false;
        let mut at = here;
        for _ in 0..n {
            let next = if forward {
                hits.iter().find(|&&i| i > at).copied()
            } else {
                hits.iter().rev().find(|&&i| i < at).copied()
            };
            at = match next {
                Some(i) => i,
                None => {
                    wrapped = true;
                    if forward { hits[0] } else { hits[hits.len() - 1] }
                }
            };
        }
        self.caret.line = at;
        self.caret.move_to_match(&self.lines[..], query);
        Some(wrapped)
    }
}

/// The gutter marks for one side of a file's diff, `len` lines long.
pub fn marks(file: &FileDiff, side: Side, len: usize) -> Vec<Option<Mark>> {
    let mut marks = vec![None; len];
    let mut set = |no: Option<u32>, mark: Mark| {
        if let Some(slot) = no.and_then(|n| (n as usize).checked_sub(1)).and_then(|i| marks.get_mut(i)) {
            // A gap never hides the line's own change.
            if slot.is_none() || mark != Mark::Gap {
                *slot = Some(mark);
            }
        }
    };
    let (mine, theirs) = match side {
        Side::New => (LineKind::Added, LineKind::Removed),
        Side::Old => (LineKind::Removed, LineKind::Added),
    };
    let number = |l: &crate::diff::DiffLine| match side {
        Side::New => l.new_no,
        Side::Old => l.old_no,
    };
    for hunk in &file.hunks {
        let mut i = 0;
        while i < hunk.lines.len() {
            if !matches!(hunk.lines[i].kind, LineKind::Added | LineKind::Removed) {
                i += 1;
                continue;
            }
            let run_end = (i..hunk.lines.len())
                .find(|&j| !matches!(hunk.lines[j].kind, LineKind::Added | LineKind::Removed))
                .unwrap_or(hunk.lines.len());
            let run = &hunk.lines[i..run_end];
            let has_mine = run.iter().any(|l| l.kind == mine);
            let has_theirs = run.iter().any(|l| l.kind == theirs);
            if has_mine {
                let mark = if has_theirs { Mark::Modified } else { Mark::Changed };
                for l in run.iter().filter(|l| l.kind == mine) {
                    set(number(l), mark);
                }
            } else {
                // Only the other side changed: mark the line after the gap,
                // or the last one when the gap is at the end.
                let after = hunk.lines[run_end..].iter().find_map(number);
                let before = hunk.lines[..i].iter().rev().find_map(number);
                set(after.or(before), Mark::Gap);
            }
            i = run_end;
        }
    }
    marks
}

impl App {
    /// Enter on a code line: the whole file, at the cursor. A removed line
    /// opens the old version, anything else the new one.
    pub(super) fn open_file_view(&mut self) {
        let Some(row) = self.current_row() else { return };
        let (file, side, no) = match row {
            Row::Line { file, hunk, line } => {
                let l = self.line(file, hunk, line);
                match (l.kind, l.old_no, l.new_no) {
                    (LineKind::Removed, Some(no), _) => (file, Side::Old, no),
                    (_, _, Some(no)) => (file, Side::New, no),
                    (_, Some(no), None) => (file, Side::Old, no),
                    _ => return,
                }
            }
            Row::Context { file, no, .. } => match self.diff.context_source(file) {
                Some((_, side)) => (file, side, no),
                None => return,
            },
            _ => return,
        };
        let f = &self.diff.files[file];
        let sources = self.diff.sources.get(file);
        let source = sources.and_then(|s| match side {
            Side::New => s.new.clone(),
            Side::Old => s.old.clone(),
        });
        let Some(lines) = source else {
            self.message = Some(format!("No text to show for {}", f.display_path()));
            return;
        };
        let path = match side {
            Side::New => f.new_path.clone(),
            Side::Old => f.old_path.clone(),
        };
        let path = path.unwrap_or_else(|| f.path().to_string());
        let marks = marks(f, side, lines.len());
        let mut view = FileView::new(path, side, lines, Some(file), marks);
        view.caret = super::column::Caret {
            line: (no as usize).saturating_sub(1).min(view.last()),
            column: self.cursor_column().unwrap_or(0),
        };
        view.scroll_with_above(self.viewport_height / 2);
        self.file_view = Some(view);
    }

    /// Keys while the file view is open; those it has no use for do nothing.
    pub(super) fn update_file_view(&mut self, action: Action) {
        let half = self.half_page().unsigned_abs();
        let height = self.viewport_height;
        let highlighting = self.highlighted_search().is_some();
        let Some(view) = self.file_view.as_mut() else { return };
        let last = view.last();
        match action {
            Action::Down(n) => view.caret.line = view.caret.line.saturating_add(n).min(last),
            Action::Up(n) => view.caret.line = view.caret.line.saturating_sub(n),
            Action::HalfPageDown => view.caret.line = view.caret.line.saturating_add(half).min(last),
            Action::HalfPageUp => view.caret.line = view.caret.line.saturating_sub(half),
            Action::Top => view.caret.line = 0,
            Action::Bottom => view.caret.line = last,
            Action::NextHunk(n) => {
                let here = view.caret.line;
                match view.change_starts().into_iter().filter(|&i| i > here).take(n).last() {
                    Some(i) => view.caret.line = i,
                    None => self.message = Some("No more changes below".to_string()),
                }
            }
            Action::PrevHunk(n) => {
                let here = view.caret.line;
                match view.change_starts().into_iter().rev().filter(|&i| i < here).take(n).last() {
                    Some(i) => view.caret.line = i,
                    None => self.message = Some("No more changes above".to_string()),
                }
            }
            Action::ScrollCursorTop => view.scroll = view.caret.line,
            Action::ScrollCursorCenter => view.scroll_with_above(height / 2),
            Action::ScrollCursorBottom => view.scroll_with_above(height.saturating_sub(1)),
            Action::StartSearch => self.start_search(),
            Action::SearchNext(n) => self.search_next(n, true),
            Action::SearchPrev(n) => self.search_next(n, false),
            Action::ToggleHelp => self.help_scroll = Some(0),
            Action::Cancel if highlighting => self.clear_search_highlight(),
            Action::Cancel | Action::Quit => self.file_view = None,
            Action::JumpBack => self.jump_back(),
            Action::GoTo(kind) => self.go_to(kind),
            Action::References => self.references(),
            Action::Hover => self.hover(),
            action if action.is_horizontal() => view.caret.apply(&view.lines[..], action),
            _ => {}
        }
        if let Some(view) = self.file_view.as_mut() {
            view.keep_in_view(height);
        }
    }

    /// `n` / `N` in the file view. Returns `false` when there's no file view.
    pub(super) fn search_file_view(&mut self, query: &str, n: usize, forward: bool) -> bool {
        let height = self.viewport_height;
        let Some(view) = self.file_view.as_mut() else { return false };
        match view.search(query, n, forward) {
            None => self.message = Some(format!("Pattern not found: {query}")),
            Some(true) => {
                self.message = Some(
                    if forward { "search hit BOTTOM, continuing at TOP" } else { "search hit TOP, continuing at BOTTOM" }
                        .to_string(),
                )
            }
            Some(false) => {}
        }
        if let Some(view) = self.file_view.as_mut() {
            view.keep_in_view(height);
        }
        true
    }

    /// The cursor's character in the file view, as bytes of its line.
    pub fn file_view_cursor(&self) -> Option<std::ops::Range<usize>> {
        let view = self.file_view.as_ref()?;
        view.caret.bytes(&view.lines[..])
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::test_support::*;
    use super::*;
    use crate::diff::{Diff, DiffLine, FileStatus, Hunk, Sources};
    use crate::review::ReviewState;

    fn line(kind: LineKind, old_no: Option<u32>, new_no: Option<u32>, content: &str) -> DiffLine {
        DiffLine { kind, old_no, new_no, content: content.into() }
    }

    /// `a.rs`: line 2 modified, line 4 removed, line 6 added.
    fn sample() -> (FileDiff, Sources) {
        use LineKind::*;
        let old = ["fn a() {", "    old();", "    keep();", "    gone();", "    end();", "}"];
        let new = ["fn a() {", "    new();", "    keep();", "    end();", "    added();", "}"];
        let hunk = Hunk {
            id: Default::default(),
            header: "@@ -1,6 +1,6 @@".into(),
            old_start: 1,
            old_lines: 6,
            new_start: 1,
            new_lines: 6,
            lines: vec![
                line(Context, Some(1), Some(1), old[0]),
                line(Removed, Some(2), None, old[1]),
                line(Added, None, Some(2), new[1]),
                line(Context, Some(3), Some(3), old[2]),
                line(Removed, Some(4), None, old[3]),
                line(Context, Some(5), Some(4), old[4]),
                line(Added, None, Some(5), new[4]),
                line(Context, Some(6), Some(6), old[5]),
            ],
        };
        let file = FileDiff {
            old_path: Some("a.rs".into()),
            new_path: Some("a.rs".into()),
            status: FileStatus::Modified,
            is_binary: false,
            hunks: vec![hunk],
        };
        let lines = |l: &[&str]| Arc::new(l.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        (file, Sources { old: Some(lines(&old)), new: Some(lines(&new)) })
    }

    fn sample_app() -> App {
        let (file, sources) = sample();
        let mut diff = Diff::new(vec![file]);
        diff.sources = vec![sources];
        let mut app = App::new(String::new(), diff, ReviewState::default());
        app.set_viewport(80, 20);
        app
    }

    #[test]
    fn marks_each_side() {
        use Mark::*;
        let (file, _) = sample();
        assert_eq!(marks(&file, Side::New, 6), [None, Some(Modified), None, Some(Gap), Some(Changed), None]);
        assert_eq!(marks(&file, Side::Old, 6), [None, Some(Modified), None, Some(Changed), None, Some(Gap)]);
    }

    #[test]
    fn enter_opens_the_file_at_the_cursor() {
        let mut app = sample_app();
        // Rows: header, hunk, then the hunk's lines; row 4 is `+ new();`.
        app.cursor = 4;
        app.update(Action::WordForward { n: 1, big: false });
        assert_eq!(app.cursor_column(), Some(4));
        app.update(Action::Select);
        let view = app.file_view.as_ref().unwrap();
        assert_eq!((view.side, view.line_no(), view.caret.column), (Side::New, 2, 4));
        assert_eq!(app.file_view_cursor(), Some(4..5));
        // Back where it was.
        app.update(Action::Quit);
        assert!(app.file_view.is_none());
        assert!(!app.should_quit);
        assert_eq!(app.cursor, 4);
    }

    #[test]
    fn a_removed_line_opens_the_old_file() {
        let mut app = sample_app();
        app.cursor = 6; // `- gone();`
        app.update(Action::Select);
        let view = app.file_view.as_ref().unwrap();
        assert_eq!((view.side, view.line_no()), (Side::Old, 4));
        app.update(Action::Cancel);
        assert!(app.file_view.is_none());
    }

    #[test]
    fn moves_and_jumps_between_changes() {
        let mut app = sample_app();
        app.cursor = 2;
        app.update(Action::Select);
        app.update(Action::NextHunk(1));
        assert_eq!(app.file_view.as_ref().unwrap().line_no(), 2);
        // Line 4 is where `gone();` was; 5, added right after it, is the same change.
        app.update(Action::NextHunk(2));
        assert_eq!(app.file_view.as_ref().unwrap().line_no(), 4);
        app.update(Action::NextHunk(1));
        assert_eq!(app.message.as_deref(), Some("No more changes below"));
        app.update(Action::PrevHunk(1));
        assert_eq!(app.file_view.as_ref().unwrap().line_no(), 2);
        app.update(Action::Bottom);
        app.update(Action::LineEnd);
        app.update(Action::Up(1));
        let view = app.file_view.as_ref().unwrap();
        assert_eq!((view.line_no(), view.caret.on(&view.lines[..])), (5, Some(11)));
        app.update(Action::Top);
        app.update(Action::Down(100));
        assert_eq!(app.file_view.as_ref().unwrap().line_no(), 6);
    }

    #[test]
    fn searches_the_file() {
        let mut app = sample_app();
        app.cursor = 2;
        app.update(Action::Select);
        app.update(Action::StartSearch);
        type_text(&mut app, "end");
        app.update_input(crate::input::InputAction::Submit);
        let view = app.file_view.as_ref().unwrap();
        assert_eq!((view.line_no(), view.caret.column), (4, 4));
        app.update(Action::SearchNext(1));
        assert_eq!(app.message.as_deref(), Some("search hit BOTTOM, continuing at TOP"));
        // Esc clears the highlight first, then closes.
        app.update(Action::Cancel);
        assert!(app.file_view.is_some());
        app.update(Action::Cancel);
        assert!(app.file_view.is_none());
    }

    #[test]
    fn keeps_the_cursor_on_screen() {
        let mut view = FileView::new(
            "a".into(),
            Side::New,
            Arc::new((0..100).map(|i| i.to_string()).collect()),
            None,
            vec![None; 100],
        );
        view.caret.line = 50;
        view.keep_in_view(10);
        assert_eq!(view.scroll, 41);
        view.caret.line = 3;
        view.keep_in_view(10);
        assert_eq!(view.scroll, 3);
    }

    #[test]
    fn lines_without_text_stay_in_the_diff() {
        let mut app = app_with(vec![file("a.rs", &[2])]);
        app.cursor = 2;
        app.update(Action::Select);
        assert!(app.file_view.is_none());
        assert_eq!(app.message.as_deref(), Some("No text to show for a.rs"));
    }
}
