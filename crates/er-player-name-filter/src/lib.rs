//! Standalone remote-player name filter DLL.
//!
//! The hook targets `CS::SessionManagerPlayerEntryBase::Copy` in Elden Ring 1.16.2. That is the
//! storage/copy point feeding vanilla overhead friendly tags, including Seamless Co-op's default
//! overhead-name mode. The DLL rewrites the copied inline Steam display-name string when it matches
//! a configured group.
//!
//! One hook is not enough, because the overhead tag is not always built from that string.
//! `CS::CSFeManImp::UpdateEnemyTags` builds each overhead nameplate through
//! `CS::GetPlayerChrName`, which branches on `CS::GameSettings::ShowPlayerNames`: with Steam
//! names switched off it takes the name out of `PlayerGameData` (the character name, which is
//! also the string Seamless Co-op syncs), and only with them switched on does it read the
//! `steamName` the copy hook above rewrites. Measured live on 2026-08-22 with three remote
//! players: both flags read 0, so every tag came from the character-name branch and the
//! rewritten Steam names were never displayed. So this DLL hooks `GetPlayerChrName` as well and
//! rewrites the produced `MenuString`, which covers both settings with one detour.
//!
//! Configuration lives beside the loaded DLL as `er_player_name_filter.toml` when the artifact is
//! `er_player_name_filter.dll`. Groups use `player_name_filter.<group>.<field>` keys; each group has
//! a replacement label plus any number of exact names and/or regexes. First matching group wins;
//! filtered players get a stable per-group numeric suffix.

#![cfg_attr(not(windows), allow(dead_code))]

use std::{collections::HashMap, path::PathBuf};

use regex::{Regex, RegexBuilder};

const DLL_MAIN_SUCCESS: i32 = 1;
const SESSION_MANAGER_PLAYER_ENTRY_BASE_COPY_RVA: usize = 0x023f1bf0;
/// `CS::GetPlayerChrName(MenuString *out, PlayerIns *player, char decorate)` -- the single
/// producer of a player's displayed name, reached from `CSFeManImp::UpdateEnemyTags` via
/// `GetChrName`. NPCs leave through `MenuString::NpcName` before either name branch.
const GET_PLAYER_CHR_NAME_RVA: usize = 0x0075f800;
/// `CS::PlayerIns::GetSessionManagerPlayerEntry` @0x140657b20 is `mov rax, [rcx+0x6b8]; ret`,
/// so the entry is read inline rather than called.
const PLAYER_INS_SESSION_MANAGER_PLAYER_ENTRY_OFFSET: usize = 0x6b8;
/// `DLTX::DLString<wchar_t>::FromU16Array(DLString *out, const wchar_t *text, DLAllocator *)`.
const DL_STRING_FROM_U16_ARRAY_RVA: usize = 0x0011a360;
/// `DLTX::DLString<wchar_t>::Copy(DLString *dst, const DLString *src)`, used by
/// `GetPlayerChrName` itself to fill its out-parameter.
const DL_STRING_COPY_RVA: usize = 0x00117910;
/// The `GLOBAL_MenuHeapAllocator` pointer `GetPlayerChrName` passes to `FromU16Array`.
const MENU_HEAP_ALLOCATOR_POINTER_RVA: usize = er_game_base::rva::GLOBAL_MENU_HEAP_ALLOCATOR_RVA;
/// `DLAllocator::Deallocate` slot, from the `call *0x68(%rax)` the game emits when releasing a
/// heap-backed `DLString`.
const DL_ALLOCATOR_DEALLOCATE_VTABLE_OFFSET: usize = 0x68;
/// A `DLString<wchar_t>` stores up to this many text units inline; above it the union holds a
/// heap pointer the allocator owns.
const DL_STRING_IN_PLACE_CAPACITY: usize = 7;
/// A displayed name longer than this is treated as a garbage read rather than a name.
const DL_STRING_MAX_PLAUSIBLE_TEXT_UNITS: usize = 512;
/// `CS::PlayerGameData::CopyChrName(PlayerGameData *pgd, const wchar_t *name)` -- the sole writer
/// of `PlayerGameData::characterName`, reached for a remote player from the network receive path
/// `TryDequeuePacket8@0x140ca3030`. Masking the name here masks it before anything can read it,
/// including third-party overlays that read the field straight out of game memory rather than
/// calling a game function (ERGG does exactly that), and it costs those overlays nothing and
/// needs no knowledge of them.
///
/// Declared once in `er-game-base::rva`, where the full three-storage layout is written down: the
/// build importer calls the same function to adopt a build's name, so the address is now shared
/// and `check-rva-alias-drift.py` requires one declaration for it.
const PLAYER_GAME_DATA_COPY_CHR_NAME_RVA: usize =
    er_game_base::rva::PLAYER_GAME_DATA_COPY_CHR_NAME_RVA;
/// `PlayerGameData::isMainPlayer`. The local player's own name is never rewritten: it is their
/// save's name and the name they send to everyone else.
const PLAYER_GAME_DATA_IS_MAIN_PLAYER_OFFSET: usize = 0x8f0;
/// `PlayerGameData::characterName` is `wchar_t[17]`, and CopyChrName copies only when
/// `len < 0x11`. A longer replacement is silently dropped and the real name survives, so the
/// replacement is clamped to this many text units rather than trusted to fit.
const CHARACTER_NAME_MAX_TEXT_UNITS: usize = 16;
include!(concat!(env!("OUT_DIR"), "/generated_prologues.rs"));
const STEAM_NAME_UTF16_CAPACITY: usize = 64;
const STEAM_NAME_UTF16_MAX_TEXT_UNITS: usize = STEAM_NAME_UTF16_CAPACITY - 1;
const FILTER_REPLACEMENT_LOG_LIMIT: u64 = 16;
/// The chr-name hook gets its own budget. Sharing one counter with the Copy hook made a live run
/// unreadable on 2026-08-22: Copy spent all 16 lines before a single tag rendered, so "the hook
/// never fired" and "the hook fired but logging was capped" produced an identical empty log.
const CHR_NAME_REPLACEMENT_LOG_LIMIT: u64 = 16;
/// The first calls into the chr-name hook are logged with their outcome, including the ones that
/// change nothing. Without this a run cannot distinguish "this function is not on the path for
/// these players" from "it is on the path and declined to match".
const CHR_NAME_CALL_LOG_LIMIT: u64 = 24;
const COPY_CHR_NAME_REPLACEMENT_LOG_LIMIT: u64 = 16;
const LOG_FILE_NAME: &str = "er-player-name-filter.log";

#[derive(Clone, Debug, PartialEq, Eq)]
struct NameFilterRuntimeConfig {
    path: PathBuf,
    /// The master switch. Absent means on, so an existing config keeps working unchanged.
    enabled: bool,
    groups: Vec<NameFilterGroupConfig>,
}

impl Default for NameFilterRuntimeConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            enabled: true,
            groups: Vec::new(),
        }
    }
}

impl NameFilterRuntimeConfig {
    fn group_mut(&mut self, id: &str) -> &mut NameFilterGroupConfig {
        if let Some(index) = self.groups.iter().position(|group| group.id == id) {
            return &mut self.groups[index];
        }
        self.groups.push(NameFilterGroupConfig {
            id: id.to_owned(),
            ..NameFilterGroupConfig::default()
        });
        self.groups
            .last_mut()
            .expect("a just-pushed name-filter group must exist")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct NameFilterGroupConfig {
    id: String,
    replacement: Option<String>,
    exact_names: Vec<String>,
    regexes: Vec<String>,
}

#[repr(C)]
struct RawDlUtf16String {
    vftable: usize,
    backing_string: *mut u16,
    length: usize,
    unk18: u32,
    char_size: u16,
    encoding: u8,
    flags: u8,
}

#[repr(C)]
struct RawDlInplaceUtf16String64 {
    base: RawDlUtf16String,
    bytes: [u16; STEAM_NAME_UTF16_CAPACITY],
    unk: usize,
}

#[repr(C)]
struct RawSessionManagerPlayerEntryBase {
    internal_thread_steam_connection: usize,
    internal_thread_steam_socket: usize,
    steam_id: u64,
    steam_name: RawDlInplaceUtf16String64,
    connection_ref_info: usize,
    voice_chat_member_ref_info: usize,
}

struct CompiledNameFilter {
    groups: Vec<CompiledNameFilterGroup>,
}

struct CompiledNameFilterGroup {
    id: String,
    replacement: String,
    exact_names: Vec<String>,
    regexes: Vec<Regex>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum PlayerNameIdentity {
    SteamId(u64),
    EntryAddress(usize),
    /// `CopyChrName` is handed a `PlayerGameData` and a string and nothing that reaches a Steam
    /// ID, so a name masked there is numbered by the name it replaced. The mask it issues is
    /// then recognised downstream and left alone, so a player still ends up with one number.
    CharacterName(String),
}

impl PlayerNameIdentity {
    fn for_entry(steam_id: u64, entry_address: usize) -> Self {
        if steam_id == 0 {
            Self::EntryAddress(entry_address)
        } else {
            Self::SteamId(steam_id)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PlayerGroupNumber {
    group_id: String,
    ordinal: u32,
}

#[derive(Debug, Default)]
struct GroupNumberAssignments {
    next_by_group: HashMap<String, u32>,
    player_by_identity: HashMap<PlayerNameIdentity, PlayerGroupNumber>,
}

impl GroupNumberAssignments {
    fn ordinal_for(&mut self, identity: &PlayerNameIdentity, group_id: &str) -> u32 {
        if let Some(existing) = self.player_by_identity.get(identity)
            && existing.group_id == group_id
        {
            return existing.ordinal;
        }

        let next = self.next_by_group.entry(group_id.to_owned()).or_insert(1);
        let ordinal = *next;
        *next = next.saturating_add(1);
        self.player_by_identity.insert(
            identity.clone(),
            PlayerGroupNumber {
                group_id: group_id.to_owned(),
                ordinal,
            },
        );
        ordinal
    }
}

fn numbered_replacement(replacement: &str, ordinal: u32) -> String {
    format!("{replacement} {ordinal}")
}

struct CompiledFilterResult {
    filter: CompiledNameFilter,
    diagnostics: Vec<String>,
}

impl CompiledNameFilter {
    fn from_config(groups: &[NameFilterGroupConfig]) -> CompiledFilterResult {
        let mut compiled = Vec::new();
        let mut diagnostics = Vec::new();
        for group in groups {
            let replacement = match &group.replacement {
                Some(replacement) if !replacement.is_empty() => replacement.clone(),
                _ => {
                    diagnostics.push(format!(
                        "skipping group '{}' with no replacement label",
                        group.id
                    ));
                    continue;
                }
            };
            let mut regexes = Vec::new();
            for pattern in group.regexes.iter().filter(|pattern| !pattern.is_empty()) {
                match RegexBuilder::new(pattern).case_insensitive(true).build() {
                    Ok(regex) => regexes.push(regex),
                    Err(err) => diagnostics.push(format!(
                        "skipping invalid regex in group '{}': {err}",
                        group.id
                    )),
                }
            }
            let exact_names: Vec<String> = group
                .exact_names
                .iter()
                .filter(|name| !name.is_empty())
                .map(|name| name.to_lowercase())
                .collect();
            if exact_names.is_empty() && regexes.is_empty() {
                diagnostics.push(format!(
                    "skipping group '{}' with no valid names or regexes",
                    group.id
                ));
                continue;
            }
            let minimum_numbered_replacement_units =
                numbered_replacement(&replacement, 1).encode_utf16().count();
            if minimum_numbered_replacement_units > STEAM_NAME_UTF16_MAX_TEXT_UNITS {
                diagnostics.push(format!(
                    "replacement label plus numeric suffix for group '{}' is at least {minimum_numbered_replacement_units} UTF-16 units; runtime write will truncate to {STEAM_NAME_UTF16_MAX_TEXT_UNITS}",
                    group.id
                ));
            }
            compiled.push(CompiledNameFilterGroup {
                id: group.id.clone(),
                replacement,
                exact_names,
                regexes,
            });
        }
        CompiledFilterResult {
            filter: Self { groups: compiled },
            diagnostics,
        }
    }

    fn replacement_for(&self, name: &str) -> Option<(&str, &str)> {
        let lowered = name.to_lowercase();
        for group in &self.groups {
            if group.exact_names.contains(&lowered)
                || group.regexes.iter().any(|regex| regex.is_match(name))
            {
                return Some((group.id.as_str(), group.replacement.as_str()));
            }
        }
        None
    }
}

fn config_path_for_module(module_path: &std::path::Path) -> Result<PathBuf, String> {
    let Some(stem) = module_path.file_stem() else {
        return Err(format!(
            "module path '{}' has no file stem",
            module_path.display()
        ));
    };
    Ok(module_path.with_file_name(stem).with_extension("toml"))
}

fn boilerplate_config() -> &'static str {
    "# er-player-name-filter runtime config.\n\
#\n\
# THIS FILE IS RE-READ WHILE THE GAME RUNS. Save it and the new rules apply within a\n\
# couple of seconds -- no restart. A name already written into a player's slot stays as it\n\
# is until the game rewrites it, so leave and rejoin the session to see a rule change take\n\
# effect on players who are already on screen. A file that fails to parse is REJECTED and\n\
# the previous rules stay live, so a typo can never silently unmask anyone.\n\
#\n\
# enabled = false        # the master switch; turns every replacement off, keeps the rules\n\
#\n\
# Uncomment and edit any number of groups. First matching group wins; filtered players\n\
# get a stable per-group suffix assigned by Steam ID, e.g. 'I am a bigot 1'.\n\
#\n\
# player_name_filter.bigot.replacement = 'I am a bigot'\n\
# player_name_filter.bigot.names = ['Exact Steam name']\n\
# player_name_filter.bigot.regexes = ['(?i)^awful.*name$']\n\
"
}

fn parse_runtime_config(path: PathBuf, contents: &str) -> Result<NameFilterRuntimeConfig, String> {
    let mut config = NameFilterRuntimeConfig {
        path,
        ..NameFilterRuntimeConfig::default()
    };
    for (line_no, raw_line) in contents.lines().enumerate() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() || (line.starts_with('[') && line.ends_with(']')) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("invalid TOML assignment on line {}", line_no + 1));
        };
        if let Some(enabled) = parse_master_switch(key.trim(), value.trim(), line_no + 1)? {
            config.enabled = enabled;
            continue;
        }
        let parsed = parse_player_name_filter_assignment(
            &mut config,
            key.trim(),
            value.trim(),
            line_no + 1,
        )?;
        if !parsed {
            return Err(format!(
                "unknown config key on line {}: '{}'",
                line_no + 1,
                key.trim()
            ));
        }
    }
    Ok(config)
}

/// The master switch, as a top-level key rather than a group field: `enabled = false` turns
/// every replacement off without deleting the rules that describe them.
fn parse_master_switch(key: &str, value: &str, line_no: usize) -> Result<Option<bool>, String> {
    let recognised = matches!(
        key,
        "enabled" | "enable" | "player_name_filter.enabled" | "name_filter.enabled"
    );
    if !recognised {
        return Ok(None);
    }
    match value {
        "true" => Ok(Some(true)),
        "false" => Ok(Some(false)),
        other => Err(format!(
            "invalid '{key}' on line {line_no}: expected true or false, got '{other}'"
        )),
    }
}

fn parse_player_name_filter_assignment(
    config: &mut NameFilterRuntimeConfig,
    key: &str,
    value: &str,
    line_no: usize,
) -> Result<bool, String> {
    let Some(rest) = key
        .strip_prefix("player_name_filter.")
        .or_else(|| key.strip_prefix("name_filter."))
        .or_else(|| key.strip_prefix("remote_name_filter."))
    else {
        return Ok(false);
    };
    let Some((group_id, field)) = rest.rsplit_once('.') else {
        return Err(format!(
            "invalid player_name_filter key on line {line_no}: expected player_name_filter.<group>.<field>"
        ));
    };
    let group_id = group_id.trim();
    if group_id.is_empty() {
        return Err(format!(
            "invalid player_name_filter key on line {line_no}: group id is empty"
        ));
    }
    let group = config.group_mut(group_id);
    match field.trim() {
        "replacement" | "label" | "replace_with" => {
            group.replacement = Some(parse_toml_string(value).map_err(|err| {
                format!("invalid player_name_filter replacement on line {line_no}: {err}")
            })?);
        }
        "name" | "names" | "exact" | "exact_names" => {
            group
                .exact_names
                .extend(parse_toml_string_list(value).map_err(|err| {
                    format!("invalid player_name_filter names on line {line_no}: {err}")
                })?);
        }
        "regex" | "regexes" | "pattern" | "patterns" => {
            group
                .regexes
                .extend(parse_toml_string_list(value).map_err(|err| {
                    format!("invalid player_name_filter regexes on line {line_no}: {err}")
                })?);
        }
        unknown => {
            return Err(format!(
                "invalid player_name_filter key on line {line_no}: unknown field '{unknown}'"
            ));
        }
    }
    Ok(true)
}

fn strip_comment(line: &str) -> &str {
    let mut in_basic_string = false;
    let mut in_literal_string = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_basic_string => escaped = true,
            '"' if !in_literal_string => in_basic_string = !in_basic_string,
            '\'' if !in_basic_string => in_literal_string = !in_literal_string,
            '#' if !in_basic_string && !in_literal_string => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn parse_toml_string_list(value: &str) -> Result<Vec<String>, &'static str> {
    let value = value.trim();
    if !value.starts_with('[') {
        return parse_toml_string(value).map(|item| vec![item]);
    }
    if !value.ends_with(']') {
        return Err("expected a TOML string or string array");
    }
    let inner = &value[1..value.len() - 1];
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut items = Vec::new();
    let mut start = 0;
    let mut in_basic_string = false;
    let mut in_literal_string = false;
    let mut escaped = false;
    for (idx, ch) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_basic_string => escaped = true,
            '"' if !in_literal_string => in_basic_string = !in_basic_string,
            '\'' if !in_basic_string => in_literal_string = !in_literal_string,
            ',' if !in_basic_string && !in_literal_string => {
                let item = inner[start..idx].trim();
                if item.is_empty() {
                    return Err("empty string-array item");
                }
                items.push(parse_toml_string(item)?);
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    if in_basic_string || in_literal_string {
        return Err("unterminated string in array");
    }
    let tail = inner[start..].trim();
    if !tail.is_empty() {
        items.push(parse_toml_string(tail)?);
    }
    Ok(items)
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

fn read_inline_steam_name(name: &RawDlInplaceUtf16String64) -> Option<String> {
    if name.base.length > STEAM_NAME_UTF16_CAPACITY {
        return None;
    }
    let inline = name.bytes.as_ptr().cast_mut();
    if name.base.backing_string != inline {
        return None;
    }
    String::from_utf16(&name.bytes[..name.base.length]).ok()
}

fn write_inline_steam_name(name: &mut RawDlInplaceUtf16String64, replacement: &str) {
    let units: Vec<u16> = replacement
        .encode_utf16()
        .take(STEAM_NAME_UTF16_MAX_TEXT_UNITS)
        .collect();
    name.bytes.fill(0);
    name.bytes[..units.len()].copy_from_slice(&units);
    name.base.backing_string = name.bytes.as_mut_ptr();
    name.base.length = units.len();
    name.base.char_size = 2;
    name.base.encoding = 1;
    name.base.flags = 0;
}

/// `DLTX::DLString<wchar_t>`: 48 bytes, the union holding either eight inline text units or a
/// pointer to an allocator-owned buffer. Which one is live is decided by `capacity`, exactly as
/// the game decides it -- `7 < capacity` means heap.
#[repr(C)]
struct RawDlStringWide {
    allocator: *mut RawDlAllocator,
    string: RawDlStringWideUnion,
    length: usize,
    capacity: usize,
    encoding_type: u8,
    _pad: [u8; 7],
}

#[repr(C)]
union RawDlStringWideUnion {
    in_place: [u16; 8],
    pointer: *mut u16,
}

/// `CS::MenuString`: a shortcut `rawString` consulted first by some consumers, and the owned
/// `DLString` every other consumer reads. `GetPlayerChrName` always leaves `rawString` null and
/// puts the name in the `DLString`, so a rewrite must do the same.
#[repr(C)]
struct RawMenuString {
    raw_string: *mut u16,
    dl_string: RawDlStringWide,
}

#[repr(C)]
struct RawDlAllocator {
    vftable: *const usize,
}

type DlStringFromU16ArrayFn =
    unsafe extern "system" fn(*mut RawDlStringWide, *const u16, *mut RawDlAllocator);
type DlStringCopyFn = unsafe extern "system" fn(*mut RawDlStringWide, *const RawDlStringWide);
type DlAllocatorDeallocateFn =
    unsafe extern "system" fn(*mut RawDlAllocator, *mut core::ffi::c_void);

/// Where a `DLString`'s text units actually live, or `None` when the header does not describe a
/// string this code is willing to touch.
///
/// # Safety
///
/// `string` must point at a live `DLString<wchar_t>`.
unsafe fn dl_string_text_units(string: &RawDlStringWide) -> Option<*const u16> {
    if string.length > string.capacity || string.length > DL_STRING_MAX_PLAUSIBLE_TEXT_UNITS {
        return None;
    }
    let pointer = if string.capacity > DL_STRING_IN_PLACE_CAPACITY {
        unsafe { string.string.pointer.cast_const() }
    } else {
        unsafe { string.string.in_place.as_ptr() }
    };
    if pointer.is_null() {
        return None;
    }
    Some(pointer)
}

/// # Safety
///
/// `menu_string` must point at a live `CS::MenuString`.
unsafe fn read_menu_string(menu_string: &RawMenuString) -> Option<String> {
    if !menu_string.raw_string.is_null() {
        let mut units = Vec::new();
        for index in 0..DL_STRING_MAX_PLAUSIBLE_TEXT_UNITS {
            let unit = unsafe { *menu_string.raw_string.add(index) };
            if unit == 0 {
                return String::from_utf16(&units).ok();
            }
            units.push(unit);
        }
        return None;
    }
    let pointer = unsafe { dl_string_text_units(&menu_string.dl_string) }?;
    let units = unsafe { std::slice::from_raw_parts(pointer, menu_string.dl_string.length) };
    String::from_utf16(units).ok()
}

/// Overwrite a `DLString`'s existing buffer when the replacement fits in the capacity the game
/// already paid for. Returns false when it does not fit and the caller must go through the
/// allocator instead.
///
/// # Safety
///
/// `string` must point at a live `DLString<wchar_t>` and `units` must be NUL-terminated.
unsafe fn overwrite_dl_string_in_place(string: &mut RawDlStringWide, units: &[u16]) -> bool {
    let text_units = units.len().saturating_sub(1);
    if text_units > string.capacity {
        return false;
    }
    let destination = if string.capacity > DL_STRING_IN_PLACE_CAPACITY {
        unsafe { string.string.pointer }
    } else {
        unsafe { string.string.in_place.as_mut_ptr() }
    };
    if destination.is_null() {
        return false;
    }
    unsafe { std::ptr::copy_nonoverlapping(units.as_ptr(), destination, units.len()) };
    string.length = text_units;
    true
}

/// Release whatever buffer a `DLString` owns and leave it empty, the same teardown the game
/// emits before it reuses one (`7 < capacity` -> `Deallocate`, then reset to the inline form).
///
/// # Safety
///
/// `string` must point at a live `DLString<wchar_t>` whose allocator is still valid.
unsafe fn release_dl_string(string: &mut RawDlStringWide) {
    if string.capacity > DL_STRING_IN_PLACE_CAPACITY && !string.allocator.is_null() {
        let vftable = unsafe { (*string.allocator).vftable };
        if !vftable.is_null() {
            let slot = unsafe { vftable.byte_add(DL_ALLOCATOR_DEALLOCATE_VTABLE_OFFSET) };
            let deallocate: DlAllocatorDeallocateFn = unsafe { std::mem::transmute(*slot) };
            let buffer = unsafe { string.string.pointer };
            unsafe { deallocate(string.allocator, buffer.cast()) };
        }
    }
    string.string = RawDlStringWideUnion { in_place: [0; 8] };
    string.length = 0;
    string.capacity = DL_STRING_IN_PLACE_CAPACITY;
}

#[cfg(windows)]
mod windows_runtime {
    use std::{
        collections::HashMap,
        ffi::c_void,
        fmt,
        os::windows::ffi::OsStrExt,
        path::{Path, PathBuf},
        sync::{
            Arc, Mutex, Once, OnceLock, RwLock,
            atomic::{AtomicU64, AtomicUsize, Ordering},
        },
        time::SystemTime,
    };

    use er_game_base::{
        log::{append_line, game_directory_path},
        mem::{game_module_base, read_bytes},
    };
    use er_hook::UnionFn;

    use super::{
        CHARACTER_NAME_MAX_TEXT_UNITS, CHR_NAME_CALL_LOG_LIMIT, CHR_NAME_REPLACEMENT_LOG_LIMIT,
        COPY_CHR_NAME_REPLACEMENT_LOG_LIMIT, CompiledNameFilter, DL_STRING_COPY_RVA,
        DL_STRING_FROM_U16_ARRAY_RVA, FILTER_REPLACEMENT_LOG_LIMIT, GET_PLAYER_CHR_NAME_PROLOGUE,
        GET_PLAYER_CHR_NAME_RVA, GroupNumberAssignments, LOG_FILE_NAME,
        MENU_HEAP_ALLOCATOR_POINTER_RVA, NameFilterRuntimeConfig,
        PLAYER_GAME_DATA_COPY_CHR_NAME_PROLOGUE, PLAYER_GAME_DATA_COPY_CHR_NAME_RVA,
        PLAYER_GAME_DATA_IS_MAIN_PLAYER_OFFSET, PLAYER_INS_SESSION_MANAGER_PLAYER_ENTRY_OFFSET,
        PlayerNameIdentity, RawDlAllocator, RawDlStringWide, RawMenuString,
        RawSessionManagerPlayerEntryBase, SESSION_MANAGER_PLAYER_ENTRY_BASE_COPY_PROLOGUE,
        SESSION_MANAGER_PLAYER_ENTRY_BASE_COPY_RVA, config_path_for_module, numbered_replacement,
        overwrite_dl_string_in_place, parse_runtime_config, read_inline_steam_name,
        read_menu_string, release_dl_string, write_inline_steam_name,
    };

    /// Hard safety cap on the change-notification wait, so a lost or never-signalled handle
    /// degrades into a slow re-check rather than a thread parked forever. It is a backstop, not
    /// the mechanism: a save signals the event and the reload happens immediately.
    const CONFIG_WAIT_CAP_MILLIS: u32 = 30_000;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 0x0000_0102;
    const INVALID_HANDLE_VALUE: isize = -1;
    /// `FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_FILE_NAME`.
    /// The last one matters: an editor that writes a temp file and renames it over the target
    /// never touches the original's write time.
    const CONFIG_NOTIFY_FILTER: u32 = 0x0000_0010 | 0x0000_0008 | 0x0000_0001;

    /// The rules as the hooks see them: what to replace, and whether to replace at all.
    struct ActiveFilter {
        enabled: bool,
        filter: CompiledNameFilter,
    }

    /// The live rules. `None` only before the first config load.
    fn active_filter() -> Option<Arc<ActiveFilter>> {
        let lock = PLAYER_NAME_FILTER.get()?;
        let guard = lock.read().unwrap_or_else(|poison| poison.into_inner());
        Some(Arc::clone(&guard))
    }

    const DLL_PROCESS_ATTACH: u32 = 1;
    const ORIGINAL_UNSET: usize = 0;
    /// Big enough for the longest prologue this DLL byte-checks.
    const MAX_PROLOGUE_BYTES: usize = 16;

    static START: Once = Once::new();
    static GAME_BASE: AtomicUsize = AtomicUsize::new(0);
    static SESSION_MANAGER_PLAYER_ENTRY_BASE_COPY_ORIG: AtomicUsize =
        AtomicUsize::new(ORIGINAL_UNSET);
    static GET_PLAYER_CHR_NAME_ORIG: AtomicUsize = AtomicUsize::new(ORIGINAL_UNSET);
    /// Replacement labels built once through the game's own allocator and then kept for the
    /// life of the process. `UpdateEnemyTags` runs every frame, so allocating and freeing a
    /// fresh source string per tag per frame would be pure churn; there is one entry per
    /// distinct label, which is bounded by the configured groups and the players in session.
    static REPLACEMENT_DL_STRINGS: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    static PLAYER_NAME_FILTER_REPLACEMENTS: AtomicU64 = AtomicU64::new(0);
    static CHR_NAME_REPLACEMENTS: AtomicU64 = AtomicU64::new(0);
    static CHR_NAME_CALLS: AtomicU64 = AtomicU64::new(0);
    static COPY_CHR_NAME_REPLACEMENTS: AtomicU64 = AtomicU64::new(0);
    static PLAYER_GAME_DATA_COPY_CHR_NAME_ORIG: AtomicUsize = AtomicUsize::new(ORIGINAL_UNSET);
    static MASK_WIDE_STRINGS: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    static ISSUED_MASKS: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    /// The live rules, swapped wholesale when the config file changes. Read on the game thread
    /// once per hooked call, so it is an `Arc` clone under a short read lock rather than a lock
    /// held across any game call.
    static PLAYER_NAME_FILTER: OnceLock<RwLock<Arc<ActiveFilter>>> = OnceLock::new();
    static PLAYER_NAME_GROUP_NUMBERS: OnceLock<Mutex<GroupNumberAssignments>> = OnceLock::new();

    unsafe extern "system" {
        fn GetModuleFileNameW(module: *mut c_void, filename: *mut u16, size: u32) -> u32;
        fn FindFirstChangeNotificationW(
            path: *const u16,
            watch_subtree: i32,
            notify_filter: u32,
        ) -> isize;
        fn FindNextChangeNotification(handle: isize) -> i32;
        fn FindCloseChangeNotification(handle: isize) -> i32;
        fn WaitForSingleObject(handle: isize, milliseconds: u32) -> u32;
    }

    pub(super) unsafe extern "system" fn dll_main(
        module: *mut c_void,
        reason: u32,
        _reserved: *mut c_void,
    ) -> i32 {
        if reason == DLL_PROCESS_ATTACH {
            START.call_once(|| spawn_install_task(module));
        }
        super::DLL_MAIN_SUCCESS
    }

    fn spawn_install_task(module: *mut c_void) {
        let module_addr = module as usize;
        let _ = std::thread::Builder::new()
            .name("er-player-name-filter".to_owned())
            .spawn(move || install_player_name_filter(module_addr as *mut c_void));
    }

    fn install_player_name_filter(module: *mut c_void) {
        // A rust_panic in a cdylib loaded into the game is otherwise anonymous: the message goes to a
        // stderr nobody reads, and what survives is a 0xe06d7363 record naming the module and nothing
        // else. Two boots were lost to one before this existed. See er_game_base::panic_report.
        er_game_base::panic_report::report_panics_to("er-player-name-filter", log_message);
        er_hook::set_hook_logger(log_message);
        let module_path = match module_path(module) {
            Ok(path) => path,
            Err(err) => {
                log_message(format_args!("config: failed to resolve DLL path: {err}"));
                return;
            }
        };
        let config = match load_config(&module_path) {
            Ok(config) => config,
            Err(err) => {
                log_message(format_args!("config: {err}; hook not installed"));
                return;
            }
        };
        let config_path = config.path.clone();
        let group_count = publish_config(config);
        // The hooks go in even with no rules and even with the master switch off, because the
        // config is re-read while the game runs: a DLL that declined to hook could never be
        // switched back on without a restart, which is the whole point of the reload.

        let mut attempts = 0_u64;
        // Bounded (2026-08-29): an unbounded `loop { yield_now() }` in two other shells starved the
        // wineserver and hung a whole boot -- see er_game_base::wait. Same shape, same fix.
        let found = er_game_base::wait::poll_until(|| match game_module_base() {
            Ok(base) => Some(base),
            Err(err) => {
                if attempts == 0 || attempts.is_multiple_of(4096) {
                    log_message(format_args!("install: waiting for game module base: {err}"));
                }
                attempts = attempts.saturating_add(1);
                None
            }
        });
        let Some(base) = found else {
            log_message(format_args!(
                "install: no game module base; nothing installed"
            ));
            return;
        };
        GAME_BASE.store(base, Ordering::SeqCst);
        install_hook(base, group_count);
        spawn_config_watcher(config_path);
    }

    /// Compile a parsed config and make it live. Returns the number of usable groups.
    fn publish_config(config: NameFilterRuntimeConfig) -> usize {
        let compiled = CompiledNameFilter::from_config(&config.groups);
        for diagnostic in compiled.diagnostics {
            log_message(format_args!("config: {diagnostic}"));
        }
        let group_count = compiled.filter.groups.len();
        let active = Arc::new(ActiveFilter {
            enabled: config.enabled,
            filter: compiled.filter,
        });
        let lock = PLAYER_NAME_FILTER.get_or_init(|| RwLock::new(Arc::clone(&active)));
        {
            let mut guard = lock.write().unwrap_or_else(|poison| poison.into_inner());
            *guard = active;
        }
        let state = if config.enabled { "on" } else { "OFF" };
        log_message(format_args!(
            "config: live with {group_count} group(s), master switch {state}"
        ));
        group_count
    }

    /// Re-read the config file whenever it changes on disk.
    ///
    /// What deliberately survives a reload, because live game state depends on it:
    ///
    /// - the per-player ordinal assignments, so a player already displayed as "Dongerino 5"
    ///   keeps that number instead of being renumbered by the new rules;
    /// - the set of masks already issued, so a name this DLL already wrote into a player slot
    ///   is still recognised as ours and is not masked a second time;
    /// - every cached replacement string, including the `DLString`s built through the game's
    ///   own menu allocator. The game copies from those and the mask buffers are read during
    ///   the call, but they are never freed, so nothing the game holds can be left dangling by
    ///   a reload.
    ///
    /// What a reload cannot do is restore a real name that has already been written into
    /// `PlayerGameData::characterName`: the original is not kept anywhere the game can be
    /// pointed back at, so turning the switch off stops new masking and leaves players already
    /// on screen as they are until the session ends and the game rewrites them.
    fn spawn_config_watcher(config_path: PathBuf) {
        let _ = std::thread::Builder::new()
            .name("er-player-name-filter-config".to_owned())
            .spawn(move || {
                let Some(directory) = config_path.parent().map(Path::to_path_buf) else {
                    log_message(format_args!(
                        "config: '{}' has no parent directory; reload disabled",
                        config_path.display()
                    ));
                    return;
                };
                let Some(handle) = watch_directory(&directory) else {
                    log_message(format_args!(
                        "config: cannot watch '{}'; reload disabled",
                        directory.display()
                    ));
                    return;
                };
                log_message(format_args!(
                    "config: watching '{}' for changes; edits apply without a restart",
                    config_path.display()
                ));
                let mut last = config_stamp(&config_path);
                loop {
                    if !wait_for_directory_change(handle) {
                        log_message(format_args!("config: watch failed; reload disabled"));
                        unsafe { FindCloseChangeNotification(handle) };
                        return;
                    }
                    let stamp = config_stamp(&config_path);
                    if stamp == last {
                        continue;
                    }
                    last = stamp;
                    match std::fs::read_to_string(&config_path) {
                        Ok(contents) => {
                            match parse_runtime_config(config_path.clone(), &contents) {
                                Ok(config) => {
                                    log_message(format_args!(
                                        "config: reloading '{}'",
                                        config_path.display()
                                    ));
                                    publish_config(config);
                                }
                                // A typo must never unmask anyone, so the previous rules stay
                                // live rather than falling back to no filtering at all.
                                Err(err) => log_message(format_args!(
                                    "config: reload REJECTED, keeping the previous rules: {err}"
                                )),
                            }
                        }
                        Err(err) => log_message(format_args!(
                            "config: reload could not read '{}', keeping the previous rules: {err}",
                            config_path.display()
                        )),
                    }
                }
            });
    }

    /// A handle that the OS signals when something in `directory` changes.
    fn watch_directory(directory: &Path) -> Option<isize> {
        let mut wide: Vec<u16> = directory.as_os_str().encode_wide().collect();
        wide.push(0);
        let handle =
            unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 0, CONFIG_NOTIFY_FILTER) };
        if handle == INVALID_HANDLE_VALUE || handle == 0 {
            return None;
        }
        Some(handle)
    }

    /// Block until the watched directory changes, or until the safety cap expires. Returns false
    /// only when the watch itself broke; a timeout is a normal return that re-checks the file.
    fn wait_for_directory_change(handle: isize) -> bool {
        let status = unsafe { WaitForSingleObject(handle, CONFIG_WAIT_CAP_MILLIS) };
        if status == WAIT_TIMEOUT {
            return true;
        }
        if status != WAIT_OBJECT_0 {
            return false;
        }
        unsafe { FindNextChangeNotification(handle) != 0 }
    }

    /// Cheap change detector: modification time and size. An editor that writes in place and an
    /// editor that renames a temp file over the target both move at least one of the two.
    fn config_stamp(path: &Path) -> Option<(SystemTime, u64)> {
        let metadata = std::fs::metadata(path).ok()?;
        Some((metadata.modified().ok()?, metadata.len()))
    }

    fn load_config(module_path: &Path) -> Result<NameFilterRuntimeConfig, String> {
        let config_path = config_path_for_module(module_path)?;
        let contents = match std::fs::read_to_string(&config_path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                std::fs::write(&config_path, super::boilerplate_config()).map_err(|write_err| {
                    format!(
                        "default config '{}' was missing and could not be auto-created: {write_err}",
                        config_path.display()
                    )
                })?;
                log_message(format_args!(
                    "config: auto-created default '{}' beside loaded DLL; edit it to enable filtering",
                    config_path.display()
                ));
                return Ok(NameFilterRuntimeConfig {
                    path: config_path,
                    ..NameFilterRuntimeConfig::default()
                });
            }
            Err(err) => {
                return Err(format!("'{}' is unreadable: {err}", config_path.display()));
            }
        };
        let config = parse_runtime_config(config_path, &contents)?;
        log_message(format_args!(
            "config: loaded '{}' with {} group(s)",
            config.path.display(),
            config.groups.len()
        ));
        Ok(config)
    }

    fn install_hook(base: usize, group_count: usize) {
        log_message(format_args!(
            "install: {group_count} configured group(s), first match wins, per-group numbering enabled"
        ));
        install_one(
            "SessionManagerPlayerEntryBase::Copy",
            er_game_base::mem::game_data_addr(
                base,
                SESSION_MANAGER_PLAYER_ENTRY_BASE_COPY_RVA,
                "SESSION_MANAGER_PLAYER_ENTRY_BASE_COPY_RVA",
            ),
            SESSION_MANAGER_PLAYER_ENTRY_BASE_COPY_RVA,
            &SESSION_MANAGER_PLAYER_ENTRY_BASE_COPY_PROLOGUE,
            session_manager_player_entry_base_copy_hook as UnionFn,
            &SESSION_MANAGER_PLAYER_ENTRY_BASE_COPY_ORIG,
        );
        install_one(
            "PlayerGameData::CopyChrName",
            er_game_base::mem::game_data_addr(
                base,
                PLAYER_GAME_DATA_COPY_CHR_NAME_RVA,
                "PLAYER_GAME_DATA_COPY_CHR_NAME_RVA",
            ),
            PLAYER_GAME_DATA_COPY_CHR_NAME_RVA,
            &PLAYER_GAME_DATA_COPY_CHR_NAME_PROLOGUE,
            player_game_data_copy_chr_name_hook as UnionFn,
            &PLAYER_GAME_DATA_COPY_CHR_NAME_ORIG,
        );
        install_one(
            "GetPlayerChrName",
            er_game_base::mem::game_data_addr(
                base,
                GET_PLAYER_CHR_NAME_RVA,
                "GET_PLAYER_CHR_NAME_RVA",
            ),
            GET_PLAYER_CHR_NAME_RVA,
            &GET_PLAYER_CHR_NAME_PROLOGUE,
            get_player_chr_name_hook as UnionFn,
            &GET_PLAYER_CHR_NAME_ORIG,
        );
    }

    fn install_one(
        label: &str,
        target: usize,
        rva: usize,
        prologue: &[u8],
        handler: UnionFn,
        original: &'static AtomicUsize,
    ) {
        if !prologue_matches(label, target, prologue) {
            return;
        }
        match unsafe { er_hook::register_union_hook(target, handler, original) } {
            Ok(()) => log_message(format_args!(
                "install: hooked {label} @0x{target:x} (rva 0x{rva:x})"
            )),
            Err(status) => log_message(format_args!(
                "install: register_union_hook {label} @0x{target:x} failed: {status:?}"
            )),
        }
    }

    fn prologue_matches(label: &str, target: usize, prologue: &[u8]) -> bool {
        let mut actual = [0_u8; MAX_PROLOGUE_BYTES];
        let actual = &mut actual[..prologue.len()];
        if !unsafe { read_bytes(target, actual) } {
            log_message(format_args!(
                "DISARMED {label} @0x{target:x}: prologue unreadable"
            ));
            return false;
        }
        if actual != prologue {
            log_message(format_args!(
                "DISARMED {label} @0x{target:x}: prologue mismatch (got {actual:02x?}, want {prologue:02x?}) -- either this is not Elden Ring 1.16.2 or the entry is already detoured"
            ));
            return false;
        }
        true
    }

    unsafe extern "system" fn session_manager_player_entry_base_copy_hook(
        dst: usize,
        src: usize,
        unused_c: usize,
        unused_d: usize,
    ) -> usize {
        let original = SESSION_MANAGER_PLAYER_ENTRY_BASE_COPY_ORIG.load(Ordering::Acquire);
        let result = if original == ORIGINAL_UNSET {
            dst
        } else {
            let original_fn: UnionFn = unsafe { std::mem::transmute(original) };
            unsafe { original_fn(dst, src, unused_c, unused_d) }
        };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if result != 0 {
                unsafe {
                    apply_name_filter(&mut *(result as *mut RawSessionManagerPlayerEntryBase))
                };
            }
        }));
        result
    }

    fn apply_name_filter(entry: &mut RawSessionManagerPlayerEntryBase) {
        let Some(active) = active_filter() else {
            return;
        };
        if !active.enabled {
            return;
        }
        let filter = &active.filter;
        let Some(name) = read_inline_steam_name(&entry.steam_name) else {
            return;
        };
        let Some((group_id, replacement)) = filter.replacement_for(&name) else {
            return;
        };
        let identity = PlayerNameIdentity::for_entry(entry.steam_id, entry as *const _ as usize);
        let ordinal = {
            let assignments = PLAYER_NAME_GROUP_NUMBERS.get_or_init(Mutex::default);
            let mut assignments = assignments
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            assignments.ordinal_for(&identity, group_id)
        };
        let numbered = numbered_replacement(replacement, ordinal);
        write_inline_steam_name(&mut entry.steam_name, &numbered);
        let n = PLAYER_NAME_FILTER_REPLACEMENTS.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= FILTER_REPLACEMENT_LOG_LIMIT {
            log_message(format_args!(
                "replaced steam_id={} group='{}' ordinal={} original='{}' replacement='{}' count={n}",
                entry.steam_id, group_id, ordinal, name, numbered
            ));
        }
    }

    /// `CS::GetPlayerChrName(MenuString *out, PlayerIns *player, char decorate)`.
    ///
    /// The original runs first and produces the name the game would have drawn -- character
    /// name or Steam name, whichever the player's settings select -- and the match then runs
    /// against that produced string, so one detour covers both branches.
    unsafe extern "system" fn get_player_chr_name_hook(
        out: usize,
        player: usize,
        decorate: usize,
        unused_d: usize,
    ) -> usize {
        let original = GET_PLAYER_CHR_NAME_ORIG.load(Ordering::Acquire);
        let result = if original == ORIGINAL_UNSET {
            out
        } else {
            let original_fn: UnionFn = unsafe { std::mem::transmute(original) };
            unsafe { original_fn(out, player, decorate, unused_d) }
        };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if result != 0 && player != 0 {
                unsafe {
                    apply_chr_name_filter(&mut *(result as *mut RawMenuString), player, decorate)
                };
            }
        }));
        result
    }

    /// # Safety
    ///
    /// `menu_string` must be the `MenuString` `GetPlayerChrName` just filled, `player` the
    /// `PlayerIns` it was asked about, and `decorate` the flag it was called with.
    unsafe fn apply_chr_name_filter(
        menu_string: &mut RawMenuString,
        player: usize,
        decorate: usize,
    ) {
        let call = CHR_NAME_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
        let trace = call <= CHR_NAME_CALL_LOG_LIMIT;
        macro_rules! bail {
            ($($reason:tt)*) => {{
                if trace {
                    log_message(format_args!(
                        "chr-name call={call} skipped: {}",
                        format_args!($($reason)*)
                    ));
                }
                return;
            }};
        }
        let Some(active) = active_filter() else {
            bail!("filter not built yet");
        };
        if !active.enabled {
            bail!("master switch is off");
        }
        let filter = &active.filter;
        let base = GAME_BASE.load(Ordering::SeqCst);
        if base == 0 {
            bail!("game base unknown");
        }
        let entry =
            unsafe { *((player + PLAYER_INS_SESSION_MANAGER_PLAYER_ENTRY_OFFSET) as *const usize) };
        if entry == 0 {
            bail!("PlayerIns 0x{player:x} has no session-manager entry");
        }
        // An NPC reaches this function too, but leaves through MenuString::NpcName without a
        // session entry carrying a Steam ID, so this is also the NPC guard.
        let steam_id = unsafe { (*(entry as *const RawSessionManagerPlayerEntryBase)).steam_id };
        if steam_id == 0 {
            bail!("entry 0x{entry:x} has steam_id=0");
        }
        let Some(name) = (unsafe { read_menu_string(menu_string) }) else {
            bail!("steam_id={steam_id} produced an unreadable MenuString");
        };
        if is_issued_mask(&name) {
            bail!("steam_id={steam_id} already masked upstream as '{name}'");
        }
        let Some((group_id, replacement)) = filter.replacement_for(&name) else {
            bail!("steam_id={steam_id} name '{name}' matched no group");
        };
        let identity = PlayerNameIdentity::for_entry(steam_id, entry);
        let ordinal = {
            let assignments = PLAYER_NAME_GROUP_NUMBERS.get_or_init(Mutex::default);
            let mut assignments = assignments
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            assignments.ordinal_for(&identity, group_id)
        };
        let numbered = numbered_replacement(replacement, ordinal);
        if name == numbered {
            // Already ours -- the Copy hook renamed the Steam name this tag was built from.
            bail!("steam_id={steam_id} already reads '{numbered}'");
        }
        if !unsafe { write_menu_string(menu_string, &numbered, base) } {
            bail!("steam_id={steam_id} MenuString write failed for '{numbered}'");
        }
        // `decorate` is how the two surfaces are told apart: the overhead nameplate producers
        // pass false, and the multiplayer system announcements ("<name> has invaded", summon and
        // death messages) pass true so the name comes back wrapped in menu-text 0xbcc/0xbcd.
        let surface = if decorate == 0 { "tag" } else { "announce" };
        register_issued_mask(&numbered);
        let n = CHR_NAME_REPLACEMENTS.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= CHR_NAME_REPLACEMENT_LOG_LIMIT {
            log_message(format_args!(
                "replaced-chr-name surface={surface} steam_id={steam_id} group='{group_id}' ordinal={ordinal} original='{name}' replacement='{numbered}' count={n}"
            ));
        }
    }

    /// Put `text` into a `MenuString`, leaving it in the shape `GetPlayerChrName` itself leaves:
    /// `rawString` null, the text owned by the `DLString`.
    ///
    /// # Safety
    ///
    /// `menu_string` must point at a live `CS::MenuString` and `base` at the game module.
    unsafe fn write_menu_string(menu_string: &mut RawMenuString, text: &str, base: usize) -> bool {
        let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        if unsafe { overwrite_dl_string_in_place(&mut menu_string.dl_string, &units) } {
            menu_string.raw_string = std::ptr::null_mut();
            return true;
        }
        let Some(source) = (unsafe { replacement_dl_string(text, &units, base) }) else {
            return false;
        };
        unsafe { release_dl_string(&mut menu_string.dl_string) };
        // Through the 1.17 gate: `base + rva` calls the 1.16.2 address on a build that moved
        // DLString::copy, and this one writes through a caller-owned string.
        let Ok(copy_addr) =
            er_game_base::mem::game_rva_named(DL_STRING_COPY_RVA as u32, "DL_STRING_COPY_RVA")
        else {
            return false;
        };
        let copy: super::DlStringCopyFn = unsafe { std::mem::transmute(copy_addr) };
        unsafe { copy(&mut menu_string.dl_string, source) };
        menu_string.raw_string = std::ptr::null_mut();
        true
    }

    /// A `DLString` holding `text`, built once through `GLOBAL_MenuHeapAllocator` and then kept.
    ///
    /// # Safety
    ///
    /// `base` must point at the loaded game module.
    unsafe fn replacement_dl_string(
        text: &str,
        units: &[u16],
        base: usize,
    ) -> Option<*const RawDlStringWide> {
        let cache = REPLACEMENT_DL_STRINGS.get_or_init(Mutex::default);
        let mut cache = cache.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Some(&address) = cache.get(text) {
            return Some(address as *const RawDlStringWide);
        }
        let allocator = er_game_base::mem::read_global_ptr(
            base,
            MENU_HEAP_ALLOCATOR_POINTER_RVA,
            "MENU_HEAP_ALLOCATOR_POINTER_RVA",
        ) as *mut RawDlAllocator;
        if allocator.is_null() {
            log_message(format_args!(
                "chr-name: GLOBAL_MenuHeapAllocator is null; cannot build a replacement string"
            ));
            return None;
        }
        // Resolve before the allocation, so a refusal does not leak the box it would have filled.
        let Ok(from_u16_addr) = er_game_base::mem::game_rva_named(
            DL_STRING_FROM_U16_ARRAY_RVA as u32,
            "DL_STRING_FROM_U16_ARRAY_RVA",
        ) else {
            return None;
        };
        let built: *mut RawDlStringWide = Box::into_raw(Box::new(unsafe { std::mem::zeroed() }));
        let from_u16_array: super::DlStringFromU16ArrayFn =
            unsafe { std::mem::transmute(from_u16_addr) };
        unsafe { from_u16_array(built, units.as_ptr(), allocator) };
        cache.insert(text.to_owned(), built as usize);
        Some(built)
    }

    /// `CS::PlayerGameData::CopyChrName(PlayerGameData *pgd, const wchar_t *name)`.
    ///
    /// Unlike the other two hooks this one rewrites the INPUT: it hands the original a different
    /// string, so the masked name is what the game stores in `characterName`. Everything
    /// downstream -- the word-checked copy at `+0x8E8`, the nameplate, and any overlay reading
    /// `+0x9C` out of process memory -- then sees the mask without knowing anything happened.
    unsafe extern "system" fn player_game_data_copy_chr_name_hook(
        player_game_data: usize,
        name: usize,
        unused_c: usize,
        unused_d: usize,
    ) -> usize {
        let substitute = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            masked_character_name(player_game_data, name)
        }))
        .unwrap_or(None);
        let name = substitute.unwrap_or(name);
        let original = PLAYER_GAME_DATA_COPY_CHR_NAME_ORIG.load(Ordering::Acquire);
        if original == ORIGINAL_UNSET {
            return 0;
        }
        let original_fn: UnionFn = unsafe { std::mem::transmute(original) };
        unsafe { original_fn(player_game_data, name, unused_c, unused_d) }
    }

    /// The replacement string to hand `CopyChrName`, or `None` to let the real name through.
    fn masked_character_name(player_game_data: usize, name: usize) -> Option<usize> {
        let active = active_filter()?;
        if !active.enabled {
            return None;
        }
        let filter = &active.filter;
        if player_game_data == 0 || name == 0 {
            return None;
        }
        let is_main_player =
            unsafe { *((player_game_data + PLAYER_GAME_DATA_IS_MAIN_PLAYER_OFFSET) as *const u8) };
        if is_main_player != 0 {
            return None;
        }
        let original_name = unsafe { read_wide_c_string(name, CHARACTER_NAME_MAX_TEXT_UNITS + 1) }?;
        // An empty name is not a player. CopyChrName is called with "" while a remote
        // PlayerGameData is still being constructed and before the name packet arrives, and a
        // wildcard pattern matches the empty string, so without this guard every one of those
        // writes is masked -- observed live stamping 'Dongerino 1' thirteen times over on
        // nobody, and burning ordinal 1 so the first real player got numbered 2.
        if original_name.is_empty() {
            return None;
        }
        if is_issued_mask(&original_name) {
            return None;
        }
        let (group_id, replacement) = filter.replacement_for(&original_name)?;
        let identity = PlayerNameIdentity::CharacterName(original_name.clone());
        let ordinal = {
            let assignments = PLAYER_NAME_GROUP_NUMBERS.get_or_init(Mutex::default);
            let mut assignments = assignments
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            assignments.ordinal_for(&identity, group_id)
        };
        let numbered = numbered_replacement(replacement, ordinal);
        let clamped: String = numbered
            .chars()
            .take(CHARACTER_NAME_MAX_TEXT_UNITS)
            .collect();
        if clamped.encode_utf16().count() > CHARACTER_NAME_MAX_TEXT_UNITS {
            log_message(format_args!(
                "copy-chr-name: '{numbered}' exceeds the {CHARACTER_NAME_MAX_TEXT_UNITS}-unit characterName field even clamped; leaving '{original_name}' alone"
            ));
            return None;
        }
        let pointer = cached_wide_string(&clamped)?;
        register_issued_mask(&clamped);
        let n = COPY_CHR_NAME_REPLACEMENTS.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= COPY_CHR_NAME_REPLACEMENT_LOG_LIMIT {
            log_message(format_args!(
                "replaced-character-name group='{group_id}' ordinal={ordinal} original='{original_name}' replacement='{clamped}' count={n}"
            ));
        }
        Some(pointer)
    }

    /// A NUL-terminated UTF-16 buffer for `text`, allocated once and kept for the life of the
    /// process. `CopyChrName` only reads it during the call, but the set of distinct masks is
    /// small and bounded, so keeping them is cheaper than churning an allocation per packet.
    fn cached_wide_string(text: &str) -> Option<usize> {
        let cache = MASK_WIDE_STRINGS.get_or_init(Mutex::default);
        let mut cache = cache.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Some(&address) = cache.get(text) {
            return Some(address);
        }
        let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let leaked = Box::leak(units.into_boxed_slice());
        let address = leaked.as_ptr() as usize;
        cache.insert(text.to_owned(), address);
        Some(address)
    }

    fn register_issued_mask(text: &str) {
        let issued = ISSUED_MASKS.get_or_init(Mutex::default);
        let mut issued = issued.lock().unwrap_or_else(|poison| poison.into_inner());
        issued.insert(text.to_owned());
    }

    /// Whether this string is a mask this DLL already issued.
    ///
    /// The comparison ignores surrounding whitespace, because `GetPlayerChrName` called with
    /// `decorate = true` -- the multiplayer announcement banners -- returns the name wrapped in
    /// menu-text 0xbcc/0xbcd, which in English is a pair of spaces. Comparing the decorated
    /// string verbatim made `' Dongerino 5 '` look like a fresh name, so the banner re-matched
    /// and issued a second number: the tag and the overlay said "Dongerino 5" while the banner
    /// for the same player said "Dongerino 2".
    fn is_issued_mask(text: &str) -> bool {
        let Some(issued) = ISSUED_MASKS.get() else {
            return false;
        };
        let issued = issued.lock().unwrap_or_else(|poison| poison.into_inner());
        issued.contains(text.trim())
    }

    /// # Safety
    ///
    /// `pointer` must point at a NUL-terminated UTF-16 string of at most `max_units` units.
    unsafe fn read_wide_c_string(pointer: usize, max_units: usize) -> Option<String> {
        let pointer = pointer as *const u16;
        let mut units = Vec::new();
        for index in 0..max_units {
            let unit = unsafe { *pointer.add(index) };
            if unit == 0 {
                return String::from_utf16(&units).ok();
            }
            units.push(unit);
        }
        None
    }

    fn module_path(module: *mut c_void) -> Result<PathBuf, String> {
        let mut buf = [0u16; 32768];
        let len =
            unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
        if len == 0 || len >= buf.len() {
            return Err(format!("GetModuleFileNameW returned {len}"));
        }
        Ok(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
    }

    fn log_message(args: fmt::Arguments<'_>) {
        let path = game_directory_path()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
            .join(LOG_FILE_NAME);
        append_line(&path, format_args!("{args}"));
    }
}

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
///
/// Called by the Windows loader. Do not call directly.
pub unsafe extern "system" fn DllMain(
    module: *mut core::ffi::c_void,
    reason: u32,
    reserved: *mut core::ffi::c_void,
) -> i32 {
    unsafe { windows_runtime::dll_main(module, reason, reserved) }
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_player_name_filter_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(contents: &str) -> Result<NameFilterRuntimeConfig, String> {
        parse_runtime_config(
            PathBuf::from("C:\\Game\\er_player_name_filter.toml"),
            contents,
        )
    }

    #[test]
    fn parses_user_defined_labels_names_and_regexes() {
        let config = parse(
            r#"
player_name_filter.bigot.replacement = "I'm a bigot"
player_name_filter.bigot.names = ["Exact Steam name", 'Literal # hash']
player_name_filter.bigot.regexes = ["(?i)^awful.*name$", 'literal#regex']
player_name_filter.racist.label = "I'm a racist"
player_name_filter.racist.name = "One Exact Name"
"#,
        )
        .expect("name-filter config must parse");

        assert_eq!(
            config.path,
            PathBuf::from("C:\\Game\\er_player_name_filter.toml")
        );
        assert_eq!(config.groups.len(), 2);
        assert_eq!(config.groups[0].id, "bigot");
        assert_eq!(config.groups[0].replacement.as_deref(), Some("I'm a bigot"));
        assert_eq!(
            config.groups[0].exact_names,
            ["Exact Steam name", "Literal # hash"]
        );
        assert_eq!(
            config.groups[0].regexes,
            ["(?i)^awful.*name$", "literal#regex"]
        );
        assert_eq!(config.groups[1].id, "racist");
        assert_eq!(
            config.groups[1].replacement.as_deref(),
            Some("I'm a racist")
        );
        assert_eq!(config.groups[1].exact_names, ["One Exact Name"]);
    }

    #[test]
    fn first_matching_group_wins() {
        let config = parse(
            r#"
player_name_filter.first.replacement = 'first'
player_name_filter.first.names = ['SameName']
player_name_filter.second.replacement = 'second'
player_name_filter.second.regexes = ['^Same.*']
"#,
        )
        .expect("name-filter config must parse");
        let compiled = CompiledNameFilter::from_config(&config.groups);
        assert!(compiled.diagnostics.is_empty());
        assert_eq!(
            compiled.filter.replacement_for("samename"),
            Some(("first", "first"))
        );
    }

    #[test]
    fn matched_players_receive_stable_per_group_numbers() {
        let mut assignments = GroupNumberAssignments::default();
        let first = PlayerNameIdentity::SteamId(111);
        let second = PlayerNameIdentity::SteamId(222);
        let third = PlayerNameIdentity::SteamId(333);

        assert_eq!(assignments.ordinal_for(&first, "bigot"), 1);
        assert_eq!(numbered_replacement("I'm a bigot", 1), "I'm a bigot 1");
        assert_eq!(assignments.ordinal_for(&second, "bigot"), 2);
        assert_eq!(assignments.ordinal_for(&first, "bigot"), 1);
        assert_eq!(assignments.ordinal_for(&third, "racist"), 1);
        assert_eq!(numbered_replacement("I'm a racist", 1), "I'm a racist 1");
        assert_eq!(
            PlayerNameIdentity::for_entry(0, 0xfeed_beef),
            PlayerNameIdentity::EntryAddress(0xfeed_beef)
        );
    }

    #[test]
    fn invalid_groups_are_diagnosed_and_skipped() {
        let config = parse(
            r#"
player_name_filter.no_label.names = ['x']
player_name_filter.bad_regex.replacement = 'bad'
player_name_filter.bad_regex.regexes = ['[']
player_name_filter.valid.replacement = 'ok'
player_name_filter.valid.names = ['x']
"#,
        )
        .expect("name-filter config must parse");
        let compiled = CompiledNameFilter::from_config(&config.groups);
        assert_eq!(compiled.filter.groups.len(), 1);
        assert!(
            compiled
                .diagnostics
                .iter()
                .any(|item| item.contains("no replacement label")),
            "missing replacement must be diagnosed: {:?}",
            compiled.diagnostics
        );
        assert!(
            compiled
                .diagnostics
                .iter()
                .any(|item| item.contains("invalid regex")),
            "invalid regex must be diagnosed: {:?}",
            compiled.diagnostics
        );
    }

    #[test]
    fn unknown_filter_field_names_the_line() {
        let err = parse("player_name_filter.bigot.typo = \"x\"\n")
            .expect_err("unknown name-filter fields must fail loudly");
        assert!(
            err.contains("player_name_filter") && err.contains("line 1") && err.contains("typo"),
            "the error must name the bad key and line: {err}"
        );
    }

    #[test]
    fn unknown_top_level_key_names_the_line() {
        let err = parse("save_file = 'not owned by this DLL'\n")
            .expect_err("standalone config must not silently accept er-quickload keys");
        assert!(
            err.contains("unknown config key")
                && err.contains("line 1")
                && err.contains("save_file"),
            "the error must name the bad key and line: {err}"
        );
    }

    #[test]
    fn master_switch_defaults_on_and_parses_both_ways() {
        let on = parse_runtime_config(
            PathBuf::from("C:\\Game\\er_player_name_filter.toml"),
            "player_name_filter.everyone.replacement = 'Masked'\n",
        )
        .expect("a config without the switch parses");
        assert!(on.enabled, "an absent switch leaves filtering on");

        let off = parse_runtime_config(
            PathBuf::from("C:\\Game\\er_player_name_filter.toml"),
            "enabled = false\nplayer_name_filter.everyone.replacement = 'Masked'\n",
        )
        .expect("the switch parses");
        assert!(!off.enabled);
        assert_eq!(
            off.groups.len(),
            1,
            "switching off keeps the rules, it does not delete them"
        );
    }

    #[test]
    fn master_switch_rejects_a_non_boolean() {
        let err = parse_runtime_config(
            PathBuf::from("C:\\Game\\er_player_name_filter.toml"),
            "enabled = 'yes'\n",
        )
        .expect_err("only true and false are accepted");
        assert!(
            err.contains("line 1") && err.contains("expected true or false"),
            "the error names the line and what was wanted: {err}"
        );
    }

    #[test]
    fn config_path_is_dll_stem_toml() {
        assert_eq!(
            config_path_for_module(std::path::Path::new("C:\\mods\\er_player_name_filter.dll"))
                .expect("config path"),
            PathBuf::from("C:\\mods\\er_player_name_filter.toml")
        );
    }

    #[test]
    fn inline_steam_name_round_trips_and_truncates() {
        let mut name = RawDlInplaceUtf16String64 {
            base: RawDlUtf16String {
                vftable: 0,
                backing_string: core::ptr::null_mut(),
                length: 0,
                unk18: 0,
                char_size: 0,
                encoding: 0,
                flags: 0,
            },
            bytes: [0; STEAM_NAME_UTF16_CAPACITY],
            unk: 0,
        };
        write_inline_steam_name(&mut name, "I'm a bigot");
        assert_eq!(
            read_inline_steam_name(&name).as_deref(),
            Some("I'm a bigot")
        );
        assert_eq!(name.base.char_size, 2);
        assert_eq!(name.base.encoding, 1);
        assert_eq!(name.base.backing_string, name.bytes.as_mut_ptr());

        let long = "x".repeat(STEAM_NAME_UTF16_CAPACITY + 10);
        write_inline_steam_name(&mut name, &long);
        assert_eq!(name.base.length, STEAM_NAME_UTF16_MAX_TEXT_UNITS);
        assert_eq!(name.bytes[STEAM_NAME_UTF16_MAX_TEXT_UNITS], 0);
    }

    #[test]
    fn session_manager_entry_layout_matches_copy_offsets() {
        assert_eq!(
            std::mem::offset_of!(RawSessionManagerPlayerEntryBase, steam_id),
            0x10
        );
        assert_eq!(
            std::mem::offset_of!(RawSessionManagerPlayerEntryBase, steam_name),
            0x18
        );
        assert_eq!(
            std::mem::offset_of!(RawSessionManagerPlayerEntryBase, connection_ref_info),
            0xc0
        );
        assert_eq!(
            std::mem::offset_of!(RawSessionManagerPlayerEntryBase, voice_chat_member_ref_info),
            0xc8
        );
    }
}
