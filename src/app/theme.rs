//! The theme picker: `T` or `:theme`. Moving through it shows each theme
//! straight away; Enter keeps the one shown, Esc goes back to the one before.
//! The OS's dark or light mode picks the theme's variant, never the picker.

use super::{Action, App, Effect};
use crate::appearance::Appearance;
use crate::theme::{THEMES, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemePicker {
    /// Index into `THEMES` of the entry under the cursor, the one shown.
    pub selected: usize,
    /// The theme from before the picker opened, to go back to.
    original: &'static Theme,
}

impl App {
    pub fn set_theme(&mut self, theme: &'static Theme) {
        self.theme = theme;
        self.palette = theme.palette(self.appearance);
    }

    /// Follows the OS into dark or light mode.
    pub fn set_appearance(&mut self, appearance: Appearance) {
        self.appearance = appearance;
        self.palette = self.theme.palette(appearance);
    }

    pub(super) fn open_theme_picker(&mut self) {
        self.theme_picker = Some(ThemePicker { selected: self.theme.index(), original: self.theme });
    }

    /// While the picker is open, motions move through it and the theme
    /// follows; anything that closes things closes it, putting the old theme back.
    pub(super) fn update_theme_picker(&mut self, action: Action) {
        let Some(picker) = self.theme_picker else { return };
        let last = THEMES.len() - 1;
        let selected = match action {
            Action::Down(n) => (picker.selected + n).min(last),
            Action::Up(n) => picker.selected.saturating_sub(n),
            Action::HalfPageDown => (picker.selected + 5).min(last),
            Action::HalfPageUp => picker.selected.saturating_sub(5),
            Action::Top => 0,
            Action::Bottom => last,
            Action::Select => {
                self.theme_picker = None;
                self.choose_theme(&THEMES[picker.selected]);
                return;
            }
            Action::PickTheme | Action::Cancel | Action::Quit => {
                self.theme_picker = None;
                self.set_theme(picker.original);
                return;
            }
            _ => picker.selected,
        };
        self.theme_picker = Some(ThemePicker { selected, ..picker });
        self.set_theme(&THEMES[selected]);
    }

    /// `:theme [name]`: the picker, or straight to a theme.
    pub(super) fn theme_command(&mut self, args: &[&str]) {
        match args {
            [] => self.open_theme_picker(),
            [name] => match Theme::find(name) {
                Some(theme) => self.choose_theme(theme),
                None => {
                    let names: Vec<&str> = THEMES.iter().map(|t| t.id.trim_start_matches("doom-")).collect();
                    self.message = Some(format!("error: no theme `{name}`, try: {}", names.join(" ")));
                }
            },
            _ => self.message = Some("error: usage: :theme [name]".to_string()),
        }
    }

    /// Switches to `theme` and keeps it for next time.
    fn choose_theme(&mut self, theme: &'static Theme) {
        self.set_theme(theme);
        self.message = Some(format!("Theme: {}", theme.name));
        self.effect = Some(Effect::SaveTheme { theme });
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;

    #[test]
    fn previews_as_it_moves_and_keeps_on_enter() {
        let mut app = sample_app();
        assert_eq!(app.theme, Theme::terminal());
        app.update(Action::PickTheme);
        app.update(Action::Down(2));
        assert_eq!(app.theme, &THEMES[2]);
        assert_eq!(app.palette, THEMES[2].palette(Appearance::Dark));
        // Other keys wait while it's open.
        app.update(Action::NextFile(1));
        assert_eq!(app.page, 0);
        app.update(Action::Select);
        assert_eq!(app.theme_picker, None);
        assert_eq!(app.theme, &THEMES[2]);
        assert_eq!(app.effect.take(), Some(Effect::SaveTheme { theme: &THEMES[2] }));
    }

    #[test]
    fn the_os_mode_picks_the_variant() {
        let mut app = sample_app();
        command(&mut app, "theme gruvbox");
        let dark = app.palette;
        app.set_appearance(Appearance::Light);
        assert_eq!(app.theme.id, "doom-gruvbox");
        assert_ne!(app.palette, dark);
        assert_eq!(app.palette, app.theme.palette(Appearance::Light));
    }

    #[test]
    fn escape_puts_the_old_theme_back() {
        let mut app = sample_app();
        app.update(Action::PickTheme);
        app.update(Action::Bottom);
        assert_eq!(app.theme, THEMES.last().unwrap());
        app.update(Action::Cancel);
        assert_eq!(app.theme, Theme::terminal());
        assert_eq!(app.effect, None);
        // `q` closes it rather than quitting.
        app.update(Action::PickTheme);
        app.update(Action::Quit);
        assert!(!app.should_quit);
    }

    #[test]
    fn theme_command_picks_by_name() {
        let mut app = sample_app();
        command(&mut app, "theme nord-light");
        assert_eq!(app.theme.id, "doom-nord");
        assert!(matches!(app.effect.take(), Some(Effect::SaveTheme { .. })));
        command(&mut app, "theme nope");
        assert!(app.message.as_deref().unwrap().starts_with("error: no theme"));
        command(&mut app, "theme");
        assert!(app.theme_picker.is_some());
    }
}
