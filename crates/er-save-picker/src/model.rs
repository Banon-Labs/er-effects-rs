//! Shared in-game save-file picker model.
//!
//! Pure filesystem/scroll-window state for the two in-game file-picker menus (the startup
//! missing-save picker and the System>Quit "Load Character from File" picker). Both menus render
//! through the native `05_010_ProfileSelect` 10-row window, so this model maps a browsable
//! directory listing onto a sliding native row window. The UI layers own all native staging (ProfileSummary preview
//! records, window submit/close); this module owns what the rows MEAN.
//!
//! Extension filtering follows the active runtime flavor: vanilla offers `.sl2`; Seamless offers
//! both `.co2` and vanilla `.sl2` sources so users can import/load a vanilla save while ERSC owns
//! the session.
//!
//! The same model serves two INTENTS (save-game-flow WP3). [`PickerIntent::LoadSource`] browses for
//! a save to LOAD (the shipping behavior). [`PickerIntent::SaveDestination`] browses for a folder to
//! SAVE INTO: a pinned `[ new ]` row writes the loaded save's own filename into the browsed folder,
//! and occupancy filtering is dropped -- an overwrite target needs no active character slot, and
//! hiding a slotless existing file would let `[ new ]` clobber it silently.
//!
//! ## The row layout is DENSE, and every index is derived
//!
//! The visible rows are a contiguous prefix of the window's 10 slots, in this order:
//!
//! 1. `DRIVES [C:]`   -- ALWAYS FIRST when more than one drive is mounted;
//! 2. `[ new ]`       -- destination intent only, directly below the drive row when it exists;
//! 3. `[..] <parent>` -- only when the current directory has a parent (absent at a drive root);
//! 4. the current scroll window's directory / save-file entries.
//!
//! Overflow is represented by the native `05_010` scrollbar affordance plus edge-hover restaging,
//! not by consuming two row slots with `[ SCROLL ^ ]` / `[ SCROLL v ]` pseudo-entries.
//!
//! `[ new ]` SITS ABOVE THE PARENT ROW, which is the one place the two intents' layouts differ,
//! and it is deliberate. Since the Save Game row press opens this browser with no question in front
//! of it (2026-07-31), [`SavePickerModel::first_selectable_row`] explicitly starts destination
//! browsing on `[ new ]` even when the always-first drive row occupies row 0. The safe default is
//! therefore preserved without making the location switcher jump below the current folder.
//!
//! Only the drive row has a fixed index (0 when it exists). [`SavePickerModel::entry_row_base`] is
//! the single place the fixed-row count is decided, and every entry query derives from it. That
//! matters for two reasons.
//!
//! First, ROW ALIGNMENT. A row's label and its per-row character text must never describe
//! different entries; a hard-coded `row - 1` was only correct in load-source intent and made every
//! destination row render the character info of the file one entry further down. Both now resolve
//! through [`SavePickerModel::row_meaning`], and [`SavePickerModel::row_file_characters`] proves
//! the entry it read is the same file the label named before returning it.
//!
//! Second, BLANK ROWS. The native list builder (`FUN_140875590`, 1.16.2) appends a row only for
//! slots whose `ProfileSummary::saveSlotsStates[slot]` byte is set, and it appends them in slot
//! order -- so occupying a contiguous PREFIX keeps `slot index == visible list index == model row`
//! (the row-populate hook reads the slot back from `rowModel+0x8`, which is that slot index). Rows
//! at or beyond [`SavePickerModel::visible_row_count`] are staged UNOCCUPIED, so the builder omits
//! them entirely: a short listing shows nothing at all below the last entry instead of placeholder
//! rows rendering a name, `Level 0` and `0:00:00`.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};

use crate::host::append_autoload_debug;

/// Rows per `05_010_ProfileSelect` window (native slot count).
pub const PICKER_ROW_COUNT: usize = 10;
/// ProfileSummary name field capacity: 16 UTF-16 units + NUL (0x22 bytes).
pub const PICKER_ROW_NAME_UTF16_MAX: usize = 16;
/// One independent cell for every Windows drive letter. Normal machines use only a populated
/// prefix; the defensive paging code remains for synthetic/non-Windows roots beyond this capacity.
/// Simultaneously visible drive controls. The asset still owns all 26 possible letter fields; a
/// seven-cell window reserves the drive row's right side for the complete-path editor and pages
/// through additional mounted drives with the existing `[<]`/`[>]` cells.
pub const DRIVE_STRIP_MAX_CELLS: usize = 7;
/// Label of the destination-intent `[ new ]` row (7 UTF-16 units, inside the name budget).
pub const PICKER_NEW_FILE_LABEL: &str = "[ new ]";
/// Marker prefixed to the stats line of the row that IS the save currently loaded.
///
/// It goes on the row's `ErStats` TOP line, not in the row NAME, and that is a capacity fact
/// rather than a preference: the name field holds 16 UTF-16 units, `ER0000.sl2` already spends 10,
/// and a 9-unit marker would push the filename out of its own row. The stats line is 630px wide at
/// the 19px `MenuFont_01` the browse rows render in; `scripts/gfx_text_width.py` measures
/// `[CURRENT] 10 CHARACTERS` at 247.1px there, against 143.4px for the bare count. It is a
/// single-line no-wordwrap field, so an overflow would clip -- this one does not come close.
pub const PICKER_CURRENT_SAVE_MARKER: &str = "[CURRENT]";

/// What the browsing session is FOR. Fixed at construction; it selects the row layout, the
/// occupancy filter, and what activating a row means.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PickerIntent {
    /// Browse for a save container to LOAD (startup missing-save picker, System>Quit load picker).
    #[default]
    LoadSource,
    /// Browse for a folder to SAVE INTO. `loaded_file_name` is the leaf the `[ new ]` row writes
    /// (always the loaded save's own filename, so the destination keeps its save flavor);
    /// `loaded_path` is the save currently loaded, used ONLY to mark its row `[CURRENT]`.
    SaveDestination {
        loaded_file_name: String,
        loaded_path: PathBuf,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerEntry {
    /// A subdirectory of the current directory.
    Dir { name: String, path: PathBuf },
    /// A save container matching the active extension filter(s).
    File {
        name: String,
        path: PathBuf,
        modified: Option<SystemTime>,
        /// The container's active loadable characters (slot/name/level), parsed ONCE at
        /// listing-build time from the same bytes the active-slot filter reads. Never empty --
        /// files with no loadable character are hidden from the listing.
        chars: Vec<crate::slots::SaveSlotInfo>,
    },
}

impl PickerEntry {
    pub fn name(&self) -> &str {
        match self {
            PickerEntry::Dir { name, .. } | PickerEntry::File { name, .. } => name,
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            PickerEntry::Dir { path, .. } | PickerEntry::File { path, .. } => path,
        }
    }
}

/// What a row in the CURRENT native scroll window means. Produced by [`SavePickerModel::row_meaning`]; the UI
/// layer stages row text from this and routes slot activation through it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerRow {
    /// Navigate to the parent directory.
    ParentDir,
    /// Switch to the next mounted drive, resuming the folder last browsed there.
    DriveCycle,
    /// Degenerate placeholder: this listing has no parent, no other drive, no entries and no
    /// `[ new ]` row, so row 0 names the dead end rather than leaving the native window with zero
    /// rows. Activation is a no-op.
    AtRoot,
    /// Open this subdirectory.
    Dir(PathBuf),
    /// Pick this save file.
    File(PathBuf),
    /// Destination intent only: save into the browsed folder under the loaded save's own filename.
    NewFile(PathBuf),
    /// Scroll the native row window toward earlier directory entries.
    ScrollUp,
    /// Scroll the native row window toward later directory entries.
    ScrollDown,
    /// Deprecated compatibility variant: pagination was removed in favor of a scroll window.
    NextPage,
    /// Row beyond the visible rows; it is staged UNOCCUPIED so the native builder omits it, and
    /// activation is a no-op.
    Empty,
}

/// Whether a row of this kind has a LAST-SAVED time to show where the native row shows a playtime.
///
/// Only a [`PickerRow::File`] row is backed by a file on disk, so only it has a modification time.
/// Everything else is navigation or intent. (The native `Level` caption and value are a different
/// story: NO browse row is a profile slot, so a level is meaningless on every one of them and they
/// are hidden across the board -- there is nothing row-kind-dependent left to decide.) The match is
/// exhaustive on purpose: a new row kind must state which side it is on.
pub fn picker_row_has_last_saved_time(row: &PickerRow) -> bool {
    match row {
        PickerRow::File(_) => true,
        PickerRow::ParentDir
        | PickerRow::DriveCycle
        | PickerRow::AtRoot
        | PickerRow::Dir(_)
        | PickerRow::NewFile(_)
        | PickerRow::ScrollUp
        | PickerRow::ScrollDown
        | PickerRow::NextPage
        | PickerRow::Empty => false,
    }
}

/// Civil date-time fields, already in whatever zone the caller shifted into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CivilDateTime {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

/// Seconds in a day.
const SECONDS_PER_DAY: i64 = 86_400;

/// Split `secs` (seconds since the Unix epoch) into civil date-time fields.
///
/// PURE, and the zone shift is the caller's business: add the UTC offset first and this same
/// function yields local time, which is what makes the local rendering testable without a machine
/// timezone. Days-to-civil is Hinnant's era-based algorithm (proleptic Gregorian, exact well past
/// any timestamp a filesystem can hold). `None` before the epoch -- a save file dated before 1970 is
/// a broken clock, not a date worth rendering.
pub fn civil_from_unix_seconds(secs: i64) -> Option<CivilDateTime> {
    if secs < 0 {
        return None;
    }
    let days = secs.div_euclid(SECONDS_PER_DAY);
    let secs_of_day = secs.rem_euclid(SECONDS_PER_DAY);
    // Shift the epoch to 0000-03-01 so leap days land at the end of the 400-year era.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // day of era, [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of (March-based) year, [0, 365]
    let mp = (5 * doy + 2) / 153; // March-based month, [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = yoe + era * 400 + i64::from(month <= 2);
    Some(CivilDateTime {
        year,
        month,
        day,
        hour: (secs_of_day / 3_600) as u32,
        minute: (secs_of_day % 3_600 / 60) as u32,
    })
}

/// The text a save-file row shows where the native row shows a playtime: when that file was last
/// written, `YYYY-MM-DD HH:MM`, in the local zone `utc_offset_seconds` describes.
///
/// PURE -- the OS supplies only the offset -- so the rendering is testable across a DST boundary by
/// passing the two offsets that boundary switches between.
///
/// NO "Last saved: " PREFIX, and that is measured rather than assumed: in the 05_010 row template
/// the `PlayTime` field is 200px wide (bounds -40..3960 twips) at a 24px `MenuFont_01`, and
/// `scripts/gfx_text_width.py` sums that font's own advance table to 268.0px for
/// `Last saved: 2026-07-29 08:03` against 163.1px for the bare timestamp. The field is
/// wordwrap+multiline inside 40px, so an overflow does not truncate -- it wraps to a second line the
/// box clips, which would hide the date and leave only the prefix.
pub fn format_last_saved(secs: i64, utc_offset_seconds: i64) -> Option<String> {
    let local = civil_from_unix_seconds(secs.checked_add(utc_offset_seconds)?)?;
    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        local.year, local.month, local.day, local.hour, local.minute
    ))
}

/// Outcome of activating a row. `Repopulate` means the listing changed (new directory, new drive or
/// new scroll window) and the UI must re-stage row records and re-present the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveStripFocus {
    Cell(usize),
    CurrentPath,
}

/// What an UP/DOWN press taken at a window edge should do to the native list, from
/// [`SavePickerModel::scroll_window_from_edge_press`]. Both variants carry the row the caller must
/// write into the native list cursor, because the list has already moved its own cursor by the time
/// the menu pump sees the press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgePressOutcome {
    /// The window advanced one row; hold the selection on the edge row it scrolled from so the next
    /// press is still an edge press.
    Scrolled { pin_row: usize },
    /// The listing has no more rows that way. Hold the selection where it was: the native list
    /// wraps to the opposite end here, and a selection that teleports from the last row to the
    /// drives row reads as the list losing the player's place.
    HeldAtLimit { pin_row: usize },
}

impl EdgePressOutcome {
    /// Row to write into the native list cursor.
    pub fn pin_row(self) -> usize {
        match self {
            Self::Scrolled { pin_row } | Self::HeldAtLimit { pin_row } => pin_row,
        }
    }

    /// True when the listing window moved and the row records must be re-staged.
    pub fn scrolled(self) -> bool {
        matches!(self, Self::Scrolled { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerActivation {
    PickedFile(PathBuf),
    /// Destination intent only: the `[ new ]` row resolved to this target path in the browsed
    /// directory. The UI layer decides whether it already exists (overwrite confirm) or not.
    PickedNewFile(PathBuf),
    Repopulate,
    Ignored,
}

#[derive(Debug, Default)]
pub struct SavePickerModel {
    current_dir: PathBuf,
    /// Display label for the extension filter(s), e.g. `sl2` or `co2/sl2`; locked at open time.
    extension: String,
    /// Extension filters (no dot), lower-cased; locked at open time.
    extensions: Vec<String>,
    /// Dirs first (name order), then files (most recently modified first).
    entries: Vec<PickerEntry>,
    scroll_offset: usize,
    /// Highlighted row index (0..PICKER_ROW_COUNT) for the overlay picker. Clamped to a
    /// selectable (non-Empty) row on every listing change.
    cursor: usize,
    /// Leftmost real drive index visible in the one-row drive strip.
    drive_strip_offset: usize,
    /// The complete-path editor is the focus target immediately to the right of the final drive.
    drive_strip_path_focused: bool,
    /// Last rejection/status line the picker should render on the current surface. Cleared by
    /// navigation or a fresh pick attempt so a stale error never follows the user into another
    /// folder/scroll window.
    status_message: Option<PickerStatusMessage>,
    /// Text the user typed into the CurrentPath editor that failed validation, kept verbatim so
    /// the control can show it back marked invalid instead of silently reverting to the old
    /// folder and losing what was typed. `None` means the control renders the real
    /// `current_dir`. Cleared by `refresh` and `clear_status_message`, i.e. the moment the
    /// listing or status reflects reality again -- so a corrected entry drops the marking
    /// without any caller having to remember to.
    rejected_path_text: Option<String>,
    /// Mounted drives that browse as folders (cached at open). Two or more of them add the drive
    /// cycler row; the overlay picker also cycles them with left/right.
    drives: Vec<PathBuf>,
    /// Where the browser was standing on each drive, keyed by that drive's root. Written when a
    /// drive is cycled AWAY from and read when it is cycled back to, so switching drives resumes
    /// the folder you were in instead of dumping you at the drive root every time.
    last_dir_per_drive: HashMap<PathBuf, PathBuf>,
    /// What this browsing session is for; locked at open time.
    intent: PickerIntent,
}

/// Mounted drives that browse as folders: probe `A:\`..`Z:\` and keep the ones that are real
/// directories. Under Wine this yields e.g. `Z:\` (Linux `/`), `C:\` (wineprefix), `S:\` (Steam),
/// and skips raw block-device drives (`D:`/`E:`/`F:` -> `/dev/sd*`) that are not directories.
fn enumerate_drives() -> Vec<PathBuf> {
    (b'A'..=b'Z')
        .filter_map(|c| {
            let root = PathBuf::from(format!("{}:\\", c as char));
            root.is_dir().then_some(root)
        })
        .collect()
}

fn windows_like_path_text(path: &Path) -> Option<&str> {
    let text = path.to_str()?;
    (text.len() >= 3
        && text.as_bytes().get(1) == Some(&b':')
        && matches!(text.as_bytes().get(2), Some(b'\\') | Some(b'/')))
    .then_some(text)
}

fn complete_directory_text_is_absolute(text: &str, candidate: &Path) -> bool {
    candidate.is_absolute() || windows_like_path_text(candidate).is_some() || text.starts_with('/')
}

fn entered_directory_candidate(text: &str) -> PathBuf {
    #[cfg(windows)]
    {
        // The picker exposes the Wine Z: filesystem. Accept the Linux-form absolute paths users
        // naturally paste/type in WSL and translate only the root/separators; case and spaces remain
        // byte-for-byte unchanged. Drive-prefixed and UNC paths pass through untouched.
        if text.starts_with('/') {
            return PathBuf::from(format!("Z:{}", text.replace('/', "\\")));
        }
    }
    PathBuf::from(text)
}

fn path_parent(path: &Path) -> Option<PathBuf> {
    let Some(text) = windows_like_path_text(path) else {
        return path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf);
    };
    let trimmed = text.trim_end_matches(['\\', '/']);
    if trimmed.len() <= 2 {
        return None;
    }
    let index = trimmed.rfind(['\\', '/'])?;
    if index <= 2 {
        Some(PathBuf::from(&text[..3]))
    } else {
        Some(PathBuf::from(&trimmed[..index]))
    }
}

fn path_file_name_text(path: &Path) -> Option<&str> {
    let Some(text) = windows_like_path_text(path) else {
        return path.file_name().and_then(|name| name.to_str());
    };
    let trimmed = text.trim_end_matches(['\\', '/']);
    if trimmed.len() <= 2 {
        return None;
    }
    let index = trimmed.rfind(['\\', '/'])?;
    trimmed.get(index + 1..).filter(|name| !name.is_empty())
}

fn path_text_eq_case_insensitive(left: &Path, right: &Path) -> bool {
    match (left.to_str(), right.to_str()) {
        (Some(left), Some(right)) => left
            .replace('/', "\\")
            .eq_ignore_ascii_case(&right.replace('/', "\\")),
        _ => false,
    }
}

/// Why a candidate path is not something this picker would offer for a given intent.
///
/// Discriminants are explicit and start at 1 so `as usize` can be exported as a telemetry
/// oracle where 0 means "no rejection recorded".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickRejection {
    /// A directory, or (load intent) a path that is not a file at all.
    NotAFile = 1,
    /// Extension outside the active runtime flavor's filter.
    WrongExtension = 2,
    /// The bytes could not be read.
    Unreadable = 3,
    /// Read, but not a BND4 save container.
    NotBnd4 = 4,
    /// A BND4 container with no slot the autoload would accept as a real character.
    NoLoadableCharacter = 5,
    /// The path did not round-trip through UTF-8, so nothing downstream can name it.
    PathNotUtf8 = 6,
    /// (Destination intent) the folder the file would live in does not exist.
    ParentMissing = 7,
}

/// Why an accepted native text-entry value cannot become the picker's current directory.
/// Validation completes before any model field changes, so every error preserves the old listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectoryChangeError {
    Empty,
    NotAbsolute,
    NotDirectory,
}

impl DirectoryChangeError {
    pub fn status_message(self) -> PickerStatusMessage {
        match self {
            Self::Empty => PickerStatusMessage::new(
                "PATH IS EMPTY",
                "Enter an absolute folder path, or press Back to cancel.",
            ),
            Self::NotAbsolute => PickerStatusMessage::new(
                "ABSOLUTE PATH REQUIRED",
                "Enter a complete path beginning with a drive letter.",
            ),
            Self::NotDirectory => PickerStatusMessage::new(
                "FOLDER NOT FOUND",
                "The path does not name an existing directory.",
            ),
        }
    }
}

/// User-facing picker status text. Product telemetry/log wording stays at the caller; the picker
/// surface only needs a concise headline plus one explanatory line it can render inline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerStatusMessage {
    headline: String,
    detail: String,
}

impl PickerStatusMessage {
    pub fn new(headline: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            headline: headline.into(),
            detail: detail.into(),
        }
    }

    pub fn headline(&self) -> &str {
        &self.headline
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl PickRejection {
    /// Convert a structural pick rejection into visible picker copy. The exact log/telemetry fields
    /// remain owned by each product path; this copy is intentionally screen-facing and reason-first.
    pub fn status_message(self, extension_label: &str) -> PickerStatusMessage {
        match self {
            PickRejection::NotAFile => PickerStatusMessage::new(
                "SAVE NOT FOUND",
                "The selected path is missing or is not a file.",
            ),
            PickRejection::WrongExtension => PickerStatusMessage::new(
                "WRONG FILE TYPE",
                format!("Choose an Elden Ring save ending in .{}.", extension_label),
            ),
            PickRejection::Unreadable => PickerStatusMessage::new(
                "SAVE UNREADABLE",
                "The save exists, but could not be read.",
            ),
            PickRejection::NotBnd4 => PickerStatusMessage::new(
                "NOT AN ELDEN RING SAVE",
                "The file is not a readable BND4 save container.",
            ),
            PickRejection::NoLoadableCharacter => PickerStatusMessage::new(
                "NO LOADABLE CHARACTER",
                "The save has no character slot this loader can use.",
            ),
            PickRejection::PathNotUtf8 => PickerStatusMessage::new(
                "PATH NOT SUPPORTED",
                "This path cannot be named safely by the save picker.",
            ),
            PickRejection::ParentMissing => PickerStatusMessage::new(
                "FOLDER NOT FOUND",
                "The destination folder does not exist.",
            ),
        }
    }
}

/// True when `path`'s extension is one the active runtime flavor accepts. THE extension filter --
/// the in-game listing, the OS dialog's post-return check and the ingest pipeline all call this,
/// so a cross-flavor container cannot be accepted by one surface and refused by another.
pub fn save_picker_extension_accepted(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            extensions
                .iter()
                .any(|allowed| ext.eq_ignore_ascii_case(allowed))
        })
}

/// THE picker's notion of "this path is offerable", parameterised by intent. There is deliberately
/// only one: the in-game listing predicate and the OS dialog's post-return check are this same
/// function, so a container one surface hides cannot be a container the other loads.
///
/// On success the container's active characters come back, so the caller pays ONE read.
///
/// **`LoadSource`** -- file, extension, BND4, and at least one LOADABLE character slot
/// (`USER_DATA010.active_slot` occupancy + PlayerGameData locate + `level >= 1`, the same
/// `parse_save_character_slots` pass the character sub-picker runs on pick). Containers with no
/// loadable character -- no active slot, or only empty-like leftovers the autoload's real-character
/// fingerprint would reject anyway -- are rejected, which for the in-game listing means "not
/// listed" and for the OS dialog means "reopen".
///
/// **`SaveDestination`** -- extension plus an existing parent directory, and NOT the slot parse.
/// Three reasons, each load-bearing: `[ new ]` and Save-As both name a file that does not exist
/// yet; an overwrite target needs no active character slot; and hiding a slotless or unreadable
/// existing file would let `[ new ]` silently clobber it. A destination whose bytes do parse still
/// returns its characters so a row can show who lives in the file. This asymmetry is what keeps
/// "keep a bogus container out of the LOAD path" from also making saving to a new file impossible.
pub fn save_picker_accepts(
    path: &Path,
    intent: &PickerIntent,
    extensions: &[&str],
) -> Result<Vec<crate::slots::SaveSlotInfo>, PickRejection> {
    if path.to_str().is_none() {
        return Err(PickRejection::PathNotUtf8);
    }
    if path.is_dir() {
        return Err(PickRejection::NotAFile);
    }
    if !save_picker_extension_accepted(path, extensions) {
        return Err(PickRejection::WrongExtension);
    }
    if matches!(intent, PickerIntent::SaveDestination { .. }) {
        if !path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty() && parent.is_dir())
        {
            return Err(PickRejection::ParentMissing);
        }
        // Best effort only: an unreadable or unparseable destination is still a legal destination.
        return Ok(std::fs::read(path)
            .map(|bytes| crate::slots::parse_save_character_slots(&bytes))
            .unwrap_or_default());
    }
    if !path.is_file() {
        return Err(PickRejection::NotAFile);
    }
    let Ok(bytes) = std::fs::read(path) else {
        return Err(PickRejection::Unreadable);
    };
    if er_save_loader::bnd4::parse_entries(&bytes).is_err() {
        return Err(PickRejection::NotBnd4);
    }
    let chars = crate::slots::parse_save_character_slots(&bytes);
    if chars.is_empty() {
        return Err(PickRejection::NoLoadableCharacter);
    }
    Ok(chars)
}

impl SavePickerModel {
    /// Build a model rooted at `dir`, listing subdirectories plus `*.{extension}` files.
    pub fn open(dir: &Path, extension: &str) -> Self {
        Self::open_with_extensions(dir, &[extension])
    }

    /// Build a model rooted at `dir`, listing subdirectories plus files whose extension matches any
    /// entry in `extensions`.
    pub fn open_with_extensions(dir: &Path, extensions: &[&str]) -> Self {
        Self::open_with_intent(dir, extensions, PickerIntent::LoadSource)
    }

    /// Build a save-DESTINATION browser rooted at `dir` (save-game-flow WP3). `loaded_file_name` is
    /// the leaf the `[ new ]` row writes into the browsed folder; `loaded_path` is the save
    /// currently loaded, so its row can be marked [`PICKER_CURRENT_SAVE_MARKER`].
    pub fn open_destination(
        dir: &Path,
        extensions: &[&str],
        loaded_file_name: &str,
        loaded_path: &Path,
    ) -> Self {
        Self::open_with_intent(
            dir,
            extensions,
            PickerIntent::SaveDestination {
                loaded_file_name: loaded_file_name.to_owned(),
                loaded_path: loaded_path.to_path_buf(),
            },
        )
    }

    fn open_with_intent(dir: &Path, extensions: &[&str], intent: PickerIntent) -> Self {
        let mut filters: Vec<String> = extensions
            .iter()
            .map(|ext| ext.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|ext| !ext.is_empty())
            .collect();
        filters.sort();
        filters.dedup();
        if filters.is_empty() {
            filters.push("sl2".to_owned());
        }
        let mut model = SavePickerModel {
            current_dir: dir.to_path_buf(),
            extension: filters.join("/"),
            extensions: filters,
            entries: Vec::new(),
            scroll_offset: 0,
            cursor: 0,
            drive_strip_offset: 0,
            status_message: None,
            rejected_path_text: None,
            drives: enumerate_drives(),
            last_dir_per_drive: HashMap::new(),
            intent,
            drive_strip_path_focused: false,
        };
        model.refresh();
        model.cursor = model.first_selectable_row();
        model
    }

    /// True when this browser is choosing a save DESTINATION rather than a load source.
    pub fn is_destination(&self) -> bool {
        matches!(self.intent, PickerIntent::SaveDestination { .. })
    }

    // ---------------------------------------------------------------------------------------
    // ROW LAYOUT. `entry_row_base` is the single decision point; every other index derives from
    // it, so a layout change cannot desynchronize labels, character text and activation routing.
    // ---------------------------------------------------------------------------------------

    /// True when the current directory has a parent to navigate to (false at a drive root).
    fn has_parent_row(&self) -> bool {
        path_parent(&self.current_dir).is_some()
    }

    /// True when the stable location row can name a mounted drive. With one drive it still owns the
    /// complete-path display/editor; with multiple drives it additionally exposes the drive strip.
    fn has_drive_row(&self) -> bool {
        !self.drives.is_empty()
    }

    /// Rows above the entries that are pure NAVIGATION -- never an entry, never a pick target:
    /// the always-first drive row and the later parent row. The initial cursor skips these when a
    /// real entry exists so a fresh listing lands on something actionable.
    fn nav_row_count(&self) -> usize {
        usize::from(self.has_parent_row()) + usize::from(self.has_drive_row())
    }

    /// Rows the destination-only `[ new ]` control occupies above the parent row and entries: 1 in
    /// destination intent, 0 in a load browse. The always-first drive row is the sole exception.
    fn pinned_row_count(&self) -> usize {
        usize::from(self.is_destination())
    }

    /// Row index of the first directory/file entry.
    /// First row of the listing proper, i.e. the row below any header rows the view puts above the
    /// entries. Public because the menu pump needs the same "first content row" the edge-press rule
    /// uses when it has to clamp a cursor step of its own.
    pub fn entry_row_base(&self) -> usize {
        self.pinned_row_count() + self.nav_row_count()
    }

    /// Row index of the pinned `[ new ]` row (destination intent only). It sits directly below the
    /// always-first drive row when one exists, otherwise it remains row 0.
    pub fn new_file_row(&self) -> Option<usize> {
        self.is_destination()
            .then(|| usize::from(self.has_drive_row()))
    }

    /// Row index of the "up one directory" row, when the current directory has a parent. The drive
    /// row and destination-only `[ new ]` row, when present, both sit above it.
    pub fn parent_row(&self) -> Option<usize> {
        self.has_parent_row()
            .then(|| usize::from(self.has_drive_row()) + self.pinned_row_count())
    }

    /// Row index of the drive cycler, when it exists. The drive switcher is the stable top-level
    /// location control, so it always owns row 0 in both picker intents and every subdirectory.
    pub fn drive_row(&self) -> Option<usize> {
        self.has_drive_row().then_some(0)
    }

    /// Row index of the old page cycler. Pagination is removed; overflow is represented by the
    /// movie's native scrollbar affordance plus native-window restaging instead.
    pub fn next_page_row(&self) -> Option<usize> {
        None
    }

    /// Compatibility accessor for the old visible scroll-up pseudo-row. Scroll controls no longer
    /// consume row slots; held native cursor edges restage the entry window instead.
    pub fn scroll_up_row(&self) -> Option<usize> {
        None
    }

    /// Compatibility accessor for the old visible scroll-down pseudo-row. Scroll controls no longer
    /// consume row slots; held native cursor edges restage the entry window instead.
    pub fn scroll_down_row(&self) -> Option<usize> {
        None
    }

    fn entry_window_row_base(&self) -> usize {
        self.entry_row_base()
    }

    /// Rows the window actually shows. Slots at or beyond this are staged UNOCCUPIED so the native
    /// list builder omits them (no name, no level, no playtime).
    pub fn visible_row_count(&self) -> usize {
        let rows = self.entry_window_row_base() + self.window_entries().len();
        // Never zero: an empty single-drive root has nothing above and nothing to list, and a
        // zero-row native list would leave the window with no selectable item at all. Row 0
        // becomes the `[ root ]` dead-end marker instead.
        rows.max(1)
    }

    /// Directory/save entries that fit in the native ten-row transport after fixed rows. Overflow
    /// does not consume row slots; the compact movie's ScrollBarV and edge-hover restaging own that
    /// affordance.
    fn entry_window_capacity(&self) -> usize {
        PICKER_ROW_COUNT
            .saturating_sub(self.entry_row_base())
            .max(1)
    }

    fn max_scroll_offset(&self) -> usize {
        self.entries
            .len()
            .saturating_sub(self.entry_window_capacity())
    }

    /// Compatibility name for callers/tests that care how many entries fit in one native window.
    pub fn entries_per_page(&self) -> usize {
        self.entry_window_capacity()
    }

    /// Destination target for the `[ new ]` row: the loaded save's own filename in the browsed
    /// directory. `None` outside destination intent.
    ///
    /// "New" NAMES THE INTENT, NOT A GUARANTEE. In the folder the destination browser opens in,
    /// this leaf IS the loaded save, so activating the row there resolves to an existing file and
    /// takes the overwrite confirm like any other pick (`save_dest_route_picked_target`). Browse
    /// anywhere else and the same row is a genuinely new file. That is why the row cannot be given
    /// a "skip the confirm" shortcut: what it means depends entirely on where you are standing.
    fn new_file_target(&self) -> Option<PathBuf> {
        match &self.intent {
            PickerIntent::SaveDestination {
                loaded_file_name, ..
            } => Some(self.current_dir.join(loaded_file_name)),
            PickerIntent::LoadSource => None,
        }
    }

    /// The save currently loaded, when this browser is a destination chooser. Display use only --
    /// see `SaveDestOrigin::loaded_path`.
    fn loaded_save_path(&self) -> Option<&Path> {
        match &self.intent {
            PickerIntent::SaveDestination { loaded_path, .. } => Some(loaded_path.as_path()),
            PickerIntent::LoadSource => None,
        }
    }

    /// True when `row` is the save file that is currently loaded -- the row the browse list marks
    /// [`PICKER_CURRENT_SAVE_MARKER`].
    ///
    /// A CASE-INSENSITIVE PATH COMPARE, AND ONLY THAT. Windows paths are case-insensitive, so
    /// `ER0000.sl2` and `er0000.SL2` are one file and a case-sensitive compare would leave the
    /// user's own save unmarked. It can still MISS -- a different mount, a link, a `..` segment --
    /// and missing is harmless here: an unmarked row is a row the user reads the filename of. It
    /// must never be promoted into a decision, because the decision "this destination IS the
    /// loaded save" is made at commit time from volume serial + file index, which is exact.
    pub fn row_is_loaded_save(&self, row: usize) -> bool {
        let (PickerRow::File(path), Some(loaded)) =
            (self.row_meaning(row), self.loaded_save_path())
        else {
            return false;
        };
        // A path that does not round-trip through UTF-8 simply does not match: the listing already
        // refuses such paths (`PickRejection::PathNotUtf8`), so this is unreachable rather than
        // lenient, and answering "not the loaded save" is the harmless direction anyway.
        path_text_eq_case_insensitive(&path, loaded)
    }

    /// Header line: the current directory path.
    pub fn location_label(&self) -> String {
        self.current_dir.display().to_string()
    }

    /// The drive root of `current_dir` (walk up to the ancestor with no parent), e.g. `Z:\` for
    /// `Z:\home\banon`.
    fn current_drive_root(&self) -> PathBuf {
        if let Some(text) = windows_like_path_text(&self.current_dir) {
            return PathBuf::from(&text[..3]);
        }
        let mut p = self.current_dir.as_path();
        while let Some(parent) = p.parent() {
            if parent.as_os_str().is_empty() {
                break;
            }
            p = parent;
        }
        p.to_path_buf()
    }

    /// Index of the current drive in the enumerated list (0 when the current path is not under any
    /// enumerated drive, so cycling still has a defined starting point).
    fn drive_index(&self) -> usize {
        let cur = self.current_drive_root();
        self.drives
            .iter()
            .position(|drive| drive == &cur)
            .unwrap_or(0)
    }

    /// The drive root one step forward/backward from the current one (wrapping). `None` with fewer
    /// than two drives -- there is nowhere to go.
    fn neighbour_drive(&self, forward: bool) -> Option<PathBuf> {
        let n = self.drives.len();
        if n < 2 {
            return None;
        }
        let idx = self.drive_index();
        let next = if forward {
            (idx + 1) % n
        } else {
            (idx + n - 1) % n
        };
        self.drives.get(next).cloned()
    }

    /// The drive roots this browser can cycle through.
    pub fn drive_count(&self) -> usize {
        self.drives.len()
    }

    fn drive_strip_real_capacity(&self) -> usize {
        let left = usize::from(self.drive_strip_offset > 0);
        let right = usize::from(
            self.drive_strip_offset + DRIVE_STRIP_MAX_CELLS.saturating_sub(left)
                < self.drives.len(),
        );
        DRIVE_STRIP_MAX_CELLS.saturating_sub(left + right).max(1)
    }

    fn clamp_drive_strip_offset(&mut self) {
        // Every nonzero page spends one cell on `[<]`; the last page therefore shows at most
        // MAX-1 real drives. Clamping against `len-MAX` made the final drive unreachable whenever
        // one extra page was needed (8 drives in a 7-cell strip snapped offset 2 back to 1).
        let last_page_real_capacity = DRIVE_STRIP_MAX_CELLS.saturating_sub(1).max(1);
        let max_offset = self.drives.len().saturating_sub(last_page_real_capacity);
        self.drive_strip_offset = self.drive_strip_offset.min(max_offset);
    }

    fn ensure_current_drive_cell_visible(&mut self) {
        let idx = self.drive_index();
        if idx < self.drive_strip_offset {
            self.drive_strip_offset = idx;
        }
        let cap = self.drive_strip_real_capacity();
        if idx >= self.drive_strip_offset + cap {
            self.drive_strip_offset = idx.saturating_sub(cap.saturating_sub(1));
        }
        self.clamp_drive_strip_offset();
    }

    pub fn drive_strip_cell_count(&self) -> usize {
        if !self.has_drive_row() {
            return 0;
        }
        let mut cells = self.drive_strip_real_capacity().min(self.drives.len());
        cells += usize::from(self.drive_strip_offset > 0);
        cells += usize::from(
            self.drive_strip_offset + self.drive_strip_real_capacity() < self.drives.len(),
        );
        cells.min(DRIVE_STRIP_MAX_CELLS)
    }

    /// Visual cell containing the current drive. Row focus is deliberately separate: callers hide
    /// the cell cursor when the native list cursor leaves the drive row without forgetting which
    /// drive becomes active when focus returns.
    pub fn drive_strip_active_cell(&self) -> Option<usize> {
        if !self.has_drive_row() {
            return None;
        }
        let drive_index = self.drive_index();
        let capacity = self.drive_strip_real_capacity();
        if drive_index < self.drive_strip_offset
            || drive_index >= self.drive_strip_offset + capacity
        {
            return None;
        }
        Some(usize::from(self.drive_strip_offset > 0) + drive_index - self.drive_strip_offset)
    }

    pub fn drive_strip_focus(&self) -> Option<DriveStripFocus> {
        if self.drive_strip_path_focused && self.has_drive_row() {
            Some(DriveStripFocus::CurrentPath)
        } else {
            self.drive_strip_active_cell().map(DriveStripFocus::Cell)
        }
    }

    fn drive_strip_cell_label(&self, drive_index: usize) -> String {
        let current = self.current_drive_root();
        let drive = self.drives.get(drive_index).unwrap_or(&current);
        let short = Self::drive_short(drive);
        if drive == &current {
            format!(">{short}<")
        } else {
            format!("[{short}]")
        }
    }

    fn drive_strip_cells(&self) -> Vec<String> {
        if !self.has_drive_row() {
            return Vec::new();
        }
        let mut labels = Vec::new();
        if self.drive_strip_offset > 0 {
            labels.push("[<]".to_owned());
        }
        let cap = self.drive_strip_real_capacity();
        for idx in self.drive_strip_offset..(self.drive_strip_offset + cap).min(self.drives.len()) {
            labels.push(self.drive_strip_cell_label(idx));
        }
        if self.drive_strip_offset + cap < self.drives.len() {
            labels.push("[>]".to_owned());
        }
        labels
    }

    fn drive_strip_label(&self) -> String {
        self.drive_strip_cells().join("  ")
    }

    /// Text for one visual drive-strip cell on the drive row. The runtime maps these cells onto
    /// distinct ProfileSelect child text fields (`DriveCell_0..25`) instead of one concatenated row label.
    pub fn drive_row_cell_label(&self, row: usize, cell: usize) -> Option<String> {
        (self.drive_row() == Some(row))
            .then(|| self.drive_strip_cells().get(cell).cloned())
            .flatten()
    }

    pub fn activate_drive_strip_cell(&mut self, cell: usize) -> bool {
        if !self.has_drive_row() || cell >= self.drive_strip_cell_count() {
            return false;
        }
        self.drive_strip_path_focused = false;
        let mut cell = cell;
        if self.drive_strip_offset > 0 {
            if cell == 0 {
                self.drive_strip_offset = self.drive_strip_offset.saturating_sub(1);
                return true;
            }
            cell -= 1;
        }
        let cap = self.drive_strip_real_capacity();
        if cell >= cap {
            if self.drive_strip_offset + cap < self.drives.len() {
                self.drive_strip_offset += 1;
                self.clamp_drive_strip_offset();
                return true;
            }
            return false;
        }
        let Some(root) = self.drives.get(self.drive_strip_offset + cell).cloned() else {
            return false;
        };
        let changed = self.switch_to_drive_root(root);
        if changed && let Some(row) = self.drive_row() {
            self.cursor = row;
        }
        changed
    }

    fn switch_to_drive_root(&mut self, root: PathBuf) -> bool {
        self.clear_status_message();
        let cur = self.current_drive_root();
        if cur == root {
            self.ensure_current_drive_cell_visible();
            return true;
        }
        self.last_dir_per_drive
            .insert(cur.clone(), self.current_dir.clone());
        let resumed = self
            .last_dir_per_drive
            .get(&root)
            .filter(|dir| dir.is_dir())
            .cloned();
        let restored = resumed.is_some();
        self.current_dir = resumed.unwrap_or_else(|| root.clone());
        self.refresh();
        self.ensure_current_drive_cell_visible();
        self.cursor = self.first_selectable_row();
        append_autoload_debug(format_args!(
            "save-picker: drive select {} -> {} (resumed_last_folder={restored} dir='{}')",
            cur.display(),
            root.display(),
            self.current_dir.display()
        ));
        true
    }

    /// Switch to the previous/next mounted drive (wrapping), RESUMING the folder last browsed on
    /// that drive. No-op with fewer than two drives.
    ///
    /// The folder being left is remembered against its own drive root first, so cycling away and
    /// back is lossless -- which is what makes the cycler useful for the case it exists for:
    /// hopping between a save directory on one drive and a save directory on another without
    /// re-walking either path. A remembered folder that has since disappeared falls back to the
    /// drive root rather than browsing a path that no longer resolves.
    pub fn cycle_drive(&mut self, forward: bool) {
        let Some(root) = self.neighbour_drive(forward) else {
            return;
        };
        let _ = self.switch_to_drive_root(root);
    }

    pub fn focus_active_drive_from_drive_strip(&mut self) -> bool {
        let changed = self.drive_strip_path_focused;
        self.drive_strip_path_focused = false;
        if let Some(row) = self.drive_row() {
            self.cursor = row;
        }
        changed
    }

    pub fn focus_current_path_from_drive_strip(&mut self) -> bool {
        if !self.has_drive_row() {
            return false;
        }
        let changed = !self.drive_strip_path_focused;
        self.drive_strip_path_focused = true;
        if let Some(row) = self.drive_row() {
            self.cursor = row;
        }
        changed
    }

    pub fn cycle_drive_from_drive_strip(&mut self, forward: bool) -> bool {
        if self.drive_strip_path_focused {
            if forward {
                return false;
            }
            self.drive_strip_path_focused = false;
            let Some(root) = self.drives.last().cloned() else {
                return false;
            };
            let _ = self.switch_to_drive_root(root);
            if let Some(row) = self.drive_row() {
                self.cursor = row;
            }
            return true;
        }

        let current_index = self.drive_index();
        if forward && current_index + 1 >= self.drives.len() {
            return self.focus_current_path_from_drive_strip();
        }

        let before = self.current_drive_root();
        self.cycle_drive(forward);
        self.drive_strip_path_focused = false;
        let changed = self.current_drive_root() != before;
        if changed && let Some(row) = self.drive_row() {
            self.cursor = row;
        }
        changed
    }

    /// True when the highlighted row is the drive cycler (so the overlay's left/right cycle drives
    /// instead of pages).
    pub fn cursor_on_drive_selector(&self) -> bool {
        self.drive_row() == Some(self.cursor)
    }

    pub fn current_dir(&self) -> &Path {
        &self.current_dir
    }

    /// Validate and commit a complete directory path entered through the native text editor.
    ///
    /// Case and spaces are preserved exactly. On Windows/Wine, a Linux-form `/home/...` absolute
    /// path is translated to the equivalent `Z:\\home\\...`; drive-prefixed paths are unchanged.
    /// Existence and absoluteness are checked against a temporary `PathBuf` before `self` changes, so
    /// invalid Accept and native Back/cancel both leave the directory/listing untouched.
    pub fn set_current_dir_from_text(&mut self, text: &str) -> Result<bool, DirectoryChangeError> {
        if text.is_empty() {
            return Err(DirectoryChangeError::Empty);
        }
        let candidate = entered_directory_candidate(text);
        if !complete_directory_text_is_absolute(text, &candidate) {
            return Err(DirectoryChangeError::NotAbsolute);
        }
        if !candidate.is_dir() {
            return Err(DirectoryChangeError::NotDirectory);
        }
        self.clear_status_message();
        if candidate == self.current_dir {
            return Ok(false);
        }

        let old_root = self.current_drive_root();
        self.last_dir_per_drive
            .insert(old_root, self.current_dir.clone());
        self.current_dir = candidate;
        self.refresh();
        self.ensure_current_drive_cell_visible();
        self.cursor = self
            .drive_row()
            .unwrap_or_else(|| self.first_selectable_row());
        Ok(true)
    }

    pub fn extension(&self) -> &str {
        &self.extension
    }

    pub fn page(&self) -> usize {
        0
    }

    pub fn page_count(&self) -> usize {
        1
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn scroll_max(&self) -> usize {
        self.max_scroll_offset()
    }

    pub fn scroll_window_one(&mut self, down: bool) -> bool {
        let old = self.scroll_offset;
        if down {
            self.scroll_offset = (self.scroll_offset + 1).min(self.max_scroll_offset());
        } else {
            self.scroll_offset = self.scroll_offset.saturating_sub(1);
        }
        let changed = self.scroll_offset != old;
        if changed {
            self.clear_status_message();
        }
        changed
    }

    /// Scroll the ten-row native window by one row for an explicit UP/DOWN press taken at an edge.
    ///
    /// `cursor` is the row the selection occupied BEFORE the press. The native list moves (and
    /// wraps) its own cursor as soon as the key is read, so the value sampled after the press is
    /// already somewhere else -- at the bottom row a DOWN press lands the selection back on the
    /// drives row, which is what the player sees as "it wrapped instead of scrolling".
    ///
    /// Scrolls if and only if the window can actually move that way, which is exactly the condition
    /// the scrollbar draws: more rows below for DOWN, more above for UP. When the list is already at
    /// that hard limit the press still reports a row to pin, so the caller can hold the selection
    /// where it was instead of letting the native list wrap around to the far end. Returns `None`
    /// only for a press that is not at an edge at all, where ordinary row movement is correct.
    ///
    /// Replaced a dwell timer (2026-08-12) that slid the window whenever the cursor merely SAT on
    /// an edge row, moving the list under a player who was only resting there.
    pub fn scroll_window_from_edge_press(
        &mut self,
        cursor: usize,
        down: bool,
    ) -> Option<EdgePressOutcome> {
        let first_content_row = self
            .entry_row_base()
            .min(PICKER_ROW_COUNT.saturating_sub(1));
        let last_visible_row = self
            .visible_row_count()
            .saturating_sub(1)
            .min(PICKER_ROW_COUNT.saturating_sub(1));
        let (at_edge, edge_row) = if down {
            (cursor >= last_visible_row, last_visible_row)
        } else {
            (cursor <= first_content_row, first_content_row)
        };
        if !at_edge {
            return None;
        }
        // `scroll_window_one` is the single source of "can the window move that way", so it decides
        // scroll-versus-hold on exactly the condition the scrollbar draws.
        if self.scroll_window_one(down) {
            return Some(EdgePressOutcome::Scrolled { pin_row: edge_row });
        }
        // No window left to move. DOWN off the last row and UP off row 0 are where the native list
        // wraps to the opposite end of the listing, which reads as the selection teleporting; hold
        // the press row instead. An UP press that merely runs out of ENTRIES is not at the top of
        // the list -- the drive and parent rows sit above it -- so leave that to normal movement.
        let hard_limit = if down { true } else { cursor == 0 };
        hard_limit.then_some(EdgePressOutcome::HeldAtLimit { pin_row: cursor })
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn status_message(&self) -> Option<&PickerStatusMessage> {
        self.status_message.as_ref()
    }

    pub fn set_status_message(&mut self, message: PickerStatusMessage) {
        self.status_message = Some(message);
    }

    /// Entering the complete-path editor starts a fresh correction attempt. Hide any rejection from
    /// the previous Accept before the user types so stale warning copy never sits behind the editor.
    pub fn begin_path_edit(&mut self) -> bool {
        self.status_message.take().is_some()
    }

    pub fn clear_status_message(&mut self) {
        self.status_message = None;
        self.rejected_path_text = None;
    }

    /// The rejected CurrentPath entry to display in place of `current_dir`, if one is outstanding.
    pub fn rejected_path_text(&self) -> Option<&str> {
        self.rejected_path_text.as_deref()
    }

    /// Keep an invalid entry on the control so the user can correct it rather than retype it.
    pub fn set_rejected_path_text(&mut self, text: &str) {
        self.rejected_path_text = Some(text.to_owned());
    }

    /// Re-enumerate `current_dir`. Unreadable directories yield an empty listing rather than an
    /// error: the picker stays navigable (the user can still go up or change drive) and the debug
    /// log records the failure.
    pub fn refresh(&mut self) {
        // The listing is about to describe a real directory again, so any rejected entry the
        // control was showing is stale by definition.
        self.rejected_path_text = None;
        self.entries.clear();
        self.scroll_offset = 0;
        // Owned copies so the per-entry predicate borrows nothing from `self` while the listing is
        // being built. `save_picker_accepts` is the SAME function the OS dialog's post-return check
        // calls, which is what keeps the two surfaces from disagreeing about what a save is.
        let intent = self.intent.clone();
        let filters = self.extensions.clone();
        let filter_refs: Vec<&str> = filters.iter().map(String::as_str).collect();
        let read = match std::fs::read_dir(&self.current_dir) {
            Ok(read) => read,
            Err(err) => {
                append_autoload_debug(format_args!(
                    "save-picker: read_dir failed for '{}': {err}",
                    self.current_dir.display()
                ));
                return;
            }
        };
        let mut dirs: Vec<PickerEntry> = Vec::new();
        let mut files: Vec<PickerEntry> = Vec::new();
        let mut raw = 0usize;
        for entry in read.flatten() {
            raw += 1;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            // Hide dot-prefixed (hidden) entries -- `.config`, `.snapshots`, `.local`, etc.
            if name.starts_with('.') {
                continue;
            }
            // Detect the kind by STAT'ing the target (`Path::is_dir`/`is_file`), not the dirent
            // `file_type` (which does not follow symlinks and mis-reports reparse points): under
            // Wine, symlinked or btrfs-subvolume directories at the `Z:\` (= `/`) root -- `/usr`,
            // `/bin`, `/home`, ... -- come back as non-directory reparse points, so `file_type`
            // dropped them and only plain dirs like `/etc`,`/run`,`/var` survived.
            if path.is_dir() {
                dirs.push(PickerEntry::Dir {
                    name: name.to_owned(),
                    path: path.clone(),
                });
                continue;
            }
            match save_picker_accepts(&path, &intent, &filter_refs) {
                Ok(chars) => files.push(PickerEntry::File {
                    name: name.to_owned(),
                    path: path.clone(),
                    modified: entry.metadata().ok().and_then(|meta| meta.modified().ok()),
                    chars,
                }),
                // The overwhelmingly common case: an ordinary file that is not a save container at
                // all. Logging it would drown the listing diagnostic in every folder's contents.
                Err(PickRejection::WrongExtension) => {}
                Err(reason) => append_autoload_debug(format_args!(
                    "save-picker: hiding '{}' -- {reason:?}",
                    path.display()
                )),
            }
        }
        dirs.sort_by(|a, b| {
            a.name()
                .to_ascii_lowercase()
                .cmp(&b.name().to_ascii_lowercase())
        });
        files.sort_by(|a, b| {
            let (PickerEntry::File { modified: ma, .. }, PickerEntry::File { modified: mb, .. }) =
                (a, b)
            else {
                return std::cmp::Ordering::Equal;
            };
            mb.cmp(ma).then_with(|| {
                a.name()
                    .to_ascii_lowercase()
                    .cmp(&b.name().to_ascii_lowercase())
            })
        });
        // Diagnostic: log every listing outcome (not just failures) so a Wine drive-root
        // enumeration quirk (e.g. `Z:\` = `/` returning fewer/other entries than a subpath) is
        // visible in the debug log.
        let sample: Vec<&str> = dirs.iter().take(6).map(PickerEntry::name).collect();
        append_autoload_debug(format_args!(
            "save-picker: listed '{}' -> {} raw entries, {} dirs, {} files (first dirs: {:?})",
            self.current_dir.display(),
            raw,
            dirs.len(),
            files.len(),
            sample
        ));
        self.entries = dirs;
        self.entries.append(&mut files);
    }

    fn window_entries(&self) -> &[PickerEntry] {
        let start = self.scroll_offset.min(self.entries.len());
        let end = (start + self.entry_window_capacity()).min(self.entries.len());
        self.entries.get(start..end).unwrap_or(&[])
    }

    /// Meaning of `row` (0..PICKER_ROW_COUNT) in the current scroll window.
    pub fn row_meaning(&self, row: usize) -> PickerRow {
        if row >= PICKER_ROW_COUNT {
            return PickerRow::Empty;
        }
        if self.new_file_row() == Some(row) {
            return self
                .new_file_target()
                .map_or(PickerRow::Empty, PickerRow::NewFile);
        }
        if self.parent_row() == Some(row) {
            return PickerRow::ParentDir;
        }
        if self.drive_row() == Some(row) {
            return PickerRow::DriveCycle;
        }
        if self.next_page_row() == Some(row) {
            return PickerRow::NextPage;
        }
        match row
            .checked_sub(self.entry_window_row_base())
            .and_then(|idx| self.window_entries().get(idx))
        {
            Some(PickerEntry::Dir { path, .. }) => PickerRow::Dir(path.clone()),
            Some(PickerEntry::File { path, .. }) => PickerRow::File(path.clone()),
            // Nothing above this row and nothing to list: name the dead end instead of leaving the
            // native window with zero rows. Reachable only at a drive root with no other drive, no
            // entries, and no `[ new ]` row -- see `visible_row_count`.
            None if row == 0 => PickerRow::AtRoot,
            None => PickerRow::Empty,
        }
    }

    /// The cached character summaries behind `row` when it is a save-file row in the current scroll
    /// window (the file's active loadable characters, parsed once at listing build). `None` for every
    /// non-file row (up, drive cycler, `[ new ]`, directory, placeholder).
    ///
    /// Derived from the SAME `row_meaning` the label comes from, then cross-checked: the entry read
    /// at the scroll-window index must be the very file the label named. One decision point plus a proof,
    /// so the stats text and the row label cannot describe different entries -- and if they ever
    /// disagree the row renders BLANK rather than a neighbour's character.
    pub fn row_file_characters(&self, row: usize) -> Option<&[crate::slots::SaveSlotInfo]> {
        let PickerRow::File(labelled) = self.row_meaning(row) else {
            return None;
        };
        match self
            .window_entries()
            .get(row.checked_sub(self.entry_row_base())?)
        {
            Some(PickerEntry::File { path, chars, .. }) if *path == labelled => Some(chars),
            _ => None,
        }
    }

    /// When the file behind `row` was last written -- the timestamp the row shows in place of the
    /// native playtime -- or `None` for every other row kind, and for a file whose metadata the
    /// listing build could not read.
    ///
    /// Cross-checked exactly like [`row_file_characters`](Self::row_file_characters), and for the
    /// same reason: the entry read at the scroll-window index must be the very file the label named, or the
    /// row would date itself from a neighbour. Reads only what the listing build already collected
    /// (`PickerEntry::File::modified`, the dirent metadata the sort order is derived from), so no
    /// row query ever touches the filesystem.
    pub fn row_last_saved(&self, row: usize) -> Option<SystemTime> {
        let PickerRow::File(labelled) = self.row_meaning(row) else {
            return None;
        };
        match self
            .window_entries()
            .get(row.checked_sub(self.entry_row_base())?)
        {
            Some(PickerEntry::File { path, modified, .. }) if *path == labelled => *modified,
            _ => None,
        }
    }

    /// Apply the effect of activating `row`.
    pub fn activate(&mut self, row: usize) -> PickerActivation {
        let meaning = self.row_meaning(row);
        if !matches!(meaning, PickerRow::AtRoot | PickerRow::Empty) {
            self.clear_status_message();
        }
        match meaning {
            PickerRow::ParentDir => {
                if let Some(parent) = path_parent(&self.current_dir) {
                    self.current_dir = parent;
                    self.refresh();
                    return PickerActivation::Repopulate;
                }
                PickerActivation::Ignored
            }
            // The row itself is a strip container. Selecting a drive is a cell-level action routed
            // through `activate_drive_strip_cell`; pressing the row background must not cycle through
            // drives one-by-one.
            PickerRow::DriveCycle => PickerActivation::Ignored,
            PickerRow::Dir(path) => {
                self.current_dir = path;
                self.refresh();
                PickerActivation::Repopulate
            }
            PickerRow::File(path) => PickerActivation::PickedFile(path),
            PickerRow::NewFile(path) => PickerActivation::PickedNewFile(path),
            PickerRow::ScrollUp => {
                self.cycle_page(false);
                PickerActivation::Repopulate
            }
            PickerRow::ScrollDown => {
                self.cycle_page(true);
                PickerActivation::Repopulate
            }
            PickerRow::NextPage => PickerActivation::Ignored,
            PickerRow::AtRoot | PickerRow::Empty => PickerActivation::Ignored,
        }
    }

    /// Name of the folder the `[..]` row navigates to (the parent of `current_dir`), or `None` at a
    /// drive root. Used to label the up row with its destination.
    fn parent_dir_name(&self) -> Option<String> {
        let parent = path_parent(&self.current_dir)?;
        Some(match path_file_name_text(&parent) {
            Some(name) => name.to_owned(),
            // A drive root has no file name; show the root itself (e.g. `Z:\`) so the row still
            // names where it goes.
            None => parent.display().to_string(),
        })
    }

    /// Drive root trimmed for a row label: `Z:\` -> `Z:`.
    fn drive_short(root: &Path) -> String {
        root.display()
            .to_string()
            .trim_end_matches(['\\', '/'])
            .to_owned()
    }

    /// Name field for the drive strip row. It is only the row title; the actual clickable drive cells
    /// render through separate DLL-owned row children so the UI no longer lies with one merged label.
    fn drive_row_label(&self) -> String {
        "DRIVES".to_owned()
    }

    /// Display label for `row`, truncated to the ProfileSummary name budget (16 UTF-16 units).
    /// Directory rows carry a `/` suffix; control rows use bracketed labels. Every VISIBLE row's
    /// label is guaranteed non-empty so staged records pass the empty-slot activation guard.
    pub fn row_label_utf16(&self, row: usize) -> Vec<u16> {
        let label = match self.row_meaning(row) {
            // Name the destination, not just the direction: `[..] Roaming` says where this row
            // goes without having to press it.
            PickerRow::ParentDir => match self.parent_dir_name() {
                Some(name) => format!("[..] {name}"),
                None => "[ .. up ]".to_owned(),
            },
            PickerRow::DriveCycle => self.drive_row_label(),
            PickerRow::AtRoot => "[ root ]".to_owned(),
            PickerRow::Dir(path) => self.dir_display_name(&path),
            PickerRow::File(path) => path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("?")
                .to_owned(),
            PickerRow::NewFile(_) => PICKER_NEW_FILE_LABEL.to_owned(),
            PickerRow::ScrollUp => "[ SCROLL ^ ]".to_owned(),
            PickerRow::ScrollDown => "[ SCROLL v ]".to_owned(),
            PickerRow::NextPage => String::new(),
            PickerRow::Empty => String::new(),
        };
        truncate_utf16(&label, PICKER_ROW_NAME_UTF16_MAX)
    }

    /// Display name for a directory row: the folder name with a `/`, or the full root path (e.g.
    /// `Z:\`) for a drive root (which has no file name).
    fn dir_display_name(&self, path: &Path) -> String {
        match path.file_name().and_then(|name| name.to_str()) {
            Some(name) => format!("{name}/"),
            None => path.display().to_string(),
        }
    }

    /// Two auxiliary display fragments for a non-file row, rendered through the injected native
    /// `05_010` `ErStats` field while the in-game picker owns the ProfileSelect rows.
    ///
    /// Row names stay inside the 16-UTF-16 `ProfileSummary` budget; explanatory text lives here
    /// instead. File rows return `None` because their two stats lines are real character summaries,
    /// and empty rows return `None` because the native list builder omits them. A visible status
    /// message owns row 0 first, matching the runtime stats-line hook.
    pub fn row_auxiliary_lines(&self, row: usize) -> Option<(String, String)> {
        if let Some(message) = &self.status_message
            && row == 0
        {
            return Some((message.headline().to_owned(), message.detail().to_owned()));
        }
        match self.row_meaning(row) {
            PickerRow::ParentDir => Some((
                "PARENT FOLDER".to_owned(),
                self.parent_dir_name()
                    .map(|name| format!("Go to {name}"))
                    .unwrap_or_else(|| "Go up one folder".to_owned()),
            )),
            PickerRow::DriveCycle => None,
            PickerRow::AtRoot => Some((
                "DRIVE ROOT".to_owned(),
                self.current_drive_root().display().to_string(),
            )),
            PickerRow::Dir(path) => Some((
                "FOLDER".to_owned(),
                format!("Open {}", self.dir_display_name(&path)),
            )),
            PickerRow::NewFile(path) => Some((
                "NEW SAVE FILE".to_owned(),
                format!(
                    "Create {}",
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("the loaded save")
                ),
            )),
            PickerRow::ScrollUp => Some((
                "MORE ABOVE".to_owned(),
                format!("Show rows before {}", self.scroll_offset + 1),
            )),
            PickerRow::ScrollDown => Some((
                "MORE BELOW".to_owned(),
                format!(
                    "Show rows after {}",
                    self.scroll_offset + self.window_entries().len()
                ),
            )),
            PickerRow::NextPage => None,
            PickerRow::File(_) | PickerRow::Empty => None,
        }
    }

    /// ASCII display label for `row` (uppercased for the 5x7 overlay font; dir rows keep a `/`
    /// suffix, control rows are bracketed). Empty string for an out-of-range row. The overlay has
    /// far more width than the native name field, so these spell the action out.
    pub fn row_label_ascii(&self, row: usize) -> String {
        let label = match self.row_meaning(row) {
            PickerRow::ParentDir => match self.parent_dir_name() {
                Some(name) => format!("[..] UP    {name}"),
                None => "[..] UP".to_owned(),
            },
            // The overlay drives this row with left/right as well as select; mouse users click a cell.
            PickerRow::DriveCycle => format!("DRIVES {}", self.drive_strip_label()),
            PickerRow::AtRoot => format!("[ROOT] {}", self.current_drive_root().display()),
            PickerRow::Dir(path) => self.dir_display_name(&path),
            PickerRow::File(path) => path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("?")
                .to_owned(),
            PickerRow::NewFile(path) => format!(
                "{PICKER_NEW_FILE_LABEL}  {}",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("?")
            ),
            PickerRow::ScrollUp => "[ SCROLL UP ]".to_owned(),
            PickerRow::ScrollDown => "[ SCROLL DOWN ]".to_owned(),
            PickerRow::NextPage => String::new(),
            PickerRow::Empty => String::new(),
        };
        label.to_ascii_uppercase()
    }

    /// True if `row` can be highlighted/activated (not a row beyond the listing).
    fn row_selectable(&self, row: usize) -> bool {
        !matches!(self.row_meaning(row), PickerRow::Empty)
    }

    fn first_selectable_row(&self) -> usize {
        // A DESTINATION BROWSE STARTS ON `[ new ]`, in every folder, always. The reviewer's whole
        // complaint about the old flow was that its default answer was the destructive one, so the
        // one row this cursor may rest on by default is the row that creates rather than replaces
        // -- and when the browsed folder happens to be the loaded save's own, the overwrite confirm
        // (default No) still stands between that row and any damage.
        if let Some(new_row) = self.new_file_row()
            && self.row_selectable(new_row)
        {
            return new_row;
        }
        // LOAD BROWSE: prefer the first row AFTER the pure-navigation rows so a fresh listing lands
        // on something actionable -- an entry -- rather than on `[..] up` or the drive cycler. Fall
        // back to any selectable row (a folder with nothing in it), else 0.
        let first_entry = self.entry_row_base();
        (first_entry..PICKER_ROW_COUNT)
            .find(|&r| self.row_selectable(r))
            .or_else(|| (0..PICKER_ROW_COUNT).find(|&r| self.row_selectable(r)))
            .unwrap_or(0)
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Move the highlight directly to a visible/selectable row. Used by mouse hit-testing surfaces
    /// that resolve a click to the row under the pointer before activating it.
    pub fn set_cursor(&mut self, row: usize) {
        if row < PICKER_ROW_COUNT && self.row_selectable(row) {
            self.cursor = row;
        }
    }

    /// Move the highlight one selectable row up (`down=false`) or down, wrapping. No-op when only
    /// one row is selectable.
    pub fn move_cursor(&mut self, down: bool) {
        let selectable: Vec<usize> = (0..PICKER_ROW_COUNT)
            .filter(|&r| self.row_selectable(r))
            .collect();
        if selectable.len() < 2 {
            self.cursor = selectable.first().copied().unwrap_or(0);
            return;
        }
        let pos = selectable
            .iter()
            .position(|&r| r == self.cursor)
            .unwrap_or(0);
        let next = if down {
            (pos + 1) % selectable.len()
        } else {
            (pos + selectable.len() - 1) % selectable.len()
        };
        self.cursor = selectable[next];
    }

    /// Activate the highlighted row. On a listing change (dir/drive/scroll) the cursor resets to the
    /// first selectable row so the highlight never lands on a stale index.
    pub fn activate_cursor(&mut self) -> PickerActivation {
        let result = self.activate(self.cursor);
        if matches!(result, PickerActivation::Repopulate) {
            self.cursor = self.first_selectable_row();
        }
        result
    }

    /// Compatibility wrapper for old overlay callers: move the scroll window by one native page.
    pub fn cycle_page(&mut self, forward: bool) {
        let cap = self.entry_window_capacity();
        if self.max_scroll_offset() == 0 {
            return;
        }
        self.clear_status_message();
        self.scroll_offset = if forward {
            (self.scroll_offset + cap).min(self.max_scroll_offset())
        } else {
            self.scroll_offset.saturating_sub(cap)
        };
        self.cursor = self.first_selectable_row();
    }

    /// Navigate to the parent directory (no-op at a drive root -- switch drives with the drive
    /// cycler row instead). Resets the cursor.
    pub fn go_up(&mut self) {
        if let Some(parent) = self.current_dir.parent().map(Path::to_path_buf)
            && !parent.as_os_str().is_empty()
        {
            self.clear_status_message();
            self.current_dir = parent;
            self.refresh();
            self.cursor = self.first_selectable_row();
        }
    }

    /// Long-form status line for the auxiliary text fields (full current dir + scroll info).
    pub fn status_line(&self) -> String {
        format!(
            "{}  (rows {}-{}/{}, *.{})",
            self.current_dir.display(),
            self.scroll_offset + 1,
            (self.scroll_offset + self.window_entries().len()).max(1),
            self.entries.len().max(1),
            self.extension
        )
    }
}

/// UTF-16 encode with truncation to `max` units (no NUL appended).
pub fn truncate_utf16(text: &str, max: usize) -> Vec<u16> {
    text.encode_utf16().take(max).collect()
}

#[cfg(test)]
mod tests;

/// The active picker instance, shared between the open path (menu action) and the activation
/// hook. `None` when no in-game picker is open. Sites: System>Quit picker and the startup
/// missing-save picker (mutually exclusive by construction -- the startup picker resolves
/// before the System menu is reachable).
pub static ACTIVE_SAVE_PICKER: Mutex<Option<SavePickerModel>> = Mutex::new(None);

/// Lock helper that recovers from poisoning (same pattern as `state_or_return`).
pub fn active_save_picker_lock() -> std::sync::MutexGuard<'static, Option<SavePickerModel>> {
    ACTIVE_SAVE_PICKER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
