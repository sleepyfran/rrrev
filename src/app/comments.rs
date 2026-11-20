//! Visual selection and the comment lifecycle: write, edit (inline or in
//! `$EDITOR`), delete.

use super::{App, CommentTarget, Effect, Mode, Row};
use crate::diff::LineKind;
use crate::input::{InputAction, TextInput};
use crate::review::Comment;

impl App {
    /// Whether a comment is on a hunk that's no longer in the diff.
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
                Row::Line { file: f, hunk: h, line } if (f, h) == (file, hunk) => Some(line),
                _ => None,
            })
            .collect();
        match (lines.first(), lines.last()) {
            (Some(&start), Some(&end)) => Some((file, hunk, start, end)),
            // Only the hunk header is selected: take the whole hunk.
            _ => Some((file, hunk, 0, self.hunk(file, hunk).lines.len().saturating_sub(1))),
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

    /// `V`: starts a selection on a hunk, or ends the current one.
    pub(super) fn toggle_visual(&mut self) {
        match (&self.mode, self.current_row()) {
            (Mode::Visual { .. }, _) => self.mode = Mode::Normal,
            (_, Some(Row::Hunk { .. } | Row::Line { .. })) => self.mode = Mode::Visual { anchor: self.cursor },
            _ => self.message = Some("Move to a hunk to select lines".to_string()),
        }
    }

    /// `c` / `C`: opens the comment box for the selection, line, hunk or
    /// existing comment under the cursor, optionally going straight to `$EDITOR`.
    pub(super) fn start_comment(&mut self, in_editor: bool) {
        if let Some(note) = self.current_note().filter(|_| !in_editor) {
            self.comment_with_note(note);
            return;
        }
        let Some(target) = self.comment_target() else {
            self.mode = Mode::Normal;
            self.message = Some("Move to a line or hunk to comment".to_string());
            return;
        };
        let text = match target {
            CommentTarget::Edit { id } => self.review.comment(id).map_or("", |c| &c.text),
            CommentTarget::New { .. } => "",
        };
        let input = TextInput::new(text);
        if in_editor {
            self.effect = Some(Effect::OpenEditor { initial: input.text().to_string() });
        }
        self.mode = Mode::Comment { target, input };
    }

    pub(super) fn delete_comment_under_cursor(&mut self) {
        match self.current_row() {
            Some(Row::Comment { id, .. }) => {
                self.review.delete_comment(id);
                self.rebuild_rows();
                self.message = Some("Comment deleted".to_string());
            }
            Some(Row::Note { note, .. }) => self.dismiss_note(note),
            _ => self.message = Some("No comment under the cursor".to_string()),
        }
    }

    pub(super) fn comment_input(&mut self, action: InputAction) {
        let Mode::Comment { input, .. } = &mut self.mode else { return };
        match action {
            InputAction::Submit => self.submit_comment(),
            InputAction::Cancel => self.mode = Mode::Normal,
            InputAction::OpenEditor => {
                self.effect = Some(Effect::OpenEditor { initial: input.text().to_string() })
            }
            other => input.apply(other),
        }
    }

    /// Whether a comment on a hunk of `file` is hidden by its fold.
    pub(super) fn has_comments_in_hunks(&self, file: usize) -> bool {
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
            Row::Note { note, .. } => {
                let n = self.notes.get(note)?;
                Some(CommentTarget::New { file: n.file, hunk: n.hunk, start: n.line, end: n.line })
            }
            Row::Group { .. } | Row::File { .. } | Row::Context { .. } => None,
        }
    }

    /// Saves the comment box. Empty text drops a new comment and deletes an
    /// existing one.
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
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::Action;
    use crate::diff::Diff;

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
}
