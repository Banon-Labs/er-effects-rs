use super::*;
use er_gfx::profile_05_010_protocol::{
    CONTROL_FILE_NAME, ProfileEditorCommand, ProfileEditorStatus, RenderMode, STATUS_FILE_NAME,
    SelectedKind,
};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;

static PROFILE_EDITOR_LAST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static PROFILE_EDITOR_STATUS_THROTTLE: AtomicU64 = AtomicU64::new(0);

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
    let scaled = unsafe {
        set_scaleform_value_scale(
            base,
            cs_value,
            transform.scale_x * 100.0,
            transform.scale_y * 100.0,
        )
    };
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
            let cs_value = child_proxy + SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET;
            let live = unsafe { scaleform_value_setter_guard(base, cs_value) };
            let (moved, error) = if live.is_ok() {
                let moved = unsafe {
                    set_scaleform_value_position(base, cs_value, field.x as f32, field.y as f32)
                };
                (moved, String::new())
            } else {
                (false, live.unwrap_err())
            };
            unsafe { destroy_resolved_row_child_proxy(base, child_proxy) };
            let sample_text_pushed =
                if command.text_probe && !field.sample_load_character.is_empty() {
                    let utf16: Vec<u16> = field
                        .sample_load_character
                        .encode_utf16()
                        .chain(core::iter::once(0))
                        .collect();
                    let mut field_name_nul = String::with_capacity(field_name.len() + 1);
                    field_name_nul.push_str(field_name);
                    field_name_nul.push('\0');
                    unsafe {
                        crate::experiments::startup_hooks::loading_cover::push_stats_text_on_row(
                            base,
                            row_proxy,
                            &field_name_nul,
                            &utf16,
                        )
                    }
                } else {
                    false
                };
            let applied = moved as u32 + sample_text_pushed as u32;
            let unsupported = if moved { 0 } else { 1 };
            (
                applied,
                unsupported,
                if error.is_empty() {
                    format!(
                        "field {field_name} live x/y probe moved={moved} text_probe={} sample_text_pushed={sample_text_pushed}; width/font/align/static text definition remain asset-level hot-reload controls",
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
