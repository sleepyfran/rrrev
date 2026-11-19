use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};

use std::ops::Range;

use crate::anim::{Animator, Popup};
use crate::app::{
    App, CommentTarget, Focus, Mode, Progress, Row, View, context_numbers, context_text,
    find_matches, is_above,
};
use crate::diff::{FileStatus, LineKind, scope};
use crate::help;
use crate::highlight::{Highlighter, Segment};
use crate::text::wrap_words;

/// Below this many columns the sidebar won't dock, as it would leave too
/// little room for code; it opens as a pop-up instead.
const COMPACT_BELOW: u16 = 100;

/// `pending` holds the keys of an unfinished command, e.g. `3]`.
pub fn render(frame: &mut Frame, app: &mut App, highlighter: &mut Highlighter, anim: &mut Animator, pending: &str) {
    anim.begin(app);
    let [title, main, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    app.set_compact(frame.area().width < COMPACT_BELOW);
    let body = if app.show_sidebar && !app.compact {
        let width = (main.width * 3 / 10).clamp(24, 48).min(main.width / 2);
        let [sidebar, body] =
            Layout::horizontal([Constraint::Length(width), Constraint::Min(0)]).areas(main);
        let (_, entries) = render_sidebar(frame, sidebar, app, false);
        anim.glint_sidebar(frame.buffer_mut(), app, &entries);
        body
    } else {
        main
    };

    // The comment box covers the bottom of the diff, so rows scroll above it
    // and the commented line stays in sight.
    let comment_box = comment_box_area(app, body);
    let diff_area = Rect { height: body.height - comment_box.map_or(0, |b| b.height), ..body };
    // Where the cursor is in its file, above the rows.
    let [crumbs, rows_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(diff_area);
    app.set_viewport(rows_area.width as usize, rows_area.height as usize);
    render_title(frame, title, app);
    anim.glint_title(frame.buffer_mut(), title);
    render_breadcrumbs(frame, crumbs, app);
    let scroll = anim.scroll(app);
    let rows = render_body(frame, rows_area, app, highlighter, scroll);
    anim.glint_rows(frame.buffer_mut(), &rows);
    anim.slide(frame.buffer_mut(), rows_area);
    let cursor = comment_box.map(|area| render_comment_box(frame, area, app));
    if let Some(unroll) = anim.popup(frame.buffer_mut(), Popup::Comment, comment_box, None)
        && let Some((x, y)) = cursor
    {
        // Keep the cursor on its row while the box rises, or on the box's top.
        let y = unroll.row(y).or(unroll.row(comment_box.map_or(y, |b| b.y))).unwrap_or(y);
        frame.set_cursor_position((x, y));
    }
    render_status(frame, status, app, pending);

    let sidebar_popup = app.compact && app.focus == Focus::Sidebar;
    if sidebar_popup || app.help_scroll.is_some() {
        dim_backdrop(frame);
    }
    // What the pop-ups cover, to show around them as they unroll.
    let under = frame.buffer_mut().clone();
    let sidebar = sidebar_popup.then(|| {
        let (rect, entries) = render_sidebar(frame, main, app, true);
        anim.glint_sidebar(frame.buffer_mut(), app, &entries);
        rect
    });
    anim.popup(frame.buffer_mut(), Popup::Sidebar, sidebar, Some(&under));
    let help = app.help_scroll.is_some().then(|| render_help(frame, frame.area(), app));
    anim.popup(frame.buffer_mut(), Popup::Help, help, Some(&under));
}

/// Greys out everything drawn so far, so a pop-up drawn on top stands out.
/// Done with plain colours rather than the dim attribute, which many
/// terminals barely show; backgrounds go too, so no highlight can leave grey
/// text on a grey row.
fn dim_backdrop(frame: &mut Frame) {
    let area = frame.area();
    let style = Style::new().fg(Color::DarkGray).bg(Color::Reset).remove_modifier(Modifier::BOLD | Modifier::REVERSED);
    frame.buffer_mut().set_style(area, style);
}

/// The `?` overlay: every key and command, grouped, centred over the screen.
/// Returns where it was drawn.
fn render_help(frame: &mut Frame, area: Rect, app: &mut App) -> Rect {
    let width = 100.min(area.width.saturating_sub(4));
    let key_width = help::SECTIONS.iter().flat_map(|s| s.entries).map(|(k, _)| k.chars().count()).max().unwrap_or(0) + 2;
    // Inside the border, after the key column.
    let text_width = (width as usize).saturating_sub(2 + 1 + key_width).max(10);

    let mut lines: Vec<Line> = Vec::new();
    for section in help::SECTIONS {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::from(Span::styled(format!(" {}", section.title), Style::new().fg(Color::Magenta).bold())));
        for (keys, what) in section.entries {
            for (i, text) in wrap_words(what, text_width).into_iter().enumerate() {
                let keys = if i == 0 { *keys } else { "" };
                lines.push(Line::from(vec![
                    Span::styled(format!(" {keys:<key_width$}"), Style::new().fg(Color::Cyan)),
                    Span::raw(text),
                ]));
            }
        }
    }

    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let rect = centered(area, width, height);
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(Style::new().fg(Color::Magenta).bold())
        .title(Span::styled(" Keys ", Style::new().bold()))
        .title_bottom(Line::from(" j/k scroll · Esc close ").dim());
    let inner = block.inner(rect);
    // Only now is it known how far the help can scroll.
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    let scroll = app.help_scroll.map_or(0, |s| s.min(max_scroll));
    app.help_scroll = Some(scroll);

    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), inner);
    rect
}

/// `segments` as spans, with matches of the highlighted search picked out.
/// A match can span several segments.
fn highlighted(segments: Vec<Segment>, app: &App) -> Vec<Span<'static>> {
    let Some(query) = app.highlighted_search() else {
        return segments.into_iter().map(|(style, text)| Span::styled(text, style)).collect();
    };
    let text: String = segments.iter().map(|(_, t)| t.as_str()).collect();
    let matches = find_matches(&text, query);
    let mut spans = Vec::new();
    let mut start = 0;
    for (style, segment) in segments {
        let end = start + segment.len();
        let mut last = start;
        for range in matches.iter().filter(|m| m.start < end && m.end > start) {
            let (from, to) = (range.start.max(start), range.end.min(end));
            if from > last {
                spans.push(Span::styled(text[last..from].to_string(), style));
            }
            spans.push(Span::styled(text[from..to].to_string(), style.bg(Color::Yellow).fg(Color::Black)));
            last = to;
        }
        if last < end {
            spans.push(Span::styled(text[last..end].to_string(), style));
        }
        start = end;
    }
    spans
}

fn render_title(frame: &mut Frame, area: Rect, app: &App) {
    let files = app.diff.files.len();
    let left = format!(" rrrev ─ {} ─ {files} file{}", app.title, if files == 1 { "" } else { "s" });
    let progress = app.progress();
    let done = if progress.is_complete() { " ✓" } else { "" };
    let comments = match app.review.comments().len() {
        0 => String::new(),
        1 => "1 comment · ".to_string(),
        n => format!("{n} comments · "),
    };
    let organizing = match app.organizing {
        Some(job) => format!("{} organizing… {}s · ", spinner(job.started), job.started.elapsed().as_secs()),
        None => String::new(),
    };
    let right = format!(
        "{organizing}{comments}{}/{} hunks reviewed{done} ",
        progress.reviewed, progress.total
    );
    // In a narrow pane the status on the right matters more than the title.
    let left = if left.chars().count() + right.chars().count() > area.width as usize {
        " rrrev ".to_string()
    } else {
        left
    };
    let pad = (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
    let right_style = if progress.is_complete() { Style::new().fg(Color::Green) } else { Style::new() };
    let line = Line::from(vec![
        Span::raw(left),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, right_style.bold()),
    ]);
    frame.render_widget(Paragraph::new(line).reversed(), area);
}

/// A braille spinner frame for something running since `started`.
fn spinner(started: std::time::Instant) -> char {
    const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    SPINNER[(started.elapsed().as_millis() / 100) as usize % SPINNER.len()]
}

/// `✓` when fully reviewed, `◐` when partly.
fn progress_mark(progress: Progress) -> Span<'static> {
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

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let (width, height) = (width.min(area.width), height.min(area.height));
    Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height }
}

/// The file list or the plan, docked at the left of `area` or as a pop-up
/// centred over it. Returns where it was drawn, and where each entry shown
/// (by file or group index) went.
fn render_sidebar(frame: &mut Frame, area: Rect, app: &App, popup: bool) -> (Rect, Vec<(usize, Rect)>) {
    match app.view {
        View::Files => render_files_sidebar(frame, area, app, popup),
        View::Plan => render_plan_sidebar(frame, area, app, popup),
    }
}

/// Draws the sidebar's frame, sized for `entries` when it's a pop-up, and
/// returns the frame's area and the area inside it.
fn sidebar_frame(frame: &mut Frame, area: Rect, app: &App, title: &str, entries: usize, popup: bool) -> (Rect, Rect) {
    let focused = app.focus == Focus::Sidebar;
    let border_style = if focused { Style::new().fg(Color::Cyan) } else { Style::new().dim() };
    let title = Span::styled(format!(" {title} "), if focused { Style::new().bold() } else { Style::new().dim() });
    let (area, block) = if popup {
        let height = (entries as u16 + 2).clamp(3, area.height.saturating_sub(2));
        let rect = centered(area, 72.min(area.width.saturating_sub(4)), height);
        frame.render_widget(Clear, rect);
        let hint = Line::from(" j/k move · Enter / Esc close ").dim();
        let border = Style::new().fg(Color::Cyan).bold();
        (rect, Block::bordered().border_type(BorderType::Thick).border_style(border).title(title).title_bottom(hint))
    } else {
        (area, Block::new().borders(Borders::RIGHT).border_style(border_style).title(title))
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);
    (area, inner)
}

/// Renders one sidebar entry, highlighted when selected, and returns where.
fn sidebar_row(frame: &mut Frame, inner: Rect, offset: usize, line: Line, selected: bool, focused: bool) -> Rect {
    let mut paragraph = Paragraph::new(line);
    if selected {
        let bg = if focused { Color::DarkGray } else { Color::Reset };
        paragraph = paragraph.style(Style::new().bg(bg).add_modifier(Modifier::BOLD));
    }
    let row_area = Rect { y: inner.y + offset as u16, height: 1, ..inner };
    frame.render_widget(paragraph, row_area);
    row_area
}

/// The first entry to show so `selected` is visible, without storing a
/// separate sidebar scroll.
fn sidebar_start(selected: Option<usize>, height: usize) -> usize {
    selected.map_or(0, |s| (s + 1).saturating_sub(height))
}

fn render_plan_sidebar(frame: &mut Frame, area: Rect, app: &App, popup: bool) -> (Rect, Vec<(usize, Rect)>) {
    let (outer, inner) = sidebar_frame(frame, area, app, "Plan", app.plan.len(), popup);
    let focused = app.focus == Focus::Sidebar;
    let height = inner.height as usize;
    let first = sidebar_start(Some(app.page), height);
    let mut entries = Vec::new();
    for (offset, (idx, group)) in app.plan.iter().enumerate().skip(first).take(height).enumerate() {
        let progress = app.group_progress(idx);
        let count = format!(" {}/{}", progress.reviewed, progress.total);
        // Number, space, review mark, space.
        let number = format!("{:>2}", idx + 1);
        let title_width = (inner.width as usize).saturating_sub(number.len() + 3 + count.len());
        let title = truncate_right(&group.title, title_width);
        let pad = title_width.saturating_sub(title.chars().count());
        let line = Line::from(vec![
            Span::raw(number).dim(),
            Span::raw(" "),
            progress_mark(progress),
            Span::raw(" "),
            Span::raw(title),
            Span::raw(" ".repeat(pad)),
            Span::raw(count).dim(),
        ]);
        entries.push((idx, sidebar_row(frame, inner, offset, line, idx == app.page, focused)));
    }
    (outer, entries)
}

fn truncate_right(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let head: String = text.chars().take(width.saturating_sub(1)).collect();
    if width == 0 { head } else { format!("{head}…") }
}

fn render_files_sidebar(frame: &mut Frame, area: Rect, app: &App, popup: bool) -> (Rect, Vec<(usize, Rect)>) {
    let (outer, inner) = sidebar_frame(frame, area, app, "Files", app.diff.files.len(), popup);
    let focused = app.focus == Focus::Sidebar;
    let height = inner.height as usize;
    let selected = app.current_file();
    let first = sidebar_start(selected, height);

    let mut entries = Vec::new();
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
            progress_mark(app.file_progress(idx)),
            Span::raw(format!("{fold} ")),
            Span::raw(dir.to_string()).dim(),
            Span::raw(name.to_string()),
            Span::raw(" ".repeat(pad)),
            Span::styled(format!(" +{added}"), Style::new().fg(Color::Green)),
            Span::styled(format!(" -{removed}"), Style::new().fg(Color::Red)),
        ]);
        entries.push((idx, sidebar_row(frame, inner, offset, line, Some(idx) == selected, focused)));
    }
    (outer, entries)
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

/// Draws the rows from `scroll` on, and returns where each one went.
fn render_body(frame: &mut Frame, area: Rect, app: &App, highlighter: &mut Highlighter, scroll: usize) -> Vec<(Row, Rect)> {
    let mut drawn = Vec::new();
    if app.rows.is_empty() {
        frame.render_widget(Paragraph::new(" No changes to review.").dim(), area);
        return drawn;
    }
    let selection = app.selection();
    let mut y = area.y;
    for (idx, row) in app.rows.iter().enumerate().skip(scroll) {
        if y >= area.bottom() {
            break;
        }
        let lines = row_lines(app, *row, highlighter);
        let row_area = Rect { y, height: (lines.len() as u16).min(area.bottom() - y), ..area };
        y += row_area.height;
        let mut paragraph = Paragraph::new(lines);
        let selected = match (*row, selection) {
            (Row::Line { hunk, line, .. }, Some((_, h, start, end))) => hunk == h && (start..=end).contains(&line),
            _ => false,
        };
        if selected {
            paragraph = paragraph.style(Style::new().bg(Color::Blue));
        } else if idx == app.cursor && app.focus == Focus::Diff {
            paragraph = paragraph.style(Style::new().bg(Color::DarkGray));
        }
        frame.render_widget(paragraph, row_area);
        drawn.push((*row, row_area));
    }
    drawn
}

/// Whether a comment on a live hunk covers this line.
fn is_commented(app: &App, file: usize, hunk: usize, line: usize) -> bool {
    let id = &app.hunk(file, hunk).id;
    app.review.comments().iter().any(|c| c.hunk == *id && (c.start..=c.end).contains(&line))
}

/// A row as drawn: one line, or several for a diff line or comment that
/// wraps (as many as `App::row_height` counts).
fn row_lines(app: &App, row: Row, highlighter: &mut Highlighter) -> Vec<Line<'static>> {
    match row {
        // Reviewed hunks fade out so what's left to read stands out.
        Row::Line { file, hunk, line } if app.is_reviewed(file, hunk) => diff_line(app, highlighter, file, hunk, line)
            .into_iter()
            .map(|l| l.patch_style(Style::new().dim()))
            .collect(),
        Row::Line { file, hunk, line } => diff_line(app, highlighter, file, hunk, line),
        Row::Context { file, hunk, no } => context_line(app, highlighter, file, hunk, no),
        Row::Comment { id, line, .. } => comment_line(app, id, line),
        row => vec![row_line(app, row)],
    }
}

/// A row that always fits on one line, cut off at the pane's edge.
fn row_line(app: &App, row: Row) -> Line<'static> {
    match row {
        Row::Group { group, line } => {
            let header = app.group_header(group);
            let Some((is_title, text)) = header.get(line).cloned() else { return Line::default() };
            let style = if is_title { Style::new().fg(Color::Magenta).bold() } else { Style::new().italic() };
            Line::from(vec![Span::styled("▌ ", Style::new().fg(Color::Magenta)), Span::styled(text, style)])
        }
        Row::File { file } => {
            let f = &app.diff.files[file];
            let collapsed = app.collapsed[file];
            let mut spans = vec![
                Span::raw(if collapsed { "▸" } else { "▾" }),
                Span::styled(format!(" {} ", f.status.label()), status_style(f.status).reversed()),
                Span::raw(" "),
                Span::styled(f.display_path(), Style::new().bold().add_modifier(Modifier::UNDERLINED)),
                Span::raw(" "),
                progress_mark(app.file_progress(file)),
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
            // The line ranges, then what the hunk is in: the definition from
            // the file's text, or git's own guess after the `@@`.
            let ranges = match header.match_indices("@@").nth(1) {
                Some((i, _)) => &header[..i + 2],
                None => header.as_str(),
            };
            let reviewed = app.is_reviewed(file, hunk);
            let mut spans = vec![
                if reviewed { Span::styled("✓ ", Style::new().fg(Color::Green).bold()) } else { Span::raw("  ") },
                Span::styled(ranges.to_string(), Style::new().fg(Color::Cyan)),
            ];
            if let Some(scope) = app.diff.hunk_scope(file, hunk) {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(scope, Style::new().bold()));
            }
            let (above, below) = app.hidden_around(file, hunk);
            let mut hidden = Vec::new();
            if above > 0 {
                hidden.push(format!("[e {above} above"));
            }
            if below > 0 {
                hidden.push(format!("]e {below} below"));
            }
            if !hidden.is_empty() && app.diff.context_source(file).is_some() {
                spans.push(Span::raw(format!("  ⋯ {} · ze function", hidden.join(" · "))).dim());
            }
            let line = Line::from(spans);
            if reviewed { line.patch_style(Style::new().dim()) } else { line }
        }
        Row::Line { .. } | Row::Context { .. } | Row::Comment { .. } => unreachable!("drawn by row_lines"),
    }
}

/// Puts `gutter` before the first line of `text`, wrapped to the room left
/// after it, and `hang` (as wide) before the rest.
fn hanging(app: &App, gutter: Vec<Span<'static>>, hang: Vec<Span<'static>>, text: Vec<Span<'static>>) -> Vec<Line<'static>> {
    let column = gutter.iter().map(|s| s.content.chars().count()).sum();
    wrap_spans(text, app.text_room(column))
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| {
            let mut spans = if i == 0 { gutter.clone() } else { hang.clone() };
            spans.extend(chunk);
            Line::from(spans)
        })
        .collect()
}

/// Hard-wraps styled text every `room` chars, splitting spans where they
/// cross a break. A `room` of 0 means no wrapping.
fn wrap_spans(spans: Vec<Span<'static>>, room: usize) -> Vec<Vec<Span<'static>>> {
    let mut lines = vec![Vec::new()];
    if room == 0 {
        lines[0] = spans;
        return lines;
    }
    let mut used = 0;
    for span in spans {
        let mut rest: &str = &span.content;
        while !rest.is_empty() {
            if used == room {
                lines.push(Vec::new());
                used = 0;
            }
            let take = rest.char_indices().nth(room - used).map_or(rest.len(), |(i, _)| i);
            let (head, tail) = rest.split_at(take);
            used += head.chars().count();
            lines.last_mut().expect("lines is never empty").push(Span::styled(head.to_string(), span.style));
            rest = tail;
        }
    }
    lines
}

fn comment_line(app: &App, id: u64, line: usize) -> Vec<Line<'static>> {
    let Some(comment) = app.review.comment(id) else { return vec![Line::default()] };
    let text = comment.text.lines().nth(line).unwrap_or("").to_string();
    // The `┃` lines up with the change bar, so the text starts past the code's column.
    let indent = Span::raw(" ".repeat(app.code_column() - 2));
    let lead = if line == 0 { "┃ ✎ " } else { "┃   " };
    let gutter = vec![indent.clone(), Span::styled(lead, Style::new().fg(Color::Yellow))];
    let hang = vec![indent, Span::styled("┃   ", Style::new().fg(Color::Yellow))];
    let mut spans = Vec::new();
    if line == 0 {
        spans.push(Span::raw(app.outdated_label(comment)).italic());
    }
    spans.extend(highlighted(vec![(Style::new().fg(Color::Yellow), text)], app));
    let lines = hanging(app, gutter, hang, spans);
    if app.is_outdated(comment) {
        lines.into_iter().map(|l| l.patch_style(Style::new().dim())).collect()
    } else {
        lines
    }
}

fn diff_line(app: &App, highlighter: &mut Highlighter, file: usize, hunk: usize, line: usize) -> Vec<Line<'static>> {
    let l = app.line(file, hunk, line);
    let w = app.gutter_width;
    let no = |n: Option<u32>| n.map_or_else(|| " ".repeat(w), |n| format!("{n:>w$}"));
    let marker = if is_commented(app, file, hunk, line) {
        Span::styled("┃", Style::new().fg(Color::Yellow))
    } else {
        Span::raw(" ")
    };
    // The change shows only in the gutter: a coloured bar, and the changed
    // side's line number in the same colour. The code keeps its syntax colours.
    let change = match l.kind {
        LineKind::Added => Some(Color::Green),
        LineKind::Removed => Some(Color::Red),
        LineKind::Context | LineKind::NoNewline => None,
    };
    let number_style = |side_changed: bool| match change {
        Some(color) if side_changed => Style::new().fg(color),
        _ => Style::new().dim(),
    };
    let bar = match change {
        Some(color) => Span::styled("▌", Style::new().fg(color)),
        None => Span::raw(" "),
    };
    let text_style = match l.kind {
        LineKind::NoNewline => Style::new().dim().italic(),
        _ => Style::new(),
    };
    let gutter = vec![
        marker.clone(),
        Span::styled(no(l.old_no), number_style(l.kind == LineKind::Removed)),
        Span::raw(" "),
        Span::styled(no(l.new_no), number_style(l.kind == LineKind::Added)),
        Span::raw(" "),
        bar.clone(),
        Span::raw(" "),
    ];
    // Wrapped code continues under itself, with the marker and bar running down.
    let hang = vec![marker, Span::raw(" ".repeat(2 * w + 2)), bar, Span::raw(" ")];
    let segments = match highlighter.line(&app.diff, file, hunk, line) {
        [] => vec![(text_style, l.content.clone())],
        segments => segments.to_vec(),
    };
    // What changed within the line, word by word, gets a tint behind it.
    let changes = app.diff.word_changes(file, hunk, line);
    let segments = emphasize(segments, changes, highlighter.emphasis(l.kind));
    let segments = segments.into_iter().map(|(style, text)| (style, text.replace('\t', "    "))).collect();
    hanging(app, gutter, hang, highlighted(segments, app))
}

/// Splits `segments` where `ranges` (byte offsets into their joined text)
/// start and end, and patches `style` onto the parts inside them.
fn emphasize(segments: Vec<Segment>, ranges: &[Range<usize>], style: Style) -> Vec<Segment> {
    if ranges.is_empty() {
        return segments;
    }
    let mut out = Vec::new();
    let mut start = 0;
    for (seg_style, text) in segments {
        let end = start + text.len();
        let mut cuts: Vec<usize> = ranges
            .iter()
            .flat_map(|r| [r.start, r.end])
            .filter(|&c| c > start && c < end && text.is_char_boundary(c - start))
            .collect();
        cuts.sort_unstable();
        cuts.dedup();
        let mut from = start;
        for to in cuts.into_iter().chain([end]) {
            let inside = ranges.iter().any(|r| r.start <= from && to <= r.end);
            let piece = text[from - start..to - start].to_string();
            out.push((if inside { seg_style.patch(style) } else { seg_style }, piece));
            from = to;
        }
        start = end;
    }
    out
}

/// A line of the file shown as extra context next to a hunk: numbered on
/// both sides, with a dotted bar to tell it from the diff's own lines.
fn context_line(app: &App, highlighter: &mut Highlighter, file: usize, hunk: usize, no: u32) -> Vec<Line<'static>> {
    let Some((_, side)) = app.diff.context_source(file) else { return vec![Line::default()] };
    let h = app.hunk(file, hunk);
    let (old_no, new_no) = context_numbers(h, side, no, is_above(h, side, no));
    let w = app.gutter_width;
    let number = |n: Option<u32>| n.map_or_else(|| " ".repeat(w), |n| format!("{n:>w$}"));
    let bar = Span::styled("┊", Style::new().fg(Color::DarkGray));
    let gutter = vec![
        Span::raw(" "),
        Span::raw(number(old_no)).dim(),
        Span::raw(" "),
        Span::raw(number(new_no)).dim(),
        Span::raw(" "),
        bar.clone(),
        Span::raw(" "),
    ];
    let hang = vec![Span::raw(" ".repeat(2 * w + 3)), bar, Span::raw(" ")];
    let text = context_text(&app.diff, file, no).unwrap_or("");
    let segments = match highlighter.source_line(&app.diff, file, no) {
        [] => vec![(Style::new(), text.to_string())],
        segments => segments.to_vec(),
    };
    let segments = segments.into_iter().map(|(style, text)| (style, text.replace('\t', "    "))).collect();
    hanging(app, gutter, hang, highlighted(segments, app))
}

/// The path of the file under the cursor and the definitions around it,
/// outermost first: `src/output.rs › impl Output › fn truncate(…)`.
fn render_breadcrumbs(frame: &mut Frame, area: Rect, app: &App) {
    let Some(row) = app.current_row() else { return };
    let Some(file) = row.file() else {
        let title = app.plan.get(app.page).map_or(String::new(), |p| p.title.clone());
        frame.render_widget(Paragraph::new(Line::from(Span::raw(format!(" {title}")).dim())), area);
        return;
    };
    let scopes: Vec<String> = match row {
        Row::Line { file, hunk, line } => app.diff.scopes(file, hunk, line).into_iter().map(|s| s.label).collect(),
        Row::Hunk { file, hunk } => {
            let line = app.hunk(file, hunk).lines.iter().position(|l| l.kind != LineKind::Context).unwrap_or(0);
            if app.hunk(file, hunk).lines.is_empty() {
                Vec::new()
            } else {
                app.diff.scopes(file, hunk, line).into_iter().map(|s| s.label).collect()
            }
        }
        Row::Context { file, no, .. } => app.diff.context_source(file).map_or_else(Vec::new, |(source, _)| {
            scope::enclosing(source, no).into_iter().map(|s| s.label).collect()
        }),
        Row::Group { .. } | Row::File { .. } | Row::Comment { .. } => Vec::new(),
    };
    let path = app.diff.files[file].path().to_string();
    let width = area.width as usize;
    // Outer scopes give way first when it doesn't fit, then the path's head.
    let mut scopes = scopes;
    let len = |scopes: &[String]| path.chars().count() + 2 + scopes.iter().map(|s| s.chars().count() + 3).sum::<usize>();
    while scopes.len() > 1 && len(&scopes) > width {
        if scopes[0] == "…" {
            scopes.remove(1);
        } else {
            scopes[0] = "…".to_string();
        }
    }
    let separator = || Span::styled(" › ", Style::new().fg(Color::DarkGray));
    let room = width.saturating_sub(len(&scopes) - path.chars().count());
    let mut spans = vec![Span::raw(" "), Span::raw(truncate_left(&path, room.max(8))).dim()];
    for (i, scope) in scopes.iter().enumerate() {
        spans.push(separator());
        let innermost = i + 1 == scopes.len();
        spans.push(Span::styled(scope.clone(), if innermost { Style::new().bold() } else { Style::new() }));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

const MAX_COMMENT_BOX_LINES: u16 = 8;

fn comment_box_area(app: &App, body: Rect) -> Option<Rect> {
    let Mode::Comment { input, .. } = &app.mode else { return None };
    let (lines, _) = wrap_input(input.text(), input.cursor(), body.width.saturating_sub(2) as usize);
    let height = (lines.len() as u16).clamp(1, MAX_COMMENT_BOX_LINES) + 2;
    let height = height.min(body.height.saturating_sub(1));
    Some(Rect { y: body.bottom() - height, height, ..body })
}

/// Returns where the text cursor goes.
fn render_comment_box(frame: &mut Frame, area: Rect, app: &App) -> (u16, u16) {
    let Mode::Comment { target, input } = &app.mode else { return (area.x, area.y) };
    let title = match *target {
        CommentTarget::New { file, hunk, start, end } => {
            let h = app.hunk(file, hunk);
            let numbers: Vec<u32> = h.lines[start..=end].iter().filter_map(|l| l.new_no.or(l.old_no)).collect();
            let range = match (numbers.iter().min(), numbers.iter().max()) {
                (Some(a), Some(b)) if a != b => format!(":{a}-{b}"),
                (Some(a), _) => format!(":{a}"),
                _ => String::new(),
            };
            format!(" Comment on {}{range} ", app.diff.files[file].path())
        }
        CommentTarget::Edit { .. } => " Edit comment (empty deletes it) ".to_string(),
    };
    let block = Block::bordered()
        .border_style(Style::new().fg(Color::Yellow))
        .title(Span::styled(title, Style::new().bold()))
        .title_bottom(Line::from(" Enter save · Alt-Enter newline · C-o $EDITOR · Esc cancel ").dim());
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let (lines, (x, y)) = wrap_input(input.text(), input.cursor(), inner.width as usize);
    // Scroll so the cursor's line is visible once the text outgrows the box.
    let skip = (y + 1).saturating_sub(inner.height as usize);
    let visible: Vec<Line> = lines.into_iter().skip(skip).map(Line::from).collect();
    frame.render_widget(Paragraph::new(visible), inner);
    let cursor = (inner.x + x as u16, inner.y + (y - skip) as u16);
    frame.set_cursor_position(cursor);
    cursor
}

/// Breaks input text into rows of at most `width` chars, hard-wrapping long
/// lines, and finds the cursor's `(column, row)` among them.
fn wrap_input(text: &str, cursor: usize, width: usize) -> (Vec<String>, (usize, usize)) {
    let width = width.max(1);
    let mut rows = vec![String::new()];
    let mut position = None;
    let full = |rows: &Vec<String>| rows.last().is_some_and(|r| r.chars().count() == width);
    for (i, ch) in text.chars().enumerate() {
        if ch != '\n' && full(&rows) {
            rows.push(String::new());
        }
        if i == cursor {
            position = Some((rows.last().map_or(0, |r| r.chars().count()), rows.len() - 1));
        }
        match ch {
            '\n' => rows.push(String::new()),
            ch => rows.last_mut().expect("rows is never empty").push(ch),
        }
    }
    let position = position.unwrap_or_else(|| {
        if full(&rows) {
            rows.push(String::new());
        }
        (rows.last().map_or(0, |r| r.chars().count()), rows.len() - 1)
    });
    (rows, position)
}

fn render_status(frame: &mut Frame, area: Rect, app: &App, pending: &str) {
    let prompt = match &app.mode {
        Mode::Command { input } => Some((':', input)),
        Mode::Search { input } => Some(('/', input)),
        _ => None,
    };
    if let Some((prefix, input)) = prompt {
        let text = format!("{prefix}{}", input.text());
        frame.render_widget(Paragraph::new(text), area);
        let x = 1 + input.cursor() as u16;
        frame.set_cursor_position((area.x + x.min(area.width.saturating_sub(1)), area.y));
        return;
    }
    let mode = match (&app.mode, app.focus) {
        (Mode::Visual { .. }, _) => " VISUAL ",
        (Mode::Comment { .. }, _) => " COMMENT ",
        (_, Focus::Sidebar) => " FILES ",
        _ => " NORMAL ",
    };
    let mut parts = vec![mode.to_string()];
    if let Some(row) = app.current_row() {
        let mut location = match row.file() {
            Some(file) => app.diff.files[file].display_path(),
            None => app.plan.get(app.page).map_or(String::new(), |p| p.title.clone()),
        };
        if let Row::Line { file, hunk, line } = row {
            let l = app.line(file, hunk, line);
            if let Some(n) = l.new_no.or(l.old_no) {
                location.push_str(&format!(":{n}"));
            }
        }
        if let Row::Context { no, .. } = row {
            location.push_str(&format!(":{no}"));
        }
        parts.push(location);
        parts.push(match app.view {
            View::Files => format!("file {}/{}", app.page + 1, app.diff.files.len()),
            View::Plan => format!("group {}/{}", app.page + 1, app.plan.len()),
        });
        if let (Some(file), Some(hunk)) = (row.file(), row.hunk()) {
            parts.push(format!("hunk {}/{}", hunk + 1, app.diff.files[file].hunks.len()));
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
            let hints = match app.mode {
                Mode::Visual { .. } => " c comment on selection  Esc cancel ",
                // The boxes list their own keys.
                Mode::Comment { .. } => " ",
                _ => " ? help  r review  c comment  [e ]e ze context  ]u unreviewed  q quit ",
            };
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

    #[test]
    fn truncates_titles_from_the_right() {
        assert_eq!(truncate_right("Core types", 20), "Core types");
        assert_eq!(truncate_right("Core types", 5), "Core…");
        assert_eq!(truncate_right("Core", 0), "");
    }

    #[test]
    fn wraps_spans_at_the_room_given() {
        let text = |lines: Vec<Vec<Span>>| -> Vec<String> {
            lines.iter().map(|l| l.iter().map(|s| s.content.as_ref()).collect()).collect()
        };
        let spans = || vec![Span::raw("abcd"), Span::raw("éfg"), Span::raw("h")];
        assert_eq!(text(wrap_spans(spans(), 3)), ["abc", "déf", "gh"]);
        assert_eq!(text(wrap_spans(spans(), 0)), ["abcdéfgh"]);
        assert_eq!(text(wrap_spans(spans(), 8)), ["abcdéfgh"]);
        assert_eq!(text(wrap_spans(vec![], 4)), [""]);
        assert_eq!(wrap_spans(spans(), 3)[1].len(), 2, "split where a span crosses a break");
    }

    #[test]
    fn emphasizes_changed_ranges_across_segments() {
        let bold = Style::new().bold();
        let tint = Style::new().bg(Color::Red);
        let segments = vec![(bold, "let x".to_string()), (Style::new(), " = 16;".to_string())];
        let out = emphasize(segments, &[4..5, 8..10], tint);
        let pieces: Vec<(&str, bool)> = out.iter().map(|(s, t)| (t.as_str(), s.bg == Some(Color::Red))).collect();
        assert_eq!(pieces, [("let ", false), ("x", true), (" = ", false), ("16", true), (";", false)]);
        assert_eq!(out[1].0.add_modifier, bold.add_modifier, "keeps the syntax style under the tint");
        assert_eq!(emphasize(vec![(bold, "x".into())], &[], tint).len(), 1);
    }

    #[test]
    fn wraps_input_and_tracks_the_cursor() {
        assert_eq!(wrap_input("", 0, 5), (vec![String::new()], (0, 0)));
        assert_eq!(wrap_input("abc", 3, 5), (vec!["abc".into()], (3, 0)));
        assert_eq!(wrap_input("abcdefg", 2, 5), (vec!["abcde".into(), "fg".into()], (2, 0)));
        assert_eq!(wrap_input("abcdefg", 6, 5).1, (1, 1));
        assert_eq!(wrap_input("ab\ncd", 2, 5), (vec!["ab".into(), "cd".into()], (2, 0)));
        assert_eq!(wrap_input("ab\n", 3, 5), (vec!["ab".into(), String::new()], (0, 1)));
        // A full last row puts an end-of-text cursor on a new row.
        assert_eq!(wrap_input("abcde", 5, 5), (vec!["abcde".into(), String::new()], (0, 1)));
    }
}
