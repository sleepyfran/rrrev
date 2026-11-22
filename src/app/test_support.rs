//! Fixtures shared by the app's tests.

use super::{Action, App};
use crate::diff::{Diff, DiffLine, FileDiff, FileStatus, Hunk, LineKind, Sources};
use crate::input::InputAction;
use crate::review::ReviewState;

/// Every line is an addition whose content includes the path and hunk
/// index, so each hunk gets a distinct id.
pub fn file(path: &str, hunk_sizes: &[usize]) -> FileDiff {
    let hunk = |(h, &n): (usize, &usize)| Hunk {
        id: Default::default(),
        header: "@@ -1,1 +1,1 @@".to_string(),
        old_start: 1,
        old_lines: 1,
        new_start: 1,
        new_lines: 1,
        lines: (0..n)
            .map(|i| DiffLine {
                kind: LineKind::Added,
                old_no: None,
                new_no: Some(i as u32 + 1),
                content: format!("{path} {h} {i}"),
            })
            .collect(),
    };
    FileDiff {
        old_path: Some(path.into()),
        new_path: Some(path.into()),
        status: FileStatus::Modified,
        is_binary: false,
        hunks: hunk_sizes.iter().enumerate().map(hunk).collect(),
    }
}

/// One added line per hunk, at `(line, content)`: hunks with their own
/// place in the file, for following them across edits.
pub fn file_at(path: &str, hunks: &[(u32, &str)]) -> FileDiff {
    let hunk = |&(no, content): &(u32, &str)| Hunk {
        id: Default::default(),
        header: format!("@@ -{no},0 +{no},1 @@"),
        old_start: no,
        old_lines: 0,
        new_start: no,
        new_lines: 1,
        lines: vec![DiffLine { kind: LineKind::Added, old_no: None, new_no: Some(no), content: content.into() }],
    };
    FileDiff {
        old_path: Some(path.into()),
        new_path: Some(path.into()),
        status: FileStatus::Modified,
        is_binary: false,
        hunks: hunks.iter().map(hunk).collect(),
    }
}

pub fn app_with(files: Vec<FileDiff>) -> App {
    App::new(String::new(), Diff::new(files), ReviewState::default())
}

/// a.rs rows: 0 header, 1 hunk, 2-4 lines, 5 hunk, 6-7 lines.
/// b.rs rows: 0 header, 1 hunk, 2-5 lines.
pub fn sample_app() -> App {
    app_with(vec![file("a.rs", &[3, 2]), file("b.rs", &[4])])
}

/// a.rs changes an import (hunk 0) and a function (hunk 1); b.rs only
/// its imports.
pub fn plumbing_app() -> App {
    use std::sync::Arc;
    let lines = |text: &str| Some(Arc::new(text.lines().map(str::to_string).collect::<Vec<_>>()));
    let old = "use a::B;\nuse c::D;\n\nfn f() {}\n";
    let new = "use a::B;\nuse e::F;\n\nfn f() { 1 }\n";
    let at = |line: u32, content: &str| {
        let mut f = file_at("x", &[(line, content)]).hunks.remove(0);
        f.lines.insert(0, DiffLine { kind: LineKind::Removed, old_no: Some(line), new_no: None, content: "-".into() });
        f
    };
    let mut imports = at(2, "use e::F;");
    imports.lines[0].content = "use c::D;".into();
    let mut code = at(4, "fn f() { 1 }");
    code.lines[0].content = "fn f() {}".into();
    let mut a = file("a.rs", &[]);
    a.hunks = vec![imports.clone(), code];
    let mut b = file("b.rs", &[]);
    b.hunks = vec![imports];
    let sources = || Sources { old: lines(old), new: lines(new) };
    let diff = Diff::new(vec![a, b]).with_sources(vec![sources(), sources()]);
    App::new(String::new(), diff, ReviewState::default())
}


/// `(page, cursor row)`.
pub fn position(app: &App) -> (usize, usize) {
    (app.page, app.cursor)
}

pub fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.update_input(InputAction::Insert(c));
    }
}

/// Runs `:text`.
pub fn command(app: &mut App, text: &str) {
    app.update(Action::EnterCommand);
    type_text(app, text);
    app.update_input(InputAction::Submit);
}

/// Comments `text` on whatever the cursor or selection is on.
pub fn comment(app: &mut App, text: &str) {
    app.update(Action::StartComment);
    type_text(app, text);
    app.update_input(InputAction::Submit);
}
