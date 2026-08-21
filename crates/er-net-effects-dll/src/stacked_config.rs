//! The `stacked_effects` list in `er-net-effects.toml`, and editing it in place.
//!
//! The selector applies ONE effect at a time: moving the cursor takes the last one back off and
//! puts the new one on. The stack is the other mode -- a set of effects that stay on regardless
//! of where the cursor is, so several can run at once. It lives in the config file rather than in
//! a sidecar because it is something a player curates and keeps between sessions, and because a
//! file they can read and hand-edit beats a binary blob they cannot.
//!
//! EDITING IT IN PLACE, NOT REWRITING IT. Numpad `+`/`-` change this list while the game is
//! running, so the DLL writes back a file the player also owns: their other keys, their comments
//! and their ordering all have to survive. So the rewrite replaces exactly the one
//! `stacked_effects` line (or appends it when absent) and copies every other byte through.

// Windows-only in practice; portable so the file rewriting is asserted by `cargo test` on the
// host, where a wrong answer costs a test rather than a player's config file.
#![cfg_attr(not(windows), allow(dead_code))]

pub(crate) const STACKED_EFFECTS_KEY: &str = "stacked_effects";

/// Parse a `[1, 2, 3]` value into ids, keeping order and dropping duplicates.
///
/// Tolerant on purpose -- this is a line a player hand-edits: whitespace anywhere, a trailing
/// comma, an empty list, and a bare id with no brackets are all accepted. An unparsable element
/// is skipped rather than voiding the whole list, because losing the entire stack over one typo
/// is a worse outcome than silently ignoring the typo.
pub(crate) fn parse_id_list(raw: &str) -> Vec<i32> {
    let trimmed = raw.trim();
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(trimmed);
    let mut ids = Vec::new();
    for token in inner.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if let Ok(id) = token.parse::<i32>()
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

/// Render ids as the value half of the config line.
pub(crate) fn format_id_list(ids: &[i32]) -> String {
    let joined = ids
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{joined}]")
}

/// Rewrite the config text so `stacked_effects` reads as `ids`, leaving everything else alone.
///
/// The key is replaced where it already is -- not moved to the end -- so a file the player has
/// organised stays organised. When the key is absent it is appended with the comment that
/// explains it, since a bare list appearing in their config with no explanation is worse than no
/// comment at all.
pub(crate) fn rewrite(config_text: &str, ids: &[i32]) -> String {
    let line = format!("{STACKED_EFFECTS_KEY} = {}", format_id_list(ids));
    let newline = if config_text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };

    let mut out = String::with_capacity(config_text.len() + line.len() + 8);
    let mut replaced = false;
    for raw_line in config_text.split_inclusive('\n') {
        let stripped = raw_line.trim_end_matches(['\n', '\r']);
        if !replaced && is_key_line(stripped, STACKED_EFFECTS_KEY) {
            out.push_str(&line);
            out.push_str(newline);
            replaced = true;
            continue;
        }
        out.push_str(raw_line);
    }

    if !replaced {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(newline);
        }
        out.push_str("# Effects that stay applied no matter where the selector cursor is.");
        out.push_str(newline);
        out.push_str("# Numpad + adds the highlighted effect, numpad - removes it.");
        out.push_str(newline);
        out.push_str(&line);
        out.push_str(newline);
    }
    out
}

/// Is this line an assignment of `key`, ignoring indentation and comment lines?
fn is_key_line(line: &str, key: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return false;
    }
    let Some((candidate, _)) = trimmed.split_once('=') else {
        return false;
    };
    candidate.trim() == key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_parses_with_whatever_spacing_a_human_typed() {
        assert_eq!(parse_id_list("[1, 2, 3]"), vec![1, 2, 3]);
        assert_eq!(parse_id_list("  [ 1,2 ,  3 , ]  "), vec![1, 2, 3]);
        assert_eq!(parse_id_list("[]"), Vec::<i32>::new());
        assert_eq!(parse_id_list(""), Vec::<i32>::new());
        assert_eq!(parse_id_list("491"), vec![491], "a bare id is still a list");
    }

    #[test]
    fn duplicates_and_typos_do_not_void_the_rest_of_the_list() {
        assert_eq!(parse_id_list("[7, 7, 8]"), vec![7, 8]);
        assert_eq!(
            parse_id_list("[7, banana, 8]"),
            vec![7, 8],
            "one bad element must not cost the player their whole stack"
        );
    }

    #[test]
    fn a_list_round_trips_through_its_written_form() {
        let ids = vec![491, 3507, 10790];
        assert_eq!(format_id_list(&ids), "[491, 3507, 10790]");
        assert_eq!(parse_id_list(&format_id_list(&ids)), ids);
        assert_eq!(format_id_list(&[]), "[]");
    }

    #[test]
    fn rewriting_replaces_the_line_where_it_stands_and_touches_nothing_else() {
        let config =
            "# a comment\nnetwork_sync = true\nstacked_effects = [1, 2]\ncatalog_dir = \"x\"\n";
        let out = rewrite(config, &[3]);
        assert_eq!(
            out,
            "# a comment\nnetwork_sync = true\nstacked_effects = [3]\ncatalog_dir = \"x\"\n"
        );
    }

    #[test]
    fn rewriting_appends_the_key_with_its_explanation_when_it_is_absent() {
        let config = "network_sync = true\n";
        let out = rewrite(config, &[491]);
        assert!(
            out.starts_with("network_sync = true\n"),
            "existing keys survive"
        );
        assert!(out.contains("stacked_effects = [491]"));
        assert!(
            out.contains("Numpad +"),
            "a list appearing with no explanation is worse than no comment"
        );
    }

    #[test]
    fn a_commented_out_key_is_not_mistaken_for_the_real_one() {
        let config = "# stacked_effects = [9]\nnetwork_sync = true\n";
        let out = rewrite(config, &[1]);
        assert!(
            out.contains("# stacked_effects = [9]"),
            "the commented line must survive verbatim"
        );
        assert!(out.contains("\nstacked_effects = [1]"));
    }

    #[test]
    fn a_file_with_no_trailing_newline_does_not_glue_the_key_onto_the_last_line() {
        let out = rewrite("network_sync = true", &[5]);
        assert!(out.contains("network_sync = true\n"));
        assert!(out.contains("stacked_effects = [5]"));
    }

    #[test]
    fn crlf_files_stay_crlf() {
        let out = rewrite("network_sync = true\r\nstacked_effects = [1]\r\n", &[2]);
        assert!(out.contains("stacked_effects = [2]\r\n"));
        assert!(!out.contains("stacked_effects = [2]\n\r"));
    }

    #[test]
    fn only_the_first_assignment_is_rewritten_and_a_later_one_is_left_visible() {
        // A duplicated key is a config the player has to fix; silently editing both would hide it.
        let out = rewrite("stacked_effects = [1]\nstacked_effects = [2]\n", &[9]);
        assert_eq!(out, "stacked_effects = [9]\nstacked_effects = [2]\n");
    }

    #[test]
    fn an_indented_key_is_still_the_key() {
        let out = rewrite("  stacked_effects = [1]\n", &[4]);
        assert_eq!(out, "stacked_effects = [4]\n");
    }
}
