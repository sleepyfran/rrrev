use crate::diff::{Diff, DiffLine};

/// One visible row of the flattened diff, pointing back into the model.
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

    fn is_file(&self) -> bool {
        matches!(self, Row::File { .. })
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
    ScrollCursorTop,
    ScrollCursorCenter,
    ScrollCursorBottom,
    ToggleFold,
    OpenFold,
    CloseFold,
    OpenAllFolds,
    CloseAllFolds,
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

pub struct App {
    pub title: String,
    pub diff: Diff,
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
    pub should_quit: bool,
}

impl App {
    pub fn new(title: String, diff: Diff) -> Self {
        let collapsed: Vec<bool> = diff.files.iter().map(|f| f.looks_generated()).collect();
        let rows = flatten(&diff, &collapsed);
        let max_line_no = diff
            .files
            .iter()
            .flat_map(|f| &f.hunks)
            .map(|h| (h.old_start + h.old_lines).max(h.new_start + h.new_lines))
            .max()
            .unwrap_or(0);
        Self {
            title,
            diff,
            rows,
            collapsed,
            cursor: 0,
            scroll: 0,
            viewport_height: 0,
            gutter_width: max_line_no.to_string().len(),
            focus: Focus::Diff,
            show_sidebar: true,
            should_quit: false,
        }
    }

    pub fn update(&mut self, action: Action) {
        let action = match self.focus {
            Focus::Diff => action,
            Focus::Sidebar => sidebar_action(action),
        };
        let half_page = (self.viewport_height / 2).max(1) as isize;
        match action {
            Action::Down(n) => self.move_by(n as isize),
            Action::Up(n) => self.move_by(-(n as isize)),
            Action::HalfPageDown => self.move_by(half_page),
            Action::HalfPageUp => self.move_by(-half_page),
            Action::Top => self.cursor = 0,
            Action::Bottom => self.cursor = self.rows.len().saturating_sub(1),
            Action::NextHunk(n) => self.jump_forward(n, Row::is_hunk),
            Action::PrevHunk(n) => self.jump_back(n, Row::is_hunk),
            Action::NextFile(n) => self.jump_forward(n, Row::is_file),
            Action::PrevFile(n) => self.jump_back(n, Row::is_file),
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

    pub fn current_file(&self) -> Option<usize> {
        self.current_row().map(Row::file)
    }

    pub fn line(&self, file: usize, hunk: usize, line: usize) -> &DiffLine {
        &self.diff.files[file].hunks[hunk].lines[line]
    }

    fn move_by(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
    }

    /// Moves to the `n`th matching row after the cursor, or the last one if
    /// there are fewer, and puts it at the top of the viewport.
    fn jump_forward(&mut self, n: usize, is_target: fn(&Row) -> bool) {
        let rows = self.rows.iter().enumerate().skip(self.cursor + 1);
        let target = rows.filter(|(_, row)| is_target(row)).take(n).last();
        self.jump_to(target.map(|(idx, _)| idx));
    }

    fn jump_back(&mut self, n: usize, is_target: fn(&Row) -> bool) {
        let rows = self.rows[..self.cursor].iter().enumerate().rev();
        let target = rows.filter(|(_, row)| is_target(row)).take(n).last();
        self.jump_to(target.map(|(idx, _)| idx));
    }

    fn jump_to(&mut self, target: Option<usize>) {
        if let Some(idx) = target {
            self.cursor = idx;
            self.scroll = idx;
        }
    }

    /// Re-flattens after a fold change, keeping the cursor on the same row, or
    /// on its file's header if that row was folded away.
    fn rebuild_rows(&mut self) {
        let current = self.current_row();
        self.rows = flatten(&self.diff, &self.collapsed);
        let position = |row: Row| self.rows.iter().position(|r| *r == row);
        self.cursor = current
            .and_then(|row| position(row).or_else(|| position(Row::File { file: row.file() })))
            .unwrap_or(0);
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
        Action::Bottom => Action::NextFile(usize::MAX),
        Action::Select => Action::FocusDiff,
        other => other,
    }
}

fn flatten(diff: &Diff, collapsed: &[bool]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (file, f) in diff.files.iter().enumerate() {
        rows.push(Row::File { file });
        if collapsed[file] {
            continue;
        }
        for (hunk, h) in f.hunks.iter().enumerate() {
            rows.push(Row::Hunk { file, hunk });
            rows.extend((0..h.lines.len()).map(|line| Row::Line { file, hunk, line }));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{FileDiff, FileStatus, Hunk, LineKind};

    fn file(path: &str, hunk_sizes: &[usize]) -> FileDiff {
        let line = |i: usize| DiffLine {
            kind: LineKind::Context,
            old_no: Some(i as u32 + 1),
            new_no: Some(i as u32 + 1),
            content: i.to_string(),
        };
        let hunk = |n: usize| Hunk {
            header: "@@ -1,1 +1,1 @@".to_string(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines: (0..n).map(line).collect(),
        };
        FileDiff {
            old_path: Some(path.into()),
            new_path: Some(path.into()),
            status: FileStatus::Modified,
            is_binary: false,
            hunks: hunk_sizes.iter().map(|&n| hunk(n)).collect(),
        }
    }

    /// Rows: 0 file a, 1 hunk, 2-4 lines, 5 hunk, 6-7 lines,
    ///       8 file b, 9 hunk, 10-13 lines.
    fn sample_app() -> App {
        let diff = Diff { files: vec![file("a.rs", &[3, 2]), file("b.rs", &[4])] };
        App::new(String::new(), diff)
    }

    #[test]
    fn flattens_files_hunks_and_lines() {
        let app = sample_app();
        assert_eq!(app.rows.len(), 14);
        assert_eq!(app.rows[2], Row::Line { file: 0, hunk: 0, line: 0 });
        assert_eq!(app.rows[8], Row::File { file: 1 });
    }

    #[test]
    fn cursor_clamps_to_bounds() {
        let mut app = sample_app();
        app.update(Action::Up(5));
        assert_eq!(app.cursor, 0);
        app.update(Action::Down(100));
        assert_eq!(app.cursor, 13);
        app.update(Action::Top);
        assert_eq!(app.cursor, 0);
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
    fn hunk_jumps_cross_files_and_stop_at_the_ends() {
        let mut app = sample_app();
        app.set_viewport_height(4);
        app.update(Action::NextHunk(1));
        assert_eq!((app.cursor, app.scroll), (1, 1));
        app.update(Action::NextHunk(2));
        assert_eq!(app.cursor, 9);
        app.update(Action::NextHunk(1));
        assert_eq!(app.cursor, 9);
        app.update(Action::NextHunk(1));
        assert_eq!(app.cursor, 9);

        // From inside a hunk, going back lands on that hunk's header first.
        app.update(Action::Down(2));
        app.update(Action::PrevHunk(1));
        assert_eq!(app.cursor, 9);
        app.update(Action::PrevHunk(10));
        assert_eq!(app.cursor, 1);
    }

    #[test]
    fn file_jumps() {
        let mut app = sample_app();
        app.update(Action::NextFile(1));
        assert_eq!(app.cursor, 8);
        app.update(Action::Down(3));
        app.update(Action::PrevFile(1));
        assert_eq!(app.cursor, 8);
    }

    #[test]
    fn sidebar_motions_move_between_files() {
        let mut app = sample_app();
        app.update(Action::ToggleFocus);
        assert_eq!(app.focus, Focus::Sidebar);
        app.update(Action::Down(1));
        assert_eq!(app.cursor, 8);
        app.update(Action::Top);
        assert_eq!(app.cursor, 0);
        app.update(Action::Bottom);
        assert_eq!(app.cursor, 8);
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
        assert_eq!(app.rows.len(), 1 + 6);
        assert_eq!(app.cursor, 0);
        app.update(Action::NextFile(1));
        assert_eq!(app.cursor, 1);

        app.update(Action::CloseAllFolds);
        assert_eq!(app.rows, [Row::File { file: 0 }, Row::File { file: 1 }]);
        assert_eq!(app.cursor, 1);
        app.update(Action::OpenAllFolds);
        assert_eq!(app.cursor, 8);
    }

    #[test]
    fn hunk_jumps_skip_folded_files() {
        let mut app = sample_app();
        app.update(Action::CloseFold);
        app.update(Action::NextHunk(1));
        assert_eq!(app.current_row(), Some(Row::Hunk { file: 1, hunk: 0 }));
    }

    #[test]
    fn generated_files_start_folded() {
        let diff = Diff { files: vec![file("Cargo.lock", &[3]), file("src/main.rs", &[1])] };
        let app = App::new(String::new(), diff);
        assert_eq!(app.collapsed, [true, false]);
        assert_eq!(app.rows.len(), 1 + 3);
    }
}
