//! Review state and everything that changes it. Nothing here touches the
//! terminal or the disk: keys arrive as [`Action`]s, and work that needs the
//! outside world leaves as an [`Effect`] for the main loop.

mod action;
mod command;
mod comments;
mod marks;
mod mode;
mod navigation;
mod view;

#[cfg(test)]
mod test_support;

pub use action::{Action, Effect};
pub use mode::{CommentTarget, Focus, Mode};
pub use view::Row;

use crate::diff::{Diff, DiffLine, Hunk};
use crate::input::InputAction;
use crate::review::ReviewState;

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
            Focus::Sidebar => navigation::sidebar_action(action),
        };
        match action {
            Action::Down(n) => self.move_by(n as isize),
            Action::Up(n) => self.move_by(-(n as isize)),
            Action::HalfPageDown => self.move_by(self.half_page()),
            Action::HalfPageUp => self.move_by(-self.half_page()),
            Action::Top => self.cursor = 0,
            Action::Bottom => self.cursor = self.rows.len().saturating_sub(1),
            Action::NextHunk(n) => self.next_hunk(n),
            Action::PrevHunk(n) => self.prev_hunk(n),
            Action::NextFile(n) => self.next_file(n),
            Action::PrevFile(n) => self.prev_file(n),
            Action::NextUnreviewed(n) => self.jump_unreviewed(n, true),
            Action::PrevUnreviewed(n) => self.jump_unreviewed(n, false),
            Action::NextComment(n) => self.jump_comment(n, true),
            Action::PrevComment(n) => self.jump_comment(n, false),
            Action::ScrollCursorTop => self.scroll = self.cursor,
            Action::ScrollCursorCenter => {
                self.scroll = self.cursor.saturating_sub(self.viewport_height / 2)
            }
            Action::ScrollCursorBottom => {
                self.scroll = (self.cursor + 1).saturating_sub(self.viewport_height)
            }
            Action::ToggleFold => self.toggle_fold(),
            Action::OpenFold => self.set_fold(false),
            Action::CloseFold => self.set_fold(true),
            Action::OpenAllFolds => self.set_all_folds(false),
            Action::CloseAllFolds => self.set_all_folds(true),
            Action::ToggleReviewed => self.toggle_reviewed(),
            Action::ToggleFileReviewed => self.toggle_file_reviewed(),
            Action::StartVisual => self.toggle_visual(),
            Action::StartComment => self.start_comment(false),
            Action::CommentInEditor => self.start_comment(true),
            Action::DeleteComment => self.delete_comment_under_cursor(),
            Action::EnterCommand => self.enter_command(),
            Action::ToggleFocus => self.toggle_focus(),
            Action::FocusSidebar => self.focus_sidebar(),
            Action::FocusDiff => self.focus = Focus::Diff,
            Action::ToggleSidebar => self.toggle_sidebar(),
            Action::Select => {
                if let Some(Row::File { .. }) = self.current_row() {
                    self.toggle_fold();
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
        match self.mode {
            Mode::Command { .. } => self.command_input(action),
            Mode::Comment { .. } => self.comment_input(action),
            Mode::Normal | Mode::Visual { .. } => {}
        }
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
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

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
}
