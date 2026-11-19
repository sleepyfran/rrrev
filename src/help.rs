//! What the `?` overlay lists. Kept apart from the keymap so it reads as a
//! reference; update it alongside `keymap.rs`.

pub struct Section {
    pub title: &'static str,
    /// `(keys, what they do)`.
    pub entries: &'static [(&'static str, &'static str)],
}

pub const SECTIONS: &[Section] = &[
    Section {
        title: "Moving",
        entries: &[
            ("j / k", "line down / up (counts work: 5j)"),
            ("C-d / C-u", "half a page down / up"),
            ("gg / G", "top / bottom of the page"),
            ("]c / [c   } / {", "next / previous hunk"),
            ("]f / [f", "next / previous file, or plan group"),
            ("]u / [u", "next / previous unreviewed hunk"),
            ("]m / [m", "next / previous comment or AI note"),
            ("zt / zz / zb", "scroll the cursor to top / middle / bottom"),
            (":<n>", "go to line n of the file"),
        ],
    },
    Section {
        title: "Reading around a change",
        entries: &[
            ("[e / ]e", "20 more lines of the file above / below the hunk (counts work: 3[e)"),
            ("ze", "the whole function, impl or class the hunk is in"),
            ("zE", "back to the hunks as git shows them"),
            ("breadcrumbs", "the line above the diff shows the file and the definitions around the cursor"),
            ("tinted words", "within a changed line, the words that changed"),
        ],
    },
    Section {
        title: "Searching",
        entries: &[
            ("/text", "search the whole diff (case-insensitive unless it has capitals)"),
            ("n / N", "next / previous match"),
            ("Esc", "clear the highlight"),
        ],
    },
    Section {
        title: "Reviewing",
        entries: &[
            ("r", "toggle the hunk reviewed; on a header, everything under it"),
            ("R", "toggle the whole page: the file, or the plan group"),
        ],
    },
    Section {
        title: "Comments",
        entries: &[
            ("c", "comment on the line or hunk; on a comment, edit it"),
            ("V / v, then c", "select lines, then comment on them"),
            ("C", "write the comment in $EDITOR"),
            ("dd", "delete the comment under the cursor"),
            ("in the box", "Enter save · Alt-Enter newline · C-o $EDITOR · Esc cancel"),
            ("c / dd on a ◆ note", "turn the AI's note into a comment / dismiss it"),
        ],
    },
    Section {
        title: "Asking the AI",
        entries: &[
            ("a", "ask about the line, selection or hunk; Claude can read the whole repository"),
            ("A", "show / hide the answers; Esc hides them too"),
            ("C-e / C-y", "scroll the answers"),
            (":ask cancel / clear", "stop the question being answered / forget them all"),
        ],
    },
    Section {
        title: "Layout",
        entries: &[
            ("za / zo / zc", "toggle / open / close the file's fold"),
            ("zR / zM", "open / close every fold"),
            ("Enter", "on a file header, toggle its fold"),
            ("Tab   h / l", "focus the sidebar / the diff; on a narrow screen, open / close it as a pop-up"),
            ("s", "show / hide the sidebar (the pop-up on a narrow screen)"),
            ("p", "switch between the plan and the files"),
        ],
    },
    Section {
        title: "Commands",
        entries: &[
            (":organize", "plan the review with AI, in the background, with notes on what to check"),
            (":organize cancel", "stop a running plan"),
            (":organize clear", "drop the plan (a stack goes back to its layers)"),
            (":export [md|json] [path]", "write the comments to a file"),
            (":w / :q", "save / quit"),
            ("?", "this help"),
        ],
    },
];
