//! What the diff pane shows: one page at a time (a file, or a group of the
//! review plan), folds, focus and scrolling.

use super::{App, Focus, Mode, StackPlace, View};
use crate::diff::LineKind;
use crate::review::Comment;
use crate::text::{wrapped_height, wrap_words};

/// One visible row of the current page, pointing back into the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// One line of a plan group's header (title, then rationale, wrapped to
    /// the pane); these rows open its page.
    Group { group: usize, line: usize },
    File { file: usize },
    Hunk { file: usize, hunk: usize },
    Line { file: usize, hunk: usize, line: usize },
    /// Line `no` of the file's text, shown as extra context next to a hunk.
    Context { file: usize, hunk: usize, no: u32 },
    /// One line of a comment's text, shown under the code it's about.
    Comment { file: usize, id: u64, line: usize },
    /// An agent note (an index into `App::notes`), under the code it's about.
    Note { file: usize, hunk: usize, note: usize },
}

impl Row {
    pub fn file(self) -> Option<usize> {
        match self {
            Row::Group { .. } => None,
            Row::File { file }
            | Row::Hunk { file, .. }
            | Row::Line { file, .. }
            | Row::Context { file, .. }
            | Row::Comment { file, .. }
            | Row::Note { file, .. } => Some(file),
        }
    }

    pub fn hunk(self) -> Option<usize> {
        match self {
            Row::Group { .. } | Row::File { .. } | Row::Comment { .. } => None,
            Row::Hunk { hunk, .. } | Row::Line { hunk, .. } | Row::Context { hunk, .. } | Row::Note { hunk, .. } => {
                Some(hunk)
            }
        }
    }

    pub(super) fn is_hunk(&self) -> bool {
        matches!(self, Row::Hunk { .. })
    }
}

impl App {
    /// The diff pane's size, reported by the UI each frame. Group headers wrap
    /// to the width, so a new width rebuilds them.
    pub fn set_viewport(&mut self, width: usize, height: usize) {
        self.viewport_height = height;
        if width != self.viewport_width {
            self.viewport_width = width;
            if self.view != View::Files {
                self.rebuild_rows();
            }
        }
        self.scroll_into_view();
    }

    /// A group's header as shown, line by line: its title, then its rationale,
    /// word-wrapped to the pane. `true` marks title lines.
    pub fn group_header(&self, group: usize) -> Vec<(bool, String)> {
        let page = &self.pages()[group];
        // Room left after the `▌ ` bar; unwrapped until the UI reports a width.
        let width = match self.viewport_width {
            0 => usize::MAX,
            w => w.saturating_sub(GROUP_HEADER_INDENT).max(1),
        };
        let title = match page.place {
            None => format!("{}/{} {}", group + 1, self.pages().len(), page.title),
            Some(place) => {
                let (layers, layer) = (self.diff.sections.len(), &self.diff.sections[place.layer].title);
                let layer = format!("{}/{layers} {layer}", place.layer + 1);
                match place.group {
                    None => layer,
                    Some((i, count)) => format!("{layer} · {}/{count} {}", i + 1, page.title),
                }
            }
        };
        let title = wrap_words(&title, width).into_iter().map(|line| (true, line));
        let rationale = wrap_words(&page.rationale, width).into_iter().map(|line| (false, line));
        title.chain(rationale).collect()
    }

    /// A page's name on one line: a stack group's says which layer it's in.
    pub fn page_label(&self, page: usize) -> String {
        let Some(p) = self.pages().get(page) else { return String::new() };
        match p.place {
            Some(StackPlace { layer, group: Some(_) }) => {
                format!("{} › {}", self.diff.sections[layer].title, p.title)
            }
            _ => p.title.clone(),
        }
    }

    /// Where the page sits, for the status line: `layer 2/3 · group 1/4`.
    pub fn page_position(&self) -> String {
        match self.view {
            View::Files => format!("file {}/{}", self.page + 1, self.diff.files.len()),
            View::Plan => format!("group {}/{}", self.page + 1, self.plan.len()),
            View::Stack => match self.stack.get(self.page).and_then(|p| p.place) {
                Some(place) => {
                    let layer = format!("layer {}/{}", place.layer + 1, self.diff.sections.len());
                    match place.group {
                        Some((i, count)) => format!("{layer} · group {}/{count}", i + 1),
                        None => layer,
                    }
                }
                None => String::new(),
            },
        }
    }

    pub(super) fn page_count(&self) -> usize {
        match self.view {
            View::Files => self.diff.files.len(),
            View::Plan | View::Stack => self.pages().len(),
        }
    }

    /// The `(file, hunk)`s a page shows, in order.
    pub(super) fn page_hunks(&self, page: usize) -> Vec<(usize, usize)> {
        match self.view {
            View::Files => (0..self.diff.files[page].hunks.len()).map(|h| (page, h)).collect(),
            View::Plan | View::Stack => self.pages()[page].hunks.clone(),
        }
    }

    pub(super) fn page_rows(&self, page: usize) -> Vec<Row> {
        match self.view {
            View::Files => self.file_rows(page),
            View::Plan | View::Stack => self.group_rows(page),
        }
    }

    /// A file: its hunks unless folded, plus comments whose hunk is gone,
    /// right under the header.
    fn file_rows(&self, file: usize) -> Vec<Row> {
        let f = &self.diff.files[file];
        let mut rows = vec![Row::File { file }];
        let outdated = self.review.comments().iter().filter(|c| c.path == f.path() && self.is_outdated(c));
        rows.extend(outdated.flat_map(|c| comment_rows(file, c)));
        if !self.collapsed[file] {
            for hunk in 0..f.hunks.len() {
                self.push_hunk_rows(&mut rows, file, hunk);
            }
        }
        rows
    }

    /// A plan group: its title, then its hunks with a file header wherever
    /// the file changes. Folded files show only that header.
    fn group_rows(&self, group: usize) -> Vec<Row> {
        let header = self.group_header(group).len();
        let mut rows: Vec<Row> = (0..header).map(|line| Row::Group { group, line }).collect();
        let mut previous = None;
        for &(file, hunk) in &self.pages()[group].hunks {
            if previous != Some(file) {
                rows.push(Row::File { file });
                previous = Some(file);
            }
            if !self.collapsed[file] {
                self.push_hunk_rows(&mut rows, file, hunk);
            }
        }
        rows
    }

    /// A hunk's header and lines, with each comment and agent note under the
    /// line it ends on, between any extra context shown above and below.
    fn push_hunk_rows(&self, rows: &mut Vec<Row>, file: usize, hunk: usize) {
        if self.is_hunk_folded(file, hunk) {
            rows.push(Row::Hunk { file, hunk });
            return;
        }
        let h = &self.diff.files[file].hunks[hunk];
        let last = h.lines.len().saturating_sub(1);
        let (above, below) = self.context_lines(file, hunk);
        rows.push(Row::Hunk { file, hunk });
        rows.extend(above.into_iter().map(|no| Row::Context { file, hunk, no }));
        for line in 0..h.lines.len() {
            // A hidden removal's comments and notes stay, under the line above.
            if self.show_removals || h.lines[line].kind != LineKind::Removed {
                rows.push(Row::Line { file, hunk, line });
            }
            let here = self.review.comments().iter().filter(|c| c.hunk == h.id && c.end.min(last) == line);
            rows.extend(here.flat_map(|c| comment_rows(file, c)));
            let notes = self.notes.iter().enumerate().filter(|(_, n)| (n.file, n.hunk, n.line) == (file, hunk, line));
            rows.extend(notes.map(|(note, _)| Row::Note { file, hunk, note }));
        }
        rows.extend(below.into_iter().map(|no| Row::Context { file, hunk, no }));
    }

    pub(super) fn open_page(&mut self, page: usize, cursor: usize) {
        if page >= self.page_count() {
            return;
        }
        if page != self.page || self.rows.is_empty() {
            self.page = page;
            self.rows = self.page_rows(page);
            self.scroll = 0;
            if let Mode::Visual { .. } = self.mode {
                self.mode = Mode::Normal;
            }
        }
        self.cursor = cursor.min(self.rows.len() - 1);
    }

    /// Re-flattens the current page after a change, keeping the cursor on the
    /// same row, or near where it was if that row is gone.
    pub(super) fn rebuild_rows(&mut self) {
        if self.page >= self.page_count() {
            return;
        }
        let current = self.current_row();
        self.rows = self.page_rows(self.page);
        let kept = current.and_then(|row| self.rows.iter().position(|r| *r == row));
        // A deleted comment hands the cursor to the row after it; a folded
        // file, to its header; a folded hunk, to the hunk's header; a header
        // line lost to rewrapping, to the header.
        let fallback = || {
            if let Some(Row::Group { group, .. }) = current {
                let header = self.rows.iter().rposition(|r| matches!(*r, Row::Group { group: g, .. } if g == group));
                if let Some(idx) = header {
                    return idx;
                }
            }
            let header = current.and_then(Row::file).map(|file| Row::File { file });
            let header = header.and_then(|h| self.rows.iter().position(|r| *r == h));
            let folded = current.and_then(Row::file).is_some_and(|f| self.collapsed[f]);
            let folded_hunk = current.and_then(|r| Some((r.file()?, r.hunk()?))).filter(|&(f, h)| self.is_hunk_folded(f, h));
            let hunk_header = folded_hunk.and_then(|(file, hunk)| self.rows.iter().position(|r| *r == Row::Hunk { file, hunk }));
            match (header, hunk_header) {
                (Some(idx), _) if folded => idx,
                (_, Some(idx)) => idx,
                _ => self.cursor.min(self.rows.len() - 1),
            }
        };
        self.cursor = kept.unwrap_or_else(fallback);
    }

    /// `-`: hides or shows removed lines. A cursor on a removal being hidden
    /// moves to the next row that stays.
    pub(super) fn toggle_removals(&mut self) {
        self.show_removals = !self.show_removals;
        if let Mode::Visual { .. } = self.mode {
            self.mode = Mode::Normal;
        }
        if !self.show_removals {
            let stays = |row: &Row| !self.is_removal(*row);
            let next = self.rows[self.cursor.min(self.rows.len())..].iter().position(stays).map(|i| self.cursor + i);
            let prev = || self.rows[..self.cursor.min(self.rows.len())].iter().rposition(stays);
            if let Some(idx) = next.or_else(prev) {
                self.cursor = idx;
            }
        }
        self.rebuild_rows();
        self.message = Some(if self.show_removals { "Showing removals" } else { "Hiding removals" }.to_string());
    }

    /// Whether `row` is a removed line.
    pub fn is_removal(&self, row: Row) -> bool {
        matches!(row, Row::Line { file, hunk, line } if self.line(file, hunk, line).kind == LineKind::Removed)
    }

    /// Whether lines were removed right above `line` and nothing took their
    /// place, so the UI can mark the spot while removals are hidden.
    pub fn removed_above(&self, file: usize, hunk: usize, line: usize) -> bool {
        let lines = &self.hunk(file, hunk).lines;
        line > 0
            && lines[line].kind != LineKind::Added
            && lines[line].kind != LineKind::Removed
            && lines[line - 1].kind == LineKind::Removed
    }

    /// `za`: on a hunk header, folds or unfolds that hunk; anywhere else,
    /// the file.
    pub(super) fn toggle_fold(&mut self) {
        if let Some(Row::Hunk { file, hunk }) = self.current_row() {
            self.set_fold(!self.is_hunk_folded(file, hunk));
        } else if let Some(file) = self.current_file() {
            self.set_fold(!self.collapsed[file]);
        }
    }

    pub(super) fn set_fold(&mut self, collapsed: bool) {
        if let Some(Row::Hunk { file, hunk }) = self.current_row() {
            self.hunk_folds[file][hunk] = collapsed;
            self.rebuild_rows();
        } else if let Some(file) = self.current_file() {
            self.collapsed[file] = collapsed;
            self.rebuild_rows();
        }
    }

    /// `zR` opens every file and hunk; `zM` closes every file, and folds
    /// the plumbing again for when they're opened.
    pub(super) fn set_all_folds(&mut self, collapsed: bool) {
        self.collapsed.fill(collapsed);
        for file in 0..self.hunk_folds.len() {
            self.hunk_folds[file] = if collapsed { self.plumbing_folds(file) } else { vec![false; self.hunk_folds[file].len()] };
        }
        self.rebuild_rows();
    }

    pub fn is_hunk_folded(&self, file: usize, hunk: usize) -> bool {
        self.hunk_folds.get(file).and_then(|f| f.get(hunk)).copied().unwrap_or(false)
    }

    /// Which of a file's hunks start folded: its plumbing, unless the whole
    /// file is plumbing, which folds as a file instead.
    pub(super) fn plumbing_folds(&self, file: usize) -> Vec<bool> {
        let hunks = self.diff.files[file].hunks.len();
        let whole = self.diff.is_plumbing_file(file);
        (0..hunks).map(|h| !whole && self.diff.is_plumbing(file, h)).collect()
    }

    /// After a grammar arrives: folds the plumbing it finds, leaving what's
    /// already folded or open alone.
    pub(super) fn fold_new_plumbing(&mut self) {
        for file in 0..self.hunk_folds.len() {
            let found = self.plumbing_folds(file);
            for (folded, plumbing) in self.hunk_folds[file].iter_mut().zip(found) {
                *folded |= plumbing;
            }
        }
        self.rebuild_rows();
    }

    /// Opens whatever hides a row of `file`'s `hunk`: the file and the hunk.
    pub(super) fn unfold_hunk(&mut self, file: usize, hunk: usize) {
        self.collapsed[file] = false;
        if let Some(folded) = self.hunk_folds.get_mut(file).and_then(|f| f.get_mut(hunk)) {
            *folded = false;
        }
    }

    /// Whether the screen is too narrow to dock the sidebar, reported by the
    /// UI each frame. Then the sidebar only shows as a pop-up, open while it
    /// has focus.
    pub fn set_compact(&mut self, compact: bool) {
        self.compact = compact;
        // Widening with the docked sidebar hidden leaves nothing to focus.
        if !self.sidebar_available() {
            self.focus = Focus::Diff;
        }
    }

    /// Whether the sidebar can take focus: docked and shown, or as the pop-up.
    fn sidebar_available(&self) -> bool {
        self.show_sidebar || self.compact
    }

    pub(super) fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Diff if self.sidebar_available() => Focus::Sidebar,
            _ => Focus::Diff,
        }
    }


    /// `s`: shows or hides the docked sidebar; on a narrow screen, opens or
    /// closes the pop-up instead.
    pub(super) fn toggle_sidebar(&mut self) {
        if self.compact {
            self.toggle_focus();
        } else {
            self.show_sidebar = !self.show_sidebar;
            self.focus = Focus::Diff;
        }
    }

    pub(super) fn half_page(&self) -> isize {
        (self.viewport_height / 2).max(1) as isize
    }

    pub(super) fn scroll_into_view(&mut self) {
        // A clean working tree has no rows to scroll.
        if self.viewport_height == 0 || self.rows.is_empty() {
            return;
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else {
            // A cursor row taller than the pane shows from its top.
            let mut shown: usize = self.rows[self.scroll..=self.cursor].iter().map(|r| self.row_height(*r)).sum();
            while self.scroll < self.cursor && shown > self.viewport_height {
                shown -= self.row_height(self.rows[self.scroll]);
                self.scroll += 1;
            }
        }
    }

    /// Scrolls so that at most `above` screen lines of rows show above the
    /// cursor (`zz`, `zb`).
    pub(super) fn scroll_with_above(&mut self, above: usize) {
        let mut used = 0;
        self.scroll = self.cursor;
        while self.scroll > 0 {
            let height = self.row_height(self.rows[self.scroll - 1]);
            if used + height > above {
                break;
            }
            used += height;
            self.scroll -= 1;
        }
    }

    /// Column where code starts on a diff line: comment marker, both line
    /// numbers and the change bar, each followed by a space but the marker.
    pub fn code_column(&self) -> usize {
        2 * self.gutter_width + 5
    }

    /// Column where a comment's text starts, after its `┃ ✎ ` lead.
    pub fn comment_column(&self) -> usize {
        self.code_column() + 2
    }

    /// `(outdated L12-14) `, leading the first line of a comment whose code
    /// has changed; empty otherwise.
    pub fn outdated_label(&self, comment: &Comment) -> String {
        if !self.is_outdated(comment) {
            return String::new();
        }
        let lines = comment.new_lines.or(comment.old_lines).map_or(String::new(), |(a, b)| {
            if a == b { format!(" L{a}") } else { format!(" L{a}-{b}") }
        });
        format!("(outdated{lines}) ")
    }

    /// Screen lines a row takes: diff lines and comments wrap to the pane,
    /// hanging under their text; other rows take one.
    pub fn row_height(&self, row: Row) -> usize {
        let (column, text) = match row {
            Row::Line { file, hunk, line } => {
                (self.code_column(), self.line(file, hunk, line).content.replace('\t', "    ").chars().count())
            }
            Row::Context { file, no, .. } => {
                let text = super::context_text(&self.diff, file, no).unwrap_or("");
                (self.code_column(), text.replace('\t', "    ").chars().count())
            }
            Row::Comment { id, line, .. } => {
                let Some(comment) = self.review.comment(id) else { return 1 };
                let label = if line == 0 { self.outdated_label(comment).chars().count() } else { 0 };
                (self.comment_column(), label + comment.text.lines().nth(line).unwrap_or("").chars().count())
            }
            Row::Note { note, .. } => {
                let Some(note) = self.notes.get(note) else { return 1 };
                (self.comment_column(), NOTE_LABEL.chars().count() + note.text.chars().count())
            }
            Row::Group { .. } | Row::File { .. } | Row::Hunk { .. } => return 1,
        };
        wrapped_height(text, self.text_room(column))
    }

    /// Columns left for text starting at `column`; 0 (don't wrap) until the
    /// UI reports a width, or when there's no room at all.
    pub fn text_room(&self, column: usize) -> usize {
        self.viewport_width.saturating_sub(column)
    }
}

/// Leads an agent note's text.
pub const NOTE_LABEL: &str = "Claude: ";

/// Columns a group header line gives to its `▌ ` bar, plus one spare.
const GROUP_HEADER_INDENT: usize = 3;

/// A row per line of the comment's text.
fn comment_rows(file: usize, comment: &Comment) -> impl Iterator<Item = Row> + use<> {
    let (id, count) = (comment.id, comment.text.lines().count().max(1));
    (0..count).map(move |line| Row::Comment { file, id, line })
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
        assert!(app.rows.iter().all(|r| r.file() == Some(0)));
        assert_eq!(app.rows[2], Row::Line { file: 0, hunk: 0, line: 0 });
    }

    #[test]
    fn scroll_follows_cursor() {
        let mut app = sample_app();
        app.set_viewport(80, 4);
        app.update(Action::Down(6));
        assert_eq!(app.scroll, 3);
        app.update(Action::HalfPageUp);
        assert_eq!((app.cursor, app.scroll), (4, 3));
        app.update(Action::Top);
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn long_lines_wrap_and_scroll_by_screen_lines() {
        let mut app = sample_app();
        // Code starts at column 7, leaving 4 for each 8-char line: 2 lines each.
        app.set_viewport(11, 6);
        assert_eq!(app.row_height(app.rows[0]), 1);
        assert_eq!(app.row_height(app.rows[2]), 2);
        app.update(Action::Down(4));
        // Header, hunk and three wrapped lines need 8: the top two scroll off.
        assert_eq!(app.scroll, 2);
        app.update(Action::ScrollCursorCenter);
        assert_eq!(app.scroll, 3);
        app.update(Action::ScrollCursorBottom);
        assert_eq!(app.scroll, 2);
        // Wide enough: back to a line each.
        app.set_viewport(80, 6);
        assert_eq!(app.row_height(app.rows[2]), 1);
    }

    #[test]
    fn hiding_the_sidebar_returns_focus_to_the_diff() {
        let mut app = sample_app();
        app.update(Action::ToggleFocus);
        app.update(Action::ToggleSidebar);
        assert_eq!(app.focus, Focus::Diff);
        app.update(Action::ToggleFocus);
        assert_eq!(app.focus, Focus::Diff);
    }

    #[test]
    fn narrow_screens_use_the_sidebar_as_a_pop_up() {
        let mut app = sample_app();
        app.update(Action::ToggleSidebar);
        app.set_compact(true);
        // Even with the docked sidebar hidden, Tab opens the pop-up.
        app.update(Action::ToggleFocus);
        assert_eq!(app.focus, Focus::Sidebar);
        app.update(Action::Down(1));
        assert_eq!(app.page, 1);
        // Esc and q close it; q doesn't quit.
        app.update(Action::Cancel);
        assert_eq!(app.focus, Focus::Diff);
        app.update(Action::ToggleSidebar);
        assert_eq!(app.focus, Focus::Sidebar);
        app.update(Action::Quit);
        assert_eq!(app.focus, Focus::Diff);
        assert!(!app.should_quit);

        // Back to a wide screen with the docked sidebar hidden: nothing to focus.
        app.update(Action::ToggleFocus);
        app.set_compact(false);
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
    fn hiding_removals_keeps_the_file_as_it_is_now() {
        use crate::diff::DiffLine;
        let mut f = file("a.rs", &[4]);
        let kinds = [LineKind::Context, LineKind::Removed, LineKind::Added, LineKind::Removed];
        for (l, kind) in f.hunks[0].lines.iter_mut().zip(kinds) {
            l.kind = kind;
        }
        f.hunks[0].lines.push(DiffLine { kind: LineKind::Context, old_no: Some(9), new_no: Some(9), content: "x".into() });
        let mut app = app_with(vec![f]);
        let line = |line| Row::Line { file: 0, hunk: 0, line };
        // On the first removal: it goes, and the cursor moves to the addition.
        app.update(Action::Down(3));
        assert_eq!(app.current_row(), Some(line(1)));
        app.update(Action::ToggleRemovals);
        assert_eq!(app.rows[2..], [line(0), line(2), line(4)]);
        assert_eq!(app.current_row(), Some(line(2)));
        // The context after the trailing removal marks where it was.
        assert!(app.removed_above(0, 0, 4));
        assert!(!app.removed_above(0, 0, 2));
        app.update(Action::ToggleRemovals);
        assert_eq!(app.rows.len(), 7);
        assert_eq!(app.current_row(), Some(line(2)));
    }

    #[test]
    fn plumbing_starts_folded() {
        let mut app = plumbing_app();
        // a.rs: the import hunk is only its header; the function shows.
        assert_eq!(app.hunk_folds, [vec![true, false], vec![false]]);
        assert_eq!(app.rows[..3], [Row::File { file: 0 }, Row::Hunk { file: 0, hunk: 0 }, Row::Hunk { file: 0, hunk: 1 }]);
        // b.rs is all plumbing, so it folds as a file.
        assert_eq!(app.collapsed, [false, true]);

        // za on the header opens it, and again folds it, cursor and all.
        app.update(Action::Down(1));
        app.update(Action::ToggleFold);
        assert_eq!(app.rows[2..4], [Row::Line { file: 0, hunk: 0, line: 0 }, Row::Line { file: 0, hunk: 0, line: 1 }]);
        app.update(Action::Down(2));
        app.update(Action::Up(2));
        app.update(Action::ToggleFold);
        assert_eq!(app.current_row(), Some(Row::Hunk { file: 0, hunk: 0 }));
        assert!(app.is_hunk_folded(0, 0));
        // zc on a line folds the file, as before.
        app.update(Action::Down(2));
        app.update(Action::CloseFold);
        assert_eq!(app.rows, [Row::File { file: 0 }]);

        // zR opens everything; zM folds the plumbing again for later.
        app.update(Action::OpenAllFolds);
        assert_eq!(app.hunk_folds, [vec![false, false], vec![false]]);
        app.update(Action::CloseAllFolds);
        assert_eq!(app.hunk_folds, [vec![true, false], vec![false]]);
    }

    #[test]
    fn search_finds_what_folded_plumbing_hides() {
        let mut app = plumbing_app();
        app.update(Action::StartSearch);
        crate::app::test_support::type_text(&mut app, "c::D");
        app.update_input(crate::input::InputAction::Submit);
        assert_eq!(app.current_row(), Some(Row::Hunk { file: 0, hunk: 0 }));
        // And in the folded file after it, on its header.
        app.update(Action::SearchNext(1));
        assert_eq!(app.current_row(), Some(Row::File { file: 1 }));
    }

    #[test]
    fn generated_files_start_folded() {
        let app = app_with(vec![file("Cargo.lock", &[3]), file("src/main.rs", &[1])]);
        assert_eq!(app.collapsed, [true, false]);
        assert_eq!(app.rows, [Row::File { file: 0 }]);
    }
}
