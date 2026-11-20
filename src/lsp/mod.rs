//! A small LSP client: one server per language and project, started the
//! first time a file of theirs is opened, from Mason's bin folder only.
//! Servers run as child processes with a reader and a writer thread each;
//! the main loop calls [`Lsp::poll`] to take in what they sent.

pub mod server;
pub mod transport;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use serde_json::Value;

use crate::languages::Registry;
pub use server::Encoding;
use server::{Server, State};

/// What the reader threads send the main loop.
pub enum Event {
    Message { server: usize, message: Value },
    Closed { server: usize, why: String },
}

/// A finished request: the server it went to, its id, and the result or
/// the server's error message.
#[derive(Debug)]
pub struct Reply {
    pub server: usize,
    pub id: u64,
    pub result: Result<Value, String>,
}

pub struct Lsp {
    /// The repository's working folder; diff paths are relative to it.
    workdir: PathBuf,
    registry: &'static Registry,
    servers: Vec<Server>,
    /// By language and project root, the server for it.
    by_root: HashMap<(String, PathBuf), usize>,
    /// Languages whose server couldn't start, and why.
    missing: HashMap<String, String>,
    events: Sender<Event>,
    incoming: Receiver<Event>,
}

impl Lsp {
    pub fn new(workdir: &Path, registry: &'static Registry) -> Lsp {
        let (events, incoming) = mpsc::channel();
        Lsp {
            workdir: workdir.to_path_buf(),
            registry,
            servers: Vec::new(),
            by_root: HashMap::new(),
            missing: HashMap::new(),
            events,
            incoming,
        }
    }

    /// Tells the right server about a file (relative to the working folder),
    /// starting it if it isn't running. Files in languages without a server,
    /// or whose server isn't in Mason, are left alone.
    pub fn open(&mut self, path: &str) {
        let Some(index) = self.server_for(path) else { return };
        let Some(language) = self.registry.for_path(path) else { return };
        let full = self.workdir.join(path);
        self.servers[index].open(&full, language_id(&language.name));
    }

    /// The server for a file, started if needed.
    pub fn server_for(&mut self, path: &str) -> Option<usize> {
        let language = self.registry.for_path(path)?;
        let lsp = language.lsp.as_ref()?;
        let root = project_root(&self.workdir, path, &lsp.roots);
        let key = (language.name.clone(), root.clone());
        if let Some(&index) = self.by_root.get(&key)
            && !matches!(self.servers[index].state, State::Exited(_))
        {
            return Some(index);
        }
        let Some(command) = lsp.find() else {
            let why = if lsp.command.contains('/') { "isn't there" } else { "isn't in Mason" };
            self.missing.insert(language.name.clone(), format!("{} {why}", lsp.command));
            return None;
        };
        let id = self.servers.len();
        let log = std::env::temp_dir().join(format!("rrrev-lsp-{}.log", language.name));
        match Server::spawn(id, &language.name, &command, &lsp.args, &root, &log, self.events.clone()) {
            Ok(server) => {
                self.missing.remove(&language.name);
                self.servers.push(server);
                self.by_root.insert(key, id);
                Some(id)
            }
            Err(e) => {
                self.missing.insert(language.name.clone(), format!("{e:#}"));
                None
            }
        }
    }

    /// Adds a server that's already connected, as tests do.
    #[cfg(test)]
    fn add(&mut self, lang: &str, root: &Path, make: impl FnOnce(usize, Sender<Event>) -> Server) -> usize {
        let id = self.servers.len();
        self.servers.push(make(id, self.events.clone()));
        self.by_root.insert((lang.to_string(), root.to_path_buf()), id);
        id
    }

    #[allow(dead_code)]
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    pub fn server(&mut self, index: usize) -> &mut Server {
        &mut self.servers[index]
    }

    /// Takes in everything the servers sent since the last call, and
    /// returns the replies to rrrev's own requests.
    pub fn poll(&mut self) -> Vec<Reply> {
        let mut replies = Vec::new();
        while let Ok(event) = self.incoming.try_recv() {
            match event {
                Event::Message { server, message } => {
                    if let Some((id, _, result)) = self.servers[server].receive(message) {
                        replies.push(Reply { server, id, result });
                    }
                }
                Event::Closed { server, why } => self.servers[server].closed(why),
            }
        }
        replies
    }

    /// For the status line: what a server is busy with, if any is.
    pub fn status(&self) -> Option<String> {
        self.servers.iter().find_map(Server::activity)
    }

    /// For `:lsp`: every server and its state, and the languages in the
    /// diff whose server couldn't start.
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = self
            .servers
            .iter()
            .filter(|s| !matches!(&s.state, State::Exited(why) if why == "stopped"))
            .map(|s| {
                let state = match &s.state {
                    State::Starting => "starting".to_string(),
                    State::Ready => s.activity().map_or_else(|| "ready".to_string(), |a| a.replacen(&format!("{}: ", s.name), "", 1)),
                    State::Exited(why) => format!("stopped, {why}"),
                };
                format!("{} {state}", s.name)
            })
            .collect();
        let mut missing: Vec<_> = self.missing.iter().map(|(lang, why)| format!("{lang}: {why}")).collect();
        missing.sort();
        parts.extend(missing);
        if parts.is_empty() { "No language servers for these files".to_string() } else { parts.join(" · ") }
    }

    /// Stops every server; each starts again with the next file opened.
    pub fn stop_all(&mut self) {
        for server in &mut self.servers {
            server.shutdown(Duration::from_millis(500));
        }
        self.by_root.clear();
        self.missing.clear();
    }
}

impl Drop for Lsp {
    fn drop(&mut self) {
        self.stop_all();
    }
}

/// The folder a file's project starts at: the nearest one above it, up to
/// the working folder, holding one of `roots` (`Cargo.toml`, `*.csproj`), or
/// the working folder itself.
pub fn project_root(workdir: &Path, path: &str, roots: &[String]) -> PathBuf {
    let mut dir = workdir.join(path);
    while dir.pop() && dir.starts_with(workdir) {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let names: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        if roots.iter().any(|root| names.iter().any(|name| glob(root, name))) {
            return dir;
        }
    }
    workdir.to_path_buf()
}

/// `*` in `pattern` matches any run of characters.
fn glob(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((head, rest)) => {
            let Some(name) = name.strip_prefix(head) else { return false };
            (0..=name.len()).filter(|&i| name.is_char_boundary(i)).any(|i| glob(rest, &name[i..]))
        }
    }
}

/// The `languageId` LSP knows a registry language by.
pub fn language_id(name: &str) -> &str {
    match name {
        "tsx" => "typescriptreact",
        "javascript" => "javascript",
        other => other,
    }
}

/// The LSP column of byte `byte` in `line`, counted in `encoding`'s units.
pub fn to_character(line: &str, byte: usize, encoding: Encoding) -> u32 {
    let before = line.get(..byte.min(line.len())).unwrap_or(line);
    (match encoding {
        Encoding::Utf8 => before.len(),
        Encoding::Utf16 => before.encode_utf16().count(),
    }) as u32
}

/// The byte in `line` at LSP column `character`, counted in `encoding`'s
/// units; the line's end when it's past it.
pub fn to_byte(line: &str, character: u32, encoding: Encoding) -> usize {
    let character = character as usize;
    match encoding {
        Encoding::Utf8 => (0..=character.min(line.len())).rev().find(|&i| line.is_char_boundary(i)).unwrap_or(0),
        Encoding::Utf16 => {
            let mut units = 0;
            for (i, c) in line.char_indices() {
                if units >= character {
                    return i;
                }
                units += c.len_utf16();
            }
            line.len()
        }
    }
}

/// The places in a `definition`-style answer, which may be a `Location`,
/// a list of them, or a list of `LocationLink`s: each one's path, line and
/// character.
pub fn locations(result: &Value) -> Vec<(PathBuf, u32, u32)> {
    let items: Vec<&Value> = match result {
        Value::Array(items) => items.iter().collect(),
        Value::Null => Vec::new(),
        one => vec![one],
    };
    items
        .into_iter()
        .filter_map(|item| {
            let uri = item.get("targetUri").or_else(|| item.get("uri"))?.as_str()?;
            let range = item.get("targetSelectionRange").or_else(|| item.get("range"))?;
            let start = &range["start"];
            Some((path_from_uri(uri)?, start["line"].as_u64()? as u32, start["character"].as_u64()? as u32))
        })
        .collect()
}

/// A `file://` URI for an absolute path.
pub fn uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The path in a `file://` URI.
pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?.as_bytes();
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut i = 0;
    while i < encoded.len() {
        if encoded[i] == b'%'
            && let Some(byte) = encoded.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok())
        {
            bytes.push(byte);
            i += 3;
        } else {
            bytes.push(encoded[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufReader, PipeWriter};
    use std::time::Instant;

    /// The far end of a connection, playing the server.
    struct Fake {
        input: mpsc::Receiver<Value>,
        output: PipeWriter,
    }

    impl Fake {
        /// The next message from rrrev, polling `lsp` meanwhile as the main
        /// loop would, since some messages are answers to what it takes in.
        fn read(&mut self, lsp: &mut Lsp) -> Value {
            let until = Instant::now() + Duration::from_secs(1);
            loop {
                lsp.poll();
                if let Ok(message) = self.input.recv_timeout(Duration::from_millis(5)) {
                    return message;
                }
                assert!(Instant::now() < until, "timed out");
            }
        }
        fn send(&mut self, message: Value) {
            transport::write(&mut self.output, &message).unwrap();
        }
    }

    fn connected(lsp: &mut Lsp, root: &Path) -> (usize, Fake) {
        let (from_server, to_client) = std::io::pipe().unwrap();
        let (from_client, to_server) = std::io::pipe().unwrap();
        let index = lsp.add("rust", root, |id, events| {
            Server::connect(id, "fake-ls", root, from_server, to_server, None, events)
        });
        let (messages, input) = mpsc::channel();
        std::thread::spawn(move || {
            let mut from_client = BufReader::new(from_client);
            while let Ok(Some(message)) = transport::read(&mut from_client) {
                if messages.send(message).is_err() {
                    break;
                }
            }
        });
        (index, Fake { input, output: to_client })
    }

    /// Polls until `done` holds, or fails after a second.
    fn poll_until(lsp: &mut Lsp, mut done: impl FnMut(&mut Lsp, &[Reply]) -> bool) {
        let until = Instant::now() + Duration::from_secs(1);
        loop {
            let replies = lsp.poll();
            if done(lsp, &replies) {
                return;
            }
            assert!(Instant::now() < until, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn talks_to_a_server() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn main() {}\n").unwrap();
        let mut lsp = Lsp::new(dir.path(), Registry::global());
        let (index, mut fake) = connected(&mut lsp, dir.path());

        let initialize = fake.read(&mut lsp);
        assert_eq!(initialize["method"], "initialize");
        assert_eq!(initialize["params"]["rootUri"], uri(dir.path()));
        assert_eq!(lsp.status().as_deref(), Some("fake-ls starting"));

        // Opened before the server is ready: held until it is.
        lsp.server(index).open(&dir.path().join("a.rs"), "rust");
        fake.send(json!({"jsonrpc": "2.0", "id": initialize["id"], "result": {"capabilities": {"positionEncoding": "utf-8"}}}));
        poll_until(&mut lsp, |lsp, _| lsp.servers[index].state == State::Ready);
        assert_eq!(fake.read(&mut lsp)["method"], "initialized");
        let open = fake.read(&mut lsp);
        assert_eq!(open["method"], "textDocument/didOpen");
        assert_eq!(open["params"]["textDocument"]["text"], "fn main() {}\n");
        assert_eq!(lsp.servers[index].encoding, server::Encoding::Utf8);
        assert_eq!(lsp.status(), None);

        // Progress shows in the status line until it ends.
        let progress = |kind: &str, extra: Value| {
            let mut value = json!({"kind": kind});
            value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            json!({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "t", "value": value}})
        };
        fake.send(progress("begin", json!({"title": "Indexing"})));
        fake.send(progress("report", json!({"percentage": 40})));
        poll_until(&mut lsp, |lsp, _| lsp.status().as_deref() == Some("fake-ls: Indexing 40%"));
        assert_eq!(lsp.describe(), "fake-ls Indexing 40%");
        fake.send(progress("end", json!({})));
        poll_until(&mut lsp, |lsp, _| lsp.status().is_none());

        // The server's own requests get answers.
        fake.send(json!({"jsonrpc": "2.0", "id": 7, "method": "workspace/configuration", "params": {"items": [{}, {}]}}));
        assert_eq!(fake.read(&mut lsp), json!({"jsonrpc": "2.0", "id": 7, "result": [null, null]}));
        fake.send(json!({"jsonrpc": "2.0", "id": 8, "method": "window/showDocument", "params": {}}));
        assert_eq!(fake.read(&mut lsp)["error"]["code"], -32601);

        // rrrev's requests come back as replies.
        let id = lsp.server(index).request("textDocument/hover", json!({}));
        assert_eq!(fake.read(&mut lsp)["id"], id);
        fake.send(json!({"jsonrpc": "2.0", "id": id, "result": {"contents": "x"}}));
        let mut got = None;
        poll_until(&mut lsp, |_, replies| {
            got = replies.first().map(|r| (r.id, r.result.clone()));
            got.is_some()
        });
        assert_eq!(got, Some((id, Ok(json!({"contents": "x"})))));

        // A server that goes away says so.
        drop(fake);
        poll_until(&mut lsp, |lsp, _| matches!(lsp.servers[index].state, State::Exited(_)));
        assert_eq!(lsp.describe(), "fake-ls stopped, it exited");
    }

    #[test]
    fn a_server_that_refuses_to_start_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let mut lsp = Lsp::new(dir.path(), Registry::global());
        let (index, mut fake) = connected(&mut lsp, dir.path());
        let initialize = fake.read(&mut lsp);
        fake.send(json!({"jsonrpc": "2.0", "id": initialize["id"], "error": {"code": 1, "message": "no"}}));
        poll_until(&mut lsp, |lsp, _| matches!(lsp.servers[index].state, State::Exited(_)));
        assert_eq!(lsp.describe(), "fake-ls stopped, it refused to start: no");
    }

    #[test]
    fn missing_servers_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Registry::builtin()
            .merge("[[language]]\nname = \"zig\"\nextensions = [\"zig\"]\nlsp = { command = \"/nowhere/zls\" }")
            .unwrap();
        let mut lsp = Lsp::new(dir.path(), Box::leak(Box::new(registry)));
        lsp.open("README.md");
        assert_eq!(lsp.describe(), "No language servers for these files");
        lsp.open("a.zig");
        assert_eq!(lsp.describe(), "zig: /nowhere/zls isn't there");
        assert_eq!(lsp.status(), None);
    }

    #[test]
    fn finds_project_roots() {
        let dir = tempfile::tempdir().unwrap();
        let crate_dir = dir.path().join("crates/a");
        std::fs::create_dir_all(crate_dir.join("src")).unwrap();
        std::fs::write(crate_dir.join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.path().join("App.csproj"), "").unwrap();
        let roots = |path, roots: &[&str]| {
            let roots: Vec<String> = roots.iter().map(|r| r.to_string()).collect();
            project_root(dir.path(), path, &roots)
        };
        assert_eq!(roots("crates/a/src/lib.rs", &["Cargo.toml"]), crate_dir);
        assert_eq!(roots("x/y.cs", &["*.csproj"]), dir.path());
        assert_eq!(roots("crates/a/src/lib.rs", &["go.mod"]), dir.path());
        assert!(glob("*.csproj", "App.csproj") && !glob("*.csproj", "App.cs") && glob("a*b*c", "aXbYc"));
    }

    #[test]
    fn converts_columns() {
        let line = "let ñ = \"😀\"; x";
        let x = line.find('x').unwrap();
        assert_eq!(to_character(line, x, Encoding::Utf8), x as u32);
        assert_eq!(to_character(line, x, Encoding::Utf16), 14);
        assert_eq!(to_byte(line, 14, Encoding::Utf16), x);
        assert_eq!(to_byte(line, x as u32, Encoding::Utf8), x);
        assert_eq!(to_byte(line, 99, Encoding::Utf16), line.len());
        assert_eq!(to_byte(line, 5, Encoding::Utf8), 4, "inside ñ, back to its start");
    }

    #[test]
    fn reads_every_shape_of_location() {
        let at = |uri: &str| json!({"uri": uri, "range": {"start": {"line": 3, "character": 4}, "end": {}}});
        assert_eq!(locations(&at("file:///a.rs")), [(PathBuf::from("/a.rs"), 3, 4)]);
        assert_eq!(locations(&json!([at("file:///a.rs"), at("file:///b.rs")])).len(), 2);
        let link = json!([{"targetUri": "file:///c.rs", "targetRange": {}, "targetSelectionRange": {"start": {"line": 1, "character": 2}}}]);
        assert_eq!(locations(&link), [(PathBuf::from("/c.rs"), 1, 2)]);
        assert!(locations(&Value::Null).is_empty());
    }

    #[test]
    fn uris_round_trip() {
        let path = Path::new("/home/me/my code/ñ#1.rs");
        assert_eq!(uri(path), "file:///home/me/my%20code/%C3%B1%231.rs");
        assert_eq!(path_from_uri(&uri(path)).as_deref(), Some(path));
        assert_eq!(path_from_uri("http://x"), None);
    }
}
