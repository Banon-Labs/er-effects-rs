use super::*;
use er_gfx::TWIPS_PER_PIXEL_F32;
use er_gfx::profile_05_010_protocol::{
    CONTROL_FILE_NAME, ProfileEditorCommand, ProfileEditorStatus, RenderMode, STATUS_FILE_NAME,
    SelectedKind,
};
use er_telemetry::counters::{PROFILE_EDITOR_DEFERRED_APPLIES, PROFILE_SELECT_WINDOW_RUN_TICKS};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

static PROFILE_EDITOR_LAST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
/// Latest ActivePathEditor command waiting for the owned 02_990 MenuWindow::Run context. Unlike a
/// row field command, this must never queue a ProfileSelect list rebuild: the editor is a separate
/// MenuWindow with a separate root proxy.
static PATH_EDITOR_PENDING_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static PROFILE_EDITOR_STATUS_THROTTLE: AtomicU64 = AtomicU64::new(0);
/// Separate from [`PROFILE_EDITOR_STATUS_THROTTLE`] on purpose: that one paces the
/// first-few-then-powers-of-two "no command" writes, and a heartbeat sharing it would make both
/// cadences depend on how often the other fired.
static PROFILE_EDITOR_HEARTBEAT_TICKS: AtomicU64 = AtomicU64::new(0);
/// `PROFILE_SELECT_WINDOW_RUN_TICKS` as of the previous necromancy poll. The DELTA is the signal:
/// an absolute value cannot distinguish "the view is up" from "the view was up an hour ago".
static PROFILE_EDITOR_LAST_SEEN_WINDOW_RUNS: AtomicU64 = AtomicU64::new(0);
static PROFILE_EDITOR_NECROMANCY_POLL_TICKS: AtomicU64 = AtomicU64::new(0);
static PROFILE_EDITOR_FIELD_TARGETS: OnceLock<Mutex<Vec<CachedProfileFieldTarget>>> =
    OnceLock::new();
/// Last editor schema observed on the row-populate thread. Drive-row cursor geometry uses it so a
/// live width/height/button edit moves the native animated cursor with the matching drive button.
static PROFILE_EDITOR_LAST_LAYOUT: OnceLock<
    Mutex<er_gfx::profile_05_010_layout::Profile05_010Layout>,
> = OnceLock::new();

/// Font size currently authored for a synthetic ProfileSelect field. Text payload builders use the
/// same editor schema as the GFX box, so changing drive/path font size is no longer a dead control.
pub(crate) fn profile_editor_field_font_height(field_name: &str) -> i32 {
    PROFILE_EDITOR_LAST_LAYOUT
        .get()
        .and_then(|layout| layout.lock().ok())
        .map(|layout| layout.field(field_name).font_height)
        .unwrap_or_else(|| {
            er_gfx::profile_05_010_layout::Profile05_010Layout::default()
                .field(field_name)
                .font_height
        })
        .clamp(1, 80)
}

#[derive(Clone)]
struct CachedProfileFieldTarget {
    field_name: String,
    component: usize,
    last_utf16: Vec<u16>,
    active_surface: &'static str,
}

fn editor_dir() -> Option<PathBuf> {
    std::env::var_os("ER_PROFILE_05_010_EDITOR_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The exact bytes last written to `status.txt`, so the heartbeat can refresh the file's mtime
/// without inventing a status or losing the last command's `applied_count`/error detail.
static PROFILE_EDITOR_LAST_STATUS_TEXT: OnceLock<Mutex<String>> = OnceLock::new();

fn write_status(dir: &PathBuf, status: ProfileEditorStatus) {
    let text = status.serialize();
    if let Ok(mut last) = PROFILE_EDITOR_LAST_STATUS_TEXT
        .get_or_init(|| Mutex::new(String::new()))
        .lock()
    {
        last.clear();
        last.push_str(&text);
    }
    write_status_text(dir, &text);
}

fn write_status_text(dir: &PathBuf, text: &str) {
    let _ = std::fs::create_dir_all(dir);
    let tmp = dir.join(format!("{STATUS_FILE_NAME}.tmp"));
    let final_path = dir.join(STATUS_FILE_NAME);
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(tmp, final_path);
    }
}

/// Re-stamp `status.txt` so its mtime says THIS process is still reading the control file.
///
/// The status file used to be written only when a new command arrived, so the last status of a dead
/// game sat on disk saying `connected = true` forever and the editor badge believed it. Two saves
/// were made against that ghost before anyone doubted the badge (2026-08-07: control sequence 62,
/// ack frozen at 57, game relaunched an hour earlier WITHOUT `ER_PROFILE_05_010_EDITOR_DIR`, so
/// nothing in the process was reading the directory at all). Liveness has to be something the live
/// process keeps SAYING, not something a file once claimed.
fn heartbeat_status(dir: &PathBuf) {
    let ticks = PROFILE_EDITOR_HEARTBEAT_TICKS.fetch_add(1, Ordering::SeqCst) + 1;
    if !ticks.is_multiple_of(2) {
        return;
    }
    let cached = PROFILE_EDITOR_LAST_STATUS_TEXT
        .get()
        .and_then(|last| last.lock().ok())
        .map(|last| last.clone())
        .filter(|text| !text.is_empty());
    match cached {
        Some(text) => write_status_text(dir, &text),
        None => write_status(
            dir,
            ProfileEditorStatus {
                version: er_gfx::profile_05_010_protocol::PROTOCOL_VERSION,
                ack_sequence: PROFILE_EDITOR_LAST_SEQUENCE.load(Ordering::SeqCst),
                connected: true,
                status: "live-runtime-idle".to_owned(),
                active_surface: "heartbeat".to_owned(),
                selected_kind: String::new(),
                selected_name: String::new(),
                applied_count: 0,
                unsupported_count: 0,
                error: String::new(),
            },
        ),
    }
}

fn read_command(dir: &Path) -> Result<Option<ProfileEditorCommand>, String> {
    let path = dir.join(CONTROL_FILE_NAME);
    if !path.exists() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    ProfileEditorCommand::parse(&text)
        .map(Some)
        .map_err(|e| format!("parse {}: {e}", path.display()))
}

fn defer_path_editor_command(dir: &PathBuf, command: &ProfileEditorCommand) {
    PATH_EDITOR_PENDING_SEQUENCE.store(command.sequence, Ordering::SeqCst);
    write_status(
        dir,
        ProfileEditorStatus {
            version: er_gfx::profile_05_010_protocol::PROTOCOL_VERSION,
            ack_sequence: PROFILE_EDITOR_LAST_SEQUENCE.load(Ordering::SeqCst),
            connected: true,
            status: "live-runtime-command-deferred".to_owned(),
            active_surface: "02_990-menu-window-run".to_owned(),
            selected_kind: command.selected_kind.as_str().to_owned(),
            selected_name: command.selected_name.clone(),
            applied_count: 0,
            unsupported_count: 0,
            error: format!(
                "queued sequence {} for the active 02_990 MenuWindow root; no ProfileSelect row rebuild was requested",
                command.sequence
            ),
        },
    );
}

fn status_for(
    command: &ProfileEditorCommand,
    active_surface: &str,
    applied_count: u32,
    unsupported_count: u32,
    error: impl Into<String>,
) -> ProfileEditorStatus {
    ProfileEditorStatus {
        version: er_gfx::profile_05_010_protocol::PROTOCOL_VERSION,
        ack_sequence: command.sequence,
        connected: true,
        status: if unsupported_count == 0 {
            "live-runtime-command-accepted".to_owned()
        } else {
            "live-runtime-command-partial".to_owned()
        },
        active_surface: active_surface.to_owned(),
        selected_kind: command.selected_kind.as_str().to_owned(),
        selected_name: command.selected_name.clone(),
        applied_count,
        unsupported_count,
        error: error.into(),
    }
}

pub(crate) fn remember_profile_editor_field_target(
    field_name_nul: &str,
    component: usize,
    utf16: &[u16],
    active_surface: &'static str,
) {
    if editor_dir().is_none() || component == 0 || utf16.len() <= 1 {
        return;
    }
    let field_name = field_name_nul
        .strip_suffix('\0')
        .unwrap_or(field_name_nul)
        .to_owned();
    if !er_gfx::profile_05_010_layout::FIELD_NAMES.contains(&field_name.as_str()) {
        return;
    }
    let targets = PROFILE_EDITOR_FIELD_TARGETS.get_or_init(|| Mutex::new(Vec::new()));
    let Ok(mut guard) = targets.lock() else {
        return;
    };
    if let Some(target) = guard
        .iter_mut()
        .find(|target| target.field_name == field_name)
    {
        target.component = component;
        target.last_utf16.clear();
        target.last_utf16.extend_from_slice(utf16);
        target.active_surface = active_surface;
        return;
    }
    guard.push(CachedProfileFieldTarget {
        field_name,
        component,
        last_utf16: utf16.to_vec(),
        active_surface,
    });
}

/// Drop every cached live component belonging to `active_surface`, because that surface is being
/// torn down and its GFx objects are about to stop existing.
///
/// Without this the cache was append-only: nothing removed an entry on movie teardown or screen
/// close, while `profile_editor_necromancy_tick` runs off `FrameBegin` in every game state. A save
/// made after backing out of Load Game would revalidate the stale pointer with a bounds check only
/// -- which proves a vtable lands inside the game image, not that the object is alive -- then call
/// through it and `write_unaligned` an f32 into what it believes is a text document.
pub(crate) fn forget_profile_editor_field_targets(active_surface: &str) {
    let Some(targets) = PROFILE_EDITOR_FIELD_TARGETS.get() else {
        return;
    };
    let Ok(mut guard) = targets.lock() else {
        return;
    };
    let before = guard.len();
    guard.retain(|target| target.active_surface != active_surface);
    let dropped = before - guard.len();
    if dropped > 0 {
        append_autoload_debug(format_args!(
            "profile-editor: dropped {dropped} cached live field target(s) for surface '{active_surface}' at teardown; live edits now report no target instead of writing through a dead component"
        ));
    }
}

fn cached_profile_editor_field_utf16(field_name: &str) -> Option<Vec<u16>> {
    PROFILE_EDITOR_FIELD_TARGETS
        .get()
        .and_then(|targets| targets.lock().ok())
        .and_then(|targets| {
            targets
                .iter()
                .find(|target| target.field_name == field_name)
                .map(|target| target.last_utf16.clone())
        })
}

fn live_player_name_utf16() -> Option<Vec<u16>> {
    let name = crate::experiments::startup_hooks::loading_cover::build_loaded_char_name()?;
    Some(name.encode_utf16().chain(core::iter::once(0)).collect())
}

fn utf16_status_preview(utf16: &[u16]) -> String {
    let body = utf16.strip_suffix(&[0]).unwrap_or(utf16);
    String::from_utf16(body)
        .unwrap_or_else(|_| "<invalid-utf16>".to_owned())
        .chars()
        .take(80)
        .collect()
}

/// Poll the live editor away from row-populate call stacks and re-animate the last known field
/// component directly. The row-populate parent `SceneObjProxy` is a short-lived stack proxy, so using
/// it after native teardown is a UAF trap. The component object returned by the field component's
/// native GetValue path is the stable body we cache; every tick revalidates its vtable before using it.
pub(crate) unsafe fn profile_editor_necromancy_tick(_base: usize) {
    let Some(dir) = editor_dir() else {
        return;
    };
    let tick = PROFILE_EDITOR_NECROMANCY_POLL_TICKS.fetch_add(1, Ordering::SeqCst) + 1;
    if !tick.is_multiple_of(15) {
        return;
    }
    let command = match read_command(&dir) {
        Ok(Some(command)) => command,
        Ok(None) => {
            heartbeat_status(&dir);
            return;
        }
        Err(error) => {
            write_status(
                &dir,
                ProfileEditorStatus {
                    version: er_gfx::profile_05_010_protocol::PROTOCOL_VERSION,
                    ack_sequence: PROFILE_EDITOR_LAST_SEQUENCE.load(Ordering::SeqCst),
                    connected: true,
                    status: "live-runtime-command-error".to_owned(),
                    active_surface: "necromancy".to_owned(),
                    selected_kind: String::new(),
                    selected_name: String::new(),
                    applied_count: 0,
                    unsupported_count: 0,
                    error,
                },
            );
            return;
        }
    };
    if command.render_mode != RenderMode::LiveRuntime {
        heartbeat_status(&dir);
        return;
    }
    if PROFILE_EDITOR_LAST_SEQUENCE.load(Ordering::SeqCst) == command.sequence {
        heartbeat_status(&dir);
        return;
    }
    // Data-only handoff: unlike cached GFx component pointers, copying the schema under a mutex is
    // safe from FrameBegin. The live 02_990 MenuWindow reads this on its own Run and follows a
    // dragged CurrentPath immediately; the underlying ProfileSelect row still waits for its owned
    // row-populate rebuild below.
    if let Ok(mut layout) = PROFILE_EDITOR_LAST_LAYOUT
        .get_or_init(|| Mutex::new(command.layout.clone()))
        .lock()
    {
        *layout = command.layout.clone();
    }
    if command.selected_kind == SelectedKind::PathEditor {
        defer_path_editor_command(&dir, &command);
        return;
    }
    // THIS PATH NO LONGER TOUCHES GFX OBJECTS AT ALL. It queues, and the row populate applies.
    //
    // It ran off `FrameBegin` and wrote through component pointers cached on earlier frames. That is
    // unsound in BOTH directions, and the user found both within half an hour on 2026-08-07:
    //
    //   * view ON SCREEN (16:39:16) -- writing to components the menu thread is simultaneously
    //     laying out. Died in `FUN_14075dc30`.
    //   * view GONE (17:03:32) -- writing to components whose owner has been destructed. Died in
    //     `_purecall` -> `purecall_crash_handler`, a deliberate write of 0xdead to NULL. The
    //     liveness check could not see it coming, because a destructed object's vtable slots hold
    //     `_purecall`, which IS inside the game image.
    //
    // Gating the first case was treating a symptom; the second one crashed a user who had done
    // exactly what the guard asked of them. There is no "safe moment" for a pointer we cached and do
    // not own. The command is left UN-ACKED and applied by `profile_editor_runtime_tick`, which runs
    // INSIDE the native row populate -- the one place the game hands us a proxy it is holding alive
    // on that very frame. Cost: an edit lands on the next populate (scroll the list or reopen Load
    // Character) rather than instantly. That is the price of not killing the process.
    //
    // `PROFILE_SELECT_WINDOW_RUN_TICKS` is kept, no longer as a gate but as the thing that tells the
    // user which nudge will make their edit appear: rising means the view is up, so a scroll applies
    // it now; flat means reopen the view.
    let window_runs = PROFILE_SELECT_WINDOW_RUN_TICKS.load(Ordering::SeqCst);
    let previous_runs =
        PROFILE_EDITOR_LAST_SEEN_WINDOW_RUNS.swap(window_runs as u64, Ordering::SeqCst);
    let view_on_screen = window_runs as u64 > previous_runs;
    let dialog = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    let picker_rebuild_queued = view_on_screen
        && dialog != 0
        && SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0
        && SAVE_PICKER_REBUILD_PENDING_DIALOG
            .compare_exchange(0, dialog, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
    let queued = PROFILE_EDITOR_DEFERRED_APPLIES.fetch_add(1, Ordering::SeqCst) + 1;
    let how = if picker_rebuild_queued {
        "the picker is on screen: its menu-pump-owned native row rebuild is queued automatically"
    } else if view_on_screen {
        "the profile view is on screen outside the picker: scroll the list one row and it appears"
    } else {
        "the profile view is closed: reopen Load Character and it appears"
    };
    // NOT `status_for`: that reports "accepted" whenever nothing was unsupported, and a queued
    // command is neither accepted nor failed. `ack_sequence` stays at the last APPLIED sequence so
    // the editor keeps showing the edit as outstanding.
    write_status(
        &dir,
        ProfileEditorStatus {
            version: er_gfx::profile_05_010_protocol::PROTOCOL_VERSION,
            ack_sequence: PROFILE_EDITOR_LAST_SEQUENCE.load(Ordering::SeqCst),
            connected: true,
            status: "live-runtime-command-deferred".to_owned(),
            active_surface: if view_on_screen {
                "queued-view-on-screen".to_owned()
            } else {
                "queued-view-closed".to_owned()
            },
            selected_kind: command.selected_kind.as_str().to_owned(),
            selected_name: command.selected_name.clone(),
            applied_count: 0,
            unsupported_count: 0,
            error: format!(
                "queued sequence {}: edits are applied by the game's own row populate, never from the frame thread (writing to cached components crashed the game in both screen states). {how}. Nothing was lost (queued={queued}).",
                command.sequence
            ),
        },
    );
}

/// Poll the editor control file from the trusted ProfileSelect row-populate hook and acknowledge
/// what the live runtime can currently do. This is intentionally inert unless
/// `ER_PROFILE_05_010_EDITOR_DIR` points at a Windows-visible editor directory.
///
/// The exact live visual integration point is here: the row proxy is alive, the native named-child
/// binder is known-good, and `push_stats_text_on_row` already proves field proxies can be resolved
/// safely from this stack frame. The remaining hard part is the display-transform setter: no stable
/// `CSScaleformValue::SetDisplayInfo` wrapper has been proven in this repo yet, so transform writes
/// fail closed instead of guessing a vtable slot and turning the game into modern art.
pub(crate) unsafe fn profile_editor_runtime_tick(
    base: usize,
    row_proxy: usize,
    row_model: usize,
    native_slot: i32,
    active_surface: &'static str,
) {
    let Some(dir) = editor_dir() else {
        return;
    };
    let command = match read_command(&dir) {
        Ok(Some(command)) => command,
        Ok(None) => {
            let count = PROFILE_EDITOR_STATUS_THROTTLE.fetch_add(1, Ordering::SeqCst) + 1;
            if count <= 2 || count.is_power_of_two() {
                write_status(&dir, ProfileEditorStatus::disconnected());
            }
            return;
        }
        Err(error) => {
            let status = ProfileEditorStatus {
                version: er_gfx::profile_05_010_protocol::PROTOCOL_VERSION,
                ack_sequence: PROFILE_EDITOR_LAST_SEQUENCE.load(Ordering::SeqCst),
                connected: true,
                status: "live-runtime-command-error".to_owned(),
                active_surface: active_surface.to_owned(),
                selected_kind: String::new(),
                selected_name: String::new(),
                applied_count: 0,
                unsupported_count: 0,
                error,
            };
            write_status(&dir, status);
            return;
        }
    };
    if command.render_mode != RenderMode::LiveRuntime {
        write_status(
            &dir,
            status_for(
                &command,
                active_surface,
                0,
                0,
                "offline command observed by runtime",
            ),
        );
        return;
    }
    if let Ok(mut layout) = PROFILE_EDITOR_LAST_LAYOUT
        .get_or_init(|| Mutex::new(command.layout.clone()))
        .lock()
    {
        *layout = command.layout.clone();
    }
    if command.selected_kind == SelectedKind::PathEditor {
        defer_path_editor_command(&dir, &command);
        return;
    }
    // Drive-button/path commands are meaningful on exactly one picker-owned row. Ordinary rows
    // populate first and used to ACK the sequence with applied_count=0 plus a "deferred" detail;
    // the browser then claimed success while the visible drive highlight kept its previous shape.
    // Leave the sequence outstanding until the actual drive row owns the setter-valid proxies.
    let live_drive_cell_count = usize::try_from(native_slot)
        .ok()
        .and_then(super::save_picker_row_slot_info)
        .map(|info| info.drive_cell_count)
        .unwrap_or(0);
    if command_targets_drive_row(&command) && live_drive_cell_count == 0 {
        if PROFILE_EDITOR_LAST_SEQUENCE.load(Ordering::SeqCst) != command.sequence {
            write_status(
                &dir,
                ProfileEditorStatus {
                    version: er_gfx::profile_05_010_protocol::PROTOCOL_VERSION,
                    ack_sequence: PROFILE_EDITOR_LAST_SEQUENCE.load(Ordering::SeqCst),
                    connected: true,
                    status: "live-runtime-command-deferred".to_owned(),
                    active_surface: active_surface.to_owned(),
                    selected_kind: command.selected_kind.as_str().to_owned(),
                    selected_name: command.selected_name.clone(),
                    applied_count: 0,
                    unsupported_count: 0,
                    error: format!(
                        "sequence {} is waiting for the picker-owned drive row; ordinary row slot {native_slot} cannot acknowledge it",
                        command.sequence
                    ),
                },
            );
        }
        return;
    }
    PROFILE_EDITOR_LAST_SEQUENCE.store(command.sequence, Ordering::SeqCst);
    let (applied, unsupported, error) =
        unsafe { apply_profile_editor_command(base, row_proxy, row_model, native_slot, &command) };
    write_status(
        &dir,
        status_for(&command, active_surface, applied, unsupported, error),
    );
}

fn command_targets_drive_row(command: &ProfileEditorCommand) -> bool {
    match command.selected_kind {
        SelectedKind::Chrome => matches!(
            command.selected_name.as_str(),
            "drive_button" | "path_button"
        ),
        SelectedKind::Field => {
            command.selected_name == "CurrentPath"
                || er_gfx::title_05_010::is_drive_cell_field_name(&command.selected_name)
        }
        SelectedKind::PathEditor | SelectedKind::List => false,
    }
}

unsafe fn apply_profile_editor_command(
    base: usize,
    row_proxy: usize,
    row_model: usize,
    native_slot: i32,
    command: &ProfileEditorCommand,
) -> (u32, u32, String) {
    if row_proxy == 0 || row_proxy == TITLE_OWNER_SCAN_START_ADDRESS {
        return (0, 1, "row_proxy unavailable".to_owned());
    }
    // A native slot number alone does not mean this is our browse row: ordinary character lists use
    // the same 0..9 values. The picker model is the ownership oracle and says exactly how many drive
    // buttons this row currently exposes. Transforming all 26 hidden buttons on every vanilla row
    // acknowledged successfully, then the process died when the next picker movie opened.
    let live_drive_cell_count = usize::try_from(native_slot)
        .ok()
        .and_then(super::save_picker_row_slot_info)
        .map(|info| info.drive_cell_count)
        .unwrap_or(0);
    match command.selected_kind {
        SelectedKind::Field => unsafe {
            apply_profile_editor_field_probe(
                base,
                row_proxy,
                row_model,
                native_slot,
                live_drive_cell_count,
                command,
            )
        },
        SelectedKind::PathEditor => (
            0,
            0,
            "active path-editor commands apply only from the owned 02_990 MenuWindow::Run context"
                .to_owned(),
        ),
        SelectedKind::Chrome => unsafe {
            let focus = usize::try_from(native_slot)
                .ok()
                .and_then(super::save_picker_row_slot_info)
                .and_then(|info| info.drive_strip_focus);
            apply_profile_editor_chrome_probe(
                base,
                row_proxy,
                live_drive_cell_count,
                focus,
                command,
            )
        },
        SelectedKind::List => (
            0,
            1,
            "list/mask/scrollbar geometry is asset-level; use rebuild hot-reload, then re-open the ProfileSelect movie".to_owned(),
        ),
    }
}

unsafe fn apply_profile_editor_chrome_probe(
    base: usize,
    row_proxy: usize,
    live_drive_cell_count: usize,
    drive_strip_focus: Option<er_save_picker::DriveStripFocus>,
    command: &ProfileEditorCommand,
) -> (u32, u32, String) {
    match command.selected_name.as_str() {
        "cursor" => unsafe {
            apply_profile_editor_named_chrome_probe(base, row_proxy, command, "Cursor")
        },
        "backing" => unsafe {
            apply_profile_editor_named_chrome_probe(base, row_proxy, command, "Backing")
        },
        "cursor_body" => unsafe {
            apply_profile_editor_nested_chrome_probe(
                base,
                row_proxy,
                command,
                "Cursor",
                "CursorBody",
            )
        },
        "drive_button" if live_drive_cell_count > 0 => unsafe {
            let (mut applied, mut unsupported, mut detail) =
                apply_profile_editor_drive_button_probe(
                    base,
                    row_proxy,
                    live_drive_cell_count,
                    command,
                );
            if let Some(focus) = drive_strip_focus {
                if apply_drive_row_native_cursor(base, row_proxy, focus) {
                    applied += 1;
                    detail.push_str(
                        " | active native Cursor reapplied from reverted drive-button geometry",
                    );
                } else {
                    unsupported += 1;
                    detail.push_str(" | active native Cursor reapply failed");
                }
            }
            (applied, unsupported, detail)
        },
        "drive_button" => (
            0,
            0,
            "drive_button transform deferred until the picker-owned drive row populates".to_owned(),
        ),
        "path_button" if live_drive_cell_count > 0 => unsafe {
            apply_profile_editor_path_button_probe(base, row_proxy, command)
        },
        "path_button" => (
            0,
            0,
            "path_button transform deferred until the picker-owned drive row populates".to_owned(),
        ),
        other => (0, 1, format!("unknown chrome object {other}")),
    }
}

unsafe fn apply_profile_editor_named_chrome_probe(
    base: usize,
    row_proxy: usize,
    command: &ProfileEditorCommand,
    native_name: &str,
) -> (u32, u32, String) {
    let t = match command.selected_name.as_str() {
        "backing" => &command.layout.row_chrome.backing,
        "cursor" => &command.layout.row_chrome.cursor,
        "cursor_body" => &command.layout.row_chrome.cursor_body,
        "drive_button" => &command.layout.row_chrome.drive_button,
        "path_button" => &command.layout.row_chrome.path_button,
        other => return (0, 1, format!("missing chrome layout {other}")),
    };
    match unsafe { resolve_row_child_proxy(base, row_proxy, native_name) } {
        Some((child_proxy, _component_slot)) => {
            let (applied, unsupported, detail) = unsafe {
                apply_profile_editor_transform_to_proxy(
                    base,
                    child_proxy,
                    t,
                    command.selected_name.as_str(),
                )
            };
            unsafe { destroy_resolved_row_child_proxy(base, child_proxy) };
            (
                applied,
                unsupported,
                format!(
                    "{} live transform via native child {native_name}: {detail}",
                    command.selected_name
                ),
            )
        }
        None => (
            0,
            1,
            format!(
                "native child {native_name} did not resolve on row_proxy=0x{row_proxy:x}; reload the edited 05_010 movie once so named chrome exists"
            ),
        ),
    }
}

unsafe fn apply_profile_editor_drive_button_probe(
    base: usize,
    row_proxy: usize,
    live_drive_cell_count: usize,
    command: &ProfileEditorCommand,
) -> (u32, u32, String) {
    use er_gfx::title_05_010::{
        DRIVE_BUTTON_FIELD_NAMES, DRIVE_BUTTON_NATIVE_ART_HEIGHT_PX,
        DRIVE_BUTTON_NATIVE_ART_WIDTH_PX,
    };

    let field0 = command.layout.field("DriveCell_0");
    let field1 = command.layout.field("DriveCell_1");
    let pitch = field1.x - field0.x;
    let relative = &command.layout.row_chrome.drive_button;
    let mut applied = 0u32;
    let mut unsupported = 0u32;
    for (index, native_name) in DRIVE_BUTTON_FIELD_NAMES
        .iter()
        .copied()
        .enumerate()
        .take(live_drive_cell_count)
    {
        let absolute = er_gfx::profile_05_010_layout::TransformLayout {
            x: field0.x - 2.0 + field0.width as f32 * 0.5 + pitch * index as f32 + relative.x,
            y: field0.y - 2.0 + field0.clip_height as f32 * 0.5 + relative.y,
            scale_x: (field0.width as f32 / DRIVE_BUTTON_NATIVE_ART_WIDTH_PX) * relative.scale_x,
            scale_y: (field0.clip_height as f32 / DRIVE_BUTTON_NATIVE_ART_HEIGHT_PX)
                * relative.scale_y,
            opacity: relative.opacity,
            editable: relative.editable,
            source: relative.source.clone(),
        };
        match unsafe { resolve_row_child_proxy(base, row_proxy, native_name) } {
            Some((child_proxy, _component_slot)) => {
                let (this_applied, this_unsupported, _) = unsafe {
                    apply_profile_editor_transform_to_proxy(
                        base,
                        child_proxy,
                        &absolute,
                        native_name,
                    )
                };
                unsafe { destroy_resolved_row_child_proxy(base, child_proxy) };
                applied += this_applied;
                unsupported += this_unsupported;
            }
            None => unsupported += 1,
        }
    }
    (
        applied,
        unsupported,
        format!(
            "drive_button live group transform: cells={live_drive_cell_count} applied={applied} unsupported={unsupported} relative=({:.2},{:.2}) scale=({:.3},{:.3}) pitch={pitch:.2}",
            relative.x, relative.y, relative.scale_x, relative.scale_y
        ),
    )
}

unsafe fn apply_profile_editor_path_button_probe(
    base: usize,
    row_proxy: usize,
    command: &ProfileEditorCommand,
) -> (u32, u32, String) {
    let transform = current_path_button_transform(&command.layout);
    match unsafe { resolve_row_child_proxy(base, row_proxy, "CurrentPathButton") } {
        Some((child_proxy, _component_slot)) => {
            let (applied, unsupported, detail) = unsafe {
                apply_profile_editor_transform_to_proxy(
                    base,
                    child_proxy,
                    &transform,
                    "CurrentPathButton",
                )
            };
            unsafe { destroy_resolved_row_child_proxy(base, child_proxy) };
            (
                applied,
                unsupported,
                format!("CurrentPathButton follows CurrentPath bounds: {detail}"),
            )
        }
        None => (
            0,
            1,
            format!(
                "native child CurrentPathButton did not resolve on drive row_proxy=0x{row_proxy:x}"
            ),
        ),
    }
}

fn current_path_button_transform(
    layout: &er_gfx::profile_05_010_layout::Profile05_010Layout,
) -> er_gfx::profile_05_010_layout::TransformLayout {
    layout.current_path_button_transform()
}

fn drive_cell_cursor_transform(
    active_cell: usize,
) -> er_gfx::profile_05_010_layout::TransformLayout {
    use er_gfx::profile_05_010_layout::TransformLayout;
    use er_gfx::title_05_010::{
        DRIVE_BUTTON_NATIVE_ART_HEIGHT_PX, DRIVE_BUTTON_NATIVE_ART_WIDTH_PX, DRIVE_CELL_FIRST_X_PX,
        DRIVE_CELL_HEIGHT_PX, DRIVE_CELL_PITCH_PX, DRIVE_CELL_WIDTH_PX, DRIVE_CELL_Y_PX,
    };

    let cached = PROFILE_EDITOR_LAST_LAYOUT
        .get()
        .and_then(|layout| layout.lock().ok())
        .map(|layout| {
            let field0 = layout.field("DriveCell_0");
            let field1 = layout.field("DriveCell_1");
            let relative = &layout.row_chrome.drive_button;
            let cursor_body = &layout.row_chrome.cursor_body;
            (
                field0.x + (field1.x - field0.x) * active_cell as f32,
                field0.y,
                field0.width as f32,
                field0.clip_height as f32,
                relative.x,
                relative.y,
                relative.scale_x,
                relative.scale_y,
                cursor_body.scale_x,
                cursor_body.scale_y,
            )
        });
    let (
        field_x,
        field_y,
        width,
        height,
        nudge_x,
        nudge_y,
        button_scale_x,
        button_scale_y,
        body_scale_x,
        body_scale_y,
    ) = cached.unwrap_or((
        DRIVE_CELL_FIRST_X_PX + DRIVE_CELL_PITCH_PX * active_cell as f32,
        DRIVE_CELL_Y_PX,
        DRIVE_CELL_WIDTH_PX,
        DRIVE_CELL_HEIGHT_PX,
        -2.0,
        0.0,
        1.0,
        1.0,
        20.0,
        1.0,
    ));
    TransformLayout {
        x: field_x - 2.0 + width * 0.5 + nudge_x,
        y: field_y - 2.0 + height * 0.5 + nudge_y,
        // CursorBody is already scaled to full-row width inside this wrapper. Shrinking the outer
        // Cursor is equivalent to shrinking its body, but uses the setter path that runtime proved
        // valid. The nested CursorBody setter failed in both pre- and post-populate contexts.
        scale_x: ((width / DRIVE_BUTTON_NATIVE_ART_WIDTH_PX) * button_scale_x) / body_scale_x,
        scale_y: ((height / DRIVE_BUTTON_NATIVE_ART_HEIGHT_PX) * button_scale_y) / body_scale_y,
        opacity: 1.0,
        editable: false,
        source: "native drive-row Cursor moves and shrinks as one setter-valid outer object"
            .to_owned(),
    }
}

fn current_path_cursor_transform_for_layout(
    layout: &er_gfx::profile_05_010_layout::Profile05_010Layout,
) -> er_gfx::profile_05_010_layout::TransformLayout {
    use er_gfx::profile_05_010_layout::TransformLayout;
    use er_gfx::title_05_010::{
        DRIVE_BUTTON_NATIVE_ART_HEIGHT_PX, DRIVE_BUTTON_NATIVE_ART_WIDTH_PX,
    };

    let field = layout.field("CurrentPath");
    let button = &layout.row_chrome.path_button;
    let body = &layout.row_chrome.cursor_body;
    TransformLayout {
        x: field.x - 2.0 + field.width as f32 * 0.5 + button.x,
        y: field.y - 2.0 + field.clip_height as f32 * 0.5 + button.y,
        scale_x: ((field.width as f32 / DRIVE_BUTTON_NATIVE_ART_WIDTH_PX) * button.scale_x)
            / body.scale_x,
        scale_y: ((field.clip_height as f32 / DRIVE_BUTTON_NATIVE_ART_HEIGHT_PX) * button.scale_y)
            / body.scale_y,
        opacity: 1.0,
        editable: false,
        source: "native drive-row Cursor follows the complete-path button bounds".to_owned(),
    }
}

fn current_path_cursor_transform() -> er_gfx::profile_05_010_layout::TransformLayout {
    if let Some(layout) = PROFILE_EDITOR_LAST_LAYOUT.get()
        && let Ok(layout) = layout.lock()
    {
        return current_path_cursor_transform_for_layout(&layout);
    }
    current_path_cursor_transform_for_layout(
        &er_gfx::profile_05_010_layout::Profile05_010Layout::default(),
    )
}

/// Resize the row's own animated native Cursor to the keyboard/mouse focus target. Visibility
/// remains under the game's list-selection code; only drive-row sub-control geometry is changed.
pub(crate) unsafe fn apply_drive_row_native_cursor(
    base: usize,
    row_proxy: usize,
    focus: er_save_picker::DriveStripFocus,
) -> bool {
    use er_gfx::title_05_010::DRIVE_CELL_CAPACITY;
    let transform = match focus {
        er_save_picker::DriveStripFocus::Cell(active_cell) => {
            if active_cell >= DRIVE_CELL_CAPACITY {
                return false;
            }
            drive_cell_cursor_transform(active_cell)
        }
        er_save_picker::DriveStripFocus::CurrentPath => current_path_cursor_transform(),
    };
    let Some((cursor_proxy, _cursor_slot)) =
        (unsafe { resolve_row_child_proxy(base, row_proxy, "Cursor") })
    else {
        return false;
    };
    let (applied, unsupported, _) = unsafe {
        apply_profile_editor_transform_to_proxy(base, cursor_proxy, &transform, "drive-row Cursor")
    };
    unsafe { destroy_resolved_row_child_proxy(base, cursor_proxy) };
    applied == 2 && unsupported == 0
}

unsafe fn apply_profile_editor_nested_chrome_probe(
    base: usize,
    row_proxy: usize,
    command: &ProfileEditorCommand,
    parent_name: &str,
    child_name: &str,
) -> (u32, u32, String) {
    let t = match command.selected_name.as_str() {
        "cursor_body" => &command.layout.row_chrome.cursor_body,
        other => return (0, 1, format!("missing nested chrome layout {other}")),
    };
    let Some((parent_proxy, _parent_slot)) =
        (unsafe { resolve_row_child_proxy(base, row_proxy, parent_name) })
    else {
        return (
            0,
            1,
            format!(
                "native parent child {parent_name} did not resolve on row_proxy=0x{row_proxy:x}; reload the edited 05_010 movie once so named chrome exists"
            ),
        );
    };
    let result = match unsafe { resolve_row_child_proxy(base, parent_proxy, child_name) } {
        Some((child_proxy, _component_slot)) => {
            let (applied, unsupported, detail) = unsafe {
                apply_profile_editor_transform_to_proxy(
                    base,
                    child_proxy,
                    t,
                    command.selected_name.as_str(),
                )
            };
            unsafe { destroy_resolved_row_child_proxy(base, child_proxy) };
            (
                applied,
                unsupported,
                format!(
                    "{} live transform via nested native child {parent_name}/{child_name}: {detail}",
                    command.selected_name
                ),
            )
        }
        None => (
            0,
            1,
            format!(
                "nested native child {parent_name}/{child_name} did not resolve on row_proxy=0x{row_proxy:x}; reload the edited 05_010 movie once so CursorBody exists"
            ),
        ),
    };
    unsafe { destroy_resolved_row_child_proxy(base, parent_proxy) };
    result
}

static PATH_EDITOR_WINDOW_POSITION_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
static PATH_EDITOR_WINDOW_POSITION_SUCCESSES: AtomicUsize = AtomicUsize::new(0);

/// Position the separate 02_990 MenuWindow root over ProfileSelect's CurrentPath field. The native
/// SoftwareKeyboard controller owns and rewrites its child display object after GFx parsing, so the
/// external MenuWindow SceneObjProxy is the stable placement boundary.
pub(crate) unsafe fn apply_path_editor_window_position(base: usize, menu_window: usize) {
    if menu_window == 0 || menu_window == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let attempt = PATH_EDITOR_WINDOW_POSITION_ATTEMPTS.fetch_add(1, Ordering::SeqCst) + 1;
    let layout = PROFILE_EDITOR_LAST_LAYOUT
        .get()
        .and_then(|layout| layout.lock().ok())
        .map(|layout| layout.clone())
        .unwrap_or_default();
    let (x, y) = er_gfx::text_input_02_990::path_editor_window_position_for_layout(&layout);
    let transform = er_gfx::profile_05_010_layout::TransformLayout {
        x,
        y,
        scale_x: 1.0,
        scale_y: 1.0,
        opacity: 1.0,
        editable: false,
        source: "native 02_990 MenuWindow root positions the editor over CurrentPath".to_owned(),
    };
    let proxy = menu_window + OPTION_SETTING_ROOT_PROXY_OFFSET;
    let (applied, unsupported, detail) = unsafe {
        apply_profile_editor_transform_to_proxy(base, proxy, &transform, "02_990 root MenuWindow")
    };
    if applied > 0 {
        PATH_EDITOR_WINDOW_POSITION_SUCCESSES.fetch_add(1, Ordering::SeqCst);
        let pending_sequence = PATH_EDITOR_PENDING_SEQUENCE.swap(0, Ordering::SeqCst);
        if pending_sequence != 0 {
            PROFILE_EDITOR_LAST_SEQUENCE.store(pending_sequence, Ordering::SeqCst);
            if let Some(dir) = editor_dir() {
                write_status(
                    &dir,
                    ProfileEditorStatus {
                        version: er_gfx::profile_05_010_protocol::PROTOCOL_VERSION,
                        ack_sequence: pending_sequence,
                        connected: true,
                        status: "live-runtime-command-accepted".to_owned(),
                        active_surface: "02_990-menu-window-run".to_owned(),
                        selected_kind: SelectedKind::PathEditor.as_str().to_owned(),
                        selected_name: "ActivePathEditor".to_owned(),
                        applied_count: applied,
                        unsupported_count: unsupported,
                        error: format!(
                            "active 02_990 root moved to ({x:.1},{y:.1}); width/height/font remain asset edits and require rebuild + reopen"
                        ),
                    },
                );
            }
        }
    }
    if attempt <= 8 || (unsupported > 0 && attempt.is_power_of_two()) {
        append_autoload_debug(format_args!(
            "save-picker-path: positioned 02_990 MenuWindow attempt={attempt} window=0x{menu_window:x} proxy=0x{proxy:x} target=({x:.1},{y:.1}) applied={applied} unsupported={unsupported} detail={detail}"
        ));
    }
    unsafe { apply_path_editor_caret_to_end(base, menu_window) };
}

/// Applications of the end-caret per editor open. The field is not guaranteed to be focused on the
/// frame the window first runs, and taking focus is what would reset a caret we set too early, so the
/// request is repeated over the same short window the positioning pass uses. It stays far shorter
/// than any human can type into a box that has only just appeared, so it can never fight the user's
/// own Home/End.
const PATH_EDITOR_CARET_APPLY_FRAMES: usize = 8;
static PATH_EDITOR_CARET_APPLIES: AtomicUsize = AtomicUsize::new(0);
static PATH_EDITOR_CARET_RESOLVED: AtomicUsize = AtomicUsize::new(0);

/// Re-arm the end-caret for a newly opened path editor. Keyed off the window's open transition rather
/// than a window pointer, because the allocator reuses that address across opens and a pointer-keyed
/// latch would silently skip every editor after the first.
pub(crate) fn reset_path_editor_caret_latch() {
    PATH_EDITOR_CARET_APPLIES.store(0, Ordering::SeqCst);
    PATH_EDITOR_CARET_RESOLVED.store(0, Ordering::SeqCst);
}

/// Put the caret at the END of the prefilled path when the editor opens.
///
/// The editor is prefilled with the current path and the caret sits at index 0, so typing prepends to
/// the path instead of appending to it. No native object owns that caret: the SoftwareKeyboard config
/// holds only a prompt string, a max length and flags, and its set-initial path is a pure `DLString`
/// assign. The caret is Scaleform's, and [`GFX_TEXT_FIELD_SET_SELECTION_RVA`] is what moves it -- the
/// same primitive ActionScript's `Selection.setSelection` calls after its own type check.
unsafe fn apply_path_editor_caret_to_end(base: usize, menu_window: usize) {
    let applies = PATH_EDITOR_CARET_APPLIES.fetch_add(1, Ordering::SeqCst);
    if applies >= PATH_EDITOR_CARET_APPLY_FRAMES {
        return;
    }
    let outcome = unsafe { place_path_editor_caret_at_end(base, menu_window) };
    // Log the first resolution either way, then stay quiet: this runs every frame of the open window
    // and a per-frame line would bury the rest of the editor's trace.
    if PATH_EDITOR_CARET_RESOLVED
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        match outcome {
            Ok(detail) => append_autoload_debug(format_args!(
                "save-picker-path: caret moved to end of the prefilled path window=0x{menu_window:x} {detail}"
            )),
            Err(error) => append_autoload_debug(format_args!(
                "save-picker-path: caret stays at the start window=0x{menu_window:x}; {error}"
            )),
        }
    }
}

/// Resolve the live editable field by its authored name (`root -> TextInput -> Text_0`) through the
/// same native `assignComponentWithName` binder the stats push uses, then move its caret.
unsafe fn place_path_editor_caret_at_end(
    base: usize,
    menu_window: usize,
) -> Result<String, String> {
    if menu_window == 0 || menu_window == TITLE_OWNER_SCAN_START_ADDRESS {
        return Err("02_990 MenuWindow not live".to_owned());
    }
    let root_proxy = menu_window + OPTION_SETTING_ROOT_PROXY_OFFSET;
    let sprite_name = er_gfx::text_input_02_990::TEXT_INPUT_SPRITE_NAME;
    let field_name = er_gfx::text_input_02_990::TEXT_FIELD_INSTANCE_NAME;
    let Some((sprite_proxy, _sprite_slot)) =
        (unsafe { resolve_row_child_proxy(base, root_proxy, sprite_name) })
    else {
        return Err(format!(
            "child {sprite_name} did not resolve on 02_990 root proxy=0x{root_proxy:x}"
        ));
    };
    let result = match unsafe { resolve_row_child_proxy(base, sprite_proxy, field_name) } {
        Some((field_proxy, _field_slot)) => {
            let outcome = unsafe {
                set_text_field_caret_to_end(
                    base,
                    field_proxy + SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET,
                )
            };
            unsafe { destroy_resolved_row_child_proxy(base, field_proxy) };
            outcome
        }
        None => Err(format!(
            "child {sprite_name}/{field_name} did not resolve on 02_990 window=0x{menu_window:x}"
        )),
    };
    unsafe { destroy_resolved_row_child_proxy(base, sprite_proxy) };
    result
}

/// Move a resolved field's caret to the end of its text.
///
/// Guarded exactly like the native text helpers: the value must carry a text object, whose vtable and
/// runtime type tag must both check out before anything is handed to a native call. The setter clamps
/// both indices to the live text length, so [`GFX_TEXT_FIELD_SELECTION_END`] asks for the end rather
/// than guessing a position, and a collapsed range leaves a caret rather than a selection.
unsafe fn set_text_field_caret_to_end(base: usize, cs_value: usize) -> Result<String, String> {
    let handle = unsafe { safe_read_usize(cs_value + CSSCALEFORMVALUE_HANDLE_OFFSET) }.unwrap_or(0);
    if handle == 0 || handle == TITLE_OWNER_SCAN_START_ADDRESS {
        return Err(format!("CSScaleformValue handle empty at 0x{cs_value:x}"));
    }
    let text_object =
        unsafe { safe_read_usize(handle + GFX_VALUE_TEXT_OBJECT_OFFSET) }.unwrap_or(0);
    if text_object == 0 || text_object == TITLE_OWNER_SCAN_START_ADDRESS {
        return Err(format!(
            "GFx value at 0x{handle:x} has no text object at +0x{GFX_VALUE_TEXT_OBJECT_OFFSET:x}"
        ));
    }
    let text_vt = unsafe { safe_read_usize(text_object) }.unwrap_or(0);
    if text_vt == 0 || !vtable_in_game_image(text_vt, base) {
        return Err(format!(
            "text object vt invalid object=0x{text_object:x} vt=0x{text_vt:x}"
        ));
    }
    let kind_fn =
        unsafe { safe_read_usize(text_vt + GFX_TEXT_OBJECT_KIND_VTABLE_SLOT) }.unwrap_or(0);
    if kind_fn == 0 || !vtable_in_game_image(kind_fn, base) {
        return Err(format!(
            "text object kind function invalid object=0x{text_object:x} vt=0x{text_vt:x} kind=0x{kind_fn:x}"
        ));
    }
    let kind: unsafe extern "system" fn(usize) -> i32 = unsafe { std::mem::transmute(kind_fn) };
    let kind = unsafe { kind(text_object) };
    if kind != GFX_TEXT_OBJECT_KIND_TEXT_FIELD {
        return Err(format!(
            "GFx object at 0x{text_object:x} is kind {kind}, not text-field kind {GFX_TEXT_OBJECT_KIND_TEXT_FIELD}"
        ));
    }
    let set_selection: unsafe extern "system" fn(usize, i64, i64) =
        unsafe { std::mem::transmute(base + GFX_TEXT_FIELD_SET_SELECTION_RVA) };
    unsafe {
        set_selection(
            text_object,
            GFX_TEXT_FIELD_SELECTION_END,
            GFX_TEXT_FIELD_SELECTION_END,
        )
    };
    Ok(format!("text object=0x{text_object:x}"))
}

unsafe fn apply_profile_editor_transform_to_proxy(
    base: usize,
    proxy: usize,
    transform: &er_gfx::profile_05_010_layout::TransformLayout,
    label: &str,
) -> (u32, u32, String) {
    let embedded = proxy + SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET;
    let (cs_value, source, guard_note) = match unsafe {
        scaleform_value_setter_guard(base, embedded)
    } {
        Ok(()) => (embedded, "embedded", String::new()),
        Err(embedded_error) => match unsafe { component_scaleform_value_for_setter(base, proxy) } {
            Ok(component_value) => (
                component_value,
                "component-get-value",
                format!("embedded value skipped: {embedded_error}; "),
            ),
            Err(component_error) => {
                return (
                    0,
                    1,
                    format!(
                        "{label} has no setter-ready value: embedded={embedded_error}; component={component_error}"
                    ),
                );
            }
        },
    };
    let moved = unsafe { set_scaleform_value_position(base, cs_value, transform.x, transform.y) };
    // Pass the schema's unit factor straight through. The native setter FUN_140d84090 already
    // converts to Scaleform's percent space itself (`local_e0 = (double)*param_2 * DAT_14329e698`
    // with DAT_14329e698 = 100.0, byte-checked in eldenring-deobf.bin at 0x14329e698), and its
    // paired getter FUN_140d82c90 divides by the same constant to hand a factor back. Multiplying
    // by 100 here as well applied every live chrome scale 100x too large -- the schema's
    // backing.scale_x = 20 would have landed as a 2000x matrix. The shipped rows look right only
    // because that value reaches the movie through the ASSET matrix in make_05_010_stats.rs; this
    // setter runs solely for live chrome edits, which is why the error stayed invisible.
    let scaled =
        unsafe { set_scaleform_value_scale(base, cs_value, transform.scale_x, transform.scale_y) };
    (
        moved as u32 + scaled as u32,
        if moved && scaled { 0 } else { 1 },
        format!("{guard_note}value_source={source} moved={moved} scaled={scaled}"),
    )
}

unsafe fn component_scaleform_value_for_setter(base: usize, proxy: usize) -> Result<usize, String> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let component_slot = proxy + SCENE_OBJ_PROXY_COMPONENT_SLOT_OFFSET;
    let comp = unsafe { safe_read_usize(component_slot) }.unwrap_or(0);
    if comp == 0 || comp == null {
        return Err(format!("component pointer empty at 0x{component_slot:x}"));
    }
    unsafe { scaleform_value_for_component(base, comp) }
}

unsafe fn scaleform_value_for_component(base: usize, comp: usize) -> Result<usize, String> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if comp == 0 || comp == null {
        return Err("component pointer empty".to_owned());
    }
    let comp_vt = unsafe { safe_read_usize(comp) }.unwrap_or(0);
    let get_value = if comp_vt != 0 {
        unsafe { safe_read_usize(comp_vt + COMPONENT_GET_VALUE_VTABLE_SLOT_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    if comp_vt == 0 || !vtable_in_game_image(comp_vt, base) {
        return Err(format!(
            "component vt invalid comp=0x{comp:x} vt=0x{comp_vt:x}"
        ));
    }
    if get_value == 0 || !vtable_in_game_image(get_value, base) {
        return Err(format!(
            "component get-value invalid comp=0x{comp:x} vt=0x{comp_vt:x} get=0x{get_value:x}"
        ));
    }
    // A DESTRUCTED OBJECT PASSES EVERY CHECK ABOVE. Its vtable is the abstract base's, whose slots
    // hold `_purecall` -- an address inside the game image, so "the pointer looks like game code"
    // says nothing. Calling it writes 0xdead to NULL and takes the process with it.
    if dispatch_target_is_purecall(get_value, base) {
        return Err(format!(
            "component 0x{comp:x} has been DESTROYED (vt=0x{comp_vt:x} get-value is the pure-virtual trap 0x{get_value:x}); its screen is gone"
        ));
    }
    let get_value: unsafe extern "system" fn(usize) -> usize =
        unsafe { std::mem::transmute(get_value) };
    let value = unsafe { get_value(comp) };
    if value == 0 || value == null {
        return Err(format!(
            "component get-value returned empty comp=0x{comp:x} vt=0x{comp_vt:x}"
        ));
    }
    unsafe { scaleform_value_setter_guard(base, value) }
        .map_err(|e| format!("component value guard failed at 0x{value:x}: {e}"))?;
    Ok(value)
}

/// Apply EVERY field in the layout, not just the one selected in the browser.
///
/// The selected field is a UI notion -- which control panel is open -- and was wrongly being used
/// as the set of things to push live. Because the native row populate rewrites all fields from the
/// baked asset on every rebuild, applying only the selected one meant each save reverted every
/// other field to its asset position. That produced the toggle the user hit: nudge A, save, nudge
/// B, save, and A snaps back; save A again and B snaps back. Only ever one live edit at a time,
/// and it looked like an ordering bug rather than a scope bug.
unsafe fn apply_profile_editor_field_probe(
    base: usize,
    row_proxy: usize,
    row_model: usize,
    native_slot: i32,
    live_drive_cell_count: usize,
    command: &ProfileEditorCommand,
) -> (u32, u32, String) {
    let selected = command.selected_name.as_str();
    let mut applied_total = 0u32;
    let mut unsupported_total = 0u32;
    let mut selected_detail = String::new();
    let mut other_failures: Vec<String> = Vec::new();
    for field_name in er_gfx::profile_05_010_layout::FIELD_NAMES {
        if !command.layout.fields.contains_key(field_name) {
            continue;
        }
        let (applied, unsupported, detail) = unsafe {
            apply_profile_editor_one_field(
                base,
                row_proxy,
                row_model,
                native_slot,
                command,
                field_name,
            )
        };
        applied_total += applied;
        unsupported_total += unsupported;
        if field_name == selected {
            selected_detail = detail;
        } else if unsupported > 0 {
            other_failures.push(format!("{field_name}: {detail}"));
        }
    }
    if selected_detail.is_empty() {
        selected_detail = format!("selected field {selected} is not a known row text field");
        unsupported_total += 1;
    }
    // DriveCell clip_height/width are shared authored cell geometry. The field setter can move and
    // reflow the drive text live, but the visible cell is the separate DriveButton_* native frame.
    // Not applying that group made every height command truthfully update the schema while changing
    // no visible button pixels. Keep this inside row-populate, where the game's child proxies are
    // owned and alive; the frame-thread cached-proxy path is deliberately forbidden above.
    if er_gfx::title_05_010::is_drive_cell_field_name(selected) && live_drive_cell_count > 0 {
        let (applied, unsupported, detail) = unsafe {
            apply_profile_editor_drive_button_probe(base, row_proxy, live_drive_cell_count, command)
        };
        applied_total += applied;
        unsupported_total += unsupported;
        selected_detail = format!("{selected_detail} | {detail}");
    }
    // CurrentPath is also a two-object control: a text field plus the separate native button frame.
    // Resizing only the text document changes no visible outline, which made a successfully-acked
    // width edit look like a no-op. Apply the button from the same field bounds on the same owned
    // drive-row populate; the native focused Cursor uses this layout too.
    if selected == "CurrentPath" && live_drive_cell_count > 0 {
        let (applied, unsupported, detail) =
            unsafe { apply_profile_editor_path_button_probe(base, row_proxy, command) };
        applied_total += applied;
        unsupported_total += unsupported;
        selected_detail = format!("{selected_detail} | {detail}");
    }
    let detail = if other_failures.is_empty() {
        selected_detail
    } else {
        format!(
            "{selected_detail} | {} other field(s) did not fully apply: {}",
            other_failures.len(),
            other_failures.join("; ")
        )
    };
    (applied_total, unsupported_total, detail)
}

unsafe fn apply_profile_editor_one_field(
    base: usize,
    row_proxy: usize,
    _row_model: usize,
    _native_slot: i32,
    command: &ProfileEditorCommand,
    field_name: &str,
) -> (u32, u32, String) {
    if !er_gfx::profile_05_010_layout::FIELD_NAMES.contains(&field_name) {
        return (0, 1, format!("unknown field {field_name}"));
    }
    let Some(field) = command.layout.fields.get(field_name) else {
        return (0, 1, format!("missing field layout {field_name}"));
    };
    match unsafe { resolve_row_child_proxy(base, row_proxy, field_name) } {
        Some((child_proxy, _component_slot)) => {
            let embedded = child_proxy + SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET;
            let (cs_value, source, guard_note) = match unsafe {
                component_scaleform_value_for_setter(base, child_proxy)
            } {
                Ok(component_value) => (component_value, "component-get-value", String::new()),
                Err(component_error) => {
                    match unsafe { scaleform_value_setter_guard(base, embedded) } {
                        Ok(()) => (
                            embedded,
                            "embedded",
                            format!("component value skipped: {component_error}; "),
                        ),
                        Err(embedded_error) => {
                            let error = format!(
                                "field {field_name} has no setter-ready value: component={component_error}; embedded={embedded_error}"
                            );
                            (embedded, "none", error)
                        }
                    }
                }
            };
            let mut field_name_nul = String::with_capacity(field_name.len() + 1);
            field_name_nul.push_str(field_name);
            field_name_nul.push('\0');
            let (repush_source, repush_preview, repush_utf16) =
                if command.text_probe && !field.sample_load_character.is_empty() {
                    let utf16: Vec<u16> = field
                        .sample_load_character
                        .encode_utf16()
                        .chain(core::iter::once(0))
                        .collect();
                    ("sample", utf16_status_preview(&utf16), Some(utf16))
                } else {
                    let real_utf16 = if field_name == "PlayerName" {
                        live_player_name_utf16()
                            .map(|text| ("live-pgd", text))
                            .or_else(|| {
                                cached_profile_editor_field_utf16(field_name)
                                    .map(|text| ("cache", text))
                            })
                    } else {
                        cached_profile_editor_field_utf16(field_name).map(|text| ("cache", text))
                    };
                    if let Some((source, utf16)) = real_utf16 {
                        (source, utf16_status_preview(&utf16), Some(utf16))
                    } else {
                        ("none", String::new(), None)
                    }
                };
            let (moved, text_repush, width_result, error) = if guard_note.is_empty()
                || source != "none"
            {
                let moved =
                    unsafe { set_scaleform_value_position(base, cs_value, field.x, field.y) };
                let text_repush = if let Some(utf16) = repush_utf16.as_ref() {
                    unsafe {
                        crate::experiments::startup_hooks::loading_cover::push_stats_text_on_resolved_field(
                            base,
                            child_proxy,
                            &field_name_nul,
                            utf16,
                        )
                    }
                } else {
                    false
                };
                let width_result =
                    unsafe { set_text_field_width_probe(base, cs_value, field.width) };
                (moved, text_repush, width_result, guard_note)
            } else {
                (false, false, Err(guard_note.clone()), guard_note)
            };
            unsafe { destroy_resolved_row_child_proxy(base, child_proxy) };
            let width_detail = match &width_result {
                Ok(detail) => detail.clone(),
                Err(e) => e.clone(),
            };
            let width_applied = width_result.is_ok();
            let applied = moved as u32 + width_applied as u32 + text_repush as u32;
            let unsupported = (!moved) as u32 + (!width_applied) as u32;
            (
                applied,
                unsupported,
                if error.is_empty() {
                    format!(
                        "field {field_name} live x/y moved={moved}; width_probe={width_applied} ({width_detail}); text_probe={} text_repush={text_repush} source={repush_source} preview={repush_preview:?}; font/align hot-reload through native SetText-owned text",
                        command.text_probe
                    )
                } else {
                    error
                },
            )
        }
        None => (
            0,
            1,
            format!("field {field_name} did not resolve on row_proxy=0x{row_proxy:x}"),
        ),
    }
}

unsafe fn resolve_row_child_proxy(
    base: usize,
    row_proxy: usize,
    name: &str,
) -> Option<(usize, usize)> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let assign = match TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_ORIG.load(Ordering::SeqCst) {
        orig if orig != null && orig != HOOK_ORIGINAL_UNSET => orig,
        _ => base + TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA,
    };
    let assign: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(assign) };
    let mut nul_name = String::with_capacity(name.len() + 1);
    nul_name.push_str(name);
    nul_name.push(' ');
    let proxy = Box::into_raw(Box::new([0u8; SCENE_OBJ_PROXY_STACK_BYTES])) as usize;
    let out = unsafe { assign(row_proxy, proxy, nul_name.as_ptr() as usize) };
    if out == 0 || out == null {
        unsafe {
            drop(Box::from_raw(
                proxy as *mut [u8; SCENE_OBJ_PROXY_STACK_BYTES],
            ))
        };
        return None;
    }
    let component_slot = out + SCENE_OBJ_PROXY_COMPONENT_SLOT_OFFSET;
    let comp = unsafe { safe_read_usize(component_slot) }.unwrap_or(0);
    let comp_vt = if comp != 0 && comp != null {
        unsafe { safe_read_usize(comp) }.unwrap_or(0)
    } else {
        0
    };
    let slot_fn = if comp_vt != 0 {
        unsafe { safe_read_usize(comp_vt + COMPONENT_GET_VALUE_VTABLE_SLOT_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    if comp_vt != 0 && vtable_in_game_image(comp_vt, base) && vtable_in_game_image(slot_fn, base) {
        Some((out, component_slot))
    } else {
        unsafe { destroy_resolved_row_child_proxy(base, out) };
        None
    }
}

unsafe fn destroy_resolved_row_child_proxy(base: usize, proxy: usize) {
    let dtor: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + CSSCALEFORMVALUE_DTOR_RVA) };
    unsafe { dtor(proxy + SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET) };
    unsafe {
        drop(Box::from_raw(
            proxy as *mut [u8; SCENE_OBJ_PROXY_STACK_BYTES],
        ))
    };
}

unsafe fn scaleform_value_setter_guard(base: usize, cs_value: usize) -> Result<(), String> {
    let datatype = unsafe { safe_read_i32(cs_value + CSSCALEFORMVALUE_DATATYPE_OFFSET) }
        .ok_or_else(|| format!("CSScaleformValue datatype unreadable at 0x{cs_value:x}"))?;
    if (datatype & CSSCALEFORMVALUE_DISPLAY_TYPE_MASK) == 0 {
        return Err(format!(
            "CSScaleformValue at 0x{cs_value:x} has empty datatype {datatype}; live setter skipped"
        ));
    }
    let object_interface =
        unsafe { safe_read_usize(cs_value + CSSCALEFORMVALUE_OBJECT_INTERFACE_OFFSET) }
            .unwrap_or(0);
    let vfptr = if object_interface != 0 {
        unsafe { safe_read_usize(object_interface) }.unwrap_or(0)
    } else {
        0
    };
    let get_display_info = if vfptr != 0 {
        unsafe { safe_read_usize(vfptr + CSSCALEFORMVALUE_GET_DISPLAY_INFO_VTABLE_SLOT) }
            .unwrap_or(0)
    } else {
        0
    };
    if object_interface == 0
        || vfptr == 0
        || !vtable_in_game_image(vfptr, base)
        || get_display_info == 0
        || !vtable_in_game_image(get_display_info, base)
    {
        return Err(format!(
            "CSScaleformValue at 0x{cs_value:x} failed setter guard: datatype={datatype} objectInterface=0x{object_interface:x} vfptr=0x{vfptr:x} getDisplayInfo=0x{get_display_info:x}"
        ));
    }
    Ok(())
}

unsafe fn set_text_field_width_probe(
    base: usize,
    cs_value: usize,
    width_px: i32,
) -> Result<String, String> {
    if !(1..=2000).contains(&width_px) {
        return Err(format!(
            "width {width_px} outside guarded live probe range 1..=2000"
        ));
    }
    let handle = unsafe { safe_read_usize(cs_value + CSSCALEFORMVALUE_HANDLE_OFFSET) }.unwrap_or(0);
    if handle == 0 || handle == TITLE_OWNER_SCAN_START_ADDRESS {
        return Err(format!("CSScaleformValue handle empty at 0x{cs_value:x}"));
    }
    let text_object =
        unsafe { safe_read_usize(handle + GFX_VALUE_TEXT_OBJECT_OFFSET) }.unwrap_or(0);
    if text_object == 0 || text_object == TITLE_OWNER_SCAN_START_ADDRESS {
        return Err(format!(
            "GFx value at 0x{handle:x} has no text object at +0x{GFX_VALUE_TEXT_OBJECT_OFFSET:x}"
        ));
    }
    let text_vt = unsafe { safe_read_usize(text_object) }.unwrap_or(0);
    let kind_fn = if text_vt != 0 {
        unsafe { safe_read_usize(text_vt + GFX_TEXT_OBJECT_KIND_VTABLE_SLOT) }.unwrap_or(0)
    } else {
        0
    };
    if text_vt == 0 || !vtable_in_game_image(text_vt, base) {
        return Err(format!(
            "text object vt invalid object=0x{text_object:x} vt=0x{text_vt:x}"
        ));
    }
    if kind_fn == 0 || !vtable_in_game_image(kind_fn, base) {
        return Err(format!(
            "text object kind function invalid object=0x{text_object:x} vt=0x{text_vt:x} kind=0x{kind_fn:x}"
        ));
    }
    let kind: unsafe extern "system" fn(usize) -> i32 = unsafe { std::mem::transmute(kind_fn) };
    let kind = unsafe { kind(text_object) };
    if kind != GFX_TEXT_OBJECT_KIND_TEXT_FIELD {
        return Err(format!(
            "GFx object at 0x{text_object:x} is kind {kind}, not text-field kind {GFX_TEXT_OBJECT_KIND_TEXT_FIELD}"
        ));
    }
    let text_doc =
        unsafe { safe_read_usize(text_object + GFX_TEXT_OBJECT_TEXT_DOC_OFFSET) }.unwrap_or(0);
    if text_doc == 0 || text_doc == TITLE_OWNER_SCAN_START_ADDRESS {
        return Err(format!(
            "text object at 0x{text_object:x} has no text document at +0x{GFX_TEXT_OBJECT_TEXT_DOC_OFFSET:x}"
        ));
    }
    let left = unsafe { safe_read_f32(text_doc + GFX_TEXT_DOC_SOURCE_LEFT_OFFSET) }
        .ok_or_else(|| format!("text doc source left unreadable at 0x{text_doc:x}"))?;
    let old_right = unsafe { safe_read_f32(text_doc + GFX_TEXT_DOC_SOURCE_RIGHT_OFFSET) }
        .ok_or_else(|| format!("text doc source right unreadable at 0x{text_doc:x}"))?;
    let old_layout_left =
        unsafe { safe_read_f32(text_doc + GFX_TEXT_DOC_LAYOUT_LEFT_OFFSET) }.unwrap_or(f32::NAN);
    let old_layout_right =
        unsafe { safe_read_f32(text_doc + GFX_TEXT_DOC_LAYOUT_RIGHT_OFFSET) }.unwrap_or(f32::NAN);
    if !left.is_finite() || !old_right.is_finite() {
        return Err(format!(
            "text doc bounds are not finite: left={left} right={old_right} doc=0x{text_doc:x}"
        ));
    }
    // `left` came out of the text document, whose source bounds are TWIPS; `width_px` is the
    // schema's pixel width. Without the conversion this wrote pixels into a twips field and made
    // the box 20x too narrow: PlayerName's 1200 px became -40 + 1200 = 1160 twips = 58 px instead
    // of -40 + 1200*20 = 23960 twips, which is exactly the x_max the asset generator emits
    // (make_05_010_stats.rs: `bounds.x_max = bounds.x_min + field.width * TW`). The name then
    // word-wrapped inside a 56 px layout area -- observed live as `records=4 content=929x2064`
    // (twips: 46.45 x 103.20 px, i.e. three 34.383 px line boxes) -- and only the first wrapped
    // line fit the field's height, which reads on screen as a truncated name.
    let new_right = left + width_px as f32 * TWIPS_PER_PIXEL_F32;
    unsafe {
        ((text_doc + GFX_TEXT_DOC_SOURCE_RIGHT_OFFSET) as *mut f32).write_unaligned(new_right);
    }
    let reflow: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + GFX_TEXT_DOC_REFLOW_RVA) };
    unsafe { reflow(text_doc) };
    let new_layout_left =
        unsafe { safe_read_f32(text_doc + GFX_TEXT_DOC_LAYOUT_LEFT_OFFSET) }.unwrap_or(f32::NAN);
    let new_layout_right =
        unsafe { safe_read_f32(text_doc + GFX_TEXT_DOC_LAYOUT_RIGHT_OFFSET) }.unwrap_or(f32::NAN);
    let layout_record_count =
        unsafe { safe_read_usize(text_doc + GFX_TEXT_DOC_LAYOUT_RECORD_COUNT_OFFSET) }.unwrap_or(0);
    let content_width =
        unsafe { safe_read_i32(text_doc + GFX_TEXT_DOC_CONTENT_WIDTH_OFFSET) }.unwrap_or(-1);
    let content_height =
        unsafe { safe_read_i32(text_doc + GFX_TEXT_DOC_CONTENT_HEIGHT_OFFSET) }.unwrap_or(-1);
    Ok(format!(
        // Every one of these is TWIPS. The old message labelled content "px", which cost real
        // diagnosis time: `content=929x2064px` reads as an absurd 2064-pixel-tall line, whereas
        // 2064 twips = 103.20 px = exactly three line boxes, which is what pointed at wrapping.
        "doc=0x{text_doc:x} source_right {old_right:.2}->{new_right:.2}tw layout [{old_layout_left:.2},{old_layout_right:.2}]->[{new_layout_left:.2},{new_layout_right:.2}]tw render_layout records={layout_record_count} content={content_width}x{content_height}tw ({:.2}x{:.2}px)",
        content_width as f32 / TWIPS_PER_PIXEL_F32,
        content_height as f32 / TWIPS_PER_PIXEL_F32,
    ))
}

unsafe fn set_scaleform_value_position(base: usize, cs_value: usize, x: f32, y: f32) -> bool {
    let set_position: unsafe extern "system" fn(usize, f32, f32) -> usize =
        unsafe { std::mem::transmute(base + TITLE_GFX_VALUE_SET_POSITION_RVA) };
    (unsafe { set_position(cs_value, x, y) }) != 0
}

unsafe fn set_scaleform_value_scale(
    base: usize,
    cs_value: usize,
    x_percent: f32,
    y_percent: f32,
) -> bool {
    let set_scale: unsafe extern "system" fn(usize, *const f32) -> usize =
        unsafe { std::mem::transmute(base + TITLE_GFX_VALUE_SET_SCALE_RVA) };
    let scale = [x_percent, y_percent];
    (unsafe { set_scale(cs_value, scale.as_ptr()) }) != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use er_gfx::profile_05_010_protocol::{ProfileEditorCommand, RenderMode, SelectedKind};

    #[test]
    fn current_path_focus_uses_the_path_button_bounds_not_a_drive_cell() {
        let layout = er_gfx::profile_05_010_layout::Profile05_010Layout::default();
        let field = layout.field("CurrentPath");
        let button = &layout.row_chrome.path_button;
        let button_transform = current_path_button_transform(&layout);
        let transform = current_path_cursor_transform_for_layout(&layout);
        assert_eq!(
            transform.x,
            field.x - 2.0 + field.width as f32 * 0.5 + button.x
        );
        assert_eq!(
            transform.y,
            field.y - 2.0 + field.clip_height as f32 * 0.5 + button.y
        );
        assert_eq!(transform.x, button_transform.x);
        assert_eq!(transform.y, button_transform.y);
        assert_eq!(
            transform.scale_x * layout.row_chrome.cursor_body.scale_x,
            button_transform.scale_x
        );
        assert_eq!(
            transform.scale_y * layout.row_chrome.cursor_body.scale_y,
            button_transform.scale_y
        );
        assert!(transform.scale_x > drive_cell_cursor_transform(0).scale_x);
    }

    #[test]
    fn current_path_button_width_is_derived_from_current_path_field_width() {
        use er_gfx::title_05_010::DRIVE_BUTTON_NATIVE_ART_WIDTH_PX;

        let mut layout = er_gfx::profile_05_010_layout::Profile05_010Layout::default();
        layout.fields.get_mut("CurrentPath").unwrap().width = 500;
        layout.row_chrome.path_button.scale_x = 1.0;
        let transform = current_path_button_transform(&layout);
        assert_eq!(transform.scale_x * DRIVE_BUTTON_NATIVE_ART_WIDTH_PX, 500.0);
    }

    #[test]
    fn drive_button_commands_wait_for_the_picker_owned_drive_row() {
        let layout = er_gfx::profile_05_010_layout::Profile05_010Layout::default();
        let drive_button = ProfileEditorCommand::from_layout(
            1,
            RenderMode::LiveRuntime,
            SelectedKind::Chrome,
            "drive_button",
            layout.clone(),
        );
        let path_button = ProfileEditorCommand::from_layout(
            2,
            RenderMode::LiveRuntime,
            SelectedKind::Chrome,
            "path_button",
            layout.clone(),
        );
        let backing = ProfileEditorCommand::from_layout(
            3,
            RenderMode::LiveRuntime,
            SelectedKind::Chrome,
            "backing",
            layout,
        );
        assert!(command_targets_drive_row(&drive_button));
        assert!(command_targets_drive_row(&path_button));
        assert!(!command_targets_drive_row(&backing));
    }

    #[test]
    fn live_command_status_serializes_ack_and_surface() {
        let command = ProfileEditorCommand::from_layout(
            44,
            RenderMode::LiveRuntime,
            SelectedKind::Chrome,
            "cursor",
            er_gfx::profile_05_010_layout::Profile05_010Layout::default(),
        );
        let status = status_for(&command, "row-populate", 0, 1, "setter missing");
        let parsed =
            er_gfx::profile_05_010_protocol::ProfileEditorStatus::parse(&status.serialize())
                .expect("status round trips");
        assert!(parsed.connected);
        assert_eq!(parsed.ack_sequence, 44);
        assert_eq!(parsed.active_surface, "row-populate");
        assert_eq!(parsed.selected_kind, "chrome");
        assert_eq!(parsed.selected_name, "cursor");
        assert_eq!(parsed.unsupported_count, 1);
    }
}
