use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::diff::Diff;
use crate::review::{ReviewState, Verdict};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Json,
}

impl Format {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "md" | "markdown" => Some(Format::Markdown),
            "json" => Some(Format::Json),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Json => "json",
        }
    }
}

/// Review comments in a shape meant for whoever acts on them next, a person
/// or an AI agent.
#[derive(Debug, Serialize)]
pub struct Export {
    pub title: String,
    /// The reviewer's verdict, once they've given one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    pub comments: Vec<ExportedComment>,
}

#[derive(Debug, Serialize)]
pub struct ExportedComment {
    pub path: String,
    pub old_lines: Option<(u32, u32)>,
    pub new_lines: Option<(u32, u32)>,
    pub code: Vec<String>,
    pub text: String,
    /// The hunk it was written on is no longer in the diff.
    pub outdated: bool,
}

impl ExportedComment {
    /// `path:41-43`, using new-side lines when there are any.
    pub fn location(&self) -> String {
        let range = |(a, b): (u32, u32)| if a == b { a.to_string() } else { format!("{a}-{b}") };
        match (self.new_lines, self.old_lines) {
            (Some(new), _) => format!("{}:{}", self.path, range(new)),
            (None, Some(old)) => format!("{} (removed lines {})", self.path, range(old)),
            (None, None) => self.path.clone(),
        }
    }
}

/// Gathers every comment, in file order then line order.
pub fn collect(title: &str, diff: &Diff, review: &ReviewState) -> Export {
    let live = |c: &crate::review::Comment| diff.files.iter().flat_map(|f| &f.hunks).any(|h| h.id == c.hunk);
    let mut comments: Vec<_> = review
        .comments()
        .iter()
        .map(|c| ExportedComment {
            path: c.path.clone(),
            old_lines: c.old_lines,
            new_lines: c.new_lines,
            code: c.code.clone(),
            text: c.text.clone(),
            outdated: !live(c),
        })
        .collect();
    comments.sort_by_key(|c| (c.path.clone(), c.new_lines.or(c.old_lines)));
    Export { title: title.to_string(), verdict: review.verdict().cloned(), comments }
}

/// The comments as a short brief for a coding agent: each as `path:line`
/// and what the reviewer wrote, the way you'd paste them into a chat.
pub fn agent_brief(export: &Export) -> String {
    let mut out = format!("Review comments on {} to address:\n", export.title);
    if let Some(verdict) = export.verdict.as_ref().filter(|v| !v.message.trim().is_empty()) {
        out += &format!("\nOverall: {}\n", verdict.message.trim());
    }
    for comment in &export.comments {
        let outdated = if comment.outdated { " (on code that has since changed)" } else { "" };
        let mut lines = comment.text.trim().lines();
        out += &format!("\n- `{}`{outdated}: {}\n", comment.location(), lines.next().unwrap_or(""));
        for line in lines {
            out += &format!("  {line}\n");
        }
    }
    out
}

pub fn render(export: &Export, format: Format) -> Result<String> {
    Ok(match format {
        Format::Json => serde_json::to_string_pretty(export)? + "\n",
        Format::Markdown => markdown(export),
    })
}

fn markdown(export: &Export) -> String {
    let mut out = format!("# Review comments: {} ({})\n", export.title, export.comments.len());
    if let Some(verdict) = &export.verdict {
        out += &format!("\n**{}**", verdict.kind.label());
        let message = verdict.message.trim();
        out += &if message.is_empty() { "\n".to_string() } else { format!(": {message}\n") };
    }
    for comment in &export.comments {
        let outdated = if comment.outdated { " (outdated)" } else { "" };
        out += &format!("\n## {}{outdated}\n\n", comment.location());
        // A fence longer than any backtick run in the code can't be closed early.
        let longest_run = comment
            .code
            .iter()
            .flat_map(|l| l.split(|c| c != '`'))
            .map(str::len)
            .max()
            .unwrap_or(0);
        let fence = "`".repeat(longest_run.max(2) + 1);
        out += &format!("{fence}diff\n{}\n{fence}\n\n{}\n", comment.code.join("\n"), comment.text.trim());
    }
    out
}

/// Where exported comments go. A file is the only destination today; handing
/// them straight to an AI agent to apply will be another.
pub trait CommentSink {
    /// Delivers the comments and returns a short note for the status line.
    fn send(&self, export: &Export) -> Result<String>;
}

pub struct FileSink {
    pub path: PathBuf,
    pub format: Format,
}

impl CommentSink for FileSink {
    fn send(&self, export: &Export) -> Result<String> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&self.path, render(export, self.format)?)
            .with_context(|| format!("cannot write {}", self.path.display()))?;
        let n = export.comments.len();
        Ok(format!("Exported {n} comment{} to {}", if n == 1 { "" } else { "s" }, self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exported(path: &str, new_lines: Option<(u32, u32)>, code: &[&str], text: &str) -> ExportedComment {
        ExportedComment {
            path: path.into(),
            old_lines: Some((7, 7)),
            new_lines,
            code: code.iter().map(|s| s.to_string()).collect(),
            text: text.into(),
            outdated: false,
        }
    }

    #[test]
    fn locations_prefer_new_lines() {
        assert_eq!(exported("a.rs", Some((3, 5)), &[], "").location(), "a.rs:3-5");
        assert_eq!(exported("a.rs", Some((3, 3)), &[], "").location(), "a.rs:3");
        assert_eq!(exported("a.rs", None, &[], "").location(), "a.rs (removed lines 7)");
    }

    #[test]
    fn markdown_quotes_code_and_comment() {
        let export = Export {
            title: "main..feat".into(),
            verdict: None,
            comments: vec![exported("a.rs", Some((3, 3)), &["-old", "+new"], "Why?\n")],
        };
        let md = render(&export, Format::Markdown).unwrap();
        assert_eq!(
            md,
            "# Review comments: main..feat (1)\n\n## a.rs:3\n\n```diff\n-old\n+new\n```\n\nWhy?\n"
        );
    }

    #[test]
    fn markdown_fence_outgrows_backticks_in_code() {
        let export = Export {
            title: String::new(),
            verdict: None,
            comments: vec![exported("README.md", Some((1, 1)), &["+```rust"], "x")],
        };
        assert!(render(&export, Format::Markdown).unwrap().contains("````diff\n+```rust\n````"));
    }

    #[test]
    fn verdicts_lead_the_markdown_and_the_brief() {
        use crate::review::VerdictKind;
        let mut outdated = exported("b.rs", Some((9, 9)), &[], "Rename\nthis too");
        outdated.outdated = true;
        let export = Export {
            title: "main..feat".into(),
            verdict: Some(Verdict { kind: VerdictKind::RequestChanges, message: "Close, two nits.".into() }),
            comments: vec![exported("a.rs", Some((3, 4)), &["+x"], "Why two locks?"), outdated],
        };
        let md = render(&export, Format::Markdown).unwrap();
        assert!(md.starts_with("# Review comments: main..feat (2)\n\n**Changes requested**: Close, two nits.\n"));
        assert_eq!(
            agent_brief(&export),
            "Review comments on main..feat to address:\n\nOverall: Close, two nits.\n\n\
             - `a.rs:3-4`: Why two locks?\n\n\
             - `b.rs:9` (on code that has since changed): Rename\n  this too\n"
        );
    }

    #[test]
    fn file_sink_writes_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out").join("c.json");
        let export = Export { title: String::new(), verdict: None, comments: vec![exported("a.rs", None, &[], "x")] };
        let note = FileSink { path: path.clone(), format: Format::Json }.send(&export).unwrap();
        assert!(note.starts_with("Exported 1 comment to"));
        let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(json["comments"][0]["text"], "x");
    }
}
