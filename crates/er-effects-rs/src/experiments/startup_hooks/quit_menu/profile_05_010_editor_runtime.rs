use super::*;
use er_gfx::TWIPS_PER_PIXEL_F32;
use er_gfx::profile_05_010_protocol::{
    CONTROL_FILE_NAME, ProfileEditorCommand, ProfileEditorStatus, RenderMode, STATUS_FILE_NAME,
    SelectedKind,
};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;

static PROFILE_EDITOR_LAST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static PROFILE_EDITOR_STATUS_THROTTLE: AtomicU64 = AtomicU64::new(0);
static PROFILE_EDITOR_NECROMANCY_POLL_TICKS: AtomicU64 = AtomicU64::new(0);
static PROFILE_EDITOR_FIELD_TARGETS: OnceLock<Mutex<Vec<CachedProfileFieldTarget>>> =
    OnceLock::new();

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

fn write_status(dir: &PathBuf, status: ProfileEditorStatus) {
    let _ = std::fs::create_dir_all(dir);
    let tmp = dir.join(format!("{STATUS_FILE_NAME}.tmp"));
    let final_path = dir.join(STATUS_FILE_NAME);
    if std::fs::write(&tmp, status.serialize()).is_ok() {
        let _ = std::fs::rename(tmp, final_path);
    }
}

fn read_command(dir: &PathBuf) -> Result<Option<ProfileEditorCommand>, String> {
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
pub(crate) unsafe fn profile_editor_necromancy_tick(base: usize) {
    let Some(dir) = editor_dir() else {
        return;
    };
    let tick = PROFILE_EDITOR_NECROMANCY_POLL_TICKS.fetch_add(1, Ordering::SeqCst) + 1;
    if tick % 15 != 0 {
        return;
    }
    let command = match read_command(&dir) {
        Ok(Some(command)) => command,
        Ok(None) => return,
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
        return;
    }
    if PROFILE_EDITOR_LAST_SEQUENCE.load(Ordering::SeqCst) == command.sequence {
        return;
    }
    if command.selected_kind != SelectedKind::Field {
        write_status(
            &dir,
            status_for(
                &command,
                "necromancy",
                0,
                1,
                "necromancy currently reanimates cached text fields only; row/list/chrome still need a live row-populate resolve",
            ),
        );
        return;
    }
    let target = PROFILE_EDITOR_FIELD_TARGETS
        .get()
        .and_then(|targets| targets.lock().ok())
        .and_then(|targets| {
            targets
                .iter()
                .find(|target| target.field_name == command.selected_name)
                .cloned()
        });
    let Some(target) = target else {
        write_status(
            &dir,
            status_for(
                &command,
                "necromancy",
                0,
                1,
                format!(
                    "no cached live field component for {}; open/repopulate ProfileSelect once, then live edits can reanimate it without another row populate",
                    command.selected_name
                ),
            ),
        );
        return;
    };
    let (applied, unsupported, detail) =
        unsafe { apply_profile_editor_cached_field(base, &target, &command) };
    PROFILE_EDITOR_LAST_SEQUENCE.store(command.sequence, Ordering::SeqCst);
    write_status(
        &dir,
        status_for(
            &command,
            &format!("{}+necromancy", target.active_surface),
            applied,
            unsupported,
            detail,
        ),
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
    PROFILE_EDITOR_LAST_SEQUENCE.store(command.sequence, Ordering::SeqCst);
    let (applied, unsupported, error) =
        unsafe { apply_profile_editor_command(base, row_proxy, row_model, native_slot, &command) };
    write_status(
        &dir,
        status_for(&command, active_surface, applied, unsupported, error),
    );
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
    match command.selected_kind {
        SelectedKind::Field => unsafe {
            apply_profile_editor_field_probe(base, row_proxy, row_model, native_slot, command)
        },
        SelectedKind::Chrome => unsafe {
            apply_profile_editor_chrome_probe(base, row_proxy, command)
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

unsafe fn apply_profile_editor_field_probe(
    base: usize,
    row_proxy: usize,
    _row_model: usize,
    native_slot: i32,
    command: &ProfileEditorCommand,
) -> (u32, u32, String) {
    let field_name = command.selected_name.as_str();
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
                let moved = unsafe {
                    set_scaleform_value_position(base, cs_value, field.x as f32, field.y as f32)
                };
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

unsafe fn apply_profile_editor_cached_field(
    base: usize,
    target: &CachedProfileFieldTarget,
    command: &ProfileEditorCommand,
) -> (u32, u32, String) {
    let Some(field) = command.layout.fields.get(target.field_name.as_str()) else {
        return (
            0,
            1,
            format!("missing cached field layout {}", target.field_name),
        );
    };
    let cs_value = match unsafe { scaleform_value_for_component(base, target.component) } {
        Ok(value) => value,
        Err(error) => {
            return (
                0,
                1,
                format!(
                    "cached field {} component is no longer live: {error}",
                    target.field_name
                ),
            );
        }
    };
    let moved = unsafe { set_scaleform_value_position(base, cs_value, field.x, field.y) };
    let mut component_slot = target.component;
    let settext: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(base + PROFILE_SETTEXT_RVA) };
    let mut field_name_nul = String::with_capacity(target.field_name.len() + 1);
    field_name_nul.push_str(&target.field_name);
    field_name_nul.push('\0');
    let live_text =
        crate::experiments::startup_hooks::loading_cover::profile_editor_live_text_for_field(
            &field_name_nul,
            &target.last_utf16,
        );
    unsafe {
        settext(
            (&mut component_slot as *mut usize) as usize,
            live_text.as_ptr() as usize,
        )
    };
    let text_pushed = true;
    let width_result = unsafe { set_text_field_width_probe(base, cs_value, field.width) };
    let width_applied = width_result.is_ok();
    let width_detail = width_result
        .as_ref()
        .map(|detail| detail.as_str())
        .unwrap_or_else(|error| error.as_str());
    let applied = moved as u32 + width_applied as u32 + text_pushed as u32;
    let unsupported = (!moved) as u32 + (!width_applied) as u32 + (!text_pushed) as u32;
    (
        applied,
        unsupported,
        format!(
            "necromancy cached field {} component=0x{:x}: moved={moved}; width_probe={width_applied} ({width_detail}); text_repush={text_pushed}; no row-populate required",
            target.field_name, target.component
        ),
    )
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
