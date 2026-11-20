use std::path::PathBuf;

use super::View;
use super::column::FindChar;
use crate::ask::AskRequest;
use crate::export::Format;
use crate::organizer::OrganizeRequest;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Down(usize),
    Up(usize),
    /// `h` / `l`: along the line, on code rows.
    Left(usize),
    Right(usize),
    /// `w` / `W`: to the next word's (or WORD's) start.
    WordForward { n: usize, big: bool },
    /// `b` / `B`: back to a word's start.
    WordBackward { n: usize, big: bool },
    /// `e` / `E`: to a word's end.
    WordEnd { n: usize, big: bool },
    /// `0`: the start of the line.
    LineStart,
    /// `^`: the line's first non-blank character.
    FirstNonBlank,
    /// `$`: the end of the line.
    LineEnd,
    /// `f` / `F` / `t` and a character.
    FindChar { n: usize, find: FindChar },
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
    FocusDiff,
    ToggleSidebar,
    /// `[e` / `]e`: more of the file above / below the hunk, in steps.
    ExpandAbove(usize),
    ExpandBelow(usize),
    /// `ze`: the whole function the hunk is in.
    ExpandScope,
    /// `zE`: drop all the extra context.
    ResetContext,
    /// `-`: show / hide removed lines, leaving the file as it is now.
    ToggleRemovals,
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
    /// `gc` / `:comments`: the list of every comment.
    ListComments,
    /// `S`: the summary of the review, and its verdict.
    ToggleVerdict,
    /// `T`: the theme picker.
    PickTheme,
    Select,
    /// `gd` / `gy` / `gi`: ask the language server where the symbol under
    /// the cursor is defined.
    GoTo(GoTo),
    /// `gr`: every reference to the symbol under the cursor.
    References,
    /// `K`: what the language server says about the symbol under the cursor.
    Hover,
    /// `C-o`: back to where the cursor jumped from.
    JumpBack,
    Cancel,
    Quit,
}

impl Action {
    /// Motions along a line, which mean nothing in the sidebar.
    pub fn is_horizontal(self) -> bool {
        matches!(
            self,
            Action::Left(_)
                | Action::Right(_)
                | Action::WordForward { .. }
                | Action::WordBackward { .. }
                | Action::WordEnd { .. }
                | Action::LineStart
                | Action::FirstNonBlank
                | Action::LineEnd
                | Action::FindChar { .. }
        )
    }
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
    /// `:lsp`: say how the language servers are doing, or restart them.
    Lsp { restart: bool },
    /// Ask the language server for `spot`'s definition, then call
    /// `finish_go_to`.
    GoTo { kind: GoTo, spot: Spot },
    /// Ask for `spot`'s references, then call `finish_references`.
    References { spot: Spot },
    /// Ask about `spot`, then call `finish_hover`.
    Hover { spot: Spot },
}

/// What `gd` and its kin look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoTo {
    Definition,
    TypeDefinition,
    Implementation,
}

impl GoTo {
    pub fn method(self) -> &'static str {
        match self {
            GoTo::Definition => "textDocument/definition",
            GoTo::TypeDefinition => "textDocument/typeDefinition",
            GoTo::Implementation => "textDocument/implementation",
        }
    }

    pub fn noun(self) -> &'static str {
        match self {
            GoTo::Definition => "definition",
            GoTo::TypeDefinition => "type definition",
            GoTo::Implementation => "implementation",
        }
    }
}

/// A place in the code as the user sees it: a file relative to the working
/// folder, a 0-based line, the byte the cursor is on and that line's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spot {
    pub path: String,
    pub line: u32,
    pub column: usize,
    pub text: String,
}
