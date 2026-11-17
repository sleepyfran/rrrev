use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::Action;

/// Turns key presses into actions, handling vim-style counts (`5j`) and
/// two-key sequences (`gg`, `]c`, `za`). Unknown sequences are dropped, as in vim.
#[derive(Debug, Default)]
pub struct KeyHandler {
    count: Option<usize>,
    prefix: Option<char>,
}

impl KeyHandler {
    /// Keys typed so far for an unfinished command, shown like vim's `showcmd`.
    pub fn pending(&self) -> String {
        let count = self.count.map(|c| c.to_string()).unwrap_or_default();
        let prefix = self.prefix.map(String::from).unwrap_or_default();
        count + &prefix
    }

    pub fn handle(&mut self, key: KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc {
            *self = Self::default();
            return None;
        }
        if let KeyCode::Char(c @ '0'..='9') = key.code
            && !ctrl
            && self.prefix.is_none()
            && (c != '0' || self.count.is_some())
        {
            let digit = c.to_digit(10).expect("matched a digit") as usize;
            self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(digit));
            return None;
        }

        let count = self.count.take();
        let n = count.unwrap_or(1);
        let action = match (self.prefix.take(), key.code) {
            (None, KeyCode::Char(c @ ('g' | 'z' | ']' | '['))) if !ctrl => {
                self.prefix = Some(c);
                self.count = count;
                return None;
            }
            (Some('g'), KeyCode::Char('g')) => Action::Top,
            (Some(']'), KeyCode::Char('c')) => Action::NextHunk(n),
            (Some('['), KeyCode::Char('c')) => Action::PrevHunk(n),
            (Some(']'), KeyCode::Char('f')) => Action::NextFile(n),
            (Some('['), KeyCode::Char('f')) => Action::PrevFile(n),
            (Some(']'), KeyCode::Char('u')) => Action::NextUnreviewed(n),
            (Some('['), KeyCode::Char('u')) => Action::PrevUnreviewed(n),
            (Some('z'), KeyCode::Char('a')) => Action::ToggleFold,
            (Some('z'), KeyCode::Char('o')) => Action::OpenFold,
            (Some('z'), KeyCode::Char('c')) => Action::CloseFold,
            (Some('z'), KeyCode::Char('R')) => Action::OpenAllFolds,
            (Some('z'), KeyCode::Char('M')) => Action::CloseAllFolds,
            (Some('z'), KeyCode::Char('t')) => Action::ScrollCursorTop,
            (Some('z'), KeyCode::Char('z')) => Action::ScrollCursorCenter,
            (Some('z'), KeyCode::Char('b')) => Action::ScrollCursorBottom,
            (Some(_), _) => return None,

            (None, KeyCode::Char('c')) if ctrl => Action::Quit,
            (None, KeyCode::Char('d')) if ctrl => Action::HalfPageDown,
            (None, KeyCode::Char('u')) if ctrl => Action::HalfPageUp,
            (None, KeyCode::Char('q')) => Action::Quit,
            (None, KeyCode::Char('j') | KeyCode::Down) => Action::Down(n),
            (None, KeyCode::Char('k') | KeyCode::Up) => Action::Up(n),
            (None, KeyCode::Char('}')) => Action::NextHunk(n),
            (None, KeyCode::Char('{')) => Action::PrevHunk(n),
            (None, KeyCode::Char('G') | KeyCode::End) => Action::Bottom,
            (None, KeyCode::Home) => Action::Top,
            (None, KeyCode::PageDown) => Action::HalfPageDown,
            (None, KeyCode::PageUp) => Action::HalfPageUp,
            (None, KeyCode::Tab) => Action::ToggleFocus,
            (None, KeyCode::Char('h')) => Action::FocusSidebar,
            (None, KeyCode::Char('l')) => Action::FocusDiff,
            (None, KeyCode::Enter) => Action::Select,
            (None, KeyCode::Char('s')) => Action::ToggleSidebar,
            (None, KeyCode::Char('r')) => Action::ToggleReviewed,
            (None, KeyCode::Char('R')) => Action::ToggleFileReviewed,
            _ => return None,
        };
        Some(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(handler: &mut KeyHandler, keys: &str) -> Option<Action> {
        keys.chars()
            .map(|c| handler.handle(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)))
            .last()
            .flatten()
    }

    #[test]
    fn counts_apply_to_motions() {
        let mut h = KeyHandler::default();
        assert_eq!(press(&mut h, "12j"), Some(Action::Down(12)));
        assert_eq!(press(&mut h, "k"), Some(Action::Up(1)));
    }

    #[test]
    fn sequences_carry_counts_through_the_prefix() {
        let mut h = KeyHandler::default();
        assert_eq!(press(&mut h, "3]"), None);
        assert_eq!(h.pending(), "3]");
        assert_eq!(press(&mut h, "c"), Some(Action::NextHunk(3)));
        assert_eq!(h.pending(), "");
        assert_eq!(press(&mut h, "gg"), Some(Action::Top));
        assert_eq!(press(&mut h, "zM"), Some(Action::CloseAllFolds));
    }

    #[test]
    fn unknown_sequences_and_escape_reset_state() {
        let mut h = KeyHandler::default();
        assert_eq!(press(&mut h, "gx"), None);
        assert_eq!(press(&mut h, "j"), Some(Action::Down(1)));
        press(&mut h, "5g");
        h.handle(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(h.pending(), "");
        assert_eq!(press(&mut h, "j"), Some(Action::Down(1)));
    }

    #[test]
    fn leading_zero_is_not_a_count() {
        let mut h = KeyHandler::default();
        assert_eq!(press(&mut h, "0"), None);
        assert_eq!(h.pending(), "");
        assert_eq!(press(&mut h, "10j"), Some(Action::Down(10)));
    }
}
