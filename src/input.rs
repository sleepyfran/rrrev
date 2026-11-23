/// Editing commands for a text field, already decoded from keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    Insert(char),
    Newline,
    Backspace,
    Delete,
    Left,
    Right,
    /// Start / end of the current line.
    Home,
    End,
    DeleteWord,
    ClearToStart,
    Submit,
    Cancel,
    OpenEditor,
}

/// A small multi-line text field with a cursor, for comments and the `:` line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    text: String,
    /// Position in chars, not bytes.
    cursor: usize,
}

impl TextInput {
    /// A field holding `text`, with the cursor at the end.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.chars().count();
        Self { text, cursor }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Applies an editing action. Submit, Cancel and OpenEditor are for the
    /// field's owner to handle and do nothing here.
    pub fn apply(&mut self, action: InputAction) {
        match action {
            InputAction::Insert(c) => self.insert(c),
            InputAction::Newline => self.insert('\n'),
            InputAction::Backspace if self.cursor > 0 => self.remove(self.cursor - 1..self.cursor),
            InputAction::Delete if self.cursor < self.len() => {
                self.remove(self.cursor..self.cursor + 1)
            }
            InputAction::Left => self.cursor = self.cursor.saturating_sub(1),
            InputAction::Right => self.cursor = (self.cursor + 1).min(self.len()),
            InputAction::Home => self.cursor = self.line_start(),
            InputAction::End => {
                let rest = self.text.chars().skip(self.cursor);
                self.cursor += rest.take_while(|&c| c != '\n').count();
            }
            InputAction::DeleteWord => {
                let before: Vec<char> = self.text.chars().take(self.cursor).collect();
                let spaces = before
                    .iter()
                    .rev()
                    .take_while(|c| c.is_whitespace() && **c != '\n')
                    .count();
                let word = before[..before.len() - spaces]
                    .iter()
                    .rev()
                    .take_while(|c| !c.is_whitespace())
                    .count();
                self.remove(self.cursor - spaces - word..self.cursor);
            }
            InputAction::ClearToStart => self.remove(self.line_start()..self.cursor),
            _ => {}
        }
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    fn byte_index(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(i, _)| i)
    }

    fn line_start(&self) -> usize {
        let before: Vec<char> = self.text.chars().take(self.cursor).collect();
        before.iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1)
    }

    fn insert(&mut self, c: char) {
        let at = self.byte_index(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
    }

    /// Removes a char range and leaves the cursor at its start.
    fn remove(&mut self, range: std::ops::Range<usize>) {
        let (start, end) = (self.byte_index(range.start), self.byte_index(range.end));
        self.text.replace_range(start..end, "");
        self.cursor = range.start;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use InputAction::*;

    fn apply(input: &mut TextInput, actions: &[InputAction]) {
        for &action in actions {
            input.apply(action);
        }
    }

    fn typed(text: &str) -> Vec<InputAction> {
        text.chars().map(Insert).collect()
    }

    #[test]
    fn inserts_and_deletes_around_the_cursor() {
        let mut input = TextInput::default();
        apply(&mut input, &typed("héllo"));
        apply(
            &mut input,
            &[
                Left,
                Left,
                Backspace,
                Insert('L'),
                End,
                Backspace,
                Home,
                Delete,
            ],
        );
        assert_eq!(input.text(), "éLl");
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn deletes_words_and_lines() {
        let mut input = TextInput::new("fix this  ");
        input.apply(DeleteWord);
        assert_eq!(input.text(), "fix ");
        apply(&mut input, &[Newline]);
        apply(&mut input, &typed("second line"));
        input.apply(ClearToStart);
        assert_eq!(input.text(), "fix \n");
        input.apply(Backspace);
        input.apply(DeleteWord);
        assert_eq!(input.text(), "");
    }

    #[test]
    fn home_and_end_stay_on_the_current_line() {
        let mut input = TextInput::new("one\ntwo");
        input.apply(Home);
        assert_eq!(input.cursor(), 4);
        apply(&mut input, &[Left, Home]);
        assert_eq!(input.cursor(), 0);
        input.apply(End);
        assert_eq!(input.cursor(), 3);
    }
}
