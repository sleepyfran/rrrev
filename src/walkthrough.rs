//! A walkthrough of a change: the AI sorts its hunks into what matters and
//! what's plumbing, then presents what matters as slides, in the order that
//! makes the change easiest to follow, each explaining the code next to it.

use std::collections::HashSet;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::diff::{Hunk, HunkId, LineKind};
use crate::organizer::{Cancel, OrganizeRequest};

/// Something that can walk a reviewer through a diff. The `claude` CLI is
/// the only one today. Runs on a background thread, like the organizer.
pub trait Narrator: Send + Sync {
    fn walk_through(&self, request: &OrganizeRequest, cancel: &Cancel) -> Result<Walkthrough>;
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Walkthrough {
    /// What the change does as a whole, in Markdown: the opening slide.
    #[serde(default)]
    pub summary: String,
    pub slides: Vec<Slide>,
    /// Hunks judged plumbing: left out of the slides, listed at the end.
    #[serde(default)]
    pub plumbing: Vec<HunkId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slide {
    pub title: String,
    /// The explanation, in Markdown.
    pub body: String,
    /// The code it's about, in reading order.
    #[serde(default)]
    pub code: Vec<CodeRef>,
}

/// A hunk on a slide, whole or only some of its new-side lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeRef {
    pub hunk: HunkId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<u32>,
}

impl Walkthrough {
    /// Makes a walkthrough from a model safe to use: code on hunks not in
    /// the diff is dropped, as are slides left with nothing to say, and a
    /// hunk on a slide isn't plumbing. Hunks it leaves out are the caller's
    /// to list.
    pub fn sanitize(self, known: &HashSet<&HunkId>) -> Walkthrough {
        let slides: Vec<Slide> = self
            .slides
            .into_iter()
            .map(|mut slide| {
                slide.code.retain(|c| known.contains(&c.hunk));
                for code in &mut slide.code {
                    if let (Some(from), Some(to)) = (code.from, code.to)
                        && from > to
                    {
                        (code.from, code.to) = (Some(to), Some(from));
                    }
                }
                slide
            })
            .filter(|s| !s.code.is_empty() || !s.body.trim().is_empty())
            .collect();
        let shown: HashSet<&HunkId> = slides
            .iter()
            .flat_map(|s| &s.code)
            .map(|c| &c.hunk)
            .collect();
        let mut seen = HashSet::new();
        let plumbing = self
            .plumbing
            .into_iter()
            .filter(|id| known.contains(id) && !shown.contains(id) && seen.insert(id.clone()))
            .collect();
        Walkthrough {
            summary: self.summary.trim().to_string(),
            slides,
            plumbing,
        }
    }

    /// Every hunk it mentions, on a slide or as plumbing.
    pub fn hunks(&self) -> impl Iterator<Item = &HunkId> {
        self.slides
            .iter()
            .flat_map(|s| s.code.iter().map(|c| &c.hunk))
            .chain(&self.plumbing)
    }

    /// Whether it's about any of `hunks`.
    pub fn covers(&self, hunks: &HashSet<&HunkId>) -> bool {
        self.hunks().any(|id| hunks.contains(id))
    }
}

impl CodeRef {
    /// The indices of the hunk's lines this shows: those whose new-side line
    /// is within `from..=to`, a removed line counting as the line it sat
    /// before. With no range, or one that misses the hunk, all of them.
    pub fn lines(&self, hunk: &Hunk) -> Vec<usize> {
        let all = || (0..hunk.lines.len()).collect();
        if self.from.is_none() && self.to.is_none() {
            return all();
        }
        let (from, to) = (self.from.unwrap_or(0), self.to.unwrap_or(u32::MAX));
        let mut positions = vec![0; hunk.lines.len()];
        // Backwards, so a removed line takes the number of what follows it.
        let mut next = hunk.new_start + hunk.new_lines;
        for (i, line) in hunk.lines.iter().enumerate().rev() {
            if let Some(no) = line.new_no.filter(|_| line.kind != LineKind::Removed) {
                next = no;
            }
            positions[i] = next;
        }
        // `\ No newline` goes with the line before it.
        for i in 1..hunk.lines.len() {
            if hunk.lines[i].kind == LineKind::NoNewline {
                positions[i] = positions[i - 1];
            }
        }
        let picked: Vec<usize> = (0..hunk.lines.len())
            .filter(|&i| (from..=to).contains(&positions[i]))
            .collect();
        if picked.is_empty() { all() } else { picked }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{Diff, DiffLine, FileDiff, FileStatus};

    fn line(kind: LineKind, old_no: Option<u32>, new_no: Option<u32>) -> DiffLine {
        DiffLine {
            kind,
            old_no,
            new_no,
            content: format!("{kind:?} {old_no:?} {new_no:?}"),
        }
    }

    /// New side 10-14: context 10, removed (old 11), added 11 and 12,
    /// context 13, removed (old 14) at the end.
    fn hunk() -> Hunk {
        Hunk {
            id: HunkId::default(),
            header: "@@ -10,5 +10,4 @@".into(),
            old_start: 10,
            old_lines: 5,
            new_start: 10,
            new_lines: 4,
            lines: vec![
                line(LineKind::Context, Some(10), Some(10)),
                line(LineKind::Removed, Some(11), None),
                line(LineKind::Added, None, Some(11)),
                line(LineKind::Added, None, Some(12)),
                line(LineKind::Context, Some(12), Some(13)),
                line(LineKind::Removed, Some(13), None),
            ],
        }
    }

    fn code(from: Option<u32>, to: Option<u32>) -> CodeRef {
        CodeRef {
            hunk: HunkId::default(),
            from,
            to,
        }
    }

    #[test]
    fn picks_lines_by_their_new_side_number() {
        let hunk = hunk();
        assert_eq!(code(None, None).lines(&hunk), [0, 1, 2, 3, 4, 5]);
        // The removed line sits before 11, so it comes with it.
        assert_eq!(code(Some(11), Some(12)).lines(&hunk), [1, 2, 3]);
        assert_eq!(code(Some(13), Some(13)).lines(&hunk), [4]);
        // A removal at the end sits after the last line.
        assert_eq!(code(Some(14), None).lines(&hunk), [5]);
        assert_eq!(code(Some(13), None).lines(&hunk), [4, 5]);
        // A range that misses the hunk shows it whole.
        assert_eq!(code(Some(90), Some(99)).lines(&hunk).len(), 6);
    }

    #[test]
    fn sanitize_drops_unknown_code_and_empty_slides() {
        let files = vec![FileDiff {
            old_path: Some("a.rs".into()),
            new_path: Some("a.rs".into()),
            status: FileStatus::Modified,
            is_binary: false,
            hunks: vec![
                Hunk {
                    lines: vec![line(LineKind::Added, None, Some(1))],
                    ..hunk()
                },
                hunk(),
            ],
        }];
        let diff = Diff::new(files);
        let [one, two] = [0, 1].map(|h| diff.files[0].hunks[h].id.clone());
        let bogus: HunkId = serde_json::from_str("\"nope\"").unwrap();
        let at = |hunk: &HunkId, from, to| CodeRef {
            hunk: hunk.clone(),
            from,
            to,
        };
        let walkthrough = Walkthrough {
            summary: "  Adds things.\n".into(),
            slides: vec![
                Slide {
                    title: "Types".into(),
                    body: "First.".into(),
                    code: vec![at(&one, Some(20), Some(12)), at(&bogus, None, None)],
                },
                Slide {
                    title: "Gone".into(),
                    body: " ".into(),
                    code: vec![at(&bogus, None, None)],
                },
            ],
            plumbing: vec![one.clone(), two.clone(), two.clone(), bogus],
        };
        let known: HashSet<&HunkId> = [&one, &two].into();
        let clean = walkthrough.sanitize(&known);
        assert_eq!(clean.summary, "Adds things.");
        assert_eq!(clean.slides.len(), 1);
        assert_eq!(clean.slides[0].code, [at(&one, Some(12), Some(20))]);
        // On a slide, so not plumbing; and listed once.
        assert_eq!(clean.plumbing, std::slice::from_ref(&two));
        assert!(clean.covers(&[&two].into()));
    }
}
