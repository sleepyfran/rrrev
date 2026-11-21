//! Syntax highlighting with tree-sitter, from each language's
//! `highlights.scm`. A file's whole text is highlighted when rrrev has it, so
//! every line starts in the right state; a hunk without it is highlighted on
//! its own, a side at a time. Either way it's done the first time it's drawn
//! and kept, as the diff never changes.

use std::collections::HashMap;
use std::ops::Range;

use ratatui::style::{Color, Style};
use tree_sitter::{Parser, QueryCursor, StreamingIterator};

use crate::appearance::Appearance;
use crate::diff::{Diff, Hunk, LineKind, Side};
use crate::languages::{Registry, grammar, query};
use crate::theme::{Syntax, Theme};

/// Longer lines (minified code, data) are left plain: reading them is
/// hopeless anyway.
const MAX_LINE_LEN: usize = 2000;

/// Files longer than this are shown as plain text around hunks, rather than
/// highlighted whole.
const MAX_SOURCE_LINES: usize = 20_000;

/// A run of a line's text and the style to draw it in.
pub type Segment = (Style, String);

pub struct Highlighter {
    /// The app's theme the syntax colours come from.
    source: &'static Theme,
    syntax: Syntax,
    /// The theme's tints for changed words, if it has its own.
    tints: Option<(Color, Color)>,
    appearance: Appearance,
    /// Per `(file, hunk)`: each line's segments, empty for lines left plain.
    cache: HashMap<(usize, usize), Vec<Vec<Segment>>>,
    /// Per file and side: each line of its whole text.
    sources: HashMap<(usize, Side), Vec<Vec<Segment>>>,
    /// Per path and side: each line of a file shown whole in the file view.
    files: HashMap<(String, bool), Vec<Vec<Segment>>>,
}

impl Highlighter {
    pub fn new(theme: &'static Theme, appearance: Appearance) -> Self {
        Self {
            source: theme,
            syntax: theme.syntax(appearance),
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
            self.syntax = theme.syntax(appearance);
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
    /// context shown around its hunks.
    pub fn source_line(&mut self, diff: &Diff, file: usize, no: u32) -> &[Segment] {
        match diff.context_source(file) {
            Some((_, side)) => self.side_line(diff, file, side, no),
            None => &[],
        }
    }

    /// The segments of line `index` (0-based) of a file shown whole, as
    /// `source_line` does for context; `old` tells the two sides of a file apart.
    pub fn file_line(&mut self, path: &str, old: bool, source: &[String], index: usize) -> &[Segment] {
        let syntax = &self.syntax;
        let lines = self.files.entry((path.to_string(), old)).or_insert_with(|| {
            if source.len() <= MAX_SOURCE_LINES { highlight(syntax, path, source).unwrap_or_default() } else { Vec::new() }
        });
        lines.get(index).map_or(&[], Vec::as_slice)
    }

    /// Line `no` (1-based) of one side of a file, highlighted with the rest
    /// of that side the first time it's asked for; empty if rrrev doesn't
    /// have that side's text.
    fn side_line(&mut self, diff: &Diff, file: usize, side: Side, no: u32) -> &[Segment] {
        let lines = self.sources.entry((file, side)).or_insert_with(|| {
            let sources = diff.sources.get(file);
            let source = match side {
                Side::Old => sources.and_then(|s| s.old.as_ref()),
                Side::New => sources.and_then(|s| s.new.as_ref()),
            };
            match source {
                Some(source) if source.len() <= MAX_SOURCE_LINES => {
                    highlight(&self.syntax, diff.files[file].path(), source).unwrap_or_default()
                }
                _ => Vec::new(),
            }
        });
        let index = (no as usize).checked_sub(1);
        index.and_then(|i| lines.get(i)).map_or(&[], Vec::as_slice)
    }

    fn has_side(diff: &Diff, file: usize, side: Side) -> bool {
        diff.sources.get(file).is_some_and(|s| match side {
            Side::Old => s.old.is_some(),
            Side::New => s.new.is_some(),
        })
    }

    /// The segments of one diff line; empty when it should be drawn plain,
    /// e.g. in a language there's no grammar for.
    pub fn line(&mut self, diff: &Diff, file: usize, hunk: usize, line: usize) -> &[Segment] {
        if !self.cache.contains_key(&(file, hunk)) {
            let lines = self.highlight_hunk(diff, file, hunk);
            self.cache.insert((file, hunk), lines);
        }
        &self.cache[&(file, hunk)][line]
    }

    /// Each line from its side's whole text when rrrev has it. Otherwise the
    /// old side (context and removed lines) and the new side (context and
    /// added lines) are highlighted apart, so each reads as the code it was;
    /// a hunk that begins inside a comment or string can then be off.
    fn highlight_hunk(&mut self, diff: &Diff, file: usize, hunk: usize) -> Vec<Vec<Segment>> {
        let path = diff.files[file].path();
        let h: &Hunk = &diff.files[file].hunks[hunk];
        let fragment = |side: Side| -> Option<HashMap<usize, Vec<Segment>>> {
            if Self::has_side(diff, file, side) {
                return None;
            }
            let on_side = |kind: LineKind| match side {
                Side::Old => kind != LineKind::Added,
                Side::New => kind != LineKind::Removed,
            };
            let picked: Vec<usize> =
                (0..h.lines.len()).filter(|&i| h.lines[i].kind != LineKind::NoNewline && on_side(h.lines[i].kind)).collect();
            let text: Vec<String> = picked.iter().map(|&i| h.lines[i].content.clone()).collect();
            let lines = highlight(&self.syntax, path, &text)?;
            Some(picked.into_iter().zip(lines).collect())
        };
        let (mut old, mut new) = (fragment(Side::Old), fragment(Side::New));
        (0..h.lines.len())
            .map(|i| {
                let line = &h.lines[i];
                let (side, no, from_fragment) = match line.kind {
                    LineKind::NoNewline => return Vec::new(),
                    LineKind::Removed => (Side::Old, line.old_no, old.as_mut()),
                    LineKind::Added | LineKind::Context => (Side::New, line.new_no, new.as_mut()),
                };
                match from_fragment {
                    Some(lines) => lines.remove(&i).unwrap_or_default(),
                    None => no.map_or_else(Vec::new, |no| self.side_line(diff, file, side, no).to_vec()),
                }
            })
            .collect()
    }
}

/// Each line of `lines` as segments, if the file's language (from `path`)
/// has a grammar and a `highlights.scm`. Lines longer than `MAX_LINE_LEN`
/// come back empty, to be drawn plain.
fn highlight(syntax: &Syntax, path: &str, lines: &[String]) -> Option<Vec<Vec<Segment>>> {
    let lang = &Registry::global().for_path(path)?.name;
    let grammar = grammar::bundled(lang)?;
    let query = query::compiled(lang, "highlights", &grammar)?;
    let text = lines.join("\n");
    let mut parser = Parser::new();
    parser.set_language(&grammar).ok()?;
    let tree = parser.parse(&text, None)?;

    // The styles captures map to; captures without one leave text alone.
    let styles: Vec<Option<Style>> = query.capture_names().iter().map(|n| syntax.style(n)).collect();
    // Per node, the last pattern that styles it wins.
    let mut nodes: HashMap<usize, (Range<usize>, usize, Style)> = HashMap::new();
    let mut cursor = QueryCursor::new();
    let mut captures = cursor.captures(query, tree.root_node(), text.as_bytes());
    while let Some((m, i)) = captures.next() {
        let capture = m.captures()[*i];
        let Some(style) = styles[capture.index as usize] else { continue };
        let node = capture.node;
        let entry = nodes.entry(node.id()).or_insert((node.byte_range(), m.pattern_index, style));
        if m.pattern_index >= entry.1 {
            *entry = (node.byte_range(), m.pattern_index, style);
        }
    }
    // Outer nodes first, so inner ones are drawn over them.
    let mut spans: Vec<_> = nodes.into_values().collect();
    spans.sort_by_key(|(range, _, _)| (range.start, std::cmp::Reverse(range.end)));
    let mut painted: Vec<Option<Style>> = vec![None; text.len()];
    for (range, _, style) in spans {
        painted[range].fill(Some(style));
    }

    let mut start = 0;
    Some(
        lines
            .iter()
            .map(|line| {
                let range = start..start + line.len();
                start = range.end + 1;
                if line.len() > MAX_LINE_LEN {
                    return Vec::new();
                }
                segments(line, &painted[range])
            })
            .collect(),
    )
}

/// `line` cut where its style changes.
fn segments(line: &str, styles: &[Option<Style>]) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    if line.is_empty() {
        return out;
    }
    let mut run = 0;
    for (at, _) in line.char_indices().skip(1).chain([(line.len(), ' ')]) {
        if at < line.len() && styles[at] == styles[run] {
            continue;
        }
        out.push((styles[run].unwrap_or_default(), line[run..at].to_string()));
        run = at;
    }
    out
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
    fn knows_the_languages_with_a_grammar() {
        for path in ["web/app.ts", "web/App.tsx", "web/app.js", "a.py", "a.go", "a.lua"] {
            let diff = diff(path, &[(LineKind::Added, "const a = 1")]);
            let mut highlighter = Highlighter::new(Theme::terminal(), Appearance::Dark);
            assert!(highlighter.line(&diff, 0, 0, 0).iter().any(|(s, _)| s.fg.is_some()), "{path}");
        }
        // No grammar for TOML yet, so it's plain.
        let diff = diff("Cargo.toml", &[(LineKind::Added, "a = 1")]);
        assert!(Highlighter::new(Theme::terminal(), Appearance::Dark).line(&diff, 0, 0, 0).is_empty());
    }

    #[test]
    fn light_theme_is_readable_on_white() {
        let diff = diff("a.rs", &[(LineKind::Added, "fn main() { let s = \"x\"; } // done")]);
        let mut highlighter = Highlighter::new(Theme::terminal(), Appearance::Light);
        let colors: Vec<_> = highlighter.line(&diff, 0, 0, 0).iter().filter_map(|(style, _)| style.fg).collect();
        assert!(colors.len() > 1);
        for color in colors {
            let Color::Rgb(r, g, b) = color else { panic!("{color:?}") };
            let white = crate::theme::doom::Rgb(255, 255, 255);
            assert!(crate::theme::contrast(crate::theme::doom::Rgb(r, g, b), white) >= 4.5, "{color:?}");
        }
    }

    #[test]
    fn highlights_whole_sources_for_context() {
        use crate::diff::Sources;
        let mut diff = diff("a.rs", &[(LineKind::Added, "x")]);
        diff.sources = vec![Sources {
            old: None,
            new: Some(std::sync::Arc::new(vec!["/*".into(), "fn a() {}".into(), "*/".into(), String::new()])),
        }];
        let mut highlighter = Highlighter::new(Theme::terminal(), Appearance::Dark);
        assert_eq!(text(highlighter.source_line(&diff, 0, 2)), "fn a() {}");
        // Line 2 is inside the comment line 1 opened.
        let inside = highlighter.source_line(&diff, 0, 2).to_vec();
        let plain = Highlighter::new(Theme::terminal(), Appearance::Dark).line(&super::tests::diff("a.rs", &[(LineKind::Added, "fn a() {}")]), 0, 0, 0).to_vec();
        assert_ne!(inside, plain);
        assert!(highlighter.source_line(&diff, 0, 0).is_empty());
        assert!(highlighter.source_line(&diff, 0, 4).is_empty());
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
            &[
                (LineKind::Context, "/*"),
                (LineKind::Removed, "fn a() {}"),
                (LineKind::Added, "fn a() {}"),
                (LineKind::Context, "*/"),
            ],
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
