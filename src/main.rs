mod anim;
mod app;
mod appearance;
mod ask;
mod clipboard;
mod config;
mod diff;
mod export;
mod help;
mod highlight;
mod input;
mod keymap;
mod languages;
mod lsp;
mod organizer;
mod review;
mod text;
mod theme;
mod ui;

use std::fs;
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use ratatui::{
    DefaultTerminal,
    crossterm::{
        event::{self, Event, KeyEventKind},
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    },
};

use anim::Animator;
use app::{App, Effect};
use ask::Assistant;
use diff::DiffSource;
use diff::git::{DiffSpec, GitSource};
use export::{CommentSink, FileSink, Format};
use appearance::Appearance;
use highlight::Highlighter;
use keymap::KeyHandler;
use organizer::claude::ClaudeCli;
use organizer::{Cancel, Organizer, Plan};
use review::ReviewState;

/// Keyboard-driven code review in the terminal.
#[derive(Parser)]
#[command(version, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Mode>,

    /// What to review, in git's own syntax: `<rev>`, `a..b` or `a...b`. The
    /// subcommands say the same in plain words. Defaults to everything not
    /// committed yet, new files included.
    spec: Option<String>,

    /// Review only the changes staged with `git add`.
    #[arg(long, conflicts_with = "spec")]
    staged: bool,

    /// Print this review's comments to stdout as `md` or `json` and exit,
    /// e.g. to pipe them into an agent.
    #[arg(long, value_name = "FORMAT", global = true)]
    export: Option<String>,
}

#[derive(Subcommand)]
enum Mode {
    /// Review the last commit, or the last COUNT commits together.
    Last {
        #[arg(default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        count: u32,
    },
    /// Review what one commit changed.
    Commit {
        /// A commit id, branch or tag.
        commit: String,
    },
    /// Review the changes from one commit to another.
    Between {
        /// Where to start: a commit id, branch or tag.
        from: String,
        /// Where to end: a commit id, branch or tag.
        to: String,
    },
    /// Review everything since a commit, uncommitted changes included.
    Since {
        /// A commit id, branch or tag.
        commit: String,
    },
    /// Review what this branch adds since it split off another one,
    /// uncommitted changes included.
    Branch {
        /// The branch it split off. Defaults to `main`, or `master`.
        base: Option<String>,
    },
    /// Review the current branch's stack (`gh stack`) one layer per group,
    /// each layer against the one below, plus what isn't committed yet.
    Stack,
    /// Turn languages' highlighting and outlines on or off, and list them.
    Lang {
        #[command(subcommand)]
        command: LangCommand,
    },
}

#[derive(Subcommand)]
enum LangCommand {
    /// Every language rrrev knows, whether its grammar is enabled and whether
    /// its LSP server is in Mason's folder.
    List,
    /// Fetch and build languages' grammars, with git and the C compiler.
    Enable {
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Delete languages' built grammars.
    Disable {
        #[arg(required = true)]
        names: Vec<String>,
    },
}

fn lang(command: LangCommand) -> Result<()> {
    let registry = languages::Registry::global();
    let find = |name: &str| {
        registry.get(name).ok_or_else(|| anyhow!("no language `{name}`; `rrrev lang list` shows them all"))
    };
    match command {
        LangCommand::List => print!("{}", languages::manage::list(registry)),
        LangCommand::Enable { names } => {
            for name in names {
                let language = find(&name)?;
                println!("Building {name}…");
                println!("{}", languages::manage::enable(language)?);
            }
        }
        LangCommand::Disable { names } => {
            for name in names {
                println!("{}", languages::manage::disable(find(&name)?)?);
            }
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // A broken languages.toml should say so before the screen is taken over.
    languages::Registry::install(languages::Registry::load()?);
    let source = match cli.command {
        Some(Mode::Last { count }) => GitSource::discover(".", DiffSpec::Last(count))?,
        Some(Mode::Commit { commit }) => GitSource::discover(".", DiffSpec::Commit(commit))?,
        Some(Mode::Between { from, to }) => GitSource::discover(".", DiffSpec::Range { from, to })?,
        Some(Mode::Since { commit }) => GitSource::discover(".", DiffSpec::RevToWorkTree(commit))?,
        Some(Mode::Branch { base }) => GitSource::branch(".", base)?,
        Some(Mode::Stack) => GitSource::stack(".")?,
        Some(Mode::Lang { command }) => return lang(command),
        None => GitSource::discover(".", DiffSpec::from_args(cli.spec.as_deref(), cli.staged))?,
    };
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

    let hint = languages::manage::hint(languages::Registry::global(), diff.files.iter().map(|f| f.path()));
    let mut app = App::new(source.describe(), diff, review);
    app.message = hint;
    let claude = Arc::new(ClaudeCli::from_env());
    let organizer: Arc<dyn Organizer> = claude.clone();
    let assistant: Arc<dyn Assistant> = claude;
    app.set_appearance(Appearance::detect());
    app.set_theme(config::theme());
    let highlighter = Highlighter::new(app.theme, app.appearance);
    let mut lsp = lsp::Lsp::new(source.workdir(), languages::Registry::global());
    open_in_servers(&mut lsp, &app);
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, &state_file, Ai { organizer, assistant }, highlighter, &mut lsp);
    ratatui::restore();
    result
}

/// How often the screen redraws without input, to move the spinner along and
/// pick up background results.
const TICK: Duration = Duration::from_millis(100);

/// What a background organizer run sends back: its job id and its answer.
type PlanResult = (u64, Result<Plan>);
/// What a background question sends back: its job id and the answer.
type AskResult = (u64, Result<String>);

/// The AI behind `:organize` and the ask panel.
struct Ai {
    organizer: Arc<dyn Organizer>,
    assistant: Arc<dyn Assistant>,
}

/// A background AI run on its own thread.
struct Running {
    job: u64,
    cancel: Cancel,
    thread: thread::JoinHandle<()>,
}

impl Running {
    fn stop(self) {
        self.cancel.cancel();
        let _ = self.thread.join();
    }
}

fn run(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    state_file: &Path,
    ai: Ai,
    mut highlighter: Highlighter,
    lsp: &mut lsp::Lsp,
) -> Result<()> {
    let mut keys = KeyHandler::default();
    let (plans_tx, plans): (Sender<PlanResult>, Receiver<PlanResult>) = mpsc::channel();
    let (answers_tx, answers): (Sender<AskResult>, Receiver<AskResult>) = mpsc::channel();
    let mut running: Option<Running> = None;
    let mut asking: Option<Running> = None;
    let mut anim = Animator::from_env();
    let appearance = appearance::Watch::start();
    while !app.should_quit {
        terminal.draw(|frame| ui::render(frame, app, &mut highlighter, &mut anim, &keys.pending()))?;

        if let Some(mode) = appearance.changed() {
            app.set_appearance(mode);
        }
        while let Ok((job, result)) = plans.try_recv() {
            if running.as_ref().is_some_and(|r| r.job == job) {
                running = None;
            }
            app.finish_organize(job, result);
        }
        // Replies to rrrev's own requests come with go to definition.
        let _ = lsp.poll();
        app.lsp_status = lsp.status();
        while let Ok((job, result)) = answers.try_recv() {
            if asking.as_ref().is_some_and(|r| r.job == job) {
                asking = None;
            }
            app.finish_ask(job, result);
        }
        if event::poll(if anim.is_animating() { anim::FRAME } else { TICK })?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            if app.dismiss_prompt.is_some() {
                if let Some(dismiss) = keymap::confirm_action(key) {
                    app.answer_dismiss_copied(dismiss);
                }
            } else if app.is_typing() {
                if let Some(action) = keymap::input_action(key) {
                    app.update_input(action);
                }
            } else if app.show_verdict && app.help_scroll.is_none() {
                // The verdict page has its own keys, but `?` still opens the help.
                if let Some(action) = keymap::verdict_action(key) {
                    app.verdict_action(action);
                } else if let Some(action) = keys.handle(key) {
                    app.update(action);
                }
            } else if let Some(action) = keys.handle(key) {
                app.update(action);
            }
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
            Some(Effect::Organize { job, request }) => {
                let (organizer, done, cancel) = (Arc::clone(&ai.organizer), plans_tx.clone(), Cancel::default());
                let token = cancel.clone();
                let thread = thread::spawn(move || {
                    let _ = done.send((job, organizer.organize(&request, &token)));
                });
                running = Some(Running { job, cancel, thread });
            }
            Some(Effect::CancelOrganize { job }) => {
                // The thread kills the process and exits shortly; the app
                // ignores whatever it sends back.
                if let Some(run) = running.take_if(|r| r.job == job) {
                    run.cancel.cancel();
                }
            }
            Some(Effect::Ask { job, request }) => {
                let (assistant, done, cancel) = (Arc::clone(&ai.assistant), answers_tx.clone(), Cancel::default());
                let token = cancel.clone();
                let thread = thread::spawn(move || {
                    let _ = done.send((job, assistant.ask(&request, &token)));
                });
                asking = Some(Running { job, cancel, thread });
            }
            Some(Effect::CancelAsk { job }) => {
                if let Some(run) = asking.take_if(|r| r.job == job) {
                    run.cancel.cancel();
                }
            }
            Some(Effect::SaveTheme { theme }) => {
                if let Err(e) = config::save_theme(theme) {
                    app.message = Some(format!("error: could not save the theme: {e:#}"));
                }
            }
            Some(Effect::Lsp { restart }) => {
                if restart {
                    lsp.stop_all();
                    open_in_servers(lsp, app);
                }
                app.message = Some(lsp.describe());
            }
            Some(Effect::Copy { text }) => {
                if let Err(e) = clipboard::copy(&text) {
                    app.message = Some(format!("error: could not copy: {e:#}"));
                    // Nothing was handed over, so nothing to dismiss.
                    app.dismiss_prompt = None;
                }
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
    // Don't leave an AI process running after we're gone.
    for run in [running, asking].into_iter().flatten() {
        run.stop();
    }
    Ok(())
}

/// Tells the language servers about every file in the diff that's still
/// there, starting the servers they need.
fn open_in_servers(lsp: &mut lsp::Lsp, app: &App) {
    for file in &app.diff.files {
        if file.new_path.is_some() {
            lsp.open(file.path());
        }
    }
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
