//! Colour themes: the terminal's own colours, or one of the doom-themes (or
//! GitHub's) ported in `doom.rs`. Each theme has a dark and a light variant, and the
//! OS's mode picks between them: you choose the theme, not the variant. A
//! theme gives the UI its [`Palette`] and the highlighter its syntax colours.

pub mod doom;

use ratatui::style::{Color, Modifier, Style};

use crate::appearance::Appearance;
use doom::{Doom, Rgb, blend};

#[derive(Debug, PartialEq, Eq)]
pub struct Theme {
    /// What `:theme` and the config file call it.
    pub id: &'static str,
    pub name: &'static str,
    /// `None` for the terminal's own colours.
    variants: Option<Variants>,
}

/// A doom theme's dark and light variants, with doom's own names for them.
#[derive(Debug, PartialEq, Eq)]
struct Variants {
    dark: (&'static str, &'static Doom),
    light: (&'static str, &'static Doom),
}

const fn doom(
    id: &'static str,
    name: &'static str,
    dark: (&'static str, &'static Doom),
    light: (&'static str, &'static Doom),
) -> Theme {
    Theme {
        id,
        name,
        variants: Some(Variants { dark, light }),
    }
}

/// Every theme, in the order the picker lists them. Only themes that come in
/// both a dark and a light variant are here.
pub static THEMES: &[Theme] = &[
    Theme {
        id: "terminal",
        name: "Terminal",
        variants: None,
    },
    doom(
        "doom-one",
        "Doom One",
        ("doom-one", &doom::ONE),
        ("doom-one-light", &doom::ONE_LIGHT),
    ),
    doom(
        "doom-nord",
        "Nord",
        ("doom-nord", &doom::NORD),
        ("doom-nord-light", &doom::NORD_LIGHT),
    ),
    doom(
        "doom-gruvbox",
        "Gruvbox",
        ("doom-gruvbox", &doom::GRUVBOX),
        ("doom-gruvbox-light", &doom::GRUVBOX_LIGHT),
    ),
    doom(
        "doom-solarized",
        "Solarized",
        ("doom-solarized-dark", &doom::SOLARIZED_DARK),
        ("doom-solarized-light", &doom::SOLARIZED_LIGHT),
    ),
    doom(
        "doom-tomorrow",
        "Tomorrow",
        ("doom-tomorrow-night", &doom::TOMORROW_NIGHT),
        ("doom-tomorrow-day", &doom::TOMORROW_DAY),
    ),
    doom(
        "github",
        "GitHub",
        ("github-dark", &doom::GITHUB_DARK),
        ("github-light", &doom::GITHUB_LIGHT),
    ),
];

impl Theme {
    /// The terminal's own colours.
    pub fn terminal() -> &'static Theme {
        &THEMES[0]
    }

    /// By id, or by the doom name of either variant, each with or without
    /// its `doom-` prefix, ignoring case: `nord`, `doom-nord-light`.
    pub fn find(name: &str) -> Option<&'static Theme> {
        let name = name.trim().to_ascii_lowercase();
        let is = |id: &str| id == name || id.strip_prefix("doom-") == Some(name.as_str());
        THEMES.iter().find(|t| {
            is(t.id)
                || t.variants
                    .as_ref()
                    .is_some_and(|v| is(v.dark.0) || is(v.light.0))
        })
    }

    /// Where it sits in [`THEMES`].
    pub fn index(&self) -> usize {
        THEMES.iter().position(|t| t.id == self.id).unwrap_or(0)
    }

    /// The doom variant for the OS's mode.
    fn variant(&self, appearance: Appearance) -> Option<&'static Doom> {
        let v = self.variants.as_ref()?;
        Some(match appearance {
            Appearance::Dark => v.dark.1,
            Appearance::Light => v.light.1,
        })
    }

    pub fn palette(&self, appearance: Appearance) -> Palette {
        self.variant(appearance)
            .map_or(Palette::TERMINAL, Palette::from_doom)
    }

    /// Its syntax colours. The terminal's theme takes base16 Ocean's, in the
    /// OS's shade.
    pub fn syntax(&self, appearance: Appearance) -> Syntax {
        match (self.variant(appearance), appearance) {
            (Some(d), _) => Syntax::from_doom(d, |c| c),
            (None, Appearance::Dark) => Syntax::from_doom(&doom::OCEAN, |c| c),
            // Ocean's light shade keeps the dark one's pale accents, which
            // wash out on a white background.
            (None, Appearance::Light) => Syntax::from_doom(&doom::OCEAN_LIGHT, readable_on_white),
        }
    }
}

/// The colours the UI draws with, by what they're for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Text and what it's drawn on; `Reset` keeps the terminal's.
    pub fg: Color,
    pub bg: Color,
    /// Borders, separators and what's greyed out behind a pop-up.
    pub muted: Color,
    /// Grey that still reads as text.
    pub faint: Color,
    /// Behind the cursor's row, and the sidebar's selected entry.
    pub cursor: Color,
    /// Behind a visual selection.
    pub selection: Color,
    /// The title and status lines, `(fg, bg)`; `None` draws them reversed.
    pub bar: Option<(Color, Color)>,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub blue: Color,
    pub magenta: Color,
    pub cyan: Color,
    /// Text on an accent's background, like a search match.
    pub on_accent: Color,
    /// Behind the words that changed in an added and a removed line; `None`
    /// leaves it to the highlighter, which knows the terminal's background.
    pub tints: Option<(Color, Color)>,
}

impl Palette {
    const TERMINAL: Palette = Palette {
        fg: Color::Reset,
        bg: Color::Reset,
        muted: Color::DarkGray,
        faint: Color::Gray,
        cursor: Color::DarkGray,
        selection: Color::Blue,
        bar: None,
        red: Color::Red,
        green: Color::Green,
        yellow: Color::Yellow,
        blue: Color::Blue,
        magenta: Color::Magenta,
        cyan: Color::Cyan,
        on_accent: Color::Black,
        tints: None,
    };

    /// Accents are the theme's own, nudged toward its text colour where
    /// they'd be too faint to read on its background (pale yellows on white).
    fn from_doom(d: &Doom) -> Palette {
        let accent = |c: Rgb| rgb(readable(c, d.bg, d.fg));
        Palette {
            fg: rgb(d.fg),
            bg: rgb(d.bg),
            muted: rgb(blend(d.fg, d.bg, 0.4)),
            faint: rgb(blend(d.fg, d.bg, 0.6)),
            cursor: rgb(d.region),
            selection: rgb(blend(d.highlight, d.bg, 0.35)),
            bar: Some((rgb(d.fg), rgb(d.bg_alt))),
            red: accent(d.red),
            green: accent(d.green),
            yellow: accent(d.yellow),
            blue: accent(d.blue),
            magenta: accent(d.magenta),
            cyan: accent(d.cyan),
            on_accent: rgb(d.bg),
            // As magit's highlighted diff lines, a little stronger.
            tints: Some((
                rgb(blend(d.vc_added, d.bg, 0.3)),
                rgb(blend(d.vc_deleted, d.bg, 0.3)),
            )),
        }
    }

    /// What everything is drawn on.
    pub fn base(&self) -> Style {
        Style::new().fg(self.fg).bg(self.bg)
    }

    /// The title and status lines.
    pub fn bar(&self) -> Style {
        match self.bar {
            Some((fg, bg)) => Style::new().fg(fg).bg(bg),
            None => Style::new().reversed(),
        }
    }
}

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// `color`, moved toward `fg` until it has 3:1 contrast with `bg`, which
/// WCAG asks of large text and UI marks.
fn readable(color: Rgb, bg: Rgb, fg: Rgb) -> Rgb {
    let mut out = color;
    let mut toward = 0.0;
    while contrast(out, bg) < 3.0 && toward < 1.0 {
        toward += 0.1;
        out = blend(fg, color, toward);
    }
    out
}

pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn luminance(c: Rgb) -> f32 {
    let linear = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(c.0) + 0.7152 * linear(c.1) + 0.0722 * linear(c.2)
}

/// Colours for tree-sitter's highlight captures (`keyword`,
/// `function.method`, …), matched up with a doom theme's faces the way its
/// Emacs font-lock faces are.
#[derive(Debug, Clone, PartialEq)]
pub struct Syntax {
    rules: Vec<(&'static str, Style)>,
}

impl Syntax {
    fn from_doom(d: &Doom, adjust: fn(Rgb) -> Rgb) -> Syntax {
        let bold = Modifier::BOLD;
        let none = Modifier::empty();
        let faces: &[(&[&'static str], Rgb, Modifier)] = &[
            (&["comment"], d.comments, none),
            (&["comment.documentation"], d.doc_comments, none),
            (&["string", "character"], d.strings, none),
            (
                &[
                    "string.escape",
                    "escape",
                    "string.regexp",
                    "string.special.regex",
                    "character.special",
                ],
                d.operators,
                bold,
            ),
            (&["string.special.symbol"], d.constants, none),
            (&["number", "float"], d.numbers, none),
            (
                &["constant", "constant.builtin", "constant.macro", "boolean"],
                d.constants,
                none,
            ),
            (
                &["keyword", "conditional", "repeat", "exception", "include"],
                d.keywords,
                none,
            ),
            (&["operator"], d.operators, none),
            (
                &[
                    "type",
                    "type.builtin",
                    "type.definition",
                    "constructor",
                    "module",
                    "namespace",
                ],
                d.types,
                none,
            ),
            (
                &[
                    "function",
                    "function.call",
                    "function.macro",
                    "function.special",
                ],
                d.functions,
                none,
            ),
            (
                &["function.method", "function.member", "method"],
                d.methods,
                none,
            ),
            (&["variable.parameter", "parameter"], d.variables, none),
            (
                &[
                    "variable.builtin",
                    "function.builtin",
                    "module.builtin",
                    "variable.parameter.builtin",
                ],
                d.builtin,
                none,
            ),
            (
                &[
                    "attribute",
                    "label",
                    "preproc",
                    "keyword.directive",
                    "punctuation.special",
                    "embedded",
                ],
                d.operators,
                none,
            ),
            (&["tag"], d.tags, none),
            (&["tag.attribute"], d.types, none),
            (
                &[
                    "property",
                    "property.definition",
                    "field",
                    "variable.member",
                ],
                d.properties,
                none,
            ),
        ];
        let rules = faces
            .iter()
            .flat_map(|&(names, color, modifier)| {
                // Text in the theme's own colour keeps the terminal's.
                let mut style = Style::new().add_modifier(modifier);
                if color != d.fg {
                    style = style.fg(rgb(adjust(color)));
                }
                names.iter().map(move |&name| (name, style))
            })
            .collect();
        Syntax { rules }
    }

    /// The style of a capture: its own rule, or its nearest parent's, so
    /// `function.method.call` falls back to `function.method`, then
    /// `function`. `None` leaves the text plain.
    pub fn style(&self, capture: &str) -> Option<Style> {
        let mut name = capture;
        loop {
            if let Some(&(_, style)) = self.rules.iter().find(|(n, _)| *n == name) {
                return Some(style);
            }
            name = name.rsplit_once('.')?.0;
        }
    }
}

/// Darkens `color`, keeping its hue, until it has the 4.5:1 contrast with
/// white that WCAG asks of body text.
pub fn readable_on_white(mut color: Rgb) -> Rgb {
    while contrast(color, Rgb(255, 255, 255)) < 4.5 {
        let darker = |c: u8| (f32::from(c) * 0.9) as u8;
        color = Rgb(darker(color.0), darker(color.1), darker(color.2));
    }
    color
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_themes_by_id_or_either_variant() {
        let nord = Theme::find("nord").unwrap();
        assert_eq!(nord.name, "Nord");
        assert_eq!(Theme::find("doom-nord"), Some(nord));
        assert_eq!(Theme::find("doom-nord-light"), Some(nord));
        assert_eq!(
            Theme::find("Tomorrow-Day").map(|t| t.name),
            Some("Tomorrow")
        );
        assert_eq!(Theme::find("terminal"), Some(Theme::terminal()));
        assert_eq!(Theme::find("dracula"), None);
    }

    #[test]
    fn ids_are_unique() {
        let ids: std::collections::HashSet<_> = THEMES.iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), THEMES.len());
    }

    #[test]
    fn every_theme_has_a_dark_and_a_light_variant_that_read() {
        for theme in &THEMES[1..] {
            for appearance in [Appearance::Dark, Appearance::Light] {
                let syntax = theme.syntax(appearance);
                assert!(syntax.rules.len() > 10, "{}", theme.id);
                let d = theme.variant(appearance).unwrap();
                assert_eq!(
                    luminance(d.bg) > 0.5,
                    appearance == Appearance::Light,
                    "{} {appearance:?}",
                    theme.id
                );
                let Palette {
                    yellow: Color::Rgb(r, g, b),
                    ..
                } = theme.palette(appearance)
                else {
                    panic!()
                };
                assert!(
                    contrast(Rgb(r, g, b), d.bg) >= 3.0,
                    "{} {appearance:?} yellow",
                    theme.id
                );
            }
        }
    }

    #[test]
    fn terminal_theme_keeps_the_terminal_colours() {
        let palette = Theme::terminal().palette(Appearance::Light);
        assert_eq!(palette.bg, Color::Reset);
        // Ocean's colours, with plain text left to the terminal.
        let syntax = Theme::terminal().syntax(Appearance::Dark);
        assert_eq!(
            syntax.style("keyword.function").and_then(|s| s.fg),
            Some(Color::Rgb(0xb4, 0x8e, 0xad))
        );
        assert_eq!(syntax.style("variable"), None);
        assert_eq!(syntax.style("punctuation.bracket"), None);
    }
}
