//! Markdown laid out for a narrow terminal pane, as the AI's answers come:
//! wrapped paragraphs keeping their inline styles, headings, lists, quotes,
//! code blocks (highlighted when there's a grammar for them), rules and
//! tables.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::highlight::Segment;
use crate::theme::Palette;

/// Highlights a code block's lines in a language, by its fence's tag; a
/// line it leaves empty, or one it doesn't return, is drawn plain.
pub type CodeHighlighter<'a> = dyn FnMut(&str, &[String]) -> Vec<Vec<Segment>> + 'a;

/// `text` as lines at most `width` columns wide.
pub fn render(text: &str, width: usize, palette: &Palette, code: &mut CodeHighlighter) -> Vec<Line<'static>> {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut renderer = Renderer {
        width: width.max(1),
        p: palette,
        code,
        lines: Vec::new(),
        inline: Vec::new(),
        styles: Vec::new(),
        containers: Vec::new(),
        lists: Vec::new(),
        links: Vec::new(),
        code_block: None,
        table: None,
    };
    for event in Parser::new_ext(text, options) {
        renderer.event(event);
    }
    renderer.flush();
    while renderer.lines.last().is_some_and(|l| l.width() == 0) {
        renderer.lines.pop();
    }
    renderer.lines
}

/// A block the lines being written are inside of, which puts something
/// before each of them.
enum Container {
    Quote,
    /// `marker` starts the item's first line and its width in spaces the rest.
    Item { marker: String, used: bool },
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<Vec<Span<'static>>>>,
    row: Vec<Vec<Span<'static>>>,
    /// Whether the first row is a header.
    head: bool,
}

struct Renderer<'a, 'b> {
    width: usize,
    p: &'a Palette,
    code: &'a mut CodeHighlighter<'b>,
    lines: Vec<Line<'static>>,
    /// The paragraph, heading or list item text being gathered.
    inline: Vec<Span<'static>>,
    styles: Vec<Style>,
    containers: Vec<Container>,
    /// Per list: the number of its next item, or `None` for bullets.
    lists: Vec<Option<u64>>,
    /// Per open link: where it goes and where its text starts in `inline`.
    links: Vec<(String, usize)>,
    /// The fence's tag and the text so far.
    code_block: Option<(String, String)>,
    table: Option<Table>,
}

impl Renderer<'_, '_> {
    fn event(&mut self, event: Event) {
        if let Some((_, text)) = &mut self.code_block {
            match event {
                Event::Text(t) => text.push_str(&t),
                Event::End(TagEnd::CodeBlock) => self.end_code_block(),
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) | Event::InlineHtml(text) | Event::InlineMath(text) => self.text(&text, self.style()),
            Event::Html(text) | Event::DisplayMath(text) => {
                for line in text.lines() {
                    self.text(line, self.style());
                    self.flush();
                }
            }
            Event::Code(text) => self.text(&text, self.style().fg(self.p.yellow)),
            Event::SoftBreak => self.text(" ", self.style()),
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.block();
                let room = self.room();
                self.emit(vec![Span::styled("─".repeat(room), Style::new().fg(self.p.muted))]);
            }
            Event::TaskListMarker(done) => {
                let (mark, style) = if done { ("✓ ", Style::new().fg(self.p.green)) } else { ("☐ ", Style::new()) };
                self.inline.push(Span::styled(mark, style));
            }
            Event::FootnoteReference(name) => self.text(&format!("[{name}]"), self.style().dim()),
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.block(),
            Tag::Heading { level, .. } => {
                self.block();
                let style = Style::new().fg(self.p.blue).bold();
                self.styles.push(if level == HeadingLevel::H1 { style.underlined() } else { style });
            }
            Tag::BlockQuote(_) => {
                self.block();
                self.containers.push(Container::Quote);
                self.styles.push(self.style().italic());
            }
            Tag::CodeBlock(kind) => {
                self.block();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info.split([' ', ',', '{']).next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code_block = Some((language, String::new()));
            }
            Tag::List(first) => {
                // A list in an item hangs from it, without a gap.
                if self.lists.is_empty() {
                    self.block();
                } else {
                    self.flush();
                }
                self.lists.push(first);
            }
            Tag::Item => {
                self.flush();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}. ", *n - 1)
                    }
                    _ => "• ".to_string(),
                };
                self.containers.push(Container::Item { marker, used: false });
            }
            Tag::Table(_) => {
                self.block();
                self.table = Some(Table::default());
            }
            Tag::TableHead => {
                if let Some(table) = &mut self.table {
                    table.head = true;
                }
                self.styles.push(self.style().bold());
            }
            Tag::TableRow | Tag::TableCell => {}
            Tag::Emphasis => self.styles.push(self.style().italic()),
            Tag::Strong => self.styles.push(self.style().bold()),
            Tag::Strikethrough => self.styles.push(self.style().crossed_out()),
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.links.push((dest_url.to_string(), self.inline.len()));
                self.styles.push(self.style().fg(self.p.blue).underlined());
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush(),
            TagEnd::Heading(_) => {
                self.flush();
                self.styles.pop();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.containers.pop();
                self.styles.pop();
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
            }
            TagEnd::Item => {
                self.flush();
                // An empty item still shows its marker.
                if let Some(Container::Item { used: false, .. }) = self.containers.last() {
                    self.emit(Vec::new());
                }
                self.containers.pop();
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.inline);
                if let Some(table) = &mut self.table {
                    table.row.push(cell);
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if tag == TagEnd::TableHead {
                    self.styles.pop();
                }
                if let Some(table) = &mut self.table {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.end_table(table);
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link | TagEnd::Image => {
                self.styles.pop();
                if let Some((url, start)) = self.links.pop() {
                    let text: String = self.inline[start..].iter().map(|s| s.content.as_ref()).collect();
                    if !url.is_empty() && text != url && !url.starts_with('#') {
                        self.text(&format!(" ({url})"), self.style().dim());
                    }
                }
            }
            _ => {}
        }
    }

    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn text(&mut self, text: &str, style: Style) {
        if !text.is_empty() {
            self.inline.push(Span::styled(text.to_string(), style));
        }
    }

    /// Starts a block: ends the text before it, and leaves a blank line
    /// after what came before, unless it opens a list item.
    fn block(&mut self) {
        self.flush();
        let fresh_item = matches!(self.containers.last(), Some(Container::Item { used: false, .. }));
        if fresh_item || self.lines.last().is_none_or(|l| l.width() == 0 || is_bar_only(l)) {
            return;
        }
        let bars: Vec<Span> = self
            .containers
            .iter()
            .filter(|c| matches!(c, Container::Quote))
            .map(|_| Span::styled("│", Style::new().fg(self.p.muted)))
            .collect();
        self.lines.push(Line::from(bars));
    }

    /// Columns left for text after what the containers put before it.
    fn room(&self) -> usize {
        let taken: usize = self
            .containers
            .iter()
            .map(|c| match c {
                Container::Quote => 2,
                Container::Item { marker, .. } => marker.chars().count(),
            })
            .sum();
        self.width.saturating_sub(taken).max(1)
    }

    /// Writes one line inside the containers: an item's marker the first
    /// time, its indent after.
    fn emit(&mut self, content: Vec<Span<'static>>) {
        let mut spans = Vec::new();
        for container in &mut self.containers {
            match container {
                Container::Quote => spans.push(Span::styled("│ ", Style::new().fg(self.p.muted))),
                Container::Item { marker, used } => {
                    let text = if *used { " ".repeat(marker.chars().count()) } else { marker.clone() };
                    spans.push(Span::styled(text, Style::new().fg(self.p.magenta)));
                    *used = true;
                }
            }
        }
        spans.extend(content);
        self.lines.push(Line::from(spans));
    }

    /// Wraps and writes the text gathered so far.
    fn flush(&mut self) {
        if self.inline.is_empty() || self.table.is_some() {
            return;
        }
        let spans = std::mem::take(&mut self.inline);
        for line in wrap(&spans, self.room()) {
            self.emit(line);
        }
    }

    fn end_code_block(&mut self) {
        let Some((language, text)) = self.code_block.take() else { return };
        let source: Vec<String> = text.trim_end_matches('\n').split('\n').map(|l| l.replace('\t', "    ")).collect();
        let highlighted = if language.is_empty() { Vec::new() } else { (self.code)(&language, &source) };
        let gutter = Style::new().fg(self.p.muted);
        let room = self.room().saturating_sub(2).max(1);
        for (i, line) in source.iter().enumerate() {
            let segments = match highlighted.get(i) {
                Some(segments) if !segments.is_empty() => segments.clone(),
                _ => vec![(Style::new().fg(self.p.faint), line.clone())],
            };
            for piece in split_segments(segments, room) {
                let mut spans = vec![Span::styled("▎ ", gutter)];
                spans.extend(piece.into_iter().map(|(style, text)| Span::styled(text, style)));
                self.emit(spans);
            }
        }
    }

    /// Side by side when the columns fit, else each row as `header: value`
    /// lines.
    fn end_table(&mut self, table: Table) {
        let width = |cell: &[Span]| cell.iter().map(Span::width).sum::<usize>();
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        let widths: Vec<usize> = (0..columns)
            .map(|c| table.rows.iter().filter_map(|r| r.get(c)).map(|cell| width(cell)).max().unwrap_or(0))
            .collect();
        let separator = Style::new().fg(self.p.muted);
        let room = self.room();
        if widths.iter().sum::<usize>() + 3 * columns.saturating_sub(1) <= room {
            for (r, row) in table.rows.iter().enumerate() {
                let mut spans = Vec::new();
                for (c, w) in widths.iter().enumerate() {
                    if c > 0 {
                        spans.push(Span::styled(" │ ", separator));
                    }
                    let cell = row.get(c).map_or(&[][..], Vec::as_slice);
                    spans.extend(cell.iter().cloned());
                    if c + 1 < columns {
                        spans.push(Span::raw(" ".repeat(w - width(cell))));
                    }
                }
                self.emit(spans);
                if r == 0 && table.head {
                    let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                    self.emit(vec![Span::styled(rule.join("─┼─"), separator)]);
                }
            }
            return;
        }
        let (header, body) = match table.head {
            true => (table.rows.first().cloned().unwrap_or_default(), &table.rows[1.min(table.rows.len())..]),
            false => (Vec::new(), &table.rows[..]),
        };
        for (r, row) in body.iter().enumerate() {
            if r > 0 {
                self.emit(Vec::new());
            }
            for (c, cell) in row.iter().enumerate() {
                let mut spans: Vec<Span<'static>> = Vec::new();
                if let Some(name) = header.get(c).filter(|h| width(h) > 0) {
                    spans.extend(name.iter().cloned());
                    spans.push(Span::styled(": ", Style::new().bold()));
                }
                spans.extend(cell.iter().cloned());
                for line in wrap(&spans, room) {
                    self.emit(line);
                }
            }
        }
    }
}

fn is_bar_only(line: &Line) -> bool {
    line.spans.iter().all(|s| s.content.trim().is_empty() || s.content == "│")
}

/// Greedy word wrap of styled text to `width` columns; a word longer than a
/// line is split. Words may change style midway, like `**bold**,`.
fn wrap(spans: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>> {
    // Each word as its runs of one style.
    let mut words: Vec<Vec<Segment>> = Vec::new();
    let mut word: Vec<Segment> = Vec::new();
    for span in spans {
        for c in span.content.chars() {
            if c.is_whitespace() {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                continue;
            }
            match word.last_mut() {
                Some((style, text)) if *style == span.style => text.push(c),
                _ => word.push((span.style, c.to_string())),
            }
        }
    }
    if !word.is_empty() {
        words.push(word);
    }

    let mut lines: Vec<Vec<Segment>> = Vec::new();
    let mut line: Vec<Segment> = Vec::new();
    let mut used = 0;
    for word in words {
        let len: usize = word.iter().map(|(_, t)| t.chars().count()).sum();
        if used > 0 && used + 1 + len <= width {
            line.push((Style::new(), " ".to_string()));
            line.extend(word);
            used += 1 + len;
            continue;
        }
        if used > 0 {
            lines.push(std::mem::take(&mut line));
        }
        let mut pieces = split_segments(word, width);
        let last = pieces.pop().unwrap_or_default();
        lines.extend(pieces);
        used = last.iter().map(|(_, t)| t.chars().count()).sum();
        line = last;
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines.into_iter().map(|l| l.into_iter().map(|(style, text)| Span::styled(text, style)).collect()).collect()
}

/// Cuts a line of segments every `width` characters.
fn split_segments(segments: Vec<Segment>, width: usize) -> Vec<Vec<Segment>> {
    let mut lines = vec![Vec::new()];
    let mut used = 0;
    for (style, text) in segments {
        let mut chars = text.chars().peekable();
        while chars.peek().is_some() {
            if used == width {
                lines.push(Vec::new());
                used = 0;
            }
            let piece: String = chars.by_ref().take(width - used).collect();
            used += piece.chars().count();
            lines.last_mut().expect("never empty").push((style, piece));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::Appearance;
    use crate::theme::Theme;
    use ratatui::style::Modifier;

    fn plain(text: &str, width: usize) -> Vec<String> {
        lines(text, width).iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
    }

    fn lines(text: &str, width: usize) -> Vec<Line<'static>> {
        let palette = Theme::terminal().palette(Appearance::Dark);
        render(text, width, &palette, &mut |_, _| Vec::new())
    }

    #[test]
    fn wraps_paragraphs_keeping_inline_styles() {
        let out = lines("It calls **`floor`** on the\nslice, then *returns*.", 16);
        let text: Vec<String> = out.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect();
        assert_eq!(text, ["It calls floor", "on the slice,", "then returns."]);
        let floor = out[0].spans.iter().find(|s| s.content == "floor").unwrap();
        assert!(floor.style.add_modifier.contains(Modifier::BOLD));
        assert!(floor.style.fg.is_some());
        let returns = out[2].spans.iter().find(|s| s.content == "returns").unwrap();
        assert!(returns.style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn separates_blocks_and_hangs_list_items() {
        let text = "# Title\n\nFirst.\n\n- one two three\n- four\n  1. five\n\n> quoted";
        assert_eq!(
            plain(text, 12),
            ["Title", "", "First.", "", "• one two", "  three", "• four", "  1. five", "", "│ quoted"]
        );
    }

    #[test]
    fn code_blocks_keep_their_lines_and_get_highlighted() {
        let palette = Theme::terminal().palette(Appearance::Dark);
        let keyword = Style::new().fg(palette.red);
        let mut asked = Vec::new();
        let out = render("```rust\nfn  a() {}\n\tb\n```", 20, &palette, &mut |lang, src| {
            asked.push((lang.to_string(), src.to_vec()));
            vec![vec![(keyword, "fn".into()), (Style::new(), "  a() {}".into())]]
        });
        assert_eq!(asked, [("rust".to_string(), vec!["fn  a() {}".to_string(), "    b".to_string()])]);
        let text: Vec<String> = out.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect();
        assert_eq!(text, ["▎ fn  a() {}", "▎     b"]);
        assert_eq!(out[0].spans[1].style, keyword);
    }

    #[test]
    fn long_code_lines_and_words_are_cut() {
        assert_eq!(plain("```\nabcdefgh\n```", 6), ["▎ abcd", "▎ efgh"]);
        assert_eq!(plain("x abcdefghij", 4), ["x", "abcd", "efgh", "ij"]);
    }

    #[test]
    fn links_show_where_they_go() {
        assert_eq!(plain("See [the docs](https://x.dev).", 80), ["See the docs (https://x.dev)."]);
        assert_eq!(plain("<https://x.dev>", 80), ["https://x.dev"]);
    }

    #[test]
    fn tables_line_up_or_fall_back_to_rows() {
        let table = "| a | bb |\n|---|---|\n| ccc | d |";
        assert_eq!(plain(table, 40), ["a   │ bb", "────┼───", "ccc │ d"]);
        assert_eq!(plain(table, 6), ["a: ccc", "bb: d"]);
    }

    #[test]
    fn generics_outside_backticks_stay_as_text() {
        assert_eq!(plain("Returns Vec<T> here.", 80), ["Returns Vec<T> here."]);
    }
}
