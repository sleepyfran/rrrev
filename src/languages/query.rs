//! The tree-sitter queries that ship with rrrev, in `queries/<language>/`:
//! `scopes.scm` for breadcrumbs and `ze`, `highlights.scm` for colours,
//! `plumbing.scm` for the imports and exports that fold away.

use std::collections::HashMap;
use std::sync::Mutex;

use tree_sitter::{Language, Query};

macro_rules! files {
    ($($lang:literal / $kind:literal,)*) => {
        &[$((concat!($lang, "/", $kind), include_str!(concat!("queries/", $lang, "/", $kind, ".scm"))),)*]
    };
}

static FILES: &[(&str, &str)] = files![
    "rust" / "scopes",
    "rust" / "highlights",
    "python" / "scopes",
    "python" / "highlights",
    "ecma" / "highlights",
    "jsx" / "highlights",
    "javascript" / "scopes",
    "javascript" / "highlights",
    "typescript" / "scopes",
    "typescript" / "highlights",
    "tsx" / "scopes",
    "tsx" / "highlights",
    "go" / "scopes",
    "go" / "highlights",
    "java" / "scopes",
    "java" / "highlights",
    "c" / "scopes",
    "c" / "highlights",
    "cpp" / "scopes",
    "cpp" / "highlights",
    "csharp" / "scopes",
    "csharp" / "highlights",
    "ruby" / "scopes",
    "ruby" / "highlights",
    "swift" / "scopes",
    "swift" / "highlights",
    "kotlin" / "scopes",
    "kotlin" / "highlights",
    "lua" / "scopes",
    "lua" / "highlights",
    "fsharp" / "scopes",
    "fsharp" / "highlights",
    "rust" / "plumbing",
    "python" / "plumbing",
    "ecma" / "plumbing",
    "javascript" / "plumbing",
    "typescript" / "plumbing",
    "tsx" / "plumbing",
    "go" / "plumbing",
    "java" / "plumbing",
    "c" / "plumbing",
    "cpp" / "plumbing",
    "csharp" / "plumbing",
    "ruby" / "plumbing",
    "swift" / "plumbing",
    "kotlin" / "plumbing",
    "lua" / "plumbing",
    "fsharp" / "plumbing",
];

/// A query's text, with what it inherits first: a first line
/// `; inherits: a, b` puts those languages' queries of the same kind ahead.
pub fn source(lang: &str, kind: &str) -> Option<String> {
    let path = format!("{lang}/{kind}");
    let own = FILES.iter().find(|(p, _)| *p == path)?.1;
    let mut text = String::new();
    if let Some(parents) = own
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("; inherits:"))
    {
        for parent in parents.split(',').map(str::trim) {
            text.push_str(&source(parent, kind)?);
            text.push('\n');
        }
    }
    text.push_str(own);
    Some(text)
}

/// A language's query of `kind`, compiled for `grammar` the first time it's
/// asked for and kept. `None` if there's no such query or it doesn't compile.
pub fn compiled(lang: &str, kind: &'static str, grammar: &Language) -> Option<&'static Query> {
    type Cache = Mutex<HashMap<(String, &'static str), Option<&'static Query>>>;
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *cache.entry((lang.to_string(), kind)).or_insert_with(|| {
        let query = Query::new(grammar, &source(lang, kind)?).ok()?;
        Some(&*Box::leak(Box::new(query)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::languages::{Registry, grammar};

    #[test]
    fn every_linked_grammar_has_a_language_and_queries_that_compile() {
        let registry = Registry::builtin();
        for &name in grammar::LINKED {
            assert!(
                registry.get(name).is_some(),
                "{name} isn't in languages.toml"
            );
            let grammar = grammar::get(name).unwrap();
            for kind in ["scopes", "highlights", "plumbing"] {
                let text = source(name, kind).unwrap_or_else(|| panic!("no {name}/{kind}.scm"));
                if let Err(e) = Query::new(&grammar, &text) {
                    panic!("{name}/{kind}.scm: {e}");
                }
            }
        }
    }

    #[test]
    fn inherits_put_the_parents_first() {
        let tsx = source("tsx", "highlights").unwrap();
        let ecma = tsx.find(&source("ecma", "highlights").unwrap()).unwrap();
        let jsx = tsx
            .find(
                FILES
                    .iter()
                    .find(|(p, _)| *p == "jsx/highlights")
                    .unwrap()
                    .1,
            )
            .unwrap();
        let ts = tsx.find("(type_identifier) @type").unwrap();
        assert!(ecma < ts && ts < jsx, "{ecma} {ts} {jsx}");
        assert!(source("zig", "highlights").is_none());
    }
}
