use std::path::PathBuf;

use crate::export::Format;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Down(usize),
    Up(usize),
    HalfPageDown,
    HalfPageUp,
    Top,
    Bottom,
    NextHunk(usize),
    PrevHunk(usize),
    NextFile(usize),
    PrevFile(usize),
    NextUnreviewed(usize),
    PrevUnreviewed(usize),
    NextComment(usize),
    PrevComment(usize),
    ScrollCursorTop,
    ScrollCursorCenter,
    ScrollCursorBottom,
    ToggleFold,
    OpenFold,
    CloseFold,
    OpenAllFolds,
    CloseAllFolds,
    ToggleReviewed,
    ToggleFileReviewed,
    StartVisual,
    StartComment,
    CommentInEditor,
    DeleteComment,
    EnterCommand,
    ToggleFocus,
    FocusSidebar,
    FocusDiff,
    ToggleSidebar,
    Select,
    Cancel,
    Quit,
}

/// Work `update` can't do itself, for the main loop to carry out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Edit the comment being written in `$EDITOR`, then call `finish_editor`.
    OpenEditor { initial: String },
    Export { format: Format, path: Option<PathBuf> },
}
