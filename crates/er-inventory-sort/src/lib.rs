//! Standalone one-shot inventory sort defaults DLL.
//!
//! This DLL sets Elden Ring's session-local equipment menu sort defaults once
//! per process. It is intentionally separate from the main product DLL: own DLL
//! name, own config file, own log file, and no dependency on save/autoload/render
//! product crates.

// HOST-BUILD HYGIENE. This crate is a windows `cdylib`: on a non-windows host every item
// whose only consumer is `DllMain` or a hook reads as dead, and `[workspace.lints.rust]
// warnings = "deny"` promotes that to a hard compile ERROR -- so `cargo test -p er-inventory-sort`
// failed outright, and its unit tests had therefore never executed in ANY gate. Same fix,
// same reason, as er-save-suppress / er-seamless-bugfixes / er-armament-icons. The shipping
// target is unaffected: this allow does not exist there.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

use std::{
    fmt,
    path::PathBuf,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use er_game_base::{
    log::{append_line, game_directory_path},
    mem::{game_module_base, safe_read_i32, safe_read_usize},
    rva::GAME_DATA_MAN_GLOBAL_RVA,
};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;

const CONFIG_FILE_NAME: &str = "er-inventory-sort.toml";
const LOG_FILE_NAME: &str = "er-inventory-sort.log";

const SORT_ARMAMENTS_ENV: &str = "ER_INVENTORY_SORT_ARMAMENTS";
const SORT_ARMOR_ENV: &str = "ER_INVENTORY_SORT_ARMOR";
const SORT_TALISMANS_ENV: &str = "ER_INVENTORY_SORT_TALISMANS";

/// `GameDataMan -> menuSystemSaveLoad`. Static RE: `GetMenuSystemSaveLoad`
/// returns exactly `GLOBAL_GameDataMan->menuSystemSaveLoad`.
const GAME_DATA_MAN_MENU_SAVELOAD_60_OFFSET: usize = 0x60;

/// `CSMenuSystemSaveLoad::field_0x1440`: session-local remembered sort state array.
/// Static RE: `FUN_1408581c0` resolves the active sort criterion as
/// `field_0x1440[sort_menu_type] & 0x7fffffff`; the sign bit stores reverse/descending.
const MENU_SORT_STATE_ARRAY_OFFSET: usize = 0x1440;
const MENU_SORT_STATE_ENTRY_SIZE: usize = 4;
const MENU_SORT_DIRECTION_FLAG: u32 = 0x8000_0000;
const MENU_SORT_ID_MASK: u32 = 0x7fff_ffff;

/// GR_MenuText 6105 "Item Type" maps to comparator id 0x5141 in the sort-option tables.
const MENU_SORT_ITEM_TYPE_ID: u32 = 0x5141;
/// GR_MenuText 6190 "Order of Acquisition" maps to comparator id 0x5140.
const MENU_SORT_ORDER_OF_ACQUISITION_ID: u32 = 0x5140;

/// Sort-menu table types used by target categories:
/// - 4: Armaments (static MenuEquipTableData row 0x29, label 40550)
/// - 6: Armor (row 0x2a, label 40551; head/chest/arms/legs rows 0x20..0x23)
/// - 9: Talismans (SortMenu option list 0x143b35f50..0x143b35f80)
const MENU_SORT_TYPE_ARMAMENTS: usize = 4;
const MENU_SORT_TYPE_ARMOR: usize = 6;
const MENU_SORT_TYPE_TALISMANS: usize = 9;

const MENU_SORT_DEFAULTS_NOT_APPLIED: usize = 0;
const MENU_SORT_DEFAULTS_APPLIED: usize = 1;

static MENU_SORT_DEFAULTS_APPLIED_STATE: AtomicUsize =
    AtomicUsize::new(MENU_SORT_DEFAULTS_NOT_APPLIED);
static LOG_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
static START: std::sync::Once = std::sync::Once::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MenuSortDefault {
    /// Keep the game's vanilla boot value untouched for this category.
    Preserve,
    /// GR_MenuText 6105 / comparator id 0x5141.
    ItemType,
    /// GR_MenuText 6190 / comparator id 0x5140.
    OrderOfAcquisition,
}

impl MenuSortDefault {
    fn label(self) -> &'static str {
        match self {
            MenuSortDefault::Preserve => "preserve",
            MenuSortDefault::ItemType => "item_type",
            MenuSortDefault::OrderOfAcquisition => "order_of_acquisition",
        }
    }
}

#[derive(Clone, Debug, Default)]
struct RuntimeConfig {
    path: PathBuf,
    armaments: Option<MenuSortDefault>,
    armor: Option<MenuSortDefault>,
    talismans: Option<MenuSortDefault>,
}

fn log_message(args: fmt::Arguments<'_>) {
    let path = game_directory_path()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(LOG_FILE_NAME);
    let seq = LOG_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1;
    append_line(&path, format_args!("[{seq:06}] {args}"));
}

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
///
/// Called by the Windows loader. Do not call directly.
pub unsafe extern "system" fn DllMain(
    _module: *mut core::ffi::c_void,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // FIRST, before anything that can panic. A panic in a cdylib crosses an
        // `extern "system"` boundary and becomes an ABORT, which does not dispatch to a
        // vectored handler -- so `er_crash_logging` writes no record at all and the process
        // just vanishes. This hook is what turns that silence into a file:line. The hook is
        // per-DLL: every cdylib links its own `er-game-base`, so another shell installing it
        // does nothing here. Enforced by `scripts/check-panic-reporter-installed.py`.
        er_game_base::panic_report::report_panics_to("er-inventory-sort", log_message);

        // This DLL installs no detours (it registers a FrameBegin tick), so it has no er-hook
        // dependency -- but it still resolves game addresses, and a refusal is silent HERE unless
        // the sink is installed, because every cdylib links its own copy of er-game-base.
        er_game_base::game_build::set_address_logger(log_message);
        START.call_once(spawn_inventory_sort_task);
    }
    DLL_MAIN_SUCCESS
}

#[cfg(windows)]
fn spawn_inventory_sort_task() {
    let _ = std::thread::Builder::new()
        .name("er-inventory-sort".to_owned())
        .spawn(|| {
            let config = match load_runtime_config() {
                Ok(config) => {
                    log_message(format_args!(
                        "config: loaded '{}' armaments={} armor={} talismans={}",
                        config.path.display(),
                        configured_sort(SORT_ARMAMENTS_ENV, config.armaments).label(),
                        configured_sort(SORT_ARMOR_ENV, config.armor).label(),
                        configured_sort(SORT_TALISMANS_ENV, config.talismans).label()
                    ));
                    config
                }
                Err(err) => {
                    log_message(format_args!("config: {err}; using defaults"));
                    RuntimeConfig::default()
                }
            };

            use eldenring::{
                cs::{CSTaskGroupIndex, CSTaskImp},
                fd4::FD4TaskData,
            };
            use fromsoftware_shared::{FromStatic, SharedTaskImpExt};

            // BOUNDED (2026-08-29): see er_game_base::wait -- the unbounded form of this loop
            // starved the wineserver and hung a boot.
            let Some(task) =
                er_game_base::wait::poll_until(|| unsafe { CSTaskImp::instance() }.ok())
            else {
                return;
            };
            log_message(format_args!(
                "task: CSTaskImp ready; registering FrameBegin tick"
            ));
            task.run_recurring(
                move |_task_data: &FD4TaskData| apply_default_menu_sort_preferences_once(&config),
                CSTaskGroupIndex::FrameBegin,
            );
        });
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_inventory_sort_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}

fn apply_default_menu_sort_preferences_once(config: &RuntimeConfig) {
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
            configured_sort(SORT_ARMAMENTS_ENV, config.armaments),
        ),
        (
            "armor",
            MENU_SORT_TYPE_ARMOR,
            configured_sort(SORT_ARMOR_ENV, config.armor),
        ),
        (
            "talismans",
            MENU_SORT_TYPE_TALISMANS,
            configured_sort(SORT_TALISMANS_ENV, config.talismans),
        ),
    ];

    let mut changed = 0usize;
    let mut already = 0usize;
    let mut skipped = 0usize;
    for (label, sort_type, configured_default) in configured_defaults {
        let addr = menu_system_save_load
            + MENU_SORT_STATE_ARRAY_OFFSET
            + sort_type * MENU_SORT_STATE_ENTRY_SIZE;
        let Some(current) = (unsafe { safe_read_i32(addr) }) else {
            log_message(format_args!(
                "defaults: deferred; could not read sort_type={sort_type} addr=0x{addr:x}"
            ));
            return;
        };
        let current_u32 = current as u32;
        match decide_menu_sort_write(current_u32, configured_default) {
            MenuSortWriteDecision::PreserveConfigured => {
                skipped += 1;
                log_message(format_args!(
                    "defaults: preserve configured category={label} sort_type={sort_type}"
                ));
            }
            MenuSortWriteDecision::AlreadyMatches => {
                already += 1;
            }
            MenuSortWriteDecision::Write(target_value) => {
                unsafe {
                    // The same slot was just read successfully via ReadProcessMemory, and the native
                    // sort-state array is writable session RAM owned by this process.
                    (addr as *mut u32).write_volatile(target_value);
                }
                changed += 1;
                log_message(format_args!(
                    "defaults: write category={label} sort_type={sort_type} value=0x{current_u32:x} -> 0x{target_value:x} configured={}",
                    configured_default.label()
                ));
            }
        }
    }

    MENU_SORT_DEFAULTS_APPLIED_STATE.store(MENU_SORT_DEFAULTS_APPLIED, Ordering::SeqCst);
    log_message(format_args!(
        "defaults: applied mss=0x{menu_system_save_load:x} changed={changed} already={already} skipped={skipped} targets={:?}",
        configured_defaults
    ));
}

unsafe fn resolve_menu_system_save_load(base: usize) -> Option<usize> {
    let gdm = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            GAME_DATA_MAN_GLOBAL_RVA,
            "GAME_DATA_MAN_GLOBAL_RVA",
        ))
    }
    .filter(|&value| value != 0)?;
    unsafe { safe_read_usize(gdm + GAME_DATA_MAN_MENU_SAVELOAD_60_OFFSET) }
        .filter(|&value| value != 0)
}

fn configured_sort(env_name: &str, config_value: Option<MenuSortDefault>) -> MenuSortDefault {
    if let Ok(value) = std::env::var(env_name) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            match parse_menu_sort_default_label(trimmed) {
                Ok(choice) => return choice,
                Err(err) => log_message(format_args!("config: ignoring invalid {env_name}: {err}")),
            }
        }
    }
    config_value.unwrap_or(MenuSortDefault::OrderOfAcquisition)
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MenuSortWriteDecision {
    PreserveConfigured,
    AlreadyMatches,
    Write(u32),
}

fn decide_menu_sort_write(
    current_u32: u32,
    configured_default: MenuSortDefault,
) -> MenuSortWriteDecision {
    let Some(target_value) = menu_sort_default_value(configured_default) else {
        return MenuSortWriteDecision::PreserveConfigured;
    };
    let target_id = target_value & MENU_SORT_ID_MASK;
    let current_id = current_u32 & MENU_SORT_ID_MASK;
    if current_id == target_id {
        return MenuSortWriteDecision::AlreadyMatches;
    }
    MenuSortWriteDecision::Write(target_value)
}

fn load_runtime_config() -> Result<RuntimeConfig, String> {
    let config_dir = game_directory_path()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let path = config_dir.join(CONFIG_FILE_NAME);
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let contents = boilerplate_config();
            if let Err(write_err) = std::fs::write(&path, &contents) {
                log_message(format_args!(
                    "config: default '{}' missing and could not be created: {write_err}; using defaults for this run",
                    path.display()
                ));
                return Ok(RuntimeConfig {
                    path,
                    ..RuntimeConfig::default()
                });
            }
            contents
        }
        Err(err) => return Err(format!("config '{}' is unreadable: {err}", path.display())),
    };
    parse_runtime_config(path, &contents)
}

fn boilerplate_config() -> String {
    "\
# er-inventory-sort runtime config (auto-created next to the game executable).
# All keys are optional. Defaults force these equipment menus to Order of Acquisition once per process.
# Use preserve to keep the game's current remembered choice for that category.
# Values: order_of_acquisition, item_type, preserve.
armaments = \"order_of_acquisition\"
armor = \"order_of_acquisition\"
talismans = \"order_of_acquisition\"
"
    .to_owned()
}

fn parse_runtime_config(path: PathBuf, contents: &str) -> Result<RuntimeConfig, String> {
    let mut config = RuntimeConfig {
        path,
        ..RuntimeConfig::default()
    };
    for (line_no, raw_line) in contents.lines().enumerate() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() || (line.starts_with('[') && line.ends_with(']')) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("invalid TOML assignment on line {}", line_no + 1));
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "armaments" | "sort.armaments" | "inventory_sort.armaments" => {
                config.armaments = Some(parse_menu_sort_default_value(value).map_err(|err| {
                    format!("invalid armaments sort on line {}: {err}", line_no + 1)
                })?);
            }
            "armor" | "sort.armor" | "inventory_sort.armor" => {
                config.armor =
                    Some(parse_menu_sort_default_value(value).map_err(|err| {
                        format!("invalid armor sort on line {}: {err}", line_no + 1)
                    })?);
            }
            "talismans" | "sort.talismans" | "inventory_sort.talismans" => {
                config.talismans = Some(parse_menu_sort_default_value(value).map_err(|err| {
                    format!("invalid talismans sort on line {}: {err}", line_no + 1)
                })?);
            }
            _ => {}
        }
    }
    Ok(config)
}

fn parse_menu_sort_default_value(value: &str) -> Result<MenuSortDefault, &'static str> {
    let label = parse_toml_string(value)?;
    parse_menu_sort_default_label(&label)
}

fn parse_menu_sort_default_label(value: &str) -> Result<MenuSortDefault, &'static str> {
    let normalized = value.trim().to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "preserve" | "disabled" | "disable" | "off" | "none" | "vanilla" => {
            Ok(MenuSortDefault::Preserve)
        }
        "item_type" | "type" => Ok(MenuSortDefault::ItemType),
        "order_of_acquisition" | "acquisition" | "order_acquisition" | "acquired" => {
            Ok(MenuSortDefault::OrderOfAcquisition)
        }
        _ => Err("expected order_of_acquisition, item_type, or preserve"),
    }
}

fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '#' if !in_string => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn parse_toml_string(value: &str) -> Result<String, &'static str> {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return Ok(value[1..value.len() - 1].to_owned());
    }
    if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
        return Err("expected a quoted TOML string");
    }
    let inner = &value[1..value.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let Some(next) = chars.next() else {
            return Err("trailing escape");
        };
        match next {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            _ => return Err("unsupported escape"),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sort_labels() {
        assert_eq!(
            parse_menu_sort_default_label("order-of-acquisition"),
            Ok(MenuSortDefault::OrderOfAcquisition)
        );
        assert_eq!(
            parse_menu_sort_default_label("type"),
            Ok(MenuSortDefault::ItemType)
        );
        assert_eq!(
            parse_menu_sort_default_label("off"),
            Ok(MenuSortDefault::Preserve)
        );
    }

    #[test]
    fn parses_config_keys_without_product_config_name() {
        let config = parse_runtime_config(
            PathBuf::from(CONFIG_FILE_NAME),
            "armaments = \"item_type\"\narmor = 'preserve'\ninventory_sort.talismans = \"order_of_acquisition\"\n",
        )
        .expect("config parses");
        assert_eq!(config.armaments, Some(MenuSortDefault::ItemType));
        assert_eq!(config.armor, Some(MenuSortDefault::Preserve));
        assert_eq!(config.talismans, Some(MenuSortDefault::OrderOfAcquisition));
    }

    #[test]
    fn sort_values_match_existing_runtime_contract() {
        assert_eq!(
            menu_sort_default_value(MenuSortDefault::ItemType),
            Some(MENU_SORT_DIRECTION_FLAG | MENU_SORT_ITEM_TYPE_ID)
        );
        assert_eq!(
            menu_sort_default_value(MenuSortDefault::OrderOfAcquisition),
            Some(MENU_SORT_DIRECTION_FLAG | MENU_SORT_ORDER_OF_ACQUISITION_ID)
        );
        assert_eq!(menu_sort_default_value(MenuSortDefault::Preserve), None);
    }

    #[test]
    fn configured_sort_overrides_existing_non_item_sort() {
        let existing_weight_sort = 0x5142;

        assert_eq!(
            decide_menu_sort_write(
                MENU_SORT_DIRECTION_FLAG | existing_weight_sort,
                MenuSortDefault::OrderOfAcquisition
            ),
            MenuSortWriteDecision::Write(
                MENU_SORT_DIRECTION_FLAG | MENU_SORT_ORDER_OF_ACQUISITION_ID
            )
        );
    }
}
