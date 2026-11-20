//! The AI's notes from the review plan, shown next to the code they're
//! about: turned into a comment with `c`, or dismissed with `dd`.

use std::collections::HashMap;

use super::{App, CommentTarget, Mode, Row};
use crate::diff::HunkId;
use crate::input::TextInput;
use crate::review::note_key;

/// An agent note placed on the current diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedNote {
    pub file: usize,
    pub hunk: usize,
    /// The hunk line it shows under.
    pub line: usize,
    pub text: String,
    /// Remembers a dismissal across runs.
    pub key: String,
}

impl App {
    /// Places the saved plan's notes on the diff, leaving out dismissed ones
    /// and those on hunks no longer in it.
    pub(super) fn place_notes(&mut self) {
        let positions: HashMap<&HunkId, (usize, usize)> = self
            .diff
            .files
            .iter()
            .enumerate()
            .flat_map(|(f, file)| file.hunks.iter().enumerate().map(move |(h, hunk)| (&hunk.id, (f, h))))
            .collect();
        let notes = self.review.plan().map_or(&[][..], |plan| plan.notes.as_slice());
        self.notes = notes
            .iter()
            .filter_map(|note| {
                let key = note_key(&note.hunk, &note.text);
                if self.review.is_note_dismissed(&key) {
                    return None;
                }
                let &(file, hunk) = positions.get(&note.hunk)?;
                let lines = &self.diff.files[file].hunks[hunk].lines;
                // On the line it names, by its new number or else its old one;
                // under the whole hunk otherwise.
                let line = note
                    .line
                    .and_then(|no| {
                        lines.iter().position(|l| l.new_no == Some(no)).or_else(|| lines.iter().position(|l| l.old_no == Some(no)))
                    })
                    .unwrap_or(lines.len().saturating_sub(1));
                let text = note.text.split_whitespace().collect::<Vec<_>>().join(" ");
                Some(PlacedNote { file, hunk, line, text, key })
            })
            .collect();
    }

    /// `dd` on a note: it won't show again for this diff.
    pub(super) fn dismiss_note(&mut self, note: usize) {
        let Some(placed) = self.notes.get(note) else { return };
        self.review.dismiss_note(placed.key.clone());
        self.place_notes();
        self.rebuild_rows();
        self.message = Some("Note dismissed".to_string());
    }

    /// `c` on a note: a comment on the line it's about, starting from its text.
    pub(super) fn comment_with_note(&mut self, note: usize) {
        let Some(placed) = self.notes.get(note) else { return };
        let target = CommentTarget::New { file: placed.file, hunk: placed.hunk, start: placed.line, end: placed.line };
        self.mode = Mode::Comment { target, input: TextInput::new(placed.text.clone()) };
    }

    /// The note under the cursor, if the cursor is on one.
    pub(super) fn current_note(&self) -> Option<usize> {
        match self.current_row()? {
            Row::Note { note, .. } => Some(note),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::{Action, Effect};
    use crate::input::InputAction;
    use crate::organizer::{AgentNote, Plan, PlanGroup};

    fn noted_app() -> App {
        let mut app = sample_app();
        command(&mut app, "organize");
        let Some(Effect::Organize { job, .. }) = app.effect.take() else { panic!() };
        let id = |f: usize, h: usize| app.diff.files[f].hunks[h].id.clone();
        let plan = Plan {
            groups: vec![PlanGroup { title: "All".into(), rationale: String::new(), hunks: vec![id(0, 0), id(0, 1)] }],
            notes: vec![
                AgentNote { hunk: id(0, 0), line: Some(2), text: "Check the\n bound.".into() },
                AgentNote { hunk: id(0, 1), line: None, text: "Easy to miss.".into() },
            ],
            ..Plan::default()
        };
        app.finish_organize(job, Ok(plan));
        app.update(Action::ShowView(crate::app::View::Files));
        app
    }

    #[test]
    fn notes_show_under_their_line_or_hunk() {
        let app = noted_app();
        // a.rs: header, hunk, line 0, line 1, note, line 2, hunk, line 0, line 1, note.
        assert_eq!(app.rows[4], Row::Note { file: 0, hunk: 0, note: 0 });
        assert_eq!(app.rows[9], Row::Note { file: 0, hunk: 1, note: 1 });
        assert_eq!(app.notes[0].text, "Check the bound.");
    }

    #[test]
    fn comment_jumps_stop_at_notes() {
        let mut app = noted_app();
        app.update(Action::NextComment(1));
        assert_eq!(app.cursor, 4);
        app.update(Action::NextComment(1));
        assert_eq!(app.cursor, 9);
    }

    #[test]
    fn a_note_becomes_a_comment() {
        let mut app = noted_app();
        app.update(Action::Down(4));
        app.update(Action::StartComment);
        assert!(matches!(&app.mode, Mode::Comment { input, .. } if input.text() == "Check the bound."));
        app.update_input(InputAction::Submit);
        let c = &app.review.comments()[0];
        assert_eq!((c.start, c.end, c.text.as_str()), (1, 1, "Check the bound."));
    }

    #[test]
    fn dismissed_notes_stay_dismissed() {
        let mut app = noted_app();
        app.update(Action::Down(4));
        app.update(Action::DeleteComment);
        assert_eq!(app.notes.len(), 1);
        assert!(!app.rows.contains(&Row::Note { file: 0, hunk: 0, note: 0 }));
        let reopened = App::new(String::new(), app.diff.clone(), app.review);
        assert_eq!(reopened.notes.len(), 1);
        assert_eq!(reopened.notes[0].text, "Easy to miss.");
    }
}
