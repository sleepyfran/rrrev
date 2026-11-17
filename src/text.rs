//! Text layout helpers shared by the app and the UI.

/// Greedy word wrap to `width` columns; a word longer than a line is split.
pub fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        while !word.is_empty() {
            let used = current.chars().count();
            let gap = usize::from(used > 0);
            if used + gap + word.len() <= width {
                if gap == 1 {
                    current.push(' ');
                }
                current.extend(word.drain(..));
            } else if used > 0 {
                lines.push(std::mem::take(&mut current));
            } else {
                current.extend(word.drain(..width));
                lines.push(std::mem::take(&mut current));
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Screen lines `len` columns of text take, hard-wrapped every `room`
/// columns; a `room` of 0 means no wrapping.
pub fn wrapped_height(len: usize, room: usize) -> usize {
    if room == 0 { 1 } else { len.div_ceil(room).max(1) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_words_to_the_width() {
        assert_eq!(wrap_words("Adds the organizer module", 12), ["Adds the", "organizer", "module"]);
        assert_eq!(wrap_words("tiny", 80), ["tiny"]);
        assert_eq!(wrap_words("  ", 10), Vec::<String>::new());
        assert_eq!(wrap_words("abcdefghij xy", 4), ["abcd", "efgh", "ij", "xy"]);
    }
}
