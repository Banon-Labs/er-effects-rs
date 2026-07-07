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
//! Extension filtering is mode-locked via [`crate::telemetry::expected_save_extension`]
//! (`.co2` when Seamless Co-op is resident, else `.sl2`; user directive 2026-07-06 -- never
//! offer the flavor the active runtime cannot load).

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
    /// A save container matching the active extension filter.
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
    /// Extension filter (no dot), e.g. `sl2`; locked at open time.
    extension: String,
    /// Dirs first (name order), then files (most recently modified first).
    entries: Vec<PickerEntry>,
    page: usize,
}

impl SavePickerModel {
    /// Build a model rooted at `dir`, listing subdirectories plus `*.{extension}` files.
    pub(crate) fn open(dir: &Path, extension: &str) -> Self {
        let mut model = SavePickerModel {
            current_dir: dir.to_path_buf(),
            extension: extension.to_ascii_lowercase(),
            entries: Vec::new(),
            page: 0,
        };
        model.refresh();
        model
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
        for entry in read.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                dirs.push(PickerEntry::Dir {
                    name: name.to_owned(),
                    path: path.clone(),
                });
            } else if file_type.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case(&self.extension))
            {
                files.push(PickerEntry::File {
                    name: name.to_owned(),
                    path: path.clone(),
                    modified: entry.metadata().ok().and_then(|meta| meta.modified().ok()),
                });
            }
        }
        dirs.sort_by(|a, b| a.name().to_ascii_lowercase().cmp(&b.name().to_ascii_lowercase()));
        files.sort_by(|a, b| {
            let (PickerEntry::File { modified: ma, .. }, PickerEntry::File { modified: mb, .. }) =
                (a, b)
            else {
                return std::cmp::Ordering::Equal;
            };
            mb.cmp(ma)
                .then_with(|| a.name().to_ascii_lowercase().cmp(&b.name().to_ascii_lowercase()))
        });
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
            PickerRow::Dir(path) => {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("?");
                format!("{name}/")
            }
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
pub(crate) fn active_save_picker_lock()
-> std::sync::MutexGuard<'static, Option<SavePickerModel>> {
    ACTIVE_SAVE_PICKER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
