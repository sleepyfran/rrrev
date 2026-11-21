//! Syntax highlighting for diff lines, with `syntect` and bat's syntaxes
//! from `two-face`. Hunks are highlighted the first time they're drawn and
//! kept, as the diff never changes.

use std::collections::HashMap;

use ratatui::style::{Color, Modifier, Style};
use syntect::easy::HighlightLines;
use syntect::highlighting::{self, FontStyle, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::appearance::Appearance;
use crate::diff::{Diff, Hunk, LineKind};
use crate::theme::Theme;

/// syntect's theme for the terminal's own colours, in the OS's shade.
fn base16(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::Dark => "base16-ocean.dark",
        Appearance::Light => "base16-ocean.light",
    }
}

/// Longer lines (minified code, data) are left plain: highlighting them is
/// slow and reading them is hopeless anyway.
const MAX_LINE_LEN: usize = 2000;

/// A run of a line's text and the style to draw it in.
pub type Segment = (Style, String);

pub struct Highlighter {
    syntaxes: SyntaxSet,
    /// The app's theme the syntax colours come from.
    source: &'static Theme,
    theme: highlighting::Theme,
    /// The theme's tints for changed words, if it has its own.
    tints: Option<(Color, Color)>,
    appearance: Appearance,
    /// Per `(file, hunk)`: each line's segments, empty for lines left plain.
    cache: HashMap<(usize, usize), Vec<Vec<Segment>>>,
    /// Per file: each line of its whole text, for context shown around hunks.
    sources: HashMap<usize, Vec<Vec<Segment>>>,
    /// Per path and side: each line of a file shown whole in the file view.
    files: HashMap<(String, bool), Vec<Vec<Segment>>>,
}

/// Files longer than this are shown as plain text around hunks, rather than
/// highlighted whole.
const MAX_SOURCE_LINES: usize = 20_000;

impl Highlighter {
    pub fn new(theme: &'static Theme, appearance: Appearance) -> Self {
        Self {
            syntaxes: two_face::syntax::extra_newlines(),
            source: theme,
            theme: syntax_theme(theme, appearance),
            tints: theme.palette(appearance).tints,
            appearance,
            cache: HashMap::new(),
            sources: HashMap::new(),
            files: HashMap::new(),
        }
    }

    /// Follows the app's theme and the OS's mode, highlighting again in
    /// their colours when either changes.
    pub fn set_theme(&mut self, theme: &'static Theme, appearance: Appearance) {
        if theme != self.source || appearance != self.appearance {
            self.source = theme;
            self.appearance = appearance;
            self.theme = syntax_theme(theme, appearance);
            self.tints = theme.palette(appearance).tints;
            self.cache.clear();
            self.sources.clear();
            self.files.clear();
        }
    }

    /// The tint behind words that changed within a changed line.
    pub fn emphasis(&self, kind: LineKind) -> Style {
        if let Some((added, removed)) = self.tints {
            return Style::new().bg(if kind == LineKind::Removed { removed } else { added });
        }
        let bg = match (self.appearance, kind) {
            (Appearance::Dark, LineKind::Removed) => Color::Rgb(110, 38, 44),
            (Appearance::Dark, _) => Color::Rgb(30, 92, 50),
            (Appearance::Light, LineKind::Removed) => Color::Rgb(255, 196, 196),
            (Appearance::Light, _) => Color::Rgb(188, 240, 196),
        };
        Style::new().bg(bg)
    }

    /// The segments of line `no` (1-based) of a file's whole text, for
    /// context shown around its hunks. The file is highlighted top to bottom
    /// the first time, so lines start in the right state.
    pub fn source_line(&mut self, diff: &Diff, file: usize, no: u32) -> &[Segment] {
        if !self.sources.contains_key(&file) {
            let lines = match diff.context_source(file) {
                Some((source, _)) if source.len() <= MAX_SOURCE_LINES => {
                    self.highlight_source(diff.files[file].path(), source)
                }
                _ => Vec::new(),
            };
            self.sources.insert(file, lines);
        }
        let index = (no as usize).checked_sub(1);
        index.and_then(|i| self.sources[&file].get(i)).map_or(&[], Vec::as_slice)
    }

    /// The segments of line `index` (0-based) of a file shown whole, as
    /// `source_line` does for context; `old` tells the two sides of a file apart.
    pub fn file_line(&mut self, path: &str, old: bool, source: &[String], index: usize) -> &[Segment] {
        let key = (path.to_string(), old);
        if !self.files.contains_key(&key) {
            let lines =
                if source.len() <= MAX_SOURCE_LINES { self.highlight_source(path, source) } else { Vec::new() };
            self.files.insert(key.clone(), lines);
        }
        self.files[&key].get(index).map_or(&[], Vec::as_slice)
    }

    fn highlight_source(&self, path: &str, source: &[String]) -> Vec<Vec<Segment>> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let syntax = name
            .rsplit_once('.')
            .and_then(|(_, ext)| self.syntaxes.find_syntax_by_extension(ext))
            .or_else(|| self.syntaxes.find_syntax_by_extension(name))
            .or_else(|| source.first().and_then(|l| self.syntaxes.find_syntax_by_first_line(l)))
            .filter(|s| s.name != "Plain Text");
        let Some(syntax) = syntax else { return Vec::new() };
        let mut parser = HighlightLines::new(syntax, &self.theme);
        source
            .iter()
            .map(|line| {
                let text = format!("{line}\n");
                match parser.highlight_line(&text, &self.syntaxes) {
                    Ok(ranges) if line.len() <= MAX_LINE_LEN => ranges
                        .into_iter()
                        .map(|(style, text)| (self.style(style), text.trim_end_matches('\n').to_string()))
                        .filter(|(_, text)| !text.is_empty())
                        .collect(),
                    _ => Vec::new(),
                }
            })
            .collect()
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

/// The theme's own syntax colours for the OS's mode, or for the terminal's
/// theme syntect's base16 ocean in that shade.
fn syntax_theme(theme: &Theme, appearance: Appearance) -> highlighting::Theme {
    if let Some(theme) = theme.syntax(appearance) {
        return theme;
    }
    let mut themes = ThemeSet::load_defaults();
    let mut theme = themes.themes.remove(base16(appearance)).expect("syntect ships this theme");
    // The light theme keeps the dark one's pale accents, which wash out on
    // a white background.
    if appearance == Appearance::Light {
        for scope in &mut theme.scopes {
            scope.style.foreground = scope.style.foreground.map(readable_on_white);
        }
    }
    theme
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
        let mut highlighter = Highlighter::new(Theme::terminal(), Appearance::Dark);
        let segments = highlighter.line(&diff, 0, 0, 0);
        assert_eq!(text(segments), "fn main() {}");
        assert!(segments.iter().any(|(style, _)| style.fg.is_some()));
    }

    #[test]
    fn knows_typescript_and_toml() {
        for path in ["web/app.ts", "web/App.tsx", "Cargo.toml"] {
            let diff = diff(path, &[(LineKind::Added, "const a = 1")]);
            assert!(!Highlighter::new(Theme::terminal(), Appearance::Dark).line(&diff, 0, 0, 0).is_empty(), "{path}");
        }
    }

    #[test]
    fn light_theme_is_readable_on_white() {
        let diff = diff("a.rs", &[(LineKind::Added, "fn main() { let s = \"x\"; } // done")]);
        let mut highlighter = Highlighter::new(Theme::terminal(), Appearance::Light);
        let colors: Vec<_> = highlighter.line(&diff, 0, 0, 0).iter().filter_map(|(style, _)| style.fg).collect();
        assert!(colors.len() > 1);
        for color in colors {
            let Color::Rgb(r, g, b) = color else { panic!("{color:?}") };
            assert!(contrast_with_white(highlighting::Color { r, g, b, a: 255 }) >= 4.5, "{color:?}");
        }
    }

    #[test]
    fn highlights_whole_sources_for_context() {
        use crate::diff::Sources;
        let mut diff = diff("a.rs", &[(LineKind::Added, "x")]);
        diff.sources = vec![Sources { old: None, new: Some(std::sync::Arc::new(vec!["/*".into(), "fn a() {}".into()])) }];
        let mut highlighter = Highlighter::new(Theme::terminal(), Appearance::Dark);
        assert_eq!(text(highlighter.source_line(&diff, 0, 2)), "fn a() {}");
        // Line 2 is inside the comment line 1 opened.
        let inside = highlighter.source_line(&diff, 0, 2).to_vec();
        let plain = Highlighter::new(Theme::terminal(), Appearance::Dark).line(&super::tests::diff("a.rs", &[(LineKind::Added, "fn a() {}")]), 0, 0, 0).to_vec();
        assert_ne!(inside, plain);
        assert!(highlighter.source_line(&diff, 0, 0).is_empty());
        assert!(highlighter.source_line(&diff, 0, 9).is_empty());
    }

    #[test]
    fn follows_the_theme() {
        let diff = diff("a.rs", &[(LineKind::Added, "fn main() {}")]);
        let mut highlighter = Highlighter::new(Theme::terminal(), Appearance::Dark);
        let before = highlighter.line(&diff, 0, 0, 0).to_vec();
        let one = Theme::find("doom-one").unwrap();
        highlighter.set_theme(one, Appearance::Dark);
        let after = highlighter.line(&diff, 0, 0, 0).to_vec();
        assert_eq!(text(&after), "fn main() {}");
        assert_ne!(before, after);
        // doom-one's keywords are its blue; doom-one-light's its red.
        assert_eq!(after[0].0.fg, Some(Color::Rgb(0x51, 0xaf, 0xef)));
        highlighter.set_theme(one, Appearance::Light);
        assert_eq!(highlighter.line(&diff, 0, 0, 0)[0].0.fg, Some(Color::Rgb(0xe4, 0x56, 0x49)));
        assert_ne!(highlighter.emphasis(LineKind::Added), Highlighter::new(Theme::terminal(), Appearance::Dark).emphasis(LineKind::Added));
    }

    #[test]
    fn leaves_unknown_files_plain() {
        let diff = diff("notes.unknownext", &[(LineKind::Added, "fn main() {}")]);
        assert!(Highlighter::new(Theme::terminal(), Appearance::Dark).line(&diff, 0, 0, 0).is_empty());
    }

    #[test]
    fn context_lines_carry_state_into_both_sides() {
        let diff = diff(
            "a.rs",
            &[(LineKind::Context, "/*"), (LineKind::Removed, "fn a() {}"), (LineKind::Added, "fn a() {}")],
        );
        let plain = super::tests::diff("a.rs", &[(LineKind::Added, "fn a() {}")]);
        let mut highlighter = Highlighter::new(Theme::terminal(), Appearance::Dark);
        let removed = highlighter.line(&diff, 0, 0, 1).to_vec();
        let added = highlighter.line(&diff, 0, 0, 2).to_vec();
        // Both sides are inside the comment the context line opened.
        assert_eq!(removed, added);
        assert_ne!(added, Highlighter::new(Theme::terminal(), Appearance::Dark).line(&plain, 0, 0, 0));
    }
}
