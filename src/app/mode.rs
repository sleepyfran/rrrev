use crate::input::TextInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Diff,
    Sidebar,
}

/// How the diff is paged: a file at a time, or a plan group at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Files,
    Plan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Line-wise selection from `anchor` (a row index) to the cursor.
    Visual { anchor: usize },
    Comment { target: CommentTarget, input: TextInput },
    Command { input: TextInput },
    Search { input: TextInput },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentTarget {
    /// Inclusive range of line indices within one hunk.
    New { file: usize, hunk: usize, start: usize, end: usize },
    Edit { id: u64 },
}
