//! Marking hunks as reviewed, and how far along the review is.

use super::{App, Row, View};

/// Reviewed and total hunk counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub reviewed: usize,
    pub total: usize,
}

impl Progress {
    pub fn is_complete(self) -> bool {
        self.total > 0 && self.reviewed == self.total
    }
}

impl App {
    pub fn is_reviewed(&self, file: usize, hunk: usize) -> bool {
        self.review.is_reviewed(&self.hunk(file, hunk).id)
    }

    pub fn file_progress(&self, file: usize) -> Progress {
        let hunks: Vec<_> = (0..self.diff.files[file].hunks.len()).map(|h| (file, h)).collect();
        self.hunks_progress(&hunks)
    }

    pub fn group_progress(&self, group: usize) -> Progress {
        self.hunks_progress(&self.plan[group].hunks)
    }

    pub fn progress(&self) -> Progress {
        (0..self.diff.files.len()).map(|f| self.file_progress(f)).fold(
            Progress { reviewed: 0, total: 0 },
            |acc, p| Progress { reviewed: acc.reviewed + p.reviewed, total: acc.total + p.total },
        )
    }

    /// `r`: the hunk under the cursor. On a header, everything under it on
    /// this page: the whole group, or the file's hunks.
    pub(super) fn toggle_reviewed(&mut self) {
        match self.current_row() {
            Some(Row::Hunk { file, hunk } | Row::Line { file, hunk, .. } | Row::Context { file, hunk, .. }) => {
                let reviewed = self.is_reviewed(file, hunk);
                self.review.set_reviewed(&self.hunk(file, hunk).id.clone(), !reviewed);
            }
            Some(Row::Group { .. }) => self.toggle_page_reviewed(),
            Some(Row::File { file }) => {
                let hunks: Vec<_> = self.page_hunks(self.page).into_iter().filter(|&(f, _)| f == file).collect();
                self.toggle_hunks_reviewed(&hunks);
            }
            Some(Row::Comment { .. }) | None => {}
        }
    }

    /// `R`: everything on the current page.
    pub(super) fn toggle_page_reviewed(&mut self) {
        if self.page < self.page_count() {
            let hunks = self.page_hunks(self.page);
            self.toggle_hunks_reviewed(&hunks);
        }
    }

    fn hunks_progress(&self, hunks: &[(usize, usize)]) -> Progress {
        Progress {
            reviewed: hunks.iter().filter(|&&(f, h)| self.is_reviewed(f, h)).count(),
            total: hunks.len(),
        }
    }

    /// Marks the rest of a partly reviewed set; only a fully reviewed set
    /// gets unmarked. In the files view the file also folds when done, and
    /// unfolds when unmarked.
    fn toggle_hunks_reviewed(&mut self, hunks: &[(usize, usize)]) {
        let reviewed = !self.hunks_progress(hunks).is_complete();
        for &(file, hunk) in hunks {
            self.review.set_reviewed(&self.hunk(file, hunk).id.clone(), reviewed);
        }
        if self.view == View::Files {
            self.collapsed[self.page] = reviewed;
        }
        self.rebuild_rows();
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::Action;
    use crate::diff::Diff;

    #[test]
    fn r_toggles_the_hunk_under_the_cursor() {
        let mut app = sample_app();
        app.update(Action::Down(3));
        app.update(Action::ToggleReviewed);
        assert!(app.is_reviewed(0, 0));
        assert!(!app.is_reviewed(0, 1));
        assert_eq!(app.progress(), Progress { reviewed: 1, total: 3 });
        assert!(app.review.take_dirty());

        app.update(Action::ToggleReviewed);
        assert!(!app.is_reviewed(0, 0));
    }

    #[test]
    fn file_review_marks_everything_then_folds() {
        let mut app = sample_app();
        app.update(Action::Down(3));
        app.update(Action::ToggleReviewed);
        app.update(Action::TogglePageReviewed);
        assert!(app.file_progress(0).is_complete());
        assert!(app.collapsed[0]);
        assert_eq!(app.cursor, 0);

        // `r` on a file header acts on the whole file.
        app.update(Action::ToggleReviewed);
        assert_eq!(app.file_progress(0).reviewed, 0);
        assert!(!app.collapsed[0]);
    }

    #[test]
    fn reviewed_files_start_folded() {
        let files = || vec![file("a.rs", &[1]), file("b.rs", &[1])];
        let mut app = app_with(files());
        app.update(Action::TogglePageReviewed);
        let app = App::new(String::new(), Diff::new(files()), app.review);
        assert_eq!(app.collapsed, [true, false]);
    }
}
