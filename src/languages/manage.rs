//! `rrrev lang`: building the grammars of the languages you want and listing
//! what's enabled. The app builds them too, when it offers to.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use super::{Grammar, Language, Registry, grammar};

/// Where a language's grammar stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Built by `rrrev lang enable`, and it loads.
    Enabled,
    /// Built, but it doesn't load: the message says why.
    Broken(String),
    /// Known, with a grammar to build, but not built.
    Available,
    /// Known without a grammar, e.g. only for its LSP server.
    NoGrammar,
}

pub fn status(language: &Language) -> Status {
    if language.grammar.is_none() {
        return Status::NoGrammar;
    }
    match grammar::path(&language.name).filter(|p| p.exists()) {
        Some(path) => match grammar::load(&path, &grammar::symbol(&language.name)) {
            Ok(_) => Status::Enabled,
            Err(e) => Status::Broken(format!("{e:#}")),
        },
        None => Status::Available,
    }
}

/// `rrrev lang list`: every language, its grammar and its server.
pub fn list(registry: &Registry) -> String {
    let mut out = String::new();
    let width = registry.iter().map(|l| l.name.len()).max().unwrap_or(0);
    for language in registry.iter() {
        let grammar = match status(language) {
            Status::Enabled => "enabled".to_string(),
            Status::Broken(why) => format!("broken ({why}); run `rrrev lang enable {}` again", language.name),
            Status::Available => "not enabled".to_string(),
            Status::NoGrammar => "no grammar".to_string(),
        };
        let server = match &language.lsp {
            None => "no server".to_string(),
            Some(lsp) if lsp.find().is_some() => format!("{} found", lsp.command),
            Some(lsp) => format!("{} not in Mason", lsp.command),
        };
        let _ = writeln!(out, "{:width$}  {grammar:<12}  {server}", language.name);
    }
    out
}

/// `rrrev lang enable <name>`: fetches the grammar at its pinned commit,
/// builds it with the system's C compiler and checks it loads. Returns what
/// to tell the user.
pub fn enable(language: &Language) -> Result<String> {
    let name = &language.name;
    let Some(source) = &language.grammar else { bail!("{name} has no grammar to build") };
    let target = grammar::path(name).context("no home directory to keep grammars in")?;
    let work = std::env::temp_dir().join(format!("rrrev-grammar-{name}-{}", std::process::id()));
    let built = fetch(source, &work).and_then(|src| build(&src, &work.join(format!("{name}.lib")), name));
    let result = built.and_then(|lib| {
        fs::create_dir_all(target.parent().unwrap())?;
        // Moved into place only once it loads, so a failed build leaves a
        // working one alone.
        fs::copy(&lib, target.with_extension("new")).context("cannot save the grammar")?;
        fs::rename(target.with_extension("new"), &target).context("cannot save the grammar")?;
        Ok(())
    });
    let _ = fs::remove_dir_all(&work);
    result?;

    let mut out = format!("Enabled {name}: {}", target.display());
    match &language.lsp {
        Some(lsp) if lsp.find().is_none() => {
            let _ = write!(
                out,
                "\nIts server, {}, isn't in Mason's folder ({}). Install it from Neovim with :Mason \
                 to get go to definition and references.",
                lsp.command,
                super::mason_bin().map_or_else(|| "~/.local/share/nvim/mason/bin".into(), |p| p.display().to_string()),
            );
        }
        _ => {}
    }
    Ok(out)
}

/// `rrrev lang disable <name>`: deletes its built grammar.
pub fn disable(language: &Language) -> Result<String> {
    let name = &language.name;
    match grammar::path(name).filter(|p| p.exists()) {
        Some(path) => {
            fs::remove_file(&path).with_context(|| format!("cannot delete {}", path.display()))?;
            Ok(format!("Disabled {name}"))
        }
        None => Ok(format!("{name} isn't enabled")),
    }
}

/// The repository at the pinned commit, with `git` (rrrev's own git library
/// has no HTTPS). Returns the grammar's `src` folder.
fn fetch(grammar: &Grammar, work: &Path) -> Result<PathBuf> {
    let repo = work.join("repo");
    fs::create_dir_all(&repo)?;
    let git = |args: &[&str]| run(Command::new("git").arg("-C").arg(&repo).args(args), "git");
    git(&["init", "-q"])?;
    git(&["fetch", "-q", "--depth", "1", &grammar.git, &grammar.rev])
        .with_context(|| format!("cannot fetch {} at {}", grammar.git, grammar.rev))?;
    git(&["-c", "advice.detachedHead=false", "checkout", "-q", "FETCH_HEAD"])?;
    let src = grammar.path.as_ref().map_or_else(|| repo.clone(), |p| repo.join(p)).join("src");
    anyhow::ensure!(src.join("parser.c").exists(), "{} has no parser.c", src.display());
    Ok(src)
}

/// Builds `parser.c` and the grammar's scanner, if it has one, into a shared
/// library at `out`, and checks it loads.
fn build(src: &Path, out: &Path, name: &str) -> Result<PathBuf> {
    let cpp_scanner = src.join("scanner.cc");
    let c_scanner = src.join("scanner.c");
    let (compiler, mut command) = if cpp_scanner.exists() {
        let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".into());
        let mut command = Command::new(&cxx);
        command.args(["-x", "c"]).arg(src.join("parser.c")).args(["-x", "c++"]).arg(&cpp_scanner);
        (cxx, command)
    } else {
        let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
        let mut command = Command::new(&cc);
        command.arg(src.join("parser.c"));
        if c_scanner.exists() {
            command.arg(&c_scanner);
        }
        (cc, command)
    };
    command.args(["-shared", "-fPIC", "-O2", "-w", "-I"]).arg(src).arg("-o").arg(out);
    run(&mut command, &compiler).with_context(|| format!("cannot build the {name} grammar"))?;
    grammar::load(out, &grammar::symbol(name))?;
    Ok(out.to_path_buf())
}

fn run(command: &mut Command, what: &str) -> Result<()> {
    let output = command.output().with_context(|| format!("cannot run {what}; is it installed?"))?;
    if !output.status.success() {
        bail!("{what} failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_every_language() {
        let registry = Registry::builtin();
        let list = list(&registry);
        assert_eq!(list.lines().count(), registry.iter().count());
        assert!(list.lines().next().unwrap().starts_with("rust "), "{list}");
        assert!(list.contains("not enabled"));
    }
}
