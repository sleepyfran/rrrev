use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::diff::HunkId;

const VERSION: u32 = 1;

/// What the reviewer has done so far, saved between runs.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ReviewState {
    version: u32,
    /// Ids of reviewed hunks. Ids no longer in the diff are kept, so a change
    /// that comes back (after a stash, say) is still marked.
    reviewed: BTreeSet<HunkId>,
    /// Set when there are changes that haven't been saved yet.
    #[serde(skip)]
    dirty: bool,
}

impl ReviewState {
    /// Loads saved state, or starts empty if there is none yet.
    pub fn load(path: &Path) -> Result<Self> {
        let json = match fs::read_to_string(path) {
            Ok(json) => json,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        serde_json::from_str(&json).with_context(|| format!("invalid review state in {}", path.display()))
    }

    /// Writes to a temporary file first, so a crash can't leave a truncated one.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let state = Self { version: VERSION, reviewed: self.reviewed.clone(), dirty: false };
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(&state)?)?;
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
        let line = DiffLine { kind: LineKind::Added, old_no: None, new_no: Some(1), content: "x".into() };
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

    #[test]
    fn corrupt_state_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        fs::write(&path, "{ nope").unwrap();
        assert!(ReviewState::load(&path).is_err());
    }
}
