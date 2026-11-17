use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{App, Row};
use crate::diff::LineKind;

pub fn render(frame: &mut Frame, app: &mut App) {
    let [title, body, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    app.set_viewport_height(body.height as usize);
    render_title(frame, title, app);
    render_body(frame, body, app);
    render_status(frame, status, app);
}

fn render_title(frame: &mut Frame, area: Rect, app: &App) {
    let files = app.diff.files.len();
    let text = format!(" rrrev ─ {} ─ {files} file{}", app.title, if files == 1 { "" } else { "s" });
    frame.render_widget(Paragraph::new(text).reversed(), area);
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
        if idx == app.cursor {
            paragraph = paragraph.style(Style::new().bg(Color::DarkGray));
        }
        frame.render_widget(paragraph, row_area);
    }
}

fn row_line(app: &App, row: Row) -> Line<'static> {
    match row {
        Row::File { file } => {
            let f = &app.diff.files[file];
            let mut spans = vec![
                Span::styled(format!(" {} ", f.status.label()), Style::new().reversed().bold()),
                Span::raw(" "),
                Span::styled(f.display_path(), Style::new().bold().add_modifier(Modifier::UNDERLINED)),
            ];
            if f.is_binary {
                spans.push(Span::raw(" (binary)").dim());
            }
            Line::from(spans)
        }
        Row::Hunk { file, hunk } => {
            let header = &app.diff.files[file].hunks[hunk].header;
            Line::from(Span::styled(format!(" {header}"), Style::new().fg(Color::Cyan)))
        }
        Row::Line { file, hunk, line } => {
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
    }
}

fn render_status(frame: &mut Frame, area: Rect, app: &App) {
    let mut parts = vec![" NORMAL ".to_string()];
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
    let hints = " j/k move  C-d/C-u half page  G bottom  q quit ";
    let left = parts.join("  ");
    let pad = (area.width as usize).saturating_sub(left.chars().count() + hints.len());
    let line = Line::from(vec![
        Span::raw(left).bold(),
        Span::raw(" ".repeat(pad)),
        Span::raw(hints).dim(),
    ]);
    frame.render_widget(Paragraph::new(line).reversed(), area);
}
