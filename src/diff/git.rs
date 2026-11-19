use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::{Delta, DiffOptions, Patch, Repository, Tree};

use super::stack::Stack;
use super::{Diff, DiffLine, DiffSource, FileDiff, FileStatus, Hunk, LineKind};

/// Which two snapshots of the repository to compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSpec {
    /// HEAD vs the working tree, staged and unstaged changes plus untracked files.
    WorkTree,
    /// HEAD vs the index.
    Staged,
    /// A revision vs the working tree.
    RevToWorkTree(String),
    /// `from..to`: tree to tree.
    Range { from: String, to: String },
    /// `from...to`: merge base of both vs `to`.
    MergeBase { from: String, to: String },
    /// Each layer of a stack against the one below, in order.
    Stack(Stack),
}

impl DiffSpec {
    pub fn from_args(spec: Option<&str>, staged: bool) -> Self {
        let Some(spec) = spec else {
            return if staged { DiffSpec::Staged } else { DiffSpec::WorkTree };
        };
        let or_head = |s: &str| if s.is_empty() { "HEAD".to_string() } else { s.to_string() };
        if let Some((from, to)) = spec.split_once("...") {
            DiffSpec::MergeBase { from: or_head(from), to: or_head(to) }
        } else if let Some((from, to)) = spec.split_once("..") {
            DiffSpec::Range { from: or_head(from), to: or_head(to) }
        } else {
            DiffSpec::RevToWorkTree(spec.to_string())
        }
    }
}

pub struct GitSource {
    repo: Repository,
    spec: DiffSpec,
}

impl GitSource {
    pub fn discover(path: impl AsRef<Path>, spec: DiffSpec) -> Result<Self> {
        let repo = Repository::discover(path).context("not inside a git repository")?;
        Ok(Self { repo, spec })
    }

    /// The stack the checked-out branch belongs to (`gh stack`), layer by layer.
    pub fn stack(path: impl AsRef<Path>) -> Result<Self> {
        let repo = Repository::discover(path).context("not inside a git repository")?;
        let workdir = repo.workdir().unwrap_or(repo.path()).to_path_buf();
        let stack = Stack::detect(repo.commondir(), &workdir)
            .context("the current branch is not part of a stack (see `gh stack view`)")?;
        Ok(Self { repo, spec: DiffSpec::Stack(stack) })
    }

    fn tree(&self, rev: &str) -> Result<Tree<'_>> {
        self.repo
            .revparse_single(rev)
            .and_then(|obj| obj.peel_to_tree())
            .with_context(|| format!("cannot resolve `{rev}` to a tree"))
    }

    /// HEAD's tree, or `None` in a repository with no commits yet.
    fn head_tree(&self) -> Option<Tree<'_>> {
        self.repo.head().ok()?.peel_to_tree().ok()
    }

    /// `from...to`: the merge base of both vs `to`.
    fn merge_base_diff(&self, from: &str, to: &str, opts: &mut DiffOptions) -> Result<git2::Diff<'_>> {
        let mut diff = self.repo.diff_tree_to_tree(Some(&self.merge_base(from, to)?), Some(&self.tree(to)?), Some(opts))?;
        diff.find_similar(None)?;
        Ok(diff)
    }

    /// The tree of the commit `from` and `to` last had in common.
    fn merge_base(&self, from: &str, to: &str) -> Result<Tree<'_>> {
        let repo = &self.repo;
        let commit = |rev: &str| -> Result<git2::Oid> {
            Ok(repo
                .revparse_single(rev)
                .and_then(|obj| obj.peel_to_commit())
                .with_context(|| format!("cannot resolve `{rev}` to a commit"))?
                .id())
        };
        let base = repo
            .merge_base(commit(from)?, commit(to)?)
            .with_context(|| format!("no merge base between `{from}` and `{to}`"))?;
        Ok(repo.find_commit(base)?.tree()?)
    }

    /// The branch checked out, if HEAD is on one.
    fn current_branch(&self) -> Option<String> {
        let head = self.repo.head().ok()?;
        head.is_branch().then(|| head.shorthand().ok().map(str::to_string)).flatten()
    }

    /// Whether the working tree or index differ from HEAD, untracked files included.
    fn has_uncommitted_changes(&self) -> Result<bool> {
        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(true).include_ignored(false);
        Ok(!self.repo.statuses(Some(&mut opts))?.is_empty())
    }

    fn git_diff(&self) -> Result<git2::Diff<'_>> {
        let mut opts = worktree_options();

        let repo = &self.repo;
        let mut diff = match &self.spec {
            DiffSpec::WorkTree => {
                repo.diff_tree_to_workdir_with_index(self.head_tree().as_ref(), Some(&mut opts))?
            }
            DiffSpec::Staged => repo.diff_tree_to_index(self.head_tree().as_ref(), None, Some(&mut opts))?,
            DiffSpec::RevToWorkTree(rev) => {
                repo.diff_tree_to_workdir_with_index(Some(&self.tree(rev)?), Some(&mut opts))?
            }
            DiffSpec::Range { from, to } => {
                repo.diff_tree_to_tree(Some(&self.tree(from)?), Some(&self.tree(to)?), Some(&mut opts))?
            }
            DiffSpec::MergeBase { from, to } => return self.merge_base_diff(from, to, &mut opts),
            DiffSpec::Stack(_) => unreachable!("stacks are loaded layer by layer"),
        };
        diff.find_similar(None)?;
        Ok(diff)
    }

    /// File name for this spec's saved state. Revisions stay symbolic so the
    /// state follows a branch as it gets new commits.
    fn state_key(&self) -> String {
        let key = match &self.spec {
            DiffSpec::WorkTree => "worktree".to_string(),
            DiffSpec::Staged => "staged".to_string(),
            DiffSpec::RevToWorkTree(rev) => format!("rev-{rev}"),
            DiffSpec::Range { from, to } => format!("range-{from}..{to}"),
            DiffSpec::MergeBase { from, to } => format!("mergebase-{from}...{to}"),
            DiffSpec::Stack(stack) => format!("stack-{}-{}", stack.trunk, stack.top()),
        };
        key.chars()
            .map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' })
            .collect()
    }
}

impl DiffSource for GitSource {
    fn describe(&self) -> String {
        match &self.spec {
            DiffSpec::WorkTree => "HEAD → working tree".to_string(),
            DiffSpec::Staged => "HEAD → index".to_string(),
            DiffSpec::RevToWorkTree(rev) => format!("{rev} → working tree"),
            DiffSpec::Range { from, to } => format!("{from}..{to}"),
            DiffSpec::MergeBase { from, to } => format!("{from}...{to}"),
            DiffSpec::Stack(stack) => {
                let layers = stack.layers.iter().map(|l| l.branch.as_str());
                format!("stack {}", std::iter::once(stack.trunk.as_str()).chain(layers).collect::<Vec<_>>().join(" ← "))
            }
        }
    }

    fn load(&self) -> Result<Diff> {
        let DiffSpec::Stack(stack) = &self.spec else {
            return Ok(Diff::new(read_files(&self.git_diff()?)?));
        };
        let count = stack.layers.len();
        let current = self.current_branch();
        let mut parts = Vec::with_capacity(count);
        for (i, layer) in stack.layers.iter().enumerate() {
            let mut rationale = format!("Layer {} of {count}: {} → {}.", i + 1, layer.base, layer.branch);
            // The checked-out layer includes what isn't committed yet, so the
            // review covers everything the stack will carry.
            let diff = if current.as_deref() == Some(layer.branch.as_str()) && self.has_uncommitted_changes()? {
                rationale += " Includes uncommitted changes.";
                let base = self.merge_base(&layer.base, &layer.branch)?;
                let mut diff = self.repo.diff_tree_to_workdir_with_index(Some(&base), Some(&mut worktree_options()))?;
                diff.find_similar(None)?;
                diff
            } else {
                self.merge_base_diff(&layer.base, &layer.branch, &mut DiffOptions::new())?
            };
            if layer.needs_rebase {
                rationale += " Needs a rebase onto the layer below.";
            }
            parts.push((layer.title(), rationale, read_files(&diff)?));
        }
        Ok(Diff::sectioned(parts))
    }

    fn state_file(&self) -> PathBuf {
        // `.git` itself, or `.git/worktrees/<name>` in a linked worktree.
        self.repo.path().join("rrrev").join(format!("{}.json", self.state_key()))
    }
}

/// Options for diffs against the working tree: untracked files included, in full.
fn worktree_options() -> DiffOptions {
    let mut opts = DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true);
    opts
}

fn read_files(diff: &git2::Diff) -> Result<Vec<FileDiff>> {
    let mut files = Vec::with_capacity(diff.deltas().len());
    for idx in 0..diff.deltas().len() {
        let delta = diff.get_delta(idx).expect("index within deltas()");
        let path = |file: git2::DiffFile| file.path().map(|p| p.to_string_lossy().into_owned());
        let mut file = FileDiff {
            old_path: path(delta.old_file()),
            new_path: path(delta.new_file()),
            status: map_status(delta.status()),
            is_binary: delta.flags().is_binary(),
            hunks: Vec::new(),
        };
        match Patch::from_diff(diff, idx)? {
            Some(patch) => file.hunks = read_hunks(&patch)?,
            None => file.is_binary = true,
        }
        files.push(file);
    }
    Ok(files)
}

fn map_status(delta: Delta) -> FileStatus {
    match delta {
        Delta::Added | Delta::Untracked => FileStatus::Added,
        Delta::Deleted => FileStatus::Deleted,
        Delta::Modified => FileStatus::Modified,
        Delta::Renamed => FileStatus::Renamed,
        Delta::Copied => FileStatus::Copied,
        _ => FileStatus::Other,
    }
}

fn read_hunks(patch: &Patch) -> Result<Vec<Hunk>> {
    let mut hunks = Vec::with_capacity(patch.num_hunks());
    for h in 0..patch.num_hunks() {
        let (hunk, line_count) = patch.hunk(h)?;
        let mut lines = Vec::with_capacity(line_count);
        for l in 0..line_count {
            let line = patch.line_in_hunk(h, l)?;
            let kind = match line.origin() {
                '+' => LineKind::Added,
                '-' => LineKind::Removed,
                ' ' => LineKind::Context,
                _ => LineKind::NoNewline,
            };
            lines.push(DiffLine {
                kind,
                old_no: line.old_lineno(),
                new_no: line.new_lineno(),
                content: clean(line.content()),
            });
        }
        hunks.push(Hunk {
            id: Default::default(),
            header: clean(hunk.header()),
            old_start: hunk.old_start(),
            old_lines: hunk.old_lines(),
            new_start: hunk.new_start(),
            new_lines: hunk.new_lines(),
            lines,
        });
    }
    Ok(hunks)
}

/// Decodes a raw diff line for display: lossy UTF-8, no trailing newline.
fn clean(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_matches(|c| c == '\n' || c == '\r')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn commit_all(repo: &Repository, message: &str) {
        let mut index = repo.index().unwrap();
        index.add_all(["*"], git2::IndexAddOption::DEFAULT, None).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("test", "test@example.com").unwrap();
        let parent = repo.head().ok().map(|h| h.peel_to_commit().unwrap());
        let parents: Vec<_> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents).unwrap();
    }

    #[test]
    fn parses_specs() {
        assert_eq!(DiffSpec::from_args(None, false), DiffSpec::WorkTree);
        assert_eq!(DiffSpec::from_args(None, true), DiffSpec::Staged);
        assert_eq!(
            DiffSpec::from_args(Some("main..feat"), false),
            DiffSpec::Range { from: "main".into(), to: "feat".into() }
        );
        assert_eq!(
            DiffSpec::from_args(Some("main..."), false),
            DiffSpec::MergeBase { from: "main".into(), to: "HEAD".into() }
        );
        assert_eq!(DiffSpec::from_args(Some("abc123"), false), DiffSpec::RevToWorkTree("abc123".into()));
    }

    #[test]
    fn loads_worktree_changes_including_untracked() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        commit_all(&repo, "init");

        fs::write(dir.path().join("a.txt"), "one\n2\nthree\n").unwrap();
        fs::write(dir.path().join("new.txt"), "hello\n").unwrap();

        let diff = GitSource::discover(dir.path(), DiffSpec::WorkTree).unwrap().load().unwrap();
        assert_eq!(diff.files.len(), 2);

        let a = diff.files.iter().find(|f| f.new_path.as_deref() == Some("a.txt")).unwrap();
        assert_eq!(a.status, FileStatus::Modified);
        assert_eq!(a.hunks.len(), 1);
        let kinds: Vec<_> = a.hunks[0].lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [LineKind::Context, LineKind::Removed, LineKind::Added, LineKind::Context]
        );
        assert_eq!(a.hunks[0].lines[2].content, "2");
        assert_eq!(a.hunks[0].lines[2].new_no, Some(2));

        let new = diff.files.iter().find(|f| f.new_path.as_deref() == Some("new.txt")).unwrap();
        assert_eq!(new.status, FileStatus::Added);
    }

    #[test]
    fn loads_a_stack_layer_by_layer() {
        use crate::diff::stack::Layer;
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        commit_all(&repo, "init");
        let branch = |name: &str| {
            let head = repo.head().unwrap().peel_to_commit().unwrap();
            repo.branch(name, &head, true).unwrap();
        };
        branch("trunk");
        fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        commit_all(&repo, "first");
        branch("first");
        fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        fs::write(dir.path().join("b.txt"), "b\n").unwrap();
        commit_all(&repo, "second");
        branch("second");

        let layer = |branch: &str, base: &str, pr| Layer { branch: branch.into(), base: base.into(), pr, needs_rebase: false };
        let stack = Stack {
            trunk: "trunk".into(),
            layers: vec![layer("first", "trunk", Some(7)), layer("second", "first", None)],
        };
        let source = GitSource::discover(dir.path(), DiffSpec::Stack(stack)).unwrap();
        assert_eq!(source.describe(), "stack trunk ← first ← second");
        assert!(source.state_file().ends_with("rrrev/stack-trunk-second.json"));

        let diff = source.load().unwrap();
        let paths: Vec<_> = diff.files.iter().map(|f| f.path()).collect();
        assert_eq!(paths, ["a.txt", "a.txt", "b.txt"]);
        assert_eq!(diff.sections[0].title, "#7 first");
        assert_eq!(diff.sections[0].files, 0..1);
        assert_eq!(diff.sections[1].rationale, "Layer 2 of 2: first → second.");
        assert_eq!(diff.sections[1].files, 1..3);
        // Each layer shows only its own change.
        let added = |f: usize| -> Vec<&str> {
            diff.files[f].hunks[0].lines.iter().filter(|l| l.kind == LineKind::Added).map(|l| l.content.as_str()).collect()
        };
        assert_eq!(added(0), ["two"]);
        assert_eq!(added(1), ["three"]);

        // Uncommitted work lands in the checked-out layer.
        repo.set_head("refs/heads/second").unwrap();
        fs::write(dir.path().join("c.txt"), "c\n").unwrap();
        let diff = source.load().unwrap();
        let paths: Vec<_> = diff.files.iter().map(|f| f.path()).collect();
        assert_eq!(paths, ["a.txt", "a.txt", "b.txt", "c.txt"]);
        assert_eq!(diff.sections[1].rationale, "Layer 2 of 2: first → second. Includes uncommitted changes.");
        assert!(!diff.sections[0].rationale.contains("uncommitted"));
    }

    #[test]
    fn no_stack_metadata_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        Repository::init(dir.path()).unwrap();
        let error = GitSource::stack(dir.path()).err().unwrap();
        assert!(error.to_string().starts_with("the current branch is not part of a stack"));
    }

    #[test]
    fn state_file_lives_in_the_git_dir_with_a_safe_name() {
        let dir = tempfile::tempdir().unwrap();
        Repository::init(dir.path()).unwrap();
        let spec = DiffSpec::from_args(Some("main..feature/auth"), false);
        let path = GitSource::discover(dir.path(), spec).unwrap().state_file();
        assert!(path.ends_with(".git/rrrev/range-main..feature_auth.json"), "{path:?}");
    }
}
