//! The review plan: asking the organizer for one, placing it on the diff, and
//! switching between the stack's layers, the plan and the plain file list.
//!
//! In a stack every layer is planned on its own, like the pull request it
//! is. All layers' groups are saved as one plan and split back by layer when
//! shown, so a layer's plan never covers another's hunks.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::time::Instant;

use super::{App, Effect, Row, View};
use crate::diff::{Diff, HunkId};
use crate::organizer::{OrganizeRequest, Plan};

/// An organizer run in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Job {
    /// Matches the run's result to it, so a cancelled run's late answer is
    /// told apart from the current one's.
    pub id: u64,
    pub started: Instant,
}

/// A page of the plan or stack view: a group of hunks read together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanPage {
    pub title: String,
    pub rationale: String,
    /// `(file, hunk)` in reading order.
    pub hunks: Vec<(usize, usize)>,
    /// Where it sits in the stack; `None` outside the stack view.
    pub place: Option<StackPlace>,
}

/// Where a page of the stack view sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StackPlace {
    pub layer: usize,
    /// For a layer the organizer planned, which of its groups this is and
    /// how many it has; `None` for the whole layer as git shows it.
    pub group: Option<(usize, usize)>,
}

/// The last group's title: hunks the plan doesn't cover.
pub(super) const UNPLANNED: &str = "Not in the plan";

/// Places a saved plan on the current diff. Hunks it doesn't mention (new
/// since it was made, or left out) go in a last group, so none go unseen.
/// A plan with none of the diff's hunks was made for another diff and
/// resolves to nothing.
pub(super) fn resolve(plan: &Plan, diff: &Diff) -> Vec<PlanPage> {
    resolve_in(plan, diff, 0..diff.files.len())
}

/// [`resolve`], for the hunks of `files` only.
fn resolve_in(plan: &Plan, diff: &Diff, files: Range<usize>) -> Vec<PlanPage> {
    let positions: HashMap<&HunkId, (usize, usize)> = files
        .map(|f| (f, &diff.files[f]))
        .flat_map(|(f, file)| {
            file.hunks
                .iter()
                .enumerate()
                .map(move |(h, hunk)| (&hunk.id, (f, h)))
        })
        .collect();
    let mut placed = HashSet::new();
    let mut pages: Vec<PlanPage> = plan
        .groups
        .iter()
        .map(|group| PlanPage {
            title: group.title.clone(),
            rationale: group.rationale.clone(),
            hunks: group
                .hunks
                .iter()
                .filter_map(|id| positions.get(id).copied())
                .filter(|&position| placed.insert(position))
                .collect(),
            place: None,
        })
        .filter(|page| !page.hunks.is_empty())
        .collect();
    if pages.is_empty() {
        return pages;
    }

    let mut rest: Vec<(usize, usize)> = positions
        .into_values()
        .filter(|p| !placed.contains(p))
        .collect();
    rest.sort();
    if !rest.is_empty() {
        pages.push(PlanPage {
            title: UNPLANNED.to_string(),
            rationale: "Hunks the plan doesn't cover: new since it was made, or left out."
                .to_string(),
            hunks: rest,
            place: None,
        });
    }
    pages
}

/// The stack view's pages, layer by layer: a layer the plan covers as its
/// groups, any other as a single page of its hunks. Layers with nothing to
/// review are left out.
pub(super) fn stack_pages(diff: &Diff, plan: Option<&Plan>) -> Vec<PlanPage> {
    let mut pages = Vec::new();
    for (layer, section) in diff.sections.iter().enumerate() {
        let planned = plan
            .map(|p| resolve_in(p, diff, section.files.clone()))
            .unwrap_or_default();
        if planned.is_empty() {
            let hunks: Vec<_> = section
                .files
                .clone()
                .flat_map(|f| (0..diff.files[f].hunks.len()).map(move |h| (f, h)))
                .collect();
            if !hunks.is_empty() {
                pages.push(PlanPage {
                    title: section.title.clone(),
                    rationale: section.rationale.clone(),
                    hunks,
                    place: Some(StackPlace { layer, group: None }),
                });
            }
            continue;
        }
        let count = planned.len();
        pages.extend(planned.into_iter().enumerate().map(|(i, page)| PlanPage {
            place: Some(StackPlace {
                layer,
                group: Some((i, count)),
            }),
            ..page
        }));
    }
    pages
}

impl App {
    pub fn is_organizing(&self) -> bool {
        self.organizing.is_some()
    }

    pub fn is_stack(&self) -> bool {
        !self.diff.sections.is_empty()
    }

    /// The pages of the current view; none in the files view.
    pub fn pages(&self) -> &[PlanPage] {
        match self.view {
            View::Stack => &self.stack,
            View::Plan => &self.plan,
            View::Files => &[],
        }
    }

    /// The stack layer the cursor is in.
    pub fn current_layer(&self) -> Option<usize> {
        match self.view {
            View::Stack => self
                .stack
                .get(self.page)
                .and_then(|p| p.place)
                .map(|p| p.layer),
            View::Plan | View::Files => {
                let file = self.current_file()?;
                self.diff
                    .sections
                    .iter()
                    .position(|s| s.files.contains(&file))
            }
        }
    }

    /// The first stack page of `layer`.
    pub(super) fn layer_page(&self, layer: usize) -> Option<usize> {
        self.stack
            .iter()
            .position(|p| p.place.is_some_and(|p| p.layer == layer))
    }

    /// The files of `layer`, or the whole diff's without one.
    pub(super) fn layer_files(&self, layer: Option<usize>) -> Range<usize> {
        layer.map_or(0..self.diff.files.len(), |l| {
            self.diff.sections[l].files.clone()
        })
    }

    pub(super) fn hunk_ids(&self, files: Range<usize>) -> HashSet<&HunkId> {
        self.diff.files[files]
            .iter()
            .flat_map(|f| &f.hunks)
            .map(|h| &h.id)
            .collect()
    }

    /// The layer to organize or clear: the one under the cursor in a stack,
    /// none in a plain diff. `Err` when in a stack but on no layer.
    pub(super) fn target_layer(&mut self) -> Result<Option<usize>, ()> {
        if !self.is_stack() {
            return Ok(None);
        }
        match self.current_layer() {
            Some(layer) => Ok(Some(layer)),
            None => {
                self.message = Some("Move onto a layer first".to_string());
                Err(())
            }
        }
    }

    /// Keeps hunks edited since the saved plan was made in their groups,
    /// saving the plan if that changes it. Returns how many it kept.
    pub(super) fn follow_plan(&mut self) -> usize {
        let Some(saved) = self.review.plan() else {
            return 0;
        };
        let mut plan = saved.clone();
        let followed = plan.follow(&self.diff);
        // Plans saved before anchors were kept get them now.
        plan.anchor(&self.diff);
        if self.review.plan() != Some(&plan) {
            self.review.set_plan(plan);
        }
        followed
    }

    /// What opening on a saved plan has to say: hunks edited since it was
    /// made that stayed in their groups, and hunks it doesn't cover.
    pub(super) fn plan_status(&self, followed: usize) -> Option<String> {
        let unplanned: usize = self
            .plan
            .iter()
            .chain(&self.stack)
            .filter(|p| p.title == UNPLANNED)
            .map(|p| p.hunks.len())
            .sum();
        let mut parts = Vec::new();
        match followed {
            0 => {}
            1 => parts.push("1 hunk edited since the plan stays in its group".to_string()),
            n => parts.push(format!(
                "{n} hunks edited since the plan stay in their groups"
            )),
        }
        match unplanned {
            0 => {}
            1 => parts.push(format!("1 hunk isn't in the plan: see “{UNPLANNED}”")),
            n => parts.push(format!("{n} hunks aren't in the plan: see “{UNPLANNED}”")),
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// Rebuilds the plan and stack pages from the saved plan.
    pub(super) fn place_plan(&mut self) {
        if self.is_stack() {
            self.stack = stack_pages(&self.diff, self.review.plan());
            self.plan.clear();
        } else {
            self.plan = self
                .review
                .plan()
                .map(|p| resolve(p, &self.diff))
                .unwrap_or_default();
        }
    }

    /// `:organize [cancel|clear [all]]`.
    pub(super) fn organize_command(&mut self, args: &[&str]) {
        match args {
            [] => self.start_organize(),
            ["cancel"] => self.cancel_organize(),
            ["clear"] => self.clear_plan(false),
            ["clear", "all"] => self.clear_plan(true),
            _ => self.message = Some("error: usage: :organize [cancel|clear [all]]".to_string()),
        }
    }

    /// Asks the organizer for a plan in the background: of the layer under
    /// the cursor in a stack, of the whole diff otherwise.
    fn start_organize(&mut self) {
        if self.is_organizing() {
            self.message = Some("Already organizing… (:organize cancel to stop)".to_string());
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
            self.message = Some("Nothing to organize".to_string());
            return;
        }
        let title = self.target_title(layer);
        self.next_job += 1;
        let job = Job {
            id: self.next_job,
            started: Instant::now(),
        };
        let request = OrganizeRequest::from_files(&title, &self.diff, files);
        self.effect = Some(Effect::Organize {
            job: job.id,
            request,
        });
        self.organizing = Some(job);
        self.organizing_layer = layer;
        if let Some(l) = layer {
            self.message = Some(format!("Organizing {}…", self.diff.sections[l].title));
        }
    }

    /// What the AI is told it's looking at: the diff, and the layer in a stack.
    pub(super) fn target_title(&self, layer: Option<usize>) -> String {
        match layer {
            Some(l) => {
                let (section, count) = (&self.diff.sections[l], self.diff.sections.len());
                format!(
                    "{} · layer {}/{count}: {} ({})",
                    self.title,
                    l + 1,
                    section.title,
                    section.rationale
                )
            }
            None => self.title.clone(),
        }
    }

    fn cancel_organize(&mut self) {
        match self.organizing.take() {
            Some(job) => {
                self.effect = Some(Effect::CancelOrganize { job: job.id });
                self.message = Some("Organizing cancelled".to_string());
            }
            None => self.message = Some("Not organizing".to_string()),
        }
    }

    /// Drops the plan, saved copy included: in a stack, the plan of the layer
    /// under the cursor, or every layer's with `all`.
    fn clear_plan(&mut self, all: bool) {
        let layer = if all {
            None
        } else {
            let Ok(layer) = self.target_layer() else {
                return;
            };
            layer
        };
        let message = match layer {
            None => {
                if self.review.plan().is_none() {
                    self.message = Some("No plan to clear".to_string());
                    return;
                }
                self.review.clear_plan();
                if self.is_stack() {
                    "Plan cleared: back to the stack's layers"
                } else {
                    "Plan cleared"
                }
                .to_string()
            }
            Some(l) => {
                let ids = self.hunk_ids(self.layer_files(Some(l)));
                let Some(mut saved) = self.review.plan().filter(|p| p.covers(&ids)).cloned() else {
                    self.message = Some(
                        "No plan for this layer (:organize clear all for every layer)".to_string(),
                    );
                    return;
                };
                saved.forget(&ids);
                if saved.groups.is_empty() && saved.notes.is_empty() {
                    self.review.clear_plan();
                } else {
                    self.review.set_plan(saved);
                }
                format!(
                    "Plan cleared for {}: back to its hunks",
                    self.diff.sections[l].title
                )
            }
        };
        self.place_notes();
        self.place_plan();
        match self.view {
            View::Stack => {
                self.rows.clear();
                self.open_page(layer.and_then(|l| self.layer_page(l)).unwrap_or(0), 0);
            }
            View::Plan => self.set_view(View::Files),
            View::Files => self.rebuild_rows(),
        }
        self.message = Some(message);
    }

    /// Takes the organizer's answer for `job`, saves the plan and shows it.
    /// Answers for runs that were cancelled or replaced are ignored.
    pub fn finish_organize(&mut self, job: u64, result: anyhow::Result<Plan>) {
        if self.organizing.is_none_or(|current| current.id != job) {
            return;
        }
        self.organizing = None;
        let layer = self.organizing_layer.take();
        let plan = match result {
            Ok(plan) => plan,
            Err(e) => {
                self.message = Some(format!("error: organizing failed: {e:#}"));
                return;
            }
        };
        let files = self.layer_files(layer);
        let known = self.hunk_ids(files.clone());
        let plan = plan.sanitize(&known);
        if resolve_in(&plan, &self.diff, files.clone()).is_empty() {
            self.message = Some("error: the organizer didn't place any hunks".to_string());
            return;
        }
        // A layer's new plan takes the place of its old one, next to the
        // other layers'.
        let plan = match layer {
            Some(_) => {
                let mut saved = self.review.plan().cloned().unwrap_or_default();
                saved.forget(&known);
                saved.groups.extend(plan.groups);
                saved.notes.extend(plan.notes);
                saved
            }
            None => plan,
        };
        let mut plan = plan;
        plan.anchor(&self.diff);
        self.review.set_plan(plan);
        self.place_notes();
        self.place_plan();
        // A fresh plan is read from the top.
        self.view = if layer.is_some() {
            View::Stack
        } else {
            View::Plan
        };
        self.rows.clear();
        let first = layer.and_then(|l| self.layer_page(l)).unwrap_or(0);
        self.open_page(first, 0);
        let groups = match layer {
            Some(l) => self
                .stack
                .iter()
                .filter(|p| p.place.is_some_and(|p| p.layer == l))
                .count(),
            None => self.plan.len(),
        };
        let notes = match self
            .notes
            .iter()
            .filter(|n| files.contains(&n.file))
            .count()
        {
            0 => String::new(),
            1 => " and 1 note".to_string(),
            n => format!(" and {n} notes"),
        };
        let what = layer.map_or("Organized".to_string(), |l| {
            format!("Organized {}", self.diff.sections[l].title)
        });
        let back = if layer.is_some() {
            "gf files"
        } else {
            "gp plan · gf files"
        };
        self.message = Some(format!("{what} into {groups} groups{notes} · {back}"));
    }

    /// `gs` / `gp` / `gf`: the stack's layers, the plan, or the files.
    pub(super) fn show_view(&mut self, view: View) {
        if view == self.view {
            return;
        }
        match view {
            View::Stack if !self.is_stack() => {
                self.message =
                    Some("Not a stack: `rrrev stack` reviews one layer by layer".to_string())
            }
            View::Stack if self.stack.is_empty() => {
                self.message = Some("Nothing to review in the stack".to_string())
            }
            View::Plan if self.is_stack() => {
                self.message =
                    Some("In a stack each layer has its own plan: gs shows them".to_string())
            }
            View::Plan if self.plan.is_empty() => {
                self.message = Some("No plan yet: run :organize".to_string())
            }
            _ => self.set_view(view),
        }
    }

    /// `]s` / `[s`: the start of the next / previous layer, in the stack or
    /// the files. Going back from inside a layer lands on its start first.
    pub(super) fn jump_layer(&mut self, n: usize, forward: bool) {
        if !self.is_stack() {
            self.message = Some("Not a stack".to_string());
            return;
        }
        let mut starts: Vec<usize> = (0..self.diff.sections.len())
            .filter_map(|l| match self.view {
                View::Stack => self.layer_page(l),
                View::Files | View::Plan => {
                    let files = &self.diff.sections[l].files;
                    (!files.is_empty()).then_some(files.start)
                }
            })
            .collect();
        starts.dedup();
        let target = if forward {
            starts.iter().filter(|&&s| s > self.page).take(n).last()
        } else {
            starts
                .iter()
                .rev()
                .filter(|&&s| s < self.page)
                .take(n)
                .last()
        };
        match target {
            Some(&page) => self.open_page(page, 0),
            None => {
                self.message = Some(if forward { "Last layer" } else { "First layer" }.to_string())
            }
        }
    }

    /// Switches view, keeping the cursor on the same row when the new view
    /// shows it, or on the same file otherwise.
    pub(super) fn set_view(&mut self, view: View) {
        let current = self.current_row();
        let file = match current {
            Some(Row::Group { group, .. }) => self
                .pages()
                .get(group)
                .and_then(|g| g.hunks.first())
                .map(|&(f, _)| f),
            row => row.and_then(Row::file),
        };
        self.view = view;
        let find = |row: Row| {
            (0..self.page_count()).find_map(|p| {
                self.page_rows(p)
                    .iter()
                    .position(|r| *r == row)
                    .map(|i| (p, i))
            })
        };
        let (page, row) = current
            .filter(|row| !matches!(row, Row::Group { .. }))
            .and_then(find)
            .or_else(|| file.and_then(|file| find(Row::File { file })))
            .unwrap_or((0, 0));
        // Forces the page's rows to be rebuilt for the new view.
        self.rows.clear();
        self.open_page(page, row);
        self.scroll_with_above(self.viewport_height / 2);
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::Action;
    use crate::organizer::PlanGroup;
    use crate::review::ReviewState;

    /// A plan for the sample app: b.rs's hunk first, then a.rs's second hunk;
    /// a.rs's first hunk is left out.
    fn sample_plan(app: &App) -> Plan {
        let id = |f: usize, h: usize| app.diff.files[f].hunks[h].id.clone();
        Plan {
            groups: vec![
                PlanGroup {
                    title: "Types".into(),
                    rationale: "First.".into(),
                    hunks: vec![id(1, 0)],
                },
                PlanGroup {
                    title: "Logic".into(),
                    rationale: "Then.".into(),
                    hunks: vec![id(0, 1)],
                },
            ],
            ..Plan::default()
        }
    }

    fn organized_app() -> App {
        let mut app = sample_app();
        command(&mut app, "organize");
        let Some(Effect::Organize { job, .. }) = app.effect.take() else {
            panic!("no organize effect")
        };
        assert!(app.is_organizing());
        let plan = sample_plan(&app);
        app.finish_organize(job, Ok(plan));
        app
    }

    #[test]
    fn organizing_shows_the_plan_and_saves_it() {
        let app = organized_app();
        assert!(!app.is_organizing());
        assert_eq!(app.view, View::Plan);
        let titles: Vec<_> = app.plan.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, ["Types", "Logic", "Not in the plan"]);
        assert_eq!(app.plan[2].hunks, [(0, 0)]);
        assert_eq!(app.review.plan().unwrap().groups.len(), 2);
    }

    #[test]
    fn a_group_page_shows_its_title_files_and_hunks() {
        let mut app = organized_app();
        app.update(Action::Top);
        assert_eq!(app.page, 0);
        // Unwrapped (no width yet): a title line, a rationale line.
        assert_eq!(
            app.rows[..4],
            [
                Row::Group { group: 0, line: 0 },
                Row::Group { group: 0, line: 1 },
                Row::File { file: 1 },
                Row::Hunk { file: 1, hunk: 0 },
            ]
        );
        assert_eq!(app.rows.len(), 4 + 4);
    }

    #[test]
    fn page_motions_follow_the_plan() {
        let mut app = organized_app();
        app.update(Action::Top);
        app.update(Action::NextFile(1));
        assert_eq!(app.current_row(), Some(Row::Group { group: 1, line: 0 }));
        app.update(Action::NextFile(5));
        assert_eq!(app.page, 2);
        app.update(Action::NextFile(1));
        assert_eq!(app.message.as_deref(), Some("Last group"));

        // ]u walks hunks in plan order: b.rs before a.rs.
        app.update(Action::PrevFile(5));
        app.update(Action::NextUnreviewed(1));
        assert_eq!(app.current_row(), Some(Row::Hunk { file: 1, hunk: 0 }));
        app.update(Action::NextUnreviewed(1));
        assert_eq!(app.current_row(), Some(Row::Hunk { file: 0, hunk: 1 }));
    }

    #[test]
    fn r_on_a_group_header_reviews_the_group() {
        let mut app = organized_app();
        app.update(Action::Top);
        app.update(Action::ToggleReviewed);
        assert!(app.is_reviewed(1, 0));
        assert!(!app.is_reviewed(0, 1));
        assert!(app.group_progress(0).is_complete());
        // That was all of b.rs, so it folds.
        assert!(app.collapsed[1]);
        assert_eq!(app.rows[2..], [Row::File { file: 1 }]);
        app.update(Action::NextFile(1));
        app.update(Action::Down(3));
        app.update(Action::TogglePageReviewed);
        assert!(app.is_reviewed(0, 1));
        assert!(!app.is_reviewed(0, 0));
        // a.rs still has a hunk in another group, so it stays open.
        assert!(!app.collapsed[0]);
    }

    #[test]
    fn switching_views_keeps_the_cursor_on_its_row() {
        let mut app = organized_app();
        app.update(Action::Top);
        app.update(Action::NextFile(1));
        // Past the two header lines and the file header, onto the hunk's first line.
        app.update(Action::Down(4));
        let row = app.current_row();
        assert_eq!(
            row,
            Some(Row::Line {
                file: 0,
                hunk: 1,
                line: 0
            })
        );

        app.update(Action::ShowView(View::Files));
        assert_eq!(app.view, View::Files);
        assert_eq!((app.page, app.current_row()), (0, row));
        app.update(Action::ShowView(View::Plan));
        assert_eq!((app.page, app.current_row()), (1, row));

        // A group header has no place in the files view: land on its first file.
        app.update(Action::Top);
        app.update(Action::ShowView(View::Files));
        assert_eq!(app.current_row(), Some(Row::File { file: 0 }));
    }

    #[test]
    fn group_headers_wrap_to_the_pane() {
        let mut app = organized_app();
        app.plan[0].rationale = "Adds the organizer module with its plan types".into();
        app.update(Action::Top);
        app.set_viewport(23, 10);
        let header = app.group_header(0);
        assert_eq!(
            header,
            [
                (true, "1/3 Types".to_string()),
                (false, "Adds the organizer".to_string()),
                (false, "module with its plan".to_string()),
                (false, "types".to_string()),
            ]
        );
        assert_eq!(
            app.rows
                .iter()
                .filter(|r| matches!(r, Row::Group { .. }))
                .count(),
            4
        );

        // Widening rewraps, and the cursor stays on the header.
        app.update(Action::Down(3));
        app.set_viewport(200, 10);
        assert_eq!(
            app.rows
                .iter()
                .filter(|r| matches!(r, Row::Group { .. }))
                .count(),
            2
        );
        assert!(matches!(app.current_row(), Some(Row::Group { .. })));
    }

    #[test]
    fn no_plan_yet() {
        let mut app = sample_app();
        app.update(Action::ShowView(View::Plan));
        assert_eq!(app.view, View::Files);
        assert_eq!(app.message.as_deref(), Some("No plan yet: run :organize"));
        app.update(Action::ShowView(View::Stack));
        assert_eq!(app.view, View::Files);
        assert!(app.message.as_deref().unwrap().starts_with("Not a stack"));
        app.update(Action::NextLayer(1));
        assert_eq!(app.message.as_deref(), Some("Not a stack"));
    }

    #[test]
    fn cancelling_ignores_the_late_answer() {
        let mut app = sample_app();
        command(&mut app, "organize");
        let Some(Effect::Organize { job: first, .. }) = app.effect.take() else {
            panic!()
        };
        command(&mut app, "organize cancel");
        assert_eq!(
            app.effect.take(),
            Some(Effect::CancelOrganize { job: first })
        );
        assert!(!app.is_organizing());

        // A new run starts; the cancelled one's answer arriving now changes nothing.
        command(&mut app, "organize");
        let Some(Effect::Organize { job: second, .. }) = app.effect.take() else {
            panic!()
        };
        assert_ne!(first, second);
        let plan = sample_plan(&app);
        app.finish_organize(first, Ok(plan.clone()));
        assert!(app.is_organizing());
        assert!(app.plan.is_empty());
        app.finish_organize(second, Ok(plan));
        assert_eq!(app.plan.len(), 3);

        command(&mut app, "organize cancel");
        assert_eq!(app.message.as_deref(), Some("Not organizing"));
        command(&mut app, "organize now");
        assert!(app.message.as_deref().unwrap().starts_with("error: usage"));
    }

    #[test]
    fn clearing_the_plan_goes_back_to_the_files() {
        let mut app = organized_app();
        app.update(Action::NextFile(1));
        app.update(Action::Down(4));
        let row = app.current_row();
        command(&mut app, "organize clear");
        assert_eq!(app.message.as_deref(), Some("Plan cleared"));
        assert_eq!(app.view, View::Files);
        assert_eq!(app.current_row(), row);
        assert!(app.plan.is_empty());
        assert!(app.review.plan().is_none());
        command(&mut app, "organize clear");
        assert_eq!(app.message.as_deref(), Some("No plan to clear"));
    }

    #[test]
    fn failures_and_double_starts_are_reported() {
        let mut app = sample_app();
        command(&mut app, "organize");
        let Some(Effect::Organize { job, .. }) = app.effect.take() else {
            panic!()
        };
        command(&mut app, "organize");
        assert_eq!(
            app.message.as_deref(),
            Some("Already organizing… (:organize cancel to stop)")
        );
        app.finish_organize(job, Err(anyhow::anyhow!("no network")));
        assert_eq!(
            app.message.as_deref(),
            Some("error: organizing failed: no network")
        );
        assert_eq!(app.view, View::Files);
        assert!(!app.is_organizing());
    }

    #[test]
    fn a_saved_plan_opens_in_the_plan_view() {
        let app = organized_app();
        let reopened = App::new(String::new(), app.diff.clone(), app.review);
        assert_eq!(reopened.view, View::Plan);
        assert_eq!(reopened.plan, app.plan);
    }

    #[test]
    fn a_plan_for_another_diff_is_not_shown() {
        let app = organized_app();
        let other = Diff::new(vec![file("c.rs", &[2]), file("d.rs", &[1])]);
        let reopened = App::new(String::new(), other, app.review);
        assert_eq!(reopened.view, View::Files);
        assert!(reopened.plan.is_empty());
        assert!(
            reopened
                .message
                .as_deref()
                .unwrap()
                .starts_with("Saved plan is for another diff")
        );
        // Kept, in case that diff comes back.
        assert!(reopened.review.plan().is_some());
    }

    #[test]
    fn a_useless_answer_keeps_the_old_view() {
        let mut app = sample_app();
        command(&mut app, "organize");
        let Some(Effect::Organize { job, .. }) = app.effect.take() else {
            panic!()
        };
        let bogus: HunkId = serde_json::from_str("\"nope\"").unwrap();
        let plan = Plan {
            groups: vec![PlanGroup {
                title: "x".into(),
                rationale: String::new(),
                hunks: vec![bogus],
            }],
            ..Plan::default()
        };
        app.finish_organize(job, Ok(plan));
        assert_eq!(app.view, View::Files);
        assert!(app.review.plan().is_none());
        assert_eq!(
            app.message.as_deref(),
            Some("error: the organizer didn't place any hunks")
        );
    }

    /// Files: 0 a.rs in layer 1; 1 a.rs and 2 b.rs in layer 2; 3 c.rs in layer 3.
    fn stacked_app() -> App {
        let diff = Diff::sectioned(vec![
            (
                "#1 types".into(),
                "Layer 1 of 3.".into(),
                vec![file("a.rs", &[3])],
            ),
            (
                "#2 logic".into(),
                "Layer 2 of 3.".into(),
                vec![file("a.rs", &[2]), file("b.rs", &[1])],
            ),
            (
                "#3 ui".into(),
                "Layer 3 of 3.".into(),
                vec![file("c.rs", &[1])],
            ),
        ]);
        App::new(String::new(), diff, ReviewState::default())
    }

    fn titles(pages: &[PlanPage]) -> Vec<&str> {
        pages.iter().map(|p| p.title.as_str()).collect()
    }

    fn id(app: &App, file: usize, hunk: usize) -> HunkId {
        app.diff.files[file].hunks[hunk].id.clone()
    }

    fn group(title: &str, hunks: Vec<HunkId>) -> PlanGroup {
        PlanGroup {
            title: title.into(),
            rationale: String::new(),
            hunks,
        }
    }

    /// Runs `:organize` where the cursor is and answers with `groups` of
    /// `(file, hunk)`s.
    fn organize_with(app: &mut App, groups: &[(&str, &[(usize, usize)])]) {
        let groups = groups
            .iter()
            .map(|(title, hunks)| group(title, hunks.iter().map(|&(f, h)| id(app, f, h)).collect()))
            .collect();
        command(app, "organize");
        let Some(Effect::Organize { job, .. }) = app.effect.take() else {
            panic!("no organize effect")
        };
        app.finish_organize(
            job,
            Ok(Plan {
                groups,
                ..Plan::default()
            }),
        );
    }

    #[test]
    fn a_selection_stays_in_its_file_when_a_group_has_several() {
        let mut app = sample_app();
        // Both files' first hunks share an index on one page.
        organize_with(&mut app, &[("Both", &[(0, 0), (1, 0)])]);
        app.update(Action::Top);
        let first = app
            .rows
            .iter()
            .position(|r| {
                *r == Row::Line {
                    file: 0,
                    hunk: 0,
                    line: 1,
                }
            })
            .unwrap();
        let other = app
            .rows
            .iter()
            .position(|r| {
                *r == Row::Line {
                    file: 1,
                    hunk: 0,
                    line: 3,
                }
            })
            .unwrap();
        app.cursor = first;
        app.update(Action::StartVisual);
        app.cursor = other;
        assert_eq!(app.selection(), Some((0, 0, 1, 2)));

        comment(&mut app, "range");
        let c = &app.review.comments()[0];
        assert_eq!((c.path.as_str(), c.start, c.end), ("a.rs", 1, 2));
    }

    #[test]
    fn a_stack_opens_layer_by_layer() {
        let app = stacked_app();
        assert_eq!(app.view, View::Stack);
        assert_eq!(titles(&app.stack), ["#1 types", "#2 logic", "#3 ui"]);
        assert_eq!(app.stack[1].hunks, [(1, 0), (2, 0)]);
        assert_eq!(
            app.stack[1].place,
            Some(StackPlace {
                layer: 1,
                group: None
            })
        );
        assert_eq!(app.group_header(1)[0], (true, "2/3 #2 logic".to_string()));
        assert_eq!(app.page_position(), "layer 1/3");
        // Nothing saved: the layers come from the diff each time.
        assert!(app.review.plan().is_none());
        assert!(app.plan.is_empty());
    }

    #[test]
    fn organizing_plans_only_the_layer_under_the_cursor() {
        let mut app = stacked_app();
        app.update(Action::NextLayer(1));
        assert_eq!(app.current_layer(), Some(1));
        command(&mut app, "organize");
        let Some(Effect::Organize { job, request }) = app.effect.take() else {
            panic!()
        };
        let asked: Vec<_> = request.hunks.iter().map(|h| h.id.clone()).collect();
        assert_eq!(asked, [id(&app, 1, 0), id(&app, 2, 0)]);
        assert!(request.title.contains("layer 2/3: #2 logic"));

        // An id from another layer is dropped, like any unknown one.
        let groups = vec![
            group("Types", vec![id(&app, 2, 0), id(&app, 0, 0)]),
            group("Logic", vec![id(&app, 1, 0)]),
        ];
        app.finish_organize(
            job,
            Ok(Plan {
                groups,
                ..Plan::default()
            }),
        );
        assert_eq!(app.view, View::Stack);
        assert_eq!(titles(&app.stack), ["#1 types", "Types", "Logic", "#3 ui"]);
        assert_eq!(app.stack[0].hunks, [(0, 0)]);
        assert_eq!(
            app.stack[2].place,
            Some(StackPlace {
                layer: 1,
                group: Some((1, 2))
            })
        );
        assert_eq!(app.page, 1);
        assert_eq!(
            app.group_header(1)[0],
            (true, "2/3 #2 logic · 1/2 Types".to_string())
        );
        assert_eq!(app.page_label(1), "#2 logic › Types");
        assert_eq!(app.page_position(), "layer 2/3 · group 1/2");
        assert_eq!(
            app.message.as_deref(),
            Some("Organized #2 logic into 2 groups · gf files")
        );
    }

    #[test]
    fn replanning_or_clearing_a_layer_leaves_the_others_alone() {
        let mut app = stacked_app();
        organize_with(&mut app, &[("Base", &[(0, 0)])]);
        app.update(Action::NextLayer(1));
        organize_with(&mut app, &[("First", &[(1, 0), (2, 0)])]);
        organize_with(&mut app, &[("Again", &[(2, 0)])]);
        assert_eq!(
            titles(&app.stack),
            ["Base", "Again", "Not in the plan", "#3 ui"]
        );

        // From the files too: the layer is the one the file is in.
        app.update(Action::ShowView(View::Files));
        assert_eq!(app.current_file(), Some(2));
        command(&mut app, "organize clear");
        assert_eq!(
            app.message.as_deref(),
            Some("Plan cleared for #2 logic: back to its hunks")
        );
        assert_eq!(app.view, View::Files);
        assert_eq!(titles(&app.stack), ["Base", "#2 logic", "#3 ui"]);
        command(&mut app, "organize clear");
        assert!(
            app.message
                .as_deref()
                .unwrap()
                .starts_with("No plan for this layer")
        );

        command(&mut app, "organize clear all");
        assert_eq!(
            app.message.as_deref(),
            Some("Plan cleared: back to the stack's layers")
        );
        assert_eq!(titles(&app.stack), ["#1 types", "#2 logic", "#3 ui"]);
        assert!(app.review.plan().is_none());
    }

    #[test]
    fn a_plan_across_layers_is_split_by_layer() {
        // As saved before layers were planned one at a time.
        let app = stacked_app();
        let mut review = ReviewState::default();
        review.set_plan(Plan {
            groups: vec![
                group("Mixed", vec![id(&app, 2, 0), id(&app, 0, 0)]),
                group("Rest", vec![id(&app, 1, 0)]),
            ],
            ..Plan::default()
        });
        let mut app = App::new(String::new(), app.diff, review);
        assert_eq!(app.view, View::Stack);
        assert_eq!(titles(&app.stack), ["Mixed", "Mixed", "Rest", "#3 ui"]);
        assert_eq!(app.stack[0].hunks, [(0, 0)]);
        assert_eq!(app.stack[1].hunks, [(2, 0)]);
        assert!(app.plan.is_empty());

        app.update(Action::ShowView(View::Plan));
        assert_eq!(app.view, View::Stack);
        assert!(
            app.message
                .as_deref()
                .unwrap()
                .starts_with("In a stack each layer has its own plan")
        );
    }

    #[test]
    fn capital_r_in_a_stack_reviews_only_the_file_under_the_cursor() {
        let mut app = stacked_app();
        app.update(Action::NextLayer(1));
        let b = app
            .rows
            .iter()
            .position(|r| *r == Row::Hunk { file: 2, hunk: 0 })
            .unwrap();
        app.cursor = b;
        app.update(Action::TogglePageReviewed);
        assert!(app.is_reviewed(2, 0));
        assert!(!app.is_reviewed(1, 0));

        app.update(Action::TogglePageReviewed);
        assert!(!app.is_reviewed(2, 0));
    }

    #[test]
    fn layer_jumps_work_in_the_stack_and_the_files() {
        let mut app = stacked_app();
        organize_with(&mut app, &[("Base", &[(0, 0)])]);
        app.update(Action::NextLayer(1));
        organize_with(&mut app, &[("One", &[(1, 0)]), ("Two", &[(2, 0)])]);
        assert_eq!(titles(&app.stack), ["Base", "One", "Two", "#3 ui"]);

        app.update(Action::NextFile(1));
        assert_eq!(app.page, 2);
        // Back from inside a layer: its start first, then the layer before.
        app.update(Action::PrevLayer(1));
        assert_eq!(app.page, 1);
        app.update(Action::PrevLayer(1));
        assert_eq!(app.page, 0);
        app.update(Action::PrevLayer(1));
        assert_eq!(app.message.as_deref(), Some("First layer"));
        app.update(Action::NextLayer(2));
        assert_eq!(app.page, 3);
        app.update(Action::NextLayer(1));
        assert_eq!(app.message.as_deref(), Some("Last layer"));

        app.update(Action::ShowView(View::Files));
        assert_eq!(app.page, 3);
        app.update(Action::PrevLayer(1));
        assert_eq!(app.page, 1);
        app.update(Action::NextFile(1));
        app.update(Action::PrevLayer(1));
        assert_eq!(app.page, 1);
        app.update(Action::ShowView(View::Stack));
        assert_eq!(app.page, 1);
    }

    #[test]
    fn hunks_new_since_the_plan_land_in_the_last_group() {
        let app = organized_app();
        let files = vec![
            file("a.rs", &[3, 2]),
            file("b.rs", &[4]),
            file("c.rs", &[1]),
        ];
        let reopened = App::new(String::new(), Diff::new(files), app.review);
        assert_eq!(reopened.plan.last().unwrap().hunks, [(0, 0), (2, 0)]);
    }

    #[test]
    fn edited_hunks_stay_in_their_groups() {
        let before = Diff::new(vec![
            file_at("a.rs", &[(10, "one"), (50, "two")]),
            file_at("b.rs", &[(5, "three")]),
        ]);
        let id = |diff: &Diff, f: usize, h: usize| diff.files[f].hunks[h].id.clone();
        let mut plan = Plan {
            groups: vec![
                group("First", vec![id(&before, 0, 1)]),
                group("Then", vec![id(&before, 1, 0), id(&before, 0, 0)]),
            ],
            notes: vec![crate::organizer::AgentNote {
                hunk: id(&before, 0, 1),
                line: Some(50),
                text: "Look".into(),
            }],
            ..Plan::default()
        };
        plan.anchor(&before);
        let mut review = ReviewState::default();
        review.set_plan(plan);

        // a.rs's second hunk is edited, and c.rs is new.
        let after = Diff::new(vec![
            file_at("a.rs", &[(10, "one"), (50, "two, edited")]),
            file_at("b.rs", &[(5, "three")]),
            file_at("c.rs", &[(1, "four")]),
        ]);
        let edited = id(&after, 0, 1);
        let app = App::new(String::new(), after, review);
        assert_eq!(titles(&app.plan), ["First", "Then", UNPLANNED]);
        assert_eq!(app.plan[0].hunks, [(0, 1)]);
        assert_eq!(app.plan[2].hunks, [(2, 0)]);
        assert_eq!(
            app.message.as_deref(),
            Some(
                "1 hunk edited since the plan stays in its group · 1 hunk isn't in the plan: see “Not in the plan”"
            )
        );
        // Saved, so the next edit is followed from here.
        let saved = app.review.plan().unwrap();
        assert_eq!(saved.groups[0].hunks, std::slice::from_ref(&edited));
        assert_eq!(saved.notes[0].hunk, edited);
        assert_eq!(saved.notes[0].line, Some(50));
        assert!(saved.anchors.contains_key(&edited));
    }

    #[test]
    fn plans_saved_without_anchors_get_them() {
        let app = organized_app();
        let mut plan = app.review.plan().unwrap().clone();
        plan.anchors.clear();
        let mut review = ReviewState::default();
        review.set_plan(plan);
        let reopened = App::new(String::new(), app.diff.clone(), review);
        assert_eq!(reopened.review.plan().unwrap().anchors.len(), 2);
        assert_eq!(
            reopened.message.as_deref(),
            Some("1 hunk isn't in the plan: see “Not in the plan”")
        );
    }
}
