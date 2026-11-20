use std::path::PathBuf;

use super::View;
use crate::ask::AskRequest;
use crate::export::Format;
use crate::organizer::OrganizeRequest;
use crate::theme::Theme;

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
    /// `gs` / `gp` / `gf`: the stack's layers, the plan, or the files.
    ShowView(View),
    /// `]s` / `[s`: the next / previous stack layer.
    NextLayer(usize),
    PrevLayer(usize),
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
    /// `a`: ask the AI about the line, selection or hunk.
    Ask,
    /// `A`: show / hide the ask panel.
    ToggleAskPanel,
    /// `C-e` / `C-y`: scroll the ask panel.
    PanelDown(usize),
    PanelUp(usize),
    /// `y`: copy the line, selection or comment to the clipboard.
    Yank,
    /// `Y`: copy every comment, as `path:line` and its text, for an agent.
    CopyForAgent,
    /// `S`: the summary of the review, and its verdict.
    ToggleVerdict,
    /// `T`: the theme picker.
    PickTheme,
    Select,
    Cancel,
    Quit,
}

/// Keys on the verdict page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerdictAction {
    Approve,
    RequestChanges,
    Comment,
    /// Write or edit the message to the author.
    Message,
    /// Take the verdict back.
    Clear,
    CopyForAgent,
    Export,
    Close,
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
    /// Answer a question in the background, then call `finish_ask` with the
    /// same `job`.
    Ask { job: u64, request: AskRequest },
    CancelAsk { job: u64 },
    /// Put `text` on the system clipboard.
    Copy { text: String },
    /// Keep `theme` for the next session.
    SaveTheme { theme: &'static Theme },
}
