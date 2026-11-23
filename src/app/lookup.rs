//! `gr` and `K`: the list of a symbol's references, marking those the diff
//! shows, and what the language server says about a symbol.

use super::goto::Target;
use super::{Action, App, Effect};

/// One place in the references list.
#[derive(Debug, Clone)]
pub struct Reference {
    pub target: Target,
    /// Where the diff shows it, as a page and row.
    pub in_diff: Option<(usize, usize)>,
}

impl Reference {
    /// Its line's code, trimmed, for the list.
    pub fn code(&self) -> &str {
        self.target
            .lines
            .get(self.target.line)
            .map_or("", |l| l.trim())
    }
}

#[derive(Debug, Clone)]
pub struct ReferencesMenu {
    pub references: Vec<Reference>,
    /// Index into `shown()` of the entry under the cursor.
    pub selected: usize,
    /// Only the references the diff shows.
    pub only_in_diff: bool,
    /// Said under the list, e.g. that the answer may be off.
    pub note: Option<String>,
}

impl ReferencesMenu {
    /// The entries listed, after the filter.
    pub fn shown(&self) -> Vec<&Reference> {
        self.references
            .iter()
            .filter(|r| !self.only_in_diff || r.in_diff.is_some())
            .collect()
    }
}

/// What `K` showed: the server's text, a line at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hover {
    pub lines: Vec<String>,
    pub scroll: usize,
}

impl App {
    pub(super) fn references(&mut self) {
        match self.cursor_spot() {
            Ok(spot) => self.effect = Some(Effect::References { spot }),
            Err(why) => self.message = Some(why.to_string()),
        }
    }

    pub(super) fn hover(&mut self) {
        match self.cursor_spot() {
            Ok(spot) => self.effect = Some(Effect::Hover { spot }),
            Err(why) => self.message = Some(why.to_string()),
        }
    }

    /// The server's references: the list, starting on the first the diff
    /// shows.
    pub fn finish_references(&mut self, targets: Result<Vec<Target>, String>, note: Option<&str>) {
        let targets = match targets {
            Ok(targets) => targets,
            Err(e) => {
                self.message = Some(format!("error: {e}"));
                return;
            }
        };
        if targets.is_empty() {
            self.message = Some("No references found".to_string());
            return;
        }
        let references: Vec<Reference> = targets
            .into_iter()
            .map(|target| Reference {
                in_diff: self.shown_at(&target),
                target,
            })
            .collect();
        let selected = references
            .iter()
            .position(|r| r.in_diff.is_some())
            .unwrap_or(0);
        self.references = Some(ReferencesMenu {
            references,
            selected,
            only_in_diff: false,
            note: note.map(str::to_string),
        });
    }

    /// While the list is open: motions move through it, Enter goes to the
    /// reference, Tab shows only those in the diff, Esc closes it.
    pub(super) fn update_references(&mut self, action: Action) {
        let Some(menu) = self.references.as_mut() else {
            return;
        };
        let last = menu.shown().len().saturating_sub(1);
        match action {
            Action::Down(n) => menu.selected = menu.selected.saturating_add(n).min(last),
            Action::Up(n) => menu.selected = menu.selected.saturating_sub(n),
            Action::HalfPageDown => menu.selected = menu.selected.saturating_add(5).min(last),
            Action::HalfPageUp => menu.selected = menu.selected.saturating_sub(5),
            Action::Top => menu.selected = 0,
            Action::Bottom => menu.selected = last,
            Action::ToggleFocus => {
                let current = menu
                    .shown()
                    .get(menu.selected)
                    .map(|r| (r.target.path.clone(), r.target.line));
                menu.only_in_diff = !menu.only_in_diff;
                let shown = menu.shown();
                let (empty, at) = (
                    shown.is_empty(),
                    current.and_then(|(path, line)| {
                        shown
                            .iter()
                            .position(|r| r.target.path == path && r.target.line == line)
                    }),
                );
                menu.selected = at.unwrap_or(0);
                if empty {
                    self.message = Some("None of them are in the diff".to_string());
                    menu.only_in_diff = false;
                }
            }
            Action::Select => {
                let target = menu.shown().get(menu.selected).map(|r| r.target.clone());
                self.references = None;
                if let Some(target) = target {
                    self.jumps.push(self.here());
                    self.go_to_target(&target);
                }
            }
            Action::ToggleHelp => self.help_scroll = Some(0),
            Action::Cancel | Action::Quit | Action::References => self.references = None,
            _ => {}
        }
    }

    /// The server's answer to `K`, as text to read.
    pub fn finish_hover(&mut self, text: Result<String, String>, note: Option<&str>) {
        match text {
            Err(e) => self.message = Some(format!("error: {e}")),
            Ok(text) if text.trim().is_empty() => {
                self.message = Some("Nothing to say about this".to_string())
            }
            Ok(text) => {
                let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
                if let Some(note) = note {
                    lines.push(String::new());
                    lines.push(format!("({note})"));
                }
                self.hover = Some(Hover { lines, scroll: 0 });
            }
        }
    }

    /// While the hover box is open: j/k scroll it, anything else closes it.
    pub(super) fn update_hover(&mut self, action: Action) {
        let Some(hover) = self.hover.as_mut() else {
            return;
        };
        let last = hover.lines.len().saturating_sub(1);
        match action {
            Action::Down(n) => hover.scroll = hover.scroll.saturating_add(n).min(last),
            Action::Up(n) => hover.scroll = hover.scroll.saturating_sub(n),
            Action::HalfPageDown => hover.scroll = hover.scroll.saturating_add(5).min(last),
            Action::HalfPageUp => hover.scroll = hover.scroll.saturating_sub(5),
            _ => self.hover = None,
        }
    }
}

/// The text of a hover answer, which may be markup, a marked string, or a
/// list of those. Code fences and rules are dropped: the box shows plain
/// text.
pub fn hover_text(result: &serde_json::Value) -> String {
    use serde_json::Value;
    fn part(value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            Value::Object(o) => o
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            Value::Array(items) => items
                .iter()
                .map(part)
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"),
            _ => String::new(),
        }
    }
    let text = part(&result["contents"]);
    let mut lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("```"))
        .map(|l| if l.trim() == "---" { "" } else { l })
        .collect();
    lines.dedup_by(|a, b| a.trim().is_empty() && b.trim().is_empty());
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::test_support::*;
    use super::*;
    use crate::app::Row;
    use serde_json::json;

    fn target(path: &str, line: usize, lines: &[&str]) -> Target {
        Target {
            path: path.into(),
            line,
            column: 0,
            lines: Arc::new(lines.iter().map(|l| l.to_string()).collect()),
        }
    }

    /// A target on the first new-side line the sample's diff shows.
    fn shown_target(app: &crate::app::App) -> Target {
        let (file, no) = (0..app.rows.len())
            .find_map(|i| match app.rows[i] {
                Row::Line { file, hunk, line } => {
                    let l = app.line(file, hunk, line);
                    (l.kind != crate::diff::LineKind::Removed)
                        .then_some(l.new_no)
                        .flatten()
                        .map(|n| (file, n))
                }
                _ => None,
            })
            .unwrap();
        let path = app.diff.files[file].new_path.clone().unwrap();
        let lines: Vec<String> = (0..no).map(|i| format!("line {i}")).collect();
        Target {
            path,
            line: no as usize - 1,
            column: 0,
            lines: Arc::new(lines),
        }
    }

    #[test]
    fn lists_references_and_marks_those_in_the_diff() {
        let mut app = sample_app();
        let outside = target("src/elsewhere.rs", 0, &["  use_it();"]);
        let inside = shown_target(&app);
        app.finish_references(Ok(vec![outside, inside]), None);
        let menu = app.references.as_ref().unwrap();
        assert_eq!(menu.references[0].code(), "use_it();");
        assert!(menu.references[0].in_diff.is_none() && menu.references[1].in_diff.is_some());
        // It starts on the first one in the diff.
        assert_eq!(menu.selected, 1);

        // Tab keeps only those, on the same entry.
        app.update(Action::ToggleFocus);
        let menu = app.references.as_ref().unwrap();
        assert_eq!((menu.shown().len(), menu.selected), (1, 0));
        app.update(Action::ToggleFocus);
        assert_eq!(app.references.as_ref().unwrap().selected, 1);

        // Enter goes there, and C-o comes back.
        app.update(Action::Up(1));
        app.update(Action::Select);
        assert!(app.references.is_none());
        assert_eq!(app.file_view.as_ref().unwrap().path, "src/elsewhere.rs");
        app.update(Action::JumpBack);
        assert!(app.file_view.is_none());
    }

    #[test]
    fn says_when_there_are_none() {
        let mut app = sample_app();
        app.finish_references(Ok(vec![]), None);
        assert_eq!(app.message.as_deref(), Some("No references found"));
        assert!(app.references.is_none());
    }

    #[test]
    fn shows_hovers_and_closes_on_any_other_key() {
        let mut app = sample_app();
        app.finish_hover(Ok("fn a()\n\nDocs".into()), Some("may be off"));
        assert_eq!(
            app.hover.as_ref().unwrap().lines,
            ["fn a()", "", "Docs", "", "(may be off)"]
        );
        app.update(Action::Down(2));
        assert_eq!(app.hover.as_ref().unwrap().scroll, 2);
        app.update(Action::Cancel);
        assert!(app.hover.is_none());
        app.finish_hover(Ok("  ".into()), None);
        assert!(app.hover.is_none());
    }

    #[test]
    fn reads_every_shape_of_hover() {
        let markup = json!({"contents": {"kind": "markdown", "value": "```rust\nfn a()\n```\n\n---\n\nDocs"}});
        assert_eq!(hover_text(&markup), "fn a()\n\nDocs");
        let marked = json!({"contents": [{"language": "rust", "value": "fn a()"}, "Docs"]});
        assert_eq!(hover_text(&marked), "fn a()\n\nDocs");
        assert_eq!(hover_text(&json!({"contents": "x"})), "x");
        assert_eq!(hover_text(&serde_json::Value::Null), "");
    }
}
