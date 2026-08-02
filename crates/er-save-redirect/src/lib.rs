//! Shared save-source/redirect planning core.
//!
//! This is S6b.1: host-runnable state and source planning only. It deliberately does not install
//! Win32/NT save hooks and does not own boot/title-flow gates. Those are process-wide runtime
//! ownership questions for later slices.

mod reentry;
pub use reentry::{SaveDetourDepth, SaveNtCreateDetourGuard, save_detour_disk_io_allowed};

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

/// Exact byte length of Elden Ring PC `ER0000.sl2` / Seamless `.co2` save containers.
///
/// These files use a fixed BND4 layout: ten `USER_DATA00N` character slots, `USER_DATA010`, and
/// `USER_DATA011`. A different length is not a valid Elden Ring save container for this loader.
pub const EXPECTED_SAVE_FILE_BYTES: u64 = 0x1ba03d0;

/// Missing-save gate state shared by picker, redirect activation, and boot hold owners inside one
/// DLL image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingSaveState {
    Idle,
    Pending,
    Ready,
}

impl MissingSaveState {
    const fn as_usize(self) -> usize {
        match self {
            Self::Idle => 0,
            Self::Pending => 1,
            Self::Ready => 2,
        }
    }

    const fn from_usize(value: usize) -> Self {
        match value {
            1 => Self::Pending,
            2 => Self::Ready,
            _ => Self::Idle,
        }
    }
}

/// Atomic holder for the missing-save state machine.
pub struct MissingSaveGate {
    state: AtomicUsize,
}

impl MissingSaveGate {
    pub const fn new() -> Self {
        Self {
            state: AtomicUsize::new(MissingSaveState::Idle.as_usize()),
        }
    }

    pub fn set(&self, state: MissingSaveState) {
        self.state.store(state.as_usize(), Ordering::SeqCst);
    }

    pub fn state(&self) -> MissingSaveState {
        MissingSaveState::from_usize(self.state.load(Ordering::SeqCst))
    }

    pub fn is_pending(&self) -> bool {
        self.state() == MissingSaveState::Pending
    }
}

impl Default for MissingSaveGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Why a candidate save source was rejected before redirect planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveSourceRejection {
    MissingOrNotFile,
    WrongSize { len: u64, expected: u64 },
    NotBnd4,
    Unreadable,
}

/// UTF-16 Wine/Windows save-root path without a trailing separator or NUL terminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WineRootWide(Vec<u16>);

impl WineRootWide {
    pub fn as_slice(&self) -> &[u16] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<u16> {
        self.0
    }
}

/// Host-runnable source plan. Runtime hook installation is intentionally outside this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveSourcePlan {
    /// Source already lives under `<root>/EldenRing/<steamid>/ER0000.*` and the product is allowed
    /// to normalize/write it in place, so redirect the native save root to that staged root.
    StagedRoot {
        file: PathBuf,
        steam_id: u64,
        root_wide: WineRootWide,
    },
    /// Source is an arbitrary save file. Stage it under a private native save root; the source file
    /// remains read-only from the game's point of view.
    DirectFile {
        file: PathBuf,
        stage_root: PathBuf,
        root_wide: WineRootWide,
    },
}

impl SaveSourcePlan {
    pub fn file(&self) -> &Path {
        match self {
            Self::StagedRoot { file, .. } | Self::DirectFile { file, .. } => file,
        }
    }

    pub fn root_wide(&self) -> &WineRootWide {
        match self {
            Self::StagedRoot { root_wide, .. } | Self::DirectFile { root_wide, .. } => root_wide,
        }
    }
}

/// Validate a candidate picked/configured save. This is stronger than size-only: it also proves the
/// file is a structurally readable BND4 container.
pub fn validate_save_file_path(path: PathBuf) -> Result<PathBuf, SaveSourceRejection> {
    let meta = std::fs::metadata(&path).map_err(|_| SaveSourceRejection::MissingOrNotFile)?;
    if !meta.is_file() {
        return Err(SaveSourceRejection::MissingOrNotFile);
    }
    if meta.len() != EXPECTED_SAVE_FILE_BYTES {
        return Err(SaveSourceRejection::WrongSize {
            len: meta.len(),
            expected: EXPECTED_SAVE_FILE_BYTES,
        });
    }
    let bytes = std::fs::read(&path).map_err(|_| SaveSourceRejection::Unreadable)?;
    er_save_loader::bnd4::parse_entries(&bytes).map_err(|_| SaveSourceRejection::NotBnd4)?;
    Ok(path)
}

/// Convert a configured path root to the Wine drive form the in-process `CreateFileW` accepts.
/// Unix absolute paths become `Z:\...`; already-Windows/Wine paths like `Z:\...` or `C:\...`
/// are preserved. Backslash separators, no trailing separator. Returns UTF-16 without a NUL.
pub fn path_root_to_wine_wide(root: &Path) -> WineRootWide {
    let win: String = root
        // UTF-8 Lossy: OS path display/Win32 path bridge only; invalid host bytes are still mapped
        // into a deterministic Wine path string rather than decoded from game memory.
        .to_string_lossy()
        .chars()
        .map(|c| if c == '/' { '\\' } else { c })
        .collect();
    let has_drive_prefix = win.as_bytes().get(1).copied() == Some(b':');
    let mut out: Vec<u16> = if has_drive_prefix {
        win.encode_utf16().collect()
    } else {
        "Z:".encode_utf16().chain(win.encode_utf16()).collect()
    };
    while matches!(out.last(), Some(&c) if c == b'\\' as u16) {
        out.pop();
    }
    WineRootWide(out)
}

pub fn plausible_steam_id64(value: u64) -> Option<u64> {
    (value >= 10_000_000_000_000_000 && value <= 99_999_999_999_999_999).then_some(value)
}

/// If `path` is under `<root>/EldenRing/<steamid>/...`, return that root plus steam id.
pub fn staged_save_root_for_file(path: &Path) -> Option<(PathBuf, u64)> {
    let mut root = PathBuf::new();
    let mut comps = path.components().peekable();
    while let Some(comp) = comps.next() {
        // UTF-8 Lossy: path component classification only; invalid host bytes cannot be a literal
        // `EldenRing` directory name and should fail the staged-root shortcut deterministically.
        let text = comp.as_os_str().to_string_lossy();
        if text.eq_ignore_ascii_case("EldenRing") {
            let Some(steam_id_comp) = comps.peek() else {
                return None;
            };
            // UTF-8 Lossy: SteamID directory classification only; invalid host bytes are rejected by
            // the ASCII-digit check below.
            let steam_id = steam_id_comp.as_os_str().to_string_lossy();
            let is_steam_id = (16..=20).contains(&steam_id.len())
                && steam_id.as_bytes().iter().all(u8::is_ascii_digit);
            if is_steam_id {
                return steam_id
                    .parse::<u64>()
                    .ok()
                    .and_then(plausible_steam_id64)
                    .map(|value| (root, value));
            }
            return None;
        }
        root.push(comp);
    }
    None
}

/// Build the redirect source plan for an already validated save file.
pub fn plan_validated_save_source(path: PathBuf, writeback_allowed: bool) -> SaveSourcePlan {
    if writeback_allowed && let Some((staged_root, steam_id)) = staged_save_root_for_file(&path) {
        return SaveSourcePlan::StagedRoot {
            file: path,
            steam_id,
            root_wide: path_root_to_wine_wide(&staged_root),
        };
    }

    let stage_root = path
        .parent()
        .map(|parent| parent.join("er-effects-save-redirect-stage"))
        .unwrap_or_else(|| PathBuf::from("er-effects-save-redirect-stage"));
    SaveSourcePlan::DirectFile {
        file: path.clone(),
        root_wide: path_root_to_wine_wide(&stage_root),
        stage_root,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER_LEN: usize = 0x40;
    const ENTRY_STRIDE: usize = 0x20;
    const MD5_LEN: usize = 0x10;

    fn synthetic_bnd4_container() -> Vec<u8> {
        let body = vec![0_u8; 0x20];
        let name = "USER_DATA010";
        let mut name_blob: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        name_blob.extend_from_slice(&[0, 0]);
        let names_at = HEADER_LEN + ENTRY_STRIDE;
        let data_at = names_at + name_blob.len();
        let total = EXPECTED_SAVE_FILE_BYTES as usize;
        let mut out = vec![0_u8; total];
        out[..4].copy_from_slice(b"BND4");
        out[0x0c..0x10].copy_from_slice(&1_i32.to_le_bytes());
        out[0x10..0x18].copy_from_slice(&(HEADER_LEN as i64).to_le_bytes());
        out[0x20..0x28].copy_from_slice(&(ENTRY_STRIDE as i64).to_le_bytes());
        out[HEADER_LEN + 0x08..HEADER_LEN + 0x10]
            .copy_from_slice(&((MD5_LEN + body.len()) as i64).to_le_bytes());
        out[HEADER_LEN + 0x10..HEADER_LEN + 0x14].copy_from_slice(&(data_at as i32).to_le_bytes());
        out[HEADER_LEN + 0x14..HEADER_LEN + 0x18].copy_from_slice(&(names_at as i32).to_le_bytes());
        out[names_at..names_at + name_blob.len()].copy_from_slice(&name_blob);
        out[data_at + MD5_LEN..data_at + MD5_LEN + body.len()].copy_from_slice(&body);
        out
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("er-save-redirect-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir must be creatable");
        dir
    }

    #[test]
    fn missing_save_gate_moves_only_through_explicit_states() {
        let gate = MissingSaveGate::new();
        assert_eq!(gate.state(), MissingSaveState::Idle);
        assert!(!gate.is_pending());
        gate.set(MissingSaveState::Pending);
        assert!(gate.is_pending());
        gate.set(MissingSaveState::Ready);
        assert_eq!(gate.state(), MissingSaveState::Ready);
    }

    #[test]
    fn validation_rejects_wrong_size_or_non_bnd4_files() {
        let dir = scratch_dir("rejects");
        let tiny = dir.join("ER0000.sl2");
        std::fs::write(&tiny, b"BND4").unwrap();
        assert_eq!(
            validate_save_file_path(tiny),
            Err(SaveSourceRejection::WrongSize {
                len: 4,
                expected: EXPECTED_SAVE_FILE_BYTES,
            })
        );

        let garbage = dir.join("large.sl2");
        std::fs::write(&garbage, vec![0_u8; EXPECTED_SAVE_FILE_BYTES as usize]).unwrap();
        assert_eq!(
            validate_save_file_path(garbage),
            Err(SaveSourceRejection::NotBnd4)
        );
    }

    #[test]
    fn validation_accepts_a_structural_bnd4_container() {
        let dir = scratch_dir("accepts");
        let save = dir.join("ER0000.sl2");
        std::fs::write(&save, synthetic_bnd4_container()).unwrap();
        assert_eq!(validate_save_file_path(save.clone()), Ok(save));
    }

    #[test]
    fn staged_root_plan_uses_the_ancestor_before_eldenring() {
        let path = PathBuf::from("Z:/prefix/EldenRing/76561198000000000/ER0000.sl2");
        let plan = plan_validated_save_source(path.clone(), true);
        assert_eq!(
            plan,
            SaveSourcePlan::StagedRoot {
                file: path,
                steam_id: 76_561_198_000_000_000,
                root_wide: WineRootWide("Z:\\prefix".encode_utf16().collect()),
            }
        );
    }

    #[test]
    fn arbitrary_save_files_plan_a_private_stage_root() {
        let path = PathBuf::from("/tmp/picked/ER0000.sl2");
        let plan = plan_validated_save_source(path.clone(), true);
        assert_eq!(
            plan,
            SaveSourcePlan::DirectFile {
                file: path,
                stage_root: PathBuf::from("/tmp/picked/er-effects-save-redirect-stage"),
                root_wide: WineRootWide(
                    "Z:\\tmp\\picked\\er-effects-save-redirect-stage"
                        .encode_utf16()
                        .collect(),
                ),
            }
        );
    }
}
