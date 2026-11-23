//! The list of every comment (`gc` or `:comments`), to jump to one or
//! dismiss it, and the prompt after `Y` offering to dismiss what was copied.

use super::{Action, App, Row, View};
use crate::review::Comment;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommentsMenu {
    /// Index into `listed_comments()` of the entry under the cursor.
    pub selected: usize,
    /// The comment a first `dd` marked, which a second one dismisses. Any
    /// other key takes the mark away.
    pub armed: Option<u64>,
}

/// Asked right after `Y`: whether to dismiss the comments just copied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DismissCopied {
    pub ids: Vec<u64>,
}

impl App {
    /// Every comment, in file order then line order, as `Y` hands them over.
    pub fn listed_comments(&self) -> Vec<&Comment> {
        let mut comments: Vec<&Comment> = self.review.comments().iter().collect();
        comments.sort_by_key(|c| (c.path.clone(), c.new_lines.or(c.old_lines), c.id));
        comments
    }

    pub(super) fn open_comments_menu(&mut self) {
        let comments = self.listed_comments();
        if comments.is_empty() {
            self.message = Some("No comments yet".to_string());
            return;
        }
        // Start on the comment under the cursor, if there is one.
        let selected = match self.current_row() {
            Some(Row::Comment { id, .. }) => comments.iter().position(|c| c.id == id).unwrap_or(0),
            _ => 0,
        };
        self.comments_menu = Some(CommentsMenu {
            selected,
            armed: None,
        });
    }

    /// While the list is open, motions move through it, Enter goes to the
    /// comment, `dd` marks it and a second `dd` dismisses it, and `Y` copies
    /// them all; anything that
    /// closes things closes it.
    pub(super) fn update_comments_menu(&mut self, action: Action) {
        let Some(menu) = self.comments_menu else {
            return;
        };
        let last = self.review.comments().len().saturating_sub(1);
        let selected = match action {
            Action::Down(n) => menu.selected.saturating_add(n).min(last),
            Action::Up(n) => menu.selected.saturating_sub(n),
            Action::HalfPageDown => menu.selected.saturating_add(5).min(last),
            Action::HalfPageUp => menu.selected.saturating_sub(5),
            Action::Top => 0,
            Action::Bottom => last,
            Action::Select => {
                let id = self.listed_comments().get(menu.selected).map(|c| c.id);
                self.comments_menu = None;
                if let Some(id) = id {
                    self.go_to_comment(id);
                }
                return;
            }
            Action::DeleteComment => {
                let id = self.listed_comments().get(menu.selected).map(|c| c.id);
                if id.is_some() && menu.armed != id {
                    self.comments_menu = Some(CommentsMenu { armed: id, ..menu });
                    return;
                }
                if let Some(id) = id {
                    self.review.delete_comment(id);
                    self.rebuild_rows();
                    self.message = Some("Comment dismissed".to_string());
                }
                if self.review.comments().is_empty() {
                    self.comments_menu = None;
                    return;
                }
                menu.selected.min(self.review.comments().len() - 1)
            }
            Action::CopyForAgent => {
                self.copy_for_agent();
                menu.selected
            }
            Action::ToggleHelp => {
                self.help_scroll = Some(0);
                menu.selected
            }
            Action::ListComments | Action::Cancel | Action::Quit => {
                self.comments_menu = None;
                return;
            }
            _ => menu.selected,
        };
        self.comments_menu = Some(CommentsMenu {
            selected,
            armed: None,
        });
    }

    /// Answers the prompt after `Y`: dismisses the copied comments, or keeps them.
    pub fn answer_dismiss_copied(&mut self, dismiss: bool) {
        self.message = None;
        let Some(prompt) = self.dismiss_prompt.take() else {
            return;
        };
        if !dismiss {
            return;
        }
        let before = self.review.comments().len();
        for id in &prompt.ids {
            self.review.delete_comment(*id);
        }
        let n = before - self.review.comments().len();
        self.rebuild_rows();
        if self.review.comments().is_empty() {
            self.comments_menu = None;
        } else if let Some(menu) = &mut self.comments_menu {
            menu.selected = menu.selected.min(self.review.comments().len() - 1);
            menu.armed = None;
        }
        self.message = Some(format!(
            "Dismissed {n} comment{}",
            if n == 1 { "" } else { "s" }
        ));
    }

    /// Puts the cursor on comment `id`, opening its file's fold, and
    /// switching to the files if that's the only place it shows (an
    /// outdated comment sits under its file's header there).
    fn go_to_comment(&mut self, id: u64) {
        let Some(comment) = self.review.comment(id) else {
            return;
        };
        let (hunk, path) = (comment.hunk.clone(), comment.path.clone());
        let file = self
            .diff
            .files
            .iter()
            .position(|f| f.hunks.iter().any(|h| h.id == hunk))
            .or_else(|| self.diff.files.iter().position(|f| f.path() == path));
        if let Some(file) = file {
            self.collapsed[file] = false;
            if let Some(h) = self.diff.files[file]
                .hunks
                .iter()
                .position(|h| h.id == hunk)
            {
                self.unfold_hunk(file, h);
            }
            self.rebuild_rows();
        }
        let mut found = self.jump_to_comment_row(id);
        if !found && self.view != View::Files && file.is_some() {
            self.show_view(View::Files);
            found = self.jump_to_comment_row(id);
        }
        if !found {
            self.message = Some("That comment's file has left the diff".to_string());
        }
    }

    fn jump_to_comment_row(&mut self, id: u64) -> bool {
        let stops = self.stops(
            |row| matches!(*row, Row::Comment { id: c, line: 0, .. } if c == id),
            |_| false,
        );
        let Some(&(page, row)) = stops.first() else {
            return false;
        };
        self.open_page(page, row);
        self.scroll = row;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::Effect;

    /// Comments on rows 2 and 3 of a.rs, then on b.rs.
    fn app_with_comments() -> App {
        let mut app = sample_app();
        app.update(Action::Down(2));
        comment(&mut app, "first");
        app.update(Action::NextFile(1));
        app.update(Action::Down(2));
        comment(&mut app, "third");
        app.update(Action::PrevFile(1));
        app.update(Action::Down(4));
        comment(&mut app, "second");
        app.update(Action::Top);
        app
    }

    fn texts(app: &App) -> Vec<&str> {
        app.listed_comments()
            .iter()
            .map(|c| c.text.as_str())
            .collect()
    }

    #[test]
    fn lists_comments_in_file_and_line_order() {
        let mut app = app_with_comments();
        assert_eq!(texts(&app), ["first", "second", "third"]);
        app.update(Action::ListComments);
        assert_eq!(
            app.comments_menu,
            Some(CommentsMenu {
                selected: 0,
                armed: None
            })
        );
        // Other keys wait while it's open.
        app.update(Action::NextFile(1));
        assert_eq!(app.page, 0);
        app.update(Action::Down(10));
        assert_eq!(
            app.comments_menu,
            Some(CommentsMenu {
                selected: 2,
                armed: None
            })
        );
        app.update(Action::Quit);
        assert_eq!(app.comments_menu, None);
        assert!(!app.should_quit);
    }

    #[test]
    fn enter_goes_to_the_comment() {
        let mut app = app_with_comments();
        app.update(Action::ListComments);
        app.update(Action::Bottom);
        app.update(Action::Select);
        assert_eq!(app.comments_menu, None);
        let id = app.listed_comments()[2].id;
        assert_eq!(app.page, 1);
        assert_eq!(
            app.current_row(),
            Some(Row::Comment {
                file: 1,
                id,
                line: 0
            })
        );
    }

    #[test]
    fn enter_opens_a_folded_file() {
        let mut app = app_with_comments();
        app.update(Action::CloseAllFolds);
        app.update(Action::ListComments);
        app.update(Action::Down(1));
        app.update(Action::Select);
        let id = app.listed_comments()[1].id;
        assert_eq!(
            app.current_row(),
            Some(Row::Comment {
                file: 0,
                id,
                line: 0
            })
        );
    }

    #[test]
    fn dd_twice_dismisses_and_closes_when_none_are_left() {
        let mut app = app_with_comments();
        app.update(Action::ListComments);
        app.update(Action::Bottom);
        // The first `dd` only marks it.
        app.update(Action::DeleteComment);
        let third = app.listed_comments()[2].id;
        assert_eq!(
            app.comments_menu,
            Some(CommentsMenu {
                selected: 2,
                armed: Some(third)
            })
        );
        assert_eq!(app.review.comments().len(), 3);
        app.update(Action::DeleteComment);
        assert_eq!(texts(&app), ["first", "second"]);
        assert_eq!(
            app.comments_menu,
            Some(CommentsMenu {
                selected: 1,
                armed: None
            })
        );
        for _ in 0..4 {
            app.update(Action::DeleteComment);
        }
        assert!(app.review.comments().is_empty());
        assert_eq!(app.comments_menu, None);
        app.update(Action::ListComments);
        assert_eq!(app.message.as_deref(), Some("No comments yet"));
    }

    #[test]
    fn moving_takes_the_mark_away() {
        let mut app = app_with_comments();
        app.update(Action::ListComments);
        app.update(Action::DeleteComment);
        app.update(Action::Down(1));
        assert_eq!(
            app.comments_menu,
            Some(CommentsMenu {
                selected: 1,
                armed: None
            })
        );
        // A fresh `dd` here marks this one rather than dismissing it.
        app.update(Action::DeleteComment);
        assert_eq!(app.review.comments().len(), 3);
        app.update(Action::Up(1));
        app.update(Action::DeleteComment);
        app.update(Action::DeleteComment);
        assert_eq!(texts(&app), ["second", "third"]);
    }

    #[test]
    fn copying_asks_to_dismiss_what_was_copied() {
        let mut app = app_with_comments();
        app.update(Action::CopyForAgent);
        assert!(matches!(app.effect.take(), Some(Effect::Copy { .. })));
        assert_eq!(app.dismiss_prompt.as_ref().map(|p| p.ids.len()), Some(3));
        app.answer_dismiss_copied(false);
        assert_eq!(app.dismiss_prompt, None);
        assert_eq!(app.review.comments().len(), 3);

        app.update(Action::CopyForAgent);
        app.answer_dismiss_copied(true);
        assert!(app.review.comments().is_empty());
        assert_eq!(app.message.as_deref(), Some("Dismissed 3 comments"));
        assert_eq!(app.rows.len(), 8);
    }

    #[test]
    fn copying_from_the_list_then_dismissing_closes_it() {
        let mut app = app_with_comments();
        app.update(Action::ListComments);
        app.update(Action::CopyForAgent);
        assert!(app.comments_menu.is_some());
        app.answer_dismiss_copied(true);
        assert_eq!(app.comments_menu, None);
    }
}
