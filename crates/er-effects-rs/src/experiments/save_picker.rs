//! Shared in-game save-file picker model.
//!
//! Pure filesystem/pagination state for the two in-game file-picker menus (the startup
//! missing-save picker and the System>Quit "Load Save Profiles" picker). Both menus render
//! through the native `05_010_ProfileSelect` 10-row window, so this model maps a browsable
//! directory listing onto fixed-size row pages: row 0 navigates up, rows 1..=8 are directory /
//! save-file entries, row 9 cycles pages when the listing overflows. The UI layers own all
//! native staging (ProfileSummary preview records, window submit/close); this module owns what
//! the rows MEAN.
//!
//! Extension filtering follows the active runtime flavor: vanilla offers `.sl2`; Seamless offers
//! both `.co2` and vanilla `.sl2` sources so users can import/load a vanilla save while ERSC owns
//! the session.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};

use crate::telemetry::append_autoload_debug;

/// Rows per `05_010_ProfileSelect` window (native slot count).
pub(crate) const PICKER_ROW_COUNT: usize = 10;
/// Row index reserved for "up one directory" navigation.
pub(crate) const PICKER_ROW_PARENT: usize = 0;
/// Row index reserved for "next page" cycling when the listing overflows one page.
pub(crate) const PICKER_ROW_NEXT_PAGE: usize = PICKER_ROW_COUNT - 1;
/// Directory/file entries shown per page (rows 1..=8).
pub(crate) const PICKER_ENTRIES_PER_PAGE: usize = PICKER_ROW_COUNT - 2;
/// ProfileSummary name field capacity: 16 UTF-16 units + NUL (0x22 bytes).
pub(crate) const PICKER_ROW_NAME_UTF16_MAX: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PickerEntry {
    /// A subdirectory of the current directory.
    Dir { name: String, path: PathBuf },
    /// A save container matching the active extension filter(s).
    File {
        name: String,
        path: PathBuf,
        modified: Option<SystemTime>,
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
    /// The current directory has no parent (drive root); row is a no-op placeholder.
    AtRoot,
    /// Open this subdirectory.
    Dir(PathBuf),
    /// Pick this save file.
    File(PathBuf),
    /// Advance to the next page (wraps to the first page after the last).
    NextPage,
    /// Row beyond the listing on this page; activation is a no-op.
    Empty,
}

/// Outcome of activating a row. `Repopulate` means the listing changed (new directory or new
/// page) and the UI must re-stage row records and re-present the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PickerActivation {
    PickedFile(PathBuf),
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
    /// Mounted drives that browse as folders (cached at open); the top row cycles through these
    /// with left/right.
    drives: Vec<PathBuf>,
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

/// Save-file rows are only useful if the selected container can offer at least one ACTIVE
/// character slot. Deleted/inactive slots can leave stale `USER_DATA00N` character bodies behind;
/// the authoritative occupancy source is `USER_DATA010.active_slot`, so filter by that before the
/// file ever appears in either custom load menu.
fn save_file_has_active_slots(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        append_autoload_debug(format_args!(
            "save-picker: hiding '{}' -- failed to read save while checking active slots",
            path.display()
        ));
        return false;
    };
    match er_save_loader::bnd4::has_active_slot(&bytes) {
        Ok(true) => true,
        Ok(false) => {
            append_autoload_debug(format_args!(
                "save-picker: hiding '{}' -- save has no active character slots",
                path.display()
            ));
            false
        }
        Err(err) => {
            append_autoload_debug(format_args!(
                "save-picker: hiding '{}' -- active-slot bitmap unreadable ({err:?})",
                path.display()
            ));
            false
        }
    }
}

impl SavePickerModel {
    /// Build a model rooted at `dir`, listing subdirectories plus `*.{extension}` files.
    pub(crate) fn open(dir: &Path, extension: &str) -> Self {
        Self::open_with_extensions(dir, &[extension])
    }

    /// Build a model rooted at `dir`, listing subdirectories plus files whose extension matches any
    /// entry in `extensions`.
    pub(crate) fn open_with_extensions(dir: &Path, extensions: &[&str]) -> Self {
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
        };
        model.refresh();
        model.cursor = model.first_selectable_row();
        model
    }

    /// Header line: the current directory path.
    pub(crate) fn location_label(&self) -> String {
        self.current_dir.display().to_string()
    }

    /// The drive root of `current_dir` (walk up to the ancestor with no parent), e.g. `Z:\` for
    /// `Z:\home\banon`. Used by the top-row drive cycler.
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

    /// Switch to the previous/next mounted drive's root (wrapping), keeping the cursor on the
    /// top drive-selector row so it can be cycled repeatedly. No-op with fewer than two drives.
    pub(crate) fn cycle_drive(&mut self, forward: bool) {
        if self.drives.len() < 2 {
            return;
        }
        let cur = self.current_drive_root();
        let idx = self.drives.iter().position(|d| d == &cur).unwrap_or(0);
        let n = self.drives.len();
        let next = if forward {
            (idx + 1) % n
        } else {
            (idx + n - 1) % n
        };
        self.current_dir = self.drives[next].clone();
        self.refresh();
        self.cursor = PICKER_ROW_PARENT;
    }

    /// True when the highlighted row is the top drive-selector row (so left/right cycle drives
    /// instead of pages).
    pub(crate) fn cursor_on_drive_selector(&self) -> bool {
        self.cursor == PICKER_ROW_PARENT
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
        self.entries.len().div_ceil(PICKER_ENTRIES_PER_PAGE).max(1)
    }

    pub(crate) fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Re-enumerate `current_dir`. Unreadable directories yield an empty listing rather than an
    /// error: the picker stays navigable (the user can still go up) and the debug log records
    /// the failure.
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
                && save_file_has_active_slots(&path)
            {
                files.push(PickerEntry::File {
                    name: name.to_owned(),
                    path: path.clone(),
                    modified: entry.metadata().ok().and_then(|meta| meta.modified().ok()),
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
        let start = self.page * PICKER_ENTRIES_PER_PAGE;
        let end = (start + PICKER_ENTRIES_PER_PAGE).min(self.entries.len());
        self.entries.get(start..end).unwrap_or(&[])
    }

    /// Meaning of `row` (0..PICKER_ROW_COUNT) on the current page.
    pub(crate) fn row_meaning(&self, row: usize) -> PickerRow {
        match row {
            PICKER_ROW_PARENT => match self.current_dir.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => PickerRow::ParentDir,
                // At a drive root: activating goes nowhere (up). Left/right on this row cycles
                // drives (handled by the overlay), so this is not a dead end.
                _ => PickerRow::AtRoot,
            },
            PICKER_ROW_NEXT_PAGE if self.page_count() > 1 => PickerRow::NextPage,
            PICKER_ROW_NEXT_PAGE => PickerRow::Empty,
            row => match self.page_entries().get(row - 1) {
                Some(PickerEntry::Dir { path, .. }) => PickerRow::Dir(path.clone()),
                Some(PickerEntry::File { path, .. }) => PickerRow::File(path.clone()),
                None => PickerRow::Empty,
            },
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
            PickerRow::Dir(path) => {
                self.current_dir = path;
                self.refresh();
                PickerActivation::Repopulate
            }
            PickerRow::File(path) => PickerActivation::PickedFile(path),
            PickerRow::NextPage => {
                self.page = (self.page + 1) % self.page_count();
                PickerActivation::Repopulate
            }
            PickerRow::AtRoot | PickerRow::Empty => PickerActivation::Ignored,
        }
    }

    /// Display label for `row`, truncated to the ProfileSummary name budget (16 UTF-16 units).
    /// Directory rows carry a `/` suffix; control rows use bracketed labels. Every non-empty row
    /// label is guaranteed non-empty so staged records pass the empty-slot activation guard.
    pub(crate) fn row_label_utf16(&self, row: usize) -> Vec<u16> {
        let label = match self.row_meaning(row) {
            PickerRow::ParentDir => "[ .. up ]".to_owned(),
            PickerRow::AtRoot => "[ root ]".to_owned(),
            PickerRow::Dir(path) => self.dir_display_name(&path),
            PickerRow::File(path) => path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("?")
                .to_owned(),
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
    /// suffix, control rows are bracketed). Empty string for an out-of-range row.
    pub(crate) fn row_label_ascii(&self, row: usize) -> String {
        let label = match self.row_meaning(row) {
            // The top row is the inline drive selector: left/right cycle the mounted drive.
            // In a subdirectory it also acts as "up" on select; at a drive root it is drive-only.
            PickerRow::ParentDir => format!(
                "[..] UP    DRIVE < {} >",
                self.current_drive_root().display()
            ),
            PickerRow::AtRoot => format!("DRIVE < {} >", self.current_drive_root().display()),
            PickerRow::Dir(path) => self.dir_display_name(&path),
            PickerRow::File(path) => path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("?")
                .to_owned(),
            PickerRow::NextPage => format!("[PAGE {}/{}]", self.page + 1, self.page_count()),
            PickerRow::Empty => String::new(),
        };
        label.to_ascii_uppercase()
    }

    /// True if `row` can be highlighted/activated (not an empty filler; the drive skips these).
    fn row_selectable(&self, row: usize) -> bool {
        !matches!(self.row_meaning(row), PickerRow::Empty)
    }

    fn first_selectable_row(&self) -> usize {
        // Prefer the first ENTRY row (1) over the parent nav row so a fresh listing lands on a
        // file/dir; fall back to any selectable row, else 0.
        (1..PICKER_ROW_COUNT)
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

    /// Activate the highlighted row. On a listing change (dir/page) the cursor resets to the first
    /// selectable row so the highlight never lands on a stale index.
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

    /// Navigate to the parent directory (no-op at a drive root -- switch drives with the top-row
    /// left/right selector instead). Resets the cursor.
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
