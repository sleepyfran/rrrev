# TODO

## Next up

- [x] `?` help overlay listing every keybinding and `:` command.
- [x] `/` search across the whole diff, with `n` / `N` for next and previous match.
- [x] Stop the `claude` process when quitting during `:organize`.
- [x] Cancel a running `:organize`, and clear a saved plan (`:organize cancel`, `:organize clear`).

## From the native-review-tool demos

- [x] Word-level highlights of what changed inside a changed line.
- [x] Breadcrumbs: the definitions around the cursor, and on each hunk header.
- [x] Read past a hunk: `[e` / `]e` for 20 more lines, `ze` for the whole function.
- [x] Ask the AI about a line, selection or hunk, in a side panel (`a`).
- [x] Agent notes from `:organize`, inline, to turn into comments or dismiss.
- [x] What you went through (good / read on screen / not seen) and a verdict (`S`).
- [x] Copy every comment for an agent (`Y`); copy lines (`y`).
- [x] A real parser (tree-sitter) for the scopes: Rust, Python, JS, TS/TSX, Go, Java, C, C++, Ruby and Lua built in; C#, F#, Swift and Kotlin behind Cargo features (`--features fsharp,swift`, or `all-languages`); other files still go by indentation.
- [ ] Load tree-sitter grammars at runtime (`.so`/`.dylib`, as editors do) instead of compiling them in. Grammars are big (F# alone is 14 MB, all 14 take the binary from 4 MB to 43 MB), which is why the heavy ones sit behind features today; loading them on demand would keep the binary small, give every language without a rebuild, and let people add their own. Needs a place to find them (e.g. `~/.config/rrrev/grammars`), a way to get them (a `rrrev grammars install <lang>` or building from the grammar's repo) and the node-kind tables in `src/diff/scope/syntax.rs` moved to data, probably tree-sitter query files like the `tags.scm` most grammars already ship.
- [ ] A structural diff with tree-sitter; today changes come from a word-level diff.
- [ ] Show the commit messages of the range (the "description").
- [ ] GitHub mode: the PR queue, CI checks, posting the review and merging. Deliberately out of scope while rrrev is local-only.

## Polish

- [x] Syntax highlighting for diff lines (`syntect`).
- [ ] Side-by-side diff view, falling back to unified in narrow panes.
- [ ] Keybindings configurable from a TOML file.
- [ ] Files sidebar as a collapsible directory tree instead of a flat list.

## Known gaps

- [ ] Files with no hunks (binary, empty, mode-only changes) can't be marked as reviewed and don't count toward progress. Needs a file-level id.
- [ ] Comments are anchored by hunk id plus line indices within the hunk. If only the hunk's context lines change, the id stays the same but the indices can shift, so a comment can show under a neighboring line.
- [ ] Comments on a file that has left the diff aren't shown anywhere in the UI (they're still saved and exported).
- [x] `:comments` list of every comment, to jump to or delete from.
- [ ] `]u` searches after the cursor, so when the cursor already sits on an unreviewed stop, the first `]u` moves past it.

## Deferred on purpose

- [ ] Have the app hand comments to an AI agent itself (a new `CommentSink` in `src/export.rs`). Left out so the harness and session stay yours: `Y` copies a brief to paste into one, and `rrrev --export md | claude -p "apply these review comments"` pipes them.
- [ ] More diff sources behind `DiffSource` (`src/diff/mod.rs`): jj, a diff piped in on stdin (needs keys read from `/dev/tty`).
- [ ] Let the organizer split large hunks across groups (it works on whole hunks today).
