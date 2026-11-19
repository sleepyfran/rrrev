//! The `:` command line.

use std::path::PathBuf;

use super::{App, Effect, Mode};
use crate::export::Format;
use crate::input::{InputAction, TextInput};

impl App {
    pub(super) fn enter_command(&mut self) {
        self.mode = Mode::Command { input: TextInput::default() };
    }

    pub(super) fn command_input(&mut self, action: InputAction) {
        let Mode::Command { input } = &mut self.mode else { return };
        match action {
            InputAction::Submit => {
                let command = input.text().trim().to_string();
                self.mode = Mode::Normal;
                self.run_command(&command);
            }
            InputAction::Cancel => self.mode = Mode::Normal,
            // As in vim, backspacing past the `:` leaves the command line.
            InputAction::Backspace if input.text().is_empty() => self.mode = Mode::Normal,
            InputAction::Newline | InputAction::OpenEditor => {}
            other => input.apply(other),
        }
    }

    fn run_command(&mut self, command: &str) {
        let mut words = command.split_whitespace();
        let Some(name) = words.next() else { return };
        let args: Vec<&str> = words.collect();
        match name {
            "q" | "q!" | "quit" | "qa" | "wq" | "x" => self.should_quit = true,
            "w" | "write" => {
                self.review.mark_dirty();
                self.message = Some("Saved".to_string());
            }
            "export" => self.export_command(&args),
            "organize" => self.organize_command(&args),
            "ask" => self.ask_command(&args),
            "verdict" => self.show_verdict = true,
            _ => match name.parse::<u32>() {
                Ok(line) => self.go_to_line(line),
                Err(_) => self.message = Some(format!("error: not a command: {name}")),
            },
        }
    }

    /// `:export [md|json] [path]`.
    fn export_command(&mut self, args: &[&str]) {
        let format = match args.first() {
            None => Some(Format::Markdown),
            Some(arg) => Format::parse(arg),
        };
        match (format, args.get(2)) {
            (Some(format), None) => {
                let path = args.get(1).map(PathBuf::from);
                self.effect = Some(Effect::Export { format, path });
            }
            _ => self.message = Some("error: usage: :export [md|json] [path]".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::{Action, Row};

    #[test]
    fn commands() {
        let mut app = sample_app();
        command(&mut app, "export json out.json");
        assert_eq!(
            app.effect.take(),
            Some(Effect::Export { format: Format::Json, path: Some("out.json".into()) })
        );
        command(&mut app, "export");
        assert_eq!(app.effect.take(), Some(Effect::Export { format: Format::Markdown, path: None }));
        command(&mut app, "export pdf");
        assert!(app.message.as_deref().unwrap().starts_with("error:"));

        command(&mut app, "3");
        assert_eq!(app.current_row(), Some(Row::Line { file: 0, hunk: 0, line: 2 }));
        command(&mut app, "nope");
        assert_eq!(app.message.as_deref(), Some("error: not a command: nope"));
        command(&mut app, "w");
        assert!(app.review.take_dirty());
        command(&mut app, "q");
        assert!(app.should_quit);
    }

    #[test]
    fn backspace_on_empty_command_line_leaves_it() {
        let mut app = sample_app();
        app.update(Action::EnterCommand);
        type_text(&mut app, "q");
        app.update_input(InputAction::Backspace);
        assert!(app.is_typing());
        app.update_input(InputAction::Backspace);
        assert_eq!(app.mode, Mode::Normal);
        assert!(!app.should_quit);
    }
}
