use std::path::Path;

use anyhow::{Context, Result};
use git2::{Delta, DiffOptions, Patch, Repository, Tree};

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

    fn git_diff(&self) -> Result<git2::Diff<'_>> {
        let mut opts = DiffOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true);

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
            DiffSpec::MergeBase { from, to } => {
                let commit = |rev: &str| -> Result<git2::Oid> {
                    Ok(repo.revparse_single(rev)?.peel_to_commit()?.id())
                };
                let base = repo
                    .merge_base(commit(from)?, commit(to)?)
                    .with_context(|| format!("no merge base between `{from}` and `{to}`"))?;
                let base_tree = repo.find_commit(base)?.tree()?;
                repo.diff_tree_to_tree(Some(&base_tree), Some(&self.tree(to)?), Some(&mut opts))?
            }
        };
        diff.find_similar(None)?;
        Ok(diff)
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
        }
    }

    fn load(&self) -> Result<Diff> {
        let diff = self.git_diff()?;
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
            match Patch::from_diff(&diff, idx)? {
                Some(patch) => file.hunks = read_hunks(&patch)?,
                None => file.is_binary = true,
            }
            files.push(file);
        }
        Ok(Diff { files })
    }
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
}
