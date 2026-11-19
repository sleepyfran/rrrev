//! Stacked pull requests as `gh stack` tracks them: a chain of branches on a
//! trunk, each one a layer reviewed on its own.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stack {
    pub trunk: String,
    /// Bottom to top. Layers already merged are left out.
    pub layers: Vec<Layer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    pub branch: String,
    /// What the layer's diff starts from: the layer below, or the trunk.
    pub base: String,
    pub pr: Option<u64>,
    pub needs_rebase: bool,
}

impl Layer {
    /// `#12 auth-layer`, or the branch alone before the PR exists.
    pub fn title(&self) -> String {
        match self.pr {
            Some(pr) => format!("#{pr} {}", self.branch),
            None => self.branch.clone(),
        }
    }
}

impl Stack {
    /// The stack the checked-out branch belongs to, if any. `gh` only runs
    /// when the repository has stack metadata, so plain repositories don't
    /// pay for it, and any failure (no `gh`, not on a stack) means no stack.
    pub fn detect(git_dir: &Path, workdir: &Path) -> Option<Stack> {
        if !git_dir.join("gh-stack").exists() {
            return None;
        }
        Self::from_gh(workdir).ok()
    }

    fn from_gh(workdir: &Path) -> Result<Stack> {
        let program = std::env::var("RRREV_GH").unwrap_or_else(|_| "gh".to_string());
        let output = Command::new(&program)
            .args(["stack", "view", "--json"])
            .current_dir(workdir)
            .output()
            .with_context(|| format!("cannot run `{program}`"))?;
        if !output.status.success() {
            bail!("`gh stack view` failed: {}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Self::parse(&String::from_utf8_lossy(&output.stdout))
    }

    /// Reads `gh stack view --json`.
    pub fn parse(json: &str) -> Result<Stack> {
        #[derive(Deserialize)]
        struct View {
            trunk: String,
            branches: Vec<Branch>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Branch {
            name: String,
            #[serde(default)]
            is_merged: bool,
            #[serde(default)]
            needs_rebase: bool,
            #[serde(default)]
            pr: Option<Pr>,
        }
        #[derive(Deserialize)]
        struct Pr {
            number: Option<u64>,
        }
        let view: View = serde_json::from_str(json.trim()).context("unexpected output from `gh stack view`")?;
        // Merged layers are in the trunk now, so the next one builds on what's below them.
        let mut base = view.trunk.clone();
        let mut layers = Vec::new();
        for branch in view.branches.into_iter().filter(|b| !b.is_merged) {
            layers.push(Layer {
                base: std::mem::replace(&mut base, branch.name.clone()),
                branch: branch.name,
                pr: branch.pr.and_then(|pr| pr.number),
                needs_rebase: branch.needs_rebase,
            });
        }
        if layers.is_empty() {
            bail!("every layer of the stack is merged");
        }
        Ok(Stack { trunk: view.trunk, layers })
    }

    pub fn top(&self) -> &str {
        &self.layers.last().expect("a stack has a layer").branch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: &str = r#"{
      "trunk": "main",
      "currentBranch": "api",
      "branches": [
        { "name": "types", "base": "aaa", "isCurrent": false, "isMerged": true, "isQueued": false, "needsRebase": false },
        { "name": "auth", "base": "bbb", "isCurrent": false, "isMerged": false, "isQueued": false, "needsRebase": false,
          "pr": { "number": 12, "url": "https://github.com/o/r/pull/12" } },
        { "name": "api", "base": "ccc", "isCurrent": true, "isMerged": false, "isQueued": false, "needsRebase": true }
      ]
    }"#;

    #[test]
    fn parses_the_view_skipping_merged_layers() {
        let stack = Stack::parse(VIEW).unwrap();
        assert_eq!(stack.trunk, "main");
        assert_eq!(
            stack.layers,
            [
                Layer { branch: "auth".into(), base: "main".into(), pr: Some(12), needs_rebase: false },
                Layer { branch: "api".into(), base: "auth".into(), pr: None, needs_rebase: true },
            ]
        );
        assert_eq!(stack.layers[0].title(), "#12 auth");
        assert_eq!(stack.top(), "api");
    }

    #[test]
    fn rejects_junk_and_fully_merged_stacks() {
        assert!(Stack::parse("not json").is_err());
        let merged = r#"{"trunk":"main","branches":[{"name":"a","isMerged":true}]}"#;
        assert!(Stack::parse(merged).is_err());
    }

    #[test]
    fn no_metadata_means_no_stack() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Stack::detect(dir.path(), dir.path()), None);
    }
}
