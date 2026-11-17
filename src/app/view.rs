//! What the diff pane shows: the current file's rows, folds, focus and scrolling.

use super::{App, Focus, Mode};
use crate::review::Comment;

/// One visible row of the current file, pointing back into the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    File { file: usize },
    Hunk { file: usize, hunk: usize },
    Line { file: usize, hunk: usize, line: usize },
    /// One line of a comment's text, shown under the code it's about.
    Comment { file: usize, id: u64, line: usize },
}

impl Row {
    pub fn file(self) -> usize {
        match self {
            Row::File { file }
            | Row::Hunk { file, .. }
            | Row::Line { file, .. }
            | Row::Comment { file, .. } => file,
        }
    }

    pub fn hunk(self) -> Option<usize> {
        match self {
            Row::File { .. } | Row::Comment { .. } => None,
            Row::Hunk { hunk, .. } | Row::Line { hunk, .. } => Some(hunk),
        }
    }

    pub(super) fn is_hunk(&self) -> bool {
        matches!(self, Row::Hunk { .. })
    }
}

impl App {
    pub fn set_viewport_height(&mut self, height: usize) {
        self.viewport_height = height;
        self.scroll_into_view();
    }

    /// Rows of `file` as shown: hunks unless folded, comments under the
    /// lines they end on, and comments whose hunk is gone right under the header.
    pub(super) fn file_rows(&self, file: usize) -> Vec<Row> {
        let comment_rows = |c: &Comment| {
            let (id, count) = (c.id, c.text.lines().count().max(1));
            (0..count).map(move |line| Row::Comment { file, id, line })
        };
        let f = &self.diff.files[file];
        let comments = self.review.comments();
        let mut rows = vec![Row::File { file }];
        let outdated = comments.iter().filter(|c| c.path == f.path() && self.is_outdated(c));
        rows.extend(outdated.flat_map(comment_rows));
        if self.collapsed[file] {
            return rows;
        }
        for (hunk, h) in f.hunks.iter().enumerate() {
            rows.push(Row::Hunk { file, hunk });
            let last = h.lines.len().saturating_sub(1);
            for line in 0..h.lines.len() {
                rows.push(Row::Line { file, hunk, line });
                let here = comments.iter().filter(|c| c.hunk == h.id && c.end.min(last) == line);
                rows.extend(here.flat_map(comment_rows));
            }
        }
        rows
    }

    pub(super) fn open_file(&mut self, file: usize, cursor: usize) {
        if file >= self.diff.files.len() {
            return;
        }
        if file != self.file || self.rows.is_empty() {
            self.file = file;
            self.rows = self.file_rows(file);
            self.scroll = 0;
            if let Mode::Visual { .. } = self.mode {
                self.mode = Mode::Normal;
            }
        }
        self.cursor = cursor.min(self.rows.len() - 1);
    }

    /// Re-flattens the current file after a change, keeping the cursor on the
    /// same row, or on the file header if that row is gone.
    pub(super) fn rebuild_rows(&mut self) {
        let Some(file) = self.current_file() else { return };
        let current = self.current_row();
        self.rows = self.file_rows(file);
        let kept = current.and_then(|row| self.rows.iter().position(|r| *r == row));
        // A deleted comment hands the cursor to the row after it; a fold, to the header.
        let fallback = if self.collapsed[file] { 0 } else { self.cursor.min(self.rows.len() - 1) };
        self.cursor = kept.unwrap_or(fallback);
    }

    pub(super) fn toggle_fold(&mut self) {
        if let Some(file) = self.current_file() {
            self.collapsed[file] = !self.collapsed[file];
            self.rebuild_rows();
        }
    }

    pub(super) fn set_fold(&mut self, collapsed: bool) {
        if let Some(file) = self.current_file() {
            self.collapsed[file] = collapsed;
            self.rebuild_rows();
        }
    }

    pub(super) fn set_all_folds(&mut self, collapsed: bool) {
        self.collapsed.fill(collapsed);
        self.rebuild_rows();
    }

    pub(super) fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Diff if self.show_sidebar => Focus::Sidebar,
            _ => Focus::Diff,
        }
    }

    pub(super) fn focus_sidebar(&mut self) {
        if self.show_sidebar {
            self.focus = Focus::Sidebar;
        }
    }

    pub(super) fn toggle_sidebar(&mut self) {
        self.show_sidebar = !self.show_sidebar;
        self.focus = Focus::Diff;
    }

    pub(super) fn half_page(&self) -> isize {
        (self.viewport_height / 2).max(1) as isize
    }

    pub(super) fn scroll_into_view(&mut self) {
        if self.viewport_height == 0 {
            return;
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + self.viewport_height {
            self.scroll = self.cursor + 1 - self.viewport_height;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::Action;

    #[test]
    fn shows_only_the_current_file() {
        let app = sample_app();
        assert_eq!(app.rows.len(), 8);
        assert!(app.rows.iter().all(|r| r.file() == 0));
        assert_eq!(app.rows[2], Row::Line { file: 0, hunk: 0, line: 0 });
    }

    #[test]
    fn scroll_follows_cursor() {
        let mut app = sample_app();
        app.set_viewport_height(4);
        app.update(Action::Down(6));
        assert_eq!(app.scroll, 3);
        app.update(Action::HalfPageUp);
        assert_eq!((app.cursor, app.scroll), (4, 3));
        app.update(Action::Top);
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn hiding_the_sidebar_returns_focus_to_the_diff() {
        let mut app = sample_app();
        app.update(Action::FocusSidebar);
        app.update(Action::ToggleSidebar);
        assert_eq!(app.focus, Focus::Diff);
        app.update(Action::ToggleFocus);
        assert_eq!(app.focus, Focus::Diff);
    }

    #[test]
    fn folding_keeps_the_cursor_on_its_file() {
        let mut app = sample_app();
        app.update(Action::Down(3));
        app.update(Action::CloseFold);
        assert_eq!(app.rows, [Row::File { file: 0 }]);
        assert_eq!(app.cursor, 0);
        app.update(Action::OpenFold);
        assert_eq!(app.rows.len(), 8);

        app.update(Action::CloseAllFolds);
        app.update(Action::NextFile(1));
        assert_eq!(app.rows, [Row::File { file: 1 }]);
        app.update(Action::OpenAllFolds);
        assert_eq!(app.rows.len(), 6);
    }

    #[test]
    fn generated_files_start_folded() {
        let app = app_with(vec![file("Cargo.lock", &[3]), file("src/main.rs", &[1])]);
        assert_eq!(app.collapsed, [true, false]);
        assert_eq!(app.rows, [Row::File { file: 0 }]);
    }
}
