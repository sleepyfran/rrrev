//! Scopes from a real parse, with tree-sitter, for the languages that have
//! a grammar here. A file parses once into its outline, every definition in
//! it in the order they start, and lines look their scopes up from that.

use tree_sitter::{Language, Node, Parser};

use super::{Scope, shorten};

/// The languages with a grammar, picked by file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
    Java,
    C,
    Cpp,
    #[cfg(feature = "csharp")]
    CSharp,
    Ruby,
    #[cfg(feature = "swift")]
    Swift,
    #[cfg(feature = "kotlin")]
    Kotlin,
    Lua,
    #[cfg(feature = "fsharp")]
    FSharp,
}

/// Calls whose function argument reads as a definition, as test suites
/// write them: `describe("…", () => {`, `it "…" do`.
const TEST_CALLS: &[&str] = &["describe", "context", "it", "test"];

/// How many lines a label may take, for signatures split over several.
const MAX_LABEL_ROWS: usize = 12;

impl Lang {
    #[cfg(test)]
    const ALL: &[Lang] = &[
        Lang::Rust,
        Lang::Python,
        Lang::JavaScript,
        Lang::TypeScript,
        Lang::Tsx,
        Lang::Go,
        Lang::Java,
        Lang::C,
        Lang::Cpp,
        #[cfg(feature = "csharp")]
        Lang::CSharp,
        Lang::Ruby,
        #[cfg(feature = "swift")]
        Lang::Swift,
        #[cfg(feature = "kotlin")]
        Lang::Kotlin,
        Lang::Lua,
        #[cfg(feature = "fsharp")]
        Lang::FSharp,
    ];

    fn from_path(path: &str) -> Option<Lang> {
        let name = path.rsplit('/').next().unwrap_or(path);
        if matches!(name, "Gemfile" | "Rakefile") {
            return Some(Lang::Ruby);
        }
        let (_, ext) = name.rsplit_once('.')?;
        Some(match ext {
            "rs" => Lang::Rust,
            "py" | "pyi" => Lang::Python,
            "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "go" => Lang::Go,
            "java" => Lang::Java,
            "c" | "h" => Lang::C,
            "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" => Lang::Cpp,
            #[cfg(feature = "csharp")]
            "cs" => Lang::CSharp,
            "rb" | "rake" | "gemspec" => Lang::Ruby,
            #[cfg(feature = "swift")]
            "swift" => Lang::Swift,
            #[cfg(feature = "kotlin")]
            "kt" | "kts" => Lang::Kotlin,
            "lua" => Lang::Lua,
            #[cfg(feature = "fsharp")]
            "fs" | "fsx" => Lang::FSharp,
            _ => return None,
        })
    }

    fn grammar(self) -> Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Java => tree_sitter_java::LANGUAGE.into(),
            Lang::C => tree_sitter_c::LANGUAGE.into(),
            Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            #[cfg(feature = "csharp")]
            Lang::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Lang::Ruby => tree_sitter_ruby::LANGUAGE.into(),
            #[cfg(feature = "swift")]
            Lang::Swift => tree_sitter_swift::LANGUAGE.into(),
            #[cfg(feature = "kotlin")]
            Lang::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
            Lang::Lua => tree_sitter_lua::LANGUAGE.into(),
            #[cfg(feature = "fsharp")]
            Lang::FSharp => tree_sitter_fsharp::LANGUAGE_FSHARP.into(),
        }
    }

    /// Node kinds that are a definition wherever they appear, as long as
    /// they have a body or take more than a line (so `mod a;`, `struct A;`
    /// and Go's `type X int` don't count).
    fn kinds(self) -> &'static [&'static str] {
        const JS: &[&str] =
            &["function_declaration", "generator_function_declaration", "class_declaration", "method_definition"];
        const TS: &[&str] = &[
            "function_declaration",
            "generator_function_declaration",
            "class_declaration",
            "abstract_class_declaration",
            "method_definition",
            "interface_declaration",
            "enum_declaration",
            "internal_module",
            "module",
        ];
        const C: &[&str] = &["function_definition", "struct_specifier", "enum_specifier", "union_specifier"];
        match self {
            Lang::Rust => &[
                "function_item",
                "impl_item",
                "trait_item",
                "struct_item",
                "enum_item",
                "union_item",
                "mod_item",
                "macro_definition",
            ],
            Lang::Python => &["function_definition", "class_definition"],
            Lang::JavaScript => JS,
            Lang::TypeScript | Lang::Tsx => TS,
            Lang::Go => &["function_declaration", "method_declaration", "type_spec"],
            Lang::Java => &[
                "class_declaration",
                "interface_declaration",
                "enum_declaration",
                "record_declaration",
                "annotation_type_declaration",
                "method_declaration",
                "constructor_declaration",
            ],
            Lang::C => C,
            Lang::Cpp => &[
                "function_definition",
                "struct_specifier",
                "enum_specifier",
                "union_specifier",
                "class_specifier",
                "namespace_definition",
            ],
            #[cfg(feature = "csharp")]
            Lang::CSharp => &[
                "namespace_declaration",
                "class_declaration",
                "struct_declaration",
                "interface_declaration",
                "enum_declaration",
                "record_declaration",
                "method_declaration",
                "constructor_declaration",
                "local_function_statement",
                "property_declaration",
            ],
            Lang::Ruby => &["method", "singleton_method", "class", "module", "singleton_class"],
            // Swift's structs, enums, actors and extensions are all `class_declaration`.
            #[cfg(feature = "swift")]
            Lang::Swift => &[
                "class_declaration",
                "protocol_declaration",
                "function_declaration",
                "init_declaration",
                "deinit_declaration",
            ],
            #[cfg(feature = "kotlin")]
            Lang::Kotlin => &[
                "class_declaration",
                "object_declaration",
                "companion_object",
                "function_declaration",
                "secondary_constructor",
            ],
            Lang::Lua => &["function_declaration"],
            #[cfg(feature = "fsharp")]
            Lang::FSharp => &["namespace", "module_defn", "type_definition", "member_defn"],
        }
    }

    /// Kinds that are a definition only in some places, checked by `special`.
    #[cfg(test)]
    fn special_kinds(self) -> &'static [&'static str] {
        match self {
            Lang::JavaScript | Lang::TypeScript | Lang::Tsx => {
                &["arrow_function", "function_expression", "call_expression"]
            }
            Lang::Ruby => &["call"],
            Lang::Lua => &["function_definition"],
            #[cfg(feature = "fsharp")]
            Lang::FSharp => &["function_or_value_defn"],
            _ => &[],
        }
    }
}

/// Every definition in `source`, in the order they start, if its language
/// (from `path`) has a grammar here.
pub fn outline(path: &str, source: &[String]) -> Option<Vec<Scope>> {
    let lang = Lang::from_path(path)?;
    let text = source.join("\n");
    let mut parser = Parser::new();
    parser.set_language(&lang.grammar()).ok()?;
    let tree = parser.parse(&text, None)?;
    let mut scopes = Vec::new();
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        if node.is_named() && is_definition(lang, node, &text) {
            scopes.push(Scope {
                line: node.start_position().row as u32 + 1,
                end: last_row(node) as u32 + 1,
                label: label(node, body(lang, node), source),
            });
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return Some(scopes);
            }
        }
    }
}

fn is_definition(lang: Lang, node: Node, text: &str) -> bool {
    if let Some(yes) = special(lang, node, text) {
        return yes;
    }
    lang.kinds().contains(&node.kind())
        && (body(lang, node).is_some() || node.start_position().row < last_row(node))
}

/// For kinds that are a definition only in some places: whether this one is.
fn special(lang: Lang, node: Node, text: &str) -> Option<bool> {
    fn parent<'t>(n: Node<'t>) -> &'t str {
        n.parent().map_or("", |p| p.kind())
    }
    let js = matches!(lang, Lang::JavaScript | Lang::TypeScript | Lang::Tsx);
    Some(match (lang, node.kind()) {
        // A function given a name: `const f = () => {`, `k: function () {`,
        // not every callback passed to `.map`.
        (_, "arrow_function" | "function_expression") if js => matches!(
            parent(node),
            "variable_declarator" | "pair" | "assignment_expression" | "public_field_definition" | "field_definition"
        ),
        (_, "call_expression") if js => {
            test_call(node.child_by_field_name("function"), text) && body(lang, node).is_some()
        }
        (Lang::Ruby, "call") => {
            test_call(node.child_by_field_name("method"), text) && node.child_by_field_name("block").is_some()
        }
        // `local d = function() … end`, `M.e = function() … end`, `{ f = function() … end }`.
        (Lang::Lua, "function_definition") => {
            parent(node) == "field" || node.parent().is_some_and(|p| parent(p) == "assignment_statement")
        }
        // A function, or a value worked out over several lines; not `let x = 3`.
        #[cfg(feature = "fsharp")]
        (Lang::FSharp, "function_or_value_defn") => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor).any(|c| c.kind() == "function_declaration_left")
                || node.start_position().row < last_row(node)
        }
        _ => return None,
    })
}

fn test_call(callee: Option<Node>, text: &str) -> bool {
    callee.is_some_and(|c| c.kind() == "identifier" && TEST_CALLS.contains(&&text[c.byte_range()]))
}

/// Where a definition's body starts, which is where its label stops.
fn body<'t>(lang: Lang, node: Node<'t>) -> Option<Node<'t>> {
    match (lang, node.kind()) {
        (Lang::JavaScript | Lang::TypeScript | Lang::Tsx, "call_expression") => {
            let args = node.child_by_field_name("arguments")?;
            let last = args.named_child((args.named_child_count() as u32).checked_sub(1)?)?;
            matches!(last.kind(), "arrow_function" | "function_expression")
                .then(|| last.child_by_field_name("body"))
                .flatten()
        }
        (Lang::Ruby, "call") => node.child_by_field_name("block"),
        _ => node.child_by_field_name("body").or_else(|| node.child_by_field_name("block")).or_else(|| {
            // Kotlin's bodies are children rather than fields.
            let mut cursor = node.walk();
            node.named_children(&mut cursor).find(|c| c.kind().ends_with("_body"))
        }),
    }
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
    fn every_kind_exists_in_its_grammar() {
        for &lang in Lang::ALL {
            let grammar = lang.grammar();
            for kind in lang.kinds().iter().chain(lang.special_kinds()) {
                assert_ne!(grammar.id_for_node_kind(kind, true), 0, "{lang:?} has no `{kind}` nodes");
            }
        }
    }

    #[test]
    fn picks_languages_by_file_name() {
        assert_eq!(Lang::from_path("src/a.rs"), Some(Lang::Rust));
        assert_eq!(Lang::from_path("web/App.tsx"), Some(Lang::Tsx));
        assert_eq!(Lang::from_path("Gemfile"), Some(Lang::Ruby));
        // Opt-in grammars only with their feature; indentation otherwise.
        #[cfg(feature = "fsharp")]
        assert_eq!(Lang::from_path("lib/Mod.fs"), Some(Lang::FSharp));
        #[cfg(not(feature = "fsharp"))]
        assert_eq!(Lang::from_path("lib/Mod.fs"), None);
        assert_eq!(Lang::from_path("README.md"), None);
        assert_eq!(Lang::from_path("Makefile"), None);
        assert!(outline("notes.txt", &source("fn a() {}")).is_none());
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
