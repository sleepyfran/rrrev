//! Where a line sits in its file: the functions, impls, classes and modules
//! around it. Files in a language with a grammar here get them from a real
//! parse (`syntax`); the rest from indentation (`indent`).

mod indent;
mod syntax;

pub use indent::enclosing as by_indentation;
pub use syntax::outline;

/// One enclosing definition: its first line, cut down for a breadcrumb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    /// 1-based line number of the definition's first line.
    pub line: u32,
    /// 1-based line number of its last line.
    pub end: u32,
    pub label: String,
}

/// Labels longer than this are cut with `…`.
const MAX_LABEL: usize = 60;

/// The definitions in `outline` (every definition in a file, in the order
/// they start) that hold `line`, outermost first.
pub fn within(outline: &[Scope], line: u32) -> Vec<Scope> {
    outline.iter().filter(|s| s.line <= line && line <= s.end).cloned().collect()
}

/// The label without its opening brace, cut to fit a breadcrumb.
fn shorten(line: &str) -> String {
    let line = line.trim_end_matches(['{', ':', '=', ' ']).trim_end();
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

    #[test]
    fn within_keeps_the_scopes_holding_the_line() {
        let scope = |line, end, label: &str| Scope { line, end, label: label.into() };
        let outline = [scope(1, 10, "impl A"), scope(2, 4, "fn a()"), scope(6, 9, "fn b()"), scope(12, 14, "fn c()")];
        assert_eq!(within(&outline, 7), [scope(1, 10, "impl A"), scope(6, 9, "fn b()")]);
        assert_eq!(within(&outline, 1), [scope(1, 10, "impl A")]);
        assert!(within(&outline, 11).is_empty());
    }
}
