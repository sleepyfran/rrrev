//! Where a line sits in its file: the functions, impls, classes and modules
//! around it, found from the file's text by indentation. Not a parser, but
//! it reads most brace and indentation languages well enough for a
//! breadcrumb, and it never needs the language to be known.

/// One enclosing definition: its first line, cut down for a breadcrumb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    /// 1-based line number of the definition's first line.
    pub line: u32,
    /// 1-based line number of its last line, as far as indentation tells.
    pub end: u32,
    pub label: String,
}

/// How far up to look for an enclosing definition.
const MAX_LOOKBACK: usize = 5000;
/// Labels longer than this are cut with `…`.
const MAX_LABEL: usize = 60;

/// Words that, after modifiers, open a definition worth naming.
const DEFINITION_WORDS: &[&str] = &[
    "fn", "function", "func", "def", "class", "struct", "enum", "trait", "impl", "mod", "module", "interface",
    "type", "namespace", "object", "union", "macro_rules!", "describe", "it", "test", "fun", "sub", "proc",
    "method", "record", "protocol", "extension", "actor", "contract",
];

/// Words that may come before the definition word.
const MODIFIERS: &[&str] = &[
    "pub", "pub(crate)", "pub(super)", "export", "default", "async", "static", "private", "public", "protected",
    "internal", "const", "unsafe", "extern", "abstract", "override", "final", "virtual", "inline", "open",
    "sealed", "data", "suspend", "declare", "readonly", "partial", "@objc",
];

/// The definitions enclosing `line` (1-based) in `source`, outermost first.
pub fn enclosing(source: &[String], line: u32) -> Vec<Scope> {
    let Some(target) = (line as usize).checked_sub(1).filter(|&i| i < source.len()) else { return Vec::new() };
    // The line's own indentation, or the next non-blank one's for a blank line.
    let Some(mut bound) = source[target..].iter().find(|l| !l.trim().is_empty()).map(|l| indent(l)) else {
        return Vec::new();
    };
    // A definition's own line belongs to it, so it shows as its own scope.
    let mut scopes = Vec::new();
    if let Some(label) = definition(&source[target]) {
        scopes.push(Scope { line, end: scope_end(source, target), label });
    }
    let floor = target.saturating_sub(MAX_LOOKBACK);
    for i in (floor..target).rev() {
        let text = &source[i];
        if text.trim().is_empty() {
            continue;
        }
        let level = indent(text);
        if level >= bound {
            continue;
        }
        bound = level;
        if let Some(label) = definition(text) {
            scopes.push(Scope { line: i as u32 + 1, end: scope_end(source, i), label });
        }
        if level == 0 {
            break;
        }
    }
    scopes.reverse();
    scopes
}

/// The last line of the block that opens at `start` (0-based): the line
/// before the next one indented no deeper, taking a closing bracket with it.
fn scope_end(source: &[String], start: usize) -> u32 {
    let level = indent(&source[start]);
    for (i, text) in source.iter().enumerate().skip(start + 1) {
        let trimmed = text.trim();
        if trimmed.is_empty() || indent(text) > level {
            continue;
        }
        let closes = trimmed.starts_with(['}', ')', ']']) || trimmed == "end" || trimmed.starts_with("end ");
        return if closes { i as u32 + 1 } else { last_solid(source, start, i) };
    }
    last_solid(source, start, source.len())
}

/// 1-based number of the last non-blank line in `start..before`.
fn last_solid(source: &[String], start: usize, before: usize) -> u32 {
    (start..before).rev().find(|&i| !source[i].trim().is_empty()).unwrap_or(start) as u32 + 1
}

/// Columns of leading whitespace, a tab counting as four.
fn indent(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

/// The line as a breadcrumb label, if it opens a definition.
fn definition(line: &str) -> Option<String> {
    let trimmed = line.trim();
    // `mod a;`, `fn f();`, `type T = U;` declare rather than open a block.
    if trimmed.ends_with(';') {
        return None;
    }
    if trimmed.starts_with("//") || trimmed.starts_with('#') && !trimmed.starts_with("#[") || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
    {
        return None;
    }
    let mut words = trimmed.split_whitespace().skip_while(|w| MODIFIERS.contains(w) || w.starts_with("pub("));
    let first = words.next()?;
    let keyword = first.split(['(', '<', '{', ':']).next().unwrap_or(first);
    let named = DEFINITION_WORDS.contains(&keyword)
        // Test helpers like `describe(` / `it(` need their call parenthesis.
        && (!matches!(keyword, "describe" | "it" | "test") || first.contains('('));
    (named || looks_like_method(trimmed)).then(|| shorten(trimmed))
}

/// `name(args) {` or `name(args): T {`, as in JS/TS classes, C-likes and Go
/// methods, without keywords such as `if` or `for` that look the same.
fn looks_like_method(line: &str) -> bool {
    const NOT_DEFINITIONS: &[&str] =
        &["if", "for", "while", "switch", "match", "catch", "return", "else", "do", "try", "with", "loop", "elif"];
    if !line.ends_with('{') || !line.contains('(') {
        return false;
    }
    let head = line.split('(').next().unwrap_or("").trim();
    let name = head.split_whitespace().last().unwrap_or("");
    !name.is_empty()
        && !head.contains('=')
        && !head.contains('.')
        && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '*' || c == ':' || c == '<' || c == '>')
        && !NOT_DEFINITIONS.contains(&name)
        && !head.split_whitespace().any(|w| NOT_DEFINITIONS.contains(&w))
}

/// The label without its opening brace, cut to fit a breadcrumb.
fn shorten(line: &str) -> String {
    let line = line.trim_end_matches(['{', ':', ' ']).trim_end();
    let line = line.strip_suffix(" =>").unwrap_or(line);
    if line.chars().count() <= MAX_LABEL {
        return line.to_string();
    }
    let head: String = line.chars().take(MAX_LABEL - 1).collect();
    format!("{}…", head.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn innermost(source: &[String], line: u32) -> Option<Scope> {
        enclosing(source, line).pop()
    }

    fn source(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    fn labels(source: &[String], line: u32) -> Vec<String> {
        enclosing(source, line).into_iter().map(|s| s.label).collect()
    }

    const RUST: &str = "\
use std::fmt;

impl PathCache {
    /// Shortens a path.
    pub fn shorten(&self, path: &str) -> String {
        if path.is_empty() {
            return String::new();
        }
        path.to_string()
    }
}
";

    #[test]
    fn finds_rust_impls_and_functions() {
        let src = source(RUST);
        assert_eq!(labels(&src, 7), ["impl PathCache", "pub fn shorten(&self, path: &str) -> String"]);
        // The definition line itself is inside its own scope.
        assert_eq!(labels(&src, 5), ["impl PathCache", "pub fn shorten(&self, path: &str) -> String"]);
        assert_eq!(labels(&src, 1), Vec::<String>::new());
        let f = innermost(&src, 7).unwrap();
        assert_eq!((f.line, f.end), (5, 10));
        assert_eq!(innermost(&src, 4).unwrap().label, "impl PathCache");
    }

    #[test]
    fn finds_typescript_functions_and_methods() {
        let src = source(
            "export default function fffExtension(pi: ExtensionAPI) {\n  let x = 1;\n  class A {\n    run(a: number): void {\n      if (a) {\n        go();\n      }\n    }\n  }\n}\n",
        );
        assert_eq!(
            labels(&src, 6),
            ["export default function fffExtension(pi: ExtensionAPI)", "class A", "run(a: number): void"]
        );
        let outer = enclosing(&src, 2).remove(0);
        assert_eq!((outer.line, outer.end), (1, 10));
    }

    #[test]
    fn finds_python_defs_by_indentation() {
        let src = source("class A:\n    def run(self):\n        x = 1\n\n        return x\n\ny = 2\n");
        assert_eq!(labels(&src, 5), ["class A", "def run(self)"]);
        let run = innermost(&src, 3).unwrap();
        assert_eq!((run.line, run.end), (2, 5));
        assert!(labels(&src, 7).is_empty());
    }

    #[test]
    fn declarations_are_not_scopes() {
        let src = source("mod a;\nfn f();\nimpl X {\n    type T = u8;\n    fn g();\n}\n");
        assert!(labels(&src, 1).is_empty());
        assert_eq!(labels(&src, 5), ["impl X"]);
    }

    #[test]
    fn control_flow_is_not_a_scope() {
        let src = source("fn main() {\n    for x in y {\n        if x {\n            z();\n        }\n    }\n}\n");
        assert_eq!(labels(&src, 4), ["fn main()"]);
    }

    #[test]
    fn out_of_range_lines_have_no_scope() {
        let src = source("fn a() {}\n");
        assert!(enclosing(&src, 0).is_empty());
        assert!(enclosing(&src, 9).is_empty());
    }

    #[test]
    fn long_labels_are_cut() {
        let long = format!("fn {}() {{", "x".repeat(100));
        let src = source(&format!("{long}\n    y();\n}}\n"));
        let label = &labels(&src, 2)[0];
        assert_eq!(label.chars().count(), MAX_LABEL);
        assert!(label.ends_with('…'));
    }
}
