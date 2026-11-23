//! Offering to build a language's grammar the first time the cursor reaches
//! one of its files, so highlighting and breadcrumbs come on without leaving
//! for `rrrev lang enable`.

use std::collections::HashSet;
use std::time::Instant;

use anyhow::Result;

use super::{App, Effect, Focus, Mode};
use crate::languages::manage::Status;
use crate::languages::{Language, Registry};

/// Asked when the cursor reaches a file whose language rrrev could
/// highlight once its grammar is built: whether to build it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnableLanguage {
    pub name: String,
    /// Why the grammar built before doesn't load, when there is one.
    pub broken: Option<String>,
}

/// What the app knows of languages' grammars this session.
#[derive(Debug, Default)]
pub struct Grammars {
    /// Where a language's grammar stands. `None` never offers anything: the
    /// main loop sets `manage::status`, which reads the disk, and tests set
    /// their own.
    pub status: Option<fn(&Language) -> Status>,
    /// Languages already looked at, so each is asked about once at most.
    seen: HashSet<String>,
    /// The grammars being built, and since when, for the status line.
    pub building: Vec<(String, Instant)>,
}

impl App {
    /// Asks whether to build the grammar of the file the cursor is in, the
    /// first time it gets to a file in that language. It waits while the
    /// user is busy with something else, like a comment or a pop-up.
    pub fn offer_grammar(&mut self) {
        if self.is_busy() {
            return;
        }
        let Some(status) = self.grammars.status else { return };
        let path = match &self.file_view {
            Some(view) => Some(view.path.as_str()),
            None => self.current_file().map(|f| self.diff.files[f].path()),
        };
        let Some(language) = path.and_then(|p| Registry::global().for_path(p)) else { return };
        if !self.grammars.seen.insert(language.name.clone()) {
            return;
        }
        let broken = match status(language) {
            Status::Available => None,
            // A rebuild usually mends it, e.g. after updating tree-sitter.
            Status::Broken(why) => Some(why),
            Status::Enabled | Status::NoGrammar => return,
        };
        self.language_prompt = Some(EnableLanguage { name: language.name.clone(), broken });
    }

    /// Whether something has the user's attention that a question would get
    /// in the way of.
    fn is_busy(&self) -> bool {
        self.mode != Mode::Normal
            || self.help_scroll.is_some()
            || self.dismiss_prompt.is_some()
            || self.ask.confirm_clear
            || self.language_prompt.is_some()
            || self.hover.is_some()
            || self.references.is_some()
            || self.theme_picker.is_some()
            || self.comments_menu.is_some()
            || self.show_verdict
            || (self.compact && self.focus == Focus::Sidebar)
    }

    /// Answers the question from `offer_grammar`: builds the grammar in the
    /// background, or leaves the language plain for the rest of the session.
    pub fn answer_enable_language(&mut self, build: bool) {
        self.message = None;
        let Some(prompt) = self.language_prompt.take() else { return };
        let name = prompt.name;
        if build {
            self.grammars.building.push((name.clone(), Instant::now()));
            self.effect = Some(Effect::EnableLanguage { name });
        } else {
            self.message = Some(format!("{name} stays plain; `rrrev lang enable {name}` builds it later"));
        }
    }

    /// Called when a build from `Effect::EnableLanguage` ends. The main loop
    /// has already dropped the grammar and highlighting it had cached; this
    /// drops the outlines, so breadcrumbs and `ze` use the grammar too.
    pub fn finish_enable_language(&mut self, name: &str, result: Result<()>) {
        self.grammars.building.retain(|(n, _)| n != name);
        self.message = Some(match result {
            Ok(()) => {
                self.diff.forget_outlines(name);
                self.fold_new_plumbing();
                format!("Enabled {name}")
            }
            Err(e) => format!("error: cannot enable {name}: {e:#}"),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::{Action, FileView};
    use crate::diff::{FileDiff, Side};
    use crate::input::InputAction;

    /// a.rs, b.rs, c.py and d.toml, with Rust and Python not built, and
    /// TOML without a grammar.
    fn app() -> App {
        let files: Vec<FileDiff> = ["a.rs", "b.rs", "c.py", "d.toml"].iter().map(|p| file(p, &[2])).collect();
        let mut app = app_with(files);
        app.grammars.status = Some(|language| match language.name.as_str() {
            "rust" | "python" => Status::Available,
            _ => Status::NoGrammar,
        });
        app
    }

    fn asked(app: &App) -> Option<&str> {
        app.language_prompt.as_ref().map(|p| p.name.as_str())
    }

    #[test]
    fn asks_once_per_language() {
        let mut app = app();
        app.offer_grammar();
        assert_eq!(asked(&app), Some("rust"));
        app.answer_enable_language(false);
        assert_eq!(app.message.as_deref(), Some("rust stays plain; `rrrev lang enable rust` builds it later"));
        assert_eq!(app.effect, None);
        // Another Rust file doesn't ask again.
        app.update(Action::NextFile(1));
        app.offer_grammar();
        assert_eq!(asked(&app), None);
        app.update(Action::NextFile(1));
        app.offer_grammar();
        assert_eq!(asked(&app), Some("python"));
        app.answer_enable_language(false);
        app.update(Action::NextFile(1));
        app.offer_grammar();
        assert_eq!(asked(&app), None, "TOML has no grammar to build");
        app.update(Action::Top);
        app.update(Action::PrevFile(3));
        app.offer_grammar();
        assert_eq!(asked(&app), None);
    }

    #[test]
    fn never_asks_without_a_way_to_tell() {
        let mut app = app();
        app.grammars.status = None;
        app.offer_grammar();
        assert_eq!(asked(&app), None);
    }

    #[test]
    fn leaves_enabled_languages_alone_and_offers_to_rebuild_broken_ones() {
        let mut app = app();
        app.grammars.status = Some(|language| match language.name.as_str() {
            "rust" => Status::Enabled,
            _ => Status::Broken("wrong ABI".into()),
        });
        app.offer_grammar();
        assert_eq!(asked(&app), None);
        app.update(Action::NextFile(2));
        app.offer_grammar();
        let prompt = app.language_prompt.clone().unwrap();
        assert_eq!(prompt, EnableLanguage { name: "python".into(), broken: Some("wrong ABI".into()) });
    }

    #[test]
    fn waits_until_the_comment_is_done() {
        let mut app = app();
        app.update(Action::Down(2));
        app.update(Action::StartComment);
        app.offer_grammar();
        assert_eq!(asked(&app), None);
        type_text(&mut app, "y");
        assert_eq!(app.language_prompt, None, "typed into the comment, not an answer");
        app.update_input(InputAction::Submit);
        app.offer_grammar();
        assert_eq!(asked(&app), Some("rust"));
        assert_eq!(app.review.comments()[0].text, "y");
    }

    #[test]
    fn waits_for_pop_ups_to_close() {
        let mut app = app();
        app.update(Action::ToggleHelp);
        app.offer_grammar();
        assert_eq!(asked(&app), None);
        app.update(Action::ToggleHelp);
        app.offer_grammar();
        assert_eq!(asked(&app), Some("rust"));
        // While it's open, keys don't get past it.
        app.update(Action::NextFile(1));
        assert_eq!(app.page, 0);
    }

    #[test]
    fn asks_about_the_file_open_in_the_file_view() {
        let mut app = app();
        // As `gd` opens a file outside the diff, over a.rs.
        let lines = std::sync::Arc::new(vec!["def f(): pass".to_string()]);
        app.file_view = Some(FileView::new("lib/f.py".into(), Side::New, lines, None, vec![None]));
        app.offer_grammar();
        assert_eq!(asked(&app), Some("python"));
    }

    #[test]
    fn building_runs_in_the_background_until_it_finishes() {
        let mut app = app();
        app.offer_grammar();
        app.answer_enable_language(true);
        assert_eq!(app.effect.take(), Some(Effect::EnableLanguage { name: "rust".into() }));
        assert_eq!(app.grammars.building.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["rust"]);
        app.finish_enable_language("rust", Ok(()));
        assert!(app.grammars.building.is_empty());
        assert_eq!(app.message.as_deref(), Some("Enabled rust"));

        app.update(Action::NextFile(2));
        app.offer_grammar();
        app.answer_enable_language(true);
        app.finish_enable_language("python", Err(anyhow::anyhow!("cannot run git; is it installed?")));
        assert!(app.grammars.building.is_empty());
        assert_eq!(app.message.as_deref(), Some("error: cannot enable python: cannot run git; is it installed?"));
    }
}
