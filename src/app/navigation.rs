//! Cursor motions and jumps. Line and hunk motions stay inside the current
//! file; file, unreviewed and comment jumps switch files as needed.

use super::{Action, App, Row, View};

/// In the sidebar, vertical motions move between files instead of rows. In
/// the pop-up sidebar, `q` closes it rather than quitting.
pub(super) fn sidebar_action(action: Action, popup: bool) -> Action {
    match action {
        Action::Quit if popup => Action::FocusDiff,
        Action::Down(n) => Action::NextFile(n),
        Action::Up(n) => Action::PrevFile(n),
        Action::Top => Action::PrevFile(usize::MAX),
        Action::Bottom => Action::NextFile(usize::MAX),
        Action::Select => Action::FocusDiff,
        other => other,
    }
}

impl App {
    pub(super) fn move_by(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
    }

    pub(super) fn next_hunk(&mut self, n: usize) {
        let rows = self.rows.iter().enumerate().skip(self.cursor + 1);
        let target = rows.filter(|(_, row)| row.is_hunk()).take(n).last();
        self.jump_in_file(target.map(|(idx, _)| idx), "Last hunk in this file");
    }

    /// From inside a hunk, the first step back lands on that hunk's header.
    pub(super) fn prev_hunk(&mut self, n: usize) {
        let rows = self.rows[..self.cursor].iter().enumerate().rev();
        let target = rows.filter(|(_, row)| row.is_hunk()).take(n).last();
        self.jump_in_file(target.map(|(idx, _)| idx), "First hunk in this file");
    }

    /// `]f`: the next file, plan group, or stack page. At the last one, stays put
    /// rather than jumping back to the top.
    pub(super) fn next_page(&mut self, n: usize) {
        let last = self.page_count().saturating_sub(1);
        if self.page >= last {
            self.message = Some(format!("Last {}", self.page_noun()));
        } else {
            self.open_page(self.page.saturating_add(n).min(last), 0);
        }
    }

    pub(super) fn prev_page(&mut self, n: usize) {
        if self.page == 0 {
            self.message = Some(format!("First {}", self.page_noun()));
        } else {
            self.open_page(self.page.saturating_sub(n), 0);
        }
    }

    fn page_noun(&self) -> &'static str {
        match self.view {
            View::Files => "file",
            View::Plan | View::Stack => "group",
        }
    }

    pub(super) fn jump_unreviewed(&mut self, n: usize, forward: bool) {
        let stops = self.stops(
            |row| matches!(*row, Row::Hunk { file, hunk } if !self.is_reviewed(file, hunk)),
            |file| !self.file_progress(file).is_complete(),
        );
        if !self.jump_to_stop(&stops, n, forward) {
            self.message = Some(if self.progress().is_complete() {
                "Everything reviewed ✓".to_string()
            } else {
                "No more unreviewed hunks in this direction".to_string()
            });
        }
    }

    pub(super) fn jump_comment(&mut self, n: usize, forward: bool) {
        // Agent notes are stops too: they're comments waiting to be written.
        let stops = self.stops(
            |row| matches!(row, Row::Comment { line: 0, .. } | Row::Note { .. }),
            |file| self.has_comments_in_hunks(file) || self.notes.iter().any(|n| n.file == file),
        );
        if !self.jump_to_stop(&stops, n, forward) {
            self.message = Some("No more comments or notes in this direction".to_string());
        }
    }

    /// `:<n>`: the first row at or after line `n` of the new file.
    pub(super) fn go_to_line(&mut self, n: u32) {
        let target = self.rows.iter().position(|row| match *row {
            Row::Line { file, hunk, line } => self.line(file, hunk, line).new_no.is_some_and(|no| no >= n),
            _ => false,
        });
        match target {
            Some(idx) => self.cursor = idx,
            None => self.message = Some(format!("Line {n} is past the last change in this file")),
        }
    }

    /// Moves to `target` within the current file and puts it at the top of
    /// the viewport, or explains why there's nowhere to go.
    fn jump_in_file(&mut self, target: Option<usize>, at_edge: &str) {
        match target {
            Some(idx) => {
                self.cursor = idx;
                self.scroll = idx;
            }
            None => self.message = Some(format!("{at_edge} (]f / [f for other files)")),
        }
    }

    /// Every `(page, row)` a cross-page jump can land on, in page order:
    /// rows matching `row_stop`, and headers of folded files where
    /// `folded_stop` says something is hidden.
    pub(super) fn stops(&self, row_stop: impl Fn(&Row) -> bool, folded_stop: impl Fn(usize) -> bool) -> Vec<(usize, usize)> {
        let mut stops = Vec::new();
        for page in 0..self.page_count() {
            for (idx, row) in self.page_rows(page).into_iter().enumerate() {
                let hides_one = matches!(row, Row::File { file } if self.collapsed[file] && folded_stop(file));
                if hides_one || row_stop(&row) {
                    stops.push((page, idx));
                }
            }
        }
        stops
    }

    /// Jumps `n` stops forward or back, switching pages if needed. Returns
    /// false when there's no stop in that direction.
    pub(super) fn jump_to_stop(&mut self, stops: &[(usize, usize)], n: usize, forward: bool) -> bool {
        let here = (self.page, self.cursor);
        let target = if forward {
            stops.iter().filter(|&&s| s > here).take(n).last()
        } else {
            stops.iter().rev().filter(|&&s| s < here).take(n).last()
        };
        let Some(&(page, row)) = target else { return false };
        self.open_page(page, row);
        self.scroll = row;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::Focus;

    #[test]
    fn line_motions_stay_in_the_file() {
        let mut app = sample_app();
        app.update(Action::Up(5));
        assert_eq!(position(&app), (0, 0));
        app.update(Action::Down(100));
        assert_eq!(position(&app), (0, 7));
        app.update(Action::Bottom);
        assert_eq!(position(&app), (0, 7));
        app.update(Action::Top);
        assert_eq!(position(&app), (0, 0));
    }

    #[test]
    fn hunk_jumps_stay_in_the_file() {
        let mut app = sample_app();
        app.set_viewport(80, 4);
        app.update(Action::NextHunk(1));
        assert_eq!((app.cursor, app.scroll), (1, 1));
        app.update(Action::NextHunk(5));
        assert_eq!(position(&app), (0, 5));
        app.update(Action::NextHunk(1));
        assert_eq!(position(&app), (0, 5));
        assert!(app.message.as_deref().unwrap().starts_with("Last hunk"));

        app.update(Action::Down(1));
        app.update(Action::PrevHunk(1));
        assert_eq!(position(&app), (0, 5));
        app.update(Action::PrevHunk(10));
        assert_eq!(position(&app), (0, 1));
    }

    #[test]
    fn file_jumps_switch_the_view() {
        let mut app = sample_app();
        app.update(Action::Down(3));
        app.update(Action::NextFile(1));
        assert_eq!(position(&app), (1, 0));
        assert_eq!(app.rows.len(), 6);
        app.update(Action::Down(3));
        app.update(Action::NextFile(10));
        assert_eq!(position(&app), (1, 3));
        assert_eq!(app.message.as_deref(), Some("Last file"));
        app.update(Action::PrevFile(1));
        assert_eq!(position(&app), (0, 0));
        app.update(Action::PrevFile(1));
        assert_eq!(app.message.as_deref(), Some("First file"));
    }

    #[test]
    fn sidebar_motions_move_between_files() {
        let mut app = sample_app();
        app.update(Action::ToggleFocus);
        assert_eq!(app.focus, Focus::Sidebar);
        app.update(Action::Down(1));
        assert_eq!(app.page, 1);
        app.update(Action::Top);
        assert_eq!(app.page, 0);
        app.update(Action::Bottom);
        assert_eq!(app.page, 1);
        app.update(Action::Select);
        assert_eq!(app.focus, Focus::Diff);
    }

    #[test]
    fn unreviewed_jumps_cross_files() {
        let mut app = sample_app();
        app.update(Action::NextUnreviewed(1));
        assert_eq!(position(&app), (0, 1));
        app.update(Action::ToggleReviewed);
        app.update(Action::Top);
        app.update(Action::NextUnreviewed(1));
        assert_eq!(position(&app), (0, 5));
        app.update(Action::NextUnreviewed(1));
        assert_eq!(position(&app), (1, 1));
        assert!(app.rows.iter().all(|r| r.file() == Some(1)));
        app.update(Action::PrevUnreviewed(5));
        assert_eq!(position(&app), (0, 5));
    }

    #[test]
    fn unreviewed_jumps_stop_at_folded_files_with_work_left() {
        let mut app = app_with(vec![file("a.rs", &[1]), file("Cargo.lock", &[2])]);
        app.update(Action::NextUnreviewed(1));
        app.update(Action::ToggleReviewed);
        app.update(Action::NextUnreviewed(1));
        assert_eq!(position(&app), (1, 0));
        assert_eq!(app.current_row(), Some(Row::File { file: 1 }));

        app.update(Action::TogglePageReviewed);
        app.update(Action::NextUnreviewed(1));
        assert_eq!(app.message.as_deref(), Some("Everything reviewed ✓"));
    }

    #[test]
    fn comment_jumps_cross_files() {
        let mut app = sample_app();
        app.update(Action::NextFile(1));
        app.update(Action::Down(2));
        comment(&mut app, "in b");
        app.update(Action::PrevFile(1));
        app.update(Action::Down(6));
        comment(&mut app, "in a");

        app.update(Action::Top);
        app.update(Action::NextComment(1));
        assert_eq!(position(&app), (0, 7));
        app.update(Action::NextComment(1));
        assert_eq!(position(&app), (1, 3));
        app.update(Action::NextComment(1));
        assert_eq!(app.message.as_deref(), Some("No more comments or notes in this direction"));
        app.update(Action::PrevComment(1));
        assert_eq!(position(&app), (0, 7));
    }
}
