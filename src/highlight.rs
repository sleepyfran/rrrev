//! Syntax highlighting for diff lines, with `syntect` and bat's syntaxes
//! from `two-face`. Hunks are highlighted the first time they're drawn and
//! kept, as the diff never changes.

use std::collections::HashMap;

use ratatui::style::{Color, Modifier, Style};
use syntect::easy::HighlightLines;
use syntect::highlighting::{self, FontStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::diff::{Diff, Hunk, LineKind};

/// Whether the terminal draws on a dark or a light background, which picks
/// the theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    Dark,
    Light,
}

impl Background {
    /// `RRREV_THEME=light|dark` wins; otherwise the terminal is asked for its
    /// background colour, assuming dark when it doesn't answer. Asking needs
    /// the terminal to itself, so this runs before the UI takes it over.
    pub fn detect() -> Self {
        match std::env::var("RRREV_THEME").as_deref() {
            Ok("light") => return Background::Light,
            Ok("dark") => return Background::Dark,
            _ => {}
        }
        match terminal_colorsaurus::theme_mode(Default::default()) {
            Ok(terminal_colorsaurus::ThemeMode::Light) => Background::Light,
            _ => Background::Dark,
        }
    }

    fn theme(self) -> &'static str {
        match self {
            Background::Dark => "base16-ocean.dark",
            Background::Light => "base16-ocean.light",
        }
    }
}

/// Longer lines (minified code, data) are left plain: highlighting them is
/// slow and reading them is hopeless anyway.
const MAX_LINE_LEN: usize = 2000;

/// A run of a line's text and the style to draw it in.
pub type Segment = (Style, String);

pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
    background: Background,
    /// Per `(file, hunk)`: each line's segments, empty for lines left plain.
    cache: HashMap<(usize, usize), Vec<Vec<Segment>>>,
}

impl Highlighter {
    pub fn new(background: Background) -> Self {
        let mut themes = ThemeSet::load_defaults();
        let mut theme = themes.themes.remove(background.theme()).expect("syntect ships this theme");
        // The light theme keeps the dark one's pale accents, which wash out on
        // a white background.
        if background == Background::Light {
            for scope in &mut theme.scopes {
                scope.style.foreground = scope.style.foreground.map(readable_on_white);
            }
        }
        Self { syntaxes: two_face::syntax::extra_newlines(), theme, background, cache: HashMap::new() }
    }

    /// The tint behind words that changed within a changed line.
    pub fn emphasis(&self, kind: LineKind) -> Style {
        let bg = match (self.background, kind) {
            (Background::Dark, LineKind::Removed) => Color::Rgb(110, 38, 44),
            (Background::Dark, _) => Color::Rgb(30, 92, 50),
            (Background::Light, LineKind::Removed) => Color::Rgb(255, 196, 196),
            (Background::Light, _) => Color::Rgb(188, 240, 196),
        };
        Style::new().bg(bg)
    }

    /// The segments of one diff line; empty when it should be drawn plain,
    /// e.g. in a language there's no syntax for.
    pub fn line(&mut self, diff: &Diff, file: usize, hunk: usize, line: usize) -> &[Segment] {
        if !self.cache.contains_key(&(file, hunk)) {
            let f = &diff.files[file];
            let lines = self.highlight_hunk(f.path(), &f.hunks[hunk]);
            self.cache.insert((file, hunk), lines);
        }
        &self.cache[&(file, hunk)][line]
    }

    /// Runs one parser over the old side (context and removed lines) and
    /// another over the new side (context and added lines), so each side reads
    /// as the code it was. Hunks start from a blank state, so one that begins
    /// inside a comment or string can be off until the parser catches up.
    fn highlight_hunk(&self, path: &str, hunk: &Hunk) -> Vec<Vec<Segment>> {
        let Some(syntax) = self.syntax_for(path, hunk) else { return vec![Vec::new(); hunk.lines.len()] };
        let mut old = HighlightLines::new(syntax, &self.theme);
        let mut new = HighlightLines::new(syntax, &self.theme);
        hunk.lines
            .iter()
            .map(|line| {
                if line.content.len() > MAX_LINE_LEN {
                    return Vec::new();
                }
                let text = format!("{}\n", line.content);
                let ranges = match line.kind {
                    LineKind::Added => new.highlight_line(&text, &self.syntaxes),
                    LineKind::Removed => old.highlight_line(&text, &self.syntaxes),
                    LineKind::Context => {
                        let _ = old.highlight_line(&text, &self.syntaxes);
                        new.highlight_line(&text, &self.syntaxes)
                    }
                    LineKind::NoNewline => return Vec::new(),
                };
                let Ok(ranges) = ranges else { return Vec::new() };
                ranges
                    .into_iter()
                    .map(|(style, text)| (self.style(style), text.trim_end_matches('\n').to_string()))
                    .filter(|(_, text)| !text.is_empty())
                    .collect()
            })
            .collect()
    }

    /// By extension, then by whole file name (`Makefile`), then by a first line
    /// such as a shebang when the hunk starts at the top of the file.
    fn syntax_for(&self, path: &str, hunk: &Hunk) -> Option<&SyntaxReference> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let by_extension = name.rsplit_once('.').and_then(|(_, ext)| self.syntaxes.find_syntax_by_extension(ext));
        let by_first_line = || {
            let first = hunk.lines.first().filter(|l| l.new_no == Some(1) || l.old_no == Some(1))?;
            self.syntaxes.find_syntax_by_first_line(&first.content)
        };
        by_extension
            .or_else(|| self.syntaxes.find_syntax_by_extension(name))
            .or_else(by_first_line)
            .filter(|s| s.name != "Plain Text")
    }

    /// Foreground and font only: backgrounds stay free for the cursor and
    /// selection, and text in the theme's default colour keeps the terminal's.
    fn style(&self, style: highlighting::Style) -> Style {
        let mut out = Style::new();
        if Some(style.foreground) != self.theme.settings.foreground {
            let c = style.foreground;
            out = out.fg(Color::Rgb(c.r, c.g, c.b));
        }
        for (font, modifier) in [
            (FontStyle::BOLD, Modifier::BOLD),
            (FontStyle::ITALIC, Modifier::ITALIC),
            (FontStyle::UNDERLINE, Modifier::UNDERLINED),
        ] {
            if style.font_style.contains(font) {
                out = out.add_modifier(modifier);
            }
        }
        out
    }
}

/// Darkens `color`, keeping its hue, until it has the 4.5:1 contrast with
/// white that WCAG asks of body text.
fn readable_on_white(mut color: highlighting::Color) -> highlighting::Color {
    while contrast_with_white(color) < 4.5 {
        let darker = |c: u8| (f32::from(c) * 0.9) as u8;
        color = highlighting::Color { r: darker(color.r), g: darker(color.g), b: darker(color.b), a: color.a };
    }
    color
}

fn contrast_with_white(color: highlighting::Color) -> f32 {
    let linear = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.039_28 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let luminance = 0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b);
    1.05 / (luminance + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{DiffLine, FileDiff, FileStatus, HunkId};

    fn diff(path: &str, lines: &[(LineKind, &str)]) -> Diff {
        let lines = lines
            .iter()
            .enumerate()
            .map(|(i, (kind, content))| {
                let no = Some(i as u32 + 10);
                DiffLine { kind: *kind, old_no: no, new_no: no, content: content.to_string() }
            })
            .collect();
        let hunk = Hunk {
            id: HunkId::default(),
            header: String::new(),
            old_start: 10,
            old_lines: 0,
            new_start: 10,
            new_lines: 0,
            lines,
        };
        let file = FileDiff {
            old_path: Some(path.into()),
            new_path: Some(path.into()),
            status: FileStatus::Modified,
            is_binary: false,
            hunks: vec![hunk],
        };
        Diff::new(vec![file])
    }

    fn text(segments: &[Segment]) -> String {
        segments.iter().map(|(_, t)| t.as_str()).collect()
    }

    #[test]
    fn highlights_known_languages_and_keeps_the_text() {
        let diff = diff("src/main.rs", &[(LineKind::Added, "fn main() {}")]);
        let mut highlighter = Highlighter::new(Background::Dark);
        let segments = highlighter.line(&diff, 0, 0, 0);
        assert_eq!(text(segments), "fn main() {}");
        assert!(segments.iter().any(|(style, _)| style.fg.is_some()));
    }

    #[test]
    fn knows_typescript_and_toml() {
        for path in ["web/app.ts", "web/App.tsx", "Cargo.toml"] {
            let diff = diff(path, &[(LineKind::Added, "const a = 1")]);
            assert!(!Highlighter::new(Background::Dark).line(&diff, 0, 0, 0).is_empty(), "{path}");
        }
    }

    #[test]
    fn light_theme_is_readable_on_white() {
        let diff = diff("a.rs", &[(LineKind::Added, "fn main() { let s = \"x\"; } // done")]);
        let mut highlighter = Highlighter::new(Background::Light);
        let colors: Vec<_> = highlighter.line(&diff, 0, 0, 0).iter().filter_map(|(style, _)| style.fg).collect();
        assert!(colors.len() > 1);
        for color in colors {
            let Color::Rgb(r, g, b) = color else { panic!("{color:?}") };
            assert!(contrast_with_white(highlighting::Color { r, g, b, a: 255 }) >= 4.5, "{color:?}");
        }
    }

    #[test]
    fn leaves_unknown_files_plain() {
        let diff = diff("notes.unknownext", &[(LineKind::Added, "fn main() {}")]);
        assert!(Highlighter::new(Background::Dark).line(&diff, 0, 0, 0).is_empty());
    }

    #[test]
    fn context_lines_carry_state_into_both_sides() {
        let diff = diff(
            "a.rs",
            &[(LineKind::Context, "/*"), (LineKind::Removed, "fn a() {}"), (LineKind::Added, "fn a() {}")],
        );
        let plain = super::tests::diff("a.rs", &[(LineKind::Added, "fn a() {}")]);
        let mut highlighter = Highlighter::new(Background::Dark);
        let removed = highlighter.line(&diff, 0, 0, 1).to_vec();
        let added = highlighter.line(&diff, 0, 0, 2).to_vec();
        // Both sides are inside the comment the context line opened.
        assert_eq!(removed, added);
        assert_ne!(added, Highlighter::new(Background::Dark).line(&plain, 0, 0, 0));
    }
}
