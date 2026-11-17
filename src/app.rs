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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Down(usize),
    Up(usize),
    HalfPageDown,
    HalfPageUp,
    Top,
    Bottom,
    Quit,
}

pub struct App {
    pub title: String,
    pub diff: Diff,
    pub rows: Vec<Row>,
    pub cursor: usize,
    /// Index of the first visible row.
    pub scroll: usize,
    pub viewport_height: usize,
    /// Width of each line-number column, sized to the largest number in the diff.
    pub gutter_width: usize,
    pub should_quit: bool,
}

impl App {
    pub fn new(title: String, diff: Diff) -> Self {
        let rows = flatten(&diff);
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
            cursor: 0,
            scroll: 0,
            viewport_height: 0,
            gutter_width: max_line_no.to_string().len(),
            should_quit: false,
        }
    }

    pub fn update(&mut self, action: Action) {
        let half_page = (self.viewport_height / 2).max(1) as isize;
        match action {
            Action::Down(n) => self.move_by(n as isize),
            Action::Up(n) => self.move_by(-(n as isize)),
            Action::HalfPageDown => self.move_by(half_page),
            Action::HalfPageUp => self.move_by(-half_page),
            Action::Top => self.cursor = 0,
            Action::Bottom => self.cursor = self.rows.len().saturating_sub(1),
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

    pub fn line(&self, file: usize, hunk: usize, line: usize) -> &DiffLine {
        &self.diff.files[file].hunks[hunk].lines[line]
    }

    fn move_by(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
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

fn flatten(diff: &Diff) -> Vec<Row> {
    let mut rows = Vec::new();
    for (file, f) in diff.files.iter().enumerate() {
        rows.push(Row::File { file });
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

    fn sample_diff() -> Diff {
        let line = |content: &str| DiffLine {
            kind: LineKind::Context,
            old_no: Some(1),
            new_no: Some(1),
            content: content.to_string(),
        };
        let hunk = |n: usize| Hunk {
            header: "@@ -1,1 +1,1 @@".to_string(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines: (0..n).map(|i| line(&i.to_string())).collect(),
        };
        let file = |hunks: Vec<Hunk>| FileDiff {
            old_path: Some("f".into()),
            new_path: Some("f".into()),
            status: FileStatus::Modified,
            is_binary: false,
            hunks,
        };
        Diff { files: vec![file(vec![hunk(3), hunk(2)]), file(vec![hunk(4)])] }
    }

    #[test]
    fn flattens_files_hunks_and_lines() {
        let app = App::new(String::new(), sample_diff());
        // file + (hunk + 3) + (hunk + 2) + file + (hunk + 4)
        assert_eq!(app.rows.len(), 1 + 4 + 3 + 1 + 5);
        assert_eq!(app.rows[0], Row::File { file: 0 });
        assert_eq!(app.rows[2], Row::Line { file: 0, hunk: 0, line: 0 });
        assert_eq!(app.rows[8], Row::File { file: 1 });
    }

    #[test]
    fn cursor_clamps_to_bounds() {
        let mut app = App::new(String::new(), sample_diff());
        app.update(Action::Up(5));
        assert_eq!(app.cursor, 0);
        app.update(Action::Down(100));
        assert_eq!(app.cursor, app.rows.len() - 1);
        app.update(Action::Top);
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn scroll_follows_cursor() {
        let mut app = App::new(String::new(), sample_diff());
        app.set_viewport_height(4);
        app.update(Action::Down(6));
        assert_eq!(app.scroll, 3);
        app.update(Action::HalfPageUp);
        assert_eq!(app.cursor, 4);
        assert_eq!(app.scroll, 3);
        app.update(Action::Top);
        assert_eq!(app.scroll, 0);
    }
}
