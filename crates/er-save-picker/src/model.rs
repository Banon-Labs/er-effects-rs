//! Shared in-game save-file picker model.
//!
//! Pure filesystem/pagination state for the two in-game file-picker menus (the startup
//! missing-save picker and the System>Quit "Load Character from File" picker). Both menus render
//! through the native `05_010_ProfileSelect` 10-row window, so this model maps a browsable
//! directory listing onto row pages. The UI layers own all native staging (ProfileSummary preview
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
//! 1. `[ new ]`       -- destination intent only, and FIRST;
//! 2. `[..] <parent>` -- only when the current directory has a parent (absent at a drive root);
//! 3. `[ C: > Z: ]`   -- the drive cycler, only when more than one drive is mounted;
//! 4. the current page's directory / save-file entries;
//! 5. `[ page N/M ]`  -- only when the listing overflows one page.
//!
//! `[ new ]` SITS ABOVE THE NAVIGATION ROWS, which is the one place the two intents' layouts
//! differ, and it is deliberate. Since the Save Game row press opens this browser with no question
//! in front of it (2026-07-31), the destination list is the first thing the user sees after
//! pressing Save Game -- so the row that means "write a fresh file, destroy nothing" is row 0, the
//! index a freshly built native list highlights, and the index the model's own cursor starts on
//! ([`SavePickerModel::first_selectable_row`]). Row 0 in a LOAD browse still means what it always
//! did; only the destination browser has a `[ new ]` at all.
//!
//! Nothing sits at a fixed index: [`SavePickerModel::entry_row_base`] is the single place the
//! layout is decided, and every row query derives from it. That matters for two reasons.
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

/// What a row on the CURRENT page means. Produced by [`SavePickerModel::row_meaning`]; the UI
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
    /// Advance to the next page (wraps to the first page after the last).
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
/// new page) and the UI must re-stage row records and re-present the window.
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
    page: usize,
    /// Highlighted row index (0..PICKER_ROW_COUNT) for the overlay picker. Clamped to a
    /// selectable (non-Empty) row on every listing change.
    cursor: usize,
    /// Last rejection/status line the picker should render on the current surface. Cleared by
    /// navigation or a fresh pick attempt so a stale error never follows the user into another
    /// folder/page.
    status_message: Option<PickerStatusMessage>,
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
        && text.as_bytes().get(2) == Some(&b'\\'))
    .then_some(text)
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
            page: 0,
            cursor: 0,
            status_message: None,
            drives: enumerate_drives(),
            last_dir_per_drive: HashMap::new(),
            intent,
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

    /// True when a drive cycler row is worth showing. With one drive (or none enumerated) there is
    /// nowhere to cycle to, so the row would be a dead end and is omitted.
    fn has_drive_row(&self) -> bool {
        self.drives.len() >= 2
    }

    /// Rows above the entries that are pure NAVIGATION -- never an entry, never a pick target:
    /// the up row and the drive cycler. The initial cursor skips these so a fresh listing lands on
    /// something actionable.
    fn nav_row_count(&self) -> usize {
        usize::from(self.has_parent_row()) + usize::from(self.has_drive_row())
    }

    /// Rows the PINNED `[ new ]` row occupies above everything else: 1 in destination intent, 0 in
    /// a load browse. Every other row index in the layout is offset by exactly this.
    fn pinned_row_count(&self) -> usize {
        usize::from(self.is_destination())
    }

    /// Row index of the first directory/file entry.
    fn entry_row_base(&self) -> usize {
        self.pinned_row_count() + self.nav_row_count()
    }

    /// Row index of the pinned `[ new ]` row (destination intent only). ALWAYS 0 when it exists --
    /// see the module docs: it is the first thing the Save Game row press shows.
    pub fn new_file_row(&self) -> Option<usize> {
        self.is_destination().then_some(0)
    }

    /// Row index of the "up one directory" row, when the current directory has a parent. It is
    /// first in a load browse and second in a destination browse (`[ new ]` is pinned above it);
    /// at a drive root there is no up row at all.
    pub fn parent_row(&self) -> Option<usize> {
        self.has_parent_row().then(|| self.pinned_row_count())
    }

    /// Row index of the drive cycler, when it exists.
    pub fn drive_row(&self) -> Option<usize> {
        self.has_drive_row()
            .then(|| self.pinned_row_count() + usize::from(self.has_parent_row()))
    }

    /// Row index of the page cycler, when the listing overflows one page. It sits immediately
    /// after the CURRENT page's last entry, so a short final page has no gap above it -- and the
    /// native cursor re-clamp lands the highlight back on the cycler after paging.
    pub fn next_page_row(&self) -> Option<usize> {
        (self.page_count() > 1).then(|| self.entry_row_base() + self.page_entries().len())
    }

    /// Rows the window actually shows. Slots at or beyond this are staged UNOCCUPIED so the native
    /// list builder omits them (no name, no level, no playtime).
    pub fn visible_row_count(&self) -> usize {
        let rows = self.entry_row_base()
            + self.page_entries().len()
            + usize::from(self.next_page_row().is_some());
        // Never zero: an empty single-drive root has nothing above and nothing to list, and a
        // zero-row native list would leave the window with no selectable item at all. Row 0
        // becomes the `[ root ]` dead-end marker instead.
        rows.max(1)
    }

    /// Entries that fit when NO page cycler is needed (every row after the fixed rows is an entry).
    fn max_entries_single_page(&self) -> usize {
        PICKER_ROW_COUNT.saturating_sub(self.entry_row_base())
    }

    /// Entries per page. A listing that fits uses every remaining row; one that overflows spends
    /// one row on the page cycler. Non-circular: the branch reads the ENTRY count, never the page
    /// count, so `page_count` can derive from it without recursion.
    pub fn entries_per_page(&self) -> usize {
        let single = self.max_entries_single_page();
        if self.entries.len() <= single {
            single.max(1)
        } else {
            single.saturating_sub(1).max(1)
        }
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
        self.clear_status_message();
        let cur = self.current_drive_root();
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
        self.cursor = self.first_selectable_row();
        append_autoload_debug(format_args!(
            "save-picker: drive cycle {} -> {} (resumed_last_folder={restored} dir='{}')",
            cur.display(),
            root.display(),
            self.current_dir.display()
        ));
    }

    /// True when the highlighted row is the drive cycler (so the overlay's left/right cycle drives
    /// instead of pages).
    pub fn cursor_on_drive_selector(&self) -> bool {
        self.drive_row() == Some(self.cursor)
    }

    pub fn current_dir(&self) -> &Path {
        &self.current_dir
    }

    pub fn extension(&self) -> &str {
        &self.extension
    }

    pub fn page(&self) -> usize {
        self.page
    }

    pub fn page_count(&self) -> usize {
        self.entries.len().div_ceil(self.entries_per_page()).max(1)
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

    pub fn clear_status_message(&mut self) {
        self.status_message = None;
    }

    /// Re-enumerate `current_dir`. Unreadable directories yield an empty listing rather than an
    /// error: the picker stays navigable (the user can still go up or change drive) and the debug
    /// log records the failure.
    pub fn refresh(&mut self) {
        self.entries.clear();
        self.page = 0;
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

    fn page_entries(&self) -> &[PickerEntry] {
        let per_page = self.entries_per_page();
        let start = self.page * per_page;
        let end = (start + per_page).min(self.entries.len());
        self.entries.get(start..end).unwrap_or(&[])
    }

    /// Meaning of `row` (0..PICKER_ROW_COUNT) on the current page.
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
            .checked_sub(self.entry_row_base())
            .and_then(|idx| self.page_entries().get(idx))
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

    /// The cached character summaries behind `row` when it is a save-file row on the current page
    /// (the file's active loadable characters, parsed once at listing build). `None` for every
    /// non-file row (up, drive cycler, `[ new ]`, directory, page cycler, placeholder).
    ///
    /// Derived from the SAME `row_meaning` the label comes from, then cross-checked: the entry read
    /// at the page index must be the very file the label named. One decision point plus a proof,
    /// so the stats text and the row label cannot describe different entries -- and if they ever
    /// disagree the row renders BLANK rather than a neighbour's character.
    pub fn row_file_characters(&self, row: usize) -> Option<&[crate::slots::SaveSlotInfo]> {
        let PickerRow::File(labelled) = self.row_meaning(row) else {
            return None;
        };
        match self
            .page_entries()
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
    /// same reason: the entry read at the page index must be the very file the label named, or the
    /// row would date itself from a neighbour. Reads only what the listing build already collected
    /// (`PickerEntry::File::modified`, the dirent metadata the sort order is derived from), so no
    /// row query ever touches the filesystem.
    pub fn row_last_saved(&self, row: usize) -> Option<SystemTime> {
        let PickerRow::File(labelled) = self.row_meaning(row) else {
            return None;
        };
        match self
            .page_entries()
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
            // The native window gives us row ACTIVATION and nothing else, so the drive switch is a
            // row: one activation advances one drive, wrapping, and the row's own label names the
            // drive it is about to move to.
            PickerRow::DriveCycle => {
                self.cycle_drive(true);
                PickerActivation::Repopulate
            }
            PickerRow::Dir(path) => {
                self.current_dir = path;
                self.refresh();
                PickerActivation::Repopulate
            }
            PickerRow::File(path) => PickerActivation::PickedFile(path),
            PickerRow::NewFile(path) => PickerActivation::PickedNewFile(path),
            PickerRow::NextPage => {
                self.page = (self.page + 1) % self.page_count();
                PickerActivation::Repopulate
            }
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

    /// `[ C: > Z: ]` -- the drive being browsed and the one activation moves to. Naming the next
    /// drive is what keeps the row from being a blind cycler when three or more drives are mounted,
    /// and naming the current one is what makes "which drive am I on" answerable from any folder.
    /// Two 2-character drive names plus the frame is 11 UTF-16 units, inside the name budget, and
    /// contains no comma.
    fn drive_row_label(&self) -> String {
        let cur = self.current_drive_root();
        let next = self.neighbour_drive(true).unwrap_or_else(|| cur.clone());
        format!(
            "[ {} > {} ]",
            Self::drive_short(&cur),
            Self::drive_short(&next)
        )
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
            PickerRow::NextPage => {
                format!("[ page {}/{} ]", self.page + 1, self.page_count())
            }
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

    /// ASCII display label for `row` (uppercased for the 5x7 overlay font; dir rows keep a `/`
    /// suffix, control rows are bracketed). Empty string for an out-of-range row. The overlay has
    /// far more width than the native name field, so these spell the action out.
    pub fn row_label_ascii(&self, row: usize) -> String {
        let label = match self.row_meaning(row) {
            PickerRow::ParentDir => match self.parent_dir_name() {
                Some(name) => format!("[..] UP    {name}"),
                None => "[..] UP".to_owned(),
            },
            // The overlay drives this row with left/right as well as select, so say both.
            PickerRow::DriveCycle => format!(
                "DRIVE < {} >   (SELECT: NEXT)",
                self.current_drive_root().display()
            ),
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
            PickerRow::NextPage => format!("[PAGE {}/{}]", self.page + 1, self.page_count()),
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

    /// Activate the highlighted row. On a listing change (dir/drive/page) the cursor resets to the
    /// first selectable row so the highlight never lands on a stale index.
    pub fn activate_cursor(&mut self) -> PickerActivation {
        let result = self.activate(self.cursor);
        if matches!(result, PickerActivation::Repopulate) {
            self.cursor = self.first_selectable_row();
        }
        result
    }

    /// Move to the previous/next page (wrapping), resetting the cursor. No-op when single-page.
    pub fn cycle_page(&mut self, forward: bool) {
        let count = self.page_count();
        if count < 2 {
            return;
        }
        self.clear_status_message();
        self.page = if forward {
            (self.page + 1) % count
        } else {
            (self.page + count - 1) % count
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

    /// Long-form status line for the auxiliary text fields (full current dir + page info).
    pub fn status_line(&self) -> String {
        format!(
            "{}  (page {}/{}, *.{})",
            self.current_dir.display(),
            self.page + 1,
            self.page_count(),
            self.extension
        )
    }
}

/// UTF-16 encode with truncation to `max` units (no NUL appended).
pub fn truncate_utf16(text: &str, max: usize) -> Vec<u16> {
    text.encode_utf16().take(max).collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Build a model with a fixed listing, bypassing the filesystem enumeration `open` does.
    ///
    /// Each file carries ONE character whose name encodes the file's own index (`char{idx}`), and a
    /// modification time of epoch + `idx` minutes, so a test can assert that the character info AND
    /// the timestamp a row renders belong to that row's OWN file rather than a neighbour's -- the
    /// exact confusion `row_file_characters` had.
    ///
    /// `drives` is left EMPTY, so these models have no drive cycler row: the drive-row tests opt
    /// in explicitly via `with_drives`, and every other test keeps the no-drive-row layout.
    fn model_with(intent: PickerIntent, dir: &str, files: usize) -> SavePickerModel {
        SavePickerModel {
            current_dir: PathBuf::from(dir),
            extension: "sl2".to_owned(),
            extensions: vec!["sl2".to_owned()],
            entries: (0..files)
                .map(|idx| PickerEntry::File {
                    name: format!("save{idx}.sl2"),
                    path: PathBuf::from(dir).join(format!("save{idx}.sl2")),
                    modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(idx as u64 * 60)),
                    chars: vec![crate::slots::SaveSlotInfo {
                        slot: 0,
                        name: format!("char{idx}"),
                        level: 10 + idx as i32,
                    }],
                })
                .collect(),
            page: 0,
            cursor: 0,
            status_message: None,
            drives: Vec::new(),
            last_dir_per_drive: HashMap::new(),
            intent,
        }
    }

    /// Attach mounted drives so the drive cycler row exists (two or more) or deliberately does not.
    fn with_drives(mut model: SavePickerModel, drives: &[&str]) -> SavePickerModel {
        model.drives = drives.iter().map(PathBuf::from).collect();
        model
    }

    /// The character name a row's stats text would render, or `None` for a non-file row.
    fn row_char_name(model: &SavePickerModel, row: usize) -> Option<String> {
        model
            .row_file_characters(row)
            .and_then(|chars| chars.first())
            .map(|info| info.name.clone())
    }

    /// The file stem a row's LABEL would render, or `None` for a non-file row.
    fn row_label_file(model: &SavePickerModel, row: usize) -> Option<String> {
        match model.row_meaning(row) {
            PickerRow::File(path) => Some(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_owned(),
            ),
            _ => None,
        }
    }

    fn label_of(model: &SavePickerModel, row: usize) -> String {
        String::from_utf16(&model.row_label_utf16(row)).expect("row label is valid UTF-16")
    }

    fn destination(dir: &str, files: usize) -> SavePickerModel {
        destination_loading(dir, files, "Z:\\elsewhere\\ER0000.sl2")
    }

    /// A destination browse whose LOADED save is `loaded`, so the `[CURRENT]` marker has something
    /// to point at. The default `destination` deliberately loads a save that is not in the listing.
    fn destination_loading(dir: &str, files: usize, loaded: &str) -> SavePickerModel {
        model_with(
            PickerIntent::SaveDestination {
                loaded_file_name: "ER0000.sl2".to_owned(),
                loaded_path: PathBuf::from(loaded),
            },
            dir,
            files,
        )
    }

    // -----------------------------------------------------------------------------------------
    // SYNTHETIC SAVE CONTAINERS. Deterministic generators, never captured game bytes (repo rule:
    // no game-derived binaries in tree, test fixtures included). They reproduce only the fields
    // the readers under test actually parse: the BND4 header/entry index, `USER_DATA010`'s
    // active-slot bytes, and a PlayerGameData block placed where the FACE-anchored locator scans.
    // -----------------------------------------------------------------------------------------

    /// PlayerGameData field offsets the plausibility core reads (`loading_cover_save_slot.rs`).
    const PGD_HEALTH: usize = 0x08;
    const PGD_MAX_HEALTH: usize = 0x0c;
    const PGD_BASE_MAX_HEALTH: usize = 0x10;
    const PGD_STAT_BASE: usize = 0x34;
    const PGD_STAT_COUNT: usize = 8;
    const PGD_LEVEL: usize = 0x60;
    const PGD_NAME: usize = 0x94;
    const PGD_GENDER: usize = 0xb6;
    const PGD_MAX_CRIMSON: usize = 0xf9;
    const PGD_MAX_CERULEAN: usize = 0xfa;
    /// The locator finds `FACE` and scans back over `[face-0xa600, face-0xa000]`; putting the
    /// block at exactly `face - 0xa000` makes the accepted offset the only plausible one, because
    /// every other candidate reads zeros and fails the name/level checks.
    const PGD_AT: usize = 0x100;
    const FACE_AT: usize = PGD_AT + 0xa000;
    const SLOT_BODY_BYTES: usize = FACE_AT + 0x10;
    /// `USER_DATA010` body: a zero-length `CSMenuSystemSaveLoad` blob at `0x150`, so the 10
    /// active-slot bytes sit at `0x154`.
    const SYSTEM_BODY_BYTES: usize = 0x200;
    const SYSTEM_MENU_SAVE_LOAD_LEN: usize = 0x150;
    const SYSTEM_ACTIVE_SLOTS: usize = 0x154;

    /// One character slot's plaintext body carrying a locatable, plausible character.
    fn synthetic_slot_body(name: &str, level: u32) -> Vec<u8> {
        let mut body = vec![0_u8; SLOT_BODY_BYTES];
        body[FACE_AT..FACE_AT + 4].copy_from_slice(b"FACE");
        let put32 = |body: &mut Vec<u8>, at: usize, v: u32| {
            body[PGD_AT + at..PGD_AT + at + 4].copy_from_slice(&v.to_le_bytes());
        };
        put32(&mut body, PGD_HEALTH, 1000);
        put32(&mut body, PGD_MAX_HEALTH, 1000);
        put32(&mut body, PGD_BASE_MAX_HEALTH, 1000);
        for index in 0..PGD_STAT_COUNT {
            put32(&mut body, PGD_STAT_BASE + index * 4, 10);
        }
        put32(&mut body, PGD_LEVEL, level);
        body[PGD_AT + PGD_GENDER] = 0;
        body[PGD_AT + PGD_MAX_CRIMSON] = 4;
        body[PGD_AT + PGD_MAX_CERULEAN] = 3;
        for (index, unit) in name.encode_utf16().take(15).enumerate() {
            let at = PGD_AT + PGD_NAME + index * 2;
            body[at..at + 2].copy_from_slice(&unit.to_le_bytes());
        }
        body
    }

    /// A structurally complete BND4 save container. `slots[i]` is `Some((name, level))` for an
    /// ACTIVE character slot and `None` for an inactive one; a `USER_DATA010` entry always carries
    /// the resulting active-slot bytes.
    fn synthetic_save_container(slots: &[Option<(&str, u32)>]) -> Vec<u8> {
        const HEADER_LEN: usize = 0x40;
        const ENTRY_STRIDE: usize = 0x20;
        const MD5_LEN: usize = 0x10;
        let mut bodies: Vec<(String, Vec<u8>)> = slots
            .iter()
            .enumerate()
            .filter_map(|(slot, present)| {
                present.map(|(name, level)| {
                    (
                        format!("USER_DATA{slot:03}"),
                        synthetic_slot_body(name, level),
                    )
                })
            })
            .collect();
        let mut system = vec![0_u8; SYSTEM_BODY_BYTES];
        system[SYSTEM_MENU_SAVE_LOAD_LEN..SYSTEM_MENU_SAVE_LOAD_LEN + 4]
            .copy_from_slice(&0_u32.to_le_bytes());
        for (slot, present) in slots.iter().enumerate().take(PICKER_ROW_COUNT) {
            system[SYSTEM_ACTIVE_SLOTS + slot] = u8::from(present.is_some());
        }
        bodies.push(("USER_DATA010".to_owned(), system));

        let names_at = HEADER_LEN + bodies.len() * ENTRY_STRIDE;
        let name_blobs: Vec<Vec<u8>> = bodies
            .iter()
            .map(|(name, _)| {
                let mut out: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
                out.extend_from_slice(&[0, 0]);
                out
            })
            .collect();
        let data_at = names_at + name_blobs.iter().map(Vec::len).sum::<usize>();
        let total = data_at
            + bodies
                .iter()
                .map(|(_, body)| MD5_LEN + body.len())
                .sum::<usize>();
        let mut out = vec![0_u8; total];
        out[..4].copy_from_slice(b"BND4");
        out[0x0c..0x10].copy_from_slice(&(bodies.len() as i32).to_le_bytes());
        out[0x10..0x18].copy_from_slice(&(HEADER_LEN as i64).to_le_bytes());
        out[0x20..0x28].copy_from_slice(&(ENTRY_STRIDE as i64).to_le_bytes());
        let mut name_cursor = names_at;
        let mut data_cursor = data_at;
        for (index, ((_, body), name_blob)) in bodies.iter().zip(&name_blobs).enumerate() {
            let entry = HEADER_LEN + index * ENTRY_STRIDE;
            let entry_size = MD5_LEN + body.len();
            out[entry + 0x08..entry + 0x10].copy_from_slice(&(entry_size as i64).to_le_bytes());
            out[entry + 0x10..entry + 0x14].copy_from_slice(&(data_cursor as i32).to_le_bytes());
            out[entry + 0x14..entry + 0x18].copy_from_slice(&(name_cursor as i32).to_le_bytes());
            out[name_cursor..name_cursor + name_blob.len()].copy_from_slice(name_blob);
            name_cursor += name_blob.len();
            let body_at = data_cursor + MD5_LEN;
            out[body_at..body_at + body.len()].copy_from_slice(body);
            data_cursor += entry_size;
        }
        out
    }

    /// An empty temp directory of our own, so a listing test sees exactly the files it wrote.
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("er-save-picker-accepts-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
        dir
    }

    fn write_file(dir: &Path, leaf: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(leaf);
        std::fs::write(&path, bytes).expect("temp file must be writable");
        path
    }

    const SL2: &[&str] = &["sl2"];

    fn load(path: &Path) -> Result<Vec<crate::slots::SaveSlotInfo>, PickRejection> {
        save_picker_accepts(path, &PickerIntent::LoadSource, SL2)
    }

    fn dest(path: &Path) -> Result<Vec<crate::slots::SaveSlotInfo>, PickRejection> {
        save_picker_accepts(path, &dest_intent("Z:\\elsewhere\\ER0000.sl2"), SL2)
    }

    /// A destination intent loading `loaded`. The leaf is always `ER0000.sl2` because that is what
    /// `[ new ]` writes; only the folder differs between these tests.
    fn dest_intent(loaded: &str) -> PickerIntent {
        PickerIntent::SaveDestination {
            loaded_file_name: "ER0000.sl2".to_owned(),
            loaded_path: PathBuf::from(loaded),
        }
    }

    /// The generator has to actually produce a container the shipping reader accepts, or every
    /// assertion built on it is vacuous.
    #[test]
    fn the_synthetic_container_parses_as_a_real_loadable_save() {
        let bytes = synthetic_save_container(&[Some(("Tarnished", 42)), None, Some(("Second", 7))]);
        assert!(er_save_loader::bnd4::parse_entries(&bytes).is_ok());
        let chars = crate::slots::parse_save_character_slots(&bytes);
        assert_eq!(
            chars
                .iter()
                .map(|info| (info.slot, info.name.as_str(), info.level))
                .collect::<Vec<_>>(),
            vec![(0, "Tarnished", 42), (2, "Second", 7)]
        );
    }

    #[test]
    fn the_load_intent_rejects_everything_that_is_not_a_loadable_container() {
        let dir = scratch_dir("load-rejects");
        assert_eq!(load(&dir), Err(PickRejection::NotAFile));
        assert_eq!(
            load(&write_file(&dir, "notes.txt", b"not a save")),
            Err(PickRejection::WrongExtension)
        );
        assert_eq!(
            load(&write_file(&dir, "ER0000.bak", b"right name, wrong flavor")),
            Err(PickRejection::WrongExtension)
        );
        assert_eq!(
            load(&dir.join("absent.sl2")),
            Err(PickRejection::NotAFile),
            "a path that does not exist is not a load source"
        );
        let mut truncated = synthetic_save_container(&[Some(("Tarnished", 42))]);
        truncated.truncate(0x20);
        assert_eq!(
            load(&write_file(&dir, "truncated.sl2", &truncated)),
            Err(PickRejection::NotBnd4)
        );
        let slotless = synthetic_save_container(&[None, None]);
        assert_eq!(
            load(&write_file(&dir, "slotless.sl2", &slotless)),
            Err(PickRejection::NoLoadableCharacter)
        );
        let level_zero = synthetic_save_container(&[Some(("Deleted", 0))]);
        assert_eq!(
            load(&write_file(&dir, "level0.sl2", &level_zero)),
            Err(PickRejection::NoLoadableCharacter),
            "a level-0 leftover fails the autoload's own real-character fingerprint"
        );
        let real = synthetic_save_container(&[Some(("Tarnished", 42))]);
        let chars =
            load(&write_file(&dir, "real.sl2", &real)).expect("a real save is a load source");
        assert_eq!(chars.len(), 1);
        assert_eq!(chars[0].name, "Tarnished");
    }

    /// THE INTENT ASYMMETRY, pinned. A destination is an overwrite target: it needs no loadable
    /// character (hiding a slotless file would let `[ new ]` clobber it silently) and it need not
    /// exist at all (`[ new ]` and Save-As both name a file that does not). Its FOLDER must exist.
    #[test]
    fn the_destination_intent_accepts_what_the_load_intent_refuses() {
        let dir = scratch_dir("dest-accepts");
        let slotless = write_file(
            &dir,
            "slotless.sl2",
            &synthetic_save_container(&[None, None]),
        );
        assert_eq!(load(&slotless), Err(PickRejection::NoLoadableCharacter));
        assert_eq!(
            dest(&slotless),
            Ok(Vec::new()),
            "a slotless existing container is a legal overwrite target"
        );
        assert_eq!(
            dest(&dir.join("brand-new.sl2")),
            Ok(Vec::new()),
            "a leaf that does not exist yet in an existing folder is a legal destination"
        );
        assert_eq!(
            dest(&dir.join("gone").join("brand-new.sl2")),
            Err(PickRejection::ParentMissing)
        );
        assert_eq!(
            dest(&dir.join("wrong.co2")),
            Err(PickRejection::WrongExtension),
            "the flavor filter still applies to a destination"
        );
        assert_eq!(dest(&dir), Err(PickRejection::NotAFile));
    }

    /// CONTRACT 7: there is not a second notion of "valid save". Whatever the in-game listing shows
    /// is exactly what the predicate accepts, in both intents -- so the OS dialog, which calls the
    /// same predicate, cannot load a container the browser would have hidden.
    #[test]
    fn the_listing_and_the_predicate_agree_file_for_file() {
        let dir = scratch_dir("listing-agreement");
        let candidates = [
            write_file(
                &dir,
                "real.sl2",
                &synthetic_save_container(&[Some(("Ranni", 5))]),
            ),
            write_file(&dir, "slotless.sl2", &synthetic_save_container(&[None])),
            write_file(&dir, "garbage.sl2", b"not bnd4 at all"),
            write_file(&dir, "notes.txt", b"ignored"),
        ];
        for (intent, model) in [
            (
                PickerIntent::LoadSource,
                SavePickerModel::open_with_extensions(&dir, SL2),
            ),
            (
                dest_intent("Z:\\elsewhere\\ER0000.sl2"),
                SavePickerModel::open_destination(
                    &dir,
                    SL2,
                    "ER0000.sl2",
                    Path::new("Z:\\elsewhere\\ER0000.sl2"),
                ),
            ),
        ] {
            let mut listed: Vec<PathBuf> = model
                .entries
                .iter()
                .filter(|entry| matches!(entry, PickerEntry::File { .. }))
                .map(|entry| entry.path().to_path_buf())
                .collect();
            let mut accepted: Vec<PathBuf> = candidates
                .iter()
                .filter(|path| save_picker_accepts(path, &intent, SL2).is_ok())
                .cloned()
                .collect();
            listed.sort();
            accepted.sort();
            assert_eq!(
                listed, accepted,
                "the {intent:?} listing and the predicate disagree about which files are saves"
            );
        }
    }

    /// A directory that genuinely exists, so the drive-resume tests exercise the real existence
    /// filter instead of an invented path. Returns `(created_dir, its drive root)`.
    fn real_dir_and_root(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("er-save-picker-{tag}"));
        std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
        let mut root = dir.as_path();
        while let Some(parent) = root.parent() {
            if parent.as_os_str().is_empty() {
                break;
            }
            root = parent;
        }
        (dir.clone(), root.to_path_buf())
    }

    /// `[ new ]` IS PINNED ABOVE THE NAVIGATION ROWS, not below them (moved 2026-07-31, when the
    /// Save Game row press started opening this list with no confirm in front of it). Row 0 is the
    /// index a freshly built native list highlights, and the row that belongs there is the one that
    /// creates a file rather than one that replaces one.
    #[test]
    fn destination_pins_new_file_above_the_nav_rows_and_shifts_them_down() {
        let model = destination("Z:\\saves", 3);
        // No drive row here, and `Z:\saves` has a parent: `[ new ]` at 0, up at 1, entries from 2.
        assert_eq!(model.new_file_row(), Some(0));
        assert_eq!(model.parent_row(), Some(1));
        assert_eq!(model.drive_row(), None);
        assert_eq!(
            model.row_meaning(0),
            PickerRow::NewFile(PathBuf::from("Z:\\saves").join("ER0000.sl2"))
        );
        assert_eq!(model.row_meaning(1), PickerRow::ParentDir);
        for (offset, expected) in (0..3).enumerate() {
            assert_eq!(
                model.row_meaning(2 + offset),
                PickerRow::File(PathBuf::from("Z:\\saves").join(format!("save{expected}.sl2")))
            );
        }
        assert_eq!(model.row_meaning(5), PickerRow::Empty);
        assert_eq!(model.visible_row_count(), 5);
        // At a DRIVE ROOT there is no up row at all, so `[ new ]` is still 0 and entries follow it
        // directly -- the pin is not "one above the up row", it is first, full stop.
        let root = destination("Z:\\", 2);
        assert_eq!(root.new_file_row(), Some(0));
        assert_eq!(root.parent_row(), None);
        assert_eq!(
            root.row_meaning(1),
            PickerRow::File(PathBuf::from("Z:\\").join("save0.sl2"))
        );
    }

    /// REGRESSION: the per-row character info was read at `row - 1` while the row LABEL came from
    /// `entry_row_base()`, so in destination intent every row showed the character info of the file
    /// one entry further down and the pinned `[ new ]` row showed the first file's info. Pin the
    /// invariant that matters -- the text a row renders describes that row's OWN file -- across
    /// BOTH intents AND with the drive row present, which is the layout shift most likely to
    /// reintroduce it.
    #[test]
    fn row_character_info_belongs_to_that_rows_own_file() {
        for model in [
            destination("Z:\\saves", 3),
            model_with(PickerIntent::LoadSource, "Z:\\saves", 3),
            with_drives(destination("Z:\\saves", 3), &["C:\\", "Z:\\"]),
            with_drives(
                model_with(PickerIntent::LoadSource, "Z:\\saves", 3),
                &["C:\\", "Z:\\"],
            ),
            // At a drive root there is no up row, so the entry base shifts down by one.
            with_drives(
                model_with(PickerIntent::LoadSource, "Z:\\", 3),
                &["C:\\", "Z:\\"],
            ),
        ] {
            let mut file_rows = 0;
            for row in 0..PICKER_ROW_COUNT {
                match row_label_file(&model, row) {
                    Some(file) => {
                        // "save2.sl2" must render "char2", never a neighbour's character.
                        let idx = file
                            .trim_start_matches("save")
                            .trim_end_matches(".sl2")
                            .to_owned();
                        assert_eq!(
                            row_char_name(&model, row),
                            Some(format!("char{idx}")),
                            "row {row} labelled {file} rendered another file's character"
                        );
                        file_rows += 1;
                    }
                    // Every non-file row (up, drive, [ new ], page cycler, placeholder) must render
                    // no character info at all, or it shows junk borrowed from a real file.
                    None => assert_eq!(
                        row_char_name(&model, row),
                        None,
                        "non-file row {row} rendered character info"
                    ),
                }
            }
            assert_eq!(file_rows, 3, "expected all three files to occupy rows");
        }
    }

    /// Only a save-FILE row is backed by a file, so only a File row has a last-saved time to show.
    /// Enumerated per kind so the decision is pinned independently of any layout.
    #[test]
    fn only_file_rows_have_a_last_saved_time() {
        let path = PathBuf::from("Z:\\saves\\save0.sl2");
        assert!(picker_row_has_last_saved_time(&PickerRow::File(
            path.clone()
        )));
        for row in [
            PickerRow::ParentDir,
            PickerRow::DriveCycle,
            PickerRow::AtRoot,
            PickerRow::Dir(PathBuf::from("Z:\\saves\\sub")),
            PickerRow::NewFile(path),
            PickerRow::NextPage,
            PickerRow::Empty,
        ] {
            assert!(
                !picker_row_has_last_saved_time(&row),
                "{row:?} is backed by no file, so it has no last-saved time"
            );
        }
    }

    /// On a real layout the timestamp decision must agree with the row's own label and character
    /// info, across both intents and with the drive row present: exactly the file rows carry one,
    /// and each carries ITS OWN file's stamp rather than a neighbour's.
    #[test]
    fn last_saved_time_agrees_with_the_rows_own_file() {
        for model in [
            model_with(PickerIntent::LoadSource, "Z:\\saves", 3),
            destination("Z:\\saves", 3),
            with_drives(
                model_with(PickerIntent::LoadSource, "Z:\\saves", 3),
                &["C:\\", "Z:\\"],
            ),
            with_drives(destination("Z:\\", 3), &["C:\\", "Z:\\"]),
            // Long enough to need the page cycler, which carries no timestamp either.
            model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT + 4),
        ] {
            let mut dated = 0;
            for row in 0..PICKER_ROW_COUNT {
                let is_file = row_label_file(&model, row).is_some();
                assert_eq!(
                    picker_row_has_last_saved_time(&model.row_meaning(row)),
                    is_file,
                    "row {row} ({:?}) disagrees with its label about being a save file",
                    model.row_meaning(row)
                );
                assert_eq!(
                    is_file,
                    model.row_file_characters(row).is_some(),
                    "row {row} ({:?}) disagrees with its character info",
                    model.row_meaning(row)
                );
                // `model_with` stamps file `idx` at epoch + idx minutes, so the stamp a row renders
                // proves WHICH file it read -- the same own-file property the character info has.
                match (row_label_file(&model, row), model.row_last_saved(row)) {
                    (Some(file), Some(stamp)) => {
                        let idx: u64 = file
                            .trim_start_matches("save")
                            .trim_end_matches(".sl2")
                            .parse()
                            .expect("test file names carry their index");
                        assert_eq!(
                            stamp,
                            SystemTime::UNIX_EPOCH + Duration::from_secs(idx * 60),
                            "row {row} labelled {file} rendered another file's timestamp"
                        );
                        dated += 1;
                    }
                    (Some(file), None) => panic!("file row {row} ({file}) lost its timestamp"),
                    (None, Some(_)) => panic!("non-file row {row} produced a timestamp"),
                    (None, None) => {}
                }
            }
            assert!(dated > 0, "at least one file row must carry a timestamp");
        }
    }

    /// A file whose metadata the listing build could not read renders NOTHING -- never a fabricated
    /// or epoch-zero date.
    #[test]
    fn a_file_without_metadata_has_no_last_saved_time() {
        let mut model = model_with(PickerIntent::LoadSource, "Z:\\saves", 2);
        for entry in &mut model.entries {
            if let PickerEntry::File { modified, .. } = entry {
                *modified = None;
            }
        }
        let base = model.entry_row_base();
        assert!(matches!(model.row_meaning(base), PickerRow::File(_)));
        assert_eq!(model.row_last_saved(base), None);
    }

    /// Known epochs, including the leap-year cases an off-by-one in the era algorithm breaks first.
    #[test]
    fn civil_time_matches_known_epochs() {
        let cases: [(i64, (i64, u32, u32, u32, u32)); 8] = [
            (0, (1970, 1, 1, 0, 0)),
            (86_399, (1970, 1, 1, 23, 59)),
            (86_400, (1970, 1, 2, 0, 0)),
            // 2000-02-29 12:00: leap year by the 400-rule.
            (951_825_600, (2000, 2, 29, 12, 0)),
            // 2100-02-28 then 2100-03-01, one day apart: 2100 is NOT a leap year (100-rule), so a
            // date the 4-rule alone would place on 2100-02-29 must not exist.
            (4_107_456_000, (2100, 2, 28, 0, 0)),
            (4_107_542_400, (2100, 3, 1, 0, 0)),
            (1_785_312_180, (2026, 7, 29, 8, 3)),
            // Just under the signed-32-bit epoch limit, which this i64 arithmetic must not care
            // about.
            (2_147_483_640, (2038, 1, 19, 3, 14)),
        ];
        for (secs, (year, month, day, hour, minute)) in cases {
            assert_eq!(
                civil_from_unix_seconds(secs),
                Some(CivilDateTime {
                    year,
                    month,
                    day,
                    hour,
                    minute
                }),
                "epoch {secs}"
            );
        }
        assert_eq!(civil_from_unix_seconds(-1), None, "pre-epoch is not a date");
    }

    /// The rendered text, and the DST boundary the offset has to carry: US Pacific springs forward
    /// at 2026-03-08 10:00 UTC, so one minute of real time crosses from -08:00 to -07:00 and the
    /// local clock jumps 01:59 -> 03:00. Passing the two offsets that boundary switches between is
    /// exactly how the OS-supplied offset behaves, so this pins the arithmetic without a machine
    /// timezone.
    #[test]
    fn last_saved_renders_local_time_across_a_dst_boundary() {
        const PST: i64 = -8 * 3_600;
        const PDT: i64 = -7 * 3_600;
        // 2026-03-08 09:59:00 UTC, still PST.
        assert_eq!(
            format_last_saved(1_772_963_940, PST).as_deref(),
            Some("2026-03-08 01:59")
        );
        // 2026-03-08 10:00:00 UTC, one minute later, now PDT: the wall clock skips 02:00.
        assert_eq!(
            format_last_saved(1_772_964_000, PDT).as_deref(),
            Some("2026-03-08 03:00")
        );
        // The same instant in UTC and east of Greenwich, to pin the sign of the offset.
        assert_eq!(
            format_last_saved(1_772_964_000, 0).as_deref(),
            Some("2026-03-08 10:00")
        );
        assert_eq!(
            format_last_saved(1_772_964_000, 2 * 3_600).as_deref(),
            Some("2026-03-08 12:00")
        );
        // An offset that would push the instant before the epoch renders nothing.
        assert_eq!(format_last_saved(60, -3_600), None);
    }

    /// The drive cycler must be excluded from entry indexing in BOTH intents: it shifts the entry
    /// base by exactly one and never resolves to an entry itself.
    #[test]
    fn drive_row_is_excluded_from_entry_indexing_in_both_intents() {
        let load = with_drives(
            model_with(PickerIntent::LoadSource, "Z:\\saves", 2),
            &["C:\\", "Z:\\"],
        );
        assert_eq!(load.drive_row(), Some(1));
        assert_eq!(load.row_meaning(1), PickerRow::DriveCycle);
        assert_eq!(load.row_file_characters(1), None);
        assert_eq!(
            load.row_meaning(2),
            PickerRow::File(PathBuf::from("Z:\\saves").join("save0.sl2")),
            "the first entry must sit directly under the drive row"
        );

        // Destination layout: `[ new ]` is PINNED at row 0, so the up row and the cycler each
        // shift down by one and the entries start at 3.
        let dest = with_drives(destination("Z:\\saves", 2), &["C:\\", "Z:\\"]);
        assert_eq!(dest.new_file_row(), Some(0));
        assert_eq!(dest.parent_row(), Some(1));
        assert_eq!(dest.drive_row(), Some(2));
        assert_eq!(
            dest.row_meaning(0),
            PickerRow::NewFile(PathBuf::from("Z:\\saves").join("ER0000.sl2"))
        );
        assert_eq!(dest.row_meaning(1), PickerRow::ParentDir);
        assert_eq!(dest.row_meaning(2), PickerRow::DriveCycle);
        assert_eq!(dest.row_file_characters(0), None);
        assert_eq!(dest.row_file_characters(2), None);
        assert_eq!(
            dest.row_meaning(3),
            PickerRow::File(PathBuf::from("Z:\\saves").join("save0.sl2"))
        );
        // Adding the drive row costs exactly one entry row per page, in each intent, once the
        // listing is long enough for the per-page capacity to bind.
        let long = PICKER_ROW_COUNT * 2;
        assert_eq!(
            model_with(PickerIntent::LoadSource, "Z:\\saves", long).entries_per_page(),
            8
        );
        assert_eq!(
            with_drives(
                model_with(PickerIntent::LoadSource, "Z:\\saves", long),
                &["C:\\", "Z:\\"]
            )
            .entries_per_page(),
            7
        );
        assert_eq!(destination("Z:\\saves", long).entries_per_page(), 7);
        assert_eq!(
            with_drives(destination("Z:\\saves", long), &["C:\\", "Z:\\"]).entries_per_page(),
            6
        );
    }

    /// Activating the drive row must reach EVERY enumerated drive and wrap back to the start, since
    /// the native window gives us no backward direction.
    #[test]
    fn activating_the_drive_row_reaches_every_drive_and_wraps() {
        let roots = ["C:\\", "S:\\", "Z:\\"];
        let mut model = with_drives(model_with(PickerIntent::LoadSource, "C:\\", 0), &roots);
        let drive_row = model
            .drive_row()
            .expect("three drives must add a drive row");
        assert_eq!(drive_row, 0, "a drive root has no up row above the cycler");
        let mut seen = vec![model.current_dir().to_path_buf()];
        for _ in 0..roots.len() {
            assert_eq!(model.row_meaning(drive_row), PickerRow::DriveCycle);
            assert_eq!(model.activate(drive_row), PickerActivation::Repopulate);
            seen.push(model.current_dir().to_path_buf());
        }
        let expected: Vec<PathBuf> = ["C:\\", "S:\\", "Z:\\", "C:\\"]
            .iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(seen, expected, "one activation per drive, wrapping");
    }

    /// The drive row's label names the current drive AND the one activation moves to, and fits the
    /// ProfileSummary name budget with no comma in it.
    #[test]
    fn drive_row_label_names_both_drives_and_fits_the_name_budget() {
        let model = with_drives(
            model_with(PickerIntent::LoadSource, "C:\\users", 0),
            &["C:\\", "S:\\", "Z:\\"],
        );
        let row = model.drive_row().expect("drive row");
        let label = label_of(&model, row);
        assert_eq!(label, "[ C: > S: ]");
        assert!(model.row_label_utf16(row).len() <= PICKER_ROW_NAME_UTF16_MAX);
        assert!(!label.contains(','), "row labels must be comma-safe");
    }

    /// Cycling drives must RESUME the folder last browsed on the drive being returned to, instead
    /// of dumping the user at the drive root every time -- that resume is what makes the row useful
    /// for moving a save between two directories on different drives.
    #[test]
    fn cycling_drives_resumes_each_drives_remembered_folder() {
        let (real_dir, real_root) = real_dir_and_root("resume");
        // Second drive is a letter that cannot be mounted here, so "never visited" is guaranteed.
        let other_root = PathBuf::from("Q:\\");
        let mut model = with_drives(
            model_with(PickerIntent::LoadSource, "unused", 0),
            &[
                real_root.to_string_lossy().as_ref(),
                other_root.to_string_lossy().as_ref(),
            ],
        );
        model.current_dir = real_dir.clone();

        // Leaving the real drive records where we were; the unvisited drive opens at its root.
        model.cycle_drive(true);
        assert_eq!(model.current_dir(), other_root.as_path());
        assert_eq!(
            model.last_dir_per_drive.get(&real_root),
            Some(&real_dir),
            "the folder being left must be remembered against its own drive"
        );

        // Coming back RESUMES that folder instead of the drive root -- the whole point.
        model.cycle_drive(true);
        assert_eq!(model.current_dir(), real_dir.as_path());
    }

    /// A remembered folder that has since vanished must fall back to the drive root rather than
    /// browsing a dead path.
    #[test]
    fn cycling_drives_falls_back_to_the_root_when_the_remembered_folder_is_gone() {
        let (real_dir, real_root) = real_dir_and_root("vanished");
        let other_root = PathBuf::from("Q:\\");
        let mut model = with_drives(
            model_with(PickerIntent::LoadSource, "unused", 0),
            &[
                real_root.to_string_lossy().as_ref(),
                other_root.to_string_lossy().as_ref(),
            ],
        );
        model.current_dir = real_dir.clone();
        model.cycle_drive(true);
        assert_eq!(model.current_dir(), other_root.as_path());

        // The folder is remembered, but it is gone by the time we cycle back.
        std::fs::remove_dir_all(&real_dir).expect("temp dir must be removable");
        model.cycle_drive(true);
        assert_eq!(
            model.current_dir(),
            real_root.as_path(),
            "a remembered folder that no longer exists must fall back to the drive root"
        );
        // The memory is still recorded, so the fallback is about resolvability, not forgetting.
        assert_eq!(model.last_dir_per_drive.get(&real_root), Some(&real_dir));
    }

    /// With fewer than two drives there is no cycler row at all, and the cycle call is inert.
    #[test]
    fn a_single_drive_adds_no_cycler_row() {
        let mut model = with_drives(
            model_with(PickerIntent::LoadSource, "Z:\\home\\banon", 0),
            &["Z:\\"],
        );
        assert_eq!(model.drive_row(), None);
        assert_eq!(model.drive_count(), 1);
        model.cycle_drive(true);
        assert_eq!(model.current_dir(), Path::new("Z:\\home\\banon"));
        // Only the up row is fixed, so all nine remaining rows stay available to entries.
        assert_eq!(model.max_entries_single_page(), PICKER_ROW_COUNT - 1);
    }

    /// The `[..]` row names the folder it goes TO, not just the direction, and truncates rather
    /// than overflowing the record's name field.
    #[test]
    fn up_row_label_names_the_parent_folder() {
        let model = model_with(
            PickerIntent::LoadSource,
            "Z:\\home\\banon\\Roaming\\deep",
            0,
        );
        let up = model.parent_row().expect("a nested folder has an up row");
        assert_eq!(up, 0, "a load browse pins nothing above the up row");
        assert_eq!(label_of(&model, up), "[..] Roaming");
        let long = model_with(
            PickerIntent::LoadSource,
            "Z:\\a-very-long-folder-name-indeed\\child",
            0,
        );
        let long_up = long.parent_row().expect("a nested folder has an up row");
        assert!(long.row_label_utf16(long_up).len() <= PICKER_ROW_NAME_UTF16_MAX);
        // Same row, one index lower, in a destination browse: the label is derived from
        // `parent_row()` rather than a constant, so pinning `[ new ]` above it moved nothing else.
        let dest = destination("Z:\\home\\banon\\Roaming\\deep", 0);
        assert_eq!(dest.parent_row(), Some(1));
        assert_eq!(label_of(&dest, 1), "[..] Roaming");
    }

    /// Rows beyond the listing must be reported as NOT visible, so the staging layer marks their
    /// native slots unoccupied and the builder omits them -- that is what stops a short listing
    /// rendering placeholder rows with a name, `Level 0` and `0:00:00`.
    #[test]
    fn rows_beyond_the_listing_are_outside_the_visible_count() {
        // Load source, two files, up row, no drive row: up + 2 entries = 3 visible rows.
        let load = model_with(PickerIntent::LoadSource, "Z:\\saves", 2);
        assert_eq!(load.visible_row_count(), 3);
        for row in load.visible_row_count()..PICKER_ROW_COUNT {
            assert_eq!(load.row_meaning(row), PickerRow::Empty);
            assert!(load.row_label_utf16(row).is_empty());
        }
        // Destination, drive row, one file: [ new ] + up + drive + 1 entry = 4 visible rows.
        let dest = with_drives(destination("Z:\\saves", 1), &["C:\\", "Z:\\"]);
        assert_eq!(dest.visible_row_count(), 4);
        for row in dest.visible_row_count()..PICKER_ROW_COUNT {
            assert_eq!(dest.row_meaning(row), PickerRow::Empty);
            assert!(dest.row_label_utf16(row).is_empty());
        }
        // A page cycler is INSIDE the visible count -- it must never be dropped as a placeholder.
        let paged = model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT + 4);
        let pager = paged.next_page_row().expect("overflowing listing pages");
        assert!(pager < paged.visible_row_count());
        assert_eq!(paged.row_meaning(pager), PickerRow::NextPage);
        assert_eq!(paged.visible_row_count(), PICKER_ROW_COUNT);
    }

    /// Every visible row must carry a non-empty label: the native staging marks visible slots
    /// occupied, and an occupied slot with an empty name would fail the empty-slot activation
    /// guard. Checked across the layouts that move the row boundaries.
    #[test]
    fn every_visible_row_has_a_non_empty_label() {
        for model in [
            model_with(PickerIntent::LoadSource, "Z:\\saves", 0),
            model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT + 4),
            with_drives(destination("Z:\\", 0), &["C:\\", "Z:\\"]),
            with_drives(destination("Z:\\saves", 9), &["C:\\", "S:\\", "Z:\\"]),
            model_with(PickerIntent::LoadSource, "Z:\\", 0),
        ] {
            for row in 0..model.visible_row_count() {
                assert!(
                    !model.row_label_utf16(row).is_empty(),
                    "visible row {row} has an empty label in {:?} (visible={})",
                    model.current_dir(),
                    model.visible_row_count()
                );
                assert!(model.row_label_utf16(row).len() <= PICKER_ROW_NAME_UTF16_MAX);
            }
        }
    }

    /// A listing that fits uses every remaining row; one entry more spends a row on the cycler.
    #[test]
    fn the_page_cycler_appears_only_when_the_listing_overflows() {
        let fits = model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT - 1);
        assert_eq!(fits.page_count(), 1);
        assert_eq!(fits.next_page_row(), None);
        assert_eq!(fits.entries_per_page(), PICKER_ROW_COUNT - 1);

        let overflows = model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT);
        assert_eq!(overflows.entries_per_page(), PICKER_ROW_COUNT - 2);
        assert_eq!(overflows.page_count(), 2);
        assert_eq!(overflows.next_page_row(), Some(PICKER_ROW_COUNT - 1));
        // Second page holds the remainder, and the cycler moves up to sit under the last entry.
        let mut overflows = overflows;
        overflows.cycle_page(true);
        assert_eq!(
            overflows.row_meaning(1),
            PickerRow::File(PathBuf::from("Z:\\saves").join("save8.sl2"))
        );
        assert_eq!(overflows.next_page_row(), Some(3));
        assert_eq!(overflows.row_meaning(3), PickerRow::NextPage);
        assert_eq!(overflows.visible_row_count(), 4);
    }

    #[test]
    fn load_source_layout_is_unaffected_by_the_destination_intent() {
        let model = model_with(PickerIntent::LoadSource, "Z:\\saves", 8);
        assert_eq!(model.new_file_row(), None);
        assert_eq!(model.page_count(), 1);
        assert_eq!(
            model.row_meaning(1),
            PickerRow::File(PathBuf::from("Z:\\saves").join("save0.sl2"))
        );
        assert_eq!(
            model.row_meaning(8),
            PickerRow::File(PathBuf::from("Z:\\saves").join("save7.sl2"))
        );
        assert_eq!(model.row_meaning(9), PickerRow::Empty);
    }

    /// THE DESTINATION CURSOR STARTS ON `[ new ]`, IN EVERY LAYOUT. Since the Save Game row press
    /// opens this browser with no question in front of it, the row the cursor rests on is the
    /// answer a user gets for pressing confirm twice without reading -- so it must be the row that
    /// creates rather than the row that replaces. Checked with entries present and absent, with and
    /// without a drive row, and at a drive root where there is no up row at all.
    #[test]
    fn a_destination_browse_always_starts_the_cursor_on_new_file() {
        for model in [
            destination("Z:\\saves", 0),
            destination("Z:\\saves", 3),
            with_drives(destination("Z:\\saves", 0), &["C:\\", "Z:\\"]),
            with_drives(destination("Z:\\saves", 5), &["C:\\", "S:\\", "Z:\\"]),
            with_drives(destination("Z:\\", 2), &["C:\\", "Z:\\"]),
            destination("Z:\\", 0),
        ] {
            let mut model = model;
            model.cursor = model.first_selectable_row();
            assert_eq!(
                model.new_file_row(),
                Some(0),
                "`[ new ]` is pinned first in {:?}",
                model.current_dir()
            );
            assert_eq!(
                model.cursor,
                0,
                "the destination cursor must start on `[ new ]` in {:?}",
                model.current_dir()
            );
            let expected = model.current_dir().join("ER0000.sl2");
            assert_eq!(
                model.activate_cursor(),
                PickerActivation::PickedNewFile(expected)
            );
        }
    }

    #[test]
    fn direct_cursor_set_accepts_only_selectable_rows() {
        let mut model = model_with(PickerIntent::LoadSource, "Z:\\saves", 2);
        let first_entry = model.entry_row_base();
        model.set_cursor(first_entry + 1);
        assert_eq!(model.cursor(), first_entry + 1);
        model.set_cursor(PICKER_ROW_COUNT - 1);
        assert_eq!(
            model.cursor(),
            first_entry + 1,
            "setting the cursor to an Empty row must leave the current highlight alone"
        );
    }

    /// On an EMPTY drive the initial cursor must still land on a real, selectable row.
    #[test]
    fn first_selectable_row_is_sane_on_an_empty_drive() {
        // Empty drive root WITH somewhere else to go: the cycler is the only row, and the cursor
        // must land on it so the user is not stranded.
        let multi = with_drives(
            model_with(PickerIntent::LoadSource, "Z:\\", 0),
            &["C:\\", "Z:\\"],
        );
        assert_eq!(multi.visible_row_count(), 1);
        assert_eq!(multi.first_selectable_row(), 0);
        assert_eq!(multi.row_meaning(0), PickerRow::DriveCycle);

        // Empty drive root with nowhere to go: one `[ root ]` row rather than a zero-row native
        // list, and the cursor lands on it.
        let alone = with_drives(model_with(PickerIntent::LoadSource, "Z:\\", 0), &["Z:\\"]);
        assert_eq!(alone.visible_row_count(), 1);
        assert_eq!(alone.first_selectable_row(), 0);
        assert_eq!(alone.row_meaning(0), PickerRow::AtRoot);
        assert_eq!(label_of(&alone, 0), "[ root ]");

        // Empty SUBdirectory: nothing actionable below the nav rows, so fall back to the up row.
        let empty_dir = with_drives(
            model_with(PickerIntent::LoadSource, "Z:\\saves", 0),
            &["C:\\", "Z:\\"],
        );
        assert_eq!(empty_dir.visible_row_count(), 2);
        assert_eq!(empty_dir.first_selectable_row(), 0);
        assert_eq!(empty_dir.row_meaning(0), PickerRow::ParentDir);
    }

    #[test]
    fn new_file_row_label_fits_the_profile_summary_name_budget() {
        let model = destination("Z:\\saves", 0);
        let row = model
            .new_file_row()
            .expect("destination pins a [ new ] row");
        let label = model.row_label_utf16(row);
        assert!(!label.is_empty() && label.len() <= PICKER_ROW_NAME_UTF16_MAX);
        assert_eq!(String::from_utf16(&label).unwrap(), PICKER_NEW_FILE_LABEL);
    }

    /// EXACTLY ONE ROW IS `[CURRENT]`, and it is the row whose file the user is playing. With the
    /// up-front "Overwrite your loaded save?" box gone, finding that row IS the overwrite-my-own-
    /// save flow, so a marker on the wrong row (or on none) sends the user to the wrong file.
    #[test]
    fn only_the_loaded_saves_row_is_marked_current() {
        let dir = "Z:\\saves";
        let model = destination_loading(dir, 4, "Z:\\saves\\save2.sl2");
        let marked: Vec<usize> = (0..PICKER_ROW_COUNT)
            .filter(|&row| model.row_is_loaded_save(row))
            .collect();
        let base = model.entry_row_base();
        assert_eq!(
            marked,
            vec![base + 2],
            "only save2.sl2's row may be marked (entries start at row {base})"
        );
        assert_eq!(
            row_label_file(&model, base + 2).as_deref(),
            Some("save2.sl2"),
            "the marked row must be the one whose LABEL names the loaded file"
        );
    }

    /// The marker is a Windows path compare, so it must survive case differences -- `ER0000.SL2`
    /// and `er0000.sl2` are one file there, and a case-sensitive compare would leave a user's own
    /// save unmarked in the list they are being asked to find it in.
    #[test]
    fn the_current_marker_ignores_path_case() {
        let model = destination_loading("Z:\\saves", 2, "z:\\SAVES\\SAVE1.SL2");
        let row = model.entry_row_base() + 1;
        assert_eq!(row_label_file(&model, row).as_deref(), Some("save1.sl2"));
        assert!(model.row_is_loaded_save(row));
    }

    /// NOTHING is marked when the loaded save is not in the browsed folder, and NOTHING is ever
    /// marked in a load browse -- there is no "current" there, and a marker would be a claim the
    /// model cannot support.
    #[test]
    fn no_row_is_marked_current_without_a_matching_loaded_save() {
        for model in [
            destination("Z:\\saves", 4),
            model_with(PickerIntent::LoadSource, "Z:\\saves", 4),
        ] {
            for row in 0..PICKER_ROW_COUNT {
                assert!(
                    !model.row_is_loaded_save(row),
                    "row {row} was marked current in {:?}",
                    model.intent
                );
            }
        }
    }

    /// The marker never lands on a non-file row: `[ new ]` resolves to the loaded save's own leaf,
    /// and in the loaded save's own folder that path IS the loaded save -- but `[ new ]` is an
    /// ACTION row, not the file's row, and marking it would put `[CURRENT]` on two rows at once.
    #[test]
    fn the_new_file_row_is_never_marked_current_even_when_it_targets_the_loaded_save() {
        let model = destination_loading("Z:\\saves", 2, "Z:\\saves\\ER0000.sl2");
        let new_row = model.new_file_row().expect("destination pins [ new ]");
        assert_eq!(
            model.row_meaning(new_row),
            PickerRow::NewFile(PathBuf::from("Z:\\saves").join("ER0000.sl2")),
            "the row targets exactly the loaded save's path"
        );
        assert!(
            !model.row_is_loaded_save(new_row),
            "`[ new ]` is an action row; only a File row may be marked"
        );
    }

    #[test]
    fn rejection_reasons_have_distinct_visible_copy() {
        assert_eq!(
            PickRejection::NotBnd4.status_message("SL2").headline(),
            "NOT AN ELDEN RING SAVE"
        );
        assert_eq!(
            PickRejection::NoLoadableCharacter
                .status_message("SL2")
                .headline(),
            "NO LOADABLE CHARACTER"
        );
        let wrong_type = PickRejection::WrongExtension.status_message("CO2/.SL2");
        assert_eq!(wrong_type.headline(), "WRONG FILE TYPE");
        assert!(
            wrong_type.detail().contains(".CO2/.SL2"),
            "the visible reason must name the accepted extension set"
        );
    }

    #[test]
    fn picker_status_survives_rejection_and_clears_on_navigation() {
        let mut model = model_with(PickerIntent::LoadSource, "Z:\\saves", 2);
        model.set_status_message(PickerStatusMessage::new(
            "WRONG SAVE SIZE",
            "Expected a full container.",
        ));
        assert_eq!(
            model.status_message().map(PickerStatusMessage::headline),
            Some("WRONG SAVE SIZE")
        );

        assert_eq!(model.activate(0), PickerActivation::Repopulate);
        assert!(
            model.status_message().is_none(),
            "moving to another folder must not carry a stale rejection"
        );
    }

    #[test]
    fn picker_status_clears_on_direct_page_drive_and_up_navigation() {
        let mut paged = model_with(PickerIntent::LoadSource, "Z:\\saves", PICKER_ROW_COUNT);
        paged.set_status_message(PickerStatusMessage::new(
            "NO LOADABLE CHARACTER",
            "Pick another save.",
        ));
        paged.cycle_page(true);
        assert!(
            paged.status_message().is_none(),
            "direct page cycling must not carry a stale rejection"
        );

        let (real_dir, real_root) = real_dir_and_root("status-clear-drive");
        let other_root = PathBuf::from("Q:\\");
        let mut drive = with_drives(
            model_with(PickerIntent::LoadSource, "unused", 0),
            &[
                real_root.to_string_lossy().as_ref(),
                other_root.to_string_lossy().as_ref(),
            ],
        );
        drive.current_dir = real_dir.clone();
        drive.set_status_message(PickerStatusMessage::new(
            "WRONG FILE TYPE",
            "Pick another file.",
        ));
        drive.cycle_drive(true);
        assert!(
            drive.status_message().is_none(),
            "direct drive cycling must not carry a stale rejection"
        );

        let child = real_dir.join("child");
        std::fs::create_dir_all(&child).expect("temp child dir must be creatable");
        let mut up = model_with(
            PickerIntent::LoadSource,
            child.to_string_lossy().as_ref(),
            0,
        );
        up.set_status_message(PickerStatusMessage::new(
            "UNREADABLE SAVE",
            "Pick another file.",
        ));
        up.go_up();
        assert!(
            up.status_message().is_none(),
            "direct up navigation must not carry a stale rejection"
        );
    }
}

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
