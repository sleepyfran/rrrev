use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::diff::HunkId;
use crate::organizer::Plan;
use crate::walkthrough::Walkthrough;

const VERSION: u32 = 1;

/// What the reviewer has done so far, saved between runs.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ReviewState {
    version: u32,
    /// Ids of reviewed hunks. Ids no longer in the diff are kept, so a change
    /// that comes back (after a stash, say) is still marked.
    reviewed: BTreeSet<HunkId>,
    #[serde(default)]
    comments: Vec<Comment>,
    #[serde(default)]
    next_comment_id: u64,
    /// The organizer's latest plan, as it returned it.
    #[serde(default)]
    plan: Option<Plan>,
    /// Per hunk, the indices of its changed lines that have been on screen.
    #[serde(default)]
    seen: BTreeMap<HunkId, BTreeSet<usize>>,
    /// Agent notes the reviewer dismissed, by [`note_key`].
    #[serde(default)]
    dismissed_notes: BTreeSet<String>,
    /// How the review ends, once the reviewer says.
    #[serde(default)]
    verdict: Option<Verdict>,
    /// The latest walkthrough, as the AI returned it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    walkthrough: Option<Walkthrough>,
    /// Set when there are changes that haven't been saved yet.
    #[serde(skip)]
    dirty: bool,
}

/// A note on a range of lines within one hunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: u64,
    pub hunk: HunkId,
    pub path: String,
    /// Inclusive range of indices into the hunk's lines.
    pub start: usize,
    pub end: usize,
    /// File line numbers covered, on each side, when that side has any lines.
    pub old_lines: Option<(u32, u32)>,
    pub new_lines: Option<(u32, u32)>,
    /// The commented lines as they were, with their `+`/`-`/` ` sign, so the
    /// comment still makes sense once the hunk has changed.
    pub code: Vec<String>,
    pub text: String,
}

/// What the reviewer makes of the change as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictKind {
    Approve,
    RequestChanges,
    Comment,
}

impl VerdictKind {
    pub fn label(self) -> &'static str {
        match self {
            VerdictKind::Approve => "Approved",
            VerdictKind::RequestChanges => "Changes requested",
            VerdictKind::Comment => "Commented",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub kind: VerdictKind,
    /// A word to the author; may be empty.
    #[serde(default)]
    pub message: String,
}

/// Identifies an agent note across runs: its hunk and its text.
pub fn note_key(hunk: &HunkId, text: &str) -> String {
    format!("{}:{}", hunk.as_str(), text.trim())
}

impl ReviewState {
    /// Loads saved state, or starts empty if there is none yet.
    pub fn load(path: &Path) -> Result<Self> {
        let json = match fs::read_to_string(path) {
            Ok(json) => json,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        serde_json::from_str(&json)
            .with_context(|| format!("invalid review state in {}", path.display()))
    }

    /// Writes to a temporary file first, so a crash can't leave a truncated one.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut json = serde_json::to_value(self)?;
        json["version"] = VERSION.into();
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(&json)?)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn is_reviewed(&self, id: &HunkId) -> bool {
        self.reviewed.contains(id)
    }

    pub fn set_reviewed(&mut self, id: &HunkId, reviewed: bool) {
        let changed = if reviewed {
            self.reviewed.insert(id.clone())
        } else {
            self.reviewed.remove(id)
        };
        self.dirty |= changed;
    }

    pub fn comments(&self) -> &[Comment] {
        &self.comments
    }

    pub fn comment(&self, id: u64) -> Option<&Comment> {
        self.comments.iter().find(|c| c.id == id)
    }

    /// Stores `comment` under a fresh id, which it returns.
    pub fn add_comment(&mut self, mut comment: Comment) -> u64 {
        self.next_comment_id += 1;
        comment.id = self.next_comment_id;
        self.comments.push(comment);
        self.dirty = true;
        self.next_comment_id
    }

    pub fn set_comment_text(&mut self, id: u64, text: String) {
        if let Some(comment) = self.comments.iter_mut().find(|c| c.id == id) {
            comment.text = text;
            self.dirty = true;
        }
    }

    pub fn delete_comment(&mut self, id: u64) {
        let before = self.comments.len();
        self.comments.retain(|c| c.id != id);
        self.dirty |= self.comments.len() != before;
    }

    pub fn plan(&self) -> Option<&Plan> {
        self.plan.as_ref()
    }

    pub fn set_plan(&mut self, plan: Plan) {
        self.plan = Some(plan);
        self.dirty = true;
    }

    pub fn clear_plan(&mut self) {
        self.dirty |= self.plan.take().is_some();
    }

    pub fn walkthrough(&self) -> Option<&Walkthrough> {
        self.walkthrough.as_ref()
    }

    pub fn set_walkthrough(&mut self, walkthrough: Walkthrough) {
        self.walkthrough = Some(walkthrough);
        self.dirty = true;
    }

    pub fn clear_walkthrough(&mut self) {
        self.dirty |= self.walkthrough.take().is_some();
    }

    /// Whether line `line` of hunk `id` has been on screen.
    pub fn is_seen(&self, id: &HunkId, line: usize) -> bool {
        self.seen.get(id).is_some_and(|lines| lines.contains(&line))
    }

    /// Notes lines of hunk `id` as having been on screen.
    pub fn mark_seen(&mut self, id: &HunkId, lines: impl IntoIterator<Item = usize>) {
        let set = self.seen.entry(id.clone()).or_default();
        let before = set.len();
        set.extend(lines);
        self.dirty |= set.len() != before;
    }

    pub fn is_note_dismissed(&self, key: &str) -> bool {
        self.dismissed_notes.contains(key)
    }

    pub fn dismiss_note(&mut self, key: String) {
        self.dirty |= self.dismissed_notes.insert(key);
    }

    pub fn verdict(&self) -> Option<&Verdict> {
        self.verdict.as_ref()
    }

    pub fn set_verdict(&mut self, verdict: Option<Verdict>) {
        if self.verdict != verdict {
            self.verdict = verdict;
            self.dirty = true;
        }
    }

    /// Forces the next save, for an explicit `:w`.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Returns whether there were unsaved changes, and clears the flag.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{Diff, DiffLine, FileDiff, FileStatus, Hunk, LineKind};

    fn some_id() -> HunkId {
        let line = DiffLine {
            kind: LineKind::Added,
            old_no: None,
            new_no: Some(1),
            content: "x".into(),
        };
        let hunk = Hunk {
            id: HunkId::default(),
            header: String::new(),
            old_start: 0,
            old_lines: 0,
            new_start: 1,
            new_lines: 1,
            lines: vec![line],
        };
        let file = FileDiff {
            old_path: None,
            new_path: Some("a.rs".into()),
            status: FileStatus::Added,
            is_binary: false,
            hunks: vec![hunk],
        };
        Diff::new(vec![file]).files[0].hunks[0].id.clone()
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let state = ReviewState::load(&dir.path().join("nope.json")).unwrap();
        assert!(!state.is_reviewed(&some_id()));
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rrrev").join("state.json");
        let id = some_id();

        let mut state = ReviewState::default();
        state.set_reviewed(&id, true);
        assert!(state.take_dirty());
        assert!(!state.take_dirty());
        state.save(&path).unwrap();

        let loaded = ReviewState::load(&path).unwrap();
        assert!(loaded.is_reviewed(&id));
        assert_eq!(loaded.version, VERSION);
    }

    #[test]
    fn only_real_changes_mark_dirty() {
        let id = some_id();
        let mut state = ReviewState::default();
        state.set_reviewed(&id, false);
        assert!(!state.take_dirty());
        state.set_reviewed(&id, true);
        state.take_dirty();
        state.set_reviewed(&id, true);
        assert!(!state.take_dirty());
    }

    fn comment(text: &str) -> Comment {
        Comment {
            id: 0,
            hunk: some_id(),
            path: "a.rs".into(),
            start: 0,
            end: 0,
            old_lines: None,
            new_lines: Some((1, 1)),
            code: vec!["+x".into()],
            text: text.into(),
        }
    }

    #[test]
    fn comments_get_fresh_ids_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut state = ReviewState::default();
        let first = state.add_comment(comment("one"));
        let second = state.add_comment(comment("two"));
        assert_ne!(first, second);
        state.delete_comment(first);
        // Ids aren't reused after a delete.
        assert!(state.add_comment(comment("three")) > second);
        state.set_comment_text(second, "edited".into());
        state.save(&path).unwrap();

        let loaded = ReviewState::load(&path).unwrap();
        let texts: Vec<_> = loaded.comments().iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["edited", "three"]);
        assert_eq!(loaded.comment(second).unwrap().code, ["+x"]);
    }

    #[test]
    fn loads_state_saved_before_comments_existed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        fs::write(&path, r#"{ "version": 1, "reviewed": ["abc"] }"#).unwrap();
        let state = ReviewState::load(&path).unwrap();
        assert!(state.comments().is_empty());
        assert!(state.plan().is_none());
    }

    #[test]
    fn seen_lines_notes_and_verdict_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let id = some_id();
        let mut state = ReviewState::default();
        state.mark_seen(&id, [0, 2]);
        assert!(state.take_dirty());
        // Lines already seen change nothing.
        state.mark_seen(&id, [2]);
        assert!(!state.take_dirty());
        state.dismiss_note(note_key(&id, "watch out "));
        state.set_verdict(Some(Verdict {
            kind: VerdictKind::Approve,
            message: "ship it".into(),
        }));
        state.save(&path).unwrap();

        let loaded = ReviewState::load(&path).unwrap();
        assert!(loaded.is_seen(&id, 2) && !loaded.is_seen(&id, 1));
        assert!(loaded.is_note_dismissed(&note_key(&id, "watch out")));
        assert_eq!(loaded.verdict().unwrap().kind, VerdictKind::Approve);
        let json = std::fs::read_to_string(&path).unwrap();
        assert!(json.contains("\"approve\""));
    }

    #[test]
    fn corrupt_state_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        fs::write(&path, "{ nope").unwrap();
        assert!(ReviewState::load(&path).is_err());
    }
}
