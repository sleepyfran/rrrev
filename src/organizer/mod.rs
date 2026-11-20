//! Turning a diff into an ordered review plan: groups of related hunks, in
//! the order they're best read.

pub mod claude;

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::diff::{Diff, HunkId, LineKind};

/// Something that can plan a review. The `claude` CLI is the only one today.
/// Runs on a background thread, so it may block, but should give up soon
/// after `cancel` fires.
pub trait Organizer: Send + Sync {
    fn organize(&self, request: &OrganizeRequest, cancel: &Cancel) -> Result<Plan>;
}

/// Asks a running organizer to stop. Clones share the same flag.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub groups: Vec<PlanGroup>,
    /// What the organizer thinks deserves a closer look, shown in the diff
    /// next to the code it's about.
    #[serde(default)]
    pub notes: Vec<AgentNote>,
}

/// A remark from the AI on one hunk: a subtle behaviour change, a risk,
/// something easy to miss.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentNote {
    pub hunk: HunkId,
    /// The new-side line it's about, if it's about one; otherwise the note
    /// goes under the hunk.
    #[serde(default)]
    pub line: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanGroup {
    pub title: String,
    pub rationale: String,
    pub hunks: Vec<HunkId>,
}

impl Plan {
    /// Makes a plan from a model safe to use: ids not in the diff are
    /// dropped, a hunk listed twice stays in its first group, and groups left
    /// empty are removed. Hunks it leaves out are the caller's to place.
    pub fn sanitize(self, known: &HashSet<&HunkId>) -> Plan {
        let notes =
            self.notes.into_iter().filter(|n| known.contains(&n.hunk) && !n.text.trim().is_empty()).collect();
        let mut seen = HashSet::new();
        let groups = self
            .groups
            .into_iter()
            .map(|mut group| {
                group.hunks.retain(|id| known.contains(id) && seen.insert(id.clone()));
                group
            })
            .filter(|group| !group.hunks.is_empty())
            .collect();
        Plan { groups, notes }
    }

    /// Whether any group or note is about one of `hunks`.
    pub fn covers(&self, hunks: &HashSet<&HunkId>) -> bool {
        self.groups.iter().flat_map(|g| &g.hunks).chain(self.notes.iter().map(|n| &n.hunk)).any(|id| hunks.contains(id))
    }

    /// Drops everything about `hunks`, and groups left empty: what replanning
    /// one stack layer does to the others' plan.
    pub fn forget(&mut self, hunks: &HashSet<&HunkId>) {
        for group in &mut self.groups {
            group.hunks.retain(|id| !hunks.contains(id));
        }
        self.groups.retain(|group| !group.hunks.is_empty());
        self.notes.retain(|note| !hunks.contains(&note.hunk));
    }
}

/// The diff as the organizer sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrganizeRequest {
    pub title: String,
    pub hunks: Vec<HunkSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkSummary {
    pub id: HunkId,
    pub path: String,
    pub header: String,
    /// Diff lines with their sign; empty for generated files.
    pub lines: Vec<String>,
    pub generated: bool,
}

/// Lines shown per hunk; past that the organizer gets a count. Plenty to tell
/// what a hunk is about without blowing up the prompt on huge diffs.
const MAX_LINES_PER_HUNK: usize = 80;

impl OrganizeRequest {
    #[cfg(test)]
    pub fn from_diff(title: &str, diff: &Diff) -> Self {
        Self::from_files(title, diff, 0..diff.files.len())
    }

    /// The hunks of `files` only, like one layer of a stack.
    pub fn from_files(title: &str, diff: &Diff, files: Range<usize>) -> Self {
        let hunks = diff.files[files]
            .iter()
            .flat_map(|file| {
                let generated = file.looks_generated();
                file.hunks.iter().map(move |hunk| HunkSummary {
                    id: hunk.id.clone(),
                    path: file.path().to_string(),
                    header: hunk.header.clone(),
                    lines: if generated {
                        Vec::new()
                    } else {
                        hunk.lines
                            .iter()
                            .map(|line| {
                                let sign = match line.kind {
                                    LineKind::Added => '+',
                                    LineKind::Removed => '-',
                                    LineKind::Context | LineKind::NoNewline => ' ',
                                };
                                format!("{sign}{}", line.content)
                            })
                            .collect()
                    },
                    generated,
                })
            })
            .collect();
        Self { title: title.to_string(), hunks }
    }

    /// The diff as plain text, each hunk introduced by `=== <id> <path> <header>`.
    pub fn to_prompt(&self) -> String {
        let mut out = format!("Diff under review: {}\n{} hunks follow.\n", self.title, self.hunks.len());
        for hunk in &self.hunks {
            out += &format!("\n=== {} {} {}\n", hunk.id.as_str(), hunk.path, hunk.header);
            if hunk.generated {
                out += "(generated file, content omitted)\n";
                continue;
            }
            for line in hunk.lines.iter().take(MAX_LINES_PER_HUNK) {
                out += line;
                out.push('\n');
            }
            if hunk.lines.len() > MAX_LINES_PER_HUNK {
                out += &format!("… {} more lines\n", hunk.lines.len() - MAX_LINES_PER_HUNK);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{DiffLine, FileDiff, FileStatus, Hunk};

    fn diff(files: &[(&str, &[&str])]) -> Diff {
        let files = files
            .iter()
            .map(|(path, hunks)| FileDiff {
                old_path: Some(path.to_string()),
                new_path: Some(path.to_string()),
                status: FileStatus::Modified,
                is_binary: false,
                hunks: hunks
                    .iter()
                    .map(|content| Hunk {
                        id: HunkId::default(),
                        header: "@@ -1 +1 @@".into(),
                        old_start: 1,
                        old_lines: 1,
                        new_start: 1,
                        new_lines: 1,
                        lines: vec![DiffLine {
                            kind: LineKind::Added,
                            old_no: None,
                            new_no: Some(1),
                            content: content.to_string(),
                        }],
                    })
                    .collect(),
            })
            .collect();
        Diff::new(files)
    }

    #[test]
    fn prompt_lists_every_hunk_with_its_id() {
        let diff = diff(&[("src/a.rs", &["fn a()"]), ("Cargo.lock", &["version"])]);
        let request = OrganizeRequest::from_diff("main..feat", &diff);
        let prompt = request.to_prompt();
        let a = diff.files[0].hunks[0].id.as_str();
        assert!(prompt.starts_with("Diff under review: main..feat\n2 hunks follow.\n"));
        assert!(prompt.contains(&format!("=== {a} src/a.rs @@ -1 +1 @@\n+fn a()\n")));
        // Generated files are listed so they get placed, but without content.
        assert!(prompt.contains("Cargo.lock @@ -1 +1 @@\n(generated file, content omitted)\n"));
        assert!(!prompt.contains("+version"));
    }

    #[test]
    fn long_hunks_are_cut_short() {
        let mut diff = diff(&[("a.rs", &["x"])]);
        let line = diff.files[0].hunks[0].lines[0].clone();
        diff.files[0].hunks[0].lines = vec![line; MAX_LINES_PER_HUNK + 5];
        let prompt = OrganizeRequest::from_diff("", &diff).to_prompt();
        assert_eq!(prompt.matches("+x\n").count(), MAX_LINES_PER_HUNK);
        assert!(prompt.contains("… 5 more lines"));
    }

    #[test]
    fn sanitize_drops_unknown_duplicate_and_empty() {
        let diff = diff(&[("a.rs", &["1", "2"])]);
        let [one, two] = [0, 1].map(|h| diff.files[0].hunks[h].id.clone());
        let bogus: HunkId = serde_json::from_str("\"nope\"").unwrap();
        let group = |title: &str, hunks: Vec<HunkId>| PlanGroup { title: title.into(), rationale: String::new(), hunks };
        let note = |hunk: &HunkId, text: &str| AgentNote { hunk: hunk.clone(), line: None, text: text.into() };
        let plan = Plan {
            groups: vec![
                group("first", vec![two.clone(), bogus.clone()]),
                group("only bogus", vec![bogus.clone()]),
                group("second", vec![one.clone(), two.clone()]),
            ],
            notes: vec![note(&one, "look"), note(&bogus, "gone"), note(&two, "  ")],
        };
        let known: HashSet<&HunkId> = [&one, &two].into();
        let clean = plan.sanitize(&known);
        assert_eq!(clean.notes, [note(&one, "look")]);
        assert_eq!(clean.groups.len(), 2);
        assert_eq!(clean.groups[0].hunks, [two]);
        assert_eq!(clean.groups[1].hunks, [one]);
    }
}
