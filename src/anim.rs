//! Short transitions drawn over each frame. The app never hears of them: the
//! animator compares what's on screen with the last frame to see what
//! changed, then reworks the drawn buffer for the next few frames. A
//! transition never holds up a key, and the app's state stays the only truth.
//!
//! `RRREV_ANIMATIONS=off` turns them all off.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use ratatui::{
    buffer::{Buffer, Cell},
    layout::Rect,
    style::Color,
};

use crate::app::{App, Row, View};

/// How often to redraw while something moves.
pub const FRAME: Duration = Duration::from_millis(16);

const POPUP: Duration = Duration::from_millis(140);
const SLIDE: Duration = Duration::from_millis(160);
const SCROLL: Duration = Duration::from_millis(140);
const GLINT: Duration = Duration::from_millis(450);
/// How many columns a new page slides in from.
const SLIDE_COLUMNS: f32 = 8.0;
/// Shorter scrolls snap, so stepping with `j` / `k` stays crisp.
const MIN_SMOOTH_SCROLL: f32 = 3.0;
/// Width of the band that sweeps across what was just marked reviewed.
const GLINT_WIDTH: u16 = 6;

/// Fast at first, settling gently.
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

#[derive(Debug, Clone, Copy)]
struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
}

impl Tween {
    fn new(from: f32, to: f32, duration: Duration, now: Instant) -> Self {
        Self { from, to, start: now, duration }
    }

    fn progress(&self, now: Instant) -> f32 {
        progress(self.start, self.duration, now)
    }

    fn value(&self, now: Instant) -> f32 {
        self.from + (self.to - self.from) * ease_out(self.progress(now))
    }

    fn done(&self, now: Instant) -> bool {
        self.progress(now) >= 1.0
    }
}

fn progress(start: Instant, duration: Duration, now: Instant) -> f32 {
    (now.saturating_duration_since(start).as_secs_f32() / duration.as_secs_f32()).min(1.0)
}

/// The pop-ups that unroll open and roll shut.
#[derive(Debug, Clone, Copy)]
pub enum Popup {
    Help,
    Sidebar,
    Comment,
    Verdict,
}

impl Popup {
    /// The comment box rises from the status line; the others open from
    /// their middle.
    fn anchor(self) -> Anchor {
        match self {
            Popup::Comment => Anchor::Bottom,
            Popup::Help | Popup::Sidebar | Popup::Verdict => Anchor::Center,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Anchor {
    Center,
    Bottom,
}

#[derive(Default)]
struct PopupState {
    open: bool,
    /// How open it is, 0 to 1, while that's changing.
    tween: Option<Tween>,
    /// The pop-up as last drawn, to roll shut once the app has closed it.
    last: Option<Buffer>,
}

impl PopupState {
    fn openness(&self, now: Instant) -> f32 {
        self.tween.map_or(if self.open { 1.0 } else { 0.0 }, |t| t.value(now))
    }
}

/// Where a part-open pop-up's rows are shown: its top and bottom rows, with
/// as many rows from the top of its content between them as fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unroll {
    full: Rect,
    top: u16,
    height: u16,
}

impl Unroll {
    fn new(full: Rect, anchor: Anchor, openness: f32) -> Self {
        let height = ((full.height as f32 * openness).round() as u16).clamp(full.height.min(2), full.height);
        let top = match anchor {
            Anchor::Center => full.y + (full.height - height) / 2,
            Anchor::Bottom => full.bottom() - height,
        };
        Self { full, top, height }
    }

    /// Where row `y` of the open pop-up is shown now, if it is.
    pub fn row(&self, y: u16) -> Option<u16> {
        let i = y.checked_sub(self.full.y)?;
        if i + 1 == self.full.height {
            Some(self.top + self.height - 1)
        } else if i + 1 < self.height {
            Some(self.top + i)
        } else {
            None
        }
    }
}

/// A sweep across what was just marked reviewed.
struct Glint {
    hunks: HashSet<(usize, usize)>,
    start: Instant,
}

pub struct Animator {
    enabled: bool,
    now: Instant,
    /// Whether anything moved this frame, so the next one comes soon.
    busy: bool,
    popups: [PopupState; 4],
    page: Option<(View, usize)>,
    /// Columns the diff is pushed aside by, while a new page slides in.
    slide: Option<Tween>,
    /// The page and row count the scroll belongs to; a scroll on another
    /// page, or after rows fold away, can't be eased from the old one.
    scroll_rows: Option<(View, usize, usize)>,
    scroll_target: usize,
    scroll: Option<Tween>,
    /// Per file, per hunk: reviewed as of the last frame.
    reviewed: Option<Vec<Vec<bool>>>,
    glints: Vec<Glint>,
    /// When the whole review last became complete.
    completed: Option<Instant>,
}

impl Animator {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            now: Instant::now(),
            busy: false,
            popups: Default::default(),
            page: None,
            slide: None,
            scroll_rows: None,
            scroll_target: 0,
            scroll: None,
            reviewed: None,
            glints: Vec::new(),
            completed: None,
        }
    }

    pub fn from_env() -> Self {
        let off = std::env::var("RRREV_ANIMATIONS").is_ok_and(|v| matches!(v.as_str(), "off" | "0" | "false"));
        Self::new(!off)
    }

    /// Whether to draw the next frame soon rather than wait for a key.
    pub fn is_animating(&self) -> bool {
        self.busy
    }

    /// Starts a frame: notes what changed in `app` since the last one.
    pub fn begin(&mut self, app: &App) {
        self.now = Instant::now();
        self.busy = false;
        if !self.enabled {
            return;
        }

        let page = (app.view, app.page);
        if let Some(old) = self.page.replace(page)
            && old != page
        {
            // Onward comes in from the right, back from the left.
            let from = if old.0 == page.0 && page.1 < old.1 { -SLIDE_COLUMNS } else { SLIDE_COLUMNS };
            self.slide = Some(Tween::new(from, 0.0, SLIDE, self.now));
        }

        let reviewed: Vec<Vec<bool>> = app
            .diff
            .files
            .iter()
            .enumerate()
            .map(|(f, file)| (0..file.hunks.len()).map(|h| app.is_reviewed(f, h)).collect())
            .collect();
        if let Some(old) = self.reviewed.replace(reviewed) {
            let new = self.reviewed.as_ref().expect("just set");
            let hunks: HashSet<_> = new
                .iter()
                .zip(&old)
                .enumerate()
                .flat_map(|(f, (now, was))| {
                    now.iter().zip(was).enumerate().filter(|(_, (n, w))| **n && !**w).map(move |(h, _)| (f, h))
                })
                .collect();
            if !hunks.is_empty() {
                self.glints.push(Glint { hunks, start: self.now });
                if app.progress().is_complete() {
                    self.completed = Some(self.now);
                }
            }
        }
        let now = self.now;
        self.glints.retain(|g| progress(g.start, GLINT, now) < 1.0);
        self.completed = self.completed.filter(|&start| progress(start, GLINT, now) < 1.0);
        self.busy |= !self.glints.is_empty() || self.completed.is_some();
    }

    /// The first row to draw: `app.scroll`, or on the way there after a
    /// long jump.
    pub fn scroll(&mut self, app: &App) -> usize {
        let target = app.scroll;
        if !self.enabled {
            return target;
        }
        let rows = (app.view, app.page, app.rows.len());
        let moving = self.scroll.filter(|t| !t.done(self.now));
        let shown = moving.map_or(self.scroll_target as f32, |t| t.value(self.now));
        if self.scroll_rows.replace(rows) != Some(rows) {
            self.scroll = None;
        } else if target != self.scroll_target {
            let far = (target as f32 - shown).abs() >= MIN_SMOOTH_SCROLL;
            self.scroll = (far || moving.is_some()).then(|| Tween::new(shown, target as f32, SCROLL, self.now));
        }
        self.scroll_target = target;
        match self.scroll.filter(|t| !t.done(self.now)) {
            Some(t) => {
                self.busy = true;
                (t.value(self.now).round() as usize).min(app.rows.len().saturating_sub(1))
            }
            None => {
                self.scroll = None;
                target
            }
        }
    }

    /// Slides the diff drawn in `area` in from the side after a page change.
    pub fn slide(&mut self, buf: &mut Buffer, area: Rect) {
        let Some(tween) = self.slide else { return };
        if tween.done(self.now) {
            self.slide = None;
            return;
        }
        self.busy = true;
        shift(buf, area, tween.value(self.now).round() as i32);
    }

    /// Unrolls a pop-up as it opens and rolls it shut once closed. `drawn` is
    /// where it was drawn this frame, none once the app has closed it, and
    /// `under` the frame from before it was drawn (blank if none). Returns
    /// where its rows are shown while it moves.
    pub fn popup(&mut self, buf: &mut Buffer, popup: Popup, drawn: Option<Rect>, under: Option<&Buffer>) -> Option<Unroll> {
        if !self.enabled {
            return None;
        }
        let now = self.now;
        let state = &mut self.popups[popup as usize];
        let openness = state.openness(now);
        match drawn {
            Some(rect) => {
                if !state.open {
                    state.open = true;
                    state.tween = Some(Tween::new(openness, 1.0, POPUP, now));
                }
                state.last = Some(copy(buf, rect));
            }
            None if state.open => {
                state.open = false;
                state.tween = Some(Tween::new(openness, 0.0, POPUP, now));
            }
            None => {}
        }
        let Some(tween) = state.tween.filter(|t| !t.done(now)) else {
            state.tween = None;
            if !state.open {
                state.last = None;
            }
            return None;
        };
        let full = state.last.as_ref()?;
        let unroll = Unroll::new(full.area, popup.anchor(), tween.value(now));
        // Opening, rows not shown yet must show what's underneath; closing,
        // the frame was drawn without the pop-up already.
        if drawn.is_some() {
            restore(buf, full.area, under);
        }
        draw_unrolled(buf, full, unroll);
        self.busy = true;
        Some(unroll)
    }

    /// Sweeps across diff rows of hunks just marked reviewed (or their file's
    /// header, when that folded them away).
    pub fn glint_rows(&self, buf: &mut Buffer, rows: &[(Row, Rect)]) {
        for glint in &self.glints {
            let t = progress(glint.start, GLINT, self.now);
            for &(row, rect) in rows {
                let hit = match row {
                    Row::File { file } => glint.hunks.iter().any(|&(f, _)| f == file),
                    Row::Hunk { file, hunk } | Row::Line { file, hunk, .. } | Row::Context { file, hunk, .. } => {
                        glint.hunks.contains(&(file, hunk))
                    }
                    Row::Group { .. } | Row::Comment { .. } | Row::Note { .. } => false,
                };
                if hit {
                    sweep(buf, rect, t);
                }
            }
        }
    }

    /// Sweeps across sidebar entries (files, or plan groups) holding hunks
    /// just marked reviewed.
    pub fn glint_sidebar(&self, buf: &mut Buffer, app: &App, entries: &[(usize, Rect)]) {
        for glint in &self.glints {
            let t = progress(glint.start, GLINT, self.now);
            for &(idx, rect) in entries {
                let hit = match app.view {
                    View::Files => glint.hunks.iter().any(|&(f, _)| f == idx),
                    View::Plan | View::Stack => app.pages()[idx].hunks.iter().any(|h| glint.hunks.contains(h)),
                };
                if hit {
                    sweep(buf, rect, t);
                }
            }
        }
    }

    /// Sweeps across the title bar when the last hunk gets reviewed.
    pub fn glint_title(&self, buf: &mut Buffer, area: Rect) {
        if let Some(start) = self.completed {
            sweep(buf, area, progress(start, GLINT, self.now));
        }
    }
}

/// A copy of what `buf` holds in `area`.
fn copy(buf: &Buffer, area: Rect) -> Buffer {
    let area = area.intersection(buf.area);
    let mut out = Buffer::empty(area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            out[(x, y)] = buf[(x, y)].clone();
        }
    }
    out
}

/// Puts back what `under` holds in `area`, or blanks it.
fn restore(buf: &mut Buffer, area: Rect, under: Option<&Buffer>) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                *cell = under.and_then(|u| u.cell((x, y))).cloned().unwrap_or_default();
            }
        }
    }
}

fn draw_unrolled(buf: &mut Buffer, popup: &Buffer, unroll: Unroll) {
    let area = popup.area;
    for y in area.top()..area.bottom() {
        let Some(to) = unroll.row(y) else { continue };
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, to)) {
                *cell = popup[(x, y)].clone();
            }
        }
    }
}

/// Moves what's drawn in `area` right by `by` columns (left when negative),
/// blanking the columns it uncovers.
fn shift(buf: &mut Buffer, area: Rect, by: i32) {
    if by == 0 {
        return;
    }
    for y in area.top()..area.bottom() {
        let row: Vec<Cell> = (area.left()..area.right()).map(|x| buf[(x, y)].clone()).collect();
        for (i, x) in (area.left()..area.right()).enumerate() {
            let from = usize::try_from(i as i32 - by).ok().and_then(|f| row.get(f));
            buf[(x, y)] = from.cloned().unwrap_or_default();
        }
    }
}

/// A green band crossing `area` left to right as `t` goes from 0 to 1.
fn sweep(buf: &mut Buffer, area: Rect, t: f32) {
    let travel = (area.width + GLINT_WIDTH) as f32;
    let end = area.x as i32 + (travel * t) as i32;
    let start = (end - GLINT_WIDTH as i32).max(area.x as i32);
    for y in area.top()..area.bottom() {
        for x in start..end.min(area.right() as i32) {
            if let Some(cell) = buf.cell_mut((x as u16, y)) {
                cell.set_bg(Color::Green).set_fg(Color::Black);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (area.top()..area.bottom())
            .map(|y| (area.left()..area.right()).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn unrolling_keeps_both_borders() {
        let popup = Buffer::with_lines(["┌──┐", "│ab│", "│cd│", "│ef│", "└──┘"]);
        let mut buf = Buffer::with_lines(["....", "....", "....", "....", "...."]);
        let unroll = Unroll::new(popup.area, Anchor::Center, 0.6);
        draw_unrolled(&mut buf, &popup, unroll);
        assert_eq!(rows(&buf), ["....", "┌──┐", "│ab│", "└──┘", "...."]);

        let mut buf = Buffer::with_lines(["....", "....", "....", "....", "...."]);
        draw_unrolled(&mut buf, &popup, Unroll::new(popup.area, Anchor::Bottom, 0.6));
        assert_eq!(rows(&buf), ["....", "....", "┌──┐", "│ab│", "└──┘"]);
    }

    #[test]
    fn unroll_never_goes_below_its_borders() {
        let full = Rect::new(0, 0, 4, 5);
        let unroll = Unroll::new(full, Anchor::Center, 0.0);
        assert_eq!((unroll.row(0), unroll.row(1), unroll.row(4)), (Some(1), None, Some(2)));
    }

    #[test]
    fn shifts_both_ways_and_blanks_the_gap() {
        let mut buf = Buffer::with_lines(["abcd"]);
        let area = buf.area;
        shift(&mut buf, area, 2);
        assert_eq!(rows(&buf), ["  ab"]);
        let mut buf = Buffer::with_lines(["abcd"]);
        let area = buf.area;
        shift(&mut buf, area, -1);
        assert_eq!(rows(&buf), ["bcd "]);
    }

    #[test]
    fn tweens_end_where_they_aim() {
        let now = Instant::now();
        let tween = Tween::new(8.0, 0.0, SLIDE, now);
        assert_eq!(tween.value(now), 8.0);
        assert_eq!(tween.value(now + SLIDE), 0.0);
        assert!(tween.done(now + SLIDE));
    }
}
