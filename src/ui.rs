use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::{App, Focus, Row};
use crate::diff::{FileStatus, LineKind};

/// `pending` holds the keys of an unfinished command, e.g. `3]`.
pub fn render(frame: &mut Frame, app: &mut App, pending: &str) {
    let [title, main, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let body = if app.show_sidebar {
        let width = (main.width * 3 / 10).clamp(24, 48).min(main.width / 2);
        let [sidebar, body] =
            Layout::horizontal([Constraint::Length(width), Constraint::Min(0)]).areas(main);
        render_sidebar(frame, sidebar, app);
        body
    } else {
        main
    };

    app.set_viewport_height(body.height as usize);
    render_title(frame, title, app);
    render_body(frame, body, app);
    render_status(frame, status, app, pending);
}

fn render_title(frame: &mut Frame, area: Rect, app: &App) {
    let files = app.diff.files.len();
    let left = format!(" rrrev ─ {} ─ {files} file{}", app.title, if files == 1 { "" } else { "s" });
    let progress = app.progress();
    let done = if progress.is_complete() { " ✓" } else { "" };
    let right = format!("{}/{} hunks reviewed{done} ", progress.reviewed, progress.total);
    let pad = (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
    let right_style = if progress.is_complete() { Style::new().fg(Color::Green) } else { Style::new() };
    let line = Line::from(vec![
        Span::raw(left),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, right_style.bold()),
    ]);
    frame.render_widget(Paragraph::new(line).reversed(), area);
}

/// `✓` for a fully reviewed file, `◐` for a partly reviewed one.
fn review_mark(app: &App, file: usize) -> Span<'static> {
    let progress = app.file_progress(file);
    if progress.is_complete() {
        Span::styled("✓", Style::new().fg(Color::Green))
    } else if progress.reviewed > 0 {
        Span::styled("◐", Style::new().fg(Color::Yellow))
    } else {
        Span::raw(" ")
    }
}

fn status_style(status: FileStatus) -> Style {
    let color = match status {
        FileStatus::Added => Color::Green,
        FileStatus::Deleted => Color::Red,
        FileStatus::Modified => Color::Yellow,
        FileStatus::Renamed | FileStatus::Copied => Color::Blue,
        FileStatus::Other => Color::Magenta,
    };
    Style::new().fg(color).bold()
}

fn render_sidebar(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Sidebar;
    let border_style = if focused { Style::new().fg(Color::Cyan) } else { Style::new().dim() };
    let block = Block::new()
        .borders(Borders::RIGHT)
        .border_style(border_style)
        .title(Span::styled(" Files ", if focused { Style::new().bold() } else { Style::new().dim() }));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let height = inner.height as usize;
    let selected = app.current_file();
    // Keep the selected file visible without storing a separate sidebar scroll.
    let first = selected.map_or(0, |s| (s + 1).saturating_sub(height));

    for (offset, (idx, file)) in app.diff.files.iter().enumerate().skip(first).take(height).enumerate() {
        let (added, removed) = file.stats();
        let stats = format!(" +{added} -{removed}");
        let fold = if app.collapsed[idx] { "▸" } else { " " };
        // Status letter, space, review mark, fold marker, space.
        let path_width = (inner.width as usize).saturating_sub(5 + stats.chars().count());
        let path = truncate_left(file.path(), path_width);
        let (dir, name) = match path.rfind('/') {
            Some(i) => path.split_at(i + 1),
            None => ("", path.as_str()),
        };
        let pad = path_width.saturating_sub(path.chars().count());

        let line = Line::from(vec![
            Span::styled(file.status.label(), status_style(file.status)),
            Span::raw(" "),
            review_mark(app, idx),
            Span::raw(format!("{fold} ")),
            Span::raw(dir.to_string()).dim(),
            Span::raw(name.to_string()),
            Span::raw(" ".repeat(pad)),
            Span::styled(format!(" +{added}"), Style::new().fg(Color::Green)),
            Span::styled(format!(" -{removed}"), Style::new().fg(Color::Red)),
        ]);
        let mut paragraph = Paragraph::new(line);
        if Some(idx) == selected {
            let bg = if focused { Color::DarkGray } else { Color::Reset };
            paragraph = paragraph.style(Style::new().bg(bg).add_modifier(Modifier::BOLD));
        }
        let row_area = Rect { y: inner.y + offset as u16, height: 1, ..inner };
        frame.render_widget(paragraph, row_area);
    }
}

/// Keeps the end of a path, which is the part that tells files apart.
fn truncate_left(path: &str, width: usize) -> String {
    let len = path.chars().count();
    if len <= width {
        return path.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let tail: String = path.chars().skip(len - (width - 1)).collect();
    format!("…{tail}")
}

fn render_body(frame: &mut Frame, area: Rect, app: &App) {
    if app.rows.is_empty() {
        frame.render_widget(Paragraph::new(" No changes to review.").dim(), area);
        return;
    }
    let visible = app.rows.iter().enumerate().skip(app.scroll).take(area.height as usize);
    for (offset, (idx, row)) in visible.enumerate() {
        let row_area = Rect { y: area.y + offset as u16, height: 1, ..area };
        let mut paragraph = Paragraph::new(row_line(app, *row));
        if idx == app.cursor && app.focus == Focus::Diff {
            paragraph = paragraph.style(Style::new().bg(Color::DarkGray));
        }
        frame.render_widget(paragraph, row_area);
    }
}

fn row_line(app: &App, row: Row) -> Line<'static> {
    match row {
        Row::File { file } => {
            let f = &app.diff.files[file];
            let collapsed = app.collapsed[file];
            let mut spans = vec![
                Span::raw(if collapsed { "▸" } else { "▾" }),
                Span::styled(format!(" {} ", f.status.label()), status_style(f.status).reversed()),
                Span::raw(" "),
                Span::styled(f.display_path(), Style::new().bold().add_modifier(Modifier::UNDERLINED)),
                Span::raw(" "),
                review_mark(app, file),
            ];
            if f.is_binary {
                spans.push(Span::raw(" (binary)").dim());
            } else if collapsed {
                let reason = if f.looks_generated() { "generated, " } else { "" };
                spans.push(Span::raw(format!(" ({reason}{} lines folded · za to unfold)", f.line_count())).dim());
            }
            Line::from(spans)
        }
        Row::Hunk { file, hunk } => {
            let header = &app.hunk(file, hunk).header;
            if app.is_reviewed(file, hunk) {
                Line::from(vec![
                    Span::styled("✓", Style::new().fg(Color::Green).bold()),
                    Span::styled(format!(" {header}"), Style::new().fg(Color::Cyan).dim()),
                ])
            } else {
                Line::from(Span::styled(format!("  {header}"), Style::new().fg(Color::Cyan)))
            }
        }
        // Reviewed hunks fade out so what's left to read stands out.
        Row::Line { file, hunk, line } if app.is_reviewed(file, hunk) => {
            diff_line(app, file, hunk, line).patch_style(Style::new().dim())
        }
        Row::Line { file, hunk, line } => diff_line(app, file, hunk, line),
    }
}

fn diff_line(app: &App, file: usize, hunk: usize, line: usize) -> Line<'static> {
    let l = app.line(file, hunk, line);
    let w = app.gutter_width;
    let no = |n: Option<u32>| n.map_or_else(|| " ".repeat(w), |n| format!("{n:>w$}"));
    let gutter = format!(" {} {} ", no(l.old_no), no(l.new_no));
    let (sign, style) = match l.kind {
        LineKind::Added => ("+", Style::new().fg(Color::Green)),
        LineKind::Removed => ("-", Style::new().fg(Color::Red)),
        LineKind::Context => (" ", Style::new()),
        LineKind::NoNewline => (" ", Style::new().dim().italic()),
    };
    Line::from(vec![
        Span::styled(gutter, Style::new().dim()),
        Span::styled(format!("{sign} {}", l.content.replace('\t', "    ")), style),
    ])
}

fn render_status(frame: &mut Frame, area: Rect, app: &App, pending: &str) {
    let mode = match app.focus {
        Focus::Diff => " NORMAL ",
        Focus::Sidebar => " FILES ",
    };
    let mut parts = vec![mode.to_string()];
    if let Some(row) = app.current_row() {
        let file = &app.diff.files[row.file()];
        let mut location = file.display_path();
        if let Row::Line { file, hunk, line } = row {
            let l = app.line(file, hunk, line);
            if let Some(n) = l.new_no.or(l.old_no) {
                location.push_str(&format!(":{n}"));
            }
        }
        parts.push(location);
        parts.push(format!("file {}/{}", row.file() + 1, app.diff.files.len()));
        if let Some(hunk) = row.hunk() {
            parts.push(format!("hunk {}/{}", hunk + 1, file.hunks.len()));
        }
    }
    let mut left = parts.join("  ");
    let pending = format!("{pending:>6}");
    let width = area.width as usize;
    let fits = |left: &str, right: &str| {
        left.chars().count() + pending.len() + right.chars().count() <= width
    };
    // Space goes to pending keys first, then a message, then the location
    // (also visible in the sidebar), and hints last.
    let (right, right_style) = match &app.message {
        Some(msg) => {
            let right = format!(" {msg} ");
            if !fits(&left, &right) {
                left = mode.to_string();
            }
            let style = if msg.starts_with("error:") { Style::new().fg(Color::Red) } else { Style::new() };
            (right, style.bold())
        }
        None => {
            let hints = " r review  ]u unreviewed  ]c hunk  ]f file  za fold  Tab files  q quit ";
            let hints = if fits(&left, hints) { hints } else { " " };
            (hints.to_string(), Style::new().dim())
        }
    };
    let used = left.chars().count() + pending.len() + right.chars().count();
    let line = Line::from(vec![
        Span::raw(left).bold(),
        Span::raw(" ".repeat(width.saturating_sub(used))),
        Span::raw(pending).dim(),
        Span::styled(right, right_style),
    ]);
    frame.render_widget(Paragraph::new(line).reversed(), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_paths_from_the_left() {
        assert_eq!(truncate_left("src/main.rs", 20), "src/main.rs");
        assert_eq!(truncate_left("src/diff/git.rs", 8), "…/git.rs");
        assert_eq!(truncate_left("abc", 0), "");
    }
}
