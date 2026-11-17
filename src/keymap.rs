use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::Action;
use crate::input::InputAction;

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
            // Esc first drops a half-typed command, then cancels the mode.
            let had_pending = self.count.is_some() || self.prefix.is_some();
            *self = Self::default();
            return (!had_pending).then_some(Action::Cancel);
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
            (None, KeyCode::Char(c @ ('g' | 'z' | ']' | '[' | 'd'))) if !ctrl => {
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
            (Some(']'), KeyCode::Char('m')) => Action::NextComment(n),
            (Some('['), KeyCode::Char('m')) => Action::PrevComment(n),
            (Some('d'), KeyCode::Char('d')) => Action::DeleteComment,
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
            (None, KeyCode::Char('V' | 'v')) => Action::StartVisual,
            (None, KeyCode::Char('c')) => Action::StartComment,
            (None, KeyCode::Char('C')) => Action::CommentInEditor,
            (None, KeyCode::Char(':')) => Action::EnterCommand,
            _ => return None,
        };
        Some(action)
    }
}

/// Keys for the comment box and the `:` line, readline-style.
pub fn input_action(key: KeyEvent) -> Option<InputAction> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let action = match key.code {
        KeyCode::Esc => InputAction::Cancel,
        KeyCode::Char('c') if ctrl => InputAction::Cancel,
        KeyCode::Enter if alt => InputAction::Newline,
        KeyCode::Enter => InputAction::Submit,
        KeyCode::Char('o') if ctrl => InputAction::OpenEditor,
        KeyCode::Char('a') if ctrl => InputAction::Home,
        KeyCode::Char('e') if ctrl => InputAction::End,
        KeyCode::Char('w') if ctrl => InputAction::DeleteWord,
        KeyCode::Char('u') if ctrl => InputAction::ClearToStart,
        KeyCode::Char('h') if ctrl => InputAction::Backspace,
        KeyCode::Char(c) if !ctrl => InputAction::Insert(c),
        KeyCode::Backspace => InputAction::Backspace,
        KeyCode::Delete => InputAction::Delete,
        KeyCode::Left => InputAction::Left,
        KeyCode::Right => InputAction::Right,
        KeyCode::Home => InputAction::Home,
        KeyCode::End => InputAction::End,
        _ => return None,
    };
    Some(action)
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
        assert_eq!(press(&mut h, "2]m"), Some(Action::NextComment(2)));
        assert_eq!(press(&mut h, "dd"), Some(Action::DeleteComment));
    }

    #[test]
    fn unknown_sequences_and_escape_reset_state() {
        let mut h = KeyHandler::default();
        assert_eq!(press(&mut h, "gx"), None);
        assert_eq!(press(&mut h, "j"), Some(Action::Down(1)));
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        press(&mut h, "5g");
        assert_eq!(h.handle(esc), None);
        assert_eq!(h.pending(), "");
        assert_eq!(press(&mut h, "j"), Some(Action::Down(1)));
        assert_eq!(h.handle(esc), Some(Action::Cancel));
    }

    #[test]
    fn leading_zero_is_not_a_count() {
        let mut h = KeyHandler::default();
        assert_eq!(press(&mut h, "0"), None);
        assert_eq!(h.pending(), "");
        assert_eq!(press(&mut h, "10j"), Some(Action::Down(10)));
    }
}
