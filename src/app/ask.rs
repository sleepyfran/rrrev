//! The ask panel: questions about the code under the cursor, answered by
//! the AI in the background while the review goes on.

use std::time::Instant;

use super::context::sign;
use super::{App, AskTarget, Effect, Job, Mode, Row};
use crate::ask::AskRequest;
use crate::input::{InputAction, TextInput};

/// Questions asked so far, the one being answered, and whether it's shown.
#[derive(Debug, Default)]
pub struct AskPanel {
    pub open: bool,
    pub entries: Vec<AskEntry>,
    /// The question being answered, if any.
    pub running: Option<Job>,
    /// Lines scrolled down from the top of the latest question.
    pub scroll: usize,
    next_job: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskEntry {
    pub job: u64,
    pub location: String,
    pub question: String,
    pub answer: Answer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Pending,
    Done(String),
    Failed(String),
    Cancelled,
}

impl App {
    /// `a`: opens the panel with a question box for the selection, line,
    /// hunk or note under the cursor.
    pub(super) fn start_ask(&mut self) {
        if self.ask.running.is_some() {
            self.ask.open = true;
            self.message = Some("Still answering the last question… (:ask cancel to stop)".to_string());
            return;
        }
        let Some(target) = self.ask_target() else {
            self.mode = Mode::Normal;
            self.message = Some("Move to a line or hunk to ask about it".to_string());
            return;
        };
        self.ask.open = true;
        self.mode = Mode::Ask { target, input: TextInput::default() };
    }

    /// `A`: shows or hides the panel.
    pub(super) fn toggle_ask_panel(&mut self) {
        if self.ask.entries.is_empty() && !self.ask.open {
            self.message = Some("Nothing asked yet: a asks about the line under the cursor".to_string());
            return;
        }
        self.ask.open = !self.ask.open;
    }

    pub(super) fn ask_input(&mut self, action: InputAction) {
        let Mode::Ask { input, .. } = &mut self.mode else { return };
        match action {
            InputAction::Submit => {
                let Mode::Ask { target, input } = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
                self.submit_question(target, input.text().trim().to_string());
            }
            InputAction::Cancel => self.mode = Mode::Normal,
            InputAction::OpenEditor => {}
            other => input.apply(other),
        }
    }

    fn submit_question(&mut self, target: AskTarget, question: String) {
        if question.is_empty() {
            return;
        }
        self.ask.next_job += 1;
        let job = Job { id: self.ask.next_job, started: Instant::now() };
        let history = self
            .ask
            .entries
            .iter()
            .filter_map(|e| match &e.answer {
                Answer::Done(answer) => Some((format!("({}) {}", e.location, e.question), answer.clone())),
                _ => None,
            })
            .collect();
        let request = AskRequest {
            title: self.title.clone(),
            location: target.location.clone(),
            scopes: target.scopes,
            code: target.code,
            hunk: target.hunk,
            question: question.clone(),
            history,
        };
        self.ask.entries.push(AskEntry { job: job.id, location: target.location, question, answer: Answer::Pending });
        self.ask.running = Some(job);
        self.ask.scroll = 0;
        self.effect = Some(Effect::Ask { job: job.id, request });
    }

    /// Takes the answer to question `job`. Late answers to cancelled
    /// questions are dropped.
    pub fn finish_ask(&mut self, job: u64, result: anyhow::Result<String>) {
        if self.ask.running.is_none_or(|running| running.id != job) {
            return;
        }
        self.ask.running = None;
        let Some(entry) = self.ask.entries.iter_mut().find(|e| e.job == job) else { return };
        entry.answer = match result {
            Ok(text) => Answer::Done(text),
            Err(e) => Answer::Failed(format!("{e:#}")),
        };
        if !self.ask.open {
            self.message = Some("The answer is in: A shows it".to_string());
        }
    }

    /// `:ask cancel|clear`.
    pub(super) fn ask_command(&mut self, args: &[&str]) {
        match args {
            ["cancel"] => match self.ask.running.take() {
                Some(job) => {
                    self.effect = Some(Effect::CancelAsk { job: job.id });
                    if let Some(entry) = self.ask.entries.iter_mut().find(|e| e.job == job.id) {
                        entry.answer = Answer::Cancelled;
                    }
                    self.message = Some("Question cancelled".to_string());
                }
                None => self.message = Some("Not answering anything".to_string()),
            },
            ["clear"] => {
                if let Some(job) = self.ask.running.take() {
                    self.effect = Some(Effect::CancelAsk { job: job.id });
                }
                self.ask.entries.clear();
                self.ask.open = false;
                self.message = Some("Questions cleared".to_string());
            }
            [] => self.start_ask(),
            _ => self.message = Some("error: usage: :ask [cancel|clear]".to_string()),
        }
    }

    /// What the cursor points at, for a question.
    fn ask_target(&self) -> Option<AskTarget> {
        let (file, hunk, start, end) = match self.selection() {
            Some(selection) => selection,
            None => match self.current_row()? {
                Row::Line { file, hunk, line } => (file, hunk, line, line),
                Row::Note { note, .. } => {
                    let n = self.notes.get(note)?;
                    (n.file, n.hunk, n.line, n.line)
                }
                Row::Hunk { file, hunk } => (file, hunk, 0, self.hunk(file, hunk).lines.len().checked_sub(1)?),
                Row::Plumbing { file, hunk, start } => (file, hunk, start, self.run_at(file, hunk, start)?.end - 1),
                Row::Context { file, hunk, no } => return Some(self.context_target(file, hunk, no)),
                Row::Group { .. } | Row::File { .. } | Row::Comment { .. } => return None,
            },
        };
        let h = self.hunk(file, hunk);
        let signed = |lines: &[crate::diff::DiffLine]| -> Vec<String> {
            lines.iter().map(|l| format!("{}{}", sign(l.kind), l.content)).collect()
        };
        let path = self.diff.files[file].path();
        let numbers: Vec<u32> = h.lines[start..=end].iter().filter_map(|l| l.new_no.or(l.old_no)).collect();
        let location = match (numbers.iter().min(), numbers.iter().max()) {
            (Some(a), Some(b)) if a != b => format!("{path}:{a}-{b}"),
            (Some(a), _) => format!("{path}:{a}"),
            _ => path.to_string(),
        };
        Some(AskTarget {
            location,
            scopes: self.diff.scopes(file, hunk, start).into_iter().map(|s| s.label).collect(),
            code: signed(&h.lines[start..=end]),
            hunk: signed(&h.lines),
        })
    }

    fn context_target(&self, file: usize, hunk: usize, no: u32) -> AskTarget {
        let path = self.diff.files[file].path();
        let text = super::context_text(&self.diff, file, no).unwrap_or("");
        let scopes = self.diff.context_source(file).map_or_else(Vec::new, |(_, side)| {
            self.diff.scopes_at(file, side, no).into_iter().map(|s| s.label).collect()
        });
        let h = self.hunk(file, hunk);
        AskTarget {
            location: format!("{path}:{no} (unchanged)"),
            scopes,
            code: vec![format!(" {text}")],
            hunk: h.lines.iter().map(|l| format!("{}{}", sign(l.kind), l.content)).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::Action;

    fn ask(app: &mut App, question: &str) -> (u64, AskRequest) {
        app.update(Action::Ask);
        type_text(app, question);
        app.update_input(InputAction::Submit);
        let Some(Effect::Ask { job, request }) = app.effect.take() else { panic!("no ask effect") };
        (job, request)
    }

    #[test]
    fn asks_about_the_line_under_the_cursor() {
        let mut app = sample_app();
        app.update(Action::Down(3));
        let (job, request) = ask(&mut app, "why?");
        assert!(app.ask.open);
        assert_eq!(request.location, "a.rs:2");
        assert_eq!(request.code, ["+a.rs 0 1"]);
        assert_eq!(request.hunk.len(), 3);
        assert_eq!(app.ask.entries[0].answer, Answer::Pending);

        // One question at a time.
        app.update(Action::Ask);
        assert!(app.message.as_deref().unwrap().starts_with("Still answering"));
        assert_eq!(app.mode, Mode::Normal);

        app.finish_ask(job, Ok("Because.".into()));
        assert_eq!(app.ask.entries[0].answer, Answer::Done("Because.".into()));
        // The next question carries the last answer along.
        let (_, request) = ask(&mut app, "and then?");
        assert_eq!(request.history, [("(a.rs:2) why?".to_string(), "Because.".to_string())]);
    }

    #[test]
    fn selections_and_hunks_ask_about_ranges() {
        let mut app = sample_app();
        app.update(Action::NextHunk(1));
        let (_, request) = ask(&mut app, "whole hunk?");
        assert_eq!(request.location, "a.rs:1-3");
        app.finish_ask(1, Err(anyhow::anyhow!("offline")));
        assert_eq!(app.ask.entries[0].answer, Answer::Failed("offline".into()));
    }

    #[test]
    fn cancelling_drops_the_late_answer() {
        let mut app = sample_app();
        app.update(Action::Down(2));
        let (job, _) = ask(&mut app, "q");
        command(&mut app, "ask cancel");
        assert_eq!(app.effect.take(), Some(Effect::CancelAsk { job }));
        app.finish_ask(job, Ok("late".into()));
        assert_eq!(app.ask.entries[0].answer, Answer::Cancelled);
        command(&mut app, "ask clear");
        assert!(app.ask.entries.is_empty() && !app.ask.open);
    }

    #[test]
    fn esc_and_a_put_the_panel_away() {
        let mut app = sample_app();
        app.update(Action::ToggleAskPanel);
        assert!(!app.ask.open);
        app.update(Action::Down(2));
        app.update(Action::Ask);
        app.update_input(InputAction::Cancel);
        assert!(app.ask.open);
        app.update(Action::Cancel);
        assert!(!app.ask.open);
        // Headers have nothing to ask about.
        app.update(Action::Top);
        app.update(Action::Ask);
        assert!(!app.is_typing());
    }
}
