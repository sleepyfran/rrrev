use std::path::PathBuf;

use crate::diff::{Diff, DiffLine, Hunk, LineKind};
use crate::export::Format;
use crate::input::{InputAction, TextInput};
use crate::review::{Comment, ReviewState};

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
    NextComment(usize),
    PrevComment(usize),
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
    StartVisual,
    StartComment,
    CommentInEditor,
    DeleteComment,
    EnterCommand,
    ToggleFocus,
    FocusSidebar,
    FocusDiff,
    ToggleSidebar,
    Select,
    Cancel,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Diff,
    Sidebar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Line-wise selection from `anchor` (a row index) to the cursor.
    Visual { anchor: usize },
    Comment { target: CommentTarget, input: TextInput },
    Command { input: TextInput },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentTarget {
    /// Inclusive range of line indices within one hunk.
    New { file: usize, hunk: usize, start: usize, end: usize },
    Edit { id: u64 },
}

/// Work `update` can't do itself, for the main loop to carry out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Edit the comment being written in `$EDITOR`, then call `finish_editor`.
    OpenEditor { initial: String },
    Export { format: Format, path: Option<PathBuf> },
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
    pub mode: Mode,
    pub show_sidebar: bool,
    /// One-off feedback for the status line, cleared by the next action.
    pub message: Option<String>,
    pub effect: Option<Effect>,
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
            mode: Mode::Normal,
            show_sidebar: true,
            message: None,
            effect: None,
            should_quit: false,
        };
        // Generated files and files already reviewed start out of the way.
        app.collapsed = (0..app.diff.files.len())
            .map(|f| app.diff.files[f].looks_generated() || app.file_progress(f).is_complete())
            .collect();
        app.open_file(0, 0);
        app
    }

    /// True while keys should go to a text field rather than the keymap.
    pub fn is_typing(&self) -> bool {
        matches!(self.mode, Mode::Comment { .. } | Mode::Command { .. })
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
            Action::NextUnreviewed(n) | Action::PrevUnreviewed(n) => {
                let stops = self.stops(
                    |row| matches!(*row, Row::Hunk { file, hunk } if !self.is_reviewed(file, hunk)),
                    |file| !self.file_progress(file).is_complete(),
                );
                let forward = matches!(action, Action::NextUnreviewed(_));
                if !self.jump_to_stop(&stops, n, forward) {
                    self.message = Some(if self.progress().is_complete() {
                        "Everything reviewed ✓".to_string()
                    } else {
                        "No more unreviewed hunks in this direction".to_string()
                    });
                }
            }
            Action::NextComment(n) | Action::PrevComment(n) => {
                let stops = self.stops(
                    |row| matches!(row, Row::Comment { line: 0, .. }),
                    |file| self.has_comments_in_hunks(file),
                );
                let forward = matches!(action, Action::NextComment(_));
                if !self.jump_to_stop(&stops, n, forward) {
                    self.message = Some("No more comments in this direction".to_string());
                }
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
                Some(Row::Comment { .. }) | None => {}
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
            Action::StartVisual => match (&self.mode, self.current_row()) {
                (Mode::Visual { .. }, _) => self.mode = Mode::Normal,
                (_, Some(Row::Hunk { .. } | Row::Line { .. })) => {
                    self.mode = Mode::Visual { anchor: self.cursor }
                }
                _ => self.message = Some("Move to a hunk to select lines".to_string()),
            },
            Action::StartComment | Action::CommentInEditor => match self.comment_target() {
                Some(target) => {
                    let text = match target {
                        CommentTarget::Edit { id } => self.review.comment(id).map_or("", |c| &c.text),
                        CommentTarget::New { .. } => "",
                    };
                    let input = TextInput::new(text);
                    if action == Action::CommentInEditor {
                        self.effect = Some(Effect::OpenEditor { initial: input.text().to_string() });
                    }
                    self.mode = Mode::Comment { target, input };
                }
                None => {
                    self.mode = Mode::Normal;
                    self.message = Some("Move to a line or hunk to comment".to_string());
                }
            },
            Action::DeleteComment => match self.current_row() {
                Some(Row::Comment { id, .. }) => {
                    self.review.delete_comment(id);
                    self.rebuild_rows();
                    self.message = Some("Comment deleted".to_string());
                }
                _ => self.message = Some("No comment under the cursor".to_string()),
            },
            Action::EnterCommand => self.mode = Mode::Command { input: TextInput::default() },
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
            Action::Cancel => self.mode = Mode::Normal,
            Action::Quit => self.should_quit = true,
        }
        self.scroll_into_view();
    }

    /// Handles a key typed into the comment box or the `:` line.
    pub fn update_input(&mut self, action: InputAction) {
        self.message = None;
        match &mut self.mode {
            Mode::Command { input } => match action {
                InputAction::Submit => {
                    let command = input.text().trim().to_string();
                    self.mode = Mode::Normal;
                    self.run_command(&command);
                }
                // As in vim, backspacing past the `:` leaves the command line.
                InputAction::Cancel => self.mode = Mode::Normal,
                InputAction::Backspace if input.text().is_empty() => self.mode = Mode::Normal,
                InputAction::Newline | InputAction::OpenEditor => {}
                other => input.apply(other),
            },
            Mode::Comment { input, .. } => match action {
                InputAction::Submit => self.submit_comment(),
                InputAction::Cancel => self.mode = Mode::Normal,
                InputAction::OpenEditor => {
                    self.effect = Some(Effect::OpenEditor { initial: input.text().to_string() })
                }
                other => input.apply(other),
            },
            Mode::Normal | Mode::Visual { .. } => {}
        }
    }

    /// Takes the text back from `$EDITOR`; `None` means the edit was aborted
    /// and the comment box stays open as it was.
    pub fn finish_editor(&mut self, text: Option<String>) {
        if let Mode::Comment { input, .. } = &mut self.mode {
            match text {
                Some(text) => {
                    *input = TextInput::new(text);
                    self.submit_comment();
                }
                None => self.message = Some("Editor closed without saving".to_string()),
            }
        }
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

    /// Whether a comment on a hunk that's no longer in the diff.
    pub fn is_outdated(&self, comment: &Comment) -> bool {
        !self.diff.files.iter().flat_map(|f| &f.hunks).any(|h| h.id == comment.hunk)
    }

    /// The selected `(file, hunk, start, end)` lines in visual mode. The
    /// selection never leaves the hunk it started in.
    pub fn selection(&self) -> Option<(usize, usize, usize, usize)> {
        let Mode::Visual { anchor } = self.mode else { return None };
        let (file, hunk) = match self.rows.get(anchor)? {
            Row::Hunk { file, hunk } | Row::Line { file, hunk, .. } => (*file, *hunk),
            _ => return None,
        };
        let (lo, hi) = (anchor.min(self.cursor), anchor.max(self.cursor));
        let lines: Vec<usize> = self.rows[lo..=hi]
            .iter()
            .filter_map(|row| match *row {
                Row::Line { hunk: h, line, .. } if h == hunk => Some(line),
                _ => None,
            })
            .collect();
        match (lines.first(), lines.last()) {
            (Some(&start), Some(&end)) => Some((file, hunk, start, end)),
            // Only the hunk header is selected: take the whole hunk.
            _ => Some((file, hunk, 0, self.hunk(file, hunk).lines.len().saturating_sub(1))),
        }
    }

    /// Whether a comment on a hunk of `file` is hidden by its fold.
    fn has_comments_in_hunks(&self, file: usize) -> bool {
        let hunks = &self.diff.files[file].hunks;
        self.review.comments().iter().any(|c| hunks.iter().any(|h| h.id == c.hunk))
    }

    fn comment_target(&self) -> Option<CommentTarget> {
        if let Some((file, hunk, start, end)) = self.selection() {
            return Some(CommentTarget::New { file, hunk, start, end });
        }
        match self.current_row()? {
            Row::Line { file, hunk, line } => Some(CommentTarget::New { file, hunk, start: line, end: line }),
            Row::Hunk { file, hunk } => {
                let end = self.hunk(file, hunk).lines.len().saturating_sub(1);
                Some(CommentTarget::New { file, hunk, start: 0, end })
            }
            Row::Comment { id, .. } => Some(CommentTarget::Edit { id }),
            Row::File { .. } => None,
        }
    }

    fn submit_comment(&mut self) {
        let Mode::Comment { target, input } = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return;
        };
        let text = input.text().trim().to_string();
        match target {
            CommentTarget::New { .. } if text.is_empty() => {}
            CommentTarget::New { file, hunk, start, end } => {
                let comment = self.new_comment(file, hunk, start, end, text);
                self.review.add_comment(comment);
            }
            CommentTarget::Edit { id } if text.is_empty() => {
                self.review.delete_comment(id);
                self.message = Some("Comment deleted".to_string());
            }
            CommentTarget::Edit { id } => self.review.set_comment_text(id, text),
        }
        self.rebuild_rows();
    }

    fn new_comment(&self, file: usize, hunk: usize, start: usize, end: usize, text: String) -> Comment {
        let h = self.hunk(file, hunk);
        let lines = &h.lines[start..=end];
        let span = |numbers: Vec<u32>| Some((*numbers.iter().min()?, *numbers.iter().max()?));
        Comment {
            id: 0,
            hunk: h.id.clone(),
            path: self.diff.files[file].path().to_string(),
            start,
            end,
            old_lines: span(lines.iter().filter_map(|l| l.old_no).collect()),
            new_lines: span(lines.iter().filter_map(|l| l.new_no).collect()),
            code: lines
                .iter()
                .map(|l| {
                    let sign = match l.kind {
                        LineKind::Added => "+",
                        LineKind::Removed => "-",
                        LineKind::Context | LineKind::NoNewline => " ",
                    };
                    format!("{sign}{}", l.content)
                })
                .collect(),
            text,
        }
    }

    fn run_command(&mut self, command: &str) {
        let mut words = command.split_whitespace();
        let Some(name) = words.next() else { return };
        let args: Vec<&str> = words.collect();
        match name {
            "q" | "q!" | "quit" | "qa" | "wq" | "x" => self.should_quit = true,
            "w" | "write" => {
                self.review.mark_dirty();
                self.message = Some("Saved".to_string());
            }
            "export" => {
                let format = match args.first() {
                    None => Some(Format::Markdown),
                    Some(arg) => Format::parse(arg),
                };
                match (format, args.get(2)) {
                    (Some(format), None) => {
                        let path = args.get(1).map(PathBuf::from);
                        self.effect = Some(Effect::Export { format, path });
                    }
                    _ => self.message = Some("error: usage: :export [md|json] [path]".to_string()),
                }
            }
            _ if name.parse::<u32>().is_ok() => self.go_to_line(name.parse().unwrap_or(0)),
            _ => self.message = Some(format!("error: not a command: {name}")),
        }
    }

    /// `:<n>`: the first row at or after line `n` of the new file.
    fn go_to_line(&mut self, n: u32) {
        let target = self.rows.iter().position(|row| match *row {
            Row::Line { file, hunk, line } => self.line(file, hunk, line).new_no.is_some_and(|no| no >= n),
            _ => false,
        });
        match target {
            Some(idx) => self.cursor = idx,
            None => self.message = Some(format!("Line {n} is past the last change in this file")),
        }
    }

    /// Rows of `file` as shown: hunks unless folded, comments under the
    /// lines they end on, and comments whose hunk is gone right under the header.
    fn file_rows(&self, file: usize) -> Vec<Row> {
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

    /// Every `(file, row)` a cross-file jump can land on, in diff order:
    /// rows matching `row_stop`, and headers of folded files where
    /// `folded_stop` says something is hidden.
    fn stops(&self, row_stop: impl Fn(&Row) -> bool, folded_stop: impl Fn(usize) -> bool) -> Vec<(usize, usize)> {
        let mut stops = Vec::new();
        for file in 0..self.diff.files.len() {
            if self.collapsed[file] && folded_stop(file) {
                stops.push((file, 0));
            }
            let rows = self.file_rows(file).into_iter().enumerate();
            stops.extend(rows.filter(|(_, row)| row_stop(row)).map(|(idx, _)| (file, idx)));
        }
        stops
    }

    /// Jumps `n` stops forward or back, switching files if needed. Returns
    /// false when there's no stop in that direction.
    fn jump_to_stop(&mut self, stops: &[(usize, usize)], n: usize, forward: bool) -> bool {
        let here = (self.file, self.cursor);
        let target = if forward {
            stops.iter().filter(|&&s| s > here).take(n).last()
        } else {
            stops.iter().rev().filter(|&&s| s < here).take(n).last()
        };
        let Some(&(file, row)) = target else { return false };
        self.open_file(file, row);
        self.scroll = row;
        true
    }

    fn open_file(&mut self, file: usize, cursor: usize) {
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

    /// Re-flattens the current file after a change, keeping the cursor on the
    /// same row, or on the file header if that row is gone.
    fn rebuild_rows(&mut self) {
        let Some(file) = self.current_file() else { return };
        let current = self.current_row();
        self.rows = self.file_rows(file);
        let kept = current.and_then(|row| self.rows.iter().position(|r| *r == row));
        // A deleted comment hands the cursor to the row after it; a fold, to the header.
        let fallback = if self.collapsed[file] { 0 } else { self.cursor.min(self.rows.len() - 1) };
        self.cursor = kept.unwrap_or(fallback);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{FileDiff, FileStatus};

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

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.update_input(InputAction::Insert(c));
        }
    }

    fn command(app: &mut App, text: &str) {
        app.update(Action::EnterCommand);
        type_text(app, text);
        app.update_input(InputAction::Submit);
    }

    fn comment(app: &mut App, text: &str) {
        app.update(Action::StartComment);
        type_text(app, text);
        app.update_input(InputAction::Submit);
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
        for action in [
            Action::Down(1),
            Action::NextFile(1),
            Action::NextUnreviewed(1),
            Action::ToggleFileReviewed,
            Action::StartComment,
            Action::NextComment(1),
        ] {
            app.update(action);
        }
        assert_eq!(app.current_file(), None);
    }

    #[test]
    fn comments_on_a_line_show_under_it() {
        let mut app = sample_app();
        app.update(Action::Down(3));
        comment(&mut app, "why?");
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.cursor, 3);

        let c = &app.review.comments()[0];
        assert_eq!((c.start, c.end, c.new_lines), (1, 1, Some((2, 2))));
        assert_eq!(c.code, ["+a.rs 0 1"]);
        assert_eq!(app.rows[4], Row::Comment { file: 0, id: c.id, line: 0 });
        assert_eq!(app.rows.len(), 9);
    }

    #[test]
    fn multi_line_comments_take_a_row_per_line() {
        let mut app = sample_app();
        app.update(Action::Down(2));
        app.update(Action::StartComment);
        type_text(&mut app, "one");
        app.update_input(InputAction::Newline);
        type_text(&mut app, "two");
        app.update_input(InputAction::Submit);
        assert!(matches!(app.rows[3], Row::Comment { line: 0, .. }));
        assert!(matches!(app.rows[4], Row::Comment { line: 1, .. }));
    }

    #[test]
    fn visual_selection_stays_in_its_hunk() {
        let mut app = sample_app();
        app.update(Action::Down(3));
        app.update(Action::StartVisual);
        app.update(Action::Down(10));
        assert_eq!(app.selection(), Some((0, 0, 1, 2)));
        app.update(Action::Up(10));
        assert_eq!(app.selection(), Some((0, 0, 0, 1)));

        comment(&mut app, "range");
        let c = &app.review.comments()[0];
        assert_eq!((c.start, c.end), (0, 1));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn hunk_header_comments_cover_the_whole_hunk() {
        let mut app = sample_app();
        app.update(Action::NextHunk(1));
        comment(&mut app, "whole hunk");
        let c = &app.review.comments()[0];
        assert_eq!((c.start, c.end, c.new_lines), (0, 2, Some((1, 3))));
    }

    #[test]
    fn file_headers_cannot_be_commented() {
        let mut app = sample_app();
        app.update(Action::StartComment);
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.message.is_some());
        app.update(Action::StartVisual);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn editing_and_deleting_comments() {
        let mut app = sample_app();
        app.update(Action::Down(2));
        comment(&mut app, "first");
        app.update(Action::Down(1));
        // `c` on a comment edits it, starting from its text.
        app.update(Action::StartComment);
        type_text(&mut app, " draft");
        app.update_input(InputAction::Submit);
        assert_eq!(app.review.comments()[0].text, "first draft");

        app.update(Action::DeleteComment);
        assert!(app.review.comments().is_empty());
        assert_eq!(app.rows.len(), 8);

        // Clearing a comment's text deletes it too.
        comment(&mut app, "again");
        app.update(Action::Down(1));
        app.update(Action::StartComment);
        app.update_input(InputAction::ClearToStart);
        app.update_input(InputAction::Submit);
        assert!(app.review.comments().is_empty());
    }

    #[test]
    fn cancelled_and_empty_comments_are_dropped() {
        let mut app = sample_app();
        app.update(Action::Down(2));
        app.update(Action::StartComment);
        type_text(&mut app, "nope");
        app.update_input(InputAction::Cancel);
        comment(&mut app, "   ");
        assert!(app.review.comments().is_empty());
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn comment_jumps_cross_files() {
        let mut app = sample_app();
        app.update(Action::NextFile(1));
        app.update(Action::Down(2));
        comment(&mut app, "in b");
        app.update(Action::PrevFile(1));
        app.update(Action::Down(6));
        comment(&mut app, "in a");

        app.update(Action::Top);
        app.update(Action::NextComment(1));
        assert_eq!(position(&app), (0, 7));
        app.update(Action::NextComment(1));
        assert_eq!(position(&app), (1, 3));
        app.update(Action::NextComment(1));
        assert!(app.message.is_some());
        app.update(Action::PrevComment(1));
        assert_eq!(position(&app), (0, 7));
    }

    #[test]
    fn outdated_comments_stay_visible_under_the_file_header() {
        let mut app = sample_app();
        app.update(Action::Down(2));
        comment(&mut app, "old");
        // Same file, but its first hunk changed.
        let app = App::new(
            String::new(),
            Diff::new(vec![file("a.rs", &[4]), file("b.rs", &[4])]),
            app.review,
        );
        let c = &app.review.comments()[0];
        assert!(app.is_outdated(c));
        assert_eq!(app.rows[1], Row::Comment { file: 0, id: c.id, line: 0 });
    }

    #[test]
    fn editor_round_trip() {
        let mut app = sample_app();
        app.update(Action::Down(2));
        app.update(Action::CommentInEditor);
        assert_eq!(app.effect.take(), Some(Effect::OpenEditor { initial: String::new() }));
        app.finish_editor(None);
        assert!(app.is_typing());
        app.finish_editor(Some("from vim\n".into()));
        assert_eq!(app.review.comments()[0].text, "from vim");
        assert!(!app.is_typing());
    }

    #[test]
    fn commands() {
        let mut app = sample_app();
        command(&mut app, "export json out.json");
        assert_eq!(
            app.effect.take(),
            Some(Effect::Export { format: Format::Json, path: Some("out.json".into()) })
        );
        command(&mut app, "export");
        assert_eq!(app.effect.take(), Some(Effect::Export { format: Format::Markdown, path: None }));
        command(&mut app, "export pdf");
        assert!(app.message.as_deref().unwrap().starts_with("error:"));

        command(&mut app, "3");
        assert_eq!(app.current_row(), Some(Row::Line { file: 0, hunk: 0, line: 2 }));
        command(&mut app, "nope");
        assert_eq!(app.message.as_deref(), Some("error: not a command: nope"));
        command(&mut app, "w");
        assert!(app.review.take_dirty());
        command(&mut app, "q");
        assert!(app.should_quit);
    }

    #[test]
    fn backspace_on_empty_command_line_leaves_it() {
        let mut app = sample_app();
        app.update(Action::EnterCommand);
        type_text(&mut app, "q");
        app.update_input(InputAction::Backspace);
        assert!(app.is_typing());
        app.update_input(InputAction::Backspace);
        assert_eq!(app.mode, Mode::Normal);
        assert!(!app.should_quit);
    }
}
