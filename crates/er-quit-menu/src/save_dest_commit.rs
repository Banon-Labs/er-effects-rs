// Pure save-destination commit decisions moved from the product DLL in S7.

use std::path::{Path, PathBuf};

use crate::save_dest_identity::save_dest_normalize_path;

// The BND4 container magic -- the format fact every save-destination write depends on. Kept
// beside the other container constants (not folded into the test that builds a synthetic
// container) because it describes the file format, not the test.
#[allow(dead_code)]
const SAVE_DEST_BND4_MAGIC: [u8; 4] = *b"BND4";
const SAVE_DEST_WRITE_ACCESS_MASK: u32 = 0x4000_0000 | 0x2;
const SAVE_DEST_SEAMLESS_EXTENSION: &str = "co2";
const SAVE_DEST_VANILLA_EXTENSION: &str = "sl2";

/// ASCII-lowercase leaf (file name) of a wide Windows path, or `None` when the path ends in a
/// separator / is empty.
pub fn save_dest_wide_leaf_lower(path: &[u16]) -> Option<Vec<u16>> {
    let start = path
        .iter()
        .rposition(|&c| c == b'\\' as u16 || c == b'/' as u16)
        .map_or(0, |idx| idx + 1);
    let leaf = path.get(start..)?;
    if leaf.is_empty() {
        return None;
    }
    Some(leaf.iter().copied().map(save_dest_ascii_lower).collect())
}

pub fn save_dest_ascii_lower(c: u16) -> u16 {
    if (b'A' as u16..=b'Z' as u16).contains(&c) {
        c + 0x20
    } else {
        c
    }
}

/// The live save's leaf plus its counterpart-extension twin, ASCII-lowercased UTF-16.
pub fn save_dest_accepted_leaves(live_path: &Path) -> Vec<Vec<u16>> {
    save_dest_accepted_leaf_names(live_path)
        .iter()
        .map(|name| name.encode_utf16().collect())
        .collect()
}

/// Leaf names of the live save and its counterpart twin, ASCII-lowercased UTF-8.
pub fn save_dest_accepted_leaf_names(live_path: &Path) -> Vec<String> {
    let Some(leaf) = live_path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let mut names = vec![leaf.to_ascii_lowercase()];
    if let Some((stem, extension)) = leaf.rsplit_once('.') {
        let twin = if extension.eq_ignore_ascii_case(SAVE_DEST_SEAMLESS_EXTENSION) {
            Some(SAVE_DEST_VANILLA_EXTENSION)
        } else if extension.eq_ignore_ascii_case(SAVE_DEST_VANILLA_EXTENSION) {
            Some(SAVE_DEST_SEAMLESS_EXTENSION)
        } else {
            None
        };
        if let Some(twin) = twin {
            names.push(format!("{}.{twin}", stem.to_ascii_lowercase()));
        }
    }
    names
}

/// Every directory whose `ER0000.{sl2,co2}` write-open IS the loaded save's.
pub fn save_dest_accepted_dirs_for(
    live_path: &Path,
    native_source_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(parent) = live_path.parent() {
        dirs.push(parent.to_path_buf());
    }
    if let Some(native) = native_source_dir
        && !dirs.contains(&native)
    {
        dirs.push(native);
    }
    dirs
}

/// Normalized full paths that ARE the loaded save's container: every accepted leaf in every
/// accepted directory.
pub fn save_dest_accepted_paths_for(
    live_path: &Path,
    native_source_dir: Option<PathBuf>,
) -> Vec<String> {
    let leaves = save_dest_accepted_leaf_names(live_path);
    let mut paths = Vec::new();
    for dir in save_dest_accepted_dirs_for(live_path, native_source_dir) {
        let Some(dir_text) = dir.to_str() else {
            continue;
        };
        for leaf in &leaves {
            let joined = format!("{dir_text}/{leaf}");
            if let Some(normalized) = save_dest_normalize_path(&joined)
                && !paths.contains(&normalized)
            {
                paths.push(normalized);
            }
        }
    }
    paths
}

/// The `.bak` twin the native backup step copies a saved container to.
pub fn save_dest_bak_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".bak");
    PathBuf::from(name)
}

/// End offset of the last BND4 entry, i.e. the length a structurally complete container must have.
pub fn save_dest_container_end(bytes: &[u8]) -> Option<usize> {
    let entries = er_save_loader::bnd4::parse_entries(bytes).ok()?;
    if entries.is_empty() {
        return None;
    }
    let mut end = 0_usize;
    for entry in &entries {
        end = end.max(entry.data_offset.checked_add(entry.entry_size)?);
    }
    Some(end)
}

/// True when `access` is a write open (the only opens the redirect may divert).
pub fn save_dest_is_write_access(access: u32) -> bool {
    access & SAVE_DEST_WRITE_ACCESS_MASK != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save_dest_identity::save_dest_normalized_parent;
    /// Build a minimal, structurally complete BND4 container: header, `names.len()` entry headers
    /// of `entry_len` bytes each, a UTF-16 name table, then the data blobs back to back.
    ///
    /// Deterministic generator, not captured game bytes (repo rule: no game-derived binaries in
    /// tree). It reproduces only the four header/entry fields `parse_entries` reads.
    fn synthetic_container(names: &[&str], entry_len: usize) -> Vec<u8> {
        const HEADER_LEN: usize = 0x40;
        const ENTRY_STRIDE: usize = 0x20;
        let names_at = HEADER_LEN + names.len() * ENTRY_STRIDE;
        let name_bytes: Vec<Vec<u8>> = names
            .iter()
            .map(|name| {
                let mut out: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
                out.extend_from_slice(&[0, 0]);
                out
            })
            .collect();
        let names_len: usize = name_bytes.iter().map(Vec::len).sum();
        let data_at = names_at + names_len;
        let mut out = vec![0_u8; data_at + names.len() * entry_len];
        out[..4].copy_from_slice(&SAVE_DEST_BND4_MAGIC);
        out[0x0c..0x10].copy_from_slice(&(names.len() as i32).to_le_bytes());
        out[0x10..0x18].copy_from_slice(&(HEADER_LEN as i64).to_le_bytes());
        out[0x20..0x28].copy_from_slice(&(ENTRY_STRIDE as i64).to_le_bytes());
        let mut name_cursor = names_at;
        for (index, name) in name_bytes.iter().enumerate() {
            let entry = HEADER_LEN + index * ENTRY_STRIDE;
            out[entry + 0x08..entry + 0x10].copy_from_slice(&(entry_len as i64).to_le_bytes());
            out[entry + 0x10..entry + 0x14]
                .copy_from_slice(&((data_at + index * entry_len) as i32).to_le_bytes());
            out[entry + 0x14..entry + 0x18].copy_from_slice(&(name_cursor as i32).to_le_bytes());
            out[name_cursor..name_cursor + name.len()].copy_from_slice(name);
            name_cursor += name.len();
        }
        out
    }

    #[test]
    fn a_complete_container_ends_exactly_where_its_index_says() {
        let bytes = synthetic_container(&["USER_DATA000", "USER_DATA001", "USER_DATA010"], 0x200);
        assert_eq!(save_dest_container_end(&bytes), Some(bytes.len()));
    }

    #[test]
    fn a_sparse_fragment_with_no_header_is_not_a_container() {
        let complete = synthetic_container(&["USER_DATA000", "USER_DATA010"], 0x200);
        let mut sparse = vec![0_u8; complete.len()];
        let tail = sparse.len() - 0x200;
        sparse[tail..].copy_from_slice(&complete[tail..]);
        assert_eq!(save_dest_container_end(&sparse), None);
    }

    #[test]
    fn a_truncated_container_does_not_account_for_its_own_index() {
        let mut bytes = synthetic_container(&["USER_DATA000", "USER_DATA001"], 0x200);
        let full = bytes.len();
        bytes.truncate(full - 1);
        assert_eq!(save_dest_container_end(&bytes), Some(full));
    }

    #[test]
    fn the_bak_twin_is_the_container_path_plus_bak() {
        let live = Path::new(r"C:\users\steamuser\AppData\Roaming\EldenRing\1234\ER0000.sl2");
        assert_eq!(
            save_dest_bak_path(live).to_string_lossy(),
            r"C:\users\steamuser\AppData\Roaming\EldenRing\1234\ER0000.sl2.bak"
        );
    }

    #[test]
    fn another_accounts_save_of_the_same_name_is_not_an_accepted_path() {
        let live = Path::new("/wine/users/steamuser/AppData/Roaming/EldenRing/7656/ER0000.sl2");
        let accepted = save_dest_accepted_paths_for(live, None);
        assert!(accepted.contains(
            &r"z:\wine\users\steamuser\appdata\roaming\eldenring\7656\er0000.sl2".to_owned()
        ));
        assert!(accepted.contains(
            &r"z:\wine\users\steamuser\appdata\roaming\eldenring\7656\er0000.co2".to_owned()
        ));
        assert!(!accepted.contains(
            &r"z:\wine\users\steamuser\appdata\roaming\eldenring\9999\er0000.sl2".to_owned()
        ));
    }

    #[test]
    fn accepted_paths_can_include_the_native_source_dir() {
        let live = Path::new("/tmp/staged/ER0000.sl2");
        let native = PathBuf::from("/wine/native/EldenRing/7656");
        let accepted = save_dest_accepted_paths_for(live, Some(native));
        assert!(accepted.contains(&r"z:\tmp\staged\er0000.sl2".to_owned()));
        assert!(accepted.contains(&r"z:\wine\native\eldenring\7656\er0000.sl2".to_owned()));
    }

    #[test]
    fn an_incoming_write_open_matches_only_through_normalization() {
        let live = Path::new("/wine/users/steamuser/AppData/Roaming/EldenRing/7656/ER0000.sl2");
        let accepted = save_dest_accepted_paths_for(live, None);
        let same = save_dest_normalize_path(
            "/wine/users/steamuser/AppData/Roaming/EldenRing/7656/./ER0000.SL2",
        )
        .unwrap();
        assert!(accepted.contains(&same));
        let other = save_dest_normalize_path(
            "/wine/users/steamuser/AppData/Roaming/EldenRing/9999/ER0000.SL2",
        )
        .unwrap();
        assert!(!accepted.contains(&other));
    }

    #[test]
    fn write_access_is_detected_from_generic_or_file_write_bits() {
        assert!(save_dest_is_write_access(0x4000_0000));
        assert!(save_dest_is_write_access(0x2));
        assert!(!save_dest_is_write_access(0x8000_0000));
    }

    #[test]
    fn normalized_parent_helper_is_shared_with_identity_module() {
        assert_eq!(
            save_dest_normalized_parent(r"c:\users\steamuser\er0000.sl2"),
            Some(r"c:\users\steamuser")
        );
    }
}
