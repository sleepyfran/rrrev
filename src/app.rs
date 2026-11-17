use crate::diff::{Diff, DiffLine, Hunk};
use crate::review::ReviewState;

/// One visible row of the current file, pointing back into the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    File { file: usize },
    Hunk { file: usize, hunk: usize },
    Line { file: usize, hunk: usize, line: usize },
}

impl Row {
    pub fn file(self) -> usize {
        match self {
            Row::File { file } | Row::Hunk { file, .. } | Row::Line { file, .. } => file,
        }
    }

    pub fn hunk(self) -> Option<usize> {
        match self {
            Row::File { .. } => None,
            Row::Hunk { hunk, .. } | Row::Line { hunk, .. } => Some(hunk),
        }
    }

    fn is_hunk(&self) -> bool {
        matches!(self, Row::Hunk { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Down(usize),
    Up(usize),
    HalfPageDown,
    HalfPageUp,
    Top,
    Bottom,
    NextHunk(usize),
    PrevHunk(usize),
    NextFile(usize),
    PrevFile(usize),
    NextUnreviewed(usize),
    PrevUnreviewed(usize),
    ScrollCursorTop,
    ScrollCursorCenter,
    ScrollCursorBottom,
    ToggleFold,
    OpenFold,
    CloseFold,
    OpenAllFolds,
    CloseAllFolds,
    ToggleReviewed,
    ToggleFileReviewed,
    ToggleFocus,
    FocusSidebar,
    FocusDiff,
    ToggleSidebar,
    Select,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Diff,
    Sidebar,
}

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

pub struct App {
    pub title: String,
    pub diff: Diff,
    pub review: ReviewState,
    /// The file shown in the diff pane; only one is shown at a time.
    pub file: usize,
    /// Rows of the current file.
    pub rows: Vec<Row>,
    /// Per file: whether its hunks are folded away.
    pub collapsed: Vec<bool>,
    pub cursor: usize,
    /// Index of the first visible row.
    pub scroll: usize,
    pub viewport_height: usize,
    /// Width of each line-number column, sized to the largest number in the diff.
    pub gutter_width: usize,
    pub focus: Focus,
    pub show_sidebar: bool,
    /// One-off feedback for the status line, cleared by the next action.
    pub message: Option<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new(title: String, diff: Diff, review: ReviewState) -> Self {
        let max_line_no = diff
            .files
            .iter()
            .flat_map(|f| &f.hunks)
            .map(|h| (h.old_start + h.old_lines).max(h.new_start + h.new_lines))
            .max()
            .unwrap_or(0);
        let mut app = Self {
            title,
            diff,
            review,
            file: 0,
            rows: Vec::new(),
            collapsed: Vec::new(),
            cursor: 0,
            scroll: 0,
            viewport_height: 0,
            gutter_width: max_line_no.to_string().len(),
            focus: Focus::Diff,
            show_sidebar: true,
            message: None,
            should_quit: false,
        };
        // Generated files and files already reviewed start out of the way.
        app.collapsed = (0..app.diff.files.len())
            .map(|f| app.diff.files[f].looks_generated() || app.file_progress(f).is_complete())
            .collect();
        app.open_file(0, 0);
        app
    }

    pub fn update(&mut self, action: Action) {
        self.message = None;
        let action = match self.focus {
            Focus::Diff => action,
            Focus::Sidebar => sidebar_action(action),
        };
        let half_page = (self.viewport_height / 2).max(1) as isize;
        let last_file = self.diff.files.len().saturating_sub(1);
        match action {
            Action::Down(n) => self.move_by(n as isize),
            Action::Up(n) => self.move_by(-(n as isize)),
            Action::HalfPageDown => self.move_by(half_page),
            Action::HalfPageUp => self.move_by(-half_page),
            Action::Top => self.cursor = 0,
            Action::Bottom => self.cursor = self.rows.len().saturating_sub(1),
            Action::NextHunk(n) => {
                let rows = self.rows.iter().enumerate().skip(self.cursor + 1);
                let target = rows.filter(|(_, row)| row.is_hunk()).take(n).last();
                self.jump_in_file(target.map(|(idx, _)| idx), "Last hunk in this file");
            }
            Action::PrevHunk(n) => {
                let rows = self.rows[..self.cursor].iter().enumerate().rev();
                let target = rows.filter(|(_, row)| row.is_hunk()).take(n).last();
                self.jump_in_file(target.map(|(idx, _)| idx), "First hunk in this file");
            }
            Action::NextFile(_) if self.file >= last_file => {
                self.message = Some("Last file".to_string())
            }
            Action::PrevFile(_) if self.file == 0 => self.message = Some("First file".to_string()),
            Action::NextFile(n) => self.open_file(self.file.saturating_add(n).min(last_file), 0),
            Action::PrevFile(n) => self.open_file(self.file.saturating_sub(n), 0),
            Action::NextUnreviewed(n) => {
                let here = (self.file, self.cursor);
                let target = self.unreviewed_stops().into_iter().filter(|&s| s > here).take(n).last();
                self.jump_to_unreviewed(target);
            }
            Action::PrevUnreviewed(n) => {
                let here = (self.file, self.cursor);
                let stops = self.unreviewed_stops().into_iter().rev();
                let target = stops.filter(|&s| s < here).take(n).last();
                self.jump_to_unreviewed(target);
            }
            Action::ScrollCursorTop => self.scroll = self.cursor,
            Action::ScrollCursorCenter => {
                self.scroll = self.cursor.saturating_sub(self.viewport_height / 2)
            }
            Action::ScrollCursorBottom => {
                self.scroll = (self.cursor + 1).saturating_sub(self.viewport_height)
            }
            Action::ToggleFold => {
                if let Some(file) = self.current_file() {
                    self.collapsed[file] = !self.collapsed[file];
                    self.rebuild_rows();
                }
            }
            Action::OpenFold | Action::CloseFold => {
                if let Some(file) = self.current_file() {
                    self.collapsed[file] = action == Action::CloseFold;
                    self.rebuild_rows();
                }
            }
            Action::OpenAllFolds | Action::CloseAllFolds => {
                self.collapsed.fill(action == Action::CloseAllFolds);
                self.rebuild_rows();
            }
            Action::ToggleReviewed => match self.current_row() {
                Some(Row::Hunk { file, hunk } | Row::Line { file, hunk, .. }) => {
                    let reviewed = self.is_reviewed(file, hunk);
                    self.review.set_reviewed(&self.diff.files[file].hunks[hunk].id, !reviewed);
                }
                Some(Row::File { .. }) => self.update(Action::ToggleFileReviewed),
                None => {}
            },
            Action::ToggleFileReviewed => {
                if let Some(file) = self.current_file() {
                    // Mark the rest when partly done; only a fully reviewed file is unmarked.
                    let reviewed = !self.file_progress(file).is_complete();
                    for hunk in &self.diff.files[file].hunks {
                        self.review.set_reviewed(&hunk.id, reviewed);
                    }
                    self.collapsed[file] = reviewed;
                    self.rebuild_rows();
                }
            }
            Action::ToggleFocus => {
                self.focus = match self.focus {
                    Focus::Diff if self.show_sidebar => Focus::Sidebar,
                    _ => Focus::Diff,
                }
            }
            Action::FocusSidebar if self.show_sidebar => self.focus = Focus::Sidebar,
            Action::FocusSidebar => {}
            Action::FocusDiff => self.focus = Focus::Diff,
            Action::ToggleSidebar => {
                self.show_sidebar = !self.show_sidebar;
                self.focus = Focus::Diff;
            }
            Action::Select => {
                if let Some(Row::File { .. }) = self.current_row() {
                    self.update(Action::ToggleFold);
                }
            }
            Action::Quit => self.should_quit = true,
        }
        self.scroll_into_view();
    }

    pub fn set_viewport_height(&mut self, height: usize) {
        self.viewport_height = height;
        self.scroll_into_view();
    }

    pub fn current_row(&self) -> Option<Row> {
        self.rows.get(self.cursor).copied()
    }

    /// `None` only when the diff has no files.
    pub fn current_file(&self) -> Option<usize> {
        (self.file < self.diff.files.len()).then_some(self.file)
    }

    pub fn hunk(&self, file: usize, hunk: usize) -> &Hunk {
        &self.diff.files[file].hunks[hunk]
    }

    pub fn line(&self, file: usize, hunk: usize, line: usize) -> &DiffLine {
        &self.hunk(file, hunk).lines[line]
    }

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

    /// Every place `]u`/`[u` can stop, as `(file, row)` in diff order:
    /// unreviewed hunks, and the headers of folded files that still hide some.
    fn unreviewed_stops(&self) -> Vec<(usize, usize)> {
        let mut stops = Vec::new();
        for file in 0..self.diff.files.len() {
            if self.collapsed[file] {
                if !self.file_progress(file).is_complete() {
                    stops.push((file, 0));
                }
                continue;
            }
            for (row, r) in flatten_file(&self.diff, file, false).into_iter().enumerate() {
                if let Row::Hunk { file, hunk } = r
                    && !self.is_reviewed(file, hunk)
                {
                    stops.push((file, row));
                }
            }
        }
        stops
    }

    fn open_file(&mut self, file: usize, cursor: usize) {
        if file >= self.diff.files.len() {
            return;
        }
        if file != self.file || self.rows.is_empty() {
            self.file = file;
            self.rows = flatten_file(&self.diff, file, self.collapsed[file]);
            self.scroll = 0;
        }
        self.cursor = cursor.min(self.rows.len() - 1);
    }

    fn move_by(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
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

    fn jump_to_unreviewed(&mut self, target: Option<(usize, usize)>) {
        match target {
            Some((file, row)) => {
                self.open_file(file, row);
                self.scroll = row;
            }
            None if self.progress().is_complete() => {
                self.message = Some("Everything reviewed ✓".to_string())
            }
            None => self.message = Some("No more unreviewed hunks in this direction".to_string()),
        }
    }

    /// Re-flattens the current file after a fold change, keeping the cursor on
    /// the same row, or on the file header if that row was folded away.
    fn rebuild_rows(&mut self) {
        let Some(file) = self.current_file() else { return };
        let current = self.current_row();
        self.rows = flatten_file(&self.diff, file, self.collapsed[file]);
        self.cursor = current.and_then(|row| self.rows.iter().position(|r| *r == row)).unwrap_or(0);
    }

    fn scroll_into_view(&mut self) {
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

/// In the sidebar, vertical motions move between files instead of rows.
fn sidebar_action(action: Action) -> Action {
    match action {
        Action::Down(n) => Action::NextFile(n),
        Action::Up(n) => Action::PrevFile(n),
        Action::Top => Action::PrevFile(usize::MAX),
        Action::Bottom => Action::NextFile(usize::MAX),
        Action::Select => Action::FocusDiff,
        other => other,
    }
}

fn flatten_file(diff: &Diff, file: usize, collapsed: bool) -> Vec<Row> {
    let mut rows = vec![Row::File { file }];
    if collapsed {
        return rows;
    }
    for (hunk, h) in diff.files[file].hunks.iter().enumerate() {
        rows.push(Row::Hunk { file, hunk });
        rows.extend((0..h.lines.len()).map(|line| Row::Line { file, hunk, line }));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{FileDiff, FileStatus, LineKind};

    /// Every line is an addition whose content includes the path and hunk
    /// index, so each hunk gets a distinct id.
    fn file(path: &str, hunk_sizes: &[usize]) -> FileDiff {
        let hunk = |(h, &n): (usize, &usize)| Hunk {
            id: Default::default(),
            header: "@@ -1,1 +1,1 @@".to_string(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines: (0..n)
                .map(|i| DiffLine {
                    kind: LineKind::Added,
                    old_no: None,
                    new_no: Some(i as u32 + 1),
                    content: format!("{path} {h} {i}"),
                })
                .collect(),
        };
        FileDiff {
            old_path: Some(path.into()),
            new_path: Some(path.into()),
            status: FileStatus::Modified,
            is_binary: false,
            hunks: hunk_sizes.iter().enumerate().map(hunk).collect(),
        }
    }

    fn app_with(files: Vec<FileDiff>) -> App {
        App::new(String::new(), Diff::new(files), ReviewState::default())
    }

    /// a.rs rows: 0 header, 1 hunk, 2-4 lines, 5 hunk, 6-7 lines.
    /// b.rs rows: 0 header, 1 hunk, 2-5 lines.
    fn sample_app() -> App {
        app_with(vec![file("a.rs", &[3, 2]), file("b.rs", &[4])])
    }

    fn position(app: &App) -> (usize, usize) {
        (app.file, app.cursor)
    }

    #[test]
    fn shows_only_the_current_file() {
        let app = sample_app();
        assert_eq!(app.rows.len(), 8);
        assert!(app.rows.iter().all(|r| r.file() == 0));
        assert_eq!(app.rows[2], Row::Line { file: 0, hunk: 0, line: 0 });
    }

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
    fn hunk_jumps_stay_in_the_file() {
        let mut app = sample_app();
        app.set_viewport_height(4);
        app.update(Action::NextHunk(1));
        assert_eq!((app.cursor, app.scroll), (1, 1));
        app.update(Action::NextHunk(5));
        assert_eq!(position(&app), (0, 5));
        app.update(Action::NextHunk(1));
        assert_eq!(position(&app), (0, 5));
        assert!(app.message.as_deref().unwrap().starts_with("Last hunk"));

        // From inside a hunk, going back lands on that hunk's header first.
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
        // At the last file there's nowhere to go: stay put, don't jump to the top.
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
        assert_eq!(app.file, 1);
        app.update(Action::Top);
        assert_eq!(app.file, 0);
        app.update(Action::Bottom);
        assert_eq!(app.file, 1);
        app.update(Action::Select);
        assert_eq!(app.focus, Focus::Diff);
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
        // Partly reviewed: R completes the file rather than clearing it.
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
        assert!(app.rows.iter().all(|r| r.file() == 1));
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

        app.update(Action::ToggleFileReviewed);
        app.update(Action::NextUnreviewed(1));
        assert_eq!(app.message.as_deref(), Some("Everything reviewed ✓"));
    }

    #[test]
    fn reviewed_files_start_folded() {
        let files = || vec![file("a.rs", &[1]), file("b.rs", &[1])];
        let mut app = app_with(files());
        app.update(Action::ToggleFileReviewed);
        let app = App::new(String::new(), Diff::new(files()), app.review);
        assert_eq!(app.collapsed, [true, false]);
    }

    #[test]
    fn empty_diff_is_harmless() {
        let mut app = app_with(Vec::new());
        assert!(app.rows.is_empty());
        for action in [Action::Down(1), Action::NextFile(1), Action::NextUnreviewed(1), Action::ToggleFileReviewed] {
            app.update(action);
        }
        assert_eq!(app.current_file(), None);
    }
}
