//! A hand-rolled reader for the subset of TOML this DLL's config file uses.
//!
//! Hand-parsed rather than pulled through a TOML crate, for the same reason
//! `er-enemynpc-effects` and `er-refill-all` are: a settings file read once a second in a DLL
//! cross-compiled into the game process does not justify a dependency. Those two get away with a
//! four-line `find the key, split on '='` helper because their whole schema is flat. This one is
//! not -- it has named tables, and an open-ended family of them (`[chr.c4500]`, one per character
//! id the player wants to override), so "which section is this key in" is a question the parser
//! has to answer rather than ignore.
//!
//! # What it accepts, and why each piece is here
//!
//! * `key = value` at the top level, and inside `[section]` / `[section.sub]` headers.
//! * **Unknown sections and unknown keys are carried, never rejected.** `[chr.cXXXX]` is
//!   open-ended by design and a later layer adds more tables; a parser that refused what it did
//!   not recognise would turn every forward-looking edit into a file the DLL says is broken.
//! * Arrays of scalars -- `bands_m = [4.0, 12.0]`, `unusable = [3300, 3301]`.
//! * One level of inline table -- `pin = { r2 = 3046 }`.
//! * Several assignments on one line separated by `;`. Real TOML does not allow this and the
//!   shipped default file does not use it, but the schema this crate was handed was written that
//!   way, so a file a player copies from a plan or a wiki page parses instead of silently losing
//!   every assignment after the first.
//! * `#` comments, to end of line, outside quotes.
//!
//! # What it does not do
//!
//! No multi-line values, no `"""` strings, no `[[array of tables]]` semantics (a `[[x]]` header
//! is read as the section `x`, which is enough not to lose the keys under it), no escape
//! sequences inside strings, and no type system -- every value comes back as text and the caller
//! decides what it is. Anything this cannot represent belongs in a second file rather than a
//! second parser.

// Windows-only crate in practice; this module is pure text handling and stays ungated so its
// tests run on the host.
#![cfg_attr(not(windows), allow(dead_code))]

/// One `key = value`, tagged with the section it appeared under (`""` for the top level).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    section: String,
    key: String,
    value: String,
}

/// A parsed config file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Document {
    entries: Vec<Entry>,
    /// Every section header seen, in first-appearance order -- including ones with no keys under
    /// them, because `[chr.c4500]` with nothing in it is still the player naming a character.
    sections: Vec<String>,
}

impl Document {
    /// Read a whole file. This cannot fail: a line that is not a header and not an assignment is
    /// skipped, which is what makes a half-finished edit a partially-applied config rather than a
    /// rejected one.
    pub(crate) fn parse(text: &str) -> Self {
        let mut doc = Self::default();
        let mut section = String::new();
        for raw_line in text.lines() {
            let line = strip_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = section_header(line) {
                section = name.to_owned();
                if !doc.sections.iter().any(|seen| seen == &section) {
                    doc.sections.push(section.clone());
                }
                continue;
            }
            for statement in split_top_level(line, ';') {
                let Some((key, value)) = statement.split_once('=') else {
                    continue;
                };
                let key = key.trim();
                if key.is_empty() {
                    continue;
                }
                doc.entries.push(Entry {
                    section: section.clone(),
                    key: key.to_owned(),
                    value: value.trim().to_owned(),
                });
            }
        }
        doc
    }

    /// The raw text of a value, quotes and brackets included. First occurrence wins, matching the
    /// flat helper the other config crates use: a duplicated key is a mistake, and taking the
    /// first is at least stable across reloads.
    pub(crate) fn raw(&self, section: &str, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.section == section && entry.key == key)
            .map(|entry| entry.value.as_str())
    }

    /// A value with its surrounding quotes removed.
    pub(crate) fn scalar(&self, section: &str, key: &str) -> Option<&str> {
        self.raw(section, key).map(unquote)
    }

    /// The items of `[a, b, c]`, unquoted. `None` when the key is absent; an empty vector when the
    /// value is `[]`, which is a real setting ("nothing") and not a missing one.
    pub(crate) fn array(&self, section: &str, key: &str) -> Option<Vec<&str>> {
        let raw = self.raw(section, key)?;
        let inner = raw.strip_prefix('[')?.strip_suffix(']')?;
        Some(
            split_top_level(inner, ',')
                .into_iter()
                .filter(|item| !item.trim().is_empty())
                .map(unquote)
                .collect(),
        )
    }

    /// The pairs of `{ k = v, k2 = v2 }`, values unquoted.
    pub(crate) fn inline_table(&self, section: &str, key: &str) -> Option<Vec<(&str, &str)>> {
        let raw = self.raw(section, key)?;
        let inner = raw.strip_prefix('{')?.strip_suffix('}')?;
        Some(
            split_top_level(inner, ',')
                .into_iter()
                .filter_map(|item| item.split_once('='))
                .map(|(k, v)| (k.trim(), unquote(v)))
                .collect(),
        )
    }

    /// Every section named `<prefix><something>`, in file order, as the `<something>` part.
    ///
    /// This is what makes `[chr.c4500]` open-ended: the caller asks for what the file contains
    /// rather than looking up ids it already knows.
    pub(crate) fn sections_under(&self, prefix: &str) -> Vec<&str> {
        self.sections
            .iter()
            .filter_map(|name| name.strip_prefix(prefix))
            .filter(|rest| !rest.is_empty() && !rest.contains('.'))
            .collect()
    }
}

/// `[name]` / `[[name]]` -> `name`. `None` for anything else.
fn section_header(line: &str) -> Option<&str> {
    let inner = line
        .strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
        .or_else(|| line.strip_prefix('[').and_then(|r| r.strip_suffix(']')))?;
    Some(inner.trim())
}

/// Cut a `#` comment, ignoring one inside a quoted string.
fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (index, ch) in line.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..index],
            _ => {}
        }
    }
    line
}

/// Split on `separator`, but only where it is not inside quotes, brackets or braces.
///
/// Both callers need exactly this: `;` between statements must not split `pin = { a = 1; }` (were
/// anyone to write that), and `,` between array items must not split a nested `[1, 2]`.
fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_quotes = false;
    let mut start = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '[' | '{' if !in_quotes => depth += 1,
            ']' | '}' if !in_quotes => depth -= 1,
            _ if ch == separator && !in_quotes && depth == 0 => {
                parts.push(&text[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Trim, then drop one matched pair of surrounding double quotes.
fn unquote(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
enabled = true          # a trailing comment
hotkey  = "F9"

[target]
mode   = "lock_on"
chr_id = 0

[mapping]
bands_m = [4.0, 12.0]
model = "context"

[buttons]
r1 = "light"; r2 = "heavy"; l1 = "ranged"; l2 = "movement"

[chr.c4500]
speed_scale = 1.15
pin = { r2 = 3046 }
unusable = [3300, 3301]
usable = []

[chr.c2130]
speed_scale = 0.8
"#;

    /// The whole point of the section tag: `speed_scale` exists three times in this file and they
    /// are three different settings.
    #[test]
    fn a_key_is_scoped_to_its_section() {
        let doc = Document::parse(SAMPLE);
        assert_eq!(doc.scalar("", "enabled"), Some("true"));
        assert_eq!(doc.scalar("target", "mode"), Some("lock_on"));
        assert_eq!(doc.scalar("chr.c4500", "speed_scale"), Some("1.15"));
        assert_eq!(doc.scalar("chr.c2130", "speed_scale"), Some("0.8"));
        assert_eq!(doc.scalar("mapping", "speed_scale"), None);
        // ...and a top-level key is not visible from inside a table.
        assert_eq!(doc.scalar("target", "enabled"), None);
    }

    /// Quotes come off, inline comments do not survive, spacing does not matter.
    #[test]
    fn quotes_and_trailing_comments_are_stripped() {
        let doc = Document::parse(SAMPLE);
        assert_eq!(doc.scalar("", "hotkey"), Some("F9"));
        assert_eq!(doc.raw("", "hotkey"), Some("\"F9\""));
        assert_eq!(doc.scalar("", "enabled"), Some("true"));
    }

    /// A `#` inside a quoted value is part of the value. Key names in this schema can be spelled
    /// with punctuation, so a naive `split('#')` would truncate one.
    #[test]
    fn a_hash_inside_quotes_is_not_a_comment() {
        let doc = Document::parse("label = \"press # to open\" # real comment");
        assert_eq!(doc.scalar("", "label"), Some("press # to open"));
    }

    /// The `;`-separated form the handed-down schema uses for `[buttons]`.
    #[test]
    fn semicolon_separated_assignments_all_land() {
        let doc = Document::parse(SAMPLE);
        for (key, value) in [
            ("r1", "light"),
            ("r2", "heavy"),
            ("l1", "ranged"),
            ("l2", "movement"),
        ] {
            assert_eq!(doc.scalar("buttons", key), Some(value), "{key}");
        }
    }

    #[test]
    fn arrays_and_inline_tables_come_apart() {
        let doc = Document::parse(SAMPLE);
        assert_eq!(doc.array("mapping", "bands_m"), Some(vec!["4.0", "12.0"]));
        assert_eq!(
            doc.array("chr.c4500", "unusable"),
            Some(vec!["3300", "3301"])
        );
        assert_eq!(
            doc.inline_table("chr.c4500", "pin"),
            Some(vec![("r2", "3046")])
        );
    }

    /// An empty array is a setting, not an absent key. `usable = []` means "override nothing",
    /// and reading it as `None` would fall back to a default the player just cleared.
    #[test]
    fn an_empty_array_is_not_a_missing_key() {
        let doc = Document::parse(SAMPLE);
        assert_eq!(doc.array("chr.c4500", "usable"), Some(vec![]));
        assert_eq!(doc.array("chr.c4500", "nothing_here"), None);
    }

    /// The open-ended half of the schema. The parser is told a prefix and reports what the file
    /// happens to contain, so a chr id nobody has ever written down still arrives.
    #[test]
    fn dynamic_sections_are_enumerated_not_guessed() {
        let doc = Document::parse(SAMPLE);
        assert_eq!(doc.sections_under("chr."), vec!["c4500", "c2130"]);
        assert_eq!(doc.sections_under("nothing."), Vec::<&str>::new());
    }

    /// A section this build has never heard of must not cost the file its other settings. Later
    /// layers add tables, and a player reading a newer guide will paste one in early.
    #[test]
    fn an_unknown_section_is_carried_rather_than_rejected() {
        let doc = Document::parse(
            "enabled = true\n[from_a_later_layer]\nwhatever = 3\n[target]\nmode = \"nearest\"\n",
        );
        assert_eq!(doc.scalar("", "enabled"), Some("true"));
        assert_eq!(doc.scalar("target", "mode"), Some("nearest"));
        assert_eq!(doc.scalar("from_a_later_layer", "whatever"), Some("3"));
    }

    /// Junk lines are skipped, not fatal: a file being edited passes through half-written states
    /// and the poller reads it about once a second, so it will see one.
    #[test]
    fn a_malformed_line_does_not_lose_the_rest_of_the_file() {
        let doc = Document::parse("enabled = true\nthis is not an assignment\nhotkey = \"F8\"\n");
        assert_eq!(doc.scalar("", "enabled"), Some("true"));
        assert_eq!(doc.scalar("", "hotkey"), Some("F8"));
    }

    /// A duplicated key resolves the same way on every reload; the alternative is a config whose
    /// meaning depends on which pass read it.
    #[test]
    fn a_duplicated_key_resolves_to_the_first_one_every_time() {
        let doc = Document::parse("hotkey = \"F7\"\nhotkey = \"F8\"\n");
        assert_eq!(doc.scalar("", "hotkey"), Some("F7"));
    }

    #[test]
    fn an_empty_document_reads_as_empty_rather_than_failing() {
        let doc = Document::parse("");
        assert_eq!(doc.scalar("", "anything"), None);
        assert_eq!(doc.sections_under("chr."), Vec::<&str>::new());
        assert_eq!(Document::parse("# only a comment\n"), doc);
    }
}
