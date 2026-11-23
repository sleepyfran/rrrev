//! The walkthrough: asking the AI for one, laying it out as slides on the
//! diff, and moving through them. The deck covers the screen, explanation on
//! the left and the code it's about on the right.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::time::Instant;

use super::plan::Job;
use super::{App, Effect, Row, WalkthroughAction};
use crate::diff::{HunkId, LineKind};
use crate::organizer::OrganizeRequest;
use crate::walkthrough::Walkthrough;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlideKind {
    /// What the change does, and the steps ahead.
    Overview,
    Step,
    /// The plumbing left out, and anything the AI didn't place.
    LeftOut,
}

/// Some lines of a hunk, as a slide shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownHunk {
    pub file: usize,
    pub hunk: usize,
    /// Indices into the hunk's lines, in order.
    pub lines: Vec<usize>,
}

/// A file on the overview: which steps show it, and how many of its hunks
/// are plumbing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckFile {
    pub file: usize,
    /// 1-based slide numbers, as the deck counts them.
    pub slides: Vec<usize>,
    pub plumbing: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckSlide {
    pub kind: SlideKind,
    pub title: String,
    /// Markdown.
    pub body: String,
    pub code: Vec<ShownHunk>,
    /// What the overview lists on its code side.
    pub files: Vec<DeckFile>,
    /// What `r` marks: the code shown, and on the last slide the plumbing too.
    pub hunks: Vec<(usize, usize)>,
    /// Code it was about that's no longer in the diff.
    pub missing: usize,
}

/// The walkthrough on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deck {
    pub slides: Vec<DeckSlide>,
    pub slide: usize,
    /// First line shown of the code, and of the explanation.
    pub code_scroll: usize,
    pub text_scroll: usize,
    /// Set by the UI: how tall the code pane is, and the hunk at its top.
    pub code_height: usize,
    pub code_top: Option<(usize, usize)>,
}

impl Deck {
    pub fn current(&self) -> &DeckSlide {
        &self.slides[self.slide]
    }

    fn go(&mut self, slide: usize) {
        let slide = slide.min(self.slides.len() - 1);
        if slide != self.slide {
            self.slide = slide;
            self.code_scroll = 0;
            self.text_scroll = 0;
            self.code_top = None;
        }
    }
}

/// Lays `walkthrough` out on `diff` as slides: an overview, its steps, and
/// what it left out of `files`. Steps whose code has all left the diff and
/// say nothing are dropped.
pub(super) fn deck(walkthrough: &Walkthrough, app: &App, files: Range<usize>) -> Deck {
    let diff = &app.diff;
    let positions: HashMap<&HunkId, (usize, usize)> = files
        .clone()
        .flat_map(|f| {
            diff.files[f]
                .hunks
                .iter()
                .enumerate()
                .map(move |(h, hunk)| (&hunk.id, (f, h)))
        })
        .collect();

    let mut steps = Vec::new();
    for slide in &walkthrough.slides {
        let mut code = Vec::new();
        let mut missing = 0;
        for at in &slide.code {
            match positions.get(&at.hunk) {
                Some(&(file, hunk)) => code.push(ShownHunk {
                    file,
                    hunk,
                    lines: at.lines(&diff.files[file].hunks[hunk]),
                }),
                None => missing += 1,
            }
        }
        if code.is_empty() && slide.body.trim().is_empty() {
            continue;
        }
        let mut hunks: Vec<(usize, usize)> = code.iter().map(|c| (c.file, c.hunk)).collect();
        dedup_keeping_order(&mut hunks);
        steps.push(DeckSlide {
            kind: SlideKind::Step,
            title: slide.title.clone(),
            body: slide.body.clone(),
            code,
            files: Vec::new(),
            hunks,
            missing,
        });
    }

    let plumbing: Vec<(usize, usize)> = walkthrough
        .plumbing
        .iter()
        .filter_map(|id| positions.get(id).copied())
        .collect();
    let mentioned: HashSet<(usize, usize)> = steps
        .iter()
        .flat_map(|s| s.hunks.iter().copied())
        .chain(plumbing.iter().copied())
        .collect();
    let mut left: Vec<(usize, usize)> = positions
        .values()
        .copied()
        .filter(|p| !mentioned.contains(p))
        .collect();
    left.sort();

    // Which steps show each file, numbered as the deck counts slides: the
    // overview is 1.
    let mut by_file: BTreeMap<usize, DeckFile> = BTreeMap::new();
    for (i, step) in steps.iter().enumerate() {
        for &(file, _) in &step.hunks {
            let entry = by_file.entry(file).or_insert_with(|| DeckFile {
                file,
                slides: Vec::new(),
                plumbing: 0,
            });
            if entry.slides.last() != Some(&(i + 2)) {
                entry.slides.push(i + 2);
            }
        }
    }
    for &(file, _) in plumbing.iter().chain(&left) {
        by_file.entry(file).or_insert_with(|| DeckFile {
            file,
            slides: Vec::new(),
            plumbing: 0,
        });
    }
    for &(file, _) in &plumbing {
        if let Some(entry) = by_file.get_mut(&file) {
            entry.plumbing += 1;
        }
    }

    let mut overview = String::new();
    if !walkthrough.summary.is_empty() {
        overview += &walkthrough.summary;
        overview += "\n\n";
    }
    for (i, step) in steps.iter().enumerate() {
        overview += &format!("{}. {}\n", i + 2, step.title.trim());
    }
    match (plumbing.len(), left.len()) {
        (0, 0) => {}
        (p, l) => {
            let mut parts = Vec::new();
            if p > 0 {
                parts.push(format!("{} of plumbing", hunks(p)));
            }
            if l > 0 {
                parts.push(format!("{} it didn't place", hunks(l)));
            }
            overview += &format!(
                "\n{} left out: see the last slide.\n",
                capitalized(&parts.join(" and "))
            );
        }
    }

    let mut slides = vec![DeckSlide {
        kind: SlideKind::Overview,
        title: "Overview".to_string(),
        body: overview,
        code: Vec::new(),
        files: by_file.into_values().collect(),
        hunks: Vec::new(),
        missing: 0,
    }];
    slides.extend(steps);
    if !plumbing.is_empty() || !left.is_empty() {
        let describe = |&(file, hunk): &(usize, usize)| {
            let f = &diff.files[file];
            let h = &f.hunks[hunk];
            let count = |kind| h.lines.iter().filter(|l| l.kind == kind).count();
            let scope = diff
                .hunk_scope(file, hunk)
                .map_or(String::new(), |s| format!(" {s}"));
            format!(
                "- `{}:{}`{scope} (+{} −{})\n",
                f.path(),
                h.new_start,
                count(LineKind::Added),
                count(LineKind::Removed)
            )
        };
        let mut body = String::new();
        if !plumbing.is_empty() {
            body += "**Plumbing**, to take on trust:\n\n";
            body += &plumbing.iter().map(describe).collect::<String>();
        }
        if !left.is_empty() {
            if !body.is_empty() {
                body += "\n";
            }
            body += "**Not in the walkthrough**, shown here:\n\n";
            body += &left.iter().map(describe).collect::<String>();
        }
        body += "\nr marks all of it reviewed.";
        let mut hunks: Vec<(usize, usize)> = left.iter().chain(&plumbing).copied().collect();
        dedup_keeping_order(&mut hunks);
        slides.push(DeckSlide {
            kind: SlideKind::LeftOut,
            title: "Left out".to_string(),
            body,
            code: left
                .iter()
                .map(|&(file, hunk)| ShownHunk {
                    file,
                    hunk,
                    lines: (0..diff.files[file].hunks[hunk].lines.len()).collect(),
                })
                .collect(),
            files: Vec::new(),
            hunks,
            missing: 0,
        });
    }
    Deck {
        slides,
        slide: 0,
        code_scroll: 0,
        text_scroll: 0,
        code_height: 0,
        code_top: None,
    }
}

fn hunks(n: usize) -> String {
    format!("{n} hunk{}", if n == 1 { "" } else { "s" })
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map_or(String::new(), |c| c.to_uppercase().chain(chars).collect())
}

fn dedup_keeping_order(hunks: &mut Vec<(usize, usize)>) {
    let mut seen = HashSet::new();
    hunks.retain(|h| seen.insert(*h));
}

impl App {
    pub fn is_walking_through(&self) -> bool {
        self.walking.is_some()
    }

    /// The files the walkthrough is about: the layers of a stack it
    /// mentions hunks of, or the whole diff.
    fn walkthrough_files(&self, walkthrough: &Walkthrough) -> Range<usize> {
        if !self.is_stack() {
            return 0..self.diff.files.len();
        }
        let ids: HashSet<&HunkId> = walkthrough.hunks().collect();
        let layers: Vec<&Range<usize>> = self
            .diff
            .sections
            .iter()
            .map(|s| &s.files)
            .filter(|files| {
                self.hunk_ids((*files).clone())
                    .iter()
                    .any(|id| ids.contains(id))
            })
            .collect();
        match (layers.first(), layers.last()) {
            (Some(first), Some(last)) => first.start..last.end,
            _ => 0..0,
        }
    }

    /// `:walkthrough [new|cancel|clear]`.
    pub(super) fn walkthrough_command(&mut self, args: &[&str]) {
        match args {
            [] => self.open_or_start_walkthrough(),
            ["new"] => self.start_walkthrough(),
            ["cancel"] => self.cancel_walkthrough(),
            ["clear"] => {
                self.message = Some(if self.review.walkthrough().is_some() {
                    self.review.clear_walkthrough();
                    self.walkthrough = None;
                    "Walkthrough cleared".to_string()
                } else {
                    "No walkthrough to clear".to_string()
                })
            }
            _ => self.message = Some("error: usage: :walkthrough [new|cancel|clear]".to_string()),
        }
    }

    /// The saved walkthrough when it's about what's under the cursor (the
    /// layer, in a stack), else a new one.
    fn open_or_start_walkthrough(&mut self) {
        let Ok(layer) = self.target_layer() else {
            return;
        };
        let ids = self.hunk_ids(self.layer_files(layer));
        if self.review.walkthrough().is_some_and(|w| w.covers(&ids)) {
            self.open_walkthrough();
        } else {
            self.start_walkthrough();
        }
    }

    /// `gw`: the saved walkthrough, on the slide it was left at.
    pub(super) fn open_walkthrough(&mut self) {
        let Some(saved) = self.review.walkthrough() else {
            self.message = Some(if self.is_walking_through() {
                "Still writing the walkthrough… (:walkthrough cancel to stop)".to_string()
            } else {
                "No walkthrough yet: run :walkthrough".to_string()
            });
            return;
        };
        let files = self.walkthrough_files(saved);
        let mut deck = deck(saved, self, files);
        deck.go(self.walkthrough_slide);
        self.walkthrough = Some(deck);
    }

    /// Asks for a walkthrough in the background: of the layer under the
    /// cursor in a stack, of the whole diff otherwise.
    fn start_walkthrough(&mut self) {
        if self.is_walking_through() {
            self.message =
                Some("Already writing a walkthrough… (:walkthrough cancel to stop)".to_string());
            return;
        }
        let Ok(layer) = self.target_layer() else {
            return;
        };
        let files = self.layer_files(layer);
        if self.diff.files[files.clone()]
            .iter()
            .all(|f| f.hunks.is_empty())
        {
            self.message = Some("Nothing to walk through".to_string());
            return;
        }
        self.next_job += 1;
        let job = Job {
            id: self.next_job,
            started: Instant::now(),
        };
        let request =
            OrganizeRequest::from_files(&self.target_title(layer), &self.diff, files.clone());
        self.effect = Some(Effect::WalkThrough {
            job: job.id,
            request,
        });
        self.walking = Some(job);
        self.walking_files = files;
        self.message = Some(match layer {
            Some(l) => format!(
                "Writing a walkthrough of {}… it reads the code, so it takes a while",
                self.diff.sections[l].title
            ),
            None => "Writing a walkthrough… it reads the code, so it takes a while".to_string(),
        });
    }

    fn cancel_walkthrough(&mut self) {
        match self.walking.take() {
            Some(job) => {
                self.effect = Some(Effect::CancelWalkThrough { job: job.id });
                self.message = Some("Walkthrough cancelled".to_string());
            }
            None => self.message = Some("Not writing a walkthrough".to_string()),
        }
    }

    /// Takes the AI's walkthrough for `job`, saves it and opens it, unless
    /// something is being typed. Answers for cancelled runs are ignored.
    pub fn finish_walkthrough(&mut self, job: u64, result: anyhow::Result<Walkthrough>) {
        if self.walking.is_none_or(|current| current.id != job) {
            return;
        }
        self.walking = None;
        let files = std::mem::take(&mut self.walking_files);
        let walkthrough = match result {
            Ok(walkthrough) => walkthrough,
            Err(e) => {
                self.message = Some(format!("error: walkthrough failed: {e:#}"));
                return;
            }
        };
        let known = self.hunk_ids(files.clone());
        let walkthrough = walkthrough.sanitize(&known);
        if walkthrough.slides.is_empty() {
            self.message = Some("error: the walkthrough came back without slides".to_string());
            return;
        }
        let deck = deck(&walkthrough, self, files);
        let steps = deck
            .slides
            .iter()
            .filter(|s| s.kind == SlideKind::Step)
            .count();
        let plumbing = walkthrough.plumbing.len();
        self.review.set_walkthrough(walkthrough);
        self.walkthrough_slide = 0;
        let left = match plumbing {
            0 => String::new(),
            n => format!(", {} of plumbing left out", hunks(n)),
        };
        if self.is_typing() || self.walkthrough.is_some() {
            self.message = Some(format!(
                "Walkthrough ready: {steps} steps{left} · gw opens it"
            ));
            return;
        }
        self.walkthrough = Some(deck);
        self.message = Some(format!(
            "{steps} steps{left} · l / h next / previous · q close"
        ));
    }

    /// A key pressed while the walkthrough is open.
    pub fn walkthrough_action(&mut self, action: WalkthroughAction) {
        self.message = None;
        let Some(deck) = &mut self.walkthrough else {
            return;
        };
        let last = deck.slides.len() - 1;
        let half = (deck.code_height / 2).max(1);
        match action {
            WalkthroughAction::Next(n) => {
                if deck.slide == last {
                    self.message = Some("Last slide · q closes the walkthrough".to_string());
                }
                deck.go(deck.slide.saturating_add(n));
            }
            WalkthroughAction::Prev(n) => {
                if deck.slide == 0 {
                    self.message = Some("First slide".to_string());
                }
                deck.go(deck.slide.saturating_sub(n));
            }
            WalkthroughAction::First => deck.go(0),
            WalkthroughAction::Last => deck.go(last),
            WalkthroughAction::CodeDown(n) => deck.code_scroll = deck.code_scroll.saturating_add(n),
            WalkthroughAction::CodeUp(n) => deck.code_scroll = deck.code_scroll.saturating_sub(n),
            WalkthroughAction::CodeHalfDown => {
                deck.code_scroll = deck.code_scroll.saturating_add(half)
            }
            WalkthroughAction::CodeHalfUp => {
                deck.code_scroll = deck.code_scroll.saturating_sub(half)
            }
            WalkthroughAction::TextDown(n) => deck.text_scroll = deck.text_scroll.saturating_add(n),
            WalkthroughAction::TextUp(n) => deck.text_scroll = deck.text_scroll.saturating_sub(n),
            WalkthroughAction::Review => self.review_slide(),
            WalkthroughAction::Open => self.leave_walkthrough_for_code(),
            WalkthroughAction::Close => self.close_walkthrough(),
        }
    }

    fn close_walkthrough(&mut self) {
        if let Some(deck) = self.walkthrough.take() {
            self.walkthrough_slide = deck.slide;
        }
    }

    /// `r`: marks the slide's hunks reviewed, or unmarks them when they all are.
    fn review_slide(&mut self) {
        let Some(deck) = &self.walkthrough else {
            return;
        };
        let hunks = deck.current().hunks.clone();
        if hunks.is_empty() {
            self.message = Some("Nothing to mark on this slide".to_string());
            return;
        }
        self.toggle_hunks_reviewed(&hunks);
        let marked = hunks.iter().all(|&(f, h)| self.is_reviewed(f, h));
        self.message = Some(match (marked, hunks.len()) {
            (true, 1) => "Marked its hunk reviewed".to_string(),
            (true, n) => format!("Marked its {n} hunks reviewed"),
            (false, _) => "Unmarked".to_string(),
        });
    }

    /// `Enter`: closes the walkthrough on the hunk at the top of the code,
    /// in the diff, to read around it, comment on it or ask about it.
    fn leave_walkthrough_for_code(&mut self) {
        let Some(deck) = &self.walkthrough else {
            return;
        };
        let slide = deck.current();
        let target = deck
            .code_top
            .or_else(|| slide.code.first().map(|c| (c.file, c.hunk)))
            .or_else(|| {
                slide.files.first().and_then(|f| {
                    (!self.diff.files[f.file].hunks.is_empty()).then_some((f.file, 0))
                })
            });
        let Some((file, hunk)) = target else {
            self.message = Some("No code on this slide".to_string());
            return;
        };
        self.close_walkthrough();
        self.collapsed[file] = false;
        self.unfold_hunk(file, hunk);
        self.rebuild_rows();
        let stops = self.stops(|row| *row == Row::Hunk { file, hunk }, |_| false);
        if let Some(&(page, row)) = stops.first() {
            self.open_page(page, row);
            self.scroll = row;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::{Action, View};
    use crate::diff::Diff;
    use crate::review::ReviewState;
    use crate::walkthrough::{CodeRef, Slide};

    fn at(app: &App, file: usize, hunk: usize) -> CodeRef {
        CodeRef {
            hunk: app.diff.files[file].hunks[hunk].id.clone(),
            from: None,
            to: None,
        }
    }

    /// Two steps over the sample app: b.rs, then a.rs's second hunk; a.rs's
    /// first hunk is plumbing.
    fn sample(app: &App) -> Walkthrough {
        Walkthrough {
            summary: "Adds things.".into(),
            slides: vec![
                Slide {
                    title: "Types".into(),
                    body: "The types.".into(),
                    code: vec![at(app, 1, 0)],
                },
                Slide {
                    title: "Logic".into(),
                    body: "The logic.".into(),
                    code: vec![CodeRef {
                        from: Some(2),
                        to: Some(2),
                        ..at(app, 0, 1)
                    }],
                },
            ],
            plumbing: vec![at(app, 0, 0).hunk],
        }
    }

    fn walked_through_app() -> App {
        let mut app = sample_app();
        command(&mut app, "walkthrough");
        let Some(Effect::WalkThrough { job, request }) = app.effect.take() else {
            panic!("no walkthrough effect")
        };
        assert_eq!(request.hunks.len(), 3);
        assert!(app.is_walking_through());
        let walkthrough = sample(&app);
        app.finish_walkthrough(job, Ok(walkthrough));
        app
    }

    fn titles(app: &App) -> Vec<&str> {
        let deck = app.walkthrough.as_ref().unwrap();
        deck.slides.iter().map(|s| s.title.as_str()).collect()
    }

    #[test]
    fn a_walkthrough_opens_as_slides_and_is_saved() {
        let app = walked_through_app();
        assert!(!app.is_walking_through());
        assert_eq!(titles(&app), ["Overview", "Types", "Logic", "Left out"]);
        let deck = app.walkthrough.as_ref().unwrap();
        assert_eq!(deck.slides[0].kind, SlideKind::Overview);
        assert!(
            deck.slides[0]
                .body
                .starts_with("Adds things.\n\n2. Types\n3. Logic\n")
        );
        assert!(deck.slides[0].body.contains("1 hunk of plumbing left out"));
        // a.rs on slide 3 with a plumbing hunk, b.rs on slide 2.
        assert_eq!(
            deck.slides[0].files,
            [
                DeckFile {
                    file: 0,
                    slides: vec![3],
                    plumbing: 1
                },
                DeckFile {
                    file: 1,
                    slides: vec![2],
                    plumbing: 0
                },
            ]
        );
        // Only the second line of a.rs's second hunk.
        assert_eq!(
            deck.slides[2].code,
            [ShownHunk {
                file: 0,
                hunk: 1,
                lines: vec![1]
            }]
        );
        // The plumbing is listed, not shown, and r marks it.
        assert!(deck.slides[3].body.contains("`a.rs:1`"));
        assert!(deck.slides[3].code.is_empty());
        assert_eq!(deck.slides[3].hunks, [(0, 0)]);
        assert_eq!(app.review.walkthrough().unwrap().slides.len(), 2);
        assert_eq!(
            app.message.as_deref(),
            Some("2 steps, 1 hunk of plumbing left out · l / h next / previous · q close")
        );
    }

    #[test]
    fn slides_move_scroll_and_remember_where_they_were() {
        let mut app = walked_through_app();
        app.walkthrough_action(WalkthroughAction::Prev(1));
        assert_eq!(app.message.as_deref(), Some("First slide"));
        app.walkthrough_action(WalkthroughAction::Next(1));
        app.walkthrough_action(WalkthroughAction::CodeDown(3));
        assert_eq!(app.walkthrough.as_ref().unwrap().code_scroll, 3);
        // A new slide starts from its top.
        app.walkthrough_action(WalkthroughAction::Next(1));
        let deck = app.walkthrough.as_ref().unwrap();
        assert_eq!((deck.slide, deck.code_scroll), (2, 0));
        app.walkthrough_action(WalkthroughAction::Last);
        app.walkthrough_action(WalkthroughAction::Next(1));
        assert_eq!(app.walkthrough.as_ref().unwrap().slide, 3);
        assert!(app.message.as_deref().unwrap().starts_with("Last slide"));

        app.walkthrough_action(WalkthroughAction::Close);
        assert!(app.walkthrough.is_none());
        app.update(Action::OpenWalkthrough);
        assert_eq!(app.walkthrough.as_ref().unwrap().slide, 3);
        // :walkthrough opens the saved one rather than asking again.
        app.walkthrough_action(WalkthroughAction::Close);
        command(&mut app, "walkthrough");
        assert!(app.walkthrough.is_some());
        assert_eq!(app.effect, None);
    }

    #[test]
    fn r_marks_the_slide_and_enter_goes_to_its_code() {
        let mut app = walked_through_app();
        app.walkthrough_action(WalkthroughAction::Next(1));
        app.walkthrough_action(WalkthroughAction::Review);
        assert!(app.is_reviewed(1, 0));
        assert_eq!(app.message.as_deref(), Some("Marked its hunk reviewed"));
        app.walkthrough_action(WalkthroughAction::Review);
        assert!(!app.is_reviewed(1, 0));

        app.walkthrough_action(WalkthroughAction::Next(1));
        app.walkthrough_action(WalkthroughAction::Open);
        assert!(app.walkthrough.is_none());
        assert_eq!(app.view, View::Files);
        assert_eq!(app.current_row(), Some(Row::Hunk { file: 0, hunk: 1 }));
    }

    #[test]
    fn while_open_only_help_and_closing_get_through() {
        let mut app = walked_through_app();
        app.update(Action::NextFile(1));
        assert_eq!(app.page, 0);
        app.update(Action::ToggleHelp);
        assert_eq!(app.help_scroll, Some(0));
        app.update(Action::Cancel);
        app.update(Action::Quit);
        assert!(app.walkthrough.is_none());
        assert!(!app.should_quit);
    }

    #[test]
    fn hunks_it_did_not_place_are_shown_on_the_last_slide() {
        let mut app = sample_app();
        command(&mut app, "walkthrough");
        let Some(Effect::WalkThrough { job, .. }) = app.effect.take() else {
            panic!()
        };
        let mut walkthrough = sample(&app);
        walkthrough.plumbing.clear();
        app.finish_walkthrough(job, Ok(walkthrough));
        let deck = app.walkthrough.as_ref().unwrap();
        let last = deck.slides.last().unwrap();
        assert_eq!(last.kind, SlideKind::LeftOut);
        assert_eq!(last.code.len(), 1);
        assert_eq!((last.code[0].file, last.code[0].hunk), (0, 0));
        assert!(last.body.contains("Not in the walkthrough"));
        assert!(deck.slides[0].body.contains("1 hunk it didn't place"));
    }

    #[test]
    fn commands_cancel_clear_and_failures() {
        let mut app = sample_app();
        app.update(Action::OpenWalkthrough);
        assert_eq!(
            app.message.as_deref(),
            Some("No walkthrough yet: run :walkthrough")
        );
        command(&mut app, "walkthrough");
        let Some(Effect::WalkThrough { job: first, .. }) = app.effect.take() else {
            panic!()
        };
        command(&mut app, "walkthrough new");
        assert!(
            app.message
                .as_deref()
                .unwrap()
                .starts_with("Already writing")
        );
        command(&mut app, "walkthrough cancel");
        assert_eq!(
            app.effect.take(),
            Some(Effect::CancelWalkThrough { job: first })
        );
        // The cancelled run's answer changes nothing.
        let walkthrough = sample(&app);
        app.finish_walkthrough(first, Ok(walkthrough));
        assert!(app.walkthrough.is_none());
        assert!(app.review.walkthrough().is_none());

        command(&mut app, "walkthrough new");
        let Some(Effect::WalkThrough { job, .. }) = app.effect.take() else {
            panic!()
        };
        app.finish_walkthrough(job, Err(anyhow::anyhow!("no network")));
        assert_eq!(
            app.message.as_deref(),
            Some("error: walkthrough failed: no network")
        );

        command(&mut app, "walkthrough clear");
        assert_eq!(app.message.as_deref(), Some("No walkthrough to clear"));
        command(&mut app, "walkthrough sideways");
        assert!(app.message.as_deref().unwrap().starts_with("error: usage"));
    }

    #[test]
    fn a_saved_walkthrough_reopens_in_a_new_session() {
        let app = walked_through_app();
        let mut reopened = App::new(String::new(), app.diff.clone(), app.review);
        assert!(reopened.walkthrough.is_none());
        reopened.update(Action::OpenWalkthrough);
        assert_eq!(reopened.walkthrough.as_ref().unwrap().slides.len(), 4);
    }

    #[test]
    fn in_a_stack_it_walks_through_the_layer_under_the_cursor() {
        let diff = Diff::sectioned(vec![
            (
                "#1 types".into(),
                "Layer 1 of 2.".into(),
                vec![file("a.rs", &[3])],
            ),
            (
                "#2 logic".into(),
                "Layer 2 of 2.".into(),
                vec![file("b.rs", &[2, 1])],
            ),
        ]);
        let mut app = App::new(String::new(), diff, ReviewState::default());
        app.update(Action::NextLayer(1));
        command(&mut app, "walkthrough");
        let Some(Effect::WalkThrough { job, request }) = app.effect.take() else {
            panic!()
        };
        assert_eq!(request.hunks.len(), 2);
        assert!(request.title.contains("layer 2/2: #2 logic"));
        let walkthrough = Walkthrough {
            summary: String::new(),
            slides: vec![Slide {
                title: "Logic".into(),
                body: "It.".into(),
                // a.rs is another layer's: dropped.
                code: vec![at(&app, 1, 1), at(&app, 0, 0)],
            }],
            plumbing: Vec::new(),
        };
        app.finish_walkthrough(job, Ok(walkthrough));
        let deck = app.walkthrough.as_ref().unwrap();
        assert_eq!(deck.slides[1].code.len(), 1);
        // b.rs's first hunk is the one left out; nothing of layer 1.
        assert_eq!(deck.slides.last().unwrap().code[0].file, 1);
        assert_eq!(deck.slides.last().unwrap().code[0].hunk, 0);
    }
}
