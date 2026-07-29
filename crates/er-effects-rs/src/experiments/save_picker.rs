//! Shared in-game save-file picker model.
//!
//! Pure filesystem/pagination state for the two in-game file-picker menus (the startup
//! missing-save picker and the System>Quit "Load Save Profiles" picker). Both menus render
//! through the native `05_010_ProfileSelect` 10-row window, so this model maps a browsable
//! directory listing onto row pages. The UI layers own all native staging (ProfileSummary preview
//! records, window submit/close); this module owns what the rows MEAN.
//!
//! Extension filtering follows the active runtime flavor: vanilla offers `.sl2`; Seamless offers
//! both `.co2` and vanilla `.sl2` sources so users can import/load a vanilla save while ERSC owns
//! the session.
//!
//! The same model serves two INTENTS. [`PickerIntent::LoadSource`] browses for
//! a save to LOAD (the shipping behavior). [`PickerIntent::SaveDestination`] browses for a folder to
//! SAVE INTO: a pinned `[ new ]` row writes the loaded save's own filename into the browsed folder,
//! and occupancy filtering is dropped -- an overwrite target needs no active character slot, and
//! hiding a slotless existing file would let `[ new ]` clobber it silently.
//!
//! ## The row layout is DENSE, and every index is derived
//!
//! The visible rows are a contiguous prefix of the window's 10 slots, in this order:
//!
//! 1. `[..] <parent>` -- only when the current directory has a parent (absent at a drive root);
//! 2. `[ C: > Z: ]`   -- the drive cycler, only when more than one drive is mounted;
//! 3. `[ new ]`       -- destination intent only;
//! 4. the current page's directory / save-file entries;
//! 5. `[ page N/M ]`  -- only when the listing overflows one page.
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

use crate::telemetry::append_autoload_debug;

/// Rows per `05_010_ProfileSelect` window (native slot count).
pub(crate) const PICKER_ROW_COUNT: usize = 10;
/// Row index of the "up one directory" row. It is always first WHEN IT EXISTS; at a drive root
/// there is no up row and index 0 belongs to whichever row comes next in the layout.
pub(crate) const PICKER_ROW_PARENT: usize = 0;
/// ProfileSummary name field capacity: 16 UTF-16 units + NUL (0x22 bytes).
pub(crate) const PICKER_ROW_NAME_UTF16_MAX: usize = 16;
/// Label of the destination-intent `[ new ]` row (7 UTF-16 units, inside the name budget).
pub(crate) const PICKER_NEW_FILE_LABEL: &str = "[ new ]";

/// What the browsing session is FOR. Fixed at construction; it selects the row layout, the
/// occupancy filter, and what activating a row means.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum PickerIntent {
    /// Browse for a save container to LOAD (startup missing-save picker, System>Quit load picker).
    #[default]
    LoadSource,
    /// Browse for a folder to SAVE INTO. `loaded_file_name` is the leaf the `[ new ]` row writes
    /// (always the loaded save's own filename, so the destination keeps its save flavor).
    SaveDestination { loaded_file_name: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PickerEntry {
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
        chars: Vec<crate::experiments::SaveSlotInfo>,
    },
}

impl PickerEntry {
    pub(crate) fn name(&self) -> &str {
        match self {
            PickerEntry::Dir { name, .. } | PickerEntry::File { name, .. } => name,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        match self {
            PickerEntry::Dir { path, .. } | PickerEntry::File { path, .. } => path,
        }
    }
}

/// What a row on the CURRENT page means. Produced by [`SavePickerModel::row_meaning`]; the UI
/// layer stages row text from this and routes slot activation through it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PickerRow {
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

/// Outcome of activating a row. `Repopulate` means the listing changed (new directory, new drive or
/// new page) and the UI must re-stage row records and re-present the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PickerActivation {
    PickedFile(PathBuf),
    /// Destination intent only: the `[ new ]` row resolved to this target path in the browsed
    /// directory. The UI layer decides whether it already exists (overwrite confirm) or not.
    PickedNewFile(PathBuf),
    Repopulate,
    Ignored,
}

#[derive(Debug, Default)]
pub(crate) struct SavePickerModel {
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

/// Save-file rows are only useful if the selected container can offer at least one LOADABLE
/// character slot, and the browse rows display per-file character info -- so parse the active
/// character slots (slot/name/level; `USER_DATA010.active_slot` occupancy + PlayerGameData locate,
/// the same `parse_save_character_slots` pass the character sub-picker runs on pick) in ONE read of
/// the file bytes at listing-build time and cache the result in the entry. Files with no loadable
/// character (no active slot, or only empty-like leftovers the autoload's real-character
/// fingerprint would reject anyway) are hidden; these multi-MB containers are read exactly once
/// per listing build, never per frame.
fn save_file_character_slots(path: &Path) -> Option<Vec<crate::experiments::SaveSlotInfo>> {
    let Ok(bytes) = std::fs::read(path) else {
        append_autoload_debug(format_args!(
            "save-picker: hiding '{}' -- failed to read save while parsing character slots",
            path.display()
        ));
        return None;
    };
    let chars = crate::experiments::parse_save_character_slots(&bytes);
    if chars.is_empty() {
        append_autoload_debug(format_args!(
            "save-picker: hiding '{}' -- save has no loadable character slots",
            path.display()
        ));
        return None;
    }
    Some(chars)
}

impl SavePickerModel {
    /// Build a model rooted at `dir`, listing subdirectories plus `*.{extension}` files.
    pub(crate) fn open(dir: &Path, extension: &str) -> Self {
        Self::open_with_extensions(dir, &[extension])
    }

    /// Build a model rooted at `dir`, listing subdirectories plus files whose extension matches any
    /// entry in `extensions`.
    pub(crate) fn open_with_extensions(dir: &Path, extensions: &[&str]) -> Self {
        Self::open_with_intent(dir, extensions, PickerIntent::LoadSource)
    }

    /// Build a save-DESTINATION browser rooted at `dir`. `loaded_file_name` is the leaf the
    /// `[ new ]` row writes into the browsed folder.
    ///
    /// NO MENU OPENS ONE YET -- the save flow that browses for a destination is a separate piece of
    /// work. The intent is modelled here regardless, because [`Self::entry_row_base`] is ONE layout
    /// decision serving both intents: excising the destination case would leave two row-index
    /// arithmetics to keep in agreement, and disagreement between them is precisely the defect
    /// `row_character_info_belongs_to_that_rows_own_file` exists to pin.
    pub(crate) fn open_destination(
        dir: &Path,
        extensions: &[&str],
        loaded_file_name: &str,
    ) -> Self {
        Self::open_with_intent(
            dir,
            extensions,
            PickerIntent::SaveDestination {
                loaded_file_name: loaded_file_name.to_owned(),
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
            drives: enumerate_drives(),
            last_dir_per_drive: HashMap::new(),
            intent,
        };
        model.refresh();
        model.cursor = model.first_selectable_row();
        model
    }

    /// True when this browser is choosing a save DESTINATION rather than a load source.
    pub(crate) fn is_destination(&self) -> bool {
        matches!(self.intent, PickerIntent::SaveDestination { .. })
    }

    // ---------------------------------------------------------------------------------------
    // ROW LAYOUT. `entry_row_base` is the single decision point; every other index derives from
    // it, so a layout change cannot desynchronize labels, character text and activation routing.
    // ---------------------------------------------------------------------------------------

    /// True when the current directory has a parent to navigate to (false at a drive root).
    fn has_parent_row(&self) -> bool {
        self.current_dir
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
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

    /// Row index of the first directory/file entry.
    fn entry_row_base(&self) -> usize {
        self.nav_row_count() + usize::from(self.is_destination())
    }

    /// Row index of the drive cycler, when it exists.
    pub(crate) fn drive_row(&self) -> Option<usize> {
        self.has_drive_row()
            .then(|| usize::from(self.has_parent_row()))
    }

    /// Row index of the pinned `[ new ]` row (destination intent only).
    pub(crate) fn new_file_row(&self) -> Option<usize> {
        self.is_destination().then(|| self.nav_row_count())
    }

    /// Row index of the page cycler, when the listing overflows one page. It sits immediately
    /// after the CURRENT page's last entry, so a short final page has no gap above it -- and the
    /// native cursor re-clamp lands the highlight back on the cycler after paging.
    pub(crate) fn next_page_row(&self) -> Option<usize> {
        (self.page_count() > 1).then(|| self.entry_row_base() + self.page_entries().len())
    }

    /// Rows the window actually shows. Slots at or beyond this are staged UNOCCUPIED so the native
    /// list builder omits them (no name, no level, no playtime).
    pub(crate) fn visible_row_count(&self) -> usize {
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
    pub(crate) fn entries_per_page(&self) -> usize {
        let single = self.max_entries_single_page();
        if self.entries.len() <= single {
            single.max(1)
        } else {
            single.saturating_sub(1).max(1)
        }
    }

    /// Destination target for the `[ new ]` row: the loaded save's own filename in the browsed
    /// directory. `None` outside destination intent.
    fn new_file_target(&self) -> Option<PathBuf> {
        match &self.intent {
            PickerIntent::SaveDestination { loaded_file_name } => {
                Some(self.current_dir.join(loaded_file_name))
            }
            PickerIntent::LoadSource => None,
        }
    }

    /// Header line: the current directory path.
    pub(crate) fn location_label(&self) -> String {
        self.current_dir.display().to_string()
    }

    /// The drive root of `current_dir` (walk up to the ancestor with no parent), e.g. `Z:\` for
    /// `Z:\home\banon`.
    fn current_drive_root(&self) -> PathBuf {
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
    pub(crate) fn drive_count(&self) -> usize {
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
    pub(crate) fn cycle_drive(&mut self, forward: bool) {
        let Some(root) = self.neighbour_drive(forward) else {
            return;
        };
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
    pub(crate) fn cursor_on_drive_selector(&self) -> bool {
        self.drive_row() == Some(self.cursor)
    }

    pub(crate) fn current_dir(&self) -> &Path {
        &self.current_dir
    }

    pub(crate) fn extension(&self) -> &str {
        &self.extension
    }

    pub(crate) fn page(&self) -> usize {
        self.page
    }

    pub(crate) fn page_count(&self) -> usize {
        self.entries.len().div_ceil(self.entries_per_page()).max(1)
    }

    pub(crate) fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Re-enumerate `current_dir`. Unreadable directories yield an empty listing rather than an
    /// error: the picker stays navigable (the user can still go up or change drive) and the debug
    /// log records the failure.
    pub(crate) fn refresh(&mut self) {
        self.entries.clear();
        self.page = 0;
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
            } else if path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        self.extensions
                            .iter()
                            .any(|allowed| ext.eq_ignore_ascii_case(allowed))
                    })
                // Occupancy filtering is a LOAD-source concern only. A destination row is an
                // overwrite target, so it needs no active character slot -- hiding a slotless or
                // unreadable existing file would let `[ new ]` silently clobber it. Destination
                // listings still run the same single-read parse so their file rows can show who
                // lives in the file, but a container the parse rejects stays LISTED, with no
                // characters, instead of disappearing.
                && let Some(chars) = (if self.is_destination() {
                    Some(save_file_character_slots(&path).unwrap_or_default())
                } else {
                    save_file_character_slots(&path)
                })
            {
                files.push(PickerEntry::File {
                    name: name.to_owned(),
                    path: path.clone(),
                    modified: entry.metadata().ok().and_then(|meta| meta.modified().ok()),
                    chars,
                });
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
    pub(crate) fn row_meaning(&self, row: usize) -> PickerRow {
        if row >= PICKER_ROW_COUNT {
            return PickerRow::Empty;
        }
        if self.has_parent_row() && row == PICKER_ROW_PARENT {
            return PickerRow::ParentDir;
        }
        if self.drive_row() == Some(row) {
            return PickerRow::DriveCycle;
        }
        if self.new_file_row() == Some(row) {
            return self
                .new_file_target()
                .map_or(PickerRow::Empty, PickerRow::NewFile);
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
    pub(crate) fn row_file_characters(
        &self,
        row: usize,
    ) -> Option<&[crate::experiments::SaveSlotInfo]> {
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

    /// Apply the effect of activating `row`.
    pub(crate) fn activate(&mut self, row: usize) -> PickerActivation {
        match self.row_meaning(row) {
            PickerRow::ParentDir => {
                if let Some(parent) = self.current_dir.parent().map(Path::to_path_buf)
                    && !parent.as_os_str().is_empty()
                {
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
        let parent = self.current_dir.parent()?;
        if parent.as_os_str().is_empty() {
            return None;
        }
        Some(match parent.file_name().and_then(|name| name.to_str()) {
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
    pub(crate) fn row_label_utf16(&self, row: usize) -> Vec<u16> {
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
    pub(crate) fn row_label_ascii(&self, row: usize) -> String {
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
        // Prefer the first row AFTER the pure-navigation rows so a fresh listing lands on something
        // actionable -- an entry, or the pinned `[ new ]` row in an empty destination folder --
        // rather than on `[..] up` or the drive cycler. Fall back to any selectable row (a folder
        // with nothing in it), else 0.
        let nav = self.nav_row_count();
        (nav..PICKER_ROW_COUNT)
            .find(|&r| self.row_selectable(r))
            .or_else(|| (0..PICKER_ROW_COUNT).find(|&r| self.row_selectable(r)))
            .unwrap_or(0)
    }

    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    /// Move the highlight one selectable row up (`down=false`) or down, wrapping. No-op when only
    /// one row is selectable.
    pub(crate) fn move_cursor(&mut self, down: bool) {
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
    pub(crate) fn activate_cursor(&mut self) -> PickerActivation {
        let result = self.activate(self.cursor);
        if matches!(result, PickerActivation::Repopulate) {
            self.cursor = self.first_selectable_row();
        }
        result
    }

    /// Move to the previous/next page (wrapping), resetting the cursor. No-op when single-page.
    pub(crate) fn cycle_page(&mut self, forward: bool) {
        let count = self.page_count();
        if count < 2 {
            return;
        }
        self.page = if forward {
            (self.page + 1) % count
        } else {
            (self.page + count - 1) % count
        };
        self.cursor = self.first_selectable_row();
    }

    /// Navigate to the parent directory (no-op at a drive root -- switch drives with the drive
    /// cycler row instead). Resets the cursor.
    pub(crate) fn go_up(&mut self) {
        if let Some(parent) = self.current_dir.parent().map(Path::to_path_buf)
            && !parent.as_os_str().is_empty()
        {
            self.current_dir = parent;
            self.refresh();
            self.cursor = self.first_selectable_row();
        }
    }

    /// Long-form status line for the auxiliary text fields (full current dir + page info).
    pub(crate) fn status_line(&self) -> String {
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
pub(crate) fn truncate_utf16(text: &str, max: usize) -> Vec<u16> {
    text.encode_utf16().take(max).collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Build a model with a fixed listing, bypassing the filesystem enumeration `open` does.
    ///
    /// Each file carries ONE character whose name encodes the file's own index (`char{idx}`), so a
    /// test can assert that the character info a row renders belongs to that row's OWN file rather
    /// than a neighbour's -- the exact confusion `row_file_characters` had.
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
                    chars: vec![crate::experiments::SaveSlotInfo {
                        slot: 0,
                        name: format!("char{idx}"),
                        level: 10 + idx as i32,
                    }],
                })
                .collect(),
            page: 0,
            cursor: 0,
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
        model_with(
            PickerIntent::SaveDestination {
                loaded_file_name: "ER0000.sl2".to_owned(),
            },
            dir,
            files,
        )
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

    #[test]
    fn destination_pins_new_file_under_the_nav_rows_and_shifts_entries_down() {
        let model = destination("Z:\\saves", 3);
        // No drive row here, and `Z:\saves` has a parent: up at 0, `[ new ]` at 1, entries from 2.
        assert_eq!(model.new_file_row(), Some(1));
        assert_eq!(model.drive_row(), None);
        assert_eq!(
            model.row_meaning(1),
            PickerRow::NewFile(PathBuf::from("Z:\\saves").join("ER0000.sl2"))
        );
        for (offset, expected) in (0..3).enumerate() {
            assert_eq!(
                model.row_meaning(2 + offset),
                PickerRow::File(PathBuf::from("Z:\\saves").join(format!("save{expected}.sl2")))
            );
        }
        assert_eq!(model.row_meaning(5), PickerRow::Empty);
        assert_eq!(model.visible_row_count(), 5);
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

        let dest = with_drives(destination("Z:\\saves", 2), &["C:\\", "Z:\\"]);
        assert_eq!(dest.drive_row(), Some(1));
        assert_eq!(dest.new_file_row(), Some(2));
        assert_eq!(dest.row_meaning(1), PickerRow::DriveCycle);
        assert_eq!(dest.row_file_characters(1), None);
        assert_eq!(
            dest.row_meaning(2),
            PickerRow::NewFile(PathBuf::from("Z:\\saves").join("ER0000.sl2"))
        );
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
        assert_eq!(label_of(&model, PICKER_ROW_PARENT), "[..] Roaming");
        let long = model_with(
            PickerIntent::LoadSource,
            "Z:\\a-very-long-folder-name-indeed\\child",
            0,
        );
        assert!(long.row_label_utf16(PICKER_ROW_PARENT).len() <= PICKER_ROW_NAME_UTF16_MAX);
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
        // Destination, drive row, one file: up + drive + [ new ] + 1 entry = 4 visible rows.
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

    #[test]
    fn empty_destination_folder_still_lands_the_cursor_on_new_file() {
        for (model, expected_row) in [
            (destination("Z:\\saves", 0), 1),
            // With the drive row present the `[ new ]` row moves down one, and the cursor must
            // follow it rather than sticking to a hard-coded index.
            (
                with_drives(destination("Z:\\saves", 0), &["C:\\", "Z:\\"]),
                2,
            ),
        ] {
            let mut model = model;
            model.cursor = model.first_selectable_row();
            assert_eq!(model.cursor, expected_row);
            assert_eq!(model.new_file_row(), Some(expected_row));
            assert_eq!(
                model.activate_cursor(),
                PickerActivation::PickedNewFile(PathBuf::from("Z:\\saves").join("ER0000.sl2"))
            );
        }
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
}

/// The active picker instance, shared between the open path (menu action) and the activation
/// hook. `None` when no in-game picker is open. Sites: System>Quit picker and the startup
/// missing-save picker (mutually exclusive by construction -- the startup picker resolves
/// before the System menu is reachable).
pub(crate) static ACTIVE_SAVE_PICKER: Mutex<Option<SavePickerModel>> = Mutex::new(None);

/// Lock helper that recovers from poisoning (same pattern as `state_or_return`).
pub(crate) fn active_save_picker_lock() -> std::sync::MutexGuard<'static, Option<SavePickerModel>> {
    ACTIVE_SAVE_PICKER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
