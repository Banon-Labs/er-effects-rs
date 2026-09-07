//! Serialise to JSON that is pure ASCII, with every other character as a `\uXXXX` escape.
//!
//! Why, because it is not a style choice. The planner serialises with
//! `btoa(JSON.stringify(character))`, and `btoa` throws `InvalidCharacterError` on any code
//! unit above 255 -- so a build named with an acute accent, let alone katakana, cannot be
//! shared from the site at all. This crate has no such excuse to fail: it can escape those
//! characters into the JSON text instead, at which point the payload is ASCII, base64 is
//! defined on it, and the planner's own `JSON.parse` un-escapes them back on import. Verified
//! end to end against the site's decoder with a name carrying both.
//!
//! The escaping is applied to `serde_json`'s finished output rather than through a custom
//! formatter because non-ASCII characters can only occur inside JSON string literals --
//! structural JSON is ASCII by definition -- so a blanket pass over the text cannot corrupt
//! anything outside a string.

use serde::Serialize;
use std::fmt::Write as _;

/// The highest character that is left alone.
const LAST_ASCII: char = '\u{7f}';

/// Serialise `value` to JSON containing only ASCII.
///
/// # Errors
///
/// Returns the underlying `serde_json` error when the value cannot be serialised.
pub fn to_ascii_json<T>(value: &T) -> Result<String, serde_json::Error>
where
    T: ?Sized + Serialize,
{
    Ok(escape_non_ascii(&serde_json::to_string(value)?))
}

/// Rewrite every non-ASCII character of `json` as one or more `\uXXXX` escapes.
///
/// Characters outside the Basic Multilingual Plane become a surrogate pair, which is exactly
/// what `JSON.stringify` emits when it does escape, and what `JSON.parse` recombines.
///
/// ```
/// use er_build_export::json_ascii::escape_non_ascii;
///
/// let accented = "{\"n\":\"\u{e9}\"}";
/// assert_eq!(escape_non_ascii(accented), "{\"n\":\"\\u00e9\"}");
///
/// let astral = "{\"n\":\"\u{1f525}\"}";
/// assert_eq!(escape_non_ascii(astral), "{\"n\":\"\\ud83d\\udd25\"}");
/// ```
pub fn escape_non_ascii(json: &str) -> String {
    // Nothing to do for the overwhelmingly common all-ASCII case, and checking is far
    // cheaper than rebuilding the string.
    if json.is_ascii() {
        return json.to_string();
    }

    let mut out = String::with_capacity(json.len());
    let mut utf16 = [0u16; 2];
    for character in json.chars() {
        if character <= LAST_ASCII {
            out.push(character);
            continue;
        }
        for unit in character.encode_utf16(&mut utf16) {
            // Four hex digits, always: a JSON `\u` escape is fixed width.
            let _ = write!(out, "\\u{unit:04x}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_untouched() {
        let json = "{\"name\":\"Occult Mage\",\"rl\":150}";
        assert_eq!(escape_non_ascii(json), json);
    }

    #[test]
    fn latin1_becomes_a_single_escape() {
        assert_eq!(
            escape_non_ascii("\"Mis\u{e9}ricorde\""),
            "\"Mis\\u00e9ricorde\""
        );
    }

    #[test]
    fn bmp_above_latin1_becomes_a_single_escape() {
        assert_eq!(
            escape_non_ascii("\"\u{30c6}\u{30b9}\u{30c8}\""),
            "\"\\u30c6\\u30b9\\u30c8\""
        );
    }

    #[test]
    fn astral_becomes_a_surrogate_pair() {
        assert_eq!(escape_non_ascii("\"\u{1f525}\""), "\"\\ud83d\\udd25\"");
    }

    #[test]
    fn serde_escapes_survive_untouched() {
        // serde_json already escapes control characters; this pass must not double-escape
        // the backslashes it produced.
        let json = to_ascii_json("a\nb\u{1}c").expect("a string always serialises");
        assert_eq!(json, "\"a\\nb\\u0001c\"");
    }

    #[test]
    fn output_is_always_ascii() {
        let value = serde_json::json!({
            "name": "Dongerino \u{e9} \u{30c6}\u{30b9}\u{30c8} \u{1f525}",
            "nested": ["\u{fc}n\u{ef}c\u{f6}d\u{e9}", {"\u{30ad}\u{30fc}": "\u{5024}"}],
        });
        let json = to_ascii_json(&value).expect("a json value always serialises");
        assert!(json.is_ascii(), "{json}");
    }

    #[test]
    fn escaping_preserves_meaning() {
        let value =
            serde_json::json!({"name": "Dongerino \u{e9} \u{30c6}\u{30b9}\u{30c8} \u{1f525}"});
        let json = to_ascii_json(&value).expect("a json value always serialises");
        let reparsed: serde_json::Value =
            serde_json::from_str(&json).expect("escaped json still parses");
        assert_eq!(reparsed, value);
    }
}
