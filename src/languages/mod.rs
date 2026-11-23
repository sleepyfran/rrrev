//! The languages rrrev knows, in one place: which files belong to each, where
//! its tree-sitter grammar comes from and which LSP server speaks it. The
//! list ships in `languages.toml`; `~/.config/rrrev/languages.toml` can
//! change entries or add new ones.

pub mod grammar;
pub mod manage;
pub mod query;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use serde::Deserialize;

const BUILTIN: &str = include_str!("languages.toml");

/// Where a language's tree-sitter grammar is built from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grammar {
    /// The repository to fetch.
    pub git: String,
    /// The commit to build, pinned.
    pub rev: String,
    /// The grammar's folder in the repository, when it holds several.
    pub path: Option<String>,
    /// The name in the grammar's `tree_sitter_<symbol>` entry point, when it
    /// isn't the language's own.
    pub symbol: Option<String>,
}

/// How to start a language's LSP server.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lsp {
    /// The server's executable, as Mason names it, or a full path.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Files that mark a project's root, e.g. `Cargo.toml`; globs allowed.
    #[serde(default)]
    pub roots: Vec<String>,
}

/// One server or several, in order of preference.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Servers {
    One(Lsp),
    Many(Vec<Lsp>),
}

impl Servers {
    fn into_vec(self) -> Vec<Lsp> {
        match self {
            Self::One(lsp) => vec![lsp],
            Self::Many(list) => list,
        }
    }
}

impl Lsp {
    /// The server's executable: a full path as given, or the command in
    /// Mason's bin folder. rrrev never looks on `PATH` and never installs
    /// servers, so what runs is what Neovim runs.
    pub fn find(&self) -> Option<PathBuf> {
        let path = if self.command.contains('/') {
            PathBuf::from(&self.command)
        } else {
            mason_bin()?.join(&self.command)
        };
        path.is_file().then_some(path)
    }
}

/// `$XDG_DATA_HOME`, by default `~/.local/share`.
pub fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
}

/// Mason's bin folder: `$XDG_DATA_HOME/nvim/mason/bin`, where Neovim's
/// Mason puts the servers it installs.
pub fn mason_bin() -> Option<PathBuf> {
    Some(data_home()?.join("nvim/mason/bin"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    pub name: String,
    /// File extensions, without the dot.
    pub extensions: Vec<String>,
    /// Whole file names, for files without an extension like `Gemfile`.
    pub filenames: Vec<String>,
    pub grammar: Option<Grammar>,
    /// The servers that can speak it, in order of preference.
    pub lsp: Vec<Lsp>,
}

impl Language {
    /// The server to run: the first one that's installed, else the first
    /// one (so messages name what the user would install).
    pub fn server(&self) -> Option<&Lsp> {
        self.lsp.iter().find(|l| l.find().is_some()).or(self.lsp.first())
    }

    fn matches(&self, name: &str) -> bool {
        self.filenames.iter().any(|f| f == name)
            || name.rsplit_once('.').is_some_and(|(_, ext)| self.extensions.iter().any(|e| e == ext))
    }
}

/// An entry as written in a file: only `name` is required, so an override
/// can set just the fields it changes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    extensions: Option<Vec<String>>,
    filenames: Option<Vec<String>>,
    grammar: Option<Grammar>,
    lsp: Option<Servers>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    language: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    languages: Vec<Language>,
}

static GLOBAL: OnceLock<Registry> = OnceLock::new();

impl Registry {
    /// The languages that ship with rrrev.
    pub fn builtin() -> Self {
        Self { languages: Vec::new() }.merge(BUILTIN).expect("the built-in languages.toml parses")
    }

    /// The built-in languages with the user's file on top, if there is one.
    pub fn load() -> Result<Self> {
        let Some(path) = crate::config::dir().map(|dir| dir.join("languages.toml")) else {
            return Ok(Self::builtin());
        };
        Self::builtin().merge_file(&path)
    }

    /// Makes `registry` the one `global` hands out. Call once, before
    /// anything looks a language up; later calls are ignored.
    pub fn install(registry: Self) {
        let _ = GLOBAL.set(registry);
    }

    /// The registry `install` set up, or the built-in one if it never ran
    /// (as in tests).
    pub fn global() -> &'static Self {
        GLOBAL.get_or_init(Self::builtin)
    }

    fn merge_file(self, path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => self.merge(&text).with_context(|| format!("in {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(self),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    /// Adds the entries in `text`: one with a known name changes the fields
    /// it sets, a new name is a new language. A new language needs at least
    /// its extensions or file names.
    pub(crate) fn merge(mut self, text: &str) -> Result<Self> {
        let file: File = toml::from_str(text)?;
        for entry in file.language {
            if let Some(known) = self.languages.iter_mut().find(|l| l.name == entry.name) {
                if let Some(extensions) = entry.extensions {
                    known.extensions = extensions;
                }
                if let Some(filenames) = entry.filenames {
                    known.filenames = filenames;
                }
                if entry.grammar.is_some() {
                    known.grammar = entry.grammar;
                }
                if let Some(servers) = entry.lsp {
                    known.lsp = servers.into_vec();
                }
            } else {
                let language = Language {
                    extensions: entry.extensions.unwrap_or_default(),
                    filenames: entry.filenames.unwrap_or_default(),
                    grammar: entry.grammar,
                    lsp: entry.lsp.map(Servers::into_vec).unwrap_or_default(),
                    name: entry.name,
                };
                anyhow::ensure!(
                    !language.extensions.is_empty() || !language.filenames.is_empty(),
                    "language `{}` needs `extensions` or `filenames`",
                    language.name
                );
                self.languages.push(language);
            }
        }
        Ok(self)
    }

    /// The language of a file, by its name (`path` may have folders). The
    /// first match wins, so built-in languages go first.
    pub fn for_path(&self, path: &str) -> Option<&Language> {
        let name = path.rsplit('/').next().unwrap_or(path);
        self.languages.iter().find(|l| l.matches(name))
    }

    pub fn get(&self, name: &str) -> Option<&Language> {
        self.languages.iter().find(|l| l.name == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Language> {
        self.languages.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name_for(registry: &Registry, path: &str) -> Option<String> {
        registry.for_path(path).map(|l| l.name.clone())
    }

    #[test]
    fn the_builtin_list_parses_and_finds_files() {
        let registry = Registry::builtin();
        assert_eq!(registry.iter().count(), 15);
        assert_eq!(name_for(&registry, "src/main.rs").as_deref(), Some("rust"));
        assert_eq!(name_for(&registry, "a/b/App.tsx").as_deref(), Some("tsx"));
        assert_eq!(name_for(&registry, "Gemfile").as_deref(), Some("ruby"));
        assert_eq!(name_for(&registry, "lib/Gemfile").as_deref(), Some("ruby"));
        assert_eq!(name_for(&registry, "include/x.hpp").as_deref(), Some("cpp"));
        assert_eq!(name_for(&registry, "README.md"), None);
        assert_eq!(name_for(&registry, "Makefile"), None);
        let rust = registry.get("rust").unwrap();
        assert_eq!(rust.server().unwrap().command, "rust-analyzer");
        assert!(registry.iter().all(|l| l.grammar.as_ref().is_some_and(|g| g.rev.len() == 40)));
    }

    #[test]
    fn overrides_change_only_what_they_set() {
        let registry = Registry::builtin()
            .merge(
                r#"
                [[language]]
                name = "kotlin"
                lsp = { command = "/opt/kotlin-lsp/bin/kotlin-lsp" }

                [[language]]
                name = "zig"
                extensions = ["zig"]
                lsp = { command = "zls", roots = ["build.zig"] }
                "#,
            )
            .unwrap();
        let kotlin = registry.get("kotlin").unwrap();
        assert_eq!(kotlin.server().unwrap().command, "/opt/kotlin-lsp/bin/kotlin-lsp");
        assert!(kotlin.server().unwrap().args.is_empty());
        assert_eq!(kotlin.extensions, ["kt", "kts"]);
        assert!(kotlin.grammar.is_some());
        assert_eq!(name_for(&registry, "build.zig").as_deref(), Some("zig"));
        assert_eq!(registry.get("zig").unwrap().grammar, None);
    }

    #[test]
    fn typescript_prefers_tsgo_and_falls_back() {
        let registry = Registry::builtin();
        for name in ["javascript", "typescript", "tsx"] {
            let commands: Vec<_> = registry.get(name).unwrap().lsp.iter().map(|l| l.command.as_str()).collect();
            assert_eq!(commands, ["tsgo", "typescript-language-server"], "{name}");
        }
        assert_eq!(registry.get("tsx").unwrap().lsp[0].args, ["--lsp", "--stdio"]);
    }

    #[test]
    fn bad_overrides_are_errors() {
        let err = Registry::builtin().merge("[[language]]\nname = \"zig\"").unwrap_err();
        assert!(err.to_string().contains("needs `extensions`"), "{err}");
        assert!(Registry::builtin().merge("[[language]]\nname = \"rust\"\nextension = [\"rs\"]").is_err());
        assert!(Registry::builtin().merge("not toml").is_err());
    }

    #[test]
    fn reads_the_file_if_there_is_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("languages.toml");
        assert_eq!(Registry::builtin().merge_file(&path).unwrap(), Registry::builtin());
        fs::write(&path, "[[language]]\nname = \"rust\"\nextensions = [\"rs\", \"rsx\"]\n").unwrap();
        let registry = Registry::builtin().merge_file(&path).unwrap();
        assert_eq!(registry.get("rust").unwrap().extensions, ["rs", "rsx"]);
        fs::write(&path, "nope").unwrap();
        let err = Registry::builtin().merge_file(&path).unwrap_err();
        assert!(format!("{err:#}").contains("languages.toml"), "{err:#}");
    }
}
