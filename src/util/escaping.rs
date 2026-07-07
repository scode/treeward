//! Escaping helpers for terminal-facing text.

use std::borrow::Cow;

/// Escape control characters so untrusted text cannot inject terminal escape sequences.
///
/// File names, symlink targets, ward-file fields, and diagnostic messages can
/// all contain attacker-controlled text. They must not be able to inject
/// terminal control sequences (OSC/CSI), such as terminal retitling or
/// clipboard writes via OSC 52. Comparable tools (`ls`, `git`) quote control
/// characters for the same reason.
///
/// Control characters (including C1, so the single-byte 0x9B CSI is covered)
/// are rendered with Rust's debug escapes (`\n`, `\u{1b}`, ...). Literal
/// backslashes are doubled so escaped output stays unambiguous: a name
/// containing the literal text `\u{1b}` cannot be confused with an escaped
/// real ESC. All other Unicode passes through unchanged.
pub(crate) fn escape_control(s: &str) -> Cow<'_, str> {
    if !s.chars().any(|c| c.is_control() || c == '\\') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\\' {
            out.push_str("\\\\");
        } else if c.is_control() {
            out.extend(c.escape_debug());
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::escape_control;

    #[test]
    fn escape_control_passes_plain_names_through() {
        assert_eq!(escape_control("plain-name.txt"), "plain-name.txt");
        assert_eq!(escape_control("unicode-ñ-名前.txt"), "unicode-ñ-名前.txt");
    }

    #[test]
    fn escape_control_neutralizes_escape_sequences() {
        // An OSC sequence that would retitle the terminal if printed raw.
        assert_eq!(
            escape_control("\x1b]0;pwned\x07.txt"),
            "\\u{1b}]0;pwned\\u{7}.txt"
        );
        // C1 single-byte CSI (U+009B) must be caught too.
        assert_eq!(escape_control("a\u{9b}31mb"), "a\\u{9b}31mb");
        assert_eq!(escape_control("line\nbreak"), "line\\nbreak");
    }

    #[test]
    fn escape_control_doubles_literal_backslashes() {
        assert_eq!(escape_control(r"back\slash"), r"back\\slash");
        // A name containing the literal text "\u{1b}" must stay
        // distinguishable from a real escaped ESC.
        assert_eq!(escape_control("fake\\u{1b}.txt"), "fake\\\\u{1b}.txt");
    }
}
