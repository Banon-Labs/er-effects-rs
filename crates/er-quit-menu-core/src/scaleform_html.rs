//! The small Scaleform-HTML string shapes a `05_010_ProfileSelect` row's text is written in.
//!
//! Moved out of `er-quickload` on 2026-09-11 with the row chrome that composes them, so the
//! System>Quit **Load Character from File** row's browse surface reads the same with no product
//! DLL behind it. Pure string work, no game memory: the game's `SetText` wrapper takes UTF-16 and
//! Scaleform renders the markup, so everything here is decode, reshape, re-encode.
//!
//! # The one shape that matters
//!
//! A row field is written as `<p align="...">body</p>`. Merging two lines therefore cannot be a
//! concatenation -- two `<p>` blocks stack vertically, and a browse row has one visual line to
//! spend. [`merge_scaleform_html_utf16_lines`] unwraps each paragraph, joins the bodies with a
//! dimmed separator, and re-wraps the pair once.

/// Decode a nul-terminated UTF-16 field value into a `String`, or `None` when it is empty or not
/// valid UTF-16 -- which is what an unwritten field holds.
pub fn decode_scaleform_html_line(line: &[u16]) -> Option<String> {
    let body = line.strip_suffix(&[0]).unwrap_or(line);
    if body.is_empty() {
        return None;
    }
    String::from_utf16(body).ok().filter(|s| !s.is_empty())
}

/// Split a `<p align="...">body</p>` wrapper off, returning the body and whether one was found.
/// A line that is not wrapped is returned whole, so this is safe to run on any field value.
pub fn scaleform_html_body(line: &str) -> (&str, bool) {
    if let Some(rest) = line.strip_prefix("<p align=\"")
        && let Some((_align, body)) = rest.split_once("\">")
        && let Some(body) = body.strip_suffix("</p>")
    {
        return (body, true);
    }
    (line, false)
}

/// Join two field lines onto one visual line, nul-terminated for the native setter.
///
/// Either line being undecodable degrades to the other rather than to nothing, so a row missing one
/// half still says what it can.
pub fn merge_scaleform_html_utf16_lines(first: &[u16], second: &[u16]) -> Vec<u16> {
    let Some(first) = decode_scaleform_html_line(first) else {
        return second.to_vec();
    };
    let Some(second) = decode_scaleform_html_line(second) else {
        return first.encode_utf16().chain(core::iter::once(0)).collect();
    };
    let (first, _) = scaleform_html_body(&first);
    let (second, _) = scaleform_html_body(&second);
    let merged = format!(
        "<p align=\"left\">{first} <font size=\"16\" color=\"#8f887a\">/</font> {second}</p>"
    );
    merged.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Nul-terminated UTF-16 for the native setter, which reads until it finds one.
pub fn nul_terminated_utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(core::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u16> {
        nul_terminated_utf16(s)
    }

    /// The merge produces exactly one paragraph. Two would stack vertically, and a browse row has
    /// one visual line to spend -- which is the whole reason this is not a concatenation.
    #[test]
    fn merging_two_paragraphs_yields_one() {
        let merged = merge_scaleform_html_utf16_lines(
            &utf16("<p align=\"left\">alpha</p>"),
            &utf16("<p align=\"right\">beta</p>"),
        );
        let text = String::from_utf16(merged.strip_suffix(&[0]).unwrap()).unwrap();
        assert_eq!(text.matches("<p ").count(), 1, "{text}");
        assert!(text.contains("alpha"), "{text}");
        assert!(text.contains("beta"), "{text}");
    }

    /// A row missing one half says what it can rather than going blank.
    #[test]
    fn an_undecodable_half_degrades_to_the_other() {
        let only_second = merge_scaleform_html_utf16_lines(&[0], &utf16("beta"));
        assert_eq!(only_second, utf16("beta"));
        let only_first = merge_scaleform_html_utf16_lines(&utf16("alpha"), &[0]);
        assert_eq!(String::from_utf16(&only_first).unwrap(), "alpha\0");
    }

    /// An unwrapped line passes through the body split untouched, so the merge is safe to run on
    /// any field value rather than only on ones this mod wrote.
    #[test]
    fn an_unwrapped_line_is_its_own_body() {
        assert_eq!(scaleform_html_body("plain"), ("plain", false));
        assert_eq!(
            scaleform_html_body("<p align=\"left\">inner</p>"),
            ("inner", true)
        );
    }
}
