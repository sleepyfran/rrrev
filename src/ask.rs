//! Asking an AI about the code under the cursor, without leaving the review.

use anyhow::Result;

use crate::organizer::Cancel;

/// Something that answers questions about the code being reviewed. The
/// `claude` CLI is the only one today. Runs on a background thread, so it may
/// block, but should give up soon after `cancel` fires.
pub trait Assistant: Send + Sync {
    fn ask(&self, request: &AskRequest, cancel: &Cancel) -> Result<String>;
}

/// A question, with what it points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskRequest {
    /// The diff under review, e.g. `main..feature`.
    pub title: String,
    /// `path:line` or `path:first-last`.
    pub location: String,
    /// The definitions around it, outermost first.
    pub scopes: Vec<String>,
    /// The code pointed at, with `+` / `-` / ` ` signs.
    pub code: Vec<String>,
    /// The whole hunk it's in, for context, signed the same way.
    pub hunk: Vec<String>,
    pub question: String,
    /// Earlier questions and answers in this review, oldest first.
    pub history: Vec<(String, String)>,
}

/// Lines of the surrounding hunk sent along; past that, a count.
const MAX_HUNK_LINES: usize = 120;
/// Earlier exchanges sent along, newest kept.
const MAX_HISTORY: usize = 4;

impl AskRequest {
    pub fn to_prompt(&self) -> String {
        let mut out = format!("Diff under review: {}\n", self.title);
        let skip = self.history.len().saturating_sub(MAX_HISTORY);
        for (question, answer) in self.history.iter().skip(skip) {
            out += &format!("\nEarlier question: {question}\nYour answer: {answer}\n");
        }
        out += &format!("\nThe reviewer is looking at {}", self.location);
        if !self.scopes.is_empty() {
            out += &format!(", inside {}", self.scopes.join(" › "));
        }
        out += ".\n\nThe code they point at:\n";
        for line in &self.code {
            out += line;
            out.push('\n');
        }
        if self.hunk.len() > self.code.len() {
            out += "\nThe hunk it's part of:\n";
            for line in self.hunk.iter().take(MAX_HUNK_LINES) {
                out += line;
                out.push('\n');
            }
            if self.hunk.len() > MAX_HUNK_LINES {
                out += &format!("… {} more lines\n", self.hunk.len() - MAX_HUNK_LINES);
            }
        }
        out += &format!("\nTheir question: {}\n", self.question);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> AskRequest {
        AskRequest {
            title: "main..feat".into(),
            location: "src/output.rs:121".into(),
            scopes: vec!["impl Output".into(), "fn floor_char_boundary(s: &str)".into()],
            code: vec!["-fn floor_char_boundary(s: &str) -> usize {".into()],
            hunk: vec![" // helpers".into(), "-fn floor_char_boundary(s: &str) -> usize {".into()],
            question: "does std behave the same?".into(),
            history: Vec::new(),
        }
    }

    #[test]
    fn prompt_carries_the_location_code_and_question() {
        let prompt = request().to_prompt();
        assert!(prompt.starts_with("Diff under review: main..feat\n"));
        assert!(prompt.contains("looking at src/output.rs:121, inside impl Output › fn floor_char_boundary(s: &str).\n"));
        assert!(prompt.contains("point at:\n-fn floor_char_boundary"));
        assert!(prompt.contains("The hunk it's part of:\n // helpers\n"));
        assert!(prompt.ends_with("Their question: does std behave the same?\n"));
    }

    #[test]
    fn prompt_keeps_only_recent_history() {
        let mut request = request();
        request.history = (0..6).map(|i| (format!("q{i}"), format!("a{i}"))).collect();
        let prompt = request.to_prompt();
        assert!(!prompt.contains("q1\n"));
        assert!(prompt.contains("Earlier question: q2\nYour answer: a2\n"));
        assert!(prompt.contains("q5"));
    }
}
