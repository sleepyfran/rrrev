//! Noticing the diff and the open file change while rrrev runs, so the
//! review follows edits made in another window. A thread of its own hears
//! from the OS when something in the repository changes, waits for the
//! burst of changes to settle, loads the diff again and sends it over only
//! when it looks different. Where the OS can't say, it loads every so often
//! instead. The file view's one file is checked on disk by its size and
//! time, which also covers a file outside the repository or one git ignores.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use notify::event::{AccessKind, AccessMode};
use notify::{Event, EventKind, RecursiveMode, Watcher};

use crate::diff::git::GitSource;
use crate::diff::{Diff, DiffSource, Source, Sources};

/// How long the diff goes between loads when the OS can't say what changed.
const POLL_EVERY: Duration = Duration::from_secs(1);
/// How long the diff goes between loads even when the OS says nothing, in
/// case it missed something.
const CHECK_EVERY: Duration = Duration::from_secs(30);
/// How long a burst of changes, like a save or a checkout, has to go quiet
/// before the diff is loaded.
const QUIET: Duration = Duration::from_millis(150);
/// The longest a burst that won't go quiet, like a build, holds a load up.
const SETTLE_AT_MOST: Duration = Duration::from_secs(1);
/// How long between looks at the open file on disk.
const FILE_EVERY: Duration = Duration::from_millis(500);

/// The diff, loaded again in the background. Its thread stops once this is
/// dropped and it has something to send.
pub struct DiffWatch(Option<Receiver<Diff>>);

impl DiffWatch {
    /// Starts watching from `shown`, the diff on screen now, so only what
    /// changes after it comes back. Without a second handle on the
    /// repository nothing is watched.
    pub fn start(source: &GitSource, shown: &Diff) -> Self {
        let Ok(source) = source.reopen() else {
            return Self(None);
        };
        let (tx, rx) = mpsc::channel();
        let loader = Loader {
            source,
            last: shown.fingerprint(),
            tx,
        };
        thread::spawn(move || run(loader));
        Self(Some(rx))
    }

    /// The newest diff seen since the last call, if it changed.
    pub fn changed(&self) -> Option<Diff> {
        self.0.as_ref()?.try_iter().last()
    }
}

/// Loads the diff and sends it on when it changed.
struct Loader {
    source: GitSource,
    /// The fingerprint of the diff last sent, or of the one shown at start.
    last: u64,
    tx: Sender<Diff>,
}

impl Loader {
    /// Whether anyone's still listening. A load that fails, as one may
    /// halfway through a checkout, is tried again next time.
    fn load(&mut self) -> bool {
        let Ok(diff) = self.source.load() else {
            return true;
        };
        let print = diff.fingerprint();
        if print == self.last {
            return true;
        }
        self.last = print;
        self.tx.send(diff).is_ok()
    }
}

/// The watcher thread: on the OS's word while it gives it, then by polling.
fn run(mut loader: Loader) {
    // The watcher lives as long as its events are followed; when they stop,
    // or never started, polling takes over.
    if let Some((_watcher, roots, events)) = listen(&loader.source)
        && !follow(&mut loader, &roots, &events)
    {
        return;
    }
    while loader.load() {
        thread::sleep(POLL_EVERY);
    }
}

type Events = Receiver<notify::Result<Event>>;

/// Asks the OS to report changes in the working folder and git's own
/// folders, or `None` if it can't.
fn listen(source: &GitSource) -> Option<(notify::RecommendedWatcher, Roots, Events)> {
    let roots = Roots::of(source);
    let (tx, events) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx).ok()?;
    watcher
        .watch(&roots.workdir, RecursiveMode::Recursive)
        .ok()?;
    for dir in &roots.git_dirs {
        if !dir.starts_with(&roots.workdir) {
            watcher.watch(dir, RecursiveMode::Recursive).ok()?;
        }
    }
    Some((watcher, roots, events))
}

/// Loads the diff whenever a change that matters settles, and now and then
/// regardless. `false` once no one's listening; `true` if the OS stopped
/// reporting, for polling to take over.
fn follow(loader: &mut Loader, roots: &Roots, events: &Events) -> bool {
    // Once first, for what changed between the diff shown being loaded and
    // the OS starting to report.
    if !loader.load() {
        return false;
    }
    loop {
        match events.recv_timeout(CHECK_EVERY) {
            Ok(event) if !matters(&event, roots, |p| loader.source.is_ignored(p)) => continue,
            Ok(_) => settle(events),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return true,
        }
        if !loader.load() {
            return false;
        }
    }
}

/// Waits for a burst of changes to go quiet, or for long enough.
fn settle(events: &Events) {
    let start = Instant::now();
    while start.elapsed() < SETTLE_AT_MOST && events.recv_timeout(QUIET).is_ok() {}
}

/// Where the changes that matter happen, as the OS names them.
struct Roots {
    workdir: PathBuf,
    git_dirs: Vec<PathBuf>,
}

impl Roots {
    fn of(source: &GitSource) -> Self {
        // As the OS reports them: macOS names `/var` `/private/var`, say.
        let real = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        Self {
            workdir: real(source.workdir()),
            git_dirs: source.git_dirs().iter().map(|d| real(d)).collect(),
        }
    }
}

/// Whether `event` could change the diff: an edit to a file git doesn't
/// ignore, or to what's checked out, staged or on a branch. Not reads,
/// nor git's own bookkeeping, like rrrev's saved review in `.git`. When
/// it's unclear, it does.
fn matters(event: &notify::Result<Event>, roots: &Roots, ignored: impl Fn(&Path) -> bool) -> bool {
    let Ok(event) = event else {
        return true;
    };
    if event.need_rescan() {
        return true;
    }
    if let EventKind::Access(kind) = event.kind
        && kind != AccessKind::Close(AccessMode::Write)
    {
        return false;
    }
    if event.paths.is_empty() {
        return true;
    }
    event.paths.iter().any(|path| {
        if let Some(inside) = roots
            .git_dirs
            .iter()
            .find_map(|dir| path.strip_prefix(dir).ok())
        {
            return inside == Path::new("HEAD")
                || inside == Path::new("index")
                || inside == Path::new("packed-refs")
                || inside.starts_with("refs");
        }
        match path.strip_prefix(&roots.workdir) {
            Ok(inside) => inside.as_os_str().is_empty() || !ignored(inside),
            Err(_) => true,
        }
    })
}

/// The file open in the file view, as last seen on disk.
#[derive(Default)]
pub struct FileWatch {
    /// The file, relative to the working folder.
    path: Option<String>,
    stamp: Option<Stamp>,
    /// Whether the view showed what was on disk when it was first looked
    /// at. One that didn't (the old side, or a commit not checked out)
    /// isn't about the working tree, so the disk changing is no news to it.
    live: bool,
    checked: Option<Instant>,
}

/// What changes about a file when it's written.
type Stamp = (u64, Option<SystemTime>);

/// What became of the watched file.
pub enum FileChange {
    /// Its new text.
    Edited(Source),
    Gone,
}

impl FileWatch {
    /// Looks at `path` on disk, now and then: what it holds if that changed
    /// since the last look and isn't `shown` already. `None` stops watching.
    pub fn check(
        &mut self,
        workdir: &Path,
        file: Option<(&str, &Source)>,
        now: Instant,
    ) -> Option<FileChange> {
        let Some((path, shown)) = file else {
            *self = Self::default();
            return None;
        };
        let full = workdir.join(path);
        if self.path.as_deref() != Some(path) {
            let disk = read(&full);
            *self = Self {
                path: Some(path.to_string()),
                stamp: stamp(&full),
                live: disk.as_ref() == Some(shown),
                checked: Some(now),
            };
            return None;
        }
        if !self.live
            || self
                .checked
                .is_some_and(|t| now.duration_since(t) < FILE_EVERY)
        {
            return None;
        }
        self.checked = Some(now);
        let stamp = stamp(&full);
        if stamp == self.stamp {
            return None;
        }
        self.stamp = stamp;
        match read(&full) {
            None => Some(FileChange::Gone),
            Some(lines) if lines == *shown => None,
            Some(lines) => Some(FileChange::Edited(lines)),
        }
    }
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

/// A file's text as the diff splits it, or `None` if it can't be read.
pub fn read(path: &Path) -> Option<Source> {
    fs::read(path).ok().map(|bytes| Sources::lines(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::git::DiffSpec;
    use notify::event::{DataChange, Flag, ModifyKind};

    fn lines(text: &str) -> Source {
        Sources::lines(text.as_bytes())
    }

    #[test]
    fn hears_of_edits_to_a_file_shown_as_it_is_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "one\n").unwrap();
        let mut watch = FileWatch::default();
        let shown = lines("one\n");
        let t = Instant::now();
        assert!(watch.check(dir.path(), Some(("a.rs", &shown)), t).is_none());

        fs::write(dir.path().join("a.rs"), "one\ntwo\n").unwrap();
        let later = t + FILE_EVERY;
        match watch.check(dir.path(), Some(("a.rs", &shown)), later) {
            Some(FileChange::Edited(new)) => assert_eq!(*new, ["one", "two"]),
            _ => panic!("expected the edit"),
        }

        fs::remove_file(dir.path().join("a.rs")).unwrap();
        let later = later + FILE_EVERY;
        assert!(matches!(
            watch.check(dir.path(), Some(("a.rs", &shown)), later),
            Some(FileChange::Gone)
        ));
    }

    #[test]
    fn a_file_not_shown_as_on_disk_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "now\n").unwrap();
        let mut watch = FileWatch::default();
        let old = lines("before\n");
        let t = Instant::now();
        assert!(watch.check(dir.path(), Some(("a.rs", &old)), t).is_none());
        fs::write(dir.path().join("a.rs"), "later\n").unwrap();
        assert!(
            watch
                .check(dir.path(), Some(("a.rs", &old)), t + FILE_EVERY)
                .is_none()
        );
    }

    fn roots() -> Roots {
        Roots {
            workdir: "/repo".into(),
            git_dirs: vec!["/repo/.git".into()],
        }
    }

    fn edit(path: &str) -> notify::Result<Event> {
        Ok(
            Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
                .add_path(path.into()),
        )
    }

    #[test]
    fn edits_matter_unless_git_ignores_them() {
        let ignored = |p: &Path| p.starts_with("target");
        assert!(matters(&edit("/repo/src/a.rs"), &roots(), ignored));
        assert!(!matters(
            &edit("/repo/target/debug/rrrev"),
            &roots(),
            ignored
        ));
    }

    #[test]
    fn only_what_is_checked_out_staged_or_on_a_branch_matters_in_git() {
        let none = |_: &Path| false;
        for path in ["HEAD", "index", "packed-refs", "refs/heads/main"] {
            assert!(
                matters(&edit(&format!("/repo/.git/{path}")), &roots(), none),
                "{path}"
            );
        }
        for path in [
            "objects/ab/cdef",
            "rrrev/worktree.json",
            "logs/HEAD",
            "index.lock",
        ] {
            assert!(
                !matters(&edit(&format!("/repo/.git/{path}")), &roots(), none),
                "{path}"
            );
        }
    }

    #[test]
    fn reads_do_not_matter_but_a_finished_write_and_a_rescan_do() {
        let none = |_: &Path| false;
        let access = |kind| Ok(Event::new(EventKind::Access(kind)).add_path("/repo/a.rs".into()));
        assert!(!matters(&access(AccessKind::Read), &roots(), none));
        assert!(matters(
            &access(AccessKind::Close(AccessMode::Write)),
            &roots(),
            none
        ));
        let rescan = Ok(Event::new(EventKind::Other).set_flag(Flag::Rescan));
        assert!(matters(&rescan, &roots(), none));
        assert!(matters(
            &Err(notify::Error::generic("lost")),
            &roots(),
            none
        ));
    }

    #[test]
    fn hears_of_an_edit_from_the_os() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        fs::write(dir.path().join(".gitignore"), "build/\n").unwrap();
        fs::write(dir.path().join("a.rs"), "one\n").unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "start", &tree, &[])
            .unwrap();

        let source = GitSource::discover(dir.path(), DiffSpec::WorkTree).unwrap();
        // The OS reports here, so it isn't the polling that does it.
        assert!(listen(&source).is_some());
        let watch = DiffWatch::start(&source, &source.load().unwrap());
        // Time for the OS to start reporting.
        thread::sleep(Duration::from_millis(300));
        fs::write(dir.path().join("a.rs"), "one\ntwo\n").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let diff = loop {
            if let Some(diff) = watch.changed() {
                break diff;
            }
            assert!(Instant::now() < deadline, "no change heard");
            thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].path(), "a.rs");
    }
}
