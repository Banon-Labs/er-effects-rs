//! The System>Quit **Load Build from URL** row.
//!
//! The importer itself lives in `er-build-import-runtime`, which the standalone
//! `er-build-import` shell also drives. This module is only the row's two halves inside the
//! product DLL: the press, which hands the runtime a URL, and the per-frame tick, which is the game
//! thread the runtime needs in order to touch anything.
//!
//! # Why the press cannot just do the import
//!
//! The press arrives on whichever thread dispatched the menu activation, inside a native
//! `PropertyNewButtonController` action. The import mutates the inventory, the `CSGaitemImp`
//! singleton, `PlayerGameData` and the equipment slots, and it begins with a blocking HTTPS get.
//! Neither belongs there. So the press only calls `request`, which spawns the fetch worker and
//! returns; the recurring `FrameBegin` task then applies the parsed build once the game can take it
//! (params streamed, character in the world). That split is the runtime's whole shape, and this row
//! reuses it rather than inventing a second one.
//!
//! # Where the URL comes from
//!
//! The row opens a link field ([`build_url_editor`]) pre-filled from the clipboard, and imports
//! what the player accepts. `build_url` in the game-directory `er-quickload.toml` is still read -- it
//! seeds nothing here, but the boot importer uses it, and an accepted link is written back to it so
//! the next session starts from the last build that worked.

use super::*;

/// What a row press did. Every variant is reported to the debug log and counted, because "the row
/// did nothing" and "the row started an import" look identical on screen until the character
/// changes.
#[derive(Clone, Debug)]
pub(crate) enum BuildUrlPress {
    /// The link field was requested; nothing is imported until the player accepts a valid link.
    EditorOpening,
    /// A fetch was started for this URL.
    Started(String),
    /// The runtime refused: an import is already in flight, or the link carries no `?b=<id>`.
    Refused(String),
}

impl BuildUrlPress {
    fn label(&self) -> String {
        match self {
            BuildUrlPress::EditorOpening => "OPENING the link field".to_owned(),
            BuildUrlPress::Started(url) => format!("STARTED url={url:?}"),
            BuildUrlPress::Refused(reason) => format!("REFUSED ({reason})"),
        }
    }
}

/// Handle a confirmed press of the Load Build from URL row: open the link field.
///
/// The press itself imports nothing. It latches a request for the field, which the menu pump
/// submits and the player then accepts or backs out of; only an accepted, validated link becomes an
/// import. That is the whole point of the row -- pressing it must never apply a build the player
/// has not just looked at and confirmed.
pub(crate) fn system_quit_start_build_import(dialog: usize) -> BuildUrlPress {
    SYSTEM_QUIT_LOAD_BUILD_URL_ACTION_COUNT.fetch_add(1, Ordering::SeqCst);
    // Drain first, because `request` clears the runtime's error slot as it claims the machine. The
    // per-frame drain in `system_quit_build_import_tick` has almost certainly already taken it, but
    // "almost certainly" is how a failure the player asked about goes missing -- and draining here
    // also guarantees the line lands above this press in the log, attributed to the right request.
    drain_build_import_failure();
    set_build_url_row_help(er_build_import_core::BUILD_URL_ROW_HELP);
    if request_build_url_editor(dialog) {
        BuildUrlPress::EditorOpening
    } else {
        SYSTEM_QUIT_LOAD_BUILD_URL_REFUSED_COUNT.fetch_add(1, Ordering::SeqCst);
        BuildUrlPress::Refused("the link field could not be opened".to_owned())
    }
}

/// Hand a validated link to the importer. Called only by the link field, after
/// `er_build_import_core::validate_build_url` has accepted it.
pub(crate) fn system_quit_start_build_import_url(url: &str) -> BuildUrlPress {
    match er_build_import_runtime::request(url) {
        Ok(()) => {
            SYSTEM_QUIT_LOAD_BUILD_URL_REQUEST_COUNT.fetch_add(1, Ordering::SeqCst);
            BuildUrlPress::Started(url.to_owned())
        }
        Err(err) => {
            SYSTEM_QUIT_LOAD_BUILD_URL_REFUSED_COUNT.fetch_add(1, Ordering::SeqCst);
            BuildUrlPress::Refused(err.to_string())
        }
    }
}

/// Write an accepted link back to the game-directory `er-quickload.toml`.
///
/// Best effort, and deliberately quiet on failure: the import has already been requested by the
/// time this runs, so a read-only game directory must not turn a working import into an error the
/// player sees. Only links that validated reach here, so the file never gains a key the boot
/// importer would then refuse.
pub(crate) fn persist_build_url(url: &str) {
    let Some(path) = er_game_base::log::game_directory_path()
        .map(|dir| dir.join(er_build_import_runtime::CONFIG_FILE_NAME))
    else {
        return;
    };
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let key = er_build_import_core::BUILD_URL_KEY;
    let assignment = format!("{key} = '{url}'");
    let mut lines: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in existing.lines() {
        let is_key = line
            .split_once('=')
            .is_some_and(|(name, _)| name.trim() == key && !line.trim_start().starts_with('#'));
        if is_key && !replaced {
            lines.push(assignment.clone());
            replaced = true;
        } else {
            lines.push(line.to_owned());
        }
    }
    if !replaced {
        lines.push(assignment.clone());
    }
    let mut out = lines.join("\n");
    out.push('\n');
    match std::fs::write(&path, out) {
        Ok(()) => append_autoload_debug(format_args!(
            "system-quit-build-url: remembered {url:?} as `{key}` in {}",
            path.display()
        )),
        Err(err) => append_autoload_debug(format_args!(
            "system-quit-build-url: could not remember the link in {}: {err}; the import still ran",
            path.display()
        )),
    }
}

/// Log and return a press outcome in one step, so both routing hooks report it identically.
pub(crate) fn system_quit_log_build_import_press(site: &str, press: &BuildUrlPress) {
    append_autoload_debug(format_args!(
        "system-quit-build-url: row press at {site} -> {}; the import itself runs on the FrameBegin task once the character can take it",
        press.label()
    ));
}

/// Report the runtime's last ASYNCHRONOUS failure, once, if there is one pending.
///
/// A press returns `Ok` the instant the worker is spawned, so a 404, an unparseable payload or a
/// build whose level and attributes disagree all fail long afterwards -- and land only in
/// `er-build-import-core.log`. A reader following the row press through
/// `er-quickload-autoload-debug.log` would otherwise see "STARTED" and never learn it went nowhere.
fn drain_build_import_failure() {
    let Some(reason) = er_build_import_runtime::take_error() else {
        return;
    };
    SYSTEM_QUIT_LOAD_BUILD_URL_FAILED_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-build-url: the queued import FAILED and applied nothing -- {reason}"
    ));
}

/// One frame of the build importer, driven from the product's recurring `FrameBegin` task.
///
/// Does nothing at all until a press has queued a build, which is why it is safe to call every
/// frame from boot: `er_build_import_runtime::tick` returns immediately unless its phase is `Ready`.
///
/// It also drains the runtime's async failures -- see [`drain_build_import_failure`].
///
/// # Safety
///
/// Game task thread only -- the context every mutation inside the runtime requires.
pub(crate) unsafe fn system_quit_build_import_tick() {
    drain_build_import_failure();
    // Safety: the caller's contract (FrameBegin game task) carries through.
    let Some(report) = (unsafe { er_build_import_runtime::tick() }) else {
        return;
    };
    SYSTEM_QUIT_LOAD_BUILD_URL_IMPORTED_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-build-url: import applied to the live character -- build={:?} {}; every count here is a READ-BACK from game memory, not a call that returned",
        report.build_name,
        report.summary()
    ));
}
