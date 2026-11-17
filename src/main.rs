mod app;
mod diff;
mod keymap;
mod ui;

use anyhow::Result;
use clap::Parser;
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyEventKind},
};

use app::App;
use diff::DiffSource;
use diff::git::{DiffSpec, GitSource};

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
    let mut app = App::new(source.describe(), source.load()?);

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| ui::render(frame, app))?;
        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && let Some(action) = keymap::action_for(key)
        {
            app.update(action);
        }
    }
    Ok(())
}
