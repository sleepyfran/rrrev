//! Word-level changes inside changed lines: which tokens of a removed line
//! didn't survive into the added line that replaced it, and the other way
//! round. Lines read as a whole tell you *that* something changed; these
//! ranges tell you *what*.

use std::ops::Range;

use super::{DiffLine, LineKind};

/// Lines longer than this, in tokens, are left without word changes:
/// comparing them is quadratic and the result is rarely readable anyway.
const MAX_TOKENS: usize = 200;

/// Below this share of unchanged text, a pair of lines is a rewrite rather
/// than an edit, and picking out "changes" would only light up everything.
const MIN_SIMILARITY: f32 = 0.35;

/// For each line of a hunk, the byte ranges of its content that changed.
/// Empty for context lines, and for changed lines without a counterpart
/// (pure additions or removals) or whose counterpart is too different.
pub fn changes(lines: &[DiffLine]) -> Vec<Vec<Range<usize>>> {
    let mut out = vec![Vec::new(); lines.len()];
    let mut i = 0;
    while i < lines.len() {
        // A run of removals followed by a run of additions: an edit.
        let removed_start = i;
        while i < lines.len() && lines[i].kind == LineKind::Removed {
            i += 1;
        }
        let added_start = i;
        while i < lines.len() && lines[i].kind == LineKind::Added {
            i += 1;
        }
        if removed_start == added_start || added_start == i {
            i = i.max(removed_start + 1);
            continue;
        }
        for (old, new, (a, b)) in pair_up(lines, removed_start..added_start, added_start..i) {
            out[old] = a;
            out[new] = b;
        }
    }
    out
}

/// Runs longer than this on both sides together are paired in order, as
/// trying every pair would get slow.
const MAX_MATCHED_RUN: usize = 64;

/// Which removed line became which added one, with their changes. Runs of
/// the same length pair in order; otherwise each removed line takes the
/// most similar added line after the last one taken, so a line inserted in
/// between doesn't shift every pair after it.
fn pair_up(lines: &[DiffLine], removed: Range<usize>, added: Range<usize>) -> Vec<(usize, usize, PairChanges)> {
    let compare = |old: usize, new: usize| diff_pair(&lines[old].content, &lines[new].content);
    if removed.len() == added.len() || removed.len() + added.len() > MAX_MATCHED_RUN {
        return removed.zip(added).filter_map(|(old, new)| Some((old, new, compare(old, new)?.1))).collect();
    }
    let mut out = Vec::new();
    let mut next = added.start;
    for old in removed {
        let best = (next..added.end)
            .filter_map(|new| compare(old, new).map(|(score, changes)| (new, score, changes)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((new, _, changes)) = best {
            out.push((old, new, changes));
            next = new + 1;
        }
    }
    out
}

/// Changed byte ranges of a removed line and the added line that replaced it.
type PairChanges = (Vec<Range<usize>>, Vec<Range<usize>>);

/// How alike `old` and `new` are (0 to 1) and their changed byte ranges,
/// or `None` when they're too different (or too long) to compare word by word.
fn diff_pair(old: &str, new: &str) -> Option<(f32, PairChanges)> {
    let (a, b) = (tokens(old), tokens(new));
    if a.len() > MAX_TOKENS || b.len() > MAX_TOKENS {
        return None;
    }
    fn text<'a>(s: &'a str, toks: &[Range<usize>], i: usize) -> &'a str {
        &s[toks[i].clone()]
    }
    // Longest common subsequence of tokens, by dynamic programming.
    let (n, m) = (a.len(), b.len());
    let mut table = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if text(old, &a, i) == text(new, &b, j) {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let (mut keep_a, mut keep_b) = (vec![false; n], vec![false; m]);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if text(old, &a, i) == text(new, &b, j) {
            keep_a[i] = true;
            keep_b[j] = true;
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }

    // Whitespace that survived doesn't make a pair similar.
    let kept_len = |s: &str, toks: &[Range<usize>], keep: &[bool]| -> usize {
        toks.iter().zip(keep).filter(|(t, k)| **k && !s[(*t).clone()].trim().is_empty()).map(|(t, _)| t.len()).sum()
    };
    let solid_len = |s: &str| s.chars().filter(|c| !c.is_whitespace()).map(char::len_utf8).sum::<usize>();
    let total = solid_len(old) + solid_len(new);
    if total == 0 {
        return None;
    }
    let similarity = (kept_len(old, &a, &keep_a) + kept_len(new, &b, &keep_b)) as f32 / total as f32;
    if similarity < MIN_SIMILARITY {
        return None;
    }
    Some((similarity, (ranges(old, &a, &keep_a), ranges(new, &b, &keep_b))))
}

/// Merges the tokens not kept into ranges. Only whitespace between two
/// changed tokens joins them, so `foo bar` → `baz qux` lights up as one run;
/// changed whitespace on its own (indentation, padding) isn't shown.
fn ranges(s: &str, toks: &[Range<usize>], keep: &[bool]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for (tok, _) in toks.iter().zip(keep).filter(|(_, kept)| !**kept) {
        if s[tok.clone()].trim().is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if s[last.end..tok.start].trim().is_empty() => last.end = tok.end,
            _ => out.push(tok.clone()),
        }
    }
    out
}

/// Splits a line into words (letters, digits, `_`), runs of whitespace, and
/// single other characters, as byte ranges.
fn tokens(s: &str) -> Vec<Range<usize>> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut previous: Option<Class> = None;
    for (i, c) in s.char_indices() {
        let current = class(c);
        let extends = current != Class::Other && previous.as_ref() == Some(&current);
        match out.last_mut() {
            Some(last) if extends => last.end = i + c.len_utf8(),
            _ => out.push(i..i + c.len_utf8()),
        }
        previous = Some(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: LineKind, content: &str) -> DiffLine {
        DiffLine { kind, old_no: None, new_no: None, content: content.to_string() }
    }

    fn changed<'a>(content: &'a str, ranges: &[Range<usize>]) -> Vec<&'a str> {
        ranges.iter().map(|r| &content[r.clone()]).collect()
    }

    #[test]
    fn picks_out_the_tokens_that_changed() {
        let old = "static PATH: Lazy<RwLock<PathCache>> = Lazy::new(|| {";
        let new = "static PATH: LazyLock<RwLock<PathCache>> = LazyLock::new(|| {";
        let lines = [line(LineKind::Removed, old), line(LineKind::Added, new)];
        let out = changes(&lines);
        assert_eq!(changed(old, &out[0]), ["Lazy", "Lazy"]);
        assert_eq!(changed(new, &out[1]), ["LazyLock", "LazyLock"]);
    }

    #[test]
    fn joins_changed_words_across_spaces() {
        let old = "    finderPromise = (async () => {";
        let new = "    const pending = (async () => {";
        let out = changes(&[line(LineKind::Removed, old), line(LineKind::Added, new)]);
        assert_eq!(changed(old, &out[0]), ["finderPromise"]);
        assert_eq!(changed(new, &out[1]), ["const pending"]);
    }

    #[test]
    fn pairs_runs_in_order_and_leaves_extras_whole() {
        let lines = [
            line(LineKind::Context, "fn a() {"),
            line(LineKind::Removed, "let x = 1;"),
            line(LineKind::Added, "let x = 2;"),
            line(LineKind::Added, "let y = 3;"),
            line(LineKind::Context, "}"),
        ];
        let out = changes(&lines);
        assert!(out[0].is_empty() && out[4].is_empty());
        assert_eq!(changed("let x = 1;", &out[1]), ["1"]);
        assert_eq!(changed("let x = 2;", &out[2]), ["2"]);
        // A pure addition has nothing to compare against.
        assert!(out[3].is_empty());
    }

    #[test]
    fn a_line_inserted_before_the_edit_doesnt_shift_the_pairing() {
        let old = "Diff { files, sections: Vec::new() }";
        let new = "Diff { files, sections: Vec::new(), sources: Vec::new(), words }";
        let lines = [
            line(LineKind::Removed, old),
            line(LineKind::Added, "let words = files.iter().map(changes).collect();"),
            line(LineKind::Added, new),
        ];
        let out = changes(&lines);
        assert!(out[1].is_empty());
        assert_eq!(changed(new, &out[2]), [", sources: Vec::new(), words"]);
        assert_eq!(changed(old, &out[0]), Vec::<&str>::new());
    }

    #[test]
    fn rewrites_are_not_word_diffed() {
        let out = changes(&[line(LineKind::Removed, "return mainFinder;"), line(LineKind::Added, "}).catch(err => log(err));")]);
        assert!(out.iter().all(Vec::is_empty));
    }

    #[test]
    fn indentation_changes_alone_are_ignored() {
        let out = changes(&[line(LineKind::Removed, "  foo(bar);"), line(LineKind::Added, "    foo(bar);")]);
        assert!(out.iter().all(Vec::is_empty));
    }

    #[test]
    fn handles_multibyte_text() {
        let (old, new) = ("let s = \"héllo\";", "let s = \"héllø\";");
        let out = changes(&[line(LineKind::Removed, old), line(LineKind::Added, new)]);
        assert_eq!(changed(old, &out[0]), ["héllo"]);
        assert_eq!(changed(new, &out[1]), ["héllø"]);
    }
}
