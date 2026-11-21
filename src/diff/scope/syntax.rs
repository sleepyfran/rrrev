//! Scopes from a real parse, with tree-sitter, for the languages that have
//! a grammar here. A file parses once into its outline, every definition in
//! it in the order they start, and lines look their scopes up from that.

use tree_sitter::{Node, Parser, QueryCursor, StreamingIterator};

use super::{Scope, shorten};
use crate::languages::{Registry, grammar, query};

/// How many lines a label may take, for signatures split over several.
const MAX_LABEL_ROWS: usize = 12;

/// Every definition in `source`, in the order they start, if its language
/// (from `path`) has a grammar here.
pub fn outline(path: &str, source: &[String]) -> Option<Vec<Scope>> {
    let lang = &Registry::global().for_path(path)?.name;
    let grammar = grammar::bundled(lang)?;
    let query = query::compiled(lang, "scopes", &grammar)?;
    let text = source.join("\n");
    let mut parser = Parser::new();
    parser.set_language(&grammar).ok()?;
    let tree = parser.parse(&text, None)?;

    // Each definition once, with where its label stops if a pattern says.
    let mut found: Vec<(Node, Option<Node>)> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        let mut def = None;
        let mut label_end = None;
        for capture in m.captures() {
            let name = query.capture_names()[capture.index as usize];
            let node = capture.node;
            match name {
                "scope.body" => label_end = Some(node),
                "scope" if body(node).is_some() || spans_lines(node) => def = Some(node),
                "scope.always" => def = Some(node),
                "scope.multiline" if spans_lines(node) => def = Some(node),
                _ => {}
            }
        }
        let Some(node) = def else { continue };
        match found.iter_mut().find(|(n, _)| n.id() == node.id()) {
            Some((_, end)) => *end = end.or(label_end),
            None => found.push((node, label_end)),
        }
    }
    found.sort_by_key(|(n, _)| (n.start_byte(), std::cmp::Reverse(n.end_byte())));
    Some(
        found
            .into_iter()
            .map(|(node, label_end)| Scope {
                line: node.start_position().row as u32 + 1,
                end: last_row(node) as u32 + 1,
                label: label(node, label_end.or_else(|| body(node)), source),
            })
            .collect(),
    )
}

fn spans_lines(node: Node) -> bool {
    node.start_position().row < last_row(node)
}

/// Where a definition's body starts, which is where its label stops, when
/// the query doesn't say.
fn body(node: Node) -> Option<Node> {
    node.child_by_field_name("body").or_else(|| node.child_by_field_name("block")).or_else(|| {
        // Kotlin's bodies are children rather than fields.
        let mut cursor = node.walk();
        node.named_children(&mut cursor).find(|c| c.kind().ends_with("_body"))
    })
}

/// The 0-based row of a node's last character: a node that ends with its
/// line break ends on the line before.
fn last_row(node: Node) -> usize {
    let (start, end) = (node.start_position(), node.end_position());
    if end.column == 0 && end.row > start.row { end.row - 1 } else { end.row }
}

/// A definition's head, on one line: from the start of the line its name is
/// on (so modifiers like `pub` or `export` come along, and annotations on
/// the lines above don't) to where its body starts.
fn label(node: Node, body: Option<Node>, source: &[String]) -> String {
    let row = node
        .child_by_field_name("name")
        .filter(|n| n.start_byte() >= node.start_byte())
        .map_or(node.start_position().row, |n| n.start_position().row);
    let (end_row, end_col) = match body.map(|b| b.start_position()) {
        Some(p) if p.row >= row && p.row < row + MAX_LABEL_ROWS => (p.row, p.column),
        _ => (row, usize::MAX),
    };
    let parts = (row..=end_row).filter_map(|r| {
        let line = source.get(r)?;
        Some(if r == end_row { line.get(..end_col.min(line.len())).unwrap_or(line) } else { line.as_str() })
    });
    shorten(&join(parts))
}

/// Lines of a signature run together, with no spaces inside brackets and
/// no trailing comma before a closing one.
fn join<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for part in parts.map(str::trim).filter(|p| !p.is_empty()) {
        if part.starts_with([')', ']', '>']) && out.ends_with(',') {
            out.pop();
        }
        let tight = out.is_empty() || out.ends_with(['(', '[', '<']) || part.starts_with([')', ']', '>', ',']);
        if !tight {
            out.push(' ');
        }
        out.push_str(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    /// The labels around `line` of `text`, parsed as the file `path`.
    fn labels(path: &str, text: &str, line: u32) -> Vec<String> {
        let outline = outline(path, &source(text)).expect("a grammar for this file");
        super::super::within(&outline, line).into_iter().map(|s| s.label).collect()
    }

    /// The innermost scope's first and last lines.
    fn span(path: &str, text: &str, line: u32) -> (u32, u32) {
        let outline = outline(path, &source(text)).unwrap();
        let s = super::super::within(&outline, line).pop().expect("a scope");
        (s.line, s.end)
    }

    #[test]
    fn picks_languages_by_file_name() {
        let parses = |path| outline(path, &source("fn a() {}")).is_some();
        assert!(parses("src/a.rs"));
        assert!(parses("web/App.tsx"));
        assert!(parses("Gemfile"));
        // Opt-in grammars only with their feature; indentation otherwise.
        assert_eq!(parses("lib/Mod.fs"), cfg!(feature = "fsharp"));
        assert!(!parses("README.md"));
        assert!(!parses("Makefile"));
        assert!(!parses("notes.txt"));
    }

    const PYTHON: &str = "\
import os


@cache
class A(B):
    def run(
        self,
        a,
    ) -> int:
        \"\"\"Doc

more
\"\"\"
# note
        return a

    async def go(self):
        pass
";

    #[test]
    fn python_reads_past_what_broke_indentation() {
        // A split signature, a docstring and a comment at column 0.
        assert_eq!(labels("a.py", PYTHON, 15), ["class A(B)", "def run(self, a) -> int"]);
        assert_eq!(span("a.py", PYTHON, 15), (6, 15));
        assert_eq!(span("a.py", PYTHON, 7), (6, 15));
        assert_eq!(labels("a.py", PYTHON, 18), ["class A(B)", "async def go(self)"]);
        // Between methods is only in the class. Decorators sit outside it.
        assert_eq!(labels("a.py", PYTHON, 16), ["class A(B)"]);
        assert!(labels("a.py", PYTHON, 4).is_empty());
        assert!(labels("a.py", PYTHON, 2).is_empty());
    }

    const RUST: &str = "\
mod a;
struct U;

impl<T> Foo<T> {
    /// Docs.
    #[inline]
    pub fn run(
        a: u8,
    ) -> u8 {
        a
    }
}

#[cfg(test)]
mod tests {
    fn b() {}
}
";

    #[test]
    fn rust_definitions_but_not_declarations() {
        assert_eq!(labels("a.rs", RUST, 10), ["impl<T> Foo<T>", "pub fn run(a: u8) -> u8"]);
        assert_eq!(span("a.rs", RUST, 10), (7, 11));
        assert!(labels("a.rs", RUST, 1).is_empty());
        assert!(labels("a.rs", RUST, 2).is_empty());
        assert_eq!(labels("a.rs", RUST, 16), ["mod tests", "fn b()"]);
    }

    const TYPESCRIPT: &str = "\
export default function f(pi: X) {
  class A {
    @dec
    run(a: number): void {
      const g = (x) => {
        return x;
      };
      [1].map((y) => {
        return y;
      });
    }
  }
}
interface I {
  a: string;
}
describe(\"thing\", () => {
  it(\"works\", async () => {
    x();
  });
});
";

    #[test]
    fn typescript_functions_methods_and_tests() {
        let method = ["export default function f(pi: X)", "class A", "run(a: number): void"];
        assert_eq!(labels("a.ts", TYPESCRIPT, 6), [&method[..], &["const g = (x)"]].concat());
        // Callbacks aren't definitions.
        assert_eq!(labels("a.ts", TYPESCRIPT, 9), method);
        assert_eq!(labels("a.ts", TYPESCRIPT, 15), ["interface I"]);
        assert_eq!(labels("a.ts", TYPESCRIPT, 19), ["describe(\"thing\", ()", "it(\"works\", async ()"]);
        assert_eq!(labels("a.tsx", TYPESCRIPT, 6).len(), 4);
        assert_eq!(labels("a.js", "const o = {\n  m() {\n    return 1;\n  },\n};\n", 3), ["m()"]);
    }

    #[test]
    fn go_functions_and_types() {
        let src = "package main\n\ntype X int\n\ntype S struct {\n\ta int\n}\n\nfunc (s *S) Run(\n\ta int,\n) int {\n\tf := func() {\n\t}\n\treturn a\n}\n";
        assert_eq!(labels("a.go", src, 13), ["func (s *S) Run(a int) int"]);
        assert_eq!(labels("a.go", src, 6), ["type S struct"]);
        assert!(labels("a.go", src, 3).is_empty());
    }

    #[test]
    fn java_leaves_annotations_out_of_labels() {
        let src = "@Ann\npublic class A {\n    @Override\n    public void run() {\n        go();\n    }\n}\n";
        assert_eq!(labels("A.java", src, 5), ["public class A", "public void run()"]);
        // The annotation is still part of the method, for `ze`.
        assert_eq!(span("A.java", src, 5), (3, 6));
    }

    #[test]
    fn c_and_cpp() {
        let c = "struct foo x;\nstruct foo {\n  int a;\n};\nstatic int\nmain(void) {\n  return 0;\n}\n";
        assert_eq!(labels("a.c", c, 7), ["static int main(void)"]);
        assert_eq!(labels("a.c", c, 3), ["struct foo"]);
        assert!(labels("a.c", c, 1).is_empty());
        let cpp = "namespace n {\ntemplate <typename T>\nclass A {\n  void run() {\n    go();\n  }\n};\n}\n";
        assert_eq!(labels("a.cpp", cpp, 5), ["namespace n", "class A", "void run()"]);
    }

    #[cfg(feature = "csharp")]
    #[test]
    fn csharp() {
        let src = "namespace M {\n  [Attr]\n  public class A {\n    public int P { get; set; }\n    public void Run() {\n      Go();\n    }\n  }\n}\n";
        assert_eq!(labels("a.cs", src, 6), ["namespace M", "public class A", "public void Run()"]);
        assert_eq!(labels("a.cs", src, 4), ["namespace M", "public class A"]);
    }

    #[test]
    fn ruby_methods_and_specs() {
        let src = "module M\n  class A < B\n    def run(a)\n      a\n    end\n  end\nend\ndescribe \"x\" do\n  it \"y\" do\n    go\n  end\nend\n";
        assert_eq!(labels("a.rb", src, 4), ["module M", "class A < B", "def run(a)"]);
        assert_eq!(span("a.rb", src, 4), (3, 5));
        assert_eq!(labels("a_spec.rb", src, 10), ["describe \"x\"", "it \"y\""]);
    }

    #[cfg(feature = "swift")]
    #[test]
    fn swift() {
        let src = "protocol P {\n  func x()\n}\nstruct S {\n  init(a: Int) {\n  }\n  func run() -> Int {\n    return 1\n  }\n}\nextension S: P {\n  func x() {}\n}\n";
        assert_eq!(labels("a.swift", src, 8), ["struct S", "func run() -> Int"]);
        assert_eq!(labels("a.swift", src, 2), ["protocol P"]);
        assert_eq!(labels("a.swift", src, 12), ["extension S: P", "func x()"]);
    }

    #[cfg(feature = "kotlin")]
    #[test]
    fn kotlin() {
        let src = "class A(val x: Int) {\n    fun run(): Int {\n        return 1\n    }\n    companion object {\n        fun go() = 2\n    }\n}\nfun top(\n    a: Int,\n): Int {\n    return a\n}\n";
        assert_eq!(labels("a.kt", src, 3), ["class A(val x: Int)", "fun run(): Int"]);
        assert_eq!(labels("a.kt", src, 6), ["class A(val x: Int)", "companion object", "fun go()"]);
        assert_eq!(labels("a.kt", src, 12), ["fun top(a: Int): Int"]);
    }

    #[test]
    fn lua() {
        let src = "local function a(x)\n  return x\nend\nfunction M:c()\n  go()\nend\nlocal d = function()\n  go()\nend\nfor i = 1, 2 do\n  print(i)\nend\n";
        assert_eq!(labels("a.lua", src, 2), ["local function a(x)"]);
        assert_eq!(span("a.lua", src, 2), (1, 3));
        assert_eq!(labels("a.lua", src, 5), ["function M:c()"]);
        assert_eq!(labels("a.lua", src, 8), ["local d = function()"]);
        assert!(labels("a.lua", src, 11).is_empty());
    }

    #[cfg(feature = "fsharp")]
    const FSHARP: &str = "\
module M

type C(x: int) =
    member this.Run(a) =
        a + x

let add a b =
    let inner c =
        c
    inner a + b

let value = 3
";

    #[cfg(feature = "fsharp")]
    #[test]
    fn fsharp_functions_but_not_values() {
        assert_eq!(labels("a.fs", FSHARP, 5), ["type C(x: int)", "member this.Run(a)"]);
        assert_eq!(labels("a.fs", FSHARP, 9), ["let add a b", "let inner c"]);
        assert_eq!(span("a.fs", FSHARP, 10), (7, 10));
        assert!(labels("a.fs", FSHARP, 12).iter().all(|l| !l.contains("value")));
    }
}
