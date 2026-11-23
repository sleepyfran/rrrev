//! Tree-sitter grammars, by registry name. rrrev links none in: `rrrev lang
//! enable` builds them into the data folder, and they're loaded from there.
//! Tests link the grammar crates instead, so they run without a build.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use tree_sitter::Language;
use tree_sitter_language::LanguageFn;

use super::Registry;

/// The grammars loaded so far, by name, and `None` for those that aren't
/// there, so neither is looked for twice.
static LOADED: OnceLock<Mutex<HashMap<String, Option<Language>>>> = OnceLock::new();

fn loaded() -> std::sync::MutexGuard<'static, HashMap<String, Option<Language>>> {
    LOADED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// The grammar for a registry language, linked in or built, loaded the
/// first time it's asked for. `None` when it's neither, or the build doesn't
/// load (`rrrev lang list` says why).
pub fn get(name: &str) -> Option<Language> {
    #[cfg(test)]
    if let Some(language) = linked(name) {
        return Some(language);
    }
    loaded()
        .entry(name.to_string())
        .or_insert_with(|| {
            let path = path(name)?;
            path.exists()
                .then(|| load(&path, &symbol(name)))
                .and_then(Result::ok)
        })
        .clone()
}

/// Forgets what `get` found for a language, so a grammar built since rrrev
/// started is loaded the next time it's asked for.
pub fn forget(name: &str) {
    loaded().remove(name);
}

/// Where built grammars live: `$XDG_DATA_HOME/rrrev/grammars`, by default
/// `~/.local/share/rrrev/grammars`. Tests get a folder that's never there,
/// so the grammars built on the machine running them don't change what
/// they see.
pub fn dir() -> Option<PathBuf> {
    if cfg!(test) {
        return Some(
            std::env::temp_dir().join(format!("rrrev-tests-{}/grammars", std::process::id())),
        );
    }
    Some(super::data_home()?.join("rrrev/grammars"))
}

/// Where a language's grammar is built to.
pub fn path(name: &str) -> Option<PathBuf> {
    let extension = if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };
    Some(dir()?.join(format!("{name}.{extension}")))
}

/// The name in a grammar's `tree_sitter_<symbol>` entry point.
pub fn symbol(name: &str) -> String {
    let registry = Registry::global();
    registry
        .get(name)
        .and_then(|l| l.grammar.as_ref()?.symbol.clone())
        .unwrap_or_else(|| name.to_string())
}

/// Loads a built grammar and checks this tree-sitter can use it. The
/// library stays loaded for as long as rrrev runs.
pub fn load(path: &Path, symbol: &str) -> Result<Language> {
    // SAFETY: the library is a tree-sitter grammar rrrev built itself, and
    // its entry point takes nothing and returns a `TSLanguage` pointer.
    let language = unsafe {
        let library = libloading::Library::new(path)
            .with_context(|| format!("cannot load {}", path.display()))?;
        let entry: unsafe extern "C" fn() -> *const () = *library
            .get(format!("tree_sitter_{symbol}").as_bytes())
            .with_context(|| format!("{} has no tree_sitter_{symbol}", path.display()))?;
        let language = Language::new(LanguageFn::from_raw(entry));
        std::mem::forget(library);
        language
    };
    let abi = language.abi_version();
    anyhow::ensure!(
        (tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION..=tree_sitter::LANGUAGE_VERSION)
            .contains(&abi),
        "{} was made for tree-sitter ABI {abi}, this rrrev reads {} to {}",
        path.display(),
        tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION,
        tree_sitter::LANGUAGE_VERSION
    );
    Ok(language)
}

/// Every grammar linked into the tests.
#[cfg(test)]
pub const LINKED: &[&str] = &[
    "rust",
    "python",
    "javascript",
    "typescript",
    "tsx",
    "go",
    "java",
    "c",
    "cpp",
    "csharp",
    "ruby",
    "swift",
    "kotlin",
    "lua",
    "fsharp",
];

/// The grammar for a registry language, from its crate.
#[cfg(test)]
fn linked(name: &str) -> Option<Language> {
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
        "csharp" => tree_sitter_c_sharp::LANGUAGE.into(),
        "ruby" => tree_sitter_ruby::LANGUAGE.into(),
        "swift" => tree_sitter_swift::LANGUAGE.into(),
        "kotlin" => tree_sitter_kotlin_ng::LANGUAGE.into(),
        "lua" => tree_sitter_lua::LANGUAGE.into(),
        "fsharp" => tree_sitter_fsharp::LANGUAGE_FSHARP.into(),
        _ => return None,
    })
}
