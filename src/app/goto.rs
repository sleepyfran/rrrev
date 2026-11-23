//! `gd`, `gy` and `gi`: asking the language server where the symbol under
//! the cursor comes from, and going there: to its line in the diff when the
//! diff shows it, or to the file view otherwise. `C-o` comes back.

use super::column::Caret;
use super::file_view::{FileView, marks};
use super::{App, Effect, GoTo, Row, Spot};
use crate::diff::{LineKind, Side, Source};

/// Where a definition is, as the main loop found it on disk.
#[derive(Debug, Clone)]
pub struct Target {
    /// Relative to the working folder when it's inside it, else absolute.
    pub path: String,
    /// 0-based.
    pub line: usize,
    /// The byte in that line.
    pub column: usize,
    /// The file's text, for the file view.
    pub lines: Source,
}

/// Where the cursor was before a jump, for `C-o`.
#[derive(Debug, Clone)]
pub struct Jump {
    page: usize,
    cursor: usize,
    column: usize,
    scroll: usize,
    file_view: Option<FileView>,
}

/// Where the cursor is, to tell whether it moved while a server thought.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    page: usize,
    cursor: usize,
    column: usize,
    /// In the file view: its path, line and column.
    file: Option<(String, usize, usize)>,
}

impl App {
    pub fn place(&self) -> Place {
        Place {
            page: self.page,
            cursor: self.cursor,
            column: self.column,
            file: self
                .file_view
                .as_ref()
                .map(|v| (v.path.clone(), v.caret.line, v.caret.column)),
        }
    }

    /// Whether a server's answer to a question asked at `place` still
    /// applies: the cursor hasn't moved since, and nothing is being typed.
    /// Moving on is how a question is called off.
    pub fn still_at(&self, place: &Place) -> bool {
        !self.is_typing() && self.place() == *place
    }

    /// `gd` and its kin: asks the main loop to ask the server.
    pub(super) fn go_to(&mut self, kind: GoTo) {
        match self.cursor_spot() {
            Ok(spot) => self.effect = Some(Effect::GoTo { kind, spot }),
            Err(why) => self.message = Some(why.to_string()),
        }
    }

    /// The code under the cursor, as the language server sees it: only the
    /// new side, which is what's on disk.
    pub fn cursor_spot(&self) -> Result<Spot, &'static str> {
        const OLD: &str = "Removed lines aren't in the code the language server sees";
        if let Some(view) = &self.file_view {
            if view.side == Side::Old {
                return Err("This is the old version, which the language server doesn't see");
            }
            let text = view.lines.get(view.caret.line).cloned().unwrap_or_default();
            let column = view.caret.bytes(&view.lines[..]).map_or(0, |b| b.start);
            return Ok(Spot {
                path: view.path.clone(),
                line: view.caret.line as u32,
                column,
                text,
            });
        }
        let (file, no, text) = match self.current_row() {
            Some(Row::Line { file, hunk, line }) => {
                let l = self.line(file, hunk, line);
                match (l.kind, l.new_no) {
                    (LineKind::Removed, _) | (_, None) => return Err(OLD),
                    (_, Some(no)) => (file, no, l.content.clone()),
                }
            }
            Some(Row::Context { file, no, .. }) => match self.diff.context_source(file) {
                Some((source, Side::New)) => (
                    file,
                    no,
                    source.get(no as usize - 1).cloned().unwrap_or_default(),
                ),
                _ => return Err(OLD),
            },
            _ => return Err("Move to a line of code first"),
        };
        let path = self.diff.files[file].new_path.clone().ok_or(OLD)?;
        let column = self.cursor_bytes().map_or(0, |b| b.start);
        Ok(Spot {
            path,
            line: no - 1,
            column,
            text,
        })
    }

    /// The server's answer: goes to the first place it gave. `note` is said
    /// alongside, e.g. that the reviewed revision isn't checked out.
    pub fn finish_go_to(
        &mut self,
        kind: GoTo,
        targets: Result<Vec<Target>, String>,
        note: Option<&str>,
    ) {
        let targets = match targets {
            Ok(targets) => targets,
            Err(e) => {
                self.message = Some(format!("error: {e}"));
                return;
            }
        };
        let Some(target) = targets.first() else {
            self.message = Some(format!("No {} found", kind.noun()));
            return;
        };
        self.jumps.push(self.here());
        self.go_to_target(target);
        let mut message = match targets.len() {
            1 => None,
            n => Some(format!("1 of {n} places")),
        };
        if let Some(note) = note {
            message = Some(message.map_or_else(|| note.to_string(), |m| format!("{m} · {note}")));
        }
        self.message = message;
    }

    /// Where the diff shows a target's line, as a page and row.
    pub(super) fn shown_at(&self, target: &Target) -> Option<(usize, usize)> {
        let file = self
            .diff
            .files
            .iter()
            .position(|f| f.new_path.as_deref() == Some(target.path.as_str()))?;
        let no = target.line as u32 + 1;
        let shown = |row: &Row| match *row {
            Row::Line {
                file: f,
                hunk,
                line,
            } => {
                let l = self.line(f, hunk, line);
                f == file && l.kind != LineKind::Removed && l.new_no == Some(no)
            }
            Row::Context { file: f, no: n, .. } => {
                f == file && n == no && matches!(self.diff.context_source(f), Some((_, Side::New)))
            }
            _ => false,
        };
        self.stops(shown, |_| false).first().copied()
    }

    /// Its line in the diff when the diff shows it, else the file view.
    /// Already in the file view, it stays there: whoever is reading whole
    /// files wants the next one, not a hunk.
    pub(super) fn go_to_target(&mut self, target: &Target) {
        let column = target.lines.get(target.line).map_or(0, |l| {
            l.get(..target.column)
                .map_or(l.chars().count(), |before| before.chars().count())
        });
        if self.file_view.is_none()
            && let Some((page, row)) = self.shown_at(target)
        {
            self.open_page(page, row);
            self.column = column;
            self.scroll = row.saturating_sub(self.viewport_height / 2);
            return;
        }
        let file = self
            .diff
            .files
            .iter()
            .position(|f| f.new_path.as_deref() == Some(target.path.as_str()));
        let marks = match file {
            Some(file) => marks(&self.diff.files[file], Side::New, target.lines.len()),
            None => vec![None; target.lines.len()],
        };
        let mut view = FileView::new(
            target.path.clone(),
            Side::New,
            target.lines.clone(),
            file,
            marks,
        );
        view.caret = Caret {
            line: target.line.min(target.lines.len().saturating_sub(1)),
            column,
        };
        view.scroll = view.caret.line.saturating_sub(self.viewport_height / 2);
        self.file_view = Some(view);
    }

    pub(super) fn here(&self) -> Jump {
        Jump {
            page: self.page,
            cursor: self.cursor,
            column: self.column,
            scroll: self.scroll,
            file_view: self.file_view.clone(),
        }
    }

    /// `C-o`: back to where the last jump left from. With nothing to go
    /// back to, it closes the file view.
    pub(super) fn jump_back(&mut self) {
        let Some(jump) = self.jumps.pop() else {
            self.file_view = None;
            return;
        };
        self.open_page(jump.page, jump.cursor);
        (self.column, self.scroll) = (jump.column, jump.scroll);
        self.file_view = jump.file_view;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::test_support::*;
    use super::*;
    use crate::app::Action;

    fn target(path: &str, line: usize, column: usize, lines: &[&str]) -> Target {
        Target {
            path: path.into(),
            line,
            column,
            lines: Arc::new(lines.iter().map(|l| l.to_string()).collect()),
        }
    }

    #[test]
    fn asks_from_the_new_side_only() {
        let mut app = sample_app();
        app.cursor = (0..app.rows.len())
            .find(|&i| matches!(app.rows[i], Row::Line { file, hunk, line } if app.line(file, hunk, line).new_no.is_some()))
            .unwrap();
        app.update(Action::GoTo(GoTo::Definition));
        match app.effect.take() {
            Some(Effect::GoTo {
                kind: GoTo::Definition,
                spot,
            }) => {
                let row = app.current_row().unwrap();
                let Row::Line { file, hunk, line } = row else {
                    panic!("{row:?}")
                };
                let l = app.line(file, hunk, line);
                assert_eq!(spot.line + 1, l.new_no.unwrap());
                assert_eq!(spot.text, l.content);
            }
            other => panic!("{other:?}"),
        }
        // A removed line has nothing to ask about.
        let removed = (0..app.rows.len()).find(|&i| {
            matches!(app.rows[i], Row::Line { file, hunk, line } if app.line(file, hunk, line).kind == LineKind::Removed)
        });
        if let Some(i) = removed {
            app.cursor = i;
            app.update(Action::GoTo(GoTo::Definition));
            assert!(app.effect.is_none());
            assert!(app.message.as_deref().unwrap().contains("Removed lines"));
        }
    }

    #[test]
    fn goes_to_files_outside_the_diff_and_back() {
        let mut app = sample_app();
        app.update(Action::Down(2));
        let before = (app.page, app.cursor);
        let lines = ["mod a;", "", "pub fn helper() {}"];
        app.finish_go_to(
            GoTo::Definition,
            Ok(vec![target("src/other.rs", 2, 7, &lines)]),
            None,
        );
        let view = app.file_view.as_ref().expect("the file view");
        assert_eq!(
            (view.path.as_str(), view.line_no(), view.cursor_column()),
            ("src/other.rs", 3, 7)
        );
        assert_eq!(app.message, None);
        app.update(Action::JumpBack);
        assert!(app.file_view.is_none());
        assert_eq!((app.page, app.cursor), before);
    }

    #[test]
    fn goes_to_lines_the_diff_shows() {
        let mut app = sample_app();
        // The last line of the sample's first file, on the new side.
        let (file, no) = (0..app.rows.len())
            .rev()
            .find_map(|i| match app.rows[i] {
                Row::Line { file, hunk, line } => {
                    app.line(file, hunk, line).new_no.map(|n| (file, n))
                }
                _ => None,
            })
            .unwrap();
        let path = app.diff.files[file].new_path.clone().unwrap();
        let lines: Vec<String> = (0..no).map(|_| "    x".to_string()).collect();
        let t = Target {
            path,
            line: no as usize - 1,
            column: 4,
            lines: Arc::new(lines),
        };
        app.finish_go_to(GoTo::Definition, Ok(vec![t.clone(), t]), Some("may be off"));
        assert!(app.file_view.is_none());
        let Some(Row::Line { file, hunk, line }) = app.current_row() else {
            panic!()
        };
        assert_eq!(app.line(file, hunk, line).new_no, Some(no));
        assert_eq!(app.column, 4);
        assert_eq!(app.message.as_deref(), Some("1 of 2 places · may be off"));
    }

    #[test]
    fn stays_in_the_file_view_even_for_lines_the_diff_shows() {
        let mut app = sample_app();
        let (file, no) = (0..app.rows.len())
            .rev()
            .find_map(|i| match app.rows[i] {
                Row::Line { file, hunk, line } => {
                    app.line(file, hunk, line).new_no.map(|n| (file, n))
                }
                _ => None,
            })
            .unwrap();
        let path = app.diff.files[file].new_path.clone().unwrap();
        let lines: Vec<String> = (0..no).map(|_| "    x".to_string()).collect();
        let t = Target {
            path: path.clone(),
            line: no as usize - 1,
            column: 4,
            lines: Arc::new(lines),
        };
        // Open the file view somewhere else first, as if with Enter.
        let other = target("src/other.rs", 0, 0, &["fn a() {}"]);
        app.finish_go_to(GoTo::Definition, Ok(vec![other]), None);
        assert!(app.file_view.is_some());
        app.finish_go_to(GoTo::Definition, Ok(vec![t]), None);
        let view = app.file_view.as_ref().expect("still the file view");
        assert_eq!(
            (view.path.as_str(), view.line_no(), view.cursor_column()),
            (path.as_str(), no, 4)
        );
    }

    #[test]
    fn answers_apply_only_where_they_were_asked() {
        let mut app = sample_app();
        app.update(Action::Down(2));
        let asked = app.place();
        assert!(app.still_at(&asked));
        app.update(Action::Right(1));
        assert!(!app.still_at(&asked));
        app.update(Action::Left(1));
        assert!(app.still_at(&asked));
        app.update(Action::StartComment);
        assert!(!app.still_at(&asked));
    }

    #[test]
    fn says_when_nothing_is_found() {
        let mut app = sample_app();
        app.finish_go_to(GoTo::TypeDefinition, Ok(vec![]), None);
        assert_eq!(app.message.as_deref(), Some("No type definition found"));
        app.finish_go_to(GoTo::Definition, Err("boom".into()), None);
        assert_eq!(app.message.as_deref(), Some("error: boom"));
        assert!(app.jumps.is_empty());
    }
}
