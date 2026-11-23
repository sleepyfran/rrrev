//! Marking hunks as reviewed, and how far along the review is.

use std::collections::BTreeSet;

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

    /// Whether a hunk needs nothing more from you: marked reviewed, or
    /// plumbing, which counts as done without a mark.
    pub fn is_done(&self, file: usize, hunk: usize) -> bool {
        self.is_reviewed(file, hunk) || self.diff.is_plumbing(file, hunk)
    }

    pub fn file_progress(&self, file: usize) -> Progress {
        let hunks: Vec<_> = (0..self.diff.files[file].hunks.len())
            .map(|h| (file, h))
            .collect();
        self.hunks_progress(&hunks)
    }

    pub fn group_progress(&self, group: usize) -> Progress {
        self.hunks_progress(&self.pages()[group].hunks)
    }

    pub fn layer_progress(&self, layer: usize) -> Progress {
        let files = self.diff.sections[layer].files.clone();
        let hunks: Vec<_> = files
            .flat_map(|f| (0..self.diff.files[f].hunks.len()).map(move |h| (f, h)))
            .collect();
        self.hunks_progress(&hunks)
    }

    pub fn progress(&self) -> Progress {
        (0..self.diff.files.len())
            .map(|f| self.file_progress(f))
            .fold(
                Progress {
                    reviewed: 0,
                    total: 0,
                },
                |acc, p| Progress {
                    reviewed: acc.reviewed + p.reviewed,
                    total: acc.total + p.total,
                },
            )
    }

    /// `r`: the hunk under the cursor. On a header, everything under it on
    /// this page: the whole group, or the file's hunks.
    pub(super) fn toggle_reviewed(&mut self) {
        match self.current_row() {
            Some(
                Row::Hunk { file, hunk }
                | Row::Line { file, hunk, .. }
                | Row::Context { file, hunk, .. }
                | Row::Note { file, hunk, .. }
                | Row::Plumbing { file, hunk, .. },
            ) => {
                let reviewed = self.is_reviewed(file, hunk);
                self.set_hunks_reviewed(&[(file, hunk)], !reviewed);
            }
            Some(Row::Group { .. }) => self.toggle_page_reviewed(),
            Some(Row::File { file }) => self.toggle_file_on_page_reviewed(file),
            Some(Row::Comment { .. }) | None => {}
        }
    }

    /// `R`: everything on the current page. In the stack a page is a whole
    /// layer, so there it's just the file under the cursor, or the page when
    /// the cursor isn't on a file.
    pub(super) fn toggle_page_or_file_reviewed(&mut self) {
        match (self.view, self.current_file()) {
            (View::Stack, Some(file)) => self.toggle_file_on_page_reviewed(file),
            _ => self.toggle_page_reviewed(),
        }
    }

    /// Everything on the current page.
    fn toggle_page_reviewed(&mut self) {
        if self.page < self.page_count() {
            let hunks = self.page_hunks(self.page);
            self.toggle_hunks_reviewed(&hunks);
        }
    }

    /// `file`'s hunks on the current page.
    fn toggle_file_on_page_reviewed(&mut self, file: usize) {
        if self.page < self.page_count() {
            let hunks: Vec<_> = self
                .page_hunks(self.page)
                .into_iter()
                .filter(|&(f, _)| f == file)
                .collect();
            self.toggle_hunks_reviewed(&hunks);
        }
    }

    fn hunks_progress(&self, hunks: &[(usize, usize)]) -> Progress {
        Progress {
            reviewed: hunks.iter().filter(|&&(f, h)| self.is_done(f, h)).count(),
            total: hunks.len(),
        }
    }

    /// Marks the rest of a partly reviewed set; only a fully reviewed set
    /// gets unmarked. Goes by the marks alone, so plumbing that counts as
    /// done still gets marked with the rest.
    fn toggle_hunks_reviewed(&mut self, hunks: &[(usize, usize)]) {
        let reviewed = !hunks.iter().all(|&(f, h)| self.is_reviewed(f, h));
        self.set_hunks_reviewed(hunks, reviewed);
    }

    /// Marks or unmarks `hunks`. A file this finishes folds out of the way,
    /// and one it un-finishes unfolds again.
    fn set_hunks_reviewed(&mut self, hunks: &[(usize, usize)], reviewed: bool) {
        let files: BTreeSet<usize> = hunks.iter().map(|&(f, _)| f).collect();
        let was_complete: Vec<bool> = files
            .iter()
            .map(|&f| self.file_progress(f).is_complete())
            .collect();
        for &(file, hunk) in hunks {
            self.review
                .set_reviewed(&self.hunk(file, hunk).id.clone(), reviewed);
        }
        for (&file, was) in files.iter().zip(was_complete) {
            let complete = self.file_progress(file).is_complete();
            if complete != was {
                self.collapsed[file] = complete;
            }
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
    fn plumbing_counts_as_done_without_a_mark() {
        let mut app = plumbing_app();
        // a.rs: the import hunk is done, the function isn't; b.rs is all done.
        assert_eq!(
            app.file_progress(0),
            Progress {
                reviewed: 1,
                total: 2
            }
        );
        assert!(app.file_progress(1).is_complete());
        assert!(!app.is_reviewed(0, 0));
        // ]u goes past the plumbing to the function.
        app.update(Action::NextUnreviewed(1));
        assert_eq!(app.current_row(), Some(Row::Hunk { file: 0, hunk: 1 }));
        // r on the file header marks both: the marks decide, not the count.
        app.update(Action::Up(2));
        app.update(Action::ToggleReviewed);
        assert!(app.is_reviewed(0, 0) && app.is_reviewed(0, 1));
        let stats = app.line_stats();
        assert_eq!((stats.good, stats.plumbing, stats.unseen), (4, 2, 0));
    }

    #[test]
    fn r_toggles_the_hunk_under_the_cursor() {
        let mut app = sample_app();
        app.update(Action::Down(3));
        app.update(Action::ToggleReviewed);
        assert!(app.is_reviewed(0, 0));
        assert!(!app.is_reviewed(0, 1));
        assert_eq!(
            app.progress(),
            Progress {
                reviewed: 1,
                total: 3
            }
        );
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
    fn reviewing_the_last_hunk_folds_the_file() {
        let mut app = sample_app();
        app.update(Action::Down(1));
        app.update(Action::ToggleReviewed);
        assert!(!app.collapsed[0]);

        // The second hunk's first line: the last one left in a.rs.
        app.update(Action::Down(5));
        assert_eq!(
            app.current_row(),
            Some(Row::Line {
                file: 0,
                hunk: 1,
                line: 0
            })
        );
        app.update(Action::ToggleReviewed);
        assert!(app.collapsed[0]);
        assert_eq!(app.rows, [Row::File { file: 0 }]);
        assert_eq!(app.cursor, 0);
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
