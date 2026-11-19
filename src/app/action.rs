use std::path::PathBuf;

use crate::export::Format;
use crate::organizer::OrganizeRequest;

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
    /// `R`: everything on the current page, a file or a plan group.
    TogglePageReviewed,
    ToggleView,
    StartVisual,
    StartComment,
    CommentInEditor,
    DeleteComment,
    EnterCommand,
    StartSearch,
    SearchNext(usize),
    SearchPrev(usize),
    ToggleHelp,
    ToggleFocus,
    FocusSidebar,
    FocusDiff,
    ToggleSidebar,
    /// `[e` / `]e`: more of the file above / below the hunk, in steps.
    ExpandAbove(usize),
    ExpandBelow(usize),
    /// `ze`: the whole function the hunk is in.
    ExpandScope,
    /// `zE`: drop all the extra context.
    ResetContext,
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
    /// Plan the review in the background, then call `finish_organize` with
    /// the same `job`.
    Organize { job: u64, request: OrganizeRequest },
    /// Stop that job; its result, if one still arrives, is ignored.
    CancelOrganize { job: u64 },
}
