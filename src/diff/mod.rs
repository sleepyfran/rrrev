//! Source-agnostic diff model, plus the trait every diff source implements.

pub mod git;

use anyhow::Result;

/// Anything that can produce a diff to review. Only git exists today; jj or a
/// piped-in patch would be further implementations.
pub trait DiffSource {
    /// Short label for the title bar, e.g. `main..feature`.
    fn describe(&self) -> String;
    fn load(&self) -> Result<Diff>;
}

#[derive(Debug, Clone, Default)]
pub struct Diff {
    pub files: Vec<FileDiff>,
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

impl FileDiff {
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
