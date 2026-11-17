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
            ("]m / [m", "next / previous comment"),
            ("zt / zz / zb", "scroll the cursor to top / middle / bottom"),
            (":<n>", "go to line n of the file"),
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
        ],
    },
    Section {
        title: "Layout",
        entries: &[
            ("za / zo / zc", "toggle / open / close the file's fold"),
            ("zR / zM", "open / close every fold"),
            ("Enter", "on a file header, toggle its fold"),
            ("Tab   h / l", "focus the sidebar / the diff"),
            ("s", "show / hide the sidebar"),
            ("p", "switch between the plan and the files"),
        ],
    },
    Section {
        title: "Commands",
        entries: &[
            (":organize", "plan the review with AI, in the background"),
            (":organize cancel", "stop a running plan"),
            (":organize clear", "drop the plan"),
            (":export [md|json] [path]", "write the comments to a file"),
            (":w / :q", "save / quit"),
            ("?", "this help"),
        ],
    },
];
