//! LSP's framing: a `Content-Length` header, a blank line, then that many
//! bytes of JSON.

use std::io::{self, BufRead, Write};

use serde_json::Value;

pub fn write(out: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
    out.write_all(&body)?;
    out.flush()
}

/// The next message, or `None` once the other side has closed.
pub fn read(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let header = line.trim_end();
        if header.is_empty() {
            if length.is_some() {
                break;
            }
            // Stray blank lines between messages.
            continue;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = Some(value.trim().parse::<usize>().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?);
        }
    }
    let mut body = vec![0; length.unwrap_or(0)];
    input.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_messages() {
        let mut buffer = Vec::new();
        write(&mut buffer, &json!({"jsonrpc": "2.0", "method": "a", "params": {"é": 1}})).unwrap();
        write(&mut buffer, &json!({"id": 2})).unwrap();
        let text = String::from_utf8(buffer.clone()).unwrap();
        let body = serde_json::to_vec(&json!({"jsonrpc": "2.0", "method": "a", "params": {"é": 1}})).unwrap();
        assert!(text.starts_with(&format!("Content-Length: {}\r\n\r\n{{", body.len())), "{text}");
        let mut input = io::Cursor::new(buffer);
        assert_eq!(read(&mut input).unwrap().unwrap()["params"]["é"], 1);
        assert_eq!(read(&mut input).unwrap().unwrap()["id"], 2);
        assert_eq!(read(&mut input).unwrap(), None);
    }

    #[test]
    fn reads_other_headers_and_any_case() {
        let raw = "content-length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}";
        assert_eq!(read(&mut io::Cursor::new(raw)).unwrap(), Some(json!({})));
        assert!(read(&mut io::Cursor::new("Content-Length: x\r\n\r\n{}")).is_err());
    }
}
