//! Source-agnostic diff model, plus the trait every diff source implements.

pub mod git;
pub mod stack;

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Anything that can produce a diff to review. Only git exists today; jj or a
/// piped-in patch would be further implementations.
pub trait DiffSource {
    /// Short label for the title bar, e.g. `main..feature`.
    fn describe(&self) -> String;
    fn load(&self) -> Result<Diff>;
    /// Where review progress for this diff is saved.
    fn state_file(&self) -> PathBuf;
}

#[derive(Debug, Clone, Default)]
pub struct Diff {
    pub files: Vec<FileDiff>,
    /// Runs of files reviewed as one group each, like the layers of a stack;
    /// empty for a plain diff.
    pub sections: Vec<Section>,
}

/// Consecutive files of a diff that belong together, e.g. one layer's diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub rationale: String,
    pub files: Range<usize>,
}

impl Diff {
    /// Builds a diff, giving every hunk its content-based id.
    pub fn new(mut files: Vec<FileDiff>) -> Self {
        // Shared across files, since a path shows up once per section.
        let mut seen: HashMap<u64, usize> = HashMap::new();
        for file in &mut files {
            let path = file.path().to_string();
            for hunk in &mut file.hunks {
                let hash = content_hash(&path, hunk);
                let occurrence = seen.entry(hash).or_default();
                *occurrence += 1;
                hunk.id = HunkId(match *occurrence {
                    1 => format!("{hash:016x}"),
                    n => format!("{hash:016x}-{n}"),
                });
            }
        }
        Diff { files, sections: Vec::new() }
    }

    /// Builds a diff out of sections, each with its own files, in order.
    pub fn sectioned(parts: Vec<(String, String, Vec<FileDiff>)>) -> Self {
        let mut files = Vec::new();
        let mut sections = Vec::new();
        for (title, rationale, part) in parts {
            let start = files.len();
            files.extend(part);
            sections.push(Section { title, rationale, files: start..files.len() });
        }
        Diff { sections, ..Diff::new(files) }
    }
}

/// Identifies a hunk by what it changes rather than where it sits, so it stays
/// the same across rebases and unrelated edits, and changes when the hunk does.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HunkId(String);

impl HunkId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// FNV-1a over the path and the changed lines. Context lines are left out so
/// edits next to a hunk don't reset its review.
fn content_hash(path: &str, hunk: &Hunk) -> u64 {
    let changed = hunk.lines.iter().filter(|l| l.kind != LineKind::Context);
    let mut bytes: Vec<u8> = path.bytes().chain([0]).collect();
    for line in changed {
        let marker = match line.kind {
            LineKind::Added => b'+',
            LineKind::Removed => b'-',
            _ => b'\\',
        };
        bytes.push(marker);
        bytes.extend(line.content.bytes());
        bytes.push(b'\n');
    }
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    Other,
}

impl FileStatus {
    pub fn label(self) -> &'static str {
        match self {
            FileStatus::Added => "A",
            FileStatus::Deleted => "D",
            FileStatus::Modified => "M",
            FileStatus::Renamed => "R",
            FileStatus::Copied => "C",
            FileStatus::Other => "?",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub status: FileStatus,
    pub is_binary: bool,
    pub hunks: Vec<Hunk>,
}

/// File names that are machine-written and rarely worth reading line by line.
const GENERATED_FILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "poetry.lock",
    "uv.lock",
    "Gemfile.lock",
    "composer.lock",
    "go.sum",
    "flake.lock",
];

impl FileDiff {
    /// The path the file has after the change, or before it for deletions.
    pub fn path(&self) -> &str {
        self.new_path.as_deref().or(self.old_path.as_deref()).unwrap_or("")
    }

    pub fn looks_generated(&self) -> bool {
        let name = self.path().rsplit('/').next().unwrap_or("");
        GENERATED_FILES.contains(&name) || name.ends_with(".min.js") || name.ends_with(".min.css")
    }

    /// Number of added and removed lines.
    pub fn stats(&self) -> (usize, usize) {
        let lines = self.hunks.iter().flat_map(|h| &h.lines);
        lines.fold((0, 0), |(added, removed), line| match line.kind {
            LineKind::Added => (added + 1, removed),
            LineKind::Removed => (added, removed + 1),
            _ => (added, removed),
        })
    }

    pub fn line_count(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len()).sum()
    }

    pub fn display_path(&self) -> String {
        match (&self.old_path, &self.new_path) {
            (Some(old), Some(new)) if old != new => format!("{old} → {new}"),
            (_, Some(path)) | (Some(path), None) => path.clone(),
            (None, None) => "<unknown>".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Hunk {
    /// Assigned by [`Diff::new`].
    pub id: HunkId,
    /// The full `@@ -a,b +c,d @@ context` line.
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    /// `\ No newline at end of file` markers.
    NoNewline,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: LineKind, no: u32, content: &str) -> DiffLine {
        DiffLine { kind, old_no: Some(no), new_no: Some(no), content: content.to_string() }
    }

    fn hunk(start: u32, lines: Vec<DiffLine>) -> Hunk {
        Hunk {
            id: HunkId::default(),
            header: String::new(),
            old_start: start,
            old_lines: 0,
            new_start: start,
            new_lines: 0,
            lines,
        }
    }

    fn file(path: &str, hunks: Vec<Hunk>) -> FileDiff {
        FileDiff {
            old_path: Some(path.into()),
            new_path: Some(path.into()),
            status: FileStatus::Modified,
            is_binary: false,
            hunks,
        }
    }

    fn ids(diff: &Diff) -> Vec<&HunkId> {
        diff.files.iter().flat_map(|f| &f.hunks).map(|h| &h.id).collect()
    }

    #[test]
    fn hunk_ids_ignore_position_and_context() {
        let a = Diff::new(vec![file(
            "a.rs",
            vec![hunk(1, vec![line(LineKind::Context, 1, "ctx"), line(LineKind::Added, 2, "x")])],
        )]);
        let moved = Diff::new(vec![file(
            "a.rs",
            vec![hunk(90, vec![line(LineKind::Context, 90, "other"), line(LineKind::Added, 91, "x")])],
        )]);
        assert_eq!(ids(&a), ids(&moved));
        assert_eq!(ids(&a)[0].0.len(), 16);
    }

    #[test]
    fn hunk_ids_depend_on_path_and_changes() {
        let make = |path: &str, kind, content: &str| {
            Diff::new(vec![file(path, vec![hunk(1, vec![line(kind, 1, content)])])])
        };
        let base = make("a.rs", LineKind::Added, "x");
        assert_ne!(ids(&base), ids(&make("b.rs", LineKind::Added, "x")));
        assert_ne!(ids(&base), ids(&make("a.rs", LineKind::Removed, "x")));
        assert_ne!(ids(&base), ids(&make("a.rs", LineKind::Added, "y")));
    }

    #[test]
    fn a_path_repeated_across_sections_keeps_ids_distinct() {
        let same = || file("a.rs", vec![hunk(1, vec![line(LineKind::Added, 1, "x")])]);
        let diff = Diff::sectioned(vec![
            ("one".into(), String::new(), vec![same()]),
            ("two".into(), String::new(), vec![same(), file("b.rs", Vec::new())]),
        ]);
        let ids = ids(&diff);
        assert_ne!(ids[0], ids[1]);
        assert_eq!(diff.sections[1].files, 1..3);
    }

    #[test]
    fn identical_hunks_in_a_file_get_distinct_ids() {
        let same = || hunk(1, vec![line(LineKind::Added, 1, "x")]);
        let diff = Diff::new(vec![file("a.rs", vec![same(), same()])]);
        let ids = ids(&diff);
        assert_ne!(ids[0], ids[1]);
        assert!(ids[1].0.ends_with("-2"));
    }
}
