//! The tree-sitter grammars linked into rrrev, by registry name.

use tree_sitter::Language;

/// Every grammar linked in, for tests to check against the registry.
#[cfg(test)]
pub const BUNDLED: &[&str] = &[
    "rust",
    "python",
    "javascript",
    "typescript",
    "tsx",
    "go",
    "java",
    "c",
    "cpp",
    #[cfg(feature = "csharp")]
    "csharp",
    "ruby",
    #[cfg(feature = "swift")]
    "swift",
    #[cfg(feature = "kotlin")]
    "kotlin",
    "lua",
    #[cfg(feature = "fsharp")]
    "fsharp",
];

/// The grammar for a registry language, if it's linked in.
pub fn bundled(name: &str) -> Option<Language> {
    Some(match name {
        "rust" => tree_sitter_rust::LANGUAGE.into(),
        "python" => tree_sitter_python::LANGUAGE.into(),
        "javascript" => tree_sitter_javascript::LANGUAGE.into(),
        "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        "java" => tree_sitter_java::LANGUAGE.into(),
        "c" => tree_sitter_c::LANGUAGE.into(),
        "cpp" => tree_sitter_cpp::LANGUAGE.into(),
        #[cfg(feature = "csharp")]
        "csharp" => tree_sitter_c_sharp::LANGUAGE.into(),
        "ruby" => tree_sitter_ruby::LANGUAGE.into(),
        #[cfg(feature = "swift")]
        "swift" => tree_sitter_swift::LANGUAGE.into(),
        #[cfg(feature = "kotlin")]
        "kotlin" => tree_sitter_kotlin_ng::LANGUAGE.into(),
        "lua" => tree_sitter_lua::LANGUAGE.into(),
        #[cfg(feature = "fsharp")]
        "fsharp" => tree_sitter_fsharp::LANGUAGE_FSHARP.into(),
        _ => return None,
    })
}
