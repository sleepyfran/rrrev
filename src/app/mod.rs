//! Review state and everything that changes it. Nothing here touches the
//! terminal or the disk: keys arrive as [`Action`]s, and work that needs the
//! outside world leaves as an [`Effect`] for the main loop.

mod action;
mod ask;
mod column;
mod command;
mod comments;
mod comments_menu;
mod context;
mod file_view;
mod marks;
mod mode;
mod navigation;
mod notes;
mod plan;
mod search;
mod theme;
mod verdict;
mod view;

#[cfg(test)]
mod test_support;

pub use action::{Action, Effect, VerdictAction};
pub use ask::{Answer, AskPanel};
pub use column::FindChar;
pub use comments_menu::{CommentsMenu, DismissCopied};
pub use context::{context_numbers, context_text, is_above};
pub use file_view::{FileView, Mark};
pub use marks::Progress;
pub use mode::{AskTarget, CommentTarget, Focus, Mode, View};
pub use notes::PlacedNote;
pub use verdict::LineStats;
pub use plan::{Job, StackPlace};
pub use search::find_matches;
pub use theme::ThemePicker;
pub use view::{NOTE_LABEL, Row};

use std::collections::HashMap;

use crate::diff::{Diff, DiffLine, Hunk};
use crate::input::InputAction;
use crate::review::ReviewState;
use crate::appearance::Appearance;
use crate::theme::{Palette, Theme};

pub struct App {
    pub title: String,
    pub diff: Diff,
    pub review: ReviewState,
    pub view: View,
    /// The organizer's plan placed on a plain diff; empty until there is one,
    /// and always in a stack, whose layers hold their own plans.
    pub plan: Vec<plan::PlanPage>,
    /// A stack's pages, layer by layer, each layer's split into its plan's
    /// groups once organized; empty for a plain diff.
    pub stack: Vec<plan::PlanPage>,
    /// What the diff pane shows: a file index, or an index into `pages()`.
    pub page: usize,
    /// Rows of the current page.
    pub rows: Vec<Row>,
    /// Per file: whether its hunks are folded away.
    pub collapsed: Vec<bool>,
    pub cursor: usize,
    /// The column the cursor wants on code rows; see `column.rs`.
    pub column: usize,
    /// Index of the first visible row.
    pub scroll: usize,
    pub viewport_height: usize,
    pub viewport_width: usize,
    /// Width of each line-number column, sized to the largest number in the diff.
    pub gutter_width: usize,
    pub focus: Focus,
    pub mode: Mode,
    /// Whether the user wants the docked sidebar shown.
    pub show_sidebar: bool,
    /// Too narrow to dock the sidebar; see `set_compact`.
    pub compact: bool,
    /// One-off feedback for the status line, cleared by the next action.
    pub message: Option<String>,
    pub effect: Option<Effect>,
    /// The organizer run in progress, if any.
    pub organizing: Option<Job>,
    /// The stack layer that run is planning; `None` for a plain diff.
    organizing_layer: Option<usize>,
    /// Id for the next organizer run.
    next_job: u64,
    /// The last `/` query, kept for `n` / `N`.
    pub search: Option<String>,
    /// Whether matches of `search` are highlighted; `Esc` turns it off.
    pub highlight_search: bool,
    /// First line shown of the `?` overlay, while it's open.
    pub help_scroll: Option<usize>,
    /// Whether removed lines are shown; `-` hides them, leaving the file as
    /// it is now.
    pub show_removals: bool,
    /// Extra lines of the file shown around hunks, by `(file, hunk)`.
    pub expanded: HashMap<(usize, usize), context::Expansion>,
    /// The AI's notes from the plan, placed on the diff.
    pub notes: Vec<PlacedNote>,
    /// Questions asked about the code, and their answers.
    pub ask: AskPanel,
    /// Whether the verdict page is open.
    pub show_verdict: bool,
    pub theme: &'static Theme,
    /// The OS's dark or light mode, which picks the theme's variant.
    pub appearance: Appearance,
    /// The colours of the theme's variant for `appearance`, kept alongside them.
    pub palette: Palette,
    pub theme_picker: Option<ThemePicker>,
    /// The list of every comment, while it's open.
    pub comments_menu: Option<CommentsMenu>,
    /// The question after `Y`, while it waits for an answer.
    pub dismiss_prompt: Option<DismissCopied>,
    /// The whole file the cursor was on, while it's open over the diff.
    pub file_view: Option<FileView>,
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
            view: View::Files,
            plan: Vec::new(),
            stack: Vec::new(),
            page: 0,
            rows: Vec::new(),
            collapsed: Vec::new(),
            cursor: 0,
            column: 0,
            scroll: 0,
            viewport_height: 0,
            viewport_width: 0,
            gutter_width: max_line_no.to_string().len(),
            focus: Focus::Diff,
            mode: Mode::Normal,
            show_sidebar: true,
            compact: false,
            message: None,
            effect: None,
            organizing: None,
            organizing_layer: None,
            next_job: 0,
            search: None,
            highlight_search: false,
            help_scroll: None,
            show_removals: true,
            expanded: HashMap::new(),
            notes: Vec::new(),
            ask: AskPanel::default(),
            show_verdict: false,
            theme: Theme::terminal(),
            appearance: Appearance::Dark,
            palette: Theme::terminal().palette(Appearance::Dark),
            theme_picker: None,
            comments_menu: None,
            dismiss_prompt: None,
            file_view: None,
            should_quit: false,
        };
        // Generated files and files already reviewed start out of the way.
        app.collapsed = (0..app.diff.files.len())
            .map(|f| app.diff.files[f].looks_generated() || app.file_progress(f).is_complete())
            .collect();
        // A stack follows its layers, each in its plan's groups once
        // organized; a review that has a plan picks up in it.
        let followed = app.follow_plan();
        app.place_plan();
        if !app.is_stack() && app.plan.is_empty() && app.review.plan().is_some() {
            app.message = Some("Saved plan is for another diff: :organize to replan".to_string());
        } else {
            app.message = app.plan_status(followed);
        }
        if !app.stack.is_empty() {
            app.view = View::Stack;
        } else if !app.plan.is_empty() {
            app.view = View::Plan;
        }
        app.place_notes();
        app.open_page(0, 0);
        app
    }

    /// True while keys should go to a text field rather than the keymap.
    pub fn is_typing(&self) -> bool {
        matches!(
            self.mode,
            Mode::Comment { .. }
                | Mode::Command { .. }
                | Mode::Search { .. }
                | Mode::Ask { .. }
                | Mode::VerdictMessage { .. }
        )
    }

    pub fn update(&mut self, action: Action) {
        self.message = None;
        if self.help_scroll.is_some() {
            self.update_help(action);
            return;
        }
        if self.dismiss_prompt.is_some() {
            // Answered through `answer_dismiss_copied`; nothing else gets through.
            return;
        }
        if self.file_view.is_some() {
            self.update_file_view(action);
            return;
        }
        if self.theme_picker.is_some() {
            self.update_theme_picker(action);
            return;
        }
        if self.comments_menu.is_some() {
            self.update_comments_menu(action);
            self.scroll_into_view();
            return;
        }
        if self.show_verdict {
            // Only closing it, or the help, gets through.
            match action {
                Action::ToggleVerdict | Action::Cancel | Action::Quit => self.show_verdict = false,
                Action::ToggleHelp => self.help_scroll = Some(0),
                _ => {}
            }
            return;
        }
        let action = match self.focus {
            Focus::Diff => action,
            Focus::Sidebar if action.is_horizontal() => return,
            Focus::Sidebar => navigation::sidebar_action(action, self.compact),
        };
        match action {
            Action::Down(n) => self.move_by(n as isize),
            Action::Up(n) => self.move_by(-(n as isize)),
            action @ (Action::Left(_)
            | Action::Right(_)
            | Action::WordForward { .. }
            | Action::WordBackward { .. }
            | Action::WordEnd { .. }
            | Action::LineStart
            | Action::FirstNonBlank
            | Action::LineEnd
            | Action::FindChar { .. }) => self.move_along(action),
            Action::HalfPageDown => self.move_by(self.half_page()),
            Action::HalfPageUp => self.move_by(-self.half_page()),
            Action::Top => self.cursor = 0,
            Action::Bottom => self.cursor = self.rows.len().saturating_sub(1),
            Action::NextHunk(n) => self.next_hunk(n),
            Action::PrevHunk(n) => self.prev_hunk(n),
            Action::NextFile(n) => self.next_page(n),
            Action::PrevFile(n) => self.prev_page(n),
            Action::NextUnreviewed(n) => self.jump_unreviewed(n, true),
            Action::PrevUnreviewed(n) => self.jump_unreviewed(n, false),
            Action::NextComment(n) => self.jump_comment(n, true),
            Action::PrevComment(n) => self.jump_comment(n, false),
            Action::ScrollCursorTop => self.scroll = self.cursor,
            Action::ScrollCursorCenter => self.scroll_with_above(self.viewport_height / 2),
            Action::ScrollCursorBottom => {
                let cursor = self.current_row().map_or(1, |row| self.row_height(row));
                self.scroll_with_above(self.viewport_height.saturating_sub(cursor))
            }
            Action::ToggleFold => self.toggle_fold(),
            Action::OpenFold => self.set_fold(false),
            Action::CloseFold => self.set_fold(true),
            Action::OpenAllFolds => self.set_all_folds(false),
            Action::CloseAllFolds => self.set_all_folds(true),
            Action::ToggleReviewed => self.toggle_reviewed(),
            Action::TogglePageReviewed => self.toggle_page_reviewed(),
            Action::ShowView(view) => self.show_view(view),
            Action::NextLayer(n) => self.jump_layer(n, true),
            Action::PrevLayer(n) => self.jump_layer(n, false),
            Action::StartVisual => self.toggle_visual(),
            Action::StartComment => self.start_comment(false),
            Action::CommentInEditor => self.start_comment(true),
            Action::DeleteComment => self.delete_comment_under_cursor(),
            Action::EnterCommand => self.enter_command(),
            Action::StartSearch => self.start_search(),
            Action::SearchNext(n) => self.search_next(n, true),
            Action::SearchPrev(n) => self.search_next(n, false),
            Action::ToggleHelp => self.help_scroll = Some(0),
            Action::ToggleFocus => self.toggle_focus(),
            Action::FocusDiff => self.focus = Focus::Diff,
            Action::ToggleSidebar => self.toggle_sidebar(),
            Action::ExpandAbove(n) => self.expand(n, true),
            Action::ExpandBelow(n) => self.expand(n, false),
            Action::ExpandScope => self.expand_scope(),
            Action::ResetContext => self.reset_context(),
            Action::ToggleRemovals => self.toggle_removals(),
            Action::Ask => self.start_ask(),
            Action::ToggleAskPanel => self.toggle_ask_panel(),
            Action::PanelDown(n) => self.ask.scroll = self.ask.scroll.saturating_add(n),
            Action::PanelUp(n) => self.ask.scroll = self.ask.scroll.saturating_sub(n),
            Action::Yank => self.yank(),
            Action::CopyForAgent => self.copy_for_agent(),
            Action::ListComments => self.open_comments_menu(),
            Action::ToggleVerdict => self.show_verdict = true,
            Action::PickTheme => self.open_theme_picker(),
            Action::Select => match self.current_row() {
                Some(Row::File { .. }) => self.toggle_fold(),
                Some(Row::Line { .. } | Row::Context { .. }) => self.open_file_view(),
                _ => {}
            },
            // Nothing to go back to yet outside the file view.
            Action::JumpBack => {}
            Action::Cancel => {
                if self.focus == Focus::Sidebar {
                    self.focus = Focus::Diff;
                } else if self.mode == Mode::Normal {
                    // Esc puts things away one at a time: the search
                    // highlight, then the ask panel.
                    if self.highlighted_search().is_some() {
                        self.clear_search_highlight();
                    } else {
                        self.ask.open = false;
                    }
                }
                self.mode = Mode::Normal;
            }
            Action::Quit => self.should_quit = true,
        }
        self.scroll_into_view();
    }

    /// Handles a key typed into the comment box, the `:` line or the `/` line.
    pub fn update_input(&mut self, action: InputAction) {
        self.message = None;
        match self.mode {
            Mode::Command { .. } => self.command_input(action),
            Mode::Comment { .. } => self.comment_input(action),
            Mode::Search { .. } => self.search_input(action),
            Mode::Ask { .. } => self.ask_input(action),
            Mode::VerdictMessage { .. } => self.verdict_message_input(action),
            Mode::Normal | Mode::Visual { .. } => {}
        }
    }

    /// While `?` is open, motions scroll it and anything that closes things
    /// closes it; everything else waits. How far it can scroll depends on how
    /// the help wraps, so the UI caps the scroll when it draws.
    fn update_help(&mut self, action: Action) {
        let Some(scroll) = self.help_scroll else { return };
        let half = self.half_page().unsigned_abs();
        self.help_scroll = match action {
            Action::Down(n) => Some(scroll.saturating_add(n)),
            Action::Up(n) => Some(scroll.saturating_sub(n)),
            Action::HalfPageDown => Some(scroll.saturating_add(half)),
            Action::HalfPageUp => Some(scroll.saturating_sub(half)),
            Action::Top => Some(0),
            Action::Bottom => Some(usize::MAX),
            Action::ToggleHelp | Action::Cancel | Action::Quit => None,
            _ => Some(scroll),
        };
    }

    pub fn current_row(&self) -> Option<Row> {
        self.rows.get(self.cursor).copied()
    }

    /// The file under the cursor: the page itself in the files view, the
    /// cursor row's file in the plan view (none on a group header).
    pub fn current_file(&self) -> Option<usize> {
        match self.view {
            View::Files => (self.page < self.diff.files.len()).then_some(self.page),
            View::Stack | View::Plan => self.current_row().and_then(Row::file),
        }
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
    fn help_scrolls_and_closes_without_acting() {
        let mut app = sample_app();
        app.update(Action::ToggleHelp);
        assert_eq!(app.help_scroll, Some(0));
        app.update(Action::Down(3));
        app.update(Action::NextFile(1));
        assert_eq!(app.help_scroll, Some(3));
        assert_eq!(app.page, 0);
        app.update(Action::Bottom);
        assert_eq!(app.help_scroll, Some(usize::MAX));
        // `q` closes the help rather than quitting.
        app.update(Action::Quit);
        assert_eq!(app.help_scroll, None);
        assert!(!app.should_quit);
    }

    #[test]
    fn empty_diff_is_harmless() {
        let mut app = app_with(Vec::new());
        assert!(app.rows.is_empty());
        for action in [
            Action::Down(1),
            Action::NextFile(1),
            Action::NextUnreviewed(1),
            Action::TogglePageReviewed,
            Action::ShowView(View::Plan),
            Action::NextLayer(1),
            Action::StartComment,
            Action::NextComment(1),
        ] {
            app.update(action);
        }
        assert_eq!(app.current_file(), None);
    }
}
