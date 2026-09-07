//! A live command loop, so exploring the running game stops costing a relaunch.
//!
//! # Why this exists
//!
//! Every structural question about the menu so far has cost a full launch: edit the DLL, rebuild,
//! tear the session down, boot for a minute, read one line of log, repeat. That is the wrong unit of
//! work for exploration, and it is why a single wrong offset (`pause_menu_grid` deriving a vtable
//! from a window that is not up yet) burned four launches before its own log line said so.
//!
//! # Why not Frida
//!
//! Frida would give read and call in one attach, which is exactly the shape wanted here. It is not
//! usable on this target: `frida.attach()` on the Wine/Proton `eldenring.exe` injects a bootstrapper
//! that segfaults inside the game -- "bootstrapper crashed with signal 11", process gone, measured
//! 2026-08-12 on a live session that was being held open. Attaching at spawn is a different code
//! path and might survive, but me3 owns the spawn: it launches the exe and injects the mod host, so
//! there is no point at which frida could own the process first without displacing the loader the
//! whole product depends on.
//!
//! This module gets the same capability from the other end. The harness is already in the process
//! from `DLL_PROCESS_ATTACH` -- attached at the beginning, by construction, which is the condition
//! frida needs and cannot have here. It runs on the game thread, so it can read anything and (once
//! the command set grows) call anything, with no injection and nothing to crash.
//!
//! # Protocol
//!
//! `er-harness-cmd.txt` (env override `ER_HARNESS_CMD_PATH`) holds:
//!
//! ```text
//! <sequence number>
//! <command>
//! ```
//!
//! The sequence number is what makes the file a queue rather than a repeating instruction: the
//! command runs when the number changes, so rewriting the file with the same number is inert and a
//! command cannot fire twice because a frame happened to re-read it. Results go to the harness log,
//! which is already redirected per-run.
//!
//! Commands are read-only by design. Exploration must not be able to break the session it is
//! exploring -- that is the property that makes "try again" cheap, and the reason a `call` verb is
//! not here yet rather than being added speculatively.
//!
//! ```text
//! scan <hex-qword> [max-hits]   every address in committed memory holding that qword
//! read <hex-addr> [n-qwords]    dump n qwords from an address
//! grid                          every CS::GridControl instance, with its selected cell
//! picker                        the save-file picker's own cursor, named rather than guessed
//! point <x> <y>                 move the menu pointer there, then report every grid's cell
//! release                       stop authoring the pointer; the game sees the real mouse again
//! sweep [x0 y0 x1 y1 nx ny]     walk a grid of coordinates and report every hovered-cell CHANGE
//! key <dik> [polls]            hold a DirectInput scancode for N polls, then release
//! click [polls]                hold the left mouse button for N polls, then release
//! vk <code> [polls]            hold a Win32 VIRTUAL KEY -- the channel a text field reads
//! type <text>                  type an ASCII string one character per poll (SHIFT handled)
//! backspace <n>                send N backspaces, one per poll
//! pad <id> [polls]             hold one FD4 pad virtual-key id (1000..1080), then release
//! padsweep [lo] [hi]           hold each pad id in turn, reporting every picker-cursor change
//! force <code> [polls]         answer the game's own menu-code query "pressed" for N polls
//! qstats                       which menu-query candidate is live, and what codes it polls
//! ```
//!
//! `sweep` is the verb that makes the others usable. Hunting a row by trying coordinates by hand
//! costs one command and one poll per guess, and a menu that answers cell 0 everywhere is
//! indistinguishable from a pointer that is not landing at all. So the sweep drives itself: it
//! holds one coordinate per poll -- the game needs a frame to hit-test, which is exactly why this
//! cannot be a loop inside a single command -- and logs only the coordinates where a hovered cell
//! changed. The output is therefore a coordinate-to-row map, and after one sweep a row is addressed
//! by index rather than searched for.
//!
//! ```text
//! ```
//!
//! `point` is the one verb that writes, and it writes the field a real mouse writes -- the pause
//! menu is a hit-test of that coordinate pair (proven by watching a user nudge move exactly one
//! GridControl's `+0xd4` while every other one held still). Pairing the write with the resulting
//! cell in a single command is what makes navigation WATCHED: the row is read back before anything
//! is confirmed, and on the Quit tab one row off is *Return to Desktop*.

use core::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery,
};

use crate::log::harness_log;
use crate::win32::read_usize;

/// How often the command file is consulted. A file read every frame would be a syscall per frame for
/// a path that is idle almost always; 12 frames is a fifth of a second at 60fps and still costs the
/// game thread almost nothing. It was 30, which put up to half a second of dead time in front of
/// every press -- the dominant cost when the drive is walking a menu one row at a time.
const POLL_INTERVAL_FRAMES: u64 = 12;
/// Chunk size for the memory walk -- same 64KB the title-owner scan uses.
const CHUNK_BYTES: usize = 0x10000;
/// Ceiling on reported hits, so a needle that turns out to be a common value (or a mistyped 0)
/// cannot flood the log and stall the game thread. Scans that hit this say so.
const DEFAULT_MAX_HITS: usize = 64;
/// Ceiling on the qwords one `read` prints, for the same reason.
const MAX_READ_QWORDS: usize = 64;

/// `CS::GridControl`'s vtable RVA on 1.17, declared once in `game_mem` (from `scripts/er-rtti-map.py`,
/// MSVC RTTI walked in `eldenring-deobf-1.17.bin`: `.?AVGridControl@CS@@` -> 0x142a94438). Restating
/// the literal here made two declarations of one address, which is the drift this crate's own
/// scanner would then disagree with itself about.
use crate::game_mem::GRID_CONTROL_VTABLE_RVA_1170;
/// Selected cell, off the pager's own comparisons against the extents at `+0xd0`/`+0xd8`/`+0xdc`.
const GRID_CONTROL_SELECTED_D4_OFFSET: usize = 0xd4;
/// The save-file picker's own cursor grid, embedded in the `05_010_ProfileSelect` dialog. Its
/// `+0xd4` is `DIALOG_SLOT_CURSOR_B0C_OFFSET` (`0xa38 + 0xd4 == 0xb0c`), and `+0xb08` is the bound.
const PROFILE_DIALOG_CURSOR_GRID_A38_OFFSET: usize = 0xa38;
const PROFILE_DIALOG_SLOT_BOUND_B08_OFFSET: usize = 0xb08;

/// Report the picker's cursor by name rather than leaving it as one of a dozen anonymous grids.
///
/// A bare scan lists every live `CS::GridControl` and cannot say which one the screen in front of
/// the user is driving. That ambiguity produced a false negative worth avoiding twice: a Right press
/// inside the file browser moved the picker's cursor while the drive was reading a stale pause-menu
/// grid, and reported that the press did nothing.
fn log_picker_cursor(prefix: &str) {
    let dialog = crate::key_inject::save_picker_dialog();
    if dialog == 0 {
        harness_log!("repl: {prefix} picker not up (dialog=0)");
        return;
    }
    let grid = dialog + PROFILE_DIALOG_CURSOR_GRID_A38_OFFSET;
    let cursor = unsafe { read_usize(grid + GRID_CONTROL_SELECTED_D4_OFFSET) }
        .map_or(-1, |v| (v & 0xffff_ffff) as i32);
    let bound = unsafe { read_usize(dialog + PROFILE_DIALOG_SLOT_BOUND_B08_OFFSET) }
        .map_or(-1, |v| (v & 0xffff_ffff) as i32);
    harness_log!(
        "repl: {prefix} PICKER dialog=0x{dialog:x} grid=0x{grid:x} cursor={cursor} bound={bound}"
    );
}

static LAST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static FRAME: AtomicU64 = AtomicU64::new(0);

/// In-flight sweep, or `None`. Held in a mutex rather than atomics because it is a multi-field
/// cursor over a 2-D walk and a torn update would sample a coordinate from two different steps.
static SWEEP: std::sync::Mutex<Option<Sweep>> = std::sync::Mutex::new(None);

/// While a sweep runs the poll interval drops to this, so a 48-point walk finishes in a few seconds
/// instead of half a minute. One poll per point is the floor: the game must be allowed a frame to
/// hit-test the coordinate before the cell is read back.
const SWEEP_POLL_INTERVAL_FRAMES: u64 = 5;

/// A held input that releases itself after N polls. Without this a press is either instantaneous
/// (gone before the game reads it) or permanent (the drive silently owns the key for the rest of the
/// run) -- and the menu needs an edge, so neither works.
struct Hold {
    /// `Some(dik)` for a DirectInput scancode, `None` for a virtual key.
    dik: Option<u8>,
    vk: Option<u8>,
    polls_left: u32,
}

static HOLD: std::sync::Mutex<Option<Hold>> = std::sync::Mutex::new(None);

/// Poll cadence while an input edge is down. A hold used to advance on `POLL_INTERVAL_FRAMES`, so the
/// default 3-poll press occupied 90 frames -- a second and a half of held key for an edge the game
/// hit-tests once per frame. 10 frames per poll keeps the default press at 30 frames (~0.5s at 60fps),
/// which is still five times a human's ~100ms tap, so the menu cannot miss it.
const HOLD_POLL_INTERVAL_FRAMES: u64 = 10;

/// Release an expired hold and report the grid cells afterwards. Returns whether a hold is still
/// down, so the caller knows the input edge has not finished yet.
fn advance_hold(base: usize) -> bool {
    let mut guard = match HOLD.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(hold) = guard.as_mut() else {
        return false;
    };
    if hold.polls_left > 0 {
        hold.polls_left -= 1;
        return true;
    }
    // Release every channel a hold could have engaged, not just the one it declared. A `force`
    // hold declares neither a scancode nor a virtual key, and leaving its answer latched would make
    // the menu see that code held for the rest of the session.
    crate::menu_query::clear_forced();
    crate::pad_inject::set_menu_vk_id(0);
    if hold.dik.is_some() {
        crate::key_inject::hold(0);
    }
    if hold.vk.is_some() {
        crate::key_inject::hold_vk(0);
    }
    *guard = None;
    drop(guard);
    // The release is the half that commits a menu action, so the cells are read after it rather
    // than while the key is still down.
    let vtable = base + GRID_CONTROL_VTABLE_RVA_1170;
    let (hits, _) = scan_for_qword(vtable, DEFAULT_MAX_HITS);
    for hit in &hits {
        let selected = unsafe { read_usize(hit + GRID_CONTROL_SELECTED_D4_OFFSET) }
            .map_or(-1, |v| (v & 0xffff_ffff) as i32);
        harness_log!("repl: after-release GridControl 0x{hit:x} selected_cell={selected}");
    }
    log_picker_cursor("after-release");
    false
}

/// An in-flight string, one character per poll. Typing cannot be a loop inside one command for the
/// same reason a sweep cannot: the game reads the keyboard once per frame, so N characters need N
/// separate frames or they collapse into one keystroke.
struct Typing {
    /// `(dik, needs_shift)` per character, already resolved -- an unmappable character is rejected
    /// when the command is parsed rather than silently skipped mid-path, because a save path missing
    /// one character fails as a wrong path, not as an obvious error.
    keys: Vec<(u8, bool)>,
    index: usize,
    /// Two-phase per character: press, then release. A held-through transition types one character.
    releasing: bool,
}

static TYPING: std::sync::Mutex<Option<Typing>> = std::sync::Mutex::new(None);

/// Commands read but not yet run. A command file used to be executed line by line the instant it was
/// read, which quietly made multi-line files useless: two `key` lines both wrote `HOLD`, so only the
/// last survived and the first press never happened. The consequence was that a whole menu drive had
/// to be issued one shell round-trip per press -- fourteen of them to reach a save slot, each with its
/// own multi-second wait, and any one of them able to land while the previous press was still down.
/// Queuing instead means one file can carry the entire drive and the poll loop paces it: exactly one
/// command starts per poll, and only once the previous press has been released.
static QUEUE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// ASCII to DirectInput scancode, plus whether shift is required. Only the characters a Windows save
/// path can contain are mapped; anything else returns `None` so the caller can refuse the whole
/// string.
fn ascii_to_dik(c: char) -> Option<(u8, bool)> {
    // Shifted specials first. They must be tested before the base table, not after: `:` is not in
    // the base table at all, so an arrangement that matches the base first returns `None` and the
    // whole string is refused -- which is exactly what happened to
    // `Z:\home\banon\...\ER0000.co2`, a path whose only unmappable character was its drive colon.
    match c {
        ':' => return Some((0x27, true)),
        '_' => return Some((0x0c, true)),
        '+' => return Some((0x0d, true)),
        '?' => return Some((0x35, true)),
        '"' => return Some((0x28, true)),
        '<' => return Some((0x33, true)),
        '>' => return Some((0x34, true)),
        '|' => return Some((0x2b, true)),
        '{' => return Some((0x1a, true)),
        '}' => return Some((0x1b, true)),
        '(' => return Some((0x0a, true)),
        ')' => return Some((0x0b, true)),
        '!' => return Some((0x02, true)),
        '@' => return Some((0x03, true)),
        '#' => return Some((0x04, true)),
        '$' => return Some((0x05, true)),
        '%' => return Some((0x06, true)),
        '^' => return Some((0x07, true)),
        '&' => return Some((0x08, true)),
        '*' => return Some((0x09, true)),
        '~' => return Some((0x29, true)),
        _ => {}
    }
    let lower = c.to_ascii_lowercase();
    let base = match lower {
        'a' => 0x1e,
        'b' => 0x30,
        'c' => 0x2e,
        'd' => 0x20,
        'e' => 0x12,
        'f' => 0x21,
        'g' => 0x22,
        'h' => 0x23,
        'i' => 0x17,
        'j' => 0x24,
        'k' => 0x25,
        'l' => 0x26,
        'm' => 0x32,
        'n' => 0x31,
        'o' => 0x18,
        'p' => 0x19,
        'q' => 0x10,
        'r' => 0x13,
        's' => 0x1f,
        't' => 0x14,
        'u' => 0x16,
        'v' => 0x2f,
        'w' => 0x11,
        'x' => 0x2d,
        'y' => 0x15,
        'z' => 0x2c,
        '1' => 0x02,
        '2' => 0x03,
        '3' => 0x04,
        '4' => 0x05,
        '5' => 0x06,
        '6' => 0x07,
        '7' => 0x08,
        '8' => 0x09,
        '9' => 0x0a,
        '0' => 0x0b,
        '-' => 0x0c,
        '=' => 0x0d,
        '[' => 0x1a,
        ']' => 0x1b,
        ';' => 0x27,
        '\'' => 0x28,
        '\\' => 0x2b,
        ',' => 0x33,
        '.' => 0x34,
        '/' => 0x35,
        ' ' => 0x39,
        _ => return None,
    };
    Some((base, c.is_ascii_uppercase()))
}

/// Advance an in-flight string by one press-or-release. Returns whether typing is still going.
fn advance_typing() -> bool {
    let mut guard = match TYPING.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(typing) = guard.as_mut() else {
        return false;
    };
    let Some(&(dik, shift)) = typing.keys.get(typing.index) else {
        harness_log!("repl: type complete ({} character(s))", typing.index);
        *guard = None;
        return false;
    };
    if typing.releasing {
        crate::key_inject::hold(0);
        crate::key_inject::hold_vk(0);
        typing.releasing = false;
        typing.index += 1;
    } else {
        // Shift rides the USER32 virtual-key channel while the character rides the DirectInput one.
        // They are different stages, and a shifted character needs both down at the same instant.
        if shift {
            crate::key_inject::hold_vk(0x10); // VK_SHIFT
        }
        crate::key_inject::hold(dik);
        typing.releasing = true;
    }
    true
}

/// A walk over the FD4 pad virtual-key id space, one id per poll.
///
/// Why the PAD and not another key. The binding table this drive already dumps says list up/down are
/// menu codes 0x2c/0x2d, and both read `kb=0xffffffff` -- unbound on the keyboard by design, with
/// only pad ids (9 and 10) attached. That is consistent with everything measured on the save-file
/// picker: Down, S and Tab leave the dialog byte-identical, and a 36-point mouse sweep across the
/// whole screen produced no cursor transition on any of the twelve live GridControls. The pad is the
/// one channel the table says exists and the one this drive has never injected.
///
/// It is a sweep rather than a single id because the binding table's small numbers (9, 10) and the
/// virtual-key array's 1000..1080 ids are two different numbering schemes, and the mapping between
/// them has never been measured -- guessing `1000 + 9` would be exactly the kind of assumption that
/// produces a silent no-op indistinguishable from a dead channel.
struct PadSweep {
    id: u32,
    hi: u32,
    releasing: bool,
    last_cursor: i32,
}

static PAD_SWEEP: std::sync::Mutex<Option<PadSweep>> = std::sync::Mutex::new(None);

/// The picker's cursor, or `i32::MIN` when the picker is not up.
fn picker_cursor() -> i32 {
    let dialog = crate::key_inject::save_picker_dialog();
    if dialog == 0 {
        return i32::MIN;
    }
    unsafe {
        read_usize(dialog + PROFILE_DIALOG_CURSOR_GRID_A38_OFFSET + GRID_CONTROL_SELECTED_D4_OFFSET)
    }
    .map_or(i32::MIN, |v| (v & 0xffff_ffff) as i32)
}

/// Advance a pad sweep by one press-or-release. Returns whether it is still running.
fn advance_pad_sweep() -> bool {
    let mut guard = match PAD_SWEEP.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(sweep) = guard.as_mut() else {
        return false;
    };
    if sweep.id > sweep.hi {
        crate::pad_inject::set_menu_vk_id(0);
        harness_log!("repl: padsweep complete");
        *guard = None;
        return false;
    }
    if sweep.releasing {
        crate::pad_inject::set_menu_vk_id(0);
        let cursor = picker_cursor();
        if cursor != sweep.last_cursor {
            harness_log!(
                "repl: padsweep id={} MOVED the picker cursor {} -> {cursor}",
                sweep.id,
                sweep.last_cursor
            );
            sweep.last_cursor = cursor;
        }
        sweep.releasing = false;
        sweep.id += 1;
    } else {
        sweep.releasing = true;
        if !crate::pad_inject::set_menu_vk_id(sweep.id) {
            harness_log!("repl: padsweep ABORTED -- no menu pad device observed yet");
            sweep.id = sweep.hi + 1;
        }
    }
    true
}

struct Sweep {
    x0: i32,
    y0: i32,
    step_x: i32,
    step_y: i32,
    nx: u32,
    ny: u32,
    index: u32,
    /// Cells from the previous point, so only changes are logged. A sweep that printed every point
    /// would bury the four or five transitions that matter under fifty identical lines.
    previous: Vec<(usize, i32)>,
}

impl Sweep {
    fn point(&self) -> Option<(i32, i32)> {
        if self.index >= self.nx * self.ny {
            return None;
        }
        let col = (self.index % self.nx) as i32;
        let row = (self.index / self.nx) as i32;
        Some((self.x0 + col * self.step_x, self.y0 + row * self.step_y))
    }
}

/// Advance an in-flight sweep by one point. Returns whether a sweep is still running, which is what
/// lets the poll interval drop only while one is active.
fn advance_sweep(base: usize) -> bool {
    let mut guard = match SWEEP.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(sweep) = guard.as_mut() else {
        return false;
    };
    let Some((x, y)) = sweep.point() else {
        harness_log!("repl: sweep complete after {} point(s)", sweep.index);
        *guard = None;
        return false;
    };
    crate::pad_inject::write_menu_pointer(x, y);
    crate::key_inject::hold_cursor(x, y);
    let vtable = base + GRID_CONTROL_VTABLE_RVA_1170;
    let (hits, _) = scan_for_qword(vtable, DEFAULT_MAX_HITS);
    let cells: Vec<(usize, i32)> = hits
        .iter()
        .map(|hit| {
            let cell = unsafe { read_usize(hit + GRID_CONTROL_SELECTED_D4_OFFSET) }
                .map_or(-1, |v| (v & 0xffff_ffff) as i32);
            (*hit, cell)
        })
        .collect();
    for (address, cell) in &cells {
        let was = sweep
            .previous
            .iter()
            .find(|(prev_address, _)| prev_address == address)
            .map(|(_, prev_cell)| *prev_cell);
        if was != Some(*cell) {
            harness_log!(
                "repl: sweep ({x},{y}) GridControl 0x{address:x} cell {} -> {cell}",
                was.map_or_else(|| "new".to_string(), |v| v.to_string())
            );
        }
    }
    sweep.previous = cells;
    sweep.index += 1;
    true
}

fn cur_proc() -> HANDLE {
    HANDLE((-1isize) as *mut c_void)
}

fn command_path() -> std::path::PathBuf {
    er_game_base::log::redirected_artifact_path("ER_HARNESS_CMD_PATH", "er-harness-cmd.txt")
}

/// Walk this process's committed, readable regions and hand each 64KB chunk to `visit` as
/// `(chunk_base, bytes)`. Returning `false` from `visit` stops the walk.
///
/// Fault-safe throughout: `ReadProcessMemory` on the pseudo-handle returns an error for a region the
/// game freed mid-walk instead of faulting the game thread, which matters because this runs while
/// the game is live and allocating.
fn walk_memory(mut visit: impl FnMut(usize, &[u8]) -> bool) {
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut address = 0usize;
    loop {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let written = unsafe {
            VirtualQuery(
                Some(address as *const c_void),
                &mut info,
                core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if written == 0 {
            return;
        }
        let region_base = info.BaseAddress as usize;
        let region_size = info.RegionSize;
        let readable = info.State == MEM_COMMIT
            && (info.Protect & (PAGE_GUARD | PAGE_NOACCESS)).0 == 0
            && info.Protect.0 != 0;
        if readable {
            let mut offset = 0usize;
            while offset < region_size {
                let len = CHUNK_BYTES.min(region_size - offset);
                let chunk_base = region_base + offset;
                let mut read = 0usize;
                let ok = unsafe {
                    ReadProcessMemory(
                        cur_proc(),
                        chunk_base as *const c_void,
                        buf.as_mut_ptr().cast(),
                        len,
                        Some(&mut read),
                    )
                };
                if ok.is_ok() && read != 0 && !visit(chunk_base, &buf[..read.min(len)]) {
                    return;
                }
                offset += len;
            }
        }
        let Some(next) = region_base.checked_add(region_size) else {
            return;
        };
        if next <= address {
            return; // no forward progress -- refuse to spin
        }
        address = next;
    }
}

/// Every address whose qword equals `needle`, capped at `max_hits`.
fn scan_for_qword(needle: usize, max_hits: usize) -> (Vec<usize>, bool) {
    let mut hits = Vec::new();
    let mut capped = false;
    let width = core::mem::size_of::<usize>();
    walk_memory(|chunk_base, bytes| {
        let usable = bytes.len() & !(width - 1);
        let mut i = 0usize;
        while i + width <= usable {
            let Ok(quad) = bytes[i..i + width].try_into() else {
                break;
            };
            if usize::from_ne_bytes(quad) == needle {
                hits.push(chunk_base + i);
                if hits.len() >= max_hits {
                    capped = true;
                    return false;
                }
            }
            i += width;
        }
        true
    });
    (hits, capped)
}

fn parse_usize(token: Option<&str>) -> Option<usize> {
    let token = token?;
    let stripped = token.strip_prefix("0x").unwrap_or(token);
    usize::from_str_radix(stripped, if stripped == token { 10 } else { 16 }).ok()
}

fn run_command(base: usize, line: &str) {
    let mut parts = line.split_whitespace();
    match parts.next() {
        Some("scan") => {
            let Some(needle) = parse_usize(parts.next()) else {
                harness_log!("repl: scan needs a value -- 'scan <hex-qword> [max-hits]'");
                return;
            };
            if needle == 0 {
                // Zero matches every zeroed qword in the address space; the walk would fill its cap
                // instantly with noise and say nothing. The title-owner scan refuses a zero needle
                // for the same reason.
                harness_log!("repl: REFUSED to scan for 0 -- it matches every zeroed qword");
                return;
            }
            let max_hits = parse_usize(parts.next()).unwrap_or(DEFAULT_MAX_HITS);
            let (hits, capped) = scan_for_qword(needle, max_hits);
            harness_log!(
                "repl: scan 0x{needle:x} -> {} hit(s){}",
                hits.len(),
                if capped { " (CAPPED)" } else { "" }
            );
            for hit in &hits {
                harness_log!("repl:   0x{hit:x}");
            }
        }
        Some("read") => {
            let Some(address) = parse_usize(parts.next()) else {
                harness_log!("repl: read needs an address -- 'read <hex-addr> [n-qwords]'");
                return;
            };
            let count = parse_usize(parts.next()).unwrap_or(8).min(MAX_READ_QWORDS);
            for index in 0..count {
                let at = address + index * 8;
                match unsafe { read_usize(at) } {
                    Some(value) => {
                        harness_log!("repl:   [0x{at:x}] +0x{:<4x} = 0x{value:x}", index * 8)
                    }
                    None => harness_log!("repl:   [0x{at:x}] +0x{:<4x} = <unreadable>", index * 8),
                }
            }
        }
        Some("grid") => {
            let vtable = base + GRID_CONTROL_VTABLE_RVA_1170;
            let (hits, capped) = scan_for_qword(vtable, DEFAULT_MAX_HITS);
            harness_log!(
                "repl: grid vtable=0x{vtable:x} -> {} instance(s){}",
                hits.len(),
                if capped { " (CAPPED)" } else { "" }
            );
            for hit in &hits {
                let selected = unsafe { read_usize(hit + GRID_CONTROL_SELECTED_D4_OFFSET) }
                    .map_or(-1, |v| (v & 0xffff_ffff) as i32);
                harness_log!("repl:   GridControl 0x{hit:x} selected_cell={selected}");
            }
        }
        Some("point") => {
            let (Some(x), Some(y)) = (
                parts.next().and_then(|t| t.parse::<i32>().ok()),
                parts.next().and_then(|t| t.parse::<i32>().ok()),
            ) else {
                harness_log!("repl: point needs two decimal coordinates -- 'point <x> <y>'");
                return;
            };
            // Both layers. The pad-device pair is the field the menu reads; the USER32 answer is
            // what refills that field each frame. Writing only the first was measured inert (five
            // coordinates, hovered cell never left 0), so the cursor answer is the one that matters
            // and the pad write is kept only so the very next hit-test does not lag a frame behind.
            let wrote = crate::pad_inject::write_menu_pointer(x, y);
            let held = crate::key_inject::hold_cursor(x, y);
            harness_log!("repl: point {x} {y} -> pad_wrote={wrote} cursor_held={held}");
            // Report the cell in the same command. A separate read would race the game's own
            // hit-test and the pointer's decay back to whatever the device reports, which is how a
            // pointer write gets scored as ineffective when it actually worked for one frame.
            let vtable = base + GRID_CONTROL_VTABLE_RVA_1170;
            let (hits, _) = scan_for_qword(vtable, DEFAULT_MAX_HITS);
            for hit in &hits {
                let selected = unsafe { read_usize(hit + GRID_CONTROL_SELECTED_D4_OFFSET) }
                    .map_or(-1, |v| (v & 0xffff_ffff) as i32);
                harness_log!("repl:   GridControl 0x{hit:x} selected_cell={selected}");
            }
        }
        Some("sweep") => {
            let x0 = parse_usize(parts.next()).unwrap_or(64) as i32;
            let y0 = parse_usize(parts.next()).unwrap_or(64) as i32;
            let x1 = parse_usize(parts.next()).unwrap_or(1856) as i32;
            let y1 = parse_usize(parts.next()).unwrap_or(1016) as i32;
            let nx = parse_usize(parts.next()).unwrap_or(8).max(1) as u32;
            let ny = parse_usize(parts.next()).unwrap_or(6).max(1) as u32;
            let step_x = if nx > 1 {
                (x1 - x0) / (nx as i32 - 1)
            } else {
                0
            };
            let step_y = if ny > 1 {
                (y1 - y0) / (ny as i32 - 1)
            } else {
                0
            };
            harness_log!(
                "repl: sweep {x0},{y0} .. {x1},{y1} step {step_x},{step_y} -> {} point(s); only CHANGES follow",
                nx * ny
            );
            let mut guard = match SWEEP.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = Some(Sweep {
                x0,
                y0,
                step_x,
                step_y,
                nx,
                ny,
                index: 0,
                previous: Vec::new(),
            });
        }
        Some("key") => {
            let Some(dik) = parse_usize(parts.next()) else {
                harness_log!(
                    "repl: key needs a scancode -- 'key <dik> [polls]' (0xc8 up, 0xd0 down, 0x1c return)"
                );
                return;
            };
            let polls = parse_usize(parts.next()).unwrap_or(3) as u32;
            let delivered = crate::key_inject::hold(dik as u8);
            harness_log!("repl: key 0x{dik:x} for {polls} poll(s) -> delivered={delivered}");
            let mut guard = match HOLD.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = Some(Hold {
                dik: Some(dik as u8),
                vk: None,
                polls_left: polls,
            });
        }
        Some("click") => {
            let polls = parse_usize(parts.next()).unwrap_or(3) as u32;
            let delivered = crate::key_inject::hold_vk(crate::key_inject::VK_LBUTTON);
            harness_log!("repl: click for {polls} poll(s) -> delivered={delivered}");
            let mut guard = match HOLD.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = Some(Hold {
                dik: None,
                vk: Some(crate::key_inject::VK_LBUTTON),
                polls_left: polls,
            });
        }
        Some("openmenu") => {
            // The pause menu is not opened by a KEYPRESS. `Phase::OpenPauseMenu` sets the
            // request-open-IngameTop flag at `CSPopupMenu+0x121` (reached through `CSMenuMan+0x80`),
            // which `CSPopupMenu::Update` consumes on the next frame -- it opened in one frame in run
            // br-20260905-194101-bd9d. That mechanism lived only inside the phase table, so a REPL
            // drive (which runs with `phases=0`) had no way to reach it and every attempt to open the
            // menu by sending a scancode failed silently. Exposed as a verb so the drive never has to
            // rediscover it.
            let opened = crate::input_inject::input_manager(base)
                .map(crate::input_inject::request_open_ingame_menu);
            match opened {
                Some(true) => harness_log!(
                    "repl: openmenu -> requested CSPopupMenu+0x121=1; CSPopupMenu::Update opens IngameTop next frame"
                ),
                Some(false) => harness_log!(
                    "repl: openmenu REFUSED -- the guard event id is already set, CSMenuMan+0x80 is null, or +0x121 is unreadable (menu already open, or no world)"
                ),
                None => harness_log!("repl: openmenu -- input manager not resolved yet"),
            }
        }
        Some("picker") => log_picker_cursor("picker"),
        Some("type") => {
            let text: String = parts.collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                harness_log!("repl: type needs text -- 'type <text>'");
                return;
            }
            let mut keys = Vec::with_capacity(text.len());
            for c in text.chars() {
                match ascii_to_dik(c) {
                    Some(pair) => keys.push(pair),
                    None => {
                        // Refuse the whole string. Skipping one character of a save path yields a
                        // path that is wrong rather than one that is obviously broken, and the
                        // failure then looks like "the save is missing".
                        harness_log!("repl: type REFUSED -- no scancode for {c:?} in {text:?}");
                        return;
                    }
                }
            }
            harness_log!("repl: type {text:?} -> {} character(s)", keys.len());
            let mut guard = match TYPING.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = Some(Typing {
                keys,
                index: 0,
                releasing: false,
            });
        }
        Some("backspace") => {
            let count = parse_usize(parts.next()).unwrap_or(1);
            harness_log!("repl: backspace x{count}");
            let mut guard = match TYPING.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = Some(Typing {
                keys: vec![(0x0e, false); count],
                index: 0,
                releasing: false,
            });
        }
        Some("pad") => {
            let Some(id) = parse_usize(parts.next()) else {
                harness_log!("repl: pad needs an id -- 'pad <1000..1080> [polls]'");
                return;
            };
            let polls = parse_usize(parts.next()).unwrap_or(3) as u32;
            let ok = crate::pad_inject::set_menu_vk_id(id as u32);
            harness_log!(
                "repl: pad id={id} delivered={ok} for {polls} poll(s), picker cursor={}",
                picker_cursor()
            );
            let mut guard = match HOLD.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = Some(Hold {
                dik: None,
                vk: None,
                polls_left: polls,
            });
        }
        Some("padsweep") => {
            let lo = parse_usize(parts.next()).unwrap_or(1000) as u32;
            let hi = parse_usize(parts.next()).unwrap_or(1080) as u32;
            harness_log!(
                "repl: padsweep {lo}..={hi}, picker cursor={} -- only CHANGES follow",
                picker_cursor()
            );
            let mut guard = match PAD_SWEEP.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = Some(PadSweep {
                id: lo,
                hi,
                releasing: false,
                last_cursor: picker_cursor(),
            });
        }
        Some("force") => {
            let Some(code) = parse_usize(parts.next()) else {
                harness_log!(
                    "repl: force needs a menu code -- 'force <code> [polls]' (0x2c down, 0x2d up)"
                );
                return;
            };
            let polls = parse_usize(parts.next()).unwrap_or(3) as u32;
            crate::menu_query::force_code(code as u32);
            harness_log!(
                "repl: force code=0x{code:x} for {polls} poll(s), picker cursor={}",
                picker_cursor()
            );
            let mut guard = match HOLD.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = Some(Hold {
                dik: None,
                vk: None,
                polls_left: polls,
            });
        }
        Some("qstats") => {
            let (a, b, seen) = crate::menu_query::stats();
            harness_log!("repl: menu-query hits A={a} B={b} codes_polled={seen:02x?}");
        }
        Some("vk") => {
            let Some(vk) = parse_usize(parts.next()) else {
                harness_log!(
                    "repl: vk needs a code -- 'vk <hex> [polls]' (0x0d Return, 0x1b Escape)"
                );
                return;
            };
            let polls = parse_usize(parts.next()).unwrap_or(3) as u32;
            // Why a separate VERB from `key`. `key` stamps a DirectInput SCANCODE, which is what
            // the game's menus read. The native SoftwareKeyboardJob behind the path field is a
            // Windows text surface and reads USER32 instead, so DirectInput cannot reach it at all:
            // Escape (DIK 0x01) and Return (DIK 0x1c) both produced no log line and no state change
            // while the field was open, and a 74-character `type` left the field showing its
            // prefilled path. Two channels, two verbs -- collapsing them would make "the key never
            // arrived" and "the key arrived at the wrong surface" look identical.
            let delivered = crate::key_inject::hold_vk(vk as u8);
            harness_log!("repl: vk 0x{vk:x} for {polls} poll(s) -> delivered={delivered}");
            let mut guard = match HOLD.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = Some(Hold {
                dik: None,
                vk: Some(vk as u8),
                polls_left: polls,
            });
        }
        Some("release") => {
            // Hand the mouse back. While a position is held, every `GetCursorPos` in the process is
            // answered with it -- including the user's own, if they take the session over. A drive
            // that can point must be able to stop pointing, or it silently owns the pointer for the
            // rest of the run.
            let released = crate::key_inject::release_cursor();
            harness_log!("repl: release -> cursor_released={released}");
        }
        Some(other) => {
            harness_log!(
                "repl: unknown command {other:?} \
                 (scan / read / grid / picker / point / sweep / key / vk / type / backspace / pad / padsweep / force / qstats / click / release)"
            );
        }
        None => {}
    }
}

/// Poll the command file. Call once per frame from the drive; it self-throttles and does nothing
/// until the sequence number in the file changes.
pub fn on_frame(base: usize) {
    // Install the menu-code query detours here rather than at DLL attach: the shared-hook union
    // resolves through the product DLL, and a frame tick is the point at which every native in the
    // profile is guaranteed loaded.
    crate::menu_query::ensure_installed();
    let sweeping = {
        let guard = match SWEEP.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.is_some()
    };
    let typing = {
        let guard = match TYPING.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.is_some()
    };
    let pad_sweeping = {
        let guard = match PAD_SWEEP.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.is_some()
    };
    let holding = {
        let guard = match HOLD.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.is_some()
    };
    let interval = if sweeping || typing || pad_sweeping {
        SWEEP_POLL_INTERVAL_FRAMES
    } else if holding {
        HOLD_POLL_INTERVAL_FRAMES
    } else {
        POLL_INTERVAL_FRAMES
    };
    if !FRAME
        .fetch_add(1, Ordering::Relaxed)
        .is_multiple_of(interval)
    {
        return;
    }
    if advance_pad_sweep() {
        // A pad sweep owns the poll: a command read mid-sweep could hold a second id alongside the
        // one under test, and the attribution of any movement would be lost.
        return;
    }
    if advance_typing() {
        // Typing owns the poll while it runs, for the same reason a sweep does: a command read
        // mid-string would interleave a press into the middle of a path.
        return;
    }
    if advance_hold(base) {
        // An input edge in flight owns the poll. Reading a new command mid-hold would start a second
        // press before the first has been released, and the menu would see one long press.
        return;
    }
    if advance_sweep(base) {
        // A sweep owns the pointer while it runs. Reading a new command here would let a stray
        // `point` fight it for the same field and make the resulting map unattributable.
        return;
    }
    // The queue is drained before a new file is read, so a drive already in progress finishes its
    // steps rather than being interrupted by a re-read of the same sequence number.
    if advance_queue(base) {
        return;
    }
    let Ok(text) = std::fs::read_to_string(command_path()) else {
        return;
    };
    let mut lines = text.lines();
    let Some(sequence) = lines.next().and_then(|l| l.trim().parse::<u64>().ok()) else {
        return;
    };
    if sequence == LAST_SEQUENCE.swap(sequence, Ordering::SeqCst) {
        return;
    }
    let mut guard = match QUEUE.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    // Replace, don't append. A new sequence number is a new instruction from the operator; leaving
    // a half-finished previous drive in front of it would run stale presses against a screen that
    // has moved on, which is worse than dropping them.
    guard.clear();
    for line in lines {
        let line = line.trim();
        if !line.is_empty() && !line.starts_with('#') {
            guard.push(line.to_string());
        }
    }
    if guard.len() > 1 {
        harness_log!(
            "repl: #{sequence} queued {} command(s); one starts per poll, and only after the previous press releases",
            guard.len()
        );
    }
}

/// Start the next queued command, if the drive is idle. Returns whether one was started.
fn advance_queue(base: usize) -> bool {
    let next = {
        let mut guard = match QUEUE.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_empty() {
            return false;
        }
        guard.remove(0)
    };
    let left = match QUEUE.lock() {
        Ok(g) => g.len(),
        Err(poisoned) => poisoned.into_inner().len(),
    };
    harness_log!("repl: > {next}  ({left} left in queue)");
    run_command(base, &next);
    true
}
