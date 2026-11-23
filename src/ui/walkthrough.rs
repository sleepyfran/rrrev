//! The walkthrough on screen: the slide's explanation on the left, the code
//! it's about on the right, or one above the other in a narrow terminal.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use super::{clear, diff_line_in, plumbing_badge, status_style, truncate_right};
use crate::app::{App, Deck, Row, ShownHunk, SlideKind};
use crate::diff::LineKind;
use crate::highlight::Highlighter;
use crate::markdown;
use crate::text::wrap_words;

/// From this wide the explanation sits beside the code; narrower, above it.
const SIDE_BY_SIDE_FROM: u16 = 100;

/// A drawn line of the code pane: the diff row it shows, for marking what
/// was seen, and the hunk it belongs to, for `Enter`.
struct CodeLine {
    row: Option<Row>,
    hunk: Option<(usize, usize)>,
    line: Line<'static>,
}

/// Draws the open walkthrough in `area`. Returns the diff rows it showed.
pub(super) fn render_walkthrough(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    highlighter: &mut Highlighter,
) -> Vec<Row> {
    let Some(deck) = &app.walkthrough else {
        return Vec::new();
    };
    let p = app.palette;
    let side_by_side = area.width >= SIDE_BY_SIDE_FROM;
    let [text_area, code_area] = if side_by_side {
        let width = (area.width * 2 / 5).clamp(40, 80);
        Layout::horizontal([Constraint::Length(width), Constraint::Min(0)]).areas(area)
    } else {
        Layout::vertical([Constraint::Percentage(45), Constraint::Min(0)]).areas(area)
    };
    clear(frame, area, &p);

    // The explanation.
    let text_block = Block::new()
        .title(Span::styled(
            " Walkthrough ",
            Style::new().fg(p.magenta).bold(),
        ))
        .title(Span::raw(format!("· {} ", truncate_right(&app.title, 40))).dim())
        .title_bottom(
            Line::from(
                " h/l slides · j/k code · J/K text · r reviewed · Enter go there · q close ",
            )
            .dim(),
        );
    let text_inner = text_block.inner(text_area);
    frame.render_widget(text_block, text_area);
    let text_inner = Rect {
        x: text_inner.x + 1,
        width: text_inner.width.saturating_sub(2),
        ..text_inner
    };
    let text = slide_text(app, deck, text_inner.width as usize, highlighter);
    let text_height = text_inner.height as usize;
    let text_scroll = deck.text_scroll.min(text.len().saturating_sub(text_height));
    let more_text = text.len() > text_scroll + text_height;
    frame.render_widget(
        Paragraph::new(text).scroll((text_scroll as u16, 0)),
        text_inner,
    );
    if more_text && text_inner.height > 0 {
        let hint = Rect {
            y: text_inner.bottom() - 1,
            height: 1,
            ..text_inner
        };
        frame.render_widget(
            Paragraph::new(Line::from(" ⋯ J for more ").dim().right_aligned()),
            hint,
        );
    }

    // The code.
    let slide = deck.current();
    let code_block = Block::new()
        .borders(if side_by_side {
            Borders::LEFT
        } else {
            Borders::TOP
        })
        .border_style(Style::new().fg(p.muted))
        .title(Span::styled(
            match slide.kind {
                SlideKind::Overview => " Files ".to_string(),
                _ => format!(" Code · {} ", lines_label(app, &slide.code)),
            },
            Style::new().bold(),
        ));
    let code_inner = code_block.inner(code_area);
    frame.render_widget(code_block, code_area);
    let code_inner = Rect {
        x: code_inner.x + 1,
        width: code_inner.width.saturating_sub(1),
        ..code_inner
    };
    let width = code_inner.width as usize;
    let code = match slide.kind {
        SlideKind::Overview => overview_files(app, deck, width),
        SlideKind::Step | SlideKind::LeftOut => code_lines(app, &slide.code, width, highlighter),
    };
    let height = code_inner.height as usize;
    let code_scroll = deck.code_scroll.min(code.len().saturating_sub(height));
    let shown = &code[code_scroll.min(code.len())..code.len().min(code_scroll + height)];
    let rows: Vec<Row> = shown.iter().filter_map(|l| l.row).collect();
    let code_top = shown.iter().find_map(|l| l.hunk);
    frame.render_widget(
        Paragraph::new(shown.iter().map(|l| l.line.clone()).collect::<Vec<_>>()),
        code_inner,
    );

    if let Some(deck) = &mut app.walkthrough {
        deck.text_scroll = text_scroll;
        deck.code_scroll = code_scroll;
        deck.code_height = height;
        deck.code_top = code_top;
    }
    rows
}

/// The slide's position, title, how far its hunks are, and its explanation.
fn slide_text(
    app: &App,
    deck: &Deck,
    width: usize,
    highlighter: &mut Highlighter,
) -> Vec<Line<'static>> {
    let p = app.palette;
    let slide = deck.current();
    let mut lines = Vec::new();

    // Where it is in the deck, as dots when they fit.
    let count = deck.slides.len();
    let position = format!(" {}/{count}", deck.slide + 1);
    let mut dots: Vec<Span> = Vec::new();
    if count * 2 + position.len() <= width {
        for (i, s) in deck.slides.iter().enumerate() {
            let done = !s.hunks.is_empty() && s.hunks.iter().all(|&(f, h)| app.is_reviewed(f, h));
            let (mark, style) = match (i == deck.slide, done) {
                (true, _) => ("●", Style::new().fg(p.magenta)),
                (false, true) => ("●", Style::new().fg(p.green)),
                (false, false) => ("○", Style::new().fg(p.muted)),
            };
            dots.push(Span::styled(mark, style));
            dots.push(Span::raw(" "));
        }
    }
    dots.push(Span::raw(position.trim_start().to_string()).dim());
    lines.push(Line::from(dots));
    lines.push(Line::default());

    for text in wrap_words(slide.title.trim(), width) {
        lines.push(Line::from(Span::styled(
            text,
            Style::new().fg(p.magenta).bold(),
        )));
    }
    if !slide.hunks.is_empty() {
        let reviewed = slide
            .hunks
            .iter()
            .filter(|&&(f, h)| app.is_reviewed(f, h))
            .count();
        let total = slide.hunks.len();
        let plural = if total == 1 { "" } else { "s" };
        lines.push(if reviewed == total {
            Line::from(Span::styled(
                format!("✓ {total} hunk{plural} reviewed"),
                Style::new().fg(p.green),
            ))
        } else {
            Line::from(format!(
                "{reviewed} of {total} hunk{plural} reviewed · r marks them"
            ))
            .dim()
        });
    }
    lines.push(Line::default());

    lines.extend(markdown::render(
        &slide.body,
        width,
        &p,
        &mut |tag, source| highlighter.snippet(tag, source),
    ));
    if slide.missing > 0 {
        lines.push(Line::default());
        let what = match slide.missing {
            1 => "Some of its code has".to_string(),
            n => format!("{n} pieces of its code have"),
        };
        for text in wrap_words(
            &format!("{what} changed since: :walkthrough new writes a fresh one."),
            width,
        ) {
            lines.push(Line::from(Span::styled(text, Style::new().fg(p.yellow))));
        }
    }
    lines
}

/// `12 lines in 2 files`: how much code the slide shows.
fn lines_label(app: &App, code: &[ShownHunk]) -> String {
    let lines: usize = code.iter().map(|c| c.lines.len()).sum();
    let mut files: Vec<usize> = code.iter().map(|c| c.file).collect();
    files.dedup();
    files.sort_unstable();
    files.dedup();
    let path = match files.as_slice() {
        [file] => app.diff.files[*file].display_path(),
        files => format!("{} files", files.len()),
    };
    format!("{lines} line{} · {path}", if lines == 1 { "" } else { "s" })
}

/// The overview's files: each with its changes, the slides that show it
/// and how much of it is plumbing.
fn overview_files(app: &App, deck: &Deck, width: usize) -> Vec<CodeLine> {
    let p = app.palette;
    let mut out = Vec::new();
    for entry in &deck.current().files {
        let f = &app.diff.files[entry.file];
        let count = |kind| {
            f.hunks
                .iter()
                .flat_map(|h| &h.lines)
                .filter(|l| l.kind == kind)
                .count()
        };
        let mut spans = vec![
            Span::styled(
                format!(" {} ", f.status.label()),
                status_style(f.status, &p).reversed(),
            ),
            Span::raw(" "),
            Span::styled(
                truncate_right(&f.display_path(), width.saturating_sub(5)),
                Style::new().bold(),
            ),
        ];
        let mut details = vec![
            Span::raw("    "),
            Span::styled(
                format!("+{}", count(LineKind::Added)),
                Style::new().fg(p.green),
            ),
            Span::raw(" "),
            Span::styled(
                format!("−{}", count(LineKind::Removed)),
                Style::new().fg(p.red),
            ),
        ];
        if !entry.slides.is_empty() {
            let slides: Vec<String> = entry.slides.iter().map(|s| s.to_string()).collect();
            let noun = if slides.len() == 1 { "slide" } else { "slides" };
            details.push(Span::raw(format!("  {noun} {}", slides.join(", "))).dim());
        }
        if entry.plumbing > 0 {
            details.push(Span::raw("  "));
            if entry.slides.is_empty() {
                details.push(plumbing_badge(&p));
            } else {
                details.push(Span::styled(
                    format!("{} plumbing", entry.plumbing),
                    Style::new().fg(p.magenta).italic(),
                ));
            }
        }
        if entry.slides.is_empty() && entry.plumbing == 0 {
            details.push(Span::raw("  not in the walkthrough").dim());
        }
        let first = (!f.hunks.is_empty()).then_some((entry.file, 0));
        out.push(CodeLine {
            row: None,
            hunk: first,
            line: Line::from(std::mem::take(&mut spans)),
        });
        out.push(CodeLine {
            row: None,
            hunk: first,
            line: Line::from(details),
        });
    }
    if out.is_empty() {
        out.push(CodeLine {
            row: None,
            hunk: None,
            line: Line::from(" No files.").dim(),
        });
    }
    out
}

/// The slide's code: a header per file and hunk, then the lines it shows,
/// with `⋯` where it skips some.
fn code_lines(
    app: &App,
    code: &[ShownHunk],
    width: usize,
    highlighter: &mut Highlighter,
) -> Vec<CodeLine> {
    let p = app.palette;
    let mut out = Vec::new();
    let mut last_file = None;
    let plain = |line: Line<'static>, hunk| CodeLine {
        row: None,
        hunk,
        line,
    };
    for shown in code {
        let (file, hunk) = (shown.file, shown.hunk);
        let at = Some((file, hunk));
        if !out.is_empty() {
            out.push(plain(Line::default(), at));
        }
        if last_file != Some(file) {
            let f = &app.diff.files[file];
            out.push(plain(
                Line::from(vec![
                    Span::styled(
                        format!(" {} ", f.status.label()),
                        status_style(f.status, &p).reversed(),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        f.display_path(),
                        Style::new().bold().add_modifier(Modifier::UNDERLINED),
                    ),
                ]),
                at,
            ));
            last_file = Some(file);
        }
        let h = app.hunk(file, hunk);
        let ranges = match h.header.match_indices("@@").nth(1) {
            Some((i, _)) => h.header[..i + 2].to_string(),
            None => h.header.clone(),
        };
        let mut header = vec![
            Span::raw(if app.is_reviewed(file, hunk) {
                "✓ "
            } else {
                "  "
            }),
            Span::styled(ranges, Style::new().fg(p.cyan)),
        ];
        if let Some(scope) = app.diff.hunk_scope(file, hunk) {
            header.push(Span::raw(" "));
            header.push(Span::styled(scope, Style::new().bold()));
        }
        if shown.lines.len() < h.lines.len() {
            header.push(
                Span::raw(format!(
                    "  {} of {} lines",
                    shown.lines.len(),
                    h.lines.len()
                ))
                .dim(),
            );
        }
        out.push(plain(Line::from(header), at));
        let mut previous: Option<usize> = None;
        for &line in &shown.lines {
            if previous.is_some_and(|prev| prev + 1 != line) {
                out.push(plain(
                    Line::from(Span::styled(
                        format!("{:>w$}⋯", "", w = app.code_column() - 2),
                        Style::new().fg(p.muted),
                    )),
                    at,
                ));
            }
            previous = Some(line);
            let row = Row::Line { file, hunk, line };
            for drawn in diff_line_in(app, highlighter, file, hunk, line, None, width) {
                out.push(CodeLine {
                    row: Some(row),
                    hunk: at,
                    line: drawn,
                });
            }
        }
    }
    if out.is_empty() {
        out.push(plain(Line::from(" No code on this slide.").dim(), None));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::Animator;
    use crate::app::{Action, WalkthroughAction};
    use crate::appearance::Appearance;
    use crate::diff::{Diff, DiffLine, FileDiff, FileStatus, Hunk};
    use crate::review::ReviewState;
    use crate::theme::Theme;
    use crate::walkthrough::{CodeRef, Slide, Walkthrough};
    use ratatui::{Terminal, backend::TestBackend};

    fn file(path: &str, lines: &[&str]) -> FileDiff {
        FileDiff {
            old_path: Some(path.into()),
            new_path: Some(path.into()),
            status: FileStatus::Modified,
            is_binary: false,
            hunks: vec![Hunk {
                id: Default::default(),
                header: "@@ -1,0 +1,3 @@".into(),
                old_start: 1,
                old_lines: 0,
                new_start: 1,
                new_lines: lines.len() as u32,
                lines: lines
                    .iter()
                    .enumerate()
                    .map(|(i, content)| DiffLine {
                        kind: LineKind::Added,
                        old_no: None,
                        new_no: Some(i as u32 + 1),
                        content: content.to_string(),
                    })
                    .collect(),
            }],
        }
    }

    #[test]
    fn draws_the_explanation_beside_the_code() {
        let diff = Diff::new(vec![
            file(
                "src/core.rs",
                &["fn one() {}", "fn two() {}", "fn three() {}"],
            ),
            file("src/wire.rs", &["use core::one;"]),
        ]);
        let id = |f: usize| diff.files[f].hunks[0].id.clone();
        let mut review = ReviewState::default();
        review.set_walkthrough(Walkthrough {
            summary: "Adds `one`.".into(),
            slides: vec![Slide {
                title: "The core".into(),
                body: "Where **it** starts.".into(),
                code: vec![CodeRef {
                    hunk: id(0),
                    from: Some(2),
                    to: Some(3),
                }],
            }],
            plumbing: vec![id(1)],
        });
        let mut app = App::new("main..feat".into(), diff, review);
        app.update(Action::OpenWalkthrough);
        let mut highlighter = Highlighter::new(Theme::terminal(), Appearance::Dark);
        let mut anim = Animator::new(false);
        let mut screen = |app: &mut App, width: u16, height: u16| {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| super::super::render(frame, app, &mut highlighter, &mut anim, ""))
                .unwrap();
            let buffer = terminal.backend().buffer();
            (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };

        let shown = screen(&mut app, 120, 30).join("\n");
        assert!(shown.contains(" Walkthrough "));
        assert!(shown.contains("1/3"));
        assert!(shown.contains("2. The core"));
        assert!(shown.contains("slide 2"));
        assert!(shown.contains("WALKTHROUGH"));

        app.walkthrough_action(WalkthroughAction::Next(1));
        let rows = screen(&mut app, 120, 30);
        // Explanation on the left, code on the right, on the same rows.
        let row = rows.iter().find(|r| r.contains("The core")).unwrap();
        assert!(row.find("The core").unwrap() < 50);
        assert!(rows.iter().any(|r| r.contains("2 of 3 lines")));
        let two = rows.iter().find(|r| r.contains("fn two()")).unwrap();
        assert!(two.find("fn two()").unwrap() > 50);
        assert!(!rows.iter().any(|r| r.contains("fn one()")));
        // What was on screen counts as read.
        let stats = app.line_stats();
        assert_eq!(stats.read, 2);

        app.walkthrough_action(WalkthroughAction::Next(1));
        let shown = screen(&mut app, 120, 30).join("\n");
        assert!(shown.contains("Left out"));
        assert!(shown.contains("src/wire.rs:1"));

        // Too narrow to sit side by side: one above the other.
        app.walkthrough_action(WalkthroughAction::Prev(1));
        let rows = screen(&mut app, 80, 40);
        let title = rows.iter().position(|r| r.contains("The core")).unwrap();
        let code = rows.iter().position(|r| r.contains("fn two()")).unwrap();
        assert!(title < code);
    }
}
