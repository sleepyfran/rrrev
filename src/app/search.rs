//! `/` search across the whole diff, in page order, with `n` / `N`.

use std::ops::Range;

use super::{App, Mode, Row};
use crate::input::{InputAction, TextInput};

/// Byte ranges of `query` in `text`, without overlaps. Smart case, as in
/// vim: case-insensitive unless the query has a capital letter.
pub fn find_matches(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let ignore_case = !query.chars().any(char::is_uppercase);
    let same = |a: char, b: char| if ignore_case { a.to_lowercase().eq(b.to_lowercase()) } else { a == b };
    let mut matches = Vec::new();
    let mut from = 0;
    for (start, _) in text.char_indices() {
        if start < from {
            continue;
        }
        let mut rest = text[start..].char_indices();
        let mut end = start;
        let found = query.chars().all(|q| match rest.next() {
            Some((offset, c)) if same(c, q) => {
                end = start + offset + c.len_utf8();
                true
            }
            _ => false,
        });
        if found {
            matches.push(start..end);
            from = end;
        }
    }
    matches
}

impl App {
    pub(super) fn start_search(&mut self) {
        self.mode = Mode::Search { input: TextInput::default() };
    }

    pub(super) fn search_input(&mut self, action: InputAction) {
        let Mode::Search { input } = &mut self.mode else { return };
        match action {
            InputAction::Submit => {
                let query = input.text().to_string();
                self.mode = Mode::Normal;
                // As in vim, an empty search repeats the last one.
                if !query.is_empty() {
                    self.search = Some(query);
                }
                self.search_next(1, true);
            }
            InputAction::Cancel => self.mode = Mode::Normal,
            InputAction::Backspace if input.text().is_empty() => self.mode = Mode::Normal,
            InputAction::Newline | InputAction::OpenEditor => {}
            other => input.apply(other),
        }
    }

    /// `n` / `N`: the `n`th match forward or back, wrapping around the ends.
    pub(super) fn search_next(&mut self, n: usize, forward: bool) {
        let Some(query) = self.search.clone() else {
            self.message = Some("No previous search".to_string());
            return;
        };
        self.highlight_search = true;
        let stops = self.stops(|row| self.row_matches(*row, &query), |file| self.file_matches(file, &query));
        let Some(&wrapped) = (if forward { stops.first() } else { stops.last() }) else {
            self.message = Some(format!("Pattern not found: {query}"));
            return;
        };
        if !self.jump_to_stop(&stops, n, forward) {
            self.open_page(wrapped.0, wrapped.1);
            self.scroll = wrapped.1;
            self.message = Some(
                if forward { "search hit BOTTOM, continuing at TOP" } else { "search hit TOP, continuing at BOTTOM" }
                    .to_string(),
            );
        }
    }

    /// `Esc` in normal mode: hides matches, but `n` still finds the next one.
    pub(super) fn clear_search_highlight(&mut self) {
        self.highlight_search = false;
    }

    /// The query whose matches the UI should highlight, if any.
    pub fn highlighted_search(&self) -> Option<&str> {
        self.search.as_deref().filter(|_| self.highlight_search)
    }

    /// The text a search looks at on a row.
    fn row_text(&self, row: Row) -> Option<String> {
        Some(match row {
            Row::Group { group, line } => self.group_header(group).get(line)?.1.clone(),
            Row::File { file } => self.diff.files[file].display_path(),
            Row::Hunk { file, hunk } => self.hunk(file, hunk).header.clone(),
            Row::Line { file, hunk, line } => self.line(file, hunk, line).content.clone(),
            Row::Comment { id, line, .. } => self.review.comment(id)?.text.lines().nth(line)?.to_string(),
        })
    }

    fn row_matches(&self, row: Row, query: &str) -> bool {
        self.row_text(row).is_some_and(|text| !find_matches(&text, query).is_empty())
    }

    /// Whether a match is hidden inside a folded file.
    fn file_matches(&self, file: usize, query: &str) -> bool {
        let lines = self.diff.files[file].hunks.iter().flat_map(|h| &h.lines);
        lines.into_iter().any(|line| !find_matches(&line.content, query).is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::Action;

    fn search(app: &mut App, query: &str) {
        app.update(Action::StartSearch);
        type_text(app, query);
        app.update_input(InputAction::Submit);
    }

    #[test]
    fn matches_use_smart_case() {
        assert_eq!(find_matches("Fix the fix", "fix"), [0..3, 8..11]);
        // A capital makes it case-sensitive: only the first "Fix".
        assert_eq!(find_matches("Fix the fix", "Fix").first(), Some(&(0..3)));
        assert_eq!(find_matches("Fix the fix", "Fix").len(), 1);
        assert_eq!(find_matches("aaaa", "aa"), [0..2, 2..4]);
        assert_eq!(find_matches("café Café", "café"), [0..5, 6..11]);
        assert!(find_matches("abc", "").is_empty());
        assert!(find_matches("ab", "abc").is_empty());
    }

    #[test]
    fn search_jumps_across_pages_and_wraps() {
        // Line contents are "<path> <hunk> <line>", e.g. "b.rs 0 2".
        let mut app = sample_app();
        search(&mut app, "0 2");
        assert_eq!(position(&app), (0, 4));
        app.update(Action::SearchNext(1));
        assert_eq!(position(&app), (1, 4));
        app.update(Action::SearchNext(1));
        assert_eq!(position(&app), (0, 4));
        assert_eq!(app.message.as_deref(), Some("search hit BOTTOM, continuing at TOP"));
        app.update(Action::SearchPrev(1));
        assert_eq!(position(&app), (1, 4));
        assert_eq!(app.message.as_deref(), Some("search hit TOP, continuing at BOTTOM"));
    }

    #[test]
    fn search_finds_comments_and_reaches_into_folds() {
        let mut app = sample_app();
        app.update(Action::NextFile(1));
        app.update(Action::Down(2));
        comment(&mut app, "Needle here");
        app.update(Action::PrevFile(1));
        search(&mut app, "needle");
        assert_eq!(position(&app), (1, 3));

        // A match in a folded file lands on its header.
        app.update(Action::CloseFold);
        app.update(Action::PrevFile(1));
        search(&mut app, "b.rs 0 3");
        assert_eq!(app.current_row(), Some(Row::File { file: 1 }));
    }

    #[test]
    fn misses_empty_searches_and_highlight() {
        let mut app = sample_app();
        app.update(Action::SearchNext(1));
        assert_eq!(app.message.as_deref(), Some("No previous search"));
        search(&mut app, "nowhere");
        assert_eq!(app.message.as_deref(), Some("Pattern not found: nowhere"));

        search(&mut app, "a.rs 1");
        assert_eq!(app.highlighted_search(), Some("a.rs 1"));
        app.update(Action::Cancel);
        assert_eq!(app.highlighted_search(), None);
        // An empty search repeats the last one, from the current match, and
        // brings the highlight back.
        search(&mut app, "");
        assert_eq!(app.highlighted_search(), Some("a.rs 1"));
        assert_eq!(position(&app), (0, 7));
    }
}
