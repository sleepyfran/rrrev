//! One running language server: its process, a thread writing to it, a
//! thread reading from it, and what rrrev knows of its state.

use std::collections::HashMap;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{Event, transport, uri};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// `initialize` is out; notifications wait until it's answered.
    Starting,
    Ready,
    /// The connection closed; why, if rrrev knows.
    Exited(String),
}

/// Which unit a server counts columns in, as agreed at `initialize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16,
}

pub struct Server {
    /// What it's called, for the status line: the command's file name.
    pub name: String,
    pub root: PathBuf,
    pub state: State,
    pub encoding: Encoding,
    /// Work it reports with `$/progress`, by token: title, message, percent.
    pub progress: HashMap<String, (String, Option<String>, Option<u64>)>,
    outgoing: Sender<Value>,
    child: Option<Child>,
    next_id: u64,
    /// Requests out, by id: their method.
    pub pending: HashMap<u64, String>,
    /// Requests that were still out when the connection closed.
    abandoned: Vec<u64>,
    /// Notifications held until `initialize` is answered.
    queued: Vec<Value>,
    /// Files sent with `didOpen`, and the version of their text last sent.
    opened: HashMap<PathBuf, i32>,
    initialize: u64,
}

impl Server {
    /// Starts `command` in `root`, with its stderr in `log`.
    pub fn spawn(
        id: usize,
        lang: &str,
        command: &Path,
        args: &[String],
        root: &Path,
        log: &Path,
        events: Sender<Event>,
    ) -> Result<Server> {
        let stderr = std::fs::File::create(log)
            .map(Stdio::from)
            .unwrap_or_else(|_| Stdio::null());
        let mut child = Command::new(command)
            .args(args)
            .current_dir(root)
            .env("PATH", path_with_mason())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .with_context(|| format!("cannot start {}", command.display()))?;
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let name = command
            .file_name()
            .map_or_else(|| lang.to_string(), |n| n.to_string_lossy().into_owned());
        Ok(Server::connect(
            id,
            &name,
            root,
            stdout,
            stdin,
            Some(child),
            events,
        ))
    }

    /// A server over any pipe pair, as tests use. Sends `initialize`.
    #[allow(clippy::too_many_arguments)]
    pub fn connect(
        id: usize,
        name: &str,
        root: &Path,
        input: impl Read + Send + 'static,
        mut output: impl Write + Send + 'static,
        child: Option<Child>,
        events: Sender<Event>,
    ) -> Server {
        let (outgoing, queue) = mpsc::channel::<Value>();
        thread::spawn(move || {
            for message in queue {
                if transport::write(&mut output, &message).is_err() {
                    break;
                }
            }
        });
        thread::spawn(move || {
            let mut input = BufReader::new(input);
            let why = loop {
                match transport::read(&mut input) {
                    Ok(Some(message)) => {
                        if events
                            .send(Event::Message {
                                server: id,
                                message,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Ok(None) => break "it exited".to_string(),
                    Err(e) => break format!("it sent something unreadable: {e}"),
                }
            };
            let _ = events.send(Event::Closed { server: id, why });
        });
        let mut server = Server {
            name: name.to_string(),
            root: root.to_path_buf(),
            state: State::Starting,
            encoding: Encoding::Utf16,
            progress: HashMap::new(),
            outgoing,
            child,
            next_id: 1,
            pending: HashMap::new(),
            abandoned: Vec::new(),
            queued: Vec::new(),
            opened: HashMap::new(),
            initialize: 0,
        };
        server.initialize = server.request("initialize", initialize_params(root));
        server
    }

    /// Sends a request; returns its id.
    pub fn request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, method.to_string());
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        // Only `initialize` may go before the server has answered it.
        if self.state == State::Starting && method != "initialize" {
            self.queued.push(message);
        } else {
            let _ = self.outgoing.send(message);
        }
        id
    }

    /// Tells the server rrrev no longer wants the answer to request `id`,
    /// which won't come back as a reply.
    pub fn cancel(&mut self, id: u64) {
        if self.pending.remove(&id).is_some() {
            self.notify("$/cancelRequest", json!({"id": id}));
        }
    }

    /// Sends a notification, or holds it until the server is initialized.
    pub fn notify(&mut self, method: &str, params: Value) {
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        match self.state {
            State::Starting => self.queued.push(message),
            State::Ready => {
                let _ = self.outgoing.send(message);
            }
            State::Exited(_) => {}
        }
    }

    /// Tells the server about a file, once, with its text from disk.
    pub fn open(&mut self, path: &Path, language_id: &str) {
        if self.opened.contains_key(path) {
            return;
        }
        self.opened.insert(path.to_path_buf(), 1);
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri(path), "languageId": language_id, "version": 1, "text": text}}),
        );
    }

    /// Sends a file's new text from disk, if the server has it open. The
    /// whole text, which every server takes whatever sync it asked for.
    pub fn changed(&mut self, path: &Path) {
        let Some(version) = self.opened.get_mut(path) else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        *version += 1;
        let version = *version;
        self.notify(
            "textDocument/didChange",
            json!({"textDocument": {"uri": uri(path), "version": version}, "contentChanges": [{"text": text}]}),
        );
    }

    /// Handles one message from the server. Returns a response to one of
    /// rrrev's requests, other than `initialize`: its id, method and result
    /// (or error message).
    pub fn receive(&mut self, message: Value) -> Option<(u64, String, Result<Value, String>)> {
        let method = message.get("method").and_then(Value::as_str);
        match (method, message.get("id")) {
            // A request from the server.
            (Some(method), Some(id)) => {
                let result = match method {
                    "workspace/configuration" => {
                        let items = message["params"]["items"].as_array().map_or(0, Vec::len);
                        Ok(Value::Array(vec![Value::Null; items]))
                    }
                    "window/workDoneProgress/create"
                    | "client/registerCapability"
                    | "client/unregisterCapability" => Ok(Value::Null),
                    "workspace/workspaceFolders" => Ok(json!([folder(&self.root)])),
                    _ => Err(method.to_string()),
                };
                let reply = match result {
                    Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                    Err(method) => json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32601, "message": format!("rrrev doesn't handle {method}")}}),
                };
                let _ = self.outgoing.send(reply);
                None
            }
            (Some("$/progress"), None) => {
                self.progress_update(&message["params"]);
                None
            }
            (Some(_), None) => None,
            // A response to rrrev.
            (None, Some(id)) => {
                let id = id.as_u64()?;
                let method = self.pending.remove(&id)?;
                let result = match message.get("error") {
                    Some(error) => Err(error["message"].as_str().unwrap_or("error").to_string()),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                if id == self.initialize {
                    self.initialized(result);
                    return None;
                }
                Some((id, method, result))
            }
            (None, None) => None,
        }
    }

    fn initialized(&mut self, result: Result<Value, String>) {
        match result {
            Ok(result) => {
                if result["capabilities"]["positionEncoding"] == "utf-8" {
                    self.encoding = Encoding::Utf8;
                }
                self.state = State::Ready;
                let _ = self
                    .outgoing
                    .send(json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));
                for message in std::mem::take(&mut self.queued) {
                    let _ = self.outgoing.send(message);
                }
            }
            Err(e) => self.closed(format!("it refused to start: {e}")),
        }
    }

    fn progress_update(&mut self, params: &Value) {
        let token = match &params["token"] {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let value = &params["value"];
        let message = value["message"].as_str().map(str::to_string);
        let percent = value["percentage"].as_u64();
        match value["kind"].as_str() {
            Some("begin") => {
                let title = value["title"].as_str().unwrap_or("working").to_string();
                self.progress.insert(token, (title, message, percent));
            }
            Some("report") => {
                if let Some(entry) = self.progress.get_mut(&token) {
                    entry.1 = message.or(entry.1.take());
                    entry.2 = percent.or(entry.2);
                }
            }
            Some("end") => {
                self.progress.remove(&token);
            }
            _ => {}
        }
    }

    /// The connection is gone.
    pub fn closed(&mut self, why: String) {
        if !matches!(self.state, State::Exited(_)) {
            self.state = State::Exited(why);
        }
        self.progress.clear();
        let initialize = self.initialize;
        self.abandoned.extend(
            self.pending
                .drain()
                .map(|(id, _)| id)
                .filter(|&id| id != initialize),
        );
    }

    /// The requests that went unanswered when the connection closed, once.
    pub fn take_abandoned(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.abandoned)
    }

    /// What it's doing, for the status line: its longest-running work.
    pub fn activity(&self) -> Option<String> {
        if self.state == State::Starting {
            return Some(format!("{} starting", self.name));
        }
        let (title, message, percent) = self.progress.values().next()?;
        let mut text = format!("{}: {title}", self.name);
        if let Some(message) = message {
            text.push(' ');
            text.push_str(message);
        }
        // Messages can be long, like a path being loaded.
        if text.chars().count() > 60 {
            text = text.chars().take(59).collect::<String>() + "…";
        }
        if let Some(percent) = percent {
            text.push_str(&format!(" {percent}%"));
        }
        Some(text)
    }

    /// Asks it to stop, and stops it if it doesn't within `grace`.
    pub fn shutdown(&mut self, grace: Duration) {
        if self.state == State::Ready {
            self.request("shutdown", Value::Null);
            let _ = self
                .outgoing
                .send(json!({"jsonrpc": "2.0", "method": "exit"}));
        }
        if let Some(mut child) = self.child.take() {
            let until = Instant::now() + grace;
            while Instant::now() < until {
                if let Ok(Some(_)) = child.try_wait() {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        self.closed("stopped".to_string());
    }
}

/// `PATH` with Mason's bin folder first, for servers that start helpers
/// Mason installed next to them.
fn path_with_mason() -> std::ffi::OsString {
    let mut paths: Vec<PathBuf> = crate::languages::mason_bin().into_iter().collect();
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path));
    }
    std::env::join_paths(paths).unwrap_or_default()
}

fn folder(root: &Path) -> Value {
    let name = root
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    json!({"uri": uri(root), "name": name})
}

fn initialize_params(root: &Path) -> Value {
    json!({
        "processId": std::process::id(),
        "clientInfo": {"name": "rrrev", "version": env!("CARGO_PKG_VERSION")},
        "rootUri": uri(root),
        "rootPath": root,
        "workspaceFolders": [folder(root)],
        "capabilities": {
            "general": {"positionEncodings": ["utf-8", "utf-16"]},
            "window": {"workDoneProgress": true},
            "workspace": {"configuration": true, "workspaceFolders": true},
            "textDocument": {
                "synchronization": {"didSave": false},
                "definition": {"linkSupport": true},
                "typeDefinition": {"linkSupport": true},
                "implementation": {"linkSupport": true},
                "references": {},
                "hover": {"contentFormat": ["plaintext", "markdown"]},
            },
        },
    })
}
