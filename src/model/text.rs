// SPDX-License-Identifier: AGPL-3.0-or-later

//! Text helpers shared by the store, clipboard, and UI layers.
//!
//! The store keeps `\n` line endings only. Offsets exchanged with the editor
//! are UTF-16 code units.

/// Converts CRLF and lone CR line endings to LF.
pub fn normalize_newlines(s: &str) -> String {
    if !s.contains('\r') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// Converts LF line endings to CRLF (for the clipboard).
pub fn to_crlf(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + s.len() / 16);
    for c in s.chars() {
        if c == '\n' {
            out.push('\r');
        }
        out.push(c);
    }
    out
}

/// Separator to insert before appended text: a newline only when the note is
/// non-empty and does not already end with one.
pub fn append_separator(existing: &str) -> &'static str {
    if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    }
}

/// First non-blank line, trimmed. Empty string for a blank note.
pub fn first_line(s: &str) -> &str {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("")
}

/// Truncates to at most `max` chars, appending an ellipsis when cut.
pub fn ellipsize(s: &str, max: usize) -> String {
    let mut it = s.char_indices();
    match it.nth(max) {
        None => s.to_owned(),
        Some((idx, _)) => {
            let mut out = s[..idx].trim_end().to_owned();
            out.push('…');
            out
        }
    }
}

/// File-drop list as text: one path per line.
pub fn paths_to_text(paths: &[String]) -> String {
    paths.join("\n")
}

pub fn line_count(s: &str) -> usize {
    if s.is_empty() {
        0
    } else {
        s.lines().count()
    }
}

pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Maps a UTF-16 offset to a byte offset. `None` if past the end or inside a
/// surrogate pair.
pub fn utf16_to_byte(s: &str, off: usize) -> Option<usize> {
    let mut units = 0;
    for (byte, c) in s.char_indices() {
        if units == off {
            return Some(byte);
        }
        units += c.len_utf16();
        if units > off {
            return None;
        }
    }
    (units == off).then_some(s.len())
}

/// Compact count: `532`, `12.4k`, `3.1M`.
pub fn compact_count(n: usize) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// Short human description of how long ago `then_ms` was.
pub fn relative_time(now_ms: i64, then_ms: i64) -> String {
    let secs = (now_ms - then_ms).max(0) / 1000;
    match secs {
        0..=44 => "just now".into(),
        45..=3599 => format!("{}m ago", (secs + 30) / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_crlf_and_cr() {
        assert_eq!(normalize_newlines("a\r\nb\rc\nd"), "a\nb\nc\nd");
        assert_eq!(normalize_newlines("\r\r\n"), "\n\n");
        assert_eq!(normalize_newlines("plain"), "plain");
    }

    #[test]
    fn converts_to_crlf() {
        assert_eq!(to_crlf("a\nb\n"), "a\r\nb\r\n");
        assert_eq!(to_crlf(""), "");
    }

    #[test]
    fn append_separator_rule() {
        assert_eq!(append_separator(""), "");
        assert_eq!(append_separator("abc\n"), "");
        assert_eq!(append_separator("abc"), "\n");
    }

    #[test]
    fn first_line_skips_blank_lines() {
        assert_eq!(first_line("\n  \n  hello world  \nnext"), "hello world");
        assert_eq!(first_line("   "), "");
    }

    #[test]
    fn ellipsize_cuts_on_char_boundary() {
        assert_eq!(ellipsize("héllo", 10), "héllo");
        assert_eq!(ellipsize("héllo wörld", 5), "héllo…");
    }

    #[test]
    fn utf16_offsets_with_surrogates() {
        let s = "a😀b"; // a=1 unit, 😀=2 units, b=1 unit
        assert_eq!(utf16_len(s), 4);
        assert_eq!(utf16_to_byte(s, 0), Some(0));
        assert_eq!(utf16_to_byte(s, 1), Some(1));
        assert_eq!(utf16_to_byte(s, 2), None); // inside the pair
        assert_eq!(utf16_to_byte(s, 3), Some(5));
        assert_eq!(utf16_to_byte(s, 4), Some(6));
        assert_eq!(utf16_to_byte(s, 5), None);
    }

    #[test]
    fn line_counts() {
        assert_eq!(line_count(""), 0);
        assert_eq!(line_count("a"), 1);
        assert_eq!(line_count("a\nb\n"), 2);
    }

    #[test]
    fn compact_counts() {
        assert_eq!(compact_count(532), "532");
        assert_eq!(compact_count(12_400), "12.4k");
        assert_eq!(compact_count(3_100_000), "3.1M");
    }

    #[test]
    fn relative_time_buckets() {
        let now = 10_000_000_000;
        assert_eq!(relative_time(now, now - 10_000), "just now");
        assert_eq!(relative_time(now, now - 5 * 60_000), "5m ago");
        assert_eq!(relative_time(now, now - 3 * 3_600_000), "3h ago");
        assert_eq!(relative_time(now, now - 2 * 86_400_000), "2d ago");
        assert_eq!(relative_time(now, now + 5000), "just now");
    }
}
