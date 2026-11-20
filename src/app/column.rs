//! The cursor's column on code rows (diff lines and context), moved with
//! vim's motions: `h` / `l`, `w` / `b` / `e` (and the WORD versions), `0` /
//! `^` / `$`, `f` / `F` / `t`. Other rows keep the whole-row cursor.
//!
//! `App::column` is the column the cursor wants, as vim's `curswant`: `j` /
//! `k` keep it, and a shorter line shows the cursor on its last character
//! until a longer one comes along. `$` wants the end of every line.

use std::ops::Range;

use super::{App, Row};
use crate::app::find_matches;

/// What `f`, `F` and `t` look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindChar {
    pub target: char,
    pub forward: bool,
    /// `t`: stop just before the character rather than on it.
    pub till: bool,
}

/// Vim's character classes: blanks, keyword characters, and the rest
/// (punctuation). A WORD (`W`, `B`, `E`) only tells blanks from the rest.
fn class(c: char, big: bool) -> u8 {
    if c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        2
    } else {
        1
    }
}

/// Where the next word starts after `i`, on this line.
fn next_word_start(chars: &[char], i: usize, big: bool) -> Option<usize> {
    let mut j = i;
    if let Some(&c) = chars.get(j) {
        let here = class(c, big);
        if here != 0 {
            while chars.get(j).is_some_and(|&c| class(c, big) == here) {
                j += 1;
            }
        }
    }
    while chars.get(j).is_some_and(|c| c.is_whitespace()) {
        j += 1;
    }
    (j < chars.len()).then_some(j)
}

/// Where the word before `i` (or the one `i` is inside of) starts.
fn prev_word_start(chars: &[char], i: usize, big: bool) -> Option<usize> {
    let mut j = i.min(chars.len()).checked_sub(1)?;
    while j > 0 && chars[j].is_whitespace() {
        j -= 1;
    }
    if chars[j].is_whitespace() {
        return None;
    }
    let here = class(chars[j], big);
    while j > 0 && class(chars[j - 1], big) == here {
        j -= 1;
    }
    Some(j)
}

/// Where the word after `i` (or the one `i` is inside of, if `i` isn't
/// already its last character) ends. `None` for `i` itself means "before the
/// line", to find the end of its first word.
fn word_end(chars: &[char], i: Option<usize>, big: bool) -> Option<usize> {
    let mut j = i.map_or(0, |i| i + 1);
    while chars.get(j).is_some_and(|c| c.is_whitespace()) {
        j += 1;
    }
    let here = class(*chars.get(j)?, big);
    while chars.get(j + 1).is_some_and(|&c| class(c, big) == here) {
        j += 1;
    }
    Some(j)
}

fn first_non_blank(chars: &[char]) -> usize {
    chars.iter().position(|c| !c.is_whitespace()).unwrap_or(0)
}

impl App {
    /// The code on a row, for rows that have a column.
    pub fn code_text(&self, row: Row) -> Option<&str> {
        match row {
            Row::Line { file, hunk, line } => Some(&self.line(file, hunk, line).content),
            Row::Context { file, no, .. } => super::context_text(&self.diff, file, no),
            _ => None,
        }
    }

    /// The character the cursor is on, as an index into its row's code;
    /// `None` on rows without code. An empty line has its cursor at 0.
    pub fn cursor_column(&self) -> Option<usize> {
        let text = self.code_text(self.current_row()?)?;
        Some(self.column.min(text.chars().count().saturating_sub(1)))
    }

    /// The bytes of the code under the cursor, for the UI to draw it; empty
    /// on an empty line.
    pub fn cursor_bytes(&self) -> Option<Range<usize>> {
        let text = self.code_text(self.current_row()?)?;
        let column = self.cursor_column()?;
        Some(match text.char_indices().nth(column) {
            Some((start, c)) => start..start + c.len_utf8(),
            None => text.len()..text.len(),
        })
    }

    fn cursor_chars(&self) -> Option<Vec<char>> {
        Some(self.code_text(self.current_row()?)?.chars().collect())
    }

    /// The next (or previous) row after the cursor's that has code, on this page.
    fn code_row(&self, forward: bool) -> Option<usize> {
        let has_code = |&idx: &usize| self.code_text(self.rows[idx]).is_some();
        if forward {
            (self.cursor + 1..self.rows.len()).find(has_code)
        } else {
            (0..self.cursor).rev().find(has_code)
        }
    }

    /// Moves to row `idx` with the column `pick` chooses on its characters.
    fn land(&mut self, idx: usize, pick: impl Fn(&[char]) -> Option<usize>) {
        self.cursor = idx;
        let chars = self.cursor_chars().unwrap_or_default();
        self.column = pick(&chars).unwrap_or(0);
    }

    /// `h` / `l`.
    pub(super) fn move_column(&mut self, delta: isize) {
        let (Some(column), Some(chars)) = (self.cursor_column(), self.cursor_chars()) else { return };
        self.column = column.saturating_add_signed(delta).min(chars.len().saturating_sub(1));
    }

    /// `0` / `^` / `$`.
    pub(super) fn line_start(&mut self, skip_blanks: bool) {
        if let Some(chars) = self.cursor_chars() {
            self.column = if skip_blanks { first_non_blank(&chars) } else { 0 };
        }
    }

    pub(super) fn line_end(&mut self) {
        if self.cursor_chars().is_some() {
            self.column = usize::MAX;
        }
    }

    /// `w` / `W`: the next word's start, carrying on to the next code row.
    pub(super) fn word_forward(&mut self, n: usize, big: bool) {
        for _ in 0..n {
            let here = self.cursor_chars().zip(self.cursor_column());
            if let Some(next) = here.and_then(|(chars, i)| next_word_start(&chars, i, big)) {
                self.column = next;
            } else if let Some(row) = self.code_row(true) {
                // Like vim, stops at the start of an empty line.
                self.land(row, |chars| Some(first_non_blank(chars)));
            } else {
                break;
            }
        }
    }

    /// `b` / `B`: the start of this word or the one before, carrying on to
    /// the previous code row.
    pub(super) fn word_backward(&mut self, n: usize, big: bool) {
        for _ in 0..n {
            let here = self.cursor_chars().zip(self.cursor_column());
            if let Some(prev) = here.and_then(|(chars, i)| prev_word_start(&chars, i, big)) {
                self.column = prev;
            } else if let Some(row) = self.code_row(false) {
                self.land(row, |chars| prev_word_start(chars, chars.len(), big));
            } else {
                break;
            }
        }
    }

    /// `e` / `E`: the end of this word or the next, carrying on to the next code row.
    pub(super) fn word_end(&mut self, n: usize, big: bool) {
        for _ in 0..n {
            let here = self.cursor_chars().zip(self.cursor_column());
            if let Some(end) = here.and_then(|(chars, i)| word_end(&chars, Some(i), big)) {
                self.column = end;
            } else if let Some(row) = self.code_row(true) {
                self.land(row, |chars| word_end(chars, None, big));
            } else {
                break;
            }
        }
    }

    /// `f` / `F` / `t`: the `n`th `target` on the line; stays put if there
    /// aren't that many.
    pub(super) fn find_char(&mut self, n: usize, find: FindChar) {
        let (Some(chars), Some(column)) = (self.cursor_chars(), self.cursor_column()) else { return };
        let hits: Vec<usize> = if find.forward {
            // `t` right before a match would find it again; vim skips it.
            let from = column + 1 + usize::from(find.till);
            (from..chars.len()).filter(|&i| chars[i] == find.target).collect()
        } else {
            (0..column).rev().filter(|&i| chars[i] == find.target).collect()
        };
        if let Some(&hit) = hits.get(n.saturating_sub(1)) {
            self.column = if find.till { hit - 1 } else { hit };
        }
    }

    /// After a search lands on a row, puts the column on the first match.
    pub(super) fn column_on_match(&mut self, query: &str) {
        let Some(text) = self.current_row().and_then(|row| self.code_text(row)) else { return };
        if let Some(found) = find_matches(text, query).first() {
            self.column = text[..found.start].chars().count();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::{Action, Focus};

    /// One file, one hunk, a line per entry; rows 2.. are the lines.
    fn lines_app(lines: &[&str]) -> App {
        let mut f = file("a.rs", &[lines.len()]);
        for (line, text) in f.hunks[0].lines.iter_mut().zip(lines) {
            line.content = text.to_string();
        }
        let mut app = app_with(vec![f]);
        app.cursor = 2;
        app
    }

    fn at(app: &App) -> (usize, Option<usize>) {
        (app.cursor, app.cursor_column())
    }

    #[test]
    fn words_split_on_punctuation_and_words_on_blanks() {
        let mut app = lines_app(&["let total = items.iter().sum();"]);
        let mut stops = Vec::new();
        for _ in 0..6 {
            app.update(Action::WordForward { n: 1, big: false });
            stops.push(app.cursor_column().unwrap());
        }
        // total, =, items, ., iter, ().
        assert_eq!(stops, [4, 10, 12, 17, 18, 22]);
        app.update(Action::LineStart);
        app.update(Action::WordForward { n: 3, big: true });
        assert_eq!(app.cursor_column(), Some(12));
        app.update(Action::WordBackward { n: 1, big: false });
        assert_eq!(app.cursor_column(), Some(10));
        app.update(Action::WordEnd { n: 2, big: false });
        assert_eq!(app.cursor_column(), Some(17));
        app.update(Action::WordEnd { n: 1, big: true });
        assert_eq!(app.cursor_column(), Some(30));
    }

    #[test]
    fn word_motions_carry_on_across_lines() {
        let mut app = lines_app(&["fn a() {", "    b", "}"]);
        app.update(Action::LineEnd);
        app.update(Action::WordForward { n: 1, big: false });
        assert_eq!(at(&app), (3, Some(4)));
        app.update(Action::WordForward { n: 1, big: false });
        assert_eq!(at(&app), (4, Some(0)));
        app.update(Action::WordBackward { n: 2, big: false });
        assert_eq!(at(&app), (2, Some(7)));
        app.update(Action::WordEnd { n: 1, big: false });
        assert_eq!(at(&app), (3, Some(4)));
        // Nowhere further to go: stays put.
        app.update(Action::Bottom);
        app.update(Action::WordForward { n: 5, big: false });
        assert_eq!(at(&app), (4, Some(0)));
    }

    #[test]
    fn line_motions_and_a_wanted_column() {
        let mut app = lines_app(&["    let long = 1;", "x", "    let other = 2;"]);
        app.update(Action::FirstNonBlank);
        assert_eq!(app.cursor_column(), Some(4));
        app.update(Action::Right(5));
        assert_eq!(app.cursor_column(), Some(9));
        // A short line shows the cursor on its last character, and the
        // column comes back on the next long one.
        app.update(Action::Down(1));
        assert_eq!(app.cursor_column(), Some(0));
        app.update(Action::Down(1));
        assert_eq!(app.cursor_column(), Some(9));
        app.update(Action::LineEnd);
        assert_eq!(app.cursor_column(), Some(17));
        app.update(Action::Up(2));
        assert_eq!(app.cursor_column(), Some(16));
        app.update(Action::Left(100));
        assert_eq!(app.cursor_column(), Some(0));
        app.update(Action::Right(100));
        assert_eq!(app.cursor_column(), Some(16));
        app.update(Action::LineStart);
        assert_eq!(app.cursor_column(), Some(0));
    }

    #[test]
    fn find_and_till_a_character() {
        let mut app = lines_app(&["a(b, c(d), e)"]);
        let find = |target, forward, till| FindChar { target, forward, till };
        app.update(Action::FindChar { n: 2, find: find('(', true, false) });
        assert_eq!(app.cursor_column(), Some(6));
        app.update(Action::FindChar { n: 1, find: find(')', true, true) });
        assert_eq!(app.cursor_column(), Some(7));
        // `t` again doesn't get stuck on the same match.
        app.update(Action::FindChar { n: 1, find: find(')', true, true) });
        assert_eq!(app.cursor_column(), Some(11));
        app.update(Action::FindChar { n: 1, find: find('b', false, false) });
        assert_eq!(app.cursor_column(), Some(2));
        app.update(Action::FindChar { n: 1, find: find('z', true, false) });
        assert_eq!(app.cursor_column(), Some(2));
    }

    #[test]
    fn rows_without_code_have_no_column() {
        let mut app = lines_app(&["one", ""]);
        app.cursor = 0;
        assert_eq!(app.cursor_column(), None);
        assert_eq!(app.cursor_bytes(), None);
        app.update(Action::Right(1));
        assert_eq!(app.cursor_column(), None);
        // `w` from a header goes to the first code row.
        app.update(Action::WordForward { n: 1, big: false });
        assert_eq!(at(&app), (2, Some(0)));
        app.update(Action::Down(1));
        assert_eq!(app.cursor_bytes(), Some(0..0));
    }

    #[test]
    fn the_cursor_covers_a_whole_character() {
        let mut app = lines_app(&["é = ü"]);
        app.update(Action::LineEnd);
        assert_eq!(app.cursor_bytes(), Some(5..7));
    }

    #[test]
    fn search_puts_the_column_on_the_match() {
        let mut app = lines_app(&["one two", "three four"]);
        app.search = Some("four".into());
        app.update(Action::SearchNext(1));
        assert_eq!(at(&app), (3, Some(6)));
    }

    #[test]
    fn h_and_l_stay_out_of_the_sidebar() {
        let mut app = lines_app(&["abc"]);
        app.update(Action::ToggleFocus);
        assert_eq!(app.focus, Focus::Sidebar);
        app.update(Action::Right(1));
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(app.cursor_column(), Some(0));
    }
}
