use crate::input::TextInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Diff,
    Sidebar,
}

/// How the diff is paged: a stack layer (or a group of one) at a time, a
/// plan group at a time, or a file at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Stack,
    Plan,
    Files,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Line-wise selection from `anchor` (a row index) to the cursor.
    Visual {
        anchor: usize,
    },
    Comment {
        target: CommentTarget,
        input: TextInput,
    },
    Command {
        input: TextInput,
    },
    Search {
        input: TextInput,
    },
    /// Typing a question about `target` for the ask panel.
    Ask {
        target: AskTarget,
        input: TextInput,
    },
    /// Typing the verdict's message to the author.
    VerdictMessage {
        input: TextInput,
    },
}

/// What a question is about: a line, a selection or a hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskTarget {
    /// `path:12` or `path:12-14`.
    pub location: String,
    /// Definitions around it, outermost first.
    pub scopes: Vec<String>,
    /// The lines pointed at, signed.
    pub code: Vec<String>,
    /// The whole hunk, signed.
    pub hunk: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentTarget {
    /// Inclusive range of line indices within one hunk.
    New {
        file: usize,
        hunk: usize,
        start: usize,
        end: usize,
    },
    Edit {
        id: u64,
    },
}
