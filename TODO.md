# TODO

## Next up

- [x] `?` help overlay listing every keybinding and `:` command.
- [x] `/` search across the whole diff, with `n` / `N` for next and previous match.
- [x] Stop the `claude` process when quitting during `:organize`.
- [x] Cancel a running `:organize`, and clear a saved plan (`:organize cancel`, `:organize clear`).

## Polish

- [x] Syntax highlighting for diff lines (`syntect`).
- [ ] Side-by-side diff view, falling back to unified in narrow panes.
- [ ] Keybindings configurable from a TOML file.
- [ ] Files sidebar as a collapsible directory tree instead of a flat list.

## Known gaps

- [ ] Files with no hunks (binary, empty, mode-only changes) can't be marked as reviewed and don't count toward progress. Needs a file-level id.
- [ ] Comments are anchored by hunk id plus line indices within the hunk. If only the hunk's context lines change, the id stays the same but the indices can shift, so a comment can show under a neighboring line.
- [ ] Comments on a file that has left the diff aren't shown anywhere in the UI (they're still saved and exported).
- [ ] `:comments` list of every comment, to jump to or delete from.
- [ ] `]u` searches after the cursor, so when the cursor already sits on an unreviewed stop, the first `]u` moves past it.

## Deferred on purpose

- [ ] Send comments to an AI agent to apply them: a new `CommentSink` in `src/export.rs`. Until then: `rrrev --export md | claude -p "apply these review comments"`.
- [ ] More diff sources behind `DiffSource` (`src/diff/mod.rs`): jj, a diff piped in on stdin (needs keys read from `/dev/tty`).
- [ ] Let the organizer split large hunks across groups (it works on whole hunks today).
