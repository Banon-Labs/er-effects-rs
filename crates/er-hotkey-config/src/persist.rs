//! Writing one setting back into a config file the player also owns.
//!
//! # Why not just render the file
//!
//! Several of these DLLs change a setting from in-game and have to store it: a hotkey that toggles
//! a feature is worthless if the feature is off again on the next launch. The obvious
//! implementation -- keep the settings in a struct and write the struct out -- destroys the file.
//! These configs are mostly comments, and the comments are the feature's only documentation: what
//! the key names are, which SpEffect row is which, a commented-out alternative left in place for
//! the player to re-enable later. A renderer emits the four keys it knows about and every one of
//! those is gone, silently, the first time the player presses the key.
//!
//! So [`set_scalar`] does the other thing: it copies the file through byte for byte and rewrites
//! exactly one line. Comments, blank lines, ordering, indentation, trailing comments, keys this
//! build has never heard of and lines that are commented out all survive, because nothing ever
//! looks at them.
//!
//! # Why the write is staged and renamed
//!
//! [`write_atomic`] writes a sibling temp file and renames it over the destination. `fs::write` is
//! truncate-then-write: a crash or a full disk part way through leaves a config that is neither
//! the old one nor the new one, and the parser's own tolerance makes that worse rather than better
//! -- a half-written file usually still parses, into something the player did not ask for. Every
//! failure of the staged form instead ends with the destination holding exactly its previous
//! contents.
//!
//! The rename is also what makes the write invisible to a reader that catches it mid-flight: the
//! game re-reads this file about once a second (see [`crate::reload::HotFile`]), and on the
//! target these DLLs actually ship to -- a Windows PE under Wine -- `fs::rename` is `MoveFileExW`
//! with `MOVEFILE_REPLACE_EXISTING`, which Wine services with `rename(2)` on the underlying Linux
//! filesystem. A reader sees the old file or the new one, never a partial one.
//!
//! # The lost update this does not solve
//!
//! Read-modify-write is not a lock. If the player's editor saves between the read and the rename,
//! the rename wins and their save is gone. That window is microseconds wide and there is no
//! portable way to close it here; the mitigation is that only one line is ever rewritten, so
//! everything the two writers disagree about is confined to that key. The caller should re-read
//! after writing and adopt what came back -- [`crate::reload::HotFile::adopt`] -- which both keeps
//! its own write from reading as somebody's edit and picks up an edit that landed just before it.

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

/// Suffix of the staging file [`write_atomic`] renames into place.
///
/// Distinctive on purpose: it sits in the game directory next to the real config, so it has to be
/// obvious what left it there if a crash ever does.
const TEMP_SUFFIX: &str = ".er-config-write.tmp";

/// Rewrite `key`'s value in place, leaving every other byte of `text` alone.
///
/// The line keeps its position, its indentation and its trailing `#` comment; only the value
/// between the `=` and the comment is replaced. When `key` has no uncommented assignment anywhere
/// in the file, `comment_when_absent` (raw lines, `#` included, or empty for none) and the new
/// assignment are appended -- a bare key appearing in somebody's config with no explanation is
/// worse than no key at all.
///
/// Only the first uncommented assignment is rewritten, matching how the hand-rolled readers in
/// this workspace resolve a duplicated key: first one wins.
///
/// `value` is written verbatim, so it must not contain a `#` or a newline.
#[must_use]
pub fn set_scalar(text: &str, key: &str, value: &str, comment_when_absent: &str) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = String::with_capacity(text.len() + value.len() + comment_when_absent.len() + 16);
    let mut replaced = false;

    for raw_line in text.split_inclusive('\n') {
        if !replaced && let Some(rewritten) = rewrite_assignment(raw_line, key, value) {
            out.push_str(&rewritten);
            replaced = true;
            continue;
        }
        out.push_str(raw_line);
    }

    if !replaced {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(newline);
        }
        for comment in comment_when_absent.lines() {
            out.push_str(comment);
            out.push_str(newline);
        }
        out.push_str(key);
        out.push_str(" = ");
        out.push_str(value);
        out.push_str(newline);
    }
    out
}

/// Rewrite one line if it assigns `key`, or `None` if it does not.
///
/// Splits the line's terminator off first so a `\r\n` file stays a `\r\n` file, and keeps the
/// trailing comment because a player who wrote `effect_id = 503350 # bewitching` expects that note
/// to be there next time they look.
fn rewrite_assignment(raw_line: &str, key: &str, value: &str) -> Option<String> {
    let body = raw_line.trim_end_matches(['\n', '\r']);
    let terminator = &raw_line[body.len()..];
    let trimmed = body.trim_start();
    if trimmed.starts_with('#') {
        return None;
    }
    let (name, rest) = trimmed.split_once('=')?;
    if name.trim() != key {
        return None;
    }
    let indent = &body[..body.len() - trimmed.len()];
    let mut line = format!("{indent}{key} = {value}");
    if let Some((_, comment)) = rest.split_once('#') {
        line.push_str(" #");
        line.push_str(comment);
    }
    line.push_str(terminator);
    Some(line)
}

/// Where [`write_atomic`] stages its replacement: a sibling, so the rename cannot cross a mount.
fn temp_sibling(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?;
    let mut temp = OsString::from(name);
    temp.push(TEMP_SUFFIX);
    Some(path.with_file_name(temp))
}

/// Replace `path` with `contents`, or leave it exactly as it was.
///
/// The staged file's length is re-read before the rename, because a full disk can end a buffered
/// write without the write call itself reporting an error -- which would rename a truncated config
/// over a good one.
///
/// Every failure path removes the staging file, so a game directory does not slowly fill with
/// `.tmp` leftovers from a read-only or full volume.
pub fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    let Some(temp) = temp_sibling(path) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "'{}' has no file name to stage a replacement beside",
                path.display()
            ),
        ));
    };
    let _ = fs::remove_file(&temp);
    if let Err(err) = fs::write(&temp, contents) {
        let _ = fs::remove_file(&temp);
        return Err(err);
    }
    let staged = match fs::metadata(&temp) {
        Ok(meta) => meta.len(),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            return Err(err);
        }
    };
    if staged != contents.len() as u64 {
        let _ = fs::remove_file(&temp);
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            format!(
                "the staging copy '{}' came out {staged} bytes, expected {}",
                temp.display(),
                contents.len()
            ),
        ));
    }
    if let Err(err) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reload::{FileChange, HotFile};

    /// A config with everything a rewrite could destroy: a comment block, a commented-out
    /// alternative the player means to re-enable, trailing comments, blank lines, an indented
    /// line, and a key this build has never heard of.
    const HOSTILE: &str = "\
# er-example configuration.
#
# The comments ARE the documentation.
hotkey = \"ctrl+alt+c\"
# effect_id = 20503350 # bewitching
effect_id = 1653000 # darkness

  indented = 1
future_setting = \"a key this build does not know\"
enabled = false
";

    fn changed_lines(before: &str, after: &str) -> Vec<(String, String)> {
        let before: Vec<&str> = before.lines().collect();
        let after: Vec<&str> = after.lines().collect();
        assert_eq!(
            before.len(),
            after.len(),
            "the rewrite changed the line COUNT"
        );
        before
            .iter()
            .zip(after.iter())
            .filter(|(a, b)| a != b)
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    /// The DELIVERABLE. Rewriting one value changes exactly one line and nothing else -- the
    /// comments, the commented-out alternative, the blank line, the indentation and the unknown
    /// key all come through byte for byte.
    #[test]
    fn rewriting_one_value_changes_exactly_one_line() {
        let out = set_scalar(HOSTILE, "enabled", "true", "# unused");
        assert_eq!(
            changed_lines(HOSTILE, &out),
            vec![("enabled = false".to_owned(), "enabled = true".to_owned())]
        );
        assert!(out.contains("# effect_id = 20503350 # bewitching"));
        assert!(out.contains("future_setting = \"a key this build does not know\""));
        assert!(out.ends_with('\n'), "the trailing newline survives");
    }

    /// A trailing comment on the rewritten line is the player's note about that setting. Losing it
    /// is losing the reason they chose the value.
    #[test]
    fn a_trailing_comment_on_the_rewritten_line_survives() {
        let out = set_scalar(
            "effect_id = 1653000 # darkness\n",
            "effect_id",
            "503350",
            "",
        );
        assert_eq!(out, "effect_id = 503350 # darkness\n");
    }

    /// Indentation and the line's position are preserved: a file somebody organised stays
    /// organised, and the key does not migrate to the bottom.
    #[test]
    fn indentation_and_position_are_preserved() {
        let out = set_scalar("a = 1\n  enabled = false\nz = 9\n", "enabled", "true", "");
        assert_eq!(out, "a = 1\n  enabled = true\nz = 9\n");
    }

    /// A commented-out assignment is not an assignment -- rewriting it would silently re-enable a
    /// line the player deliberately switched off.
    #[test]
    fn a_commented_out_assignment_is_not_the_one_rewritten() {
        let out = set_scalar("# enabled = true\nenabled = false\n", "enabled", "true", "");
        assert_eq!(out, "# enabled = true\nenabled = true\n");
    }

    /// An absent key is appended with its explanation. This is the upgrade path: a config written
    /// by an older build has no such line, and the first toggle has to add one.
    #[test]
    fn an_absent_key_is_appended_with_its_comment() {
        let out = set_scalar(
            "hotkey = \"f9\"\n",
            "enabled",
            "true",
            "# Whether it is on.",
        );
        assert_eq!(
            out,
            "hotkey = \"f9\"\n# Whether it is on.\nenabled = true\n"
        );
    }

    /// ...and appending to a file with no trailing newline does not glue the new key onto the last
    /// line, which would corrupt both.
    #[test]
    fn appending_to_a_file_with_no_trailing_newline_still_starts_a_line() {
        let out = set_scalar("hotkey = \"f9\"", "enabled", "true", "");
        assert_eq!(out, "hotkey = \"f9\"\nenabled = true\n");
    }

    /// A CRLF file stays a CRLF file. These configs are edited on Windows as often as not, and
    /// rewriting one line in LF would make the next diff show the whole file.
    #[test]
    fn a_crlf_file_stays_crlf() {
        let out = set_scalar("a = 1\r\nenabled = false\r\n", "enabled", "true", "");
        assert_eq!(out, "a = 1\r\nenabled = true\r\n");
        let appended = set_scalar("a = 1\r\n", "enabled", "true", "# note");
        assert_eq!(appended, "a = 1\r\n# note\r\nenabled = true\r\n");
    }

    /// A duplicated key: the readers in this workspace take the first, so the writer rewrites the
    /// first. Rewriting the second would leave the file saying one thing and meaning another.
    #[test]
    fn a_duplicated_key_is_rewritten_where_the_reader_would_look() {
        let out = set_scalar("enabled = false\nenabled = false\n", "enabled", "true", "");
        assert_eq!(out, "enabled = true\nenabled = false\n");
    }

    /// A key that merely starts with the name is a different key.
    #[test]
    fn a_key_that_only_shares_a_prefix_is_not_matched() {
        let out = set_scalar("enabled_extra = 1\n", "enabled", "true", "");
        assert_eq!(out, "enabled_extra = 1\nenabled = true\n");
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "er-hotkey-config-persist-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// The staged write replaces the file and leaves nothing behind. Proven against a real
    /// filesystem rather than asserted, because the whole point of the staging is a syscall
    /// sequence.
    #[test]
    fn an_atomic_write_replaces_the_file_and_leaves_no_staging_copy() {
        let dir = scratch_dir("replace");
        let path = dir.join("er-example.toml");
        fs::write(&path, "enabled = false\n").expect("seed");

        write_atomic(&path, "enabled = true\n").expect("atomic write");

        assert_eq!(
            fs::read_to_string(&path).expect("read back"),
            "enabled = true\n"
        );
        let staging = dir.join(format!("er-example.toml{TEMP_SUFFIX}"));
        assert!(
            !staging.exists(),
            "staging copy left behind: {}",
            staging.display()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A write that cannot be staged leaves the destination alone. This is the read-only game
    /// directory case, and the requirement is that it degrades to "not persisted", never to a
    /// destroyed config.
    #[test]
    fn a_write_that_cannot_be_staged_leaves_the_destination_alone() {
        let dir = scratch_dir("nostage");
        // A directory in the staging file's place: `fs::write` to it fails on every platform.
        let path = dir.join("er-example.toml");
        fs::write(&path, "enabled = false\n").expect("seed");
        let blocker = dir.join(format!("er-example.toml{TEMP_SUFFIX}"));
        fs::create_dir(&blocker).expect("blocker");

        assert!(write_atomic(&path, "enabled = true\n").is_err());
        assert_eq!(
            fs::read_to_string(&path).expect("read back"),
            "enabled = false\n",
            "a failed write must not touch the destination"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A path with no file name cannot be staged beside anything, and says so instead of panicking
    /// inside a game process.
    #[test]
    fn a_path_with_no_file_name_is_an_error_not_a_panic() {
        assert!(write_atomic(Path::new("/"), "a = 1").is_err());
    }

    /// End to end, and the feedback loop this module exists inside: write the file, adopt what
    /// came back, and the watcher must stay silent -- our own write is not somebody's edit. A real
    /// edit right after it is still seen.
    #[test]
    fn our_own_write_is_not_reported_back_as_an_edit() {
        let dir = scratch_dir("adopt");
        let path = dir.join("er-example.toml");
        fs::write(&path, "enabled = false\n").expect("seed");

        let mut hot = HotFile::with_interval(&path, 0);
        assert!(hot.poll().is_some(), "the first read is a load");

        let updated = set_scalar(
            &fs::read_to_string(&path).expect("read"),
            "enabled",
            "true",
            "",
        );
        write_atomic(&path, &updated).expect("atomic write");
        hot.adopt(fs::read_to_string(&path).expect("read back"));

        assert_eq!(hot.poll(), None, "our own write must not churn the watcher");
        assert_eq!(
            hot.poll(),
            None,
            "and must not churn on the poll after that"
        );

        fs::write(&path, "enabled = true\nhotkey = \"f9\"\n").expect("hand edit");
        assert_eq!(
            hot.poll(),
            Some(FileChange::Text(
                "enabled = true\nhotkey = \"f9\"\n".to_owned()
            )),
            "a real edit after our write is still seen"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
