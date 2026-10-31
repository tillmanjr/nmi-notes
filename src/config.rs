// SPDX-License-Identifier: AGPL-3.0-or-later

//! User configuration (`%APPDATA%\nmi-notes\config.toml`).

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub new_note: String,
    pub append: String,
    pub show_current: String,
    pub copy_current: String,
    pub note_list: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Hotkeys {
            new_note: "Win+Alt+N".into(),
            append: "Win+Alt+A".into(),
            show_current: "Win+Alt+S".into(),
            copy_current: "Win+Alt+C".into(),
            note_list: "Win+Alt+L".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Editor font face; Fira Mono is built in. Falls back to Consolas if missing.
    pub font: String,
    /// Editor font size in points.
    pub font_size: i32,
    /// Register in HKCU\...\Run so the app starts at logon.
    pub start_with_windows: bool,
    pub hotkeys: Hotkeys,
}

impl Default for Config {
    fn default() -> Self {
        Config { font: "Fira Mono".into(), font_size: 11, start_with_windows: false, hotkeys: Hotkeys::default() }
    }
}

/// Loads the config, writing defaults if the file doesn't exist. A malformed
/// file yields defaults plus an error message (the file is left untouched).
pub fn load_or_create(path: &Path) -> (Config, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(s) => match toml::from_str(&s) {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(format!("config.toml invalid, using defaults: {e}"))),
        },
        Err(_) => {
            let c = Config::default();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(body) = toml::to_string_pretty(&c) {
                let _ = std::fs::write(path, body);
            }
            (c, None)
        }
    }
}

pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

/// Parses e.g. `Win+Alt+N` into (`RegisterHotKey` modifiers, virtual key).
pub fn parse_hotkey(s: &str) -> Result<(u32, u32), String> {
    let mut mods = 0;
    let mut vk = None;
    for part in s.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "win" => mods |= MOD_WIN,
            "alt" => mods |= MOD_ALT,
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "shift" => mods |= MOD_SHIFT,
            key => {
                if vk.is_some() {
                    return Err(format!("'{s}': more than one key"));
                }
                vk = Some(parse_key(key).ok_or_else(|| format!("'{s}': unknown key '{part}'"))?);
            }
        }
    }
    let vk = vk.ok_or_else(|| format!("'{s}': no key"))?;
    if mods == 0 {
        return Err(format!("'{s}': needs at least one modifier"));
    }
    Ok((mods, vk))
}

fn parse_key(k: &str) -> Option<u32> {
    let b = k.as_bytes();
    if b.len() == 1 && (b[0].is_ascii_alphanumeric()) {
        return Some(b[0].to_ascii_uppercase() as u32);
    }
    if let Some(n) = k.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    Some(match k {
        "space" => 0x20,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "insert" => 0x2D,
        "pause" => 0x13,
        "`" | "backquote" => 0xC0,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hotkeys() {
        assert_eq!(parse_hotkey("Win+Alt+N"), Ok((MOD_WIN | MOD_ALT, 'N' as u32)));
        assert_eq!(parse_hotkey("ctrl + shift + f5"), Ok((MOD_CONTROL | MOD_SHIFT, 0x74)));
        assert_eq!(parse_hotkey("Alt+Space"), Ok((MOD_ALT, 0x20)));
        assert_eq!(parse_hotkey("Win+3"), Ok((MOD_WIN, '3' as u32)));
    }

    #[test]
    fn rejects_bad_hotkeys() {
        assert!(parse_hotkey("N").is_err());
        assert!(parse_hotkey("Win+Alt").is_err());
        assert!(parse_hotkey("Win+A+B").is_err());
        assert!(parse_hotkey("Win+Banana").is_err());
        assert!(parse_hotkey("Win+F25").is_err());
    }

    #[test]
    fn defaults_parse_and_roundtrip() {
        let c = Config::default();
        for h in [&c.hotkeys.new_note, &c.hotkeys.append, &c.hotkeys.show_current, &c.hotkeys.copy_current, &c.hotkeys.note_list] {
            assert!(parse_hotkey(h).is_ok(), "{h}");
        }
        let back: Config = toml::from_str(&toml::to_string_pretty(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn partial_file_fills_defaults_and_creates_missing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "font_size = 14\n[hotkeys]\nappend = \"Ctrl+Alt+A\"\n").unwrap();
        let (c, err) = load_or_create(&p);
        assert!(err.is_none());
        assert_eq!(c.font_size, 14);
        assert_eq!(c.hotkeys.append, "Ctrl+Alt+A");
        assert_eq!(c.hotkeys.new_note, "Win+Alt+N");

        let p2 = dir.path().join("new").join("config.toml");
        let (c2, _) = load_or_create(&p2);
        assert_eq!(c2, Config::default());
        assert!(p2.exists());
    }
}
