//! Resolving, writing and destroying a `CS::SceneObjProxy` over a live Scaleform display object.
//!
//! Moved out of `er-quickload`'s `quit_menu/profile_05_010_editor_runtime.rs`, which keeps the
//! ProfileSelect browse surface and now calls these through here. They are shared primitives rather
//! than a feature: three surfaces drive them (the loading-screen stats text, the ProfileSelect
//! editor and the System>Quit link field), and the third is the one that has to work with no
//! product DLL behind it.
//!
//! # The pair of proxies, which is what the crash was about
//!
//! Every resolve boxes `SCENE_OBJ_PROXY_STACK_BYTES` of scratch, hands it to the native
//! `assignComponentWithName`, and owes a `~CSScaleformValue` on the embedded value plus a `Box`
//! free afterwards. Destroying the wrong offset stamps a vtable over the component-link node, and
//! doing it on only one of a nested pair leaks a GFx handle per frame in a per-frame caller such as
//! the live clipboard mirror. [`with_text_input_02_990_field`] exists so that sequence is written
//! exactly once.
//!
//! # Why every call is guarded before it is dispatched
//!
//! A destructed component passes the obvious checks: its vtable is the abstract base's, whose slots
//! hold `_purecall` -- an address inside the game image, so "the pointer looks like game code" says
//! nothing. Calling it writes `0xdead` to a null pointer and takes the process with it. Hence
//! [`dispatch_target_is_purecall`] on the slot before every virtual dispatch.

use std::sync::atomic::{AtomicUsize, Ordering};

use er_game_base::mem::{
    game_data_addr, game_rva_named, safe_read_i32, safe_read_usize, vtable_in_game_image,
};

use crate::host::append_autoload_debug;

/// A null pointer, named. Same value as the product's `TITLE_OWNER_SCAN_START_ADDRESS`.
const NULL_POINTER: usize = usize::MIN;

/// `CS::SceneObjProxy` layout: the component pointer, the embedded `CSScaleformValue`, and how much
/// stack the native binder writes into.
pub const SCENE_OBJ_PROXY_COMPONENT_SLOT_OFFSET: usize = 0x8;
pub const SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET: usize = 0x28;
pub const SCENE_OBJ_PROXY_STACK_BYTES: usize = 0x80;

/// The `GetValue` slot in a menu component's vtable.
pub const COMPONENT_GET_VALUE_VTABLE_SLOT_OFFSET: usize = 0x8;

/// `CS::CSScaleformValue` layout.
pub const CSSCALEFORMVALUE_OBJECT_INTERFACE_OFFSET: usize = 0x18;
pub const CSSCALEFORMVALUE_DATATYPE_OFFSET: usize = 0x20;
pub const CSSCALEFORMVALUE_HANDLE_OFFSET: usize = 0x28;
/// The display-object bits of the datatype word.
pub const CSSCALEFORMVALUE_DISPLAY_TYPE_MASK: i32 = 0x8f;
/// `getDisplayInfo` in the value's object-interface vtable.
pub const CSSCALEFORMVALUE_GET_DISPLAY_INFO_VTABLE_SLOT: usize = 0xd8;

/// `GFx::Value` layout and the two type tags that mean "this resolved to nothing".
pub const GFX_VALUE_TEXT_OBJECT_OFFSET: usize = 0x88;
pub const GFX_TEXT_OBJECT_KIND_VTABLE_SLOT: usize = 0x290;
pub const GFX_TEXT_OBJECT_KIND_TEXT_FIELD: i32 = 4;
const GFX_VALUE_TYPE_UNDEFINED: usize = 0;
const GFX_VALUE_TYPE_NULL: usize = 1;
/// `SetSelection` clamps both indices to the live text length, so asking for the end is exact
/// rather than a guess, and a collapsed range leaves a caret rather than a selection.
pub const GFX_TEXT_FIELD_SELECTION_END: i64 = i64::MAX;

/// `CS::OptionSettingTopDialog`'s root `SceneObjProxy`, the parent every named child resolves from.
pub const OPTION_SETTING_ROOT_PROXY_OFFSET: usize = 0x188;

/// The product's trampoline for `assignComponentWithName`, when the product has detoured it.
///
/// Zero in a standalone shell, which is the whole reason this is a slot rather than a constant: a
/// shell that never installs that detour must call the game function directly, while the product
/// must call its own trampoline or the resolve re-enters its detour.
static NAMED_CHILD_BIND_TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);

/// Publish the host's `assignComponentWithName` trampoline. Called by a host that detours it.
pub fn set_named_child_bind_trampoline(address: usize) {
    NAMED_CHILD_BIND_TRAMPOLINE.store(address, Ordering::SeqCst);
}

/// A verified address for `rva` on the running build, or `None`.
///
/// Every direct call goes through here for the reason `er-hook` refuses an unrecognised build: a
/// hand-built `base + rva` asks nothing, and on a build where the function moved it transfers
/// control into the middle of an unrelated one.
fn gated_game_fn(rva: usize, what: &'static str) -> Option<usize> {
    game_rva_named(rva as u32, what).ok()
}

/// Did a `CSScaleformValue` resolve to a real display object?
pub fn gfx_value_type_is_resolved(datatype: usize) -> bool {
    datatype != GFX_VALUE_TYPE_UNDEFINED && datatype != GFX_VALUE_TYPE_NULL
}

/// Is `target` one of the two pure-virtual traps a destructed object's vtable slot points at?
pub fn dispatch_target_is_purecall(target: usize, base: usize) -> bool {
    if target == 0 {
        return false;
    }
    let purecall = game_data_addr(base, er_game_base::rva::PURECALL_RVA, "PURECALL_RVA");
    let crash_handler = game_data_addr(
        base,
        er_game_base::rva::PURECALL_CRASH_HANDLER_RVA,
        "PURECALL_CRASH_HANDLER_RVA",
    );
    target == purecall || target == crash_handler
}

/// Resolve a named child of `row_proxy` into a freshly boxed proxy.
///
/// Returns the proxy and its component slot, or `None` when the child does not exist or its
/// component is not live. A `None` has already freed whatever it allocated; a `Some` owes
/// [`destroy_resolved_row_child_proxy`].
///
/// # Safety
///
/// `row_proxy` must be a live `SceneObjProxy` and the caller must be inside the `MenuWindowJob::Run`
/// context that owns it.
pub unsafe fn resolve_row_child_proxy(
    base: usize,
    row_proxy: usize,
    name: &str,
) -> Option<(usize, usize)> {
    let assign = match NAMED_CHILD_BIND_TRAMPOLINE.load(Ordering::SeqCst) {
        orig if orig != NULL_POINTER => orig,
        _ => game_data_addr(
            base,
            er_game_base::rva::TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA,
            "TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA",
        ),
    };
    let assign: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(assign) };
    let mut nul_name = String::with_capacity(name.len() + 1);
    nul_name.push_str(name);
    nul_name.push('\0');
    let proxy = Box::into_raw(Box::new([0u8; SCENE_OBJ_PROXY_STACK_BYTES])) as usize;
    let out = unsafe { assign(row_proxy, proxy, nul_name.as_ptr() as usize) };
    if out == 0 || out == NULL_POINTER {
        unsafe {
            drop(Box::from_raw(
                proxy as *mut [u8; SCENE_OBJ_PROXY_STACK_BYTES],
            ))
        };
        return None;
    }
    let component_slot = out + SCENE_OBJ_PROXY_COMPONENT_SLOT_OFFSET;
    let comp = unsafe { safe_read_usize(component_slot) }.unwrap_or(0);
    let comp_vt = if comp != 0 && comp != NULL_POINTER {
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

/// Destruct the embedded `CSScaleformValue` and free the box a resolve allocated.
///
/// # Safety
///
/// `proxy` must be the pointer a [`resolve_row_child_proxy`] returned, destroyed exactly once.
pub unsafe fn destroy_resolved_row_child_proxy(_base: usize, proxy: usize) {
    let dtor: unsafe extern "system" fn(usize) = unsafe {
        std::mem::transmute(
            match gated_game_fn(
                er_game_base::rva::CSSCALEFORMVALUE_DTOR_RVA,
                "CSSCALEFORMVALUE_DTOR_RVA",
            ) {
                Some(address) => address,
                None => return,
            },
        )
    };
    unsafe { dtor(proxy + SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET) };
    unsafe {
        drop(Box::from_raw(
            proxy as *mut [u8; SCENE_OBJ_PROXY_STACK_BYTES],
        ))
    };
}

/// Refuse a `CSScaleformValue` that no live display object stands behind.
///
/// # Safety
///
/// `cs_value` is read through the fault-safe readers, so a wild value is an `Err`.
pub unsafe fn scaleform_value_setter_guard(base: usize, cs_value: usize) -> Result<(), String> {
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

/// The `CSScaleformValue` a component's own `GetValue` hands back, guarded.
///
/// # Safety
///
/// `comp` must be a live menu component.
pub unsafe fn scaleform_value_for_component(base: usize, comp: usize) -> Result<usize, String> {
    if comp == 0 || comp == NULL_POINTER {
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
    if dispatch_target_is_purecall(get_value, base) {
        return Err(format!(
            "component 0x{comp:x} has been DESTROYED (vt=0x{comp_vt:x} get-value is the pure-virtual trap 0x{get_value:x}); its screen is gone"
        ));
    }
    let get_value: unsafe extern "system" fn(usize) -> usize =
        unsafe { std::mem::transmute(get_value) };
    let value = unsafe { get_value(comp) };
    if value == 0 || value == NULL_POINTER {
        return Err(format!(
            "component get-value returned empty comp=0x{comp:x} vt=0x{comp_vt:x}"
        ));
    }
    unsafe { scaleform_value_setter_guard(base, value) }
        .map_err(|e| format!("component value guard failed at 0x{value:x}: {e}"))?;
    Ok(value)
}

/// [`scaleform_value_for_component`] reached through a proxy's component slot.
///
/// # Safety
///
/// `proxy` must be a live `SceneObjProxy`.
pub unsafe fn component_scaleform_value_for_setter(
    base: usize,
    proxy: usize,
) -> Result<usize, String> {
    let component_slot = proxy + SCENE_OBJ_PROXY_COMPONENT_SLOT_OFFSET;
    let comp = unsafe { safe_read_usize(component_slot) }.unwrap_or(0);
    if comp == 0 || comp == NULL_POINTER {
        return Err(format!("component pointer empty at 0x{component_slot:x}"));
    }
    unsafe { scaleform_value_for_component(base, comp) }
}

/// Move a resolved display object.
///
/// # Safety
///
/// `cs_value` must have passed [`scaleform_value_setter_guard`].
pub unsafe fn set_scaleform_value_position(_base: usize, cs_value: usize, x: f32, y: f32) -> bool {
    let set_position: unsafe extern "system" fn(usize, f32, f32) -> usize = unsafe {
        std::mem::transmute(
            match gated_game_fn(
                er_game_base::rva::TITLE_GFX_VALUE_SET_POSITION_RVA,
                "TITLE_GFX_VALUE_SET_POSITION_RVA",
            ) {
                Some(address) => address,
                None => return false,
            },
        )
    };
    (unsafe { set_position(cs_value, x, y) }) != 0
}

/// Scale a resolved display object.
///
/// The native setter converts to Scaleform's percent space itself, so the schema's unit factor is
/// passed straight through. Multiplying by 100 here as well applied every live chrome scale a
/// hundred times too large.
///
/// # Safety
///
/// `cs_value` must have passed [`scaleform_value_setter_guard`].
pub unsafe fn set_scaleform_value_scale(
    _base: usize,
    cs_value: usize,
    x_percent: f32,
    y_percent: f32,
) -> bool {
    let set_scale: unsafe extern "system" fn(usize, *const f32) -> usize = unsafe {
        std::mem::transmute(
            match gated_game_fn(
                er_game_base::rva::TITLE_GFX_VALUE_SET_SCALE_RVA,
                "TITLE_GFX_VALUE_SET_SCALE_RVA",
            ) {
                Some(address) => address,
                None => return false,
            },
        )
    };
    let scale = [x_percent, y_percent];
    (unsafe { set_scale(cs_value, scale.as_ptr()) }) != 0
}

/// Position and scale one proxy from a layout transform, reporting what applied.
///
/// # Safety
///
/// `proxy` must be a live `SceneObjProxy` inside its window's run context.
pub unsafe fn apply_transform_to_proxy(
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
    let scaled =
        unsafe { set_scaleform_value_scale(base, cs_value, transform.scale_x, transform.scale_y) };
    (
        moved as u32 + scaled as u32,
        if moved && scaled { 0 } else { 1 },
        format!("{guard_note}value_source={source} moved={moved} scaled={scaled}"),
    )
}

/// Resolve the live editable field by its authored name (`root -> TextInput -> Text_0`) through the
/// same native binder the stats push uses, hand the resolved field proxy to `apply`, and destroy
/// both proxies afterwards however `apply` went.
///
/// # Safety
///
/// `menu_window` must be a live 02_990 `MenuWindow`, and the caller must be in its own
/// `MenuWindowJob::Run` context -- the proxies resolved here are only valid while that window is
/// running.
pub unsafe fn with_text_input_02_990_field<T>(
    base: usize,
    menu_window: usize,
    apply: impl FnOnce(usize) -> Result<T, String>,
) -> Result<T, String> {
    if menu_window == 0 || menu_window == NULL_POINTER {
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
            let outcome = apply(field_proxy);
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

/// Replace the open field's text with `utf16` (NUL-terminated) through the game's own `SetText`,
/// then leave the caret at the end of what was written.
///
/// # Why the engine's setter and not a write into the field's buffer
///
/// The visible field is the same object the accept reads back: the Scaleform half of the software
/// keyboard hands the confirmed text to the job as a `wchar_t const*` taken from this field, which
/// the job copies into the controller's `DLString`. So writing the field through the engine's own
/// setter changes what is drawn and what is accepted in one move, with no second source of truth
/// to keep in step. A memcpy into the text buffer would change neither reliably: the field
/// re-lays-out from its text document, and the editor kit's caret indices would still point into
/// the old length.
///
/// # Safety
///
/// 02_990 `MenuWindowJob::Run` context, `menu_window` live. `utf16` must be NUL-terminated.
pub unsafe fn set_text_input_02_990_text(
    base: usize,
    menu_window: usize,
    utf16: &[u16],
) -> Result<String, String> {
    if utf16.last() != Some(&0) {
        return Err("field text is not NUL-terminated".to_owned());
    }
    let apply = |field_proxy: usize| -> Result<String, String> {
        unsafe { push_text_on_resolved_02_990_field(base, field_proxy, utf16) }?;
        // Typing continues where the text ends, not in front of it: the field's own caret stays at
        // 0 across a text change, so without this the next keystroke prepends to the link.
        let caret = unsafe {
            set_text_field_caret_to_end(base, field_proxy + SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET)
        };
        Ok(match caret {
            Ok(detail) => format!("units={} {detail}", utf16.len().saturating_sub(1)),
            Err(error) => format!(
                "units={} text set but caret stayed put: {error}",
                utf16.len().saturating_sub(1)
            ),
        })
    };
    unsafe { with_text_input_02_990_field(base, menu_window, apply) }
}

/// The native `SetText` call itself, on an already-resolved field proxy.
///
/// # Safety
///
/// `field_proxy` must come from [`with_text_input_02_990_field`] and still be live.
unsafe fn push_text_on_resolved_02_990_field(
    base: usize,
    field_proxy: usize,
    utf16: &[u16],
) -> Result<(), String> {
    let component_slot = field_proxy + SCENE_OBJ_PROXY_COMPONENT_SLOT_OFFSET;
    let comp = unsafe { safe_read_usize(component_slot) }.unwrap_or(0);
    if comp == 0 || comp == NULL_POINTER {
        return Err(format!("component pointer empty at 0x{component_slot:x}"));
    }
    let comp_vt = unsafe { safe_read_usize(comp) }.unwrap_or(0);
    let slot_fn = if comp_vt != 0 {
        unsafe { safe_read_usize(comp_vt + COMPONENT_GET_VALUE_VTABLE_SLOT_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    // A named-child resolve that missed still hands back a proxy with a game-image vtable, so the
    // datatype word is what says a field is really behind it.
    let resolved = unsafe {
        safe_read_i32(
            field_proxy + SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET + CSSCALEFORMVALUE_DATATYPE_OFFSET,
        )
    }
    .map(|raw| (raw as u32 & 0x8f) as usize)
    .is_some_and(gfx_value_type_is_resolved);
    if !resolved {
        return Err(format!(
            "field proxy 0x{field_proxy:x} carries no resolved GFx value"
        ));
    }
    if !vtable_in_game_image(comp_vt, base) || !vtable_in_game_image(slot_fn, base) {
        return Err(format!(
            "component not live comp=0x{comp:x} vt=0x{comp_vt:x} slot_fn=0x{slot_fn:x}"
        ));
    }
    if dispatch_target_is_purecall(slot_fn, base) {
        return Err(format!(
            "component 0x{comp:x} has been DESTROYED (GetValue slot is the pure-virtual trap 0x{slot_fn:x})"
        ));
    }
    let Some(settext_addr) = gated_game_fn(
        er_game_base::rva::PROFILE_SETTEXT_RVA,
        "PROFILE_SETTEXT_RVA",
    ) else {
        return Err("SetText has no verified address for this build".to_owned());
    };
    let settext: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(settext_addr) };
    unsafe { settext(component_slot, utf16.as_ptr() as usize) };
    Ok(())
}

/// Put the caret at the end of whatever the open field currently holds.
///
/// Window-generic on purpose: both fields that load `02_990` want this, and only the placement of
/// the window differs between them. Scoping it to the save picker is why the build-url field opened
/// with its caret at index 0 and typing prepended to the prefilled link.
///
/// # Safety
///
/// 02_990 `MenuWindowJob::Run` context, `menu_window` live.
pub unsafe fn place_text_input_02_990_caret_at_end(
    base: usize,
    menu_window: usize,
) -> Result<String, String> {
    let apply = |field_proxy: usize| -> Result<String, String> {
        unsafe {
            set_text_field_caret_to_end(base, field_proxy + SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET)
        }
    };
    unsafe { with_text_input_02_990_field(base, menu_window, apply) }
}

/// Move a resolved field's caret to the end of its text.
///
/// Guarded exactly like the native text helpers: the value must carry a text object, whose vtable
/// and runtime type tag must both check out before anything is handed to a native call.
///
/// # Safety
///
/// `cs_value` must be the embedded value of a live, resolved field proxy.
pub unsafe fn set_text_field_caret_to_end(base: usize, cs_value: usize) -> Result<String, String> {
    let handle = unsafe { safe_read_usize(cs_value + CSSCALEFORMVALUE_HANDLE_OFFSET) }.unwrap_or(0);
    if handle == 0 || handle == NULL_POINTER {
        return Err(format!("CSScaleformValue handle empty at 0x{cs_value:x}"));
    }
    let text_object =
        unsafe { safe_read_usize(handle + GFX_VALUE_TEXT_OBJECT_OFFSET) }.unwrap_or(0);
    if text_object == 0 || text_object == NULL_POINTER {
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
    let Some(set_selection_addr) = gated_game_fn(
        er_game_base::rva::GFX_TEXT_FIELD_SET_SELECTION_RVA,
        "GFX_TEXT_FIELD_SET_SELECTION_RVA",
    ) else {
        return Err("GFx SetSelection has no verified address for this build".to_owned());
    };
    let set_selection: unsafe extern "system" fn(usize, i64, i64) =
        unsafe { std::mem::transmute(set_selection_addr) };
    unsafe {
        set_selection(
            text_object,
            GFX_TEXT_FIELD_SELECTION_END,
            GFX_TEXT_FIELD_SELECTION_END,
        )
    };
    Ok(format!("text object=0x{text_object:x}"))
}

// ---- the link field's own window placement ------------------------------------------------

/// Frames of one open field over which the end-caret pass runs.
///
/// The field is not guaranteed to be focused on the frame the window first runs, and taking focus
/// is what would reset a caret set too early, so the request is repeated over a short window. It
/// stays far shorter than any human can type into a box that has only just appeared, so it can
/// never fight the user's own Home or End.
const BUILD_URL_CARET_APPLY_FRAMES: usize = 8;
static BUILD_URL_CARET_APPLIES: AtomicUsize = AtomicUsize::new(0);
static BUILD_URL_CARET_RESOLVED: AtomicUsize = AtomicUsize::new(0);
static BUILD_URL_WINDOW_POSITION_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
static BUILD_URL_WINDOW_POSITION_SUCCESSES: AtomicUsize = AtomicUsize::new(0);

/// Re-arm both per-open latches for a newly opened link field: the end-caret pass and the window
/// placement's attempt counter.
///
/// Keyed off the window's open transition rather than a window pointer, because the allocator
/// reuses that address across opens and a pointer-keyed latch would silently skip every field
/// after the first.
///
/// Both counters are per-open and neither used to be reset, because the only caller reached for
/// the save picker's [`crate::host::reset_path_editor_caret_latch`] instead -- a different editor's
/// latch, and in a standalone shell with no host a no-op. The link field therefore got its caret
/// moved to the end exactly once per process: on the second field of a session
/// `BUILD_URL_CARET_APPLIES` was already past [`BUILD_URL_CARET_APPLY_FRAMES`], so the caret stayed
/// at index 0 and typing prepended to the prefilled link. `BUILD_URL_WINDOW_POSITION_ATTEMPTS` has
/// the same shape with a smaller blast radius: it only gates how many placement lines reach the
/// log, so past the first field the placement trace went silent.
pub fn reset_build_url_field_latches() {
    BUILD_URL_CARET_APPLIES.store(0, Ordering::SeqCst);
    BUILD_URL_CARET_RESOLVED.store(0, Ordering::SeqCst);
    BUILD_URL_WINDOW_POSITION_ATTEMPTS.store(0, Ordering::SeqCst);
    BUILD_URL_WINDOW_POSITION_SUCCESSES.store(0, Ordering::SeqCst);
}

/// Count one placement pass without touching game memory, so a test can prove the per-open latch
/// resets. The live counter is bumped inside [`apply_build_url_editor_window_position`], which
/// writes a transform through a live `SceneObjProxy` and cannot run off a game thread.
#[cfg(test)]
pub(crate) fn note_build_url_window_position_attempt_for_test() {
    BUILD_URL_WINDOW_POSITION_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
}

/// How many placement attempts succeeded, for the telemetry line a run reads back.
pub fn build_url_window_position_counts() -> (usize, usize) {
    (
        BUILD_URL_WINDOW_POSITION_ATTEMPTS.load(Ordering::SeqCst),
        BUILD_URL_WINDOW_POSITION_SUCCESSES.load(Ordering::SeqCst),
    )
}

/// Centre the System>Quit link field's own 02_990 `MenuWindow` on the stage, then re-end its caret.
///
/// Separate from the save picker's placement because the two fields answer to different geometry:
/// the picker's editor is placed over a ProfileSelect row, while the link field is a modal over the
/// Quit tab and belongs in the middle of the screen. Sharing the picker's helper would have put the
/// link field where a ProfileSelect row is -- which is why the Quit tab shipped with no placement
/// at all, and the field stayed at the movie's authored top-left origin.
///
/// The target comes from [`er_gfx::build_url_02_990::build_url_window_position`], which derives it
/// from the movie's own authored geometry rather than from a tuned constant.
///
/// # Safety
///
/// 02_990 `MenuWindowJob::Run` context, `menu_window` live.
pub unsafe fn apply_build_url_editor_window_position(base: usize, menu_window: usize) {
    if menu_window == 0 || menu_window == NULL_POINTER {
        return;
    }
    let attempt = BUILD_URL_WINDOW_POSITION_ATTEMPTS.fetch_add(1, Ordering::SeqCst) + 1;
    let (x, y) = er_gfx::build_url_02_990::build_url_window_position();
    let transform = er_gfx::profile_05_010_layout::TransformLayout {
        x,
        y,
        scale_x: 1.0,
        scale_y: 1.0,
        opacity: 1.0,
        editable: false,
        source: "native 02_990 MenuWindow root centres the link field on the stage".to_owned(),
    };
    let proxy = menu_window + OPTION_SETTING_ROOT_PROXY_OFFSET;
    let (applied, unsupported, detail) =
        unsafe { apply_transform_to_proxy(base, proxy, &transform, "02_990 build-url window") };
    if applied > 0 {
        BUILD_URL_WINDOW_POSITION_SUCCESSES.fetch_add(1, Ordering::SeqCst);
    }
    if attempt <= 8 || (unsupported > 0 && attempt.is_power_of_two()) {
        append_autoload_debug(format_args!(
            "system-quit-build-url: positioned 02_990 MenuWindow attempt={attempt} window=0x{menu_window:x} proxy=0x{proxy:x} target=({x:.1},{y:.1}) applied={applied} unsupported={unsupported} detail={detail}"
        ));
    }
    unsafe { apply_build_url_caret_to_end(base, menu_window) };
}

/// Put the caret at the end of the prefilled link when the field opens.
///
/// # Safety
///
/// 02_990 `MenuWindowJob::Run` context, `menu_window` live.
unsafe fn apply_build_url_caret_to_end(base: usize, menu_window: usize) {
    let applies = BUILD_URL_CARET_APPLIES.fetch_add(1, Ordering::SeqCst);
    if applies >= BUILD_URL_CARET_APPLY_FRAMES {
        return;
    }
    let outcome = unsafe { place_text_input_02_990_caret_at_end(base, menu_window) };
    // Log the first resolution either way, then stay quiet: this runs every frame of the open
    // window and a per-frame line would bury the rest of the field's trace.
    if BUILD_URL_CARET_RESOLVED
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        match outcome {
            Ok(detail) => append_autoload_debug(format_args!(
                "system-quit-build-url: caret moved to end of the prefilled link window=0x{menu_window:x} {detail}"
            )),
            Err(error) => append_autoload_debug(format_args!(
                "system-quit-build-url: caret stays at the start window=0x{menu_window:x}; {error}"
            )),
        }
    }
}
