//! Whether the OS is in dark or light mode, which picks each theme's
//! variant. Read at start and watched while rrrev runs, so the colours
//! follow the OS when it switches.

/// The OS's dark or light mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

impl Appearance {
    /// The OS's mode. When the OS can't say (no desktop portal, say), the
    /// terminal is asked for its background, and dark is assumed when it
    /// doesn't answer. Asking the terminal needs it to itself, so this runs
    /// before the UI takes it over.
    pub fn detect() -> Self {
        match dark_light::detect().ok().and_then(from_os) {
            Some(appearance) => appearance,
            None => match terminal_colorsaurus::theme_mode(Default::default()) {
                Ok(terminal_colorsaurus::ThemeMode::Light) => Appearance::Light,
                _ => Appearance::Dark,
            },
        }
    }
}

fn from_os(mode: dark_light::Mode) -> Option<Appearance> {
    match mode {
        dark_light::Mode::Dark => Some(Appearance::Dark),
        dark_light::Mode::Light => Some(Appearance::Light),
        dark_light::Mode::Unspecified => None,
    }
}

/// Hears about the OS switching between dark and light mode, for as long as
/// it's kept. `None` when the OS can't be watched; the mode read at start then holds.
pub struct Watch(Option<dark_light::Watcher>);

impl Watch {
    pub fn start() -> Self {
        Self(dark_light::subscribe().ok())
    }

    /// The latest mode the OS switched to since the last call, if it did.
    pub fn changed(&self) -> Option<Appearance> {
        self.0.as_ref()?.try_iter().filter_map(from_os).last()
    }
}
