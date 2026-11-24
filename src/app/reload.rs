//! Picking the diff up again after the files changed under it, as they do
//! when Neovim edits one from the file view, or another window does while
//! rrrev watches.

use std::collections::HashMap;

use super::file_view::{FileView, marks};
use super::{App, Mode, Row, View};
use crate::diff::{Diff, HunkId, Side, Source};

impl App {
    /// The diff as it is now, and the text of the file the file view is on
    /// (`None` if it's gone). Everything that hangs off the old diff's
    /// files and hunks is rebuilt from the new one, as a fresh start would;
    /// the review itself is kept by hunk id. The file view and the diff's
    /// cursor stay where they were, as near as the new text allows.
    /// Whether anything changed.
    pub fn reload(&mut self, diff: Diff, file: Option<Source>) -> bool {
        let diff_changed = self.reload_diff(diff);
        let file_changed = self.file_view.is_some() && self.reload_file(file);
        diff_changed || file_changed
    }

    /// The diff as it is now, if it's any different. The file view keeps
    /// its text but has its marks redone from the new diff. Whether it was
    /// different.
    pub fn reload_diff(&mut self, diff: Diff) -> bool {
        if self.diff.fingerprint() == diff.fingerprint() {
            return false;
        }
        self.replace_diff(diff);
        if let Some(view) = self.file_view.take() {
            let lines = view.lines.clone();
            self.reopen_file_view(view, lines);
        }
        true
    }

    /// The text of the file the file view is on now, or `None` if it's
    /// gone, which closes the view. Whether it changed.
    pub fn reload_file(&mut self, file: Option<Source>) -> bool {
        let Some(view) = self.file_view.take() else {
            return false;
        };
        let Some(lines) = file else {
            self.message = Some(format!("{} is gone", view.path));
            return true;
        };
        let changed = lines != view.lines;
        self.reopen_file_view(view, lines);
        changed
    }

    /// Whether a reload can go ahead without pulling something out from
    /// under the user: not while they type, select, or have a pop-up,
    /// prompt or page open over the diff, all of which a rebuild would
    /// close or point at the wrong rows.
    pub fn can_reload(&self) -> bool {
        matches!(self.mode, Mode::Normal)
            && self.help_scroll.is_none()
            && !self.show_verdict
            && self.walkthrough.is_none()
            && self.theme_picker.is_none()
            && self.comments_menu.is_none()
            && self.dismiss_prompt.is_none()
            && self.language_prompt.is_none()
            && !self.ask.confirm_clear
            && self.references.is_none()
            && self.hover.is_none()
    }

    /// `view` again on `lines`, its marks from the diff as it is now, its
    /// cursor and scroll kept as near as the new text allows.
    fn reopen_file_view(&mut self, view: FileView, lines: Source) {
        let file = self.diff.files.iter().position(|f| {
            let path = match view.side {
                Side::New => &f.new_path,
                Side::Old => &f.old_path,
            };
            path.as_deref() == Some(view.path.as_str())
        });
        let marks = match file {
            Some(file) => marks(&self.diff.files[file], view.side, lines.len()),
            None => vec![None; lines.len()],
        };
        let mut fresh = FileView::new(view.path, view.side, lines, file, marks);
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
        fresh.updated = self.updated;
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

        // Folded files stay folded, by path, and folded or widened hunks
        // stay so, by id.
        let folded: HashMap<&str, bool> = self
            .diff
            .files
            .iter()
            .zip(&self.collapsed)
            .map(|(f, &c)| (f.path(), c))
            .collect();
        let mut hunks: HashMap<&HunkId, (usize, usize)> = HashMap::new();
        for (i, f) in self.diff.files.iter().enumerate() {
            for (j, h) in f.hunks.iter().enumerate() {
                hunks.insert(&h.id, (i, j));
            }
        }
        for (i, f) in fresh.diff.files.iter().enumerate() {
            if let Some(&c) = folded.get(f.path()) {
                fresh.collapsed[i] = c;
            }
            for (j, h) in f.hunks.iter().enumerate() {
                let Some(&(oi, oj)) = hunks.get(&h.id) else {
                    continue;
                };
                if let Some(&fold) = self.hunk_folds.get(oi).and_then(|f| f.get(oj)) {
                    fresh.hunk_folds[i][j] = fold;
                }
                if let Some(&e) = self.expanded.get(&(oi, oj)) {
                    fresh.expanded.insert((i, j), e);
                }
            }
        }
        fresh.rebuild_rows();

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
    fn an_unchanged_diff_is_no_news() {
        let mut app = app_in_file_view();
        assert!(!app.reload_diff(before()));
        assert!(!app.reload_file(Some(text(30))));
        assert!(app.reload_file(Some(text(31))));
    }

    #[test]
    fn folded_hunks_stay_folded_by_id() {
        let mut app = app_in_file_view();
        app.file_view = None;
        app.hunk_folds[0][1] = true;
        app.expanded.insert((0, 1), Default::default());
        let after = Diff::new(vec![
            file_at("a.rs", &[(2, "x"), (5, "edited"), (10, "y")]),
            file_at("b.rs", &[(1, "z")]),
        ]);
        assert!(app.reload_diff(after));
        assert_eq!(app.hunk_folds[0], [false, false, true]);
        assert!(app.expanded.contains_key(&(0, 2)));
    }

    #[test]
    fn the_old_side_keeps_its_text_when_the_diff_changes() {
        let mut app = app_in_file_view();
        let old = text(12);
        let view = FileView::new(
            "a.rs".into(),
            Side::Old,
            old.clone(),
            Some(0),
            vec![None; 12],
        );
        app.file_view = Some(view);
        let after = Diff::new(vec![
            file_at("b.rs", &[(1, "z")]),
            file_at("a.rs", &[(2, "x")]),
        ]);
        assert!(app.reload_diff(after));
        let view = app.file_view.as_ref().unwrap();
        assert_eq!((view.side, view.file), (Side::Old, Some(1)));
        assert_eq!(view.lines, old);
    }

    #[test]
    fn waits_while_the_user_types_or_has_a_pop_up_open() {
        let mut app = app_in_file_view();
        assert!(app.can_reload());
        app.help_scroll = Some(0);
        assert!(!app.can_reload());
        app.help_scroll = None;
        app.mode = Mode::Search {
            input: Default::default(),
        };
        assert!(!app.can_reload());
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
