use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{Action, FindChar, GoTo, VerdictAction, View};
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
            (None, KeyCode::Char(c @ ('g' | 'z' | ']' | '[' | 'd' | 'f' | 'F' | 't'))) if !ctrl => {
                self.prefix = Some(c);
                self.count = count;
                return None;
            }
            (Some(p @ ('f' | 'F' | 't')), KeyCode::Char(target)) => {
                Action::FindChar { n, find: FindChar { target, forward: p != 'F', till: p == 't' } }
            }
            (Some('g'), KeyCode::Char('g')) => Action::Top,
            (Some('g'), KeyCode::Char('s')) => Action::ShowView(View::Stack),
            (Some('g'), KeyCode::Char('p')) => Action::ShowView(View::Plan),
            (Some('g'), KeyCode::Char('f')) => Action::ShowView(View::Files),
            (Some('g'), KeyCode::Char('c')) => Action::ListComments,
            (Some('g'), KeyCode::Char('d')) => Action::GoTo(GoTo::Definition),
            (Some('g'), KeyCode::Char('y')) => Action::GoTo(GoTo::TypeDefinition),
            (Some('g'), KeyCode::Char('i')) => Action::GoTo(GoTo::Implementation),
            (Some('g'), KeyCode::Char('r')) => Action::References,
            (Some(']'), KeyCode::Char('s')) => Action::NextLayer(n),
            (Some('['), KeyCode::Char('s')) => Action::PrevLayer(n),
            (Some(']'), KeyCode::Char('c')) => Action::NextHunk(n),
            (Some('['), KeyCode::Char('c')) => Action::PrevHunk(n),
            (Some(']'), KeyCode::Char('f')) => Action::NextFile(n),
            (Some('['), KeyCode::Char('f')) => Action::PrevFile(n),
            (Some(']'), KeyCode::Char('u')) => Action::NextUnreviewed(n),
            (Some('['), KeyCode::Char('u')) => Action::PrevUnreviewed(n),
            (Some(']'), KeyCode::Char('m')) => Action::NextComment(n),
            (Some('['), KeyCode::Char('m')) => Action::PrevComment(n),
            (Some('['), KeyCode::Char('e')) => Action::ExpandAbove(n),
            (Some(']'), KeyCode::Char('e')) => Action::ExpandBelow(n),
            (Some('d'), KeyCode::Char('d')) => Action::DeleteComment,
            (Some('z'), KeyCode::Char('a')) => Action::ToggleFold,
            (Some('z'), KeyCode::Char('o')) => Action::OpenFold,
            (Some('z'), KeyCode::Char('c')) => Action::CloseFold,
            (Some('z'), KeyCode::Char('R')) => Action::OpenAllFolds,
            (Some('z'), KeyCode::Char('M')) => Action::CloseAllFolds,
            (Some('z'), KeyCode::Char('t')) => Action::ScrollCursorTop,
            (Some('z'), KeyCode::Char('z')) => Action::ScrollCursorCenter,
            (Some('z'), KeyCode::Char('b')) => Action::ScrollCursorBottom,
            (Some('z'), KeyCode::Char('e')) => Action::ExpandScope,
            (Some('z'), KeyCode::Char('E')) => Action::ResetContext,
            (Some(_), _) => return None,

            (None, KeyCode::Char('c')) if ctrl => Action::Quit,
            (None, KeyCode::Char('d')) if ctrl => Action::HalfPageDown,
            (None, KeyCode::Char('u')) if ctrl => Action::HalfPageUp,
            (None, KeyCode::Char('o')) if ctrl => Action::JumpBack,
            (None, KeyCode::Char('e')) if ctrl => Action::PanelDown(3 * n),
            (None, KeyCode::Char('y')) if ctrl => Action::PanelUp(3 * n),
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
            (None, KeyCode::Char('h') | KeyCode::Left) => Action::Left(n),
            (None, KeyCode::Char('l') | KeyCode::Right) => Action::Right(n),
            (None, KeyCode::Char('w')) => Action::WordForward { n, big: false },
            (None, KeyCode::Char('W')) => Action::WordForward { n, big: true },
            (None, KeyCode::Char('b')) => Action::WordBackward { n, big: false },
            (None, KeyCode::Char('B')) => Action::WordBackward { n, big: true },
            (None, KeyCode::Char('e')) => Action::WordEnd { n, big: false },
            (None, KeyCode::Char('E')) => Action::WordEnd { n, big: true },
            (None, KeyCode::Char('0')) => Action::LineStart,
            (None, KeyCode::Char('^')) => Action::FirstNonBlank,
            (None, KeyCode::Char('$')) => Action::LineEnd,
            (None, KeyCode::Enter) => Action::Select,
            (None, KeyCode::Char('s')) => Action::ToggleSidebar,
            (None, KeyCode::Char('r')) => Action::ToggleReviewed,
            (None, KeyCode::Char('K')) => Action::Hover,
            (None, KeyCode::Char('R')) => Action::TogglePageReviewed,
            (None, KeyCode::Char('V' | 'v')) => Action::StartVisual,
            (None, KeyCode::Char('c')) => Action::StartComment,
            (None, KeyCode::Char('C')) => Action::CommentInEditor,
            (None, KeyCode::Char(':')) => Action::EnterCommand,
            (None, KeyCode::Char('/')) => Action::StartSearch,
            (None, KeyCode::Char('n')) => Action::SearchNext(n),
            (None, KeyCode::Char('N')) => Action::SearchPrev(n),
            (None, KeyCode::Char('?')) => Action::ToggleHelp,
            (None, KeyCode::Char('a')) => Action::Ask,
            (None, KeyCode::Char('A')) => Action::ToggleAskPanel,
            (None, KeyCode::Char('y')) => Action::Yank,
            (None, KeyCode::Char('Y')) => Action::CopyForAgent,
            (None, KeyCode::Char('S')) => Action::ToggleVerdict,
            (None, KeyCode::Char('T')) => Action::PickTheme,
            (None, KeyCode::Char('-')) => Action::ToggleRemovals,
            _ => return None,
        };
        Some(action)
    }
}

/// Keys on the verdict page, where each letter picks a button.
pub fn verdict_action(key: KeyEvent) -> Option<VerdictAction> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    Some(match key.code {
        KeyCode::Char('c') if ctrl => VerdictAction::Close,
        KeyCode::Esc | KeyCode::Char('q' | 'S') => VerdictAction::Close,
        KeyCode::Char('a') => VerdictAction::Approve,
        KeyCode::Char('r') => VerdictAction::RequestChanges,
        KeyCode::Char('c') => VerdictAction::Comment,
        KeyCode::Char('m') | KeyCode::Enter => VerdictAction::Message,
        KeyCode::Char('x') => VerdictAction::Clear,
        KeyCode::Char('Y' | 'y') => VerdictAction::CopyForAgent,
        KeyCode::Char('e') => VerdictAction::Export,
        _ => return None,
    })
}

/// Keys for a yes / no question: `Some(true)` for yes.
pub fn confirm_action(key: KeyEvent) -> Option<bool> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if ctrl => Some(false),
        KeyCode::Char('y' | 'Y') | KeyCode::Enter => Some(true),
        KeyCode::Char('n' | 'N' | 'q') | KeyCode::Esc => Some(false),
        _ => None,
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
        assert_eq!(press(&mut h, "3n"), Some(Action::SearchNext(3)));
        assert_eq!(press(&mut h, "N"), Some(Action::SearchPrev(1)));
        assert_eq!(press(&mut h, "/"), Some(Action::StartSearch));
        assert_eq!(press(&mut h, "?"), Some(Action::ToggleHelp));
        assert_eq!(press(&mut h, "2[e"), Some(Action::ExpandAbove(2)));
        assert_eq!(press(&mut h, "]e"), Some(Action::ExpandBelow(1)));
        assert_eq!(press(&mut h, "ze"), Some(Action::ExpandScope));
        assert_eq!(press(&mut h, "zE"), Some(Action::ResetContext));
        assert_eq!(press(&mut h, "a"), Some(Action::Ask));
        assert_eq!(press(&mut h, "Y"), Some(Action::CopyForAgent));
        assert_eq!(press(&mut h, "S"), Some(Action::ToggleVerdict));
        assert_eq!(press(&mut h, "T"), Some(Action::PickTheme));
        assert_eq!(press(&mut h, "-"), Some(Action::ToggleRemovals));
        assert_eq!(press(&mut h, "gs"), Some(Action::ShowView(View::Stack)));
        assert_eq!(press(&mut h, "gf"), Some(Action::ShowView(View::Files)));
        assert_eq!(press(&mut h, "gc"), Some(Action::ListComments));
        assert_eq!(press(&mut h, "2]s"), Some(Action::NextLayer(2)));
        assert_eq!(press(&mut h, "[s"), Some(Action::PrevLayer(1)));
        assert_eq!(press(&mut h, "p"), None);
    }

    #[test]
    fn verdict_keys_pick_buttons() {
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        assert_eq!(verdict_action(key('a')), Some(VerdictAction::Approve));
        assert_eq!(verdict_action(key('r')), Some(VerdictAction::RequestChanges));
        assert_eq!(verdict_action(key('Y')), Some(VerdictAction::CopyForAgent));
        assert_eq!(verdict_action(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)), Some(VerdictAction::Close));
        assert_eq!(verdict_action(key('j')), None);
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
        assert_eq!(press(&mut h, "0"), Some(Action::LineStart));
        assert_eq!(h.pending(), "");
        assert_eq!(press(&mut h, "10j"), Some(Action::Down(10)));
    }

    #[test]
    fn go_to_keys() {
        let mut h = KeyHandler::default();
        assert_eq!(press(&mut h, "gd"), Some(Action::GoTo(GoTo::Definition)));
        assert_eq!(press(&mut h, "gy"), Some(Action::GoTo(GoTo::TypeDefinition)));
        assert_eq!(press(&mut h, "gi"), Some(Action::GoTo(GoTo::Implementation)));
        assert_eq!(press(&mut h, "gr"), Some(Action::References));
        assert_eq!(press(&mut h, "K"), Some(Action::Hover));
    }

    #[test]
    fn column_motions() {
        let mut h = KeyHandler::default();
        assert_eq!(press(&mut h, "3l"), Some(Action::Right(3)));
        assert_eq!(press(&mut h, "h"), Some(Action::Left(1)));
        assert_eq!(press(&mut h, "2w"), Some(Action::WordForward { n: 2, big: false }));
        assert_eq!(press(&mut h, "B"), Some(Action::WordBackward { n: 1, big: true }));
        assert_eq!(press(&mut h, "E"), Some(Action::WordEnd { n: 1, big: true }));
        assert_eq!(press(&mut h, "^"), Some(Action::FirstNonBlank));
        assert_eq!(press(&mut h, "$"), Some(Action::LineEnd));
        let find = |target, forward, till| FindChar { target, forward, till };
        assert_eq!(press(&mut h, "2f("), Some(Action::FindChar { n: 2, find: find('(', true, false) }));
        assert_eq!(press(&mut h, "t)"), Some(Action::FindChar { n: 1, find: find(')', true, true) }));
        assert_eq!(press(&mut h, "Fx"), Some(Action::FindChar { n: 1, find: find('x', false, false) }));
        // `T` is still the theme picker.
        assert_eq!(press(&mut h, "T"), Some(Action::PickTheme));
    }
}
