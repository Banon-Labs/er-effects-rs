// IS THE DESTINATION THE LOADED SAVE? -- identity, path normalization and all-or-nothing writes
// for the save-destination commit.
//
// Extracted into `er-quit-menu` in S7. The product DLL keeps a root shim that re-exports this
// module's pure path/identity/write helpers while native runtime ownership remains in the product.
//
// # Why a string compare is not an answer
//
// The commit asks one question before it writes anything: is the destination the user browsed to
// the SAME FILE as the loaded save? A "no" that is really a "yes" is the destructive answer. It
// makes the commit seed and redirect the loaded save onto itself; the native write then lands
// correctly, the live-file stamp moves because it IS the destination, the safety net reads that as
// "the redirect leaked", and it writes the pre-fire snapshot back over the save that just
// succeeded -- logging that it restored the file it destroyed.
//
// One file has many spellings, and this process sees several of them at once:
//
//   * `C:\users\steamuser\AppData\Roaming\EldenRing\<id>\ER0000.sl2` -- what the game opens, and
//     what `%APPDATA%` resolves to inside the Wine prefix;
//   * `Z:\home\<user>\...\pfx\drive_c\users\steamuser\AppData\Roaming\EldenRing\<id>\ER0000.sl2`
//     -- the same bytes reached through the `Z:` root, which is exactly what the destination
//     browser produces when its start directory came from a Linux-form remembered path;
//   * either of those with `/` separators, a trailing separator, mixed case, a `.` or `..`
//     segment, or through a symlinked directory.
//
// So the identity test is a HANDLE test: open both and compare the volume serial plus the file
// index, which is what `BY_HANDLE_FILE_INFORMATION` is for and what Wine derives from the
// underlying device and inode. Normalization below is a cheap pre-pass that can only answer
// "same" (identical normalized text really is the same file); it is never allowed to answer
// "different" on its own.
//
// # Three answers, not two
//
// [`SaveDestIdentity`] has an `Unknown`. A probe that neither succeeded nor established absence
// leaves the question open, and the caller's rule is to refuse to fire rather than guess -- a
// refused save is recoverable, a save written over the wrong file is not.

use std::{
    fs,
    path::{Path, PathBuf},
};

#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt as _;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle as _;

#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};

/// `FILE_FLAG_BACKUP_SEMANTICS`. Required to obtain a HANDLE to a DIRECTORY through
/// `CreateFileW`, which is how the redirect establishes that an incoming write-open's parent is
/// the loaded save's folder under a different spelling.
#[cfg(windows)]
const SAVE_DEST_FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

/// Filesystem identity of one open file or directory: `(volume serial, file index)`. Equal ids
/// mean one file, whatever the two paths looked like.
#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SaveDestFileId {
    volume: u32,
    index: u64,
}

/// Result of asking the filesystem to identify a path.
#[cfg(windows)]
pub enum SaveDestProbe {
    /// The path resolved and the filesystem gave it a usable identity.
    Id(SaveDestFileId),
    /// The path does not exist. Decisive: a file that is not there is not the loaded save.
    Absent,
    /// The path could not be opened, or the filesystem returned no usable identity. NOT a
    /// synonym for absent -- a sharing violation lands here, and so would a filesystem that
    /// reports a zero volume serial and a zero file index.
    Unreadable,
}

/// Whether two paths are the same file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SaveDestIdentity {
    /// Proven the same file: identical normalized text, or equal handle identities.
    SameFile,
    /// Proven distinct: two usable identities that differ, or one path that does not exist while
    /// the other does.
    Distinct,
    /// Neither could be proven. Callers must refuse rather than pick a side.
    Unknown,
}

#[cfg(windows)]
fn save_dest_file_id_from_handle(handle: *mut c_void) -> Option<SaveDestFileId> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(windows::Win32::Foundation::HANDLE(handle), &raw mut info)
    }
    .ok()?;
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    // All-zero is the filesystem declining to identify the file, not an identity. Two different
    // files would then compare equal, which is the one mistake this whole module exists to stop.
    if index == 0 && info.dwVolumeSerialNumber == 0 {
        return None;
    }
    Some(SaveDestFileId {
        volume: info.dwVolumeSerialNumber,
        index,
    })
}

#[cfg(windows)]
fn save_dest_probe_opened(file: std::io::Result<fs::File>) -> SaveDestProbe {
    match file {
        Ok(file) => match save_dest_file_id_from_handle(file.as_raw_handle()) {
            Some(id) => SaveDestProbe::Id(id),
            None => SaveDestProbe::Unreadable,
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => SaveDestProbe::Absent,
        Err(_) => SaveDestProbe::Unreadable,
    }
}

/// Identify a FILE. Read access only, and the handle is closed immediately -- nothing here may
/// perturb a file the game may be holding.
#[cfg(windows)]
pub fn save_dest_probe_file(path: &Path) -> SaveDestProbe {
    save_dest_probe_opened(fs::File::open(path))
}

/// Identify a DIRECTORY. Same contract as [`save_dest_probe_file`]; the backup-semantics flag is
/// what makes a directory openable at all.
#[cfg(windows)]
pub fn save_dest_probe_dir(path: &Path) -> SaveDestProbe {
    save_dest_probe_opened(
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(SAVE_DEST_FILE_FLAG_BACKUP_SEMANTICS)
            .open(path),
    )
}

#[cfg(windows)]
fn save_dest_identity_from_probes(left: SaveDestProbe, right: SaveDestProbe) -> SaveDestIdentity {
    match (left, right) {
        (SaveDestProbe::Id(left), SaveDestProbe::Id(right)) => {
            if left == right {
                SaveDestIdentity::SameFile
            } else {
                SaveDestIdentity::Distinct
            }
        }
        // One exists and the other does not: decisive, and the common case for a brand-new
        // destination created through the browser's `[ new ]` row.
        (SaveDestProbe::Id(_), SaveDestProbe::Absent)
        | (SaveDestProbe::Absent, SaveDestProbe::Id(_)) => SaveDestIdentity::Distinct,
        _ => SaveDestIdentity::Unknown,
    }
}

/// Are `target` and `live` the same file?
///
/// Normalized text first (an identical spelling cannot be two files, and it costs no I/O), then
/// the handle test. `Unknown` when the filesystem would not say.
#[cfg(windows)]
pub fn save_dest_file_identity(target: &Path, live: &Path) -> SaveDestIdentity {
    if let (Some(target_text), Some(live_text)) = (
        target.to_str().and_then(save_dest_normalize_path),
        live.to_str().and_then(save_dest_normalize_path),
    ) && target_text == live_text
    {
        return SaveDestIdentity::SameFile;
    }
    save_dest_identity_from_probes(save_dest_probe_file(target), save_dest_probe_file(live))
}

/// Are `left` and `right` the same DIRECTORY? Used by the write-open redirect to recognize the
/// loaded save's folder reached through another spelling.
#[cfg(windows)]
pub fn save_dest_dir_identity(left: &Path, right: &Path) -> SaveDestIdentity {
    save_dest_identity_from_probes(save_dest_probe_dir(left), save_dest_probe_dir(right))
}

/// Canonical text form of a path, for comparisons that must not care about spelling:
/// `Z:`-prefixed if Linux-form, `\` separators, collapsed separator runs, resolved `.` and `..`
/// segments, no trailing separator, ASCII-lowercased.
///
/// This can prove "same" and must never be used to prove "different" -- it knows nothing about
/// symlinks or the `C:` / `Z:` views of one Wine prefix.
pub fn save_dest_normalize_path(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let windows = if trimmed.starts_with('/') {
        format!("Z:{}", trimmed.replace('/', "\\"))
    } else {
        trimmed.replace('/', "\\")
    };
    let lower = windows.to_ascii_lowercase();
    // Split the root off first so a `..` can never walk above it.
    let bytes = lower.as_bytes();
    let (root, rest) = if let Some(rest) = lower.strip_prefix("\\\\") {
        ("\\\\".to_owned(), rest)
    } else if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        (lower[..2].to_owned(), &lower[2..])
    } else {
        (String::new(), lower.as_str())
    };
    let mut parts: Vec<&str> = Vec::new();
    for segment in rest.split('\\') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        // Root only. Kept with its separator so `c:` and `c:\` are one thing.
        return Some(format!("{root}\\"));
    }
    Some(format!("{root}\\{}", parts.join("\\")))
}

/// Normalized form of a NUL-terminated wide Windows path, or `None` when it is empty or not
/// valid UTF-16 (which no save path this flow cares about ever is).
pub fn save_dest_normalize_wide(path: &[u16]) -> Option<String> {
    let end = path
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(path.len());
    if end == 0 {
        return None;
    }
    String::from_utf16(&path[..end])
        .ok()
        .as_deref()
        .and_then(save_dest_normalize_path)
}

/// Directory part of an already-normalized path (no trailing separator except at a root), or
/// `None` when it has none.
pub fn save_dest_normalized_parent(normalized: &str) -> Option<&str> {
    let cut = normalized.rfind('\\')?;
    if cut == 0 {
        return Some(&normalized[..1]);
    }
    // `c:\er0000.sl2` -> `c:\`, not `c:`.
    if normalized[..cut].ends_with(':') {
        return Some(&normalized[..cut + 1]);
    }
    Some(&normalized[..cut])
}

/// Sibling staging path used by [`save_dest_write_atomic`]. Its `.tmp` suffix keeps it out of the
/// destination browser's `.sl2`/`.co2` listing and out of the redirect's own leaf filter.
fn save_dest_temp_sibling(path: &Path, tag: &str) -> Option<PathBuf> {
    let name = path.file_name()?;
    let mut temp = name.to_os_string();
    temp.push(format!(".er-save-dest-{tag}.tmp"));
    Some(path.with_file_name(temp))
}

/// Replace `path` with `bytes`, or leave it exactly as it was.
///
/// `fs::write` is truncate-then-write: a failure part way through ~29 MB leaves a save container
/// that is neither the old one nor the new one, and no caller can undo that. Writing a sibling
/// first and renaming it into place means every failure mode ends with the destination holding
/// its previous contents, so an aborted commit is a commit that did nothing.
///
/// The length is re-read from the staging file before the rename, because a full disk can end a
/// buffered write without the write call itself failing.
pub fn save_dest_write_atomic(path: &Path, bytes: &[u8], tag: &str) -> Result<(), String> {
    let Some(temp) = save_dest_temp_sibling(path, tag) else {
        return Err(format!(
            "'{}' has no file name to stage a replacement beside",
            path.display()
        ));
    };
    let _ = fs::remove_file(&temp);
    if let Err(err) = fs::write(&temp, bytes) {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "could not write the staging copy '{}': {err}",
            temp.display()
        ));
    }
    match fs::metadata(&temp) {
        Ok(meta) if meta.len() == bytes.len() as u64 => {}
        Ok(meta) => {
            let _ = fs::remove_file(&temp);
            return Err(format!(
                "the staging copy '{}' came out {} bytes, expected {}",
                temp.display(),
                meta.len(),
                bytes.len()
            ));
        }
        Err(err) => {
            let _ = fs::remove_file(&temp);
            return Err(format!(
                "could not stat the staging copy '{}': {err}",
                temp.display()
            ));
        }
    }
    if let Err(err) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "could not move the staging copy over '{}': {err}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod save_dest_identity_tests {
    use super::*;

    #[test]
    fn a_linux_form_path_normalizes_onto_the_z_root() {
        assert_eq!(
            save_dest_normalize_path("/home/user/EldenRing/ER0000.sl2").as_deref(),
            Some(r"z:\home\user\eldenring\er0000.sl2")
        );
    }

    #[test]
    fn case_separators_dot_segments_and_trailing_slashes_all_collapse() {
        let expected = Some(r"c:\users\steamuser\eldenring\er0000.sl2".to_owned());
        assert_eq!(
            save_dest_normalize_path(r"C:\Users\SteamUser\EldenRing\ER0000.sl2"),
            expected
        );
        assert_eq!(
            save_dest_normalize_path(r"C:/Users//SteamUser/./EldenRing/ER0000.sl2"),
            expected
        );
        assert_eq!(
            save_dest_normalize_path(r"C:\Users\SteamUser\x\..\EldenRing\ER0000.sl2"),
            expected
        );
    }

    /// The spelling difference this whole module exists for: two texts that are one file. They
    /// must NOT normalize equal -- only the handle test can join them -- which is why
    /// normalization is allowed to prove "same" and never "different".
    #[test]
    fn the_two_wine_spellings_of_one_save_do_not_normalize_equal() {
        let dos = save_dest_normalize_path(
            r"C:\users\steamuser\AppData\Roaming\EldenRing\7656\ER0000.sl2",
        );
        let unix = save_dest_normalize_path(
            "/home/user/prefix/pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing/7656/ER0000.sl2",
        );
        assert!(dos.is_some() && unix.is_some());
        assert_ne!(dos, unix);
    }

    #[test]
    fn a_root_keeps_its_separator() {
        assert_eq!(save_dest_normalize_path(r"C:\").as_deref(), Some(r"c:\"));
        assert_eq!(save_dest_normalize_path("C:").as_deref(), Some(r"c:\"));
    }

    #[test]
    fn the_parent_of_a_normalized_path_drops_the_leaf_but_keeps_a_root() {
        assert_eq!(
            save_dest_normalized_parent(r"c:\users\steamuser\er0000.sl2"),
            Some(r"c:\users\steamuser")
        );
        assert_eq!(save_dest_normalized_parent(r"c:\er0000.sl2"), Some(r"c:\"));
        assert_eq!(save_dest_normalized_parent("er0000.sl2"), None);
    }

    #[test]
    fn a_wide_path_normalizes_through_its_nul() {
        let mut wide: Vec<u16> = r"C:\Save\ER0000.SL2".encode_utf16().collect();
        wide.push(0);
        wide.extend_from_slice(&[0x41, 0x42]);
        assert_eq!(
            save_dest_normalize_wide(&wide).as_deref(),
            Some(r"c:\save\er0000.sl2")
        );
        assert_eq!(save_dest_normalize_wide(&[0]), None);
    }

    /// THE DEFECT, END TO END. Two different paths that are one file must come back `SameFile`,
    /// and the normalized text must NOT be what says so -- that is precisely the case a string
    /// compare gets wrong, and getting it wrong makes the commit redirect the loaded save onto
    /// itself and then restore a pre-save snapshot over the save that just succeeded.
    ///
    /// A hard link is the portable stand-in for the Wine `C:` / `Z:` views of one prefix: same
    /// volume, same file index, unrelated paths. Skipped rather than failed where the filesystem
    /// refuses to make one, since that says nothing about the code under test.
    #[cfg(windows)]
    #[test]
    fn two_paths_that_are_one_file_are_the_same_file() {
        let dir = std::env::temp_dir().join("er-save-dest-identity-alias");
        let _ = fs::remove_dir_all(&dir);
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let real = dir.join("ER0000.sl2");
        let alias = dir.join("another-name.sl2");
        if fs::write(&real, b"one container").is_err() {
            let _ = fs::remove_dir_all(&dir);
            return;
        }
        if fs::hard_link(&real, &alias).is_err() {
            let _ = fs::remove_dir_all(&dir);
            return;
        }
        assert_ne!(
            real.to_str().and_then(save_dest_normalize_path),
            alias.to_str().and_then(save_dest_normalize_path),
            "the two spellings must differ, or this proves nothing about the handle test"
        );
        assert!(matches!(
            save_dest_file_identity(&alias, &real),
            SaveDestIdentity::SameFile
        ));
        let unrelated = dir.join("unrelated.sl2");
        let _ = fs::write(&unrelated, b"another container");
        assert!(matches!(
            save_dest_file_identity(&unrelated, &real),
            SaveDestIdentity::Distinct
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A destination that does not exist yet -- the browser's `[ new ]` row -- is decisively not
    /// the loaded save, with no handle needed on that side.
    #[cfg(windows)]
    #[test]
    fn a_destination_that_does_not_exist_is_distinct_from_the_loaded_save() {
        let dir = std::env::temp_dir().join("er-save-dest-identity-new");
        let _ = fs::remove_dir_all(&dir);
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let live = dir.join("ER0000.sl2");
        if fs::write(&live, b"loaded save").is_err() {
            let _ = fs::remove_dir_all(&dir);
            return;
        }
        assert!(matches!(
            save_dest_file_identity(&dir.join("brand-new.sl2"), &live),
            SaveDestIdentity::Distinct
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Absent versus present is a real answer; two unreadable probes are not.
    #[cfg(windows)]
    #[test]
    fn absence_decides_and_unreadability_does_not() {
        let id = SaveDestFileId {
            volume: 7,
            index: 11,
        };
        assert!(matches!(
            save_dest_identity_from_probes(SaveDestProbe::Id(id), SaveDestProbe::Absent),
            SaveDestIdentity::Distinct
        ));
        assert!(matches!(
            save_dest_identity_from_probes(SaveDestProbe::Id(id), SaveDestProbe::Id(id)),
            SaveDestIdentity::SameFile
        ));
        assert!(matches!(
            save_dest_identity_from_probes(
                SaveDestProbe::Id(id),
                SaveDestProbe::Id(SaveDestFileId {
                    volume: 7,
                    index: 12
                })
            ),
            SaveDestIdentity::Distinct
        ));
        assert!(matches!(
            save_dest_identity_from_probes(SaveDestProbe::Unreadable, SaveDestProbe::Absent),
            SaveDestIdentity::Unknown
        ));
    }
}
