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
mod markdown;
mod organizer;
mod review;
mod text;
mod theme;
mod ui;
mod walkthrough;

use std::collections::HashMap;
use std::fs;
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
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
use appearance::Appearance;
use ask::Assistant;
use diff::git::{DiffSpec, GitSource};
use diff::{DiffSource, Sources};
use export::{CommentSink, FileSink, Format};
use highlight::Highlighter;
use keymap::KeyHandler;
use organizer::claude::ClaudeCli;
use organizer::{Cancel, Organizer, Plan};
use review::ReviewState;
use walkthrough::{Narrator, Walkthrough};

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
        registry
            .get(name)
            .ok_or_else(|| anyhow!("no language `{name}`; `rrrev lang list` shows them all"))
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
        let format = Format::parse(&format)
            .ok_or_else(|| anyhow!("unknown export format `{format}`, use md or json"))?;
        let export = export::collect(&source.describe(), &diff, &review);
        print!("{}", export::render(&export, format)?);
        return Ok(());
    }

    let mut app = App::new(source.describe(), diff, review);
    app.grammars.status = Some(languages::manage::status);
    let claude = Arc::new(ClaudeCli::from_env());
    let organizer: Arc<dyn Organizer> = claude.clone();
    let narrator: Arc<dyn Narrator> = claude.clone();
    let assistant: Arc<dyn Assistant> = claude;
    app.set_appearance(Appearance::detect());
    app.set_theme(config::theme());
    let highlighter = Highlighter::new(app.theme, app.appearance);
    let mut lsp = lsp::Lsp::new(source.workdir(), languages::Registry::global());
    open_in_servers(&mut lsp, &app);
    let mut terminal = ratatui::init();
    let result = run(
        &mut terminal,
        &mut app,
        &state_file,
        &source,
        Ai {
            organizer,
            narrator,
            assistant,
        },
        highlighter,
        &mut lsp,
    );
    ratatui::restore();
    result
}

/// How often the screen redraws without input, to move the spinner along and
/// pick up background results.
const TICK: Duration = Duration::from_millis(100);

/// What a background organizer run sends back: its job id and its answer.
type PlanResult = (u64, Result<Plan>);
/// What a background walkthrough run sends back: its job id and its answer.
type WalkthroughResult = (u64, Result<Walkthrough>);
/// What a background question sends back: its job id and the answer.
type AskResult = (u64, Result<String>);
/// What a grammar build sends back: the language, and whether it worked.
type BuildResult = (String, Result<()>);

/// The AI behind `:organize`, `:walkthrough` and the ask panel.
struct Ai {
    organizer: Arc<dyn Organizer>,
    narrator: Arc<dyn Narrator>,
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
    source: &GitSource,
    ai: Ai,
    mut highlighter: Highlighter,
    lsp: &mut lsp::Lsp,
) -> Result<()> {
    let mut keys = KeyHandler::default();
    let (plans_tx, plans): (Sender<PlanResult>, Receiver<PlanResult>) = mpsc::channel();
    let (answers_tx, answers): (Sender<AskResult>, Receiver<AskResult>) = mpsc::channel();
    let (walks_tx, walks): (Sender<WalkthroughResult>, Receiver<WalkthroughResult>) =
        mpsc::channel();
    let (builds_tx, builds): (Sender<BuildResult>, Receiver<BuildResult>) = mpsc::channel();
    let mut running: Option<Running> = None;
    let mut asking: Option<Running> = None;
    let mut walking: Option<Running> = None;
    let mut anim = Animator::from_env();
    let appearance = appearance::Watch::start();
    // The question out to a language server, if any: a new one calls off
    // the last, so only the latest answer counts.
    let mut asked: Option<Asked> = None;
    while !app.should_quit {
        app.offer_grammar();
        terminal
            .draw(|frame| ui::render(frame, app, &mut highlighter, &mut anim, &keys.pending()))?;

        if let Some(mode) = appearance.changed() {
            app.set_appearance(mode);
        }
        while let Ok((job, result)) = plans.try_recv() {
            if running.as_ref().is_some_and(|r| r.job == job) {
                running = None;
            }
            app.finish_organize(job, result);
        }
        while let Ok((job, result)) = walks.try_recv() {
            if walking.as_ref().is_some_and(|r| r.job == job) {
                walking = None;
            }
            app.finish_walkthrough(job, result);
        }
        for reply in lsp.poll() {
            let Some(asked) = asked.take_if(|a| (a.server, a.id) == (reply.server, reply.id))
            else {
                continue;
            };
            // An answer that comes after the cursor moved on is dropped,
            // rather than pulling it back from wherever it went.
            if !app.still_at(&asked.from) {
                continue;
            }
            match asked.question {
                Question::GoTo(kind) => {
                    let targets = reply
                        .result
                        .map(|result| targets(lsp, reply.server, &result));
                    app.finish_go_to(kind, targets, asked.note);
                }
                Question::References => {
                    let targets = reply
                        .result
                        .map(|result| targets(lsp, reply.server, &result));
                    app.finish_references(targets, asked.note);
                }
                Question::Hover => {
                    app.finish_hover(reply.result.map(|r| app::hover_text(&r)), asked.note)
                }
            }
        }
        app.lsp_status = lsp.status();
        while let Ok((job, result)) = answers.try_recv() {
            if asking.as_ref().is_some_and(|r| r.job == job) {
                asking = None;
            }
            app.finish_ask(job, result);
        }
        while let Ok((name, result)) = builds.try_recv() {
            if result.is_ok() {
                // What was looked up while it was missing goes, so the
                // next draw uses it.
                languages::grammar::forget(&name);
                highlighter.forget(&app.diff, &name);
            }
            app.finish_enable_language(&name, result);
        }
        if event::poll(if anim.is_animating() {
            anim::FRAME
        } else {
            TICK
        })? && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            if app.language_prompt.is_some() {
                if let Some(build) = keymap::confirm_action(key) {
                    app.answer_enable_language(build);
                }
            } else if app.dismiss_prompt.is_some() {
                if let Some(dismiss) = keymap::confirm_action(key) {
                    app.answer_dismiss_copied(dismiss);
                }
            } else if app.ask.confirm_clear {
                if let Some(clear) = keymap::confirm_action(key) {
                    app.answer_clear_ask(clear);
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
            } else if app.walkthrough.is_some() && app.help_scroll.is_none() {
                // So does the walkthrough.
                if let Some(action) = keymap::walkthrough_action(key) {
                    app.walkthrough_action(action);
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
            Some(Effect::OpenInNeovim {
                path,
                line,
                column,
                contents,
            }) => {
                let file = source.workdir().join(&path);
                // Edits are only worth making to the text on screen: a file
                // on disk that's moved on from it (or a commit that isn't
                // checked out) stays shut.
                if read_source(&file).as_ref() != Some(&contents) {
                    app.message = Some(format!(
                        "{path} on disk isn't the version shown, so there's nothing here to edit"
                    ));
                } else if let Err(e) =
                    open_in_neovim(terminal, source.workdir(), &file, line, column)
                {
                    app.message = Some(format!("error: {e:#}"));
                } else {
                    match source.load() {
                        Ok(diff) => {
                            app.reload(diff, read_source(&file));
                            highlighter.forget_files();
                            lsp.changed(&path);
                            open_in_servers(lsp, app);
                        }
                        Err(e) => app.message = Some(format!("error: could not reload: {e:#}")),
                    }
                }
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
                let (organizer, done, cancel) = (
                    Arc::clone(&ai.organizer),
                    plans_tx.clone(),
                    Cancel::default(),
                );
                let token = cancel.clone();
                let thread = thread::spawn(move || {
                    let _ = done.send((job, organizer.organize(&request, &token)));
                });
                running = Some(Running {
                    job,
                    cancel,
                    thread,
                });
            }
            Some(Effect::CancelOrganize { job }) => {
                // The thread kills the process and exits shortly; the app
                // ignores whatever it sends back.
                if let Some(run) = running.take_if(|r| r.job == job) {
                    run.cancel.cancel();
                }
            }
            Some(Effect::WalkThrough { job, request }) => {
                let (narrator, done, cancel) = (
                    Arc::clone(&ai.narrator),
                    walks_tx.clone(),
                    Cancel::default(),
                );
                let token = cancel.clone();
                let thread = thread::spawn(move || {
                    let _ = done.send((job, narrator.walk_through(&request, &token)));
                });
                walking = Some(Running {
                    job,
                    cancel,
                    thread,
                });
            }
            Some(Effect::CancelWalkThrough { job }) => {
                if let Some(run) = walking.take_if(|r| r.job == job) {
                    run.cancel.cancel();
                }
            }
            Some(Effect::Ask { job, request }) => {
                let (assistant, done, cancel) = (
                    Arc::clone(&ai.assistant),
                    answers_tx.clone(),
                    Cancel::default(),
                );
                let token = cancel.clone();
                let thread = thread::spawn(move || {
                    let _ = done.send((job, assistant.ask(&request, &token)));
                });
                asking = Some(Running {
                    job,
                    cancel,
                    thread,
                });
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
            Some(Effect::GoTo { kind, spot }) => {
                ask(lsp, app, &mut asked, Question::GoTo(kind), &spot)
            }
            Some(Effect::References { spot }) => {
                ask(lsp, app, &mut asked, Question::References, &spot)
            }
            Some(Effect::Hover { spot }) => ask(lsp, app, &mut asked, Question::Hover, &spot),
            Some(Effect::Lsp { restart }) => {
                if restart {
                    asked = None;
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
            Some(Effect::EnableLanguage { name }) => {
                // Fetching and compiling takes a while; the screen carries on.
                // Nobody waits for it on quit: a build cut short leaves nothing
                // in place.
                let done = builds_tx.clone();
                thread::spawn(move || {
                    let result = match languages::Registry::global().get(&name) {
                        Some(language) => languages::manage::enable(language).map(drop),
                        None => Err(anyhow!("no language `{name}`")),
                    };
                    let _ = done.send((name, result));
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
    // Don't leave an AI process running after we're gone.
    for run in [running, asking, walking].into_iter().flatten() {
        run.stop();
    }
    Ok(())
}

/// What a request to a language server was for.
#[derive(Clone, Copy)]
enum Question {
    GoTo(app::GoTo),
    References,
    Hover,
}

impl Question {
    fn method(self) -> &'static str {
        match self {
            Question::GoTo(kind) => kind.method(),
            Question::References => "textDocument/references",
            Question::Hover => "textDocument/hover",
        }
    }
}

/// A question out to a language server: which request it is, what it was
/// for, what to say with the answer, and where the cursor was.
struct Asked {
    server: usize,
    id: u64,
    question: Question,
    note: Option<&'static str>,
    from: app::Place,
}

/// Asks `question` about `spot`, calling off the question still out, if any.
fn ask(
    lsp: &mut lsp::Lsp,
    app: &mut App,
    asked: &mut Option<Asked>,
    question: Question,
    spot: &app::Spot,
) {
    if let Some(old) = asked.take() {
        lsp.server(old.server).cancel(old.id);
    }
    match ask_about(lsp, question.method(), spot) {
        Ok((server, id, note)) => {
            *asked = Some(Asked {
                server,
                id,
                question,
                note,
                from: app.place(),
            })
        }
        Err(why) => app.message = Some(why),
    }
}

/// Sends a request about `spot` (`gd`, `gr`, `gh`…). Returns the server and
/// request id, and a note for when the code on disk isn't what's shown.
fn ask_about(
    lsp: &mut lsp::Lsp,
    method: &str,
    spot: &app::Spot,
) -> Result<(usize, u64, Option<&'static str>), String> {
    let Some(index) = lsp.server_for(&spot.path) else {
        return Err(format!(
            "No language server for {}: {}",
            spot.path,
            lsp.describe()
        ));
    };
    lsp.open(&spot.path);
    let full = lsp.workdir().join(&spot.path);
    let on_disk = fs::read_to_string(&full)
        .ok()
        .and_then(|text| text.lines().nth(spot.line as usize).map(str::to_string));
    let note = (on_disk.as_deref() != Some(spot.text.as_str()))
        .then_some("may be off: this revision isn't the one checked out");
    let server = lsp.server(index);
    let character = lsp::to_character(&spot.text, spot.column, server.encoding);
    let mut params = serde_json::json!({
        "textDocument": {"uri": lsp::uri(&full)},
        "position": {"line": spot.line, "character": character},
    });
    if method == "textDocument/references" {
        params["context"] = serde_json::json!({"includeDeclaration": true});
    }
    let id = server.request(method, params);
    Ok((index, id, note))
}

/// The places in a server's answer, with each file's text read from disk.
fn targets(lsp: &mut lsp::Lsp, server: usize, result: &serde_json::Value) -> Vec<app::Target> {
    let encoding = lsp.server(server).encoding;
    let workdir = lsp.workdir().to_path_buf();
    let mut files: HashMap<PathBuf, Option<diff::Source>> = HashMap::new();
    lsp::locations(result)
        .into_iter()
        .filter_map(|(path, line, character)| {
            let lines = files
                .entry(path.clone())
                .or_insert_with(|| {
                    let text = fs::read_to_string(&path).ok()?;
                    Some(Arc::new(text.lines().map(str::to_string).collect()))
                })
                .clone()?;
            let column = lines
                .get(line as usize)
                .map_or(0, |l| lsp::to_byte(l, character, encoding));
            let shown = path
                .strip_prefix(&workdir)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            Some(app::Target {
                path: shown,
                line: line as usize,
                column,
                lines,
            })
        })
        .collect()
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
    let stem = state_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("review");
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

    // Through the shell so an editor with arguments (`code --wait`) works;
    // the path goes in as `$1` so it's never parsed as shell syntax.
    let status = hand_over(
        terminal,
        Command::new("sh")
            .arg("-c")
            .arg(format!("{editor} \"$1\""))
            .arg("sh")
            .arg(&path),
    )?
    .with_context(|| format!("cannot run editor `{editor}`"))?;
    let text = status
        .success()
        .then(|| fs::read_to_string(&path))
        .transpose()?;
    let _ = fs::remove_file(&path);
    Ok(text)
}

/// Runs `command` in rrrev's terminal, the screen handed over until it
/// exits and taken back after. The outer `Result` is the terminal's, the
/// inner one whether `command` could run at all.
fn hand_over(
    terminal: &mut DefaultTerminal,
    command: &mut Command,
) -> Result<std::io::Result<ExitStatus>> {
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    let status = command.status();
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    terminal.clear()?;
    Ok(status)
}

/// Opens `file` in Neovim at `line` and `column`, from `workdir`.
fn open_in_neovim(
    terminal: &mut DefaultTerminal,
    workdir: &Path,
    file: &Path,
    line: u32,
    column: usize,
) -> Result<()> {
    hand_over(
        terminal,
        Command::new("nvim")
            .current_dir(workdir)
            .arg(format!("+call cursor({line}, {column})"))
            .arg("--")
            .arg(file),
    )?
    .context("cannot run `nvim`")?;
    Ok(())
}

/// A file's text as the diff splits it, or `None` if it can't be read.
fn read_source(path: &Path) -> Option<diff::Source> {
    fs::read(path).ok().map(|bytes| Sources::lines(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_land_next_to_the_state_file() {
        let path = default_export_path(
            Path::new("/repo/.git/rrrev/worktree.json"),
            Format::Markdown,
        );
        assert_eq!(path, Path::new("/repo/.git/rrrev/worktree-comments.md"));
    }
}
