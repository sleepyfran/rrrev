//! The cursor's column on code rows (diff lines and context), moved with
//! vim's motions: `h` / `l`, `w` / `b` / `e` (and the WORD versions), `0` /
//! `^` / `$`, `f` / `F` / `t`. Other rows keep the whole-row cursor.
//!
//! `App::column` is the column the cursor wants, as vim's `curswant`: `j` /
//! `k` keep it, and a shorter line shows the cursor on its last character
//! until a longer one comes along. `$` wants the end of every line.

use std::ops::Range;

use super::{Action, App, Row};
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

/// Lines a cursor moves over: the rows of the diff, where only code rows
/// have text, or a whole file in the file view.
pub(super) trait CodeLines {
    fn count(&self) -> usize;
    /// The code on line `idx`, or `None` for a line without any (a header).
    fn code(&self, idx: usize) -> Option<&str>;
}

impl CodeLines for [String] {
    fn count(&self) -> usize {
        self.len()
    }

    fn code(&self, idx: usize) -> Option<&str> {
        self.get(idx).map(String::as_str)
    }
}

/// The diff's rows, through the app.
struct Rows<'a>(&'a App);

impl CodeLines for Rows<'_> {
    fn count(&self) -> usize {
        self.0.rows.len()
    }

    fn code(&self, idx: usize) -> Option<&str> {
        self.0.code_text(*self.0.rows.get(idx)?)
    }
}

/// A cursor with a column: the line it's on and the column it wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Caret {
    pub line: usize,
    pub column: usize,
}

impl Caret {
    /// The character the cursor is on, as an index into its line's code;
    /// `None` on lines without code. An empty line has its cursor at 0.
    pub(super) fn on(&self, lines: &(impl CodeLines + ?Sized)) -> Option<usize> {
        let text = lines.code(self.line)?;
        Some(self.column.min(text.chars().count().saturating_sub(1)))
    }

    /// The bytes of the character under the cursor; empty on an empty line.
    pub(super) fn bytes(&self, lines: &(impl CodeLines + ?Sized)) -> Option<Range<usize>> {
        let text = lines.code(self.line)?;
        let column = self.on(lines)?;
        Some(match text.char_indices().nth(column) {
            Some((start, c)) => start..start + c.len_utf8(),
            None => text.len()..text.len(),
        })
    }

    /// Carries out a motion along lines; anything else is left alone.
    pub(super) fn apply(&mut self, lines: &(impl CodeLines + ?Sized), action: Action) {
        match action {
            Action::Left(n) => self.shift(lines, -(n as isize)),
            Action::Right(n) => self.shift(lines, n as isize),
            Action::WordForward { n, big } => self.word_forward(lines, n, big),
            Action::WordBackward { n, big } => self.word_backward(lines, n, big),
            Action::WordEnd { n, big } => self.word_end(lines, n, big),
            Action::LineStart => self.line_start(lines, false),
            Action::FirstNonBlank => self.line_start(lines, true),
            Action::LineEnd => {
                if lines.code(self.line).is_some() {
                    self.column = usize::MAX;
                }
            }
            Action::FindChar { n, find } => self.find_char(lines, n, find),
            _ => {}
        }
    }

    fn chars(&self, lines: &(impl CodeLines + ?Sized)) -> Option<Vec<char>> {
        Some(lines.code(self.line)?.chars().collect())
    }

    /// The next (or previous) line with code.
    fn code_line(&self, lines: &(impl CodeLines + ?Sized), forward: bool) -> Option<usize> {
        let has_code = |&idx: &usize| lines.code(idx).is_some();
        if forward {
            (self.line + 1..lines.count()).find(has_code)
        } else {
            (0..self.line).rev().find(has_code)
        }
    }

    /// Moves to `line` with the column `pick` chooses on its characters.
    fn land(
        &mut self,
        lines: &(impl CodeLines + ?Sized),
        line: usize,
        pick: impl Fn(&[char]) -> Option<usize>,
    ) {
        self.line = line;
        let chars = self.chars(lines).unwrap_or_default();
        self.column = pick(&chars).unwrap_or(0);
    }

    /// `h` / `l`.
    fn shift(&mut self, lines: &(impl CodeLines + ?Sized), delta: isize) {
        let (Some(column), Some(chars)) = (self.on(lines), self.chars(lines)) else {
            return;
        };
        self.column = column
            .saturating_add_signed(delta)
            .min(chars.len().saturating_sub(1));
    }

    /// `0` / `^`.
    fn line_start(&mut self, lines: &(impl CodeLines + ?Sized), skip_blanks: bool) {
        if let Some(chars) = self.chars(lines) {
            self.column = if skip_blanks {
                first_non_blank(&chars)
            } else {
                0
            };
        }
    }

    /// `w` / `W`: the next word's start, carrying on to the next code line.
    fn word_forward(&mut self, lines: &(impl CodeLines + ?Sized), n: usize, big: bool) {
        for _ in 0..n {
            let here = self.chars(lines).zip(self.on(lines));
            if let Some(next) = here.and_then(|(chars, i)| next_word_start(&chars, i, big)) {
                self.column = next;
            } else if let Some(line) = self.code_line(lines, true) {
                // Like vim, stops at the start of an empty line.
                self.land(lines, line, |chars| Some(first_non_blank(chars)));
            } else {
                break;
            }
        }
    }

    /// `b` / `B`: the start of this word or the one before, carrying on to
    /// the previous code line.
    fn word_backward(&mut self, lines: &(impl CodeLines + ?Sized), n: usize, big: bool) {
        for _ in 0..n {
            let here = self.chars(lines).zip(self.on(lines));
            if let Some(prev) = here.and_then(|(chars, i)| prev_word_start(&chars, i, big)) {
                self.column = prev;
            } else if let Some(line) = self.code_line(lines, false) {
                self.land(lines, line, |chars| {
                    prev_word_start(chars, chars.len(), big)
                });
            } else {
                break;
            }
        }
    }

    /// `e` / `E`: the end of this word or the next, carrying on to the next code line.
    fn word_end(&mut self, lines: &(impl CodeLines + ?Sized), n: usize, big: bool) {
        for _ in 0..n {
            let here = self.chars(lines).zip(self.on(lines));
            if let Some(end) = here.and_then(|(chars, i)| word_end(&chars, Some(i), big)) {
                self.column = end;
            } else if let Some(line) = self.code_line(lines, true) {
                self.land(lines, line, |chars| word_end(chars, None, big));
            } else {
                break;
            }
        }
    }

    /// `f` / `F` / `t`: the `n`th `target` on the line; stays put if there
    /// aren't that many.
    fn find_char(&mut self, lines: &(impl CodeLines + ?Sized), n: usize, find: FindChar) {
        let (Some(chars), Some(column)) = (self.chars(lines), self.on(lines)) else {
            return;
        };
        let hits: Vec<usize> = if find.forward {
            // `t` right before a match would find it again; vim skips it.
            let from = column + 1 + usize::from(find.till);
            (from..chars.len())
                .filter(|&i| chars[i] == find.target)
                .collect()
        } else {
            (0..column)
                .rev()
                .filter(|&i| chars[i] == find.target)
                .collect()
        };
        if let Some(&hit) = hits.get(n.saturating_sub(1)) {
            self.column = if find.till { hit - 1 } else { hit };
        }
    }

    /// Puts the column on the first match of `query` on its line.
    pub(super) fn move_to_match(&mut self, lines: &(impl CodeLines + ?Sized), query: &str) {
        let Some(text) = lines.code(self.line) else {
            return;
        };
        if let Some(found) = find_matches(text, query).first() {
            self.column = text[..found.start].chars().count();
        }
    }
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

    fn caret(&self) -> Caret {
        Caret {
            line: self.cursor,
            column: self.column,
        }
    }

    /// The character the cursor is on, as an index into its row's code;
    /// `None` on rows without code.
    pub fn cursor_column(&self) -> Option<usize> {
        self.caret().on(&Rows(self))
    }

    /// The bytes of the code under the cursor, for the UI to draw it.
    pub fn cursor_bytes(&self) -> Option<Range<usize>> {
        self.caret().bytes(&Rows(self))
    }

    /// A motion along the line (`h`, `w`, `$`, `f`…) in the diff.
    pub(super) fn move_along(&mut self, action: Action) {
        let mut caret = self.caret();
        caret.apply(&Rows(self), action);
        (self.cursor, self.column) = (caret.line, caret.column);
    }

    /// After a search lands on a row, puts the column on the first match.
    pub(super) fn column_on_match(&mut self, query: &str) {
        let mut caret = self.caret();
        caret.move_to_match(&Rows(self), query);
        self.column = caret.column;
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::Focus;

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
        let find = |target, forward, till| FindChar {
            target,
            forward,
            till,
        };
        app.update(Action::FindChar {
            n: 2,
            find: find('(', true, false),
        });
        assert_eq!(app.cursor_column(), Some(6));
        app.update(Action::FindChar {
            n: 1,
            find: find(')', true, true),
        });
        assert_eq!(app.cursor_column(), Some(7));
        // `t` again doesn't get stuck on the same match.
        app.update(Action::FindChar {
            n: 1,
            find: find(')', true, true),
        });
        assert_eq!(app.cursor_column(), Some(11));
        app.update(Action::FindChar {
            n: 1,
            find: find('b', false, false),
        });
        assert_eq!(app.cursor_column(), Some(2));
        app.update(Action::FindChar {
            n: 1,
            find: find('z', true, false),
        });
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
