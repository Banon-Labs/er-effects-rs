use std::{
    ffi::c_void,
    fs,
    path::{Path, PathBuf},
    sync::{
        Once, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp},
    fd4::FD4TaskData,
};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};

const DLL_MAIN_SUCCESS: i32 = 1;
const DLL_PROCESS_ATTACH: u32 = 1;
type Hinstance = *mut c_void;
const CURRENT_PROCESS_PSEUDO_HANDLE: isize = -1;
const NULL: usize = 0;
const READ_PROCESS_MEMORY_FALSE: i32 = 0;
const GAME_DATA_MAN_GLOBAL_RVA: usize = 0x3d5df38;
const GAME_DATA_MAN_MENU_SAVELOAD_60_OFFSET: usize = 0x60;

/// `CSMenuSystemSaveLoad::field_0x1440`: session-local remembered sort state array.
/// Static RE in the parent product DLL found that `FUN_1408581c0` resolves the active
/// sort criterion as `field_0x1440[sort_menu_type] & 0x7fffffff`; the sign bit stores
/// reverse/descending.
const MENU_SORT_STATE_ARRAY_OFFSET: usize = 0x1440;
const MENU_SORT_STATE_ENTRY_SIZE: usize = 4;
const MENU_SORT_DIRECTION_FLAG: u32 = 0x8000_0000;
const MENU_SORT_ID_MASK: u32 = 0x7fff_ffff;

/// GR_MenuText 6105 "Item Type" maps to comparator id 0x5141 in the sort-option tables.
const MENU_SORT_ITEM_TYPE_ID: u32 = 0x5141;
/// GR_MenuText 6190 "Order of Acquisition" maps to comparator id 0x5140.
const MENU_SORT_ORDER_OF_ACQUISITION_ID: u32 = 0x5140;

/// Sort-menu table types used by the isolated DLL:
/// - 4: Armaments (static MenuEquipTableData row 0x29, label 40550)
/// - 6: Armor/protectors (row 0x2a, label 40551; head/chest/arms/legs rows 0x20..0x23)
/// - 9: Talismans (SortMenu option list 0x143b35f50..0x143b35f80: Item Type / Order of Acquisition / Weight)
const MENU_SORT_TYPE_ARMAMENTS: usize = 4;
const MENU_SORT_TYPE_ARMOR: usize = 6;
const MENU_SORT_TYPE_TALISMANS: usize = 9;

const MENU_SORT_DEFAULTS_NOT_APPLIED: usize = 0;
const MENU_SORT_DEFAULTS_APPLIED: usize = 1;
static MENU_SORT_DEFAULTS_APPLIED_STATE: AtomicUsize =
    AtomicUsize::new(MENU_SORT_DEFAULTS_NOT_APPLIED);

const MENU_SORT_ARMAMENTS_ENV: &str = "ER_EFFECTS_MENU_SORT_ARMAMENTS";
const MENU_SORT_ARMOR_ENV: &str = "ER_EFFECTS_MENU_SORT_ARMOR";
const MENU_SORT_TALISMANS_ENV: &str = "ER_EFFECTS_MENU_SORT_TALISMANS";

static START_MENU_SORT_TASK: Once = Once::new();
static RUNTIME_CONFIG: OnceLock<RuntimeConfig> = OnceLock::new();

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
    menu_sort_armaments: Option<MenuSortDefault>,
    menu_sort_armor: Option<MenuSortDefault>,
    menu_sort_talismans: Option<MenuSortDefault>,
}

#[unsafe(no_mangle)]
/// # Safety
///
/// This is called by Windows when the DLL is loaded. Do not call it directly.
pub unsafe extern "system" fn DllMain(
    _hmodule: Hinstance,
    reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    if reason != DLL_PROCESS_ATTACH {
        return DLL_MAIN_SUCCESS;
    }

    init_runtime_config();
    log(format_args!(
        "menu-sort-dll: attach armaments={} armor={} talismans={}",
        configured_menu_sort_armaments().label(),
        configured_menu_sort_armor().label(),
        configured_menu_sort_talismans().label()
    ));
    START_MENU_SORT_TASK.call_once(spawn_menu_sort_task);

    DLL_MAIN_SUCCESS
}

fn spawn_menu_sort_task() {
    let _ = std::thread::Builder::new()
        .name("er-menu-sort-task".to_owned())
        .spawn(|| {
            let cs_task = wait_for_task_instance();
            log(format_args!(
                "menu-sort-dll: CSTaskImp ready; registering recurring task"
            ));
            cs_task.run_recurring(
                move |_task_data: &FD4TaskData| {
                    apply_default_menu_sort_preferences_once();
                },
                CSTaskGroupIndex::FrameBegin,
            );
        });
}

fn wait_for_task_instance() -> &'static CSTaskImp {
    let mut attempts = 0_u64;
    loop {
        match unsafe { CSTaskImp::instance() } {
            Ok(instance) => return instance,
            Err(InstanceError::NotFound(_)) | Err(InstanceError::Null(_)) => {
                attempts = attempts.saturating_add(1);
                if attempts == 1 || attempts % 1000 == 0 {
                    log(format_args!(
                        "menu-sort-dll: waiting for CSTaskImp attempts={attempts}"
                    ));
                }
                std::thread::yield_now();
            }
        }
    }
}

fn apply_default_menu_sort_preferences_once() {
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
            log(format_args!(
                "menu-sort-dll: preserve configured category={label} sort_type={sort_type}"
            ));
            continue;
        };
        let target_id = target_value & MENU_SORT_ID_MASK;
        let addr = menu_system_save_load
            + MENU_SORT_STATE_ARRAY_OFFSET
            + sort_type * MENU_SORT_STATE_ENTRY_SIZE;
        let Some(current) = (unsafe { safe_read_i32(addr) }) else {
            log(format_args!(
                "menu-sort-dll: deferred; could not read sort_type={sort_type} addr=0x{addr:x}"
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
            log(format_args!(
                "menu-sort-dll: preserve user/non-item category={label} sort_type={sort_type} value=0x{current_u32:x} configured={}",
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
    log(format_args!(
        "menu-sort-dll: applied defaults mss=0x{menu_system_save_load:x} changed={changed} already={already} skipped={skipped} targets={:?}",
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

fn configured_menu_sort_armaments() -> MenuSortDefault {
    configured_menu_sort_default(
        MENU_SORT_ARMAMENTS_ENV,
        |config| config.menu_sort_armaments,
        "armaments",
    )
}

fn configured_menu_sort_armor() -> MenuSortDefault {
    configured_menu_sort_default(
        MENU_SORT_ARMOR_ENV,
        |config| config.menu_sort_armor,
        "armor",
    )
}

fn configured_menu_sort_talismans() -> MenuSortDefault {
    configured_menu_sort_default(
        MENU_SORT_TALISMANS_ENV,
        |config| config.menu_sort_talismans,
        "talismans",
    )
}

fn configured_menu_sort_default(
    env_name: &str,
    from_config: impl FnOnce(&RuntimeConfig) -> Option<MenuSortDefault>,
    label: &str,
) -> MenuSortDefault {
    if let Ok(value) = std::env::var(env_name) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            match parse_menu_sort_default_label(trimmed) {
                Ok(choice) => return choice,
                Err(err) => log(format_args!(
                    "menu-sort-dll: ignoring invalid {env_name} for menu_sort.{label}: {err}"
                )),
            }
        }
    }
    RUNTIME_CONFIG
        .get()
        .and_then(from_config)
        .unwrap_or(MenuSortDefault::OrderOfAcquisition)
}

fn init_runtime_config() {
    let config = load_runtime_config().unwrap_or_else(|err| {
        log(format_args!("menu-sort-dll: config load warning: {err}"));
        RuntimeConfig::default()
    });
    let _ = RUNTIME_CONFIG.set(config);
}

fn load_runtime_config() -> Result<RuntimeConfig, String> {
    let Some(config_path) = runtime_config_path() else {
        return Ok(RuntimeConfig::default());
    };
    let contents = fs::read_to_string(&config_path)
        .map_err(|err| format!("failed to read '{}': {err}", config_path.display()))?;
    parse_runtime_config(&contents, &config_path)
}

fn runtime_config_path() -> Option<PathBuf> {
    let dir = game_directory_path()?;
    // Prefer the isolated crate's config file, but accept the existing product config keys so this
    // DLL can be tested beside the current product without duplicate configuration.
    [dir.join("er-menu-sort.toml"), dir.join("er-effects.toml")]
        .into_iter()
        .find(|path| path.exists())
}

fn parse_runtime_config(contents: &str, path: &Path) -> Result<RuntimeConfig, String> {
    let mut config = RuntimeConfig::default();
    for (line_no, raw_line) in contents.lines().enumerate() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "menu_sort.armaments" | "sort.armaments" | "armaments_sort" => {
                config.menu_sort_armaments =
                    Some(parse_menu_sort_default_value(value).map_err(|err| {
                        format!(
                            "invalid menu_sort.armaments in '{}' on line {}: {err}",
                            path.display(),
                            line_no + 1
                        )
                    })?);
            }
            "menu_sort.armor"
            | "sort.armor"
            | "armor_sort"
            | "menu_sort.protectors"
            | "sort.protectors"
            | "protectors_sort" => {
                config.menu_sort_armor =
                    Some(parse_menu_sort_default_value(value).map_err(|err| {
                        format!(
                            "invalid menu_sort.armor in '{}' on line {}: {err}",
                            path.display(),
                            line_no + 1
                        )
                    })?);
            }
            "menu_sort.talismans" | "sort.talismans" | "talismans_sort" => {
                config.menu_sort_talismans =
                    Some(parse_menu_sort_default_value(value).map_err(|err| {
                        format!(
                            "invalid menu_sort.talismans in '{}' on line {}: {err}",
                            path.display(),
                            line_no + 1
                        )
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

    let mut out = String::with_capacity(value.len());
    let mut chars = value[1..value.len() - 1].chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                _ => return Err("unsupported escape in string"),
            }
        } else {
            out.push(ch);
        }
    }
    Ok(out)
}

fn game_module_base() -> Result<usize, String> {
    let module = unsafe { GetModuleHandleA(std::ptr::null()) };
    if module.is_null() {
        Err("failed to resolve game module".to_owned())
    } else {
        Ok(module as usize)
    }
}

unsafe fn resolve_menu_system_save_load(base: usize) -> Option<usize> {
    let gdm = unsafe { safe_read_usize(base + GAME_DATA_MAN_GLOBAL_RVA) }.filter(|&v| v != NULL)?;
    unsafe { safe_read_usize(gdm + GAME_DATA_MAN_MENU_SAVELOAD_60_OFFSET) }.filter(|&v| v != NULL)
}

unsafe fn safe_read_usize(addr: usize) -> Option<usize> {
    let mut value: usize = NULL;
    let mut read: usize = NULL;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut usize as *mut c_void,
            std::mem::size_of::<usize>(),
            &mut read,
        )
    };
    if ok != READ_PROCESS_MEMORY_FALSE && read == std::mem::size_of::<usize>() {
        Some(value)
    } else {
        None
    }
}

unsafe fn safe_read_i32(addr: usize) -> Option<i32> {
    let mut value: i32 = NULL as i32;
    let mut read: usize = NULL;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut i32 as *mut c_void,
            std::mem::size_of::<i32>(),
            &mut read,
        )
    };
    if ok != READ_PROCESS_MEMORY_FALSE && read == std::mem::size_of::<i32>() {
        Some(value)
    } else {
        None
    }
}

fn game_directory_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
}

fn log(args: std::fmt::Arguments<'_>) {
    use std::io::Write;
    let path = std::env::var("ER_MENU_SORT_DEBUG_PATH")
        .or_else(|_| std::env::var("ER_EFFECTS_AUTOLOAD_DEBUG_PATH"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("er-menu-sort-debug.log"));
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{args}");
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleA(module_name: *const u8) -> *mut c_void;

    fn ReadProcessMemory(
        process: isize,
        base: *const c_void,
        buffer: *mut c_void,
        size: usize,
        read: *mut usize,
    ) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sort_aliases() {
        assert_eq!(
            parse_menu_sort_default_label("order-of-acquisition"),
            Ok(MenuSortDefault::OrderOfAcquisition)
        );
        assert_eq!(
            parse_menu_sort_default_label("type"),
            Ok(MenuSortDefault::ItemType)
        );
        assert_eq!(
            parse_menu_sort_default_label("vanilla"),
            Ok(MenuSortDefault::Preserve)
        );
    }

    #[test]
    fn parses_runtime_config_subset() {
        let config = parse_runtime_config(
            r#"
            menu_sort.armaments = "order_of_acquisition"
            menu_sort.protectors = "item_type"
            menu_sort.talismans = "preserve"
            "#,
            Path::new("test.toml"),
        )
        .expect("config parses");

        assert_eq!(
            config.menu_sort_armaments,
            Some(MenuSortDefault::OrderOfAcquisition)
        );
        assert_eq!(config.menu_sort_armor, Some(MenuSortDefault::ItemType));
        assert_eq!(config.menu_sort_talismans, Some(MenuSortDefault::Preserve));
    }
}
