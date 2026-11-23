//! Putting text on the system clipboard.
//!
//! Locally, a clipboard program (`pbcopy`, `wl-copy`, `xclip`, `xsel`) is the
//! reliable way: the OSC 52 escape is silently ignored by some terminals,
//! macOS's Terminal.app among them, and by tmux without `set-clipboard on`.
//! Over SSH those programs would fill the remote machine's clipboard, so
//! there, and when none of them works, OSC 52 is used instead.

use std::env;
use std::io::{Write, stdout};
use std::process::{Command, Stdio};

use anyhow::{Result, bail};

/// Clipboard programs to try in order, with their arguments, and the
/// environment variable that has to be set for each to make sense.
const PROGRAMS: &[(&str, &[&str], Option<&str>)] = &[
    ("pbcopy", &[], None),
    ("wl-copy", &[], Some("WAYLAND_DISPLAY")),
    ("xclip", &["-selection", "clipboard"], Some("DISPLAY")),
    ("xsel", &["--clipboard", "--input"], Some("DISPLAY")),
];

pub fn copy(text: &str) -> Result<()> {
    if !over_ssh() {
        let usable = PROGRAMS
            .iter()
            .filter(|(_, _, var)| var.is_none_or(|v| env::var_os(v).is_some()));
        for (program, args, _) in usable {
            if run(program, args, text).is_ok() {
                return Ok(());
            }
        }
    }
    osc52(text)
}

fn over_ssh() -> bool {
    env::var_os("SSH_TTY").is_some() || env::var_os("SSH_CONNECTION").is_some()
}

/// Pipes `text` into `program`, failing when it can't be started or exits
/// with an error.
fn run(program: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Take stdin so it's closed before waiting, or the program never ends.
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    if !child.wait()?.success() {
        bail!("{program} failed");
    }
    Ok(())
}

/// The OSC 52 escape, which terminals (and tmux, with `set-clipboard on`)
/// pass to the system clipboard, over SSH too.
fn osc52(text: &str) -> Result<()> {
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
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
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
    fn runs_report_how_the_program_ended() {
        assert!(run("sh", &["-c", "cat >/dev/null"], "x").is_ok());
        assert!(run("sh", &["-c", "cat >/dev/null; exit 1"], "x").is_err());
        assert!(run("rrrev-no-such-program", &[], "x").is_err());
    }
}
