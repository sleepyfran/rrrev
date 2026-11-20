//! Settings that follow you across repositories, in
//! `$XDG_CONFIG_HOME/rrrev/config.json` (`~/.config/rrrev/config.json`).
//! Only the theme lives there for now.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value};

use crate::theme::Theme;

pub fn path() -> Option<PathBuf> {
    Some(dir()?.join("config.json"))
}

/// `$XDG_CONFIG_HOME/rrrev`, where everything rrrev reads from lives.
pub fn dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("rrrev"))
}

/// The theme to start with: `RRREV_THEME` when it names one, then the
/// config file's, then the terminal's colours. Its dark or light variant
/// is the OS's call.
pub fn theme() -> &'static Theme {
    let from_env = std::env::var("RRREV_THEME").ok().and_then(|name| Theme::find(&name));
    from_env.or_else(|| path().and_then(|p| saved_theme(&p))).unwrap_or_else(Theme::terminal)
}

fn saved_theme(path: &Path) -> Option<&'static Theme> {
    let config = read(path).ok()?;
    Theme::find(config.get("theme")?.as_str()?)
}

/// Saves `theme` as the one to start with, keeping anything else in the file.
pub fn save_theme(theme: &Theme) -> Result<()> {
    let path = path().context("no home directory to keep the config in")?;
    write_theme(&path, theme)
}

fn write_theme(path: &Path, theme: &Theme) -> Result<()> {
    let mut config = read(path).unwrap_or_default();
    config.insert("theme".to_string(), Value::String(theme.id.to_string()));
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(&config)? + "\n";
    fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))
}

fn read(path: &Path) -> Result<Map<String, Value>> {
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_the_theme_and_keeps_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rrrev/config.json");
        assert_eq!(saved_theme(&path), None);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"other": 1, "theme": "nope"}"#).unwrap();
        assert_eq!(saved_theme(&path), None);

        let nord = Theme::find("nord").unwrap();
        write_theme(&path, nord).unwrap();
        assert_eq!(saved_theme(&path), Some(nord));
        assert_eq!(read(&path).unwrap()["other"], 1);
    }
}
