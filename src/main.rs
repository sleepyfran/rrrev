mod app;
mod diff;
mod export;
mod input;
mod keymap;
mod review;
mod ui;

use std::fs;
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use ratatui::{
    DefaultTerminal,
    crossterm::{
        event::{self, Event, KeyEventKind},
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    },
};

use app::{App, Effect};
use diff::DiffSource;
use diff::git::{DiffSpec, GitSource};
use export::{CommentSink, FileSink, Format};
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

    /// Print this review's comments to stdout as `md` or `json` and exit,
    /// e.g. to pipe them into an agent.
    #[arg(long, value_name = "FORMAT")]
    export: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let source = GitSource::discover(".", DiffSpec::from_args(cli.spec.as_deref(), cli.staged))?;
    // Load before entering the alternate screen so errors print normally.
    let state_file = source.state_file();
    let review = ReviewState::load(&state_file)?;
    let diff = source.load()?;

    if let Some(format) = cli.export {
        let format = Format::parse(&format).ok_or_else(|| anyhow!("unknown export format `{format}`, use md or json"))?;
        let export = export::collect(&source.describe(), &diff, &review);
        print!("{}", export::render(&export, format)?);
        return Ok(());
    }

    let mut app = App::new(source.describe(), diff, review);
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, &state_file);
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal, app: &mut App, state_file: &Path) -> Result<()> {
    let mut keys = KeyHandler::default();
    while !app.should_quit {
        terminal.draw(|frame| ui::render(frame, app, &keys.pending()))?;
        let Event::Key(key) = event::read()? else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if app.is_typing() {
            if let Some(action) = keymap::input_action(key) {
                app.update_input(action);
            }
        } else if let Some(action) = keys.handle(key) {
            app.update(action);
        }

        match app.effect.take() {
            Some(Effect::OpenEditor { initial }) => {
                let text = edit_in_editor(terminal, &initial);
                if let Err(e) = &text {
                    app.message = Some(format!("error: {e:#}"));
                }
                app.finish_editor(text.ok().flatten());
            }
            Some(Effect::Export { format, path }) => {
                let path = path.unwrap_or_else(|| default_export_path(state_file, format));
                let export = export::collect(&app.title, &app.diff, &app.review);
                app.message = Some(match (FileSink { path, format }).send(&export) {
                    Ok(note) => note,
                    Err(e) => format!("error: {e:#}"),
                });
            }
            None => {}
        }

        // A failed save shouldn't end the session; the next change retries.
        if app.review.take_dirty()
            && let Err(e) = app.review.save(state_file)
        {
            app.message = Some(format!("error: could not save review state: {e:#}"));
        }
    }
    Ok(())
}

/// Next to the review state, e.g. `.git/rrrev/worktree-comments.md`, so the
/// export doesn't show up as a change in the diff being reviewed.
fn default_export_path(state_file: &Path, format: Format) -> PathBuf {
    let stem = state_file.file_stem().and_then(|s| s.to_str()).unwrap_or("review");
    state_file.with_file_name(format!("{stem}-comments.{}", format.extension()))
}

/// Hands the terminal to `$VISUAL`/`$EDITOR` on a temp file holding `initial`.
/// Returns the saved text, or `None` if the editor exited with an error
/// (like vim's `:cq`).
fn edit_in_editor(terminal: &mut DefaultTerminal, initial: &str) -> Result<Option<String>> {
    let path = std::env::temp_dir().join(format!("rrrev-comment-{}.md", std::process::id()));
    fs::write(&path, initial)?;
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());

    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    // Through the shell so an editor with arguments (`code --wait`) works;
    // the path goes in as `$1` so it's never parsed as shell syntax.
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(&path)
        .status();
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    terminal.clear()?;

    let status = status.with_context(|| format!("cannot run editor `{editor}`"))?;
    let text = status.success().then(|| fs::read_to_string(&path)).transpose()?;
    let _ = fs::remove_file(&path);
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_land_next_to_the_state_file() {
        let path = default_export_path(Path::new("/repo/.git/rrrev/worktree.json"), Format::Markdown);
        assert_eq!(path, Path::new("/repo/.git/rrrev/worktree-comments.md"));
    }
}
