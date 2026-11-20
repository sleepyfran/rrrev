//! Turning a diff into an ordered review plan: groups of related hunks, in
//! the order they're best read.

pub mod claude;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::diff::{Diff, Hunk, HunkId, LineKind};

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
    /// Where each planned hunk sat when last seen, so the plan can follow a
    /// hunk whose id changed because it was edited.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub anchors: BTreeMap<HunkId, Anchor>,
}

/// A hunk's file and line ranges: enough to find it again after an edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub path: String,
    /// `(start, lines)` on the old side.
    pub old: (u32, u32),
    /// `(start, lines)` on the new side.
    pub new: (u32, u32),
}

impl Anchor {
    fn of(path: &str, hunk: &Hunk) -> Self {
        Anchor { path: path.to_string(), old: (hunk.old_start, hunk.old_lines), new: (hunk.new_start, hunk.new_lines) }
    }
}

/// Whether two `(start, lines)` ranges share a line; an empty range (a pure
/// insertion or deletion) counts as the line it sits at.
fn overlaps(a: (u32, u32), b: (u32, u32)) -> bool {
    let end = |(start, lines): (u32, u32)| start + lines.max(1);
    a.0 < end(b) && b.0 < end(a)
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
        Plan { groups, notes, anchors: BTreeMap::new() }
    }

    /// Remembers where each planned hunk sits in `diff`. Anchors of hunks
    /// not in it are kept, for [`Plan::follow`].
    pub fn anchor(&mut self, diff: &Diff) {
        let ids: HashSet<&HunkId> = self.groups.iter().flat_map(|g| &g.hunks).collect();
        for file in &diff.files {
            for hunk in file.hunks.iter().filter(|h| ids.contains(&h.id)) {
                self.anchors.insert(hunk.id.clone(), Anchor::of(file.path(), hunk));
            }
        }
        self.anchors.retain(|id, _| ids.contains(id));
    }

    /// Follows hunks edited since the plan was made. Editing a hunk changes
    /// its id, which would drop it from its group; instead, a planned id no
    /// longer in `diff` gives way to the hunks now where it was (same file,
    /// overlapping lines, old side first) that the plan doesn't hold yet.
    /// A plan with none of the diff's hunks was made for another diff and
    /// is left alone. Returns how many planned hunks were followed.
    pub fn follow(&mut self, diff: &Diff) -> usize {
        let current: HashMap<&HunkId, &Hunk> = diff.files.iter().flat_map(|f| &f.hunks).map(|h| (&h.id, h)).collect();
        let mut claimed: HashSet<HunkId> =
            self.groups.iter().flat_map(|g| &g.hunks).filter(|id| current.contains_key(id)).cloned().collect();
        if claimed.is_empty() {
            return 0;
        }
        let mut moved: HashMap<HunkId, HunkId> = HashMap::new();
        for group in &mut self.groups {
            let mut hunks = Vec::with_capacity(group.hunks.len());
            for id in group.hunks.drain(..) {
                let anchor = self.anchors.get(&id);
                let found: Vec<&Hunk> = match anchor.filter(|_| !current.contains_key(&id)) {
                    Some(anchor) => {
                        let free: Vec<&Hunk> = diff
                            .files
                            .iter()
                            .filter(|f| f.path() == anchor.path)
                            .flat_map(|f| &f.hunks)
                            .filter(|h| !claimed.contains(&h.id))
                            .collect();
                        let on = |side: fn(&Hunk) -> (u32, u32), at: (u32, u32)| -> Vec<&Hunk> {
                            free.iter().copied().filter(|h| overlaps(side(h), at)).collect()
                        };
                        let old = on(|h| (h.old_start, h.old_lines), anchor.old);
                        if old.is_empty() { on(|h| (h.new_start, h.new_lines), anchor.new) } else { old }
                    }
                    None => Vec::new(),
                };
                if found.is_empty() {
                    // Still in the diff, or nowhere to be found: kept as is,
                    // in case it comes back.
                    hunks.push(id);
                    continue;
                }
                moved.insert(id, found[0].id.clone());
                for hunk in found {
                    claimed.insert(hunk.id.clone());
                    hunks.push(hunk.id.clone());
                }
            }
            group.hunks = hunks;
        }
        for note in &mut self.notes {
            let Some(to) = moved.get(&note.hunk) else { continue };
            note.hunk = to.clone();
            // The line it was about may have moved; then it goes under the hunk.
            let hunk = current[to];
            let inside = |line: u32| line >= hunk.new_start && line < hunk.new_start + hunk.new_lines;
            note.line = note.line.filter(|&line| inside(line));
        }
        self.anchor(diff);
        moved.len()
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
        self.anchors.retain(|id, _| !hunks.contains(id));
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
            ..Plan::default()
        };
        let known: HashSet<&HunkId> = [&one, &two].into();
        let clean = plan.sanitize(&known);
        assert_eq!(clean.notes, [note(&one, "look")]);
        assert_eq!(clean.groups.len(), 2);
        assert_eq!(clean.groups[0].hunks, [two]);
        assert_eq!(clean.groups[1].hunks, [one]);
    }

    fn at(path: &str, hunks: &[(u32, u32, &str)]) -> FileDiff {
        FileDiff {
            old_path: Some(path.to_string()),
            new_path: Some(path.to_string()),
            status: FileStatus::Modified,
            is_binary: false,
            hunks: hunks
                .iter()
                .map(|&(start, lines, content)| Hunk {
                    id: HunkId::default(),
                    header: String::new(),
                    old_start: start,
                    old_lines: lines,
                    new_start: start,
                    new_lines: lines,
                    lines: vec![DiffLine { kind: LineKind::Added, old_no: None, new_no: Some(start), content: content.into() }],
                })
                .collect(),
        }
    }

    fn planned(diff: &Diff, groups: &[&[(usize, usize)]]) -> Plan {
        let groups = groups
            .iter()
            .map(|hunks| PlanGroup {
                title: String::new(),
                rationale: String::new(),
                hunks: hunks.iter().map(|&(f, h)| diff.files[f].hunks[h].id.clone()).collect(),
            })
            .collect();
        let mut plan = Plan { groups, ..Plan::default() };
        plan.anchor(diff);
        plan
    }

    fn ids(diff: &Diff, hunks: &[(usize, usize)]) -> Vec<HunkId> {
        hunks.iter().map(|&(f, h)| diff.files[f].hunks[h].id.clone()).collect()
    }

    #[test]
    fn an_edited_hunk_split_in_two_keeps_both_halves_in_its_group() {
        let before = Diff::new(vec![at("a.rs", &[(10, 20, "big")]), at("b.rs", &[(1, 1, "b")])]);
        let mut plan = planned(&before, &[&[(1, 0)], &[(0, 0)]]);
        let after = Diff::new(vec![at("a.rs", &[(8, 4, "top"), (25, 2, "bottom"), (40, 1, "new")]), at("b.rs", &[(1, 1, "b")])]);
        assert_eq!(plan.follow(&after), 1);
        assert_eq!(plan.groups[0].hunks, ids(&after, &[(1, 0)]));
        assert_eq!(plan.groups[1].hunks, ids(&after, &[(0, 0), (0, 1)]));
        assert_eq!(plan.anchors.len(), 3);
    }

    #[test]
    fn a_hunk_the_plan_holds_is_not_taken_by_an_edited_neighbour() {
        let before = Diff::new(vec![at("a.rs", &[(10, 5, "one"), (14, 5, "two")])]);
        let mut plan = planned(&before, &[&[(0, 1)], &[(0, 0)]]);
        // The first is edited; the second, which it overlaps, is still there.
        let after = Diff::new(vec![at("a.rs", &[(10, 5, "one, edited"), (14, 5, "two")])]);
        assert_eq!(plan.follow(&after), 1);
        assert_eq!(plan.groups[0].hunks, ids(&after, &[(0, 1)]));
        assert_eq!(plan.groups[1].hunks, ids(&after, &[(0, 0)]));
    }

    #[test]
    fn a_plan_for_another_diff_is_not_followed() {
        let before = Diff::new(vec![at("a.rs", &[(10, 5, "one")])]);
        let mut plan = planned(&before, &[&[(0, 0)]]);
        let saved = plan.clone();
        let other = Diff::new(vec![at("a.rs", &[(10, 5, "something else")])]);
        assert_eq!(plan.follow(&other), 0);
        assert_eq!(plan, saved);
    }
}
