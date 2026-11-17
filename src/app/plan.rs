//! The review plan: asking the organizer for one, placing it on the diff, and
//! switching between the plan and the plain file list.

use std::collections::{HashMap, HashSet};
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

/// A plan group resolved against the current diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanPage {
    pub title: String,
    pub rationale: String,
    /// `(file, hunk)` in reading order.
    pub hunks: Vec<(usize, usize)>,
}

/// Places a saved plan on the current diff. Hunks it doesn't mention (new
/// since it was made, or left out) go in a last group, so none go unseen.
pub(super) fn resolve(plan: &Plan, diff: &Diff) -> Vec<PlanPage> {
    let positions: HashMap<&HunkId, (usize, usize)> = diff
        .files
        .iter()
        .enumerate()
        .flat_map(|(f, file)| file.hunks.iter().enumerate().map(move |(h, hunk)| (&hunk.id, (f, h))))
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
        })
        .filter(|page| !page.hunks.is_empty())
        .collect();

    let mut rest: Vec<(usize, usize)> = positions.into_values().filter(|p| !placed.contains(p)).collect();
    rest.sort();
    if !rest.is_empty() {
        pages.push(PlanPage {
            title: "Not in the plan".to_string(),
            rationale: "Hunks the plan doesn't cover: new since it was made, or left out.".to_string(),
            hunks: rest,
        });
    }
    pages
}

impl App {
    pub fn is_organizing(&self) -> bool {
        self.organizing.is_some()
    }

    /// `:organize [cancel|clear]`.
    pub(super) fn organize_command(&mut self, args: &[&str]) {
        match args {
            [] => self.start_organize(),
            ["cancel"] => self.cancel_organize(),
            ["clear"] => self.clear_plan(),
            _ => self.message = Some("error: usage: :organize [cancel|clear]".to_string()),
        }
    }

    /// Asks the organizer for a plan in the background.
    fn start_organize(&mut self) {
        if self.is_organizing() {
            self.message = Some("Already organizing… (:organize cancel to stop)".to_string());
        } else if self.diff.files.iter().all(|f| f.hunks.is_empty()) {
            self.message = Some("Nothing to organize".to_string());
        } else {
            self.next_job += 1;
            let job = Job { id: self.next_job, started: Instant::now() };
            let request = OrganizeRequest::from_diff(&self.title, &self.diff);
            self.effect = Some(Effect::Organize { job: job.id, request });
            self.organizing = Some(job);
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

    /// Drops the plan, saved copy included, and goes back to the files.
    fn clear_plan(&mut self) {
        if self.plan.is_empty() && self.review.plan().is_none() {
            self.message = Some("No plan to clear".to_string());
            return;
        }
        if self.view == View::Plan {
            self.set_view(View::Files);
        }
        self.plan.clear();
        self.review.clear_plan();
        self.message = Some("Plan cleared".to_string());
    }

    /// Takes the organizer's answer for `job`, saves the plan and shows it.
    /// Answers for runs that were cancelled or replaced are ignored.
    pub fn finish_organize(&mut self, job: u64, result: anyhow::Result<Plan>) {
        if self.organizing.is_none_or(|current| current.id != job) {
            return;
        }
        self.organizing = None;
        let plan = match result {
            Ok(plan) => plan,
            Err(e) => {
                self.message = Some(format!("error: organizing failed: {e:#}"));
                return;
            }
        };
        let known: HashSet<&HunkId> = self.diff.files.iter().flat_map(|f| &f.hunks).map(|h| &h.id).collect();
        let plan = plan.sanitize(&known);
        self.plan = resolve(&plan, &self.diff);
        self.review.set_plan(plan);
        // A fresh plan is read from the top.
        self.view = View::Plan;
        self.rows.clear();
        self.open_page(0, 0);
        let groups = self.plan.len();
        self.message = Some(format!("Organized into {groups} groups · p switches plan / files"));
    }

    /// `p`: between the plan and the file list.
    pub(super) fn toggle_view(&mut self) {
        match self.view {
            View::Files if self.plan.is_empty() => {
                self.message = Some("No plan yet: run :organize".to_string())
            }
            View::Files => self.set_view(View::Plan),
            View::Plan => self.set_view(View::Files),
        }
    }

    /// Switches view, keeping the cursor on the same row when the new view
    /// shows it, or on the same file otherwise.
    pub(super) fn set_view(&mut self, view: View) {
        let current = self.current_row();
        let file = match current {
            Some(Row::Group { group, .. }) => self.plan.get(group).and_then(|g| g.hunks.first()).map(|&(f, _)| f),
            row => row.and_then(Row::file),
        };
        self.view = view;
        let find = |row: Row| {
            (0..self.page_count()).find_map(|p| self.page_rows(p).iter().position(|r| *r == row).map(|i| (p, i)))
        };
        let (page, row) = current
            .and_then(find)
            .or_else(|| file.and_then(|file| find(Row::File { file })))
            .unwrap_or((0, 0));
        // Forces the page's rows to be rebuilt for the new view.
        self.rows.clear();
        self.open_page(page, row);
        self.scroll = row.saturating_sub(self.viewport_height / 2);
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use crate::app::Action;
    use crate::organizer::PlanGroup;

    /// A plan for the sample app: b.rs's hunk first, then a.rs's second hunk;
    /// a.rs's first hunk is left out.
    fn sample_plan(app: &App) -> Plan {
        let id = |f: usize, h: usize| app.diff.files[f].hunks[h].id.clone();
        Plan {
            groups: vec![
                PlanGroup { title: "Types".into(), rationale: "First.".into(), hunks: vec![id(1, 0)] },
                PlanGroup { title: "Logic".into(), rationale: "Then.".into(), hunks: vec![id(0, 1)] },
            ],
        }
    }

    fn organized_app() -> App {
        let mut app = sample_app();
        command(&mut app, "organize");
        let Some(Effect::Organize { job, .. }) = app.effect.take() else { panic!("no organize effect") };
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
        app.update(Action::NextFile(1));
        app.update(Action::Down(3));
        app.update(Action::TogglePageReviewed);
        assert!(app.is_reviewed(0, 1));
        assert!(!app.is_reviewed(0, 0));
    }

    #[test]
    fn switching_views_keeps_the_cursor_on_its_row() {
        let mut app = organized_app();
        app.update(Action::Top);
        app.update(Action::NextFile(1));
        // Past the two header lines and the file header, onto the hunk's first line.
        app.update(Action::Down(4));
        let row = app.current_row();
        assert_eq!(row, Some(Row::Line { file: 0, hunk: 1, line: 0 }));

        app.update(Action::ToggleView);
        assert_eq!(app.view, View::Files);
        assert_eq!((app.page, app.current_row()), (0, row));
        app.update(Action::ToggleView);
        assert_eq!((app.page, app.current_row()), (1, row));

        // A group header has no place in the files view: land on its first file.
        app.update(Action::Top);
        app.update(Action::ToggleView);
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
        assert_eq!(app.rows.iter().filter(|r| matches!(r, Row::Group { .. })).count(), 4);

        // Widening rewraps, and the cursor stays on the header.
        app.update(Action::Down(3));
        app.set_viewport(200, 10);
        assert_eq!(app.rows.iter().filter(|r| matches!(r, Row::Group { .. })).count(), 2);
        assert!(matches!(app.current_row(), Some(Row::Group { .. })));
    }

    #[test]
    fn no_plan_yet() {
        let mut app = sample_app();
        app.update(Action::ToggleView);
        assert_eq!(app.view, View::Files);
        assert_eq!(app.message.as_deref(), Some("No plan yet: run :organize"));
    }

    #[test]
    fn cancelling_ignores_the_late_answer() {
        let mut app = sample_app();
        command(&mut app, "organize");
        let Some(Effect::Organize { job: first, .. }) = app.effect.take() else { panic!() };
        command(&mut app, "organize cancel");
        assert_eq!(app.effect.take(), Some(Effect::CancelOrganize { job: first }));
        assert!(!app.is_organizing());

        // A new run starts; the cancelled one's answer arriving now changes nothing.
        command(&mut app, "organize");
        let Some(Effect::Organize { job: second, .. }) = app.effect.take() else { panic!() };
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
        let Some(Effect::Organize { job, .. }) = app.effect.take() else { panic!() };
        command(&mut app, "organize");
        assert_eq!(app.message.as_deref(), Some("Already organizing… (:organize cancel to stop)"));
        app.finish_organize(job, Err(anyhow::anyhow!("no network")));
        assert_eq!(app.message.as_deref(), Some("error: organizing failed: no network"));
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
    fn hunks_new_since_the_plan_land_in_the_last_group() {
        let app = organized_app();
        let files = vec![file("a.rs", &[3, 2]), file("b.rs", &[4]), file("c.rs", &[1])];
        let reopened = App::new(String::new(), Diff::new(files), app.review);
        assert_eq!(reopened.plan.last().unwrap().hunks, [(0, 0), (2, 0)]);
    }
}
