//! Marking hunks as reviewed, and how far along the review is.

use super::{App, Row};

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
        let hunks = &self.diff.files[file].hunks;
        Progress {
            reviewed: hunks.iter().filter(|h| self.review.is_reviewed(&h.id)).count(),
            total: hunks.len(),
        }
    }

    pub fn progress(&self) -> Progress {
        (0..self.diff.files.len()).map(|f| self.file_progress(f)).fold(
            Progress { reviewed: 0, total: 0 },
            |acc, p| Progress { reviewed: acc.reviewed + p.reviewed, total: acc.total + p.total },
        )
    }

    /// `r`: the hunk under the cursor, or the whole file on its header.
    pub(super) fn toggle_reviewed(&mut self) {
        match self.current_row() {
            Some(Row::Hunk { file, hunk } | Row::Line { file, hunk, .. }) => {
                let reviewed = self.is_reviewed(file, hunk);
                self.review.set_reviewed(&self.diff.files[file].hunks[hunk].id, !reviewed);
            }
            Some(Row::File { .. }) => self.toggle_file_reviewed(),
            Some(Row::Comment { .. }) | None => {}
        }
    }

    /// `R`: marks the rest of a partly reviewed file and folds it; only a
    /// fully reviewed file gets unmarked (and unfolded).
    pub(super) fn toggle_file_reviewed(&mut self) {
        let Some(file) = self.current_file() else { return };
        let reviewed = !self.file_progress(file).is_complete();
        for hunk in &self.diff.files[file].hunks {
            self.review.set_reviewed(&hunk.id, reviewed);
        }
        self.collapsed[file] = reviewed;
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
        app.update(Action::ToggleFileReviewed);
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
        app.update(Action::ToggleFileReviewed);
        let app = App::new(String::new(), Diff::new(files()), app.review);
        assert_eq!(app.collapsed, [true, false]);
    }
}
