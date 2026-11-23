//! Picking the diff up again after the files changed under it, as they do
//! when Neovim edits one from the file view.

use std::collections::HashMap;

use super::file_view::{FileView, marks};
use super::{App, Row, View};
use crate::diff::{Diff, Side, Source};

impl App {
    /// The diff as it is now, and the text of the file the file view is on
    /// (`None` if it's gone). Everything that hangs off the old diff's
    /// files and hunks is rebuilt from the new one, as a fresh start would;
    /// the review itself is kept by hunk id. The file view and the diff's
    /// cursor stay where they were, as near as the new text allows.
    pub fn reload(&mut self, diff: Diff, file: Option<Source>) {
        if !same_hunks(&self.diff, &diff) {
            self.replace_diff(diff);
        }
        let Some(view) = self.file_view.take() else {
            return;
        };
        let Some(lines) = file else {
            self.message = Some(format!("{} is gone", view.path));
            return;
        };
        let file = self
            .diff
            .files
            .iter()
            .position(|f| f.new_path.as_deref() == Some(view.path.as_str()));
        let marks = match file {
            Some(file) => marks(&self.diff.files[file], Side::New, lines.len()),
            None => vec![None; lines.len()],
        };
        let mut fresh = FileView::new(view.path, Side::New, lines, file, marks);
        fresh.caret = view.caret;
        fresh.caret.line = fresh.caret.line.min(fresh.lines.len().saturating_sub(1));
        fresh.scroll = view.scroll.min(fresh.caret.line);
        fresh.keep_in_view(self.viewport_height);
        self.file_view = Some(fresh);
    }

    fn replace_diff(&mut self, diff: Diff) {
        let review = std::mem::take(&mut self.review);
        let mut fresh = App::new(self.title.clone(), diff, review);

        // How the session looks and what's running in it.
        fresh.set_theme(self.theme);
        fresh.set_appearance(self.appearance);
        fresh.grammars = std::mem::take(&mut self.grammars);
        fresh.ask = std::mem::take(&mut self.ask);
        fresh.lsp_status = self.lsp_status.take();
        fresh.show_sidebar = self.show_sidebar;
        fresh.compact = self.compact;
        fresh.focus = self.focus;
        fresh.show_removals = self.show_removals;
        fresh.search = self.search.take();
        fresh.highlight_search = self.highlight_search;
        fresh.next_job = self.next_job;
        fresh.organizing = self.organizing;
        fresh.organizing_layer = self.organizing_layer;
        fresh.walking = self.walking;
        fresh.walking_files = self.walking_files.clone();
        fresh.walkthrough_slide = self.walkthrough_slide;

        // Folded files stay folded, by path.
        let folded: HashMap<&str, bool> = self
            .diff
            .files
            .iter()
            .zip(&self.collapsed)
            .map(|(f, &c)| (f.path(), c))
            .collect();
        for (i, f) in fresh.diff.files.iter().enumerate() {
            if let Some(&c) = folded.get(f.path()) {
                fresh.collapsed[i] = c;
            }
        }

        let view_works = match self.view {
            View::Files => true,
            View::Plan => !fresh.is_stack() && !fresh.plan.is_empty(),
            View::Stack => !fresh.stack.is_empty(),
        };
        if view_works && fresh.view != self.view {
            fresh.set_view(self.view);
        }
        fresh.set_viewport(self.viewport_width, self.viewport_height);

        // The diff's cursor back on its file and line, if the diff still
        // shows them.
        let here = self.current_row().and_then(|row| {
            let file = &self.diff.files[row.file()?];
            let no = match row {
                Row::Line { file, hunk, line } => {
                    let l = self.line(file, hunk, line);
                    l.new_no.or(l.old_no)
                }
                Row::Context { no, .. } => Some(no),
                _ => None,
            };
            Some((file.path().to_string(), no))
        });
        if let Some((path, no)) = here
            && let Some(file) = fresh.diff.files.iter().position(|f| f.path() == path)
        {
            let at_line = |row: &Row| {
                row.file() == Some(file)
                    && no.is_some()
                    && match *row {
                        Row::Line { file, hunk, line } => {
                            let l = fresh.line(file, hunk, line);
                            l.new_no.or(l.old_no) == no
                        }
                        Row::Context { no: n, .. } => Some(n) == no,
                        _ => false,
                    }
            };
            let in_file = |row: &Row| row.file() == Some(file);
            let stop = fresh
                .stops(at_line, |_| false)
                .first()
                .or(fresh.stops(in_file, |f| f == file).first())
                .copied();
            if let Some((page, row)) = stop {
                fresh.open_page(page, row);
                fresh.column = self.column;
            }
        }

        fresh.file_view = self.file_view.take();
        fresh.message = None;
        *self = fresh;
    }
}

/// Whether two diffs have the same files with the same hunks, which means
/// nothing the review hangs on moved.
fn same_hunks(a: &Diff, b: &Diff) -> bool {
    a.sections.len() == b.sections.len()
        && a.files.len() == b.files.len()
        && a.files.iter().zip(&b.files).all(|(x, y)| {
            x.old_path == y.old_path
                && x.new_path == y.new_path
                && x.hunks.len() == y.hunks.len()
                && x.hunks.iter().zip(&y.hunks).all(|(h, k)| h.id == k.id)
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::test_support::*;
    use super::*;
    use crate::diff::LineKind;

    fn text(lines: usize) -> Source {
        Arc::new((1..=lines).map(|i| format!("line {i}")).collect())
    }

    /// a.rs rows: 0 header, 1 hunk, 2 line 2, 3 hunk, 4 line 10.
    fn before() -> Diff {
        Diff::new(vec![
            file_at("a.rs", &[(2, "x"), (10, "y")]),
            file_at("b.rs", &[(1, "z")]),
        ])
    }

    fn app_in_file_view() -> App {
        let mut app = App::new(String::new(), before(), Default::default());
        app.set_viewport(80, 20);
        app.cursor = 4;
        let mut view = FileView::new("a.rs".into(), Side::New, text(30), Some(0), vec![None; 30]);
        view.caret.line = 9;
        app.file_view = Some(view);
        app
    }

    fn new_no(app: &App) -> Option<u32> {
        match app.current_row()? {
            Row::Line { file, hunk, line } => {
                let l = app.line(file, hunk, line);
                (l.kind == LineKind::Added).then_some(l.new_no).flatten()
            }
            _ => None,
        }
    }

    #[test]
    fn picks_up_new_hunks_and_keeps_the_review_and_the_place() {
        let mut app = app_in_file_view();
        let reviewed = app.diff.files[0].hunks[1].id.clone();
        app.review.set_reviewed(&reviewed, true);
        app.show_removals = false;
        let after = Diff::new(vec![
            file_at("a.rs", &[(2, "x"), (5, "edited"), (10, "y")]),
            file_at("b.rs", &[(1, "z")]),
        ]);

        app.reload(after, Some(text(31)));

        assert_eq!(app.diff.files[0].hunks.len(), 3);
        assert!(app.review.is_reviewed(&reviewed));
        assert!(!app.show_removals);
        assert_eq!(app.diff.files[0].path(), "a.rs");
        assert_eq!(new_no(&app), Some(10));
        let view = app.file_view.as_ref().unwrap();
        assert_eq!((view.lines.len(), view.line_no()), (31, 10));
        assert_eq!(view.file, Some(0));
    }

    #[test]
    fn the_same_hunks_only_refresh_the_file() {
        let mut app = app_in_file_view();
        app.expanded.insert((0, 0), Default::default());
        app.reload(before(), Some(text(5)));
        // Nothing the diff hangs on was rebuilt.
        assert!(app.expanded.contains_key(&(0, 0)));
        assert_eq!(app.cursor, 4);
        let view = app.file_view.as_ref().unwrap();
        assert_eq!((view.lines.len(), view.line_no()), (5, 5));
    }

    #[test]
    fn a_file_that_went_away_closes() {
        let mut app = app_in_file_view();
        app.reload(Diff::new(vec![file_at("b.rs", &[(1, "z")])]), None);
        assert!(app.file_view.is_none());
        assert_eq!(app.message.as_deref(), Some("a.rs is gone"));
        assert_eq!(app.diff.files.len(), 1);
    }
}
