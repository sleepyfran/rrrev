use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::{Cancel, OrganizeRequest, Organizer, Plan};

const SYSTEM_PROMPT: &str = "\
You plan code reviews. You get a diff split into hunks, each introduced by a \
line `=== <id> <path> <hunk header>`. Group the hunks into a short, ordered \
list of review steps so a reviewer can read the change like a story: first \
what everything else depends on (data types, interfaces, schemas, \
configuration), then the core logic, then the call sites and wiring, then \
tests, then documentation and generated files. Keep hunks that belong \
together in one group even when they are in different files. Aim for 3 to 9 \
groups, fewer for a small diff. Give each group a title of a few words and a \
one-sentence rationale saying what it changes and why it comes at this point. \
Within a group, list hunks in the order they should be read. Use every hunk \
id exactly once, copied exactly.";

const SCHEMA: &str = r#"{"type":"object","properties":{"groups":{"type":"array","items":{"type":"object","properties":{"title":{"type":"string"},"rationale":{"type":"string"},"hunks":{"type":"array","items":{"type":"string"}}},"required":["title","rationale","hunks"]}}},"required":["groups"]}"#;

/// Plans with the `claude` CLI in print mode, with no tools and our own system
/// prompt, so it only reads the diff it's given.
pub struct ClaudeCli {
    pub program: String,
    pub model: Option<String>,
}

impl ClaudeCli {
    /// `$RRREV_CLAUDE` picks the executable (default `claude`) and
    /// `$RRREV_MODEL` the model (default: the CLI's own).
    pub fn from_env() -> Self {
        Self {
            program: std::env::var("RRREV_CLAUDE").unwrap_or_else(|_| "claude".to_string()),
            model: std::env::var("RRREV_MODEL").ok().filter(|m| !m.is_empty()),
        }
    }
}

impl Organizer for ClaudeCli {
    fn organize(&self, request: &OrganizeRequest, cancel: &Cancel) -> Result<Plan> {
        let mut command = Command::new(&self.program);
        command.args([
            "-p",
            "--output-format",
            "json",
            "--no-session-persistence",
            "--tools",
            "",
            "--system-prompt",
            SYSTEM_PROMPT,
            "--json-schema",
            SCHEMA,
        ]);
        if let Some(model) = &self.model {
            command.args(["--model", model]);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("cannot run `{}`", self.program))?;

        // Each pipe gets its own thread so neither side can block the other
        // on a full buffer, and this one is free to watch for `cancel`.
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let prompt = request.to_prompt();
        let writer = thread::spawn(move || stdin.write_all(prompt.as_bytes()));
        let stdout = read_in_background(child.stdout.take().expect("stdout is piped"));
        let stderr = read_in_background(child.stderr.take().expect("stderr is piped"));

        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if cancel.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                bail!("cancelled");
            }
            thread::sleep(POLL);
        };
        let stdout = stdout.join().expect("stdout reader panicked")?;
        let stderr = stderr.join().expect("stderr reader panicked")?;
        let written = writer.join().expect("stdin writer panicked");

        let parsed = parse_response(&stdout);
        if status.success() {
            written?;
            return parsed;
        }
        // Failures usually still print a JSON result saying why; otherwise
        // the reason is on stderr.
        let reason = match parsed {
            Err(e) if !stdout.trim().is_empty() => format!("{e:#}"),
            _ => stderr.trim().to_string(),
        };
        bail!("`{}` failed ({status}): {reason}", self.program)
    }
}

/// How often a running organizer checks whether it was cancelled.
const POLL: Duration = Duration::from_millis(50);

fn read_in_background(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<std::io::Result<String>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    })
}

/// Reads the `--output-format json` envelope.
fn parse_response(stdout: &str) -> Result<Plan> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        is_error: bool,
        result: Option<String>,
        structured_output: Option<Plan>,
    }
    let envelope: Envelope = serde_json::from_str(stdout.trim()).context("unexpected output from claude")?;
    if envelope.is_error {
        bail!("{}", envelope.result.unwrap_or_else(|| "claude reported an error".to_string()));
    }
    match (envelope.structured_output, envelope.result) {
        (Some(plan), _) => Ok(plan),
        // Older CLIs return the JSON as text only.
        (None, Some(text)) => serde_json::from_str(&text).context("claude did not return a plan"),
        (None, None) => bail!("claude did not return a plan"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::Diff;

    const PLAN_JSON: &str = r#"{"groups":[{"title":"Core","rationale":"Types first.","hunks":["abc"]}]}"#;

    #[test]
    fn reads_structured_output() {
        let stdout = format!(r#"{{"type":"result","is_error":false,"result":"…","structured_output":{PLAN_JSON}}}"#);
        let plan = parse_response(&stdout).unwrap();
        assert_eq!(plan.groups[0].title, "Core");
        assert_eq!(plan.groups[0].hunks[0].as_str(), "abc");
    }

    #[test]
    fn falls_back_to_the_result_text() {
        let stdout = serde_json::json!({ "is_error": false, "result": PLAN_JSON }).to_string();
        assert_eq!(parse_response(&stdout).unwrap().groups.len(), 1);
    }

    #[test]
    fn reports_errors() {
        let stdout = r#"{"is_error":true,"result":"Credit balance is too low"}"#;
        assert_eq!(parse_response(stdout).unwrap_err().to_string(), "Credit balance is too low");
        assert!(parse_response("not json").is_err());
    }

    /// Runs a stand-in for `claude` that checks its arguments and stdin.
    #[cfg(unix)]
    #[test]
    fn runs_the_cli_with_the_prompt_on_stdin() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-claude");
        let envelope = format!(r#"{{"is_error":false,"structured_output":{PLAN_JSON}}}"#);
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 case \"$*\" in *--json-schema*) ;; *) echo 'no schema' >&2; exit 1 ;; esac\n\
                 grep -q 'Diff under review: test' || {{ echo 'no prompt' >&2; exit 1; }}\n\
                 echo '{envelope}'\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let cli = ClaudeCli { program: script.display().to_string(), model: Some("sonnet".into()) };
        let request = OrganizeRequest::from_diff("test", &Diff::default());
        assert_eq!(cli.organize(&request, &Cancel::default()).unwrap().groups[0].title, "Core");

        let missing = ClaudeCli { program: dir.path().join("nope").display().to_string(), model: None };
        let error = missing.organize(&request, &Cancel::default()).unwrap_err();
        assert!(error.to_string().starts_with("cannot run"));
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_kills_the_cli() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::Instant;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("slow-claude");
        std::fs::write(&script, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let cli = ClaudeCli { program: script.display().to_string(), model: None };
        let cancel = Cancel::default();
        let canceller = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            canceller.cancel();
        });
        let started = Instant::now();
        let request = OrganizeRequest::from_diff("test", &Diff::default());
        let error = cli.organize(&request, &cancel).unwrap_err();
        assert_eq!(error.to_string(), "cancelled");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
