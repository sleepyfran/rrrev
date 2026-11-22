//! Plumbing: imports, exports and the like, the lines that wire code
//! together without doing anything. Found with each language's
//! `plumbing.scm`, so only for the languages that have a grammar here.

use tree_sitter::{Parser, QueryCursor, StreamingIterator};

use crate::languages::{Registry, grammar, query};

/// Per line of `source`, whether it's plumbing: everything on it other than
/// whitespace sits inside a `@plumbing` capture. `None` when its language
/// (from `path`) has no grammar or no `plumbing.scm` here.
pub fn lines(path: &str, source: &[String]) -> Option<Vec<bool>> {
    let lang = &Registry::global().for_path(path)?.name;
    let grammar = grammar::get(lang)?;
    let query = query::compiled(lang, "plumbing", &grammar)?;
    let text = source.join("\n");
    let mut parser = Parser::new();
    parser.set_language(&grammar).ok()?;
    let tree = parser.parse(&text, None)?;

    let mut covered = vec![false; text.len()];
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        for capture in m.captures() {
            if query.capture_names()[capture.index as usize] == "plumbing" {
                covered[capture.node.byte_range()].fill(true);
            }
        }
    }

    let mut start = 0;
    Some(
        source
            .iter()
            .map(|line| {
                let range = start..start + line.len();
                start = range.end + 1;
                let mut bytes = line.bytes().zip(&covered[range]);
                !line.trim().is_empty() && bytes.all(|(b, &c)| c || b.is_ascii_whitespace())
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 1-based lines of `text` that are plumbing, parsed as `path`.
    fn plumbing(path: &str, text: &str) -> Vec<usize> {
        let source: Vec<String> = text.lines().map(str::to_string).collect();
        let lines = lines(path, &source).expect("a grammar for this file");
        lines.iter().enumerate().filter(|(_, p)| **p).map(|(i, _)| i + 1).collect()
    }

    #[test]
    fn rust_imports_but_not_code() {
        let text = "\
use std::sync::Arc;
use crate::{
    a,
    b,
};
#[cfg(test)]
use std::fmt;
mod scope;
mod tests {}
pub use a::B;
use x; fn f() {}
// a comment
fn main() {}
";
        assert_eq!(plumbing("a.rs", text), [1, 2, 3, 4, 5, 6, 7, 8, 10]);
    }

    #[test]
    fn python_imports_and_all() {
        let text = "\
from __future__ import annotations
import os
from a import (
    b,
)
__all__ = [\"b\"]
x = 1
";
        assert_eq!(plumbing("a.py", text), [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn typescript_imports_and_re_exports_but_not_declarations() {
        let text = "\
import { a } from \"a\";
import type { B } from \"b\";
export { c } from \"c\";
export * from \"d\";
export { a };
export function f() {}
export const g = 1;
export default a;
";
        assert_eq!(plumbing("a.ts", text), [1, 2, 3, 4, 5]);
        assert_eq!(plumbing("a.tsx", text), [1, 2, 3, 4, 5]);
        assert_eq!(plumbing("a.js", "import a from \"a\";\nexport { a };\na();\n"), [1, 2]);
    }

    #[test]
    fn the_other_languages() {
        assert_eq!(plumbing("a.go", "package a\n\nimport (\n\t\"fmt\"\n)\n\nfunc f() {}\n"), [1, 3, 4, 5]);
        assert_eq!(plumbing("A.java", "package a;\nimport b.C;\nclass A {}\n"), [1, 2]);
        assert_eq!(plumbing("a.c", "#include <stdio.h>\n#include \"a.h\"\nint x;\n"), [1, 2]);
        assert_eq!(plumbing("a.cpp", "#include <vector>\nusing std::vector;\nint x;\n"), [1, 2]);
        assert_eq!(plumbing("A.cs", "using System;\nusing static B.C;\nclass A {}\n"), [1, 2]);
        assert_eq!(plumbing("a.rb", "require \"json\"\nrequire_relative \"b\"\nfoo.require \"x\"\nputs 1\n"), [1, 2]);
        assert_eq!(plumbing("a.swift", "import Foundation\nlet a = 1\n"), [1]);
        assert_eq!(plumbing("A.kt", "package a\n\nimport b.C\nimport d.*\n\nfun f() {}\n"), [1, 3, 4]);
        assert_eq!(plumbing("a.lua", "local a = require(\"a\")\nlocal b = 1\nlocal c = f(require(\"c\"))\n"), [1]);
        assert_eq!(plumbing("A.fs", "module A\n\nopen System\nopen B.C\n\nlet a = 1\n"), [3, 4]);
    }

    #[test]
    fn no_grammar_no_answer() {
        assert!(lines("README.md", &["# a".to_string()]).is_none());
    }
}
