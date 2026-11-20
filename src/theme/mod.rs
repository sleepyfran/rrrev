//! Colour themes: the terminal's own colours, or one of the doom-themes
//! ported in `doom.rs`. Each theme has a dark and a light variant, and the
//! OS's mode picks between them: you choose the theme, not the variant. A
//! theme gives the UI its [`Palette`] and the highlighter its syntax colours.

pub mod doom;

use std::str::FromStr;

use ratatui::style::{Color, Style};
use syntect::highlighting::{self, FontStyle, ScopeSelectors, StyleModifier, ThemeItem, ThemeSettings};

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

const fn doom(id: &'static str, name: &'static str, dark: (&'static str, &'static Doom), light: (&'static str, &'static Doom)) -> Theme {
    Theme { id, name, variants: Some(Variants { dark, light }) }
}

/// Every theme, in the order the picker lists them. Only doom themes that
/// come in both a dark and a light variant are here.
pub static THEMES: &[Theme] = &[
    Theme { id: "terminal", name: "Terminal", variants: None },
    doom("doom-one", "Doom One", ("doom-one", &doom::ONE), ("doom-one-light", &doom::ONE_LIGHT)),
    doom("doom-nord", "Nord", ("doom-nord", &doom::NORD), ("doom-nord-light", &doom::NORD_LIGHT)),
    doom("doom-gruvbox", "Gruvbox", ("doom-gruvbox", &doom::GRUVBOX), ("doom-gruvbox-light", &doom::GRUVBOX_LIGHT)),
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
            is(t.id) || t.variants.as_ref().is_some_and(|v| is(v.dark.0) || is(v.light.0))
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
        self.variant(appearance).map_or(Palette::TERMINAL, Palette::from_doom)
    }

    /// Its syntax colours for syntect; `None` for the terminal's theme, which
    /// leaves the choice to the highlighter.
    pub fn syntax(&self, appearance: Appearance) -> Option<highlighting::Theme> {
        self.variant(appearance).map(|d| syntax_theme(self.name, d))
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
            tints: Some((rgb(blend(d.vc_added, d.bg, 0.3)), rgb(blend(d.vc_deleted, d.bg, 0.3)))),
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
        if c <= 0.039_28 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(c.0) + 0.7152 * linear(c.1) + 0.0722 * linear(c.2)
}

/// A syntect theme painting TextMate scopes with the doom theme's faces,
/// matched up the way its Emacs font-lock faces are.
fn syntax_theme(name: &str, d: &Doom) -> highlighting::Theme {
    let none = FontStyle::empty();
    // As doom's `font-lock-property-name-face`.
    let property = blend(d.keywords, d.fg, 0.6);
    let rules: &[(&str, Rgb, FontStyle)] = &[
        ("comment, punctuation.definition.comment", d.comments, none),
        ("comment.block.documentation, comment.line.documentation, comment.line.double-slash.documentation", d.doc_comments, none),
        ("string, punctuation.definition.string", d.strings, none),
        ("constant.character.escape, string.regexp", d.operators, FontStyle::BOLD),
        ("constant.numeric", d.numbers, none),
        ("constant.language, constant.other, support.constant, variable.other.constant, entity.name.constant", d.constants, none),
        ("keyword, storage, keyword.control", d.keywords, none),
        ("keyword.operator", d.operators, none),
        (
            "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.union, \
             entity.name.trait, entity.name.interface, entity.other.inherited-class, support.type, support.class, \
             storage.type.primitive",
            d.types,
            none,
        ),
        ("entity.name.function, support.function, variable.function", d.functions, none),
        ("meta.method-call entity.name.function, meta.function-call.method variable.function, variable.function.method", d.methods, none),
        ("variable.parameter, entity.name.variable", d.variables, none),
        ("variable.language, support.function.builtin, support.type.builtin, support.variable", d.builtin, none),
        ("meta.annotation, meta.attribute, entity.name.function.decorator, meta.preprocessor", d.operators, none),
        ("entity.name.tag", d.methods, none),
        ("entity.other.attribute-name", d.types, none),
        ("support.type.property-name, meta.mapping.key string, entity.name.tag.yaml", property, none),
        ("markup.heading, markup.heading entity.name", d.keywords, FontStyle::BOLD),
        ("markup.bold", d.fg, FontStyle::BOLD),
        ("markup.italic", d.fg, FontStyle::ITALIC),
        ("markup.raw, markup.inline.raw", d.strings, none),
        ("markup.underline.link, string.other.link", d.highlight, FontStyle::UNDERLINE),
        ("markup.inserted", d.vc_added, none),
        ("markup.deleted", d.vc_deleted, none),
        ("markup.changed", d.vc_modified, none),
        ("invalid", d.red, none),
    ];
    let color = |c: Rgb| highlighting::Color { r: c.0, g: c.1, b: c.2, a: 0xff };
    highlighting::Theme {
        name: Some(name.to_string()),
        author: None,
        settings: ThemeSettings { foreground: Some(color(d.fg)), background: Some(color(d.bg)), ..Default::default() },
        scopes: rules
            .iter()
            .map(|&(scope, c, font_style)| ThemeItem {
                scope: ScopeSelectors::from_str(scope).expect("scope selectors parse"),
                style: StyleModifier {
                    foreground: Some(color(c)),
                    background: None,
                    font_style: (!font_style.is_empty()).then_some(font_style),
                },
            })
            .collect(),
    }
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
        assert_eq!(Theme::find("Tomorrow-Day").map(|t| t.name), Some("Tomorrow"));
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
                let syntax = theme.syntax(appearance).expect("doom themes bring syntax colours");
                assert!(syntax.scopes.len() > 10, "{}", theme.id);
                let d = theme.variant(appearance).unwrap();
                assert_eq!(luminance(d.bg) > 0.5, appearance == Appearance::Light, "{} {appearance:?}", theme.id);
                let Palette { yellow: Color::Rgb(r, g, b), .. } = theme.palette(appearance) else { panic!() };
                assert!(contrast(Rgb(r, g, b), d.bg) >= 3.0, "{} {appearance:?} yellow", theme.id);
            }
        }
    }

    #[test]
    fn terminal_theme_keeps_the_terminal_colours() {
        let palette = Theme::terminal().palette(Appearance::Light);
        assert_eq!(palette.bg, Color::Reset);
        assert!(Theme::terminal().syntax(Appearance::Dark).is_none());
    }
}
