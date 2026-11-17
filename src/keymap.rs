use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::Action;

/// Maps a single key press to an action. Multi-key sequences (`gg`, `]c`) and
/// counts arrive with the pending-key state machine in milestone 2.
pub fn action_for(key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let action = match key.code {
        KeyCode::Char('c') if ctrl => Action::Quit,
        KeyCode::Char('d') if ctrl => Action::HalfPageDown,
        KeyCode::Char('u') if ctrl => Action::HalfPageUp,
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('j') | KeyCode::Down => Action::Down(1),
        KeyCode::Char('k') | KeyCode::Up => Action::Up(1),
        KeyCode::Char('G') | KeyCode::End => Action::Bottom,
        KeyCode::Home => Action::Top,
        KeyCode::PageDown => Action::HalfPageDown,
        KeyCode::PageUp => Action::HalfPageUp,
        _ => return None,
    };
    Some(action)
}
