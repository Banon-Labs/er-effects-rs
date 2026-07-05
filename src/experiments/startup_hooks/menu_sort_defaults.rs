pub(crate) fn apply_default_menu_sort_preferences_once() {
    if MENU_SORT_DEFAULTS_APPLIED_STATE.load(Ordering::SeqCst) == MENU_SORT_DEFAULTS_APPLIED {
        return;
    }
    let Ok(base) = game_module_base() else {
        return;
    };
    let Some(menu_system_save_load) = (unsafe { resolve_menu_system_save_load(base) }) else {
        return;
    };

    let configured_defaults = [
        (
            "armaments",
            MENU_SORT_TYPE_ARMAMENTS,
            configured_menu_sort_armaments(),
        ),
        ("armor", MENU_SORT_TYPE_ARMOR, configured_menu_sort_armor()),
        (
            "talismans",
            MENU_SORT_TYPE_TALISMANS,
            configured_menu_sort_talismans(),
        ),
    ];

    let mut changed = 0usize;
    let mut already = 0usize;
    let mut skipped = 0usize;
    for (label, sort_type, configured_default) in configured_defaults {
        let Some(target_value) = menu_sort_default_value(configured_default) else {
            skipped += 1;
            append_autoload_debug(format_args!(
                "menu-sort-defaults: preserve configured category={label} sort_type={sort_type}"
            ));
            continue;
        };
        let target_id = target_value & MENU_SORT_ID_MASK;
        let addr = menu_system_save_load
            + MENU_SORT_STATE_ARRAY_OFFSET
            + sort_type * MENU_SORT_STATE_ENTRY_SIZE;
        let Some(current) = (unsafe { safe_read_i32(addr) }) else {
            append_autoload_debug(format_args!(
                "menu-sort-defaults: deferred; could not read sort_type={sort_type} addr=0x{addr:x}"
            ));
            return;
        };
        let current_u32 = current as u32;
        let current_id = current_u32 & MENU_SORT_ID_MASK;
        if current_id == target_id {
            already += 1;
            continue;
        }
        if current_id != MENU_SORT_ITEM_TYPE_ID {
            skipped += 1;
            append_autoload_debug(format_args!(
                "menu-sort-defaults: preserve user/non-item category={label} sort_type={sort_type} value=0x{current_u32:x} configured={}",
                configured_default.label()
            ));
            continue;
        }

        unsafe {
            (addr as *mut u32).write_volatile(target_value);
        }
        changed += 1;
    }

    MENU_SORT_DEFAULTS_APPLIED_STATE.store(MENU_SORT_DEFAULTS_APPLIED, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "menu-sort-defaults: applied defaults mss=0x{menu_system_save_load:x} changed={changed} already={already} skipped={skipped} targets={:?}",
        configured_defaults
    ));
}

fn menu_sort_default_value(configured_default: MenuSortDefault) -> Option<u32> {
    match configured_default {
        MenuSortDefault::Preserve => None,
        MenuSortDefault::ItemType => Some(MENU_SORT_DIRECTION_FLAG | MENU_SORT_ITEM_TYPE_ID),
        MenuSortDefault::OrderOfAcquisition => {
            Some(MENU_SORT_DIRECTION_FLAG | MENU_SORT_ORDER_OF_ACQUISITION_ID)
        }
    }
}
