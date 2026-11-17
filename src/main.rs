mod app;
mod diff;
mod keymap;
mod review;
mod ui;

use std::path::Path;

use anyhow::Result;
use clap::Parser;
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyEventKind},
};

use app::App;
use diff::DiffSource;
use diff::git::{DiffSpec, GitSource};
use keymap::KeyHandler;
use review::ReviewState;

/// Keyboard-driven code review in the terminal.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// What to review: `<rev>` (vs the working tree), `a..b`, or `a...b` (from
    /// the merge base). Defaults to HEAD vs the working tree, untracked files included.
    spec: Option<String>,

    /// Review staged changes only (HEAD vs index).
    #[arg(long, conflicts_with = "spec")]
    staged: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let source = GitSource::discover(".", DiffSpec::from_args(cli.spec.as_deref(), cli.staged))?;
    // Load before entering the alternate screen so errors print normally.
    let state_file = source.state_file();
    let review = ReviewState::load(&state_file)?;
    let mut app = App::new(source.describe(), source.load()?, review);

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, &state_file);
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal, app: &mut App, state_file: &Path) -> Result<()> {
    let mut keys = KeyHandler::default();
    while !app.should_quit {
        terminal.draw(|frame| ui::render(frame, app, &keys.pending()))?;
        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && let Some(action) = keys.handle(key)
        {
            app.update(action);
            // A failed save shouldn't end the session; the next change retries.
            if app.review.take_dirty()
                && let Err(e) = app.review.save(state_file)
            {
                app.message = Some(format!("error: could not save review state: {e:#}"));
            }
        }
    }
    Ok(())
}
