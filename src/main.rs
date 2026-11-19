mod anim;
mod app;
mod ask;
mod diff;
mod export;
mod help;
mod highlight;
mod input;
mod keymap;
mod organizer;
mod review;
mod text;
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
use clap::Parser;
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
use highlight::{Background, Highlighter};
use keymap::KeyHandler;
use organizer::claude::ClaudeCli;
use organizer::{Cancel, Organizer, Plan};
use review::ReviewState;

/// Keyboard-driven code review in the terminal.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// What to review: `<rev>` (vs the working tree), `a..b`, or `a...b` (from
    /// the merge base). Defaults to HEAD vs the working tree, untracked files
    /// included. A range from a stack's trunk to one of its branches is
    /// reviewed layer by layer.
    spec: Option<String>,

    /// Review staged changes only (HEAD vs index).
    #[arg(long, conflicts_with = "spec")]
    staged: bool,

    /// Review the current branch's stack (`gh stack`), one layer per group.
    #[arg(long, conflicts_with_all = ["spec", "staged"])]
    stack: bool,

    /// Print this review's comments to stdout as `md` or `json` and exit,
    /// e.g. to pipe them into an agent.
    #[arg(long, value_name = "FORMAT")]
    export: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let source = GitSource::discover(".", DiffSpec::from_args(cli.spec.as_deref(), cli.staged))?.with_stack(cli.stack)?;
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
    let claude = Arc::new(ClaudeCli::from_env());
    let organizer: Arc<dyn Organizer> = claude.clone();
    let assistant: Arc<dyn Assistant> = claude;
    let highlighter = Highlighter::new(Background::detect());
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, &state_file, Ai { organizer, assistant }, highlighter);
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
) -> Result<()> {
    let mut keys = KeyHandler::default();
    let (plans_tx, plans): (Sender<PlanResult>, Receiver<PlanResult>) = mpsc::channel();
    let (answers_tx, answers): (Sender<AskResult>, Receiver<AskResult>) = mpsc::channel();
    let mut running: Option<Running> = None;
    let mut asking: Option<Running> = None;
    let mut anim = Animator::from_env();
    while !app.should_quit {
        terminal.draw(|frame| ui::render(frame, app, &mut highlighter, &mut anim, &keys.pending()))?;

        while let Ok((job, result)) = plans.try_recv() {
            if running.as_ref().is_some_and(|r| r.job == job) {
                running = None;
            }
            app.finish_organize(job, result);
        }
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
            if app.is_typing() {
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
            Some(Effect::Copy { text }) => {
                if let Err(e) = copy_to_clipboard(&text) {
                    app.message = Some(format!("error: could not copy: {e:#}"));
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

/// Puts `text` on the clipboard with the OSC 52 escape, which terminals
/// (and tmux, with `set-clipboard on`) pass to the system clipboard, over
/// SSH too.
fn copy_to_clipboard(text: &str) -> Result<()> {
    use std::io::Write;
    let mut out = stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    out.flush()?;
    Ok(())
}

/// Standard base64, padded.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
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
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64("héllo\n".as_bytes()), "aMOpbGxvCg==");
    }

    #[test]
    fn exports_land_next_to_the_state_file() {
        let path = default_export_path(Path::new("/repo/.git/rrrev/worktree.json"), Format::Markdown);
        assert_eq!(path, Path::new("/repo/.git/rrrev/worktree-comments.md"));
    }
}
