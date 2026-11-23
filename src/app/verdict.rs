//! How far the review got and how it ends: which changed lines have been
//! on screen, the verdict page, and handing things over through the
//! clipboard.

use std::collections::BTreeMap;

use super::context::sign;
use super::{App, DismissCopied, Effect, Mode, Row, VerdictAction};
use crate::diff::LineKind;
use crate::export::{self, Format};
use crate::input::{InputAction, TextInput};
use crate::review::{Verdict, VerdictKind};

/// Changed lines by where the review is with them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LineStats {
    /// In hunks marked reviewed.
    pub good: usize,
    /// On screen at some point, in hunks not marked yet.
    pub read: usize,
    /// Plumbing not marked yet and not read, which needs no reading: whole
    /// hunks, and runs of it in hunks that also change code.
    pub plumbing: usize,
    /// Never on screen.
    pub unseen: usize,
}

impl LineStats {
    pub fn total(self) -> usize {
        self.good + self.read + self.plumbing + self.unseen
    }

    /// Share of changed lines looked at or needing no look, 0 to 100.
    pub fn seen_percent(self) -> usize {
        match self.total() {
            0 => 100,
            total => (self.good + self.read + self.plumbing) * 100 / total,
        }
    }
}

fn is_change(kind: LineKind) -> bool {
    matches!(kind, LineKind::Added | LineKind::Removed)
}

impl App {
    /// Notes the changed lines drawn this frame as seen. Called by the UI.
    pub fn mark_seen(&mut self, rows: &[Row]) {
        let mut by_hunk: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
        for row in rows {
            if let Row::Line { file, hunk, line } = *row
                && is_change(self.line(file, hunk, line).kind)
            {
                by_hunk.entry((file, hunk)).or_default().push(line);
            }
        }
        for ((file, hunk), lines) in by_hunk {
            let id = self.hunk(file, hunk).id.clone();
            self.review.mark_seen(&id, lines);
        }
    }

    pub fn line_stats(&self) -> LineStats {
        let mut stats = LineStats::default();
        for (f, file) in self.diff.files.iter().enumerate() {
            for (h, hunk) in file.hunks.iter().enumerate() {
                let reviewed = self.review.is_reviewed(&hunk.id);
                let plumbing = self.diff.is_plumbing(f, h);
                let runs = if reviewed || plumbing {
                    Vec::new()
                } else {
                    self.diff.plumbing_runs(f, h)
                };
                for (i, _) in hunk
                    .lines
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| is_change(l.kind))
                {
                    if reviewed {
                        stats.good += 1;
                    } else if plumbing
                        || (runs.iter().any(|r| r.contains(&i))
                            && !self.review.is_seen(&hunk.id, i))
                    {
                        stats.plumbing += 1;
                    } else if self.review.is_seen(&hunk.id, i) {
                        stats.read += 1;
                    } else {
                        stats.unseen += 1;
                    }
                }
            }
        }
        stats
    }

    /// A key pressed on the verdict page.
    pub fn verdict_action(&mut self, action: VerdictAction) {
        self.message = None;
        let message = self
            .review
            .verdict()
            .map_or(String::new(), |v| v.message.clone());
        let set = |kind| {
            Some(Verdict {
                kind,
                message: message.clone(),
            })
        };
        match action {
            VerdictAction::Approve => self.review.set_verdict(set(VerdictKind::Approve)),
            VerdictAction::RequestChanges => {
                self.review.set_verdict(set(VerdictKind::RequestChanges))
            }
            VerdictAction::Comment => self.review.set_verdict(set(VerdictKind::Comment)),
            VerdictAction::Message => {
                self.mode = Mode::VerdictMessage {
                    input: TextInput::new(message.clone()),
                };
            }
            VerdictAction::Clear => self.review.set_verdict(None),
            VerdictAction::CopyForAgent => self.copy_for_agent(),
            VerdictAction::Export => {
                self.effect = Some(Effect::Export {
                    format: Format::Markdown,
                    path: None,
                })
            }
            VerdictAction::Close => self.show_verdict = false,
        }
        if let Some(verdict) = self.review.verdict().filter(|_| {
            matches!(
                action,
                VerdictAction::Approve | VerdictAction::RequestChanges | VerdictAction::Comment
            )
        }) {
            self.message = Some(format!(
                "{} · e exports it with the comments",
                verdict.kind.label()
            ));
        }
    }

    /// Typing the message to the author. Saving it without a verdict yet
    /// makes it a comment.
    pub(super) fn verdict_message_input(&mut self, action: InputAction) {
        let Mode::VerdictMessage { input } = &mut self.mode else {
            return;
        };
        match action {
            InputAction::Submit => {
                let text = input.text().trim().to_string();
                self.mode = Mode::Normal;
                let kind = self
                    .review
                    .verdict()
                    .map_or(VerdictKind::Comment, |v| v.kind);
                self.review.set_verdict(Some(Verdict {
                    kind,
                    message: text,
                }));
            }
            InputAction::Cancel => self.mode = Mode::Normal,
            InputAction::OpenEditor => {}
            other => input.apply(other),
        }
    }

    /// `Y`: every comment as `path:line` and its text, ready to paste into
    /// a coding agent.
    pub(super) fn copy_for_agent(&mut self) {
        let export = export::collect(&self.title, &self.diff, &self.review);
        if export.comments.is_empty() {
            self.message = Some("No comments to hand over yet".to_string());
            return;
        }
        let n = export.comments.len();
        self.effect = Some(Effect::Copy {
            text: export::agent_brief(&export),
        });
        self.message = Some(format!(
            "Copied {n} comment{} for an agent",
            if n == 1 { "" } else { "s" }
        ));
        // Handed over, they may well be done with: offer to clear them.
        let ids = self.review.comments().iter().map(|c| c.id).collect();
        self.dismiss_prompt = Some(DismissCopied { ids });
    }

    /// `y`: the selected lines, the line or comment under the cursor.
    pub(super) fn yank(&mut self) {
        let (text, what) = if let Some((file, hunk, start, end)) = self.selection() {
            let lines = &self.hunk(file, hunk).lines[start..=end];
            // Removed lines aren't in the file any more, so they're left out
            // unless that's all there is.
            let kept: Vec<&str> = lines
                .iter()
                .filter(|l| l.kind != LineKind::Removed)
                .map(|l| l.content.as_str())
                .collect();
            let lines: Vec<&str> = if kept.is_empty() {
                lines.iter().map(|l| l.content.as_str()).collect()
            } else {
                kept
            };
            let n = lines.len();
            let text = lines.join("\n");
            self.mode = Mode::Normal;
            (text, format!("{n} line{}", if n == 1 { "" } else { "s" }))
        } else {
            match self.current_row() {
                Some(Row::Line { file, hunk, line }) => {
                    (self.line(file, hunk, line).content.clone(), "1 line".into())
                }
                Some(Row::Context { file, no, .. }) => (
                    super::context_text(&self.diff, file, no)
                        .unwrap_or("")
                        .to_string(),
                    "1 line".into(),
                ),
                Some(Row::Hunk { file, hunk }) => {
                    let lines = &self.hunk(file, hunk).lines;
                    let text = lines
                        .iter()
                        .map(|l| format!("{}{}", sign(l.kind), l.content))
                        .collect::<Vec<_>>();
                    (text.join("\n"), "the hunk".into())
                }
                Some(Row::Plumbing { file, hunk, start }) => {
                    let run = self.run_at(file, hunk, start).unwrap_or(start..start + 1);
                    let lines = &self.hunk(file, hunk).lines[run];
                    let text = lines
                        .iter()
                        .map(|l| format!("{}{}", sign(l.kind), l.content))
                        .collect::<Vec<_>>();
                    (text.join("\n"), format!("{} lines", lines.len()))
                }
                Some(Row::Comment { id, .. }) => match self.review.comment(id) {
                    Some(c) => (c.text.clone(), "the comment".into()),
                    None => return,
                },
                Some(Row::Note { note, .. }) => match self.notes.get(note) {
                    Some(n) => (n.text.clone(), "the note".into()),
                    None => return,
                },
                Some(Row::File { file }) => {
                    (self.diff.files[file].path().to_string(), "the path".into())
                }
                Some(Row::Group { .. }) | None => {
                    self.message = Some("Nothing to copy here".to_string());
                    return;
                }
            }
        };
        self.effect = Some(Effect::Copy { text });
        self.message = Some(format!("Copied {what}"));
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::Action;

    #[test]
    fn counts_lines_good_read_and_unseen() {
        let mut app = sample_app();
        assert_eq!(
            app.line_stats(),
            LineStats {
                good: 0,
                read: 0,
                plumbing: 0,
                unseen: 9
            }
        );
        let rows = app.rows[..4].to_vec();
        app.mark_seen(&rows);
        assert_eq!(
            app.line_stats(),
            LineStats {
                good: 0,
                read: 2,
                plumbing: 0,
                unseen: 7
            }
        );
        app.update(Action::Down(2));
        app.update(Action::ToggleReviewed);
        let stats = app.line_stats();
        assert_eq!(
            stats,
            LineStats {
                good: 3,
                read: 0,
                plumbing: 0,
                unseen: 6
            }
        );
        assert_eq!(stats.seen_percent(), 33);
    }

    #[test]
    fn verdict_page_sets_the_verdict_and_message() {
        let mut app = sample_app();
        app.update(Action::ToggleVerdict);
        assert!(app.show_verdict);
        // Other keys wait while it's open.
        app.update(Action::Down(1));
        assert_eq!(app.cursor, 0);

        app.verdict_action(VerdictAction::Message);
        type_text(&mut app, "Nice work");
        app.update_input(InputAction::Submit);
        assert_eq!(
            app.review.verdict(),
            Some(&Verdict {
                kind: VerdictKind::Comment,
                message: "Nice work".into()
            })
        );
        app.verdict_action(VerdictAction::Approve);
        assert_eq!(app.review.verdict().unwrap().kind, VerdictKind::Approve);
        assert_eq!(app.review.verdict().unwrap().message, "Nice work");
        app.verdict_action(VerdictAction::Clear);
        assert!(app.review.verdict().is_none());
        app.verdict_action(VerdictAction::Close);
        assert!(!app.show_verdict);
    }

    #[test]
    fn copies_comments_for_an_agent() {
        let mut app = sample_app();
        app.update(Action::CopyForAgent);
        assert_eq!(app.message.as_deref(), Some("No comments to hand over yet"));
        app.update(Action::Down(3));
        comment(&mut app, "why the lock?");
        app.update(Action::CopyForAgent);
        let Some(Effect::Copy { text }) = app.effect.take() else {
            panic!("no copy")
        };
        assert!(text.contains("- `a.rs:2`: why the lock?"));
    }

    #[test]
    fn yanks_lines_selections_and_hunks() {
        let mut app = sample_app();
        app.update(Action::Down(2));
        app.update(Action::Yank);
        assert_eq!(
            app.effect.take(),
            Some(Effect::Copy {
                text: "a.rs 0 0".into()
            })
        );
        app.update(Action::StartVisual);
        app.update(Action::Down(1));
        app.update(Action::Yank);
        assert_eq!(
            app.effect.take(),
            Some(Effect::Copy {
                text: "a.rs 0 0\na.rs 0 1".into()
            })
        );
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.message.as_deref(), Some("Copied 2 lines"));
        app.update(Action::Up(2));
        app.update(Action::Yank);
        assert_eq!(
            app.effect.take(),
            Some(Effect::Copy {
                text: "+a.rs 0 0\n+a.rs 0 1\n+a.rs 0 2".into()
            })
        );
    }
}
