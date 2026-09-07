//! `er_npc_possess.dll` -- the "become an NPC" mod.
//!
//! # What it does
//!
//! Press the hotkey and you are the character you are looking at. `possess` is the engine; this
//! file is the shell around it -- a hot-reloadable config in the game directory, a keyboard hotkey
//! and a controller hotkey bound from it, a `FrameBegin` task that drives both, and a structured
//! line in `er-npc-possess.log` for every press saying what was requested and what happened to it.
//!
//! Layer 1 shipped everything except the possession and left [`engine`] as the seam; layer 2
//! filled that seam in and nothing above it had to change, which is the only real evidence that
//! the seam was cut in the right place.
//!
//! Layer 4 added the other half of "pick an NPC to control": [`spawn`] creates the creature named
//! in `[spawn]` rather than finding one the map placed, waits for it to become drivable, and takes
//! it away again afterwards. There is no residency restriction on which creature -- assets load on
//! demand and nothing on the spawn path validates the id -- so what replaces one is a deadline,
//! because a chr id with no assets does not fail, it waits forever.
//!
//! # What it touches in the game
//!
//! One recurring `FrameBegin` task and two Win32 input reads inside it, plus -- only while
//! something is possessed -- a handful of struct-field writes on two `ChrIns`. See `input` for
//! why polling rather than hooking is the smaller claim rather than the lazier one, and
//! [`possess::game`] for why the possession engine itself needed almost no addresses at all.
//!
//! It resolves **three** game function addresses and claims **one prologue**, and both numbers are
//! worth knowing because they were zero for two layers and every one of them is a thing that can
//! break on a game patch. [`moveset`] spends one address on `PlayAnimationByBehaviorName`, without
//! which no dodge in the game is reachable; [`spawn`] spends two, on creating and removing a
//! character, which are the two things that cannot be done by writing a field. The prologue is
//! `CS::CSFeManImp::UpdatePlayerComponents`, detoured by [`hud`] so the HP, FP and stamina bars
//! read the possessed creature. All four go through `game_rva_named`, so on a build with no
//! verified 1.16.2 -> 1.17 mapping the feature is refused rather than jumping into whatever now
//! occupies those bytes -- `er-hook` logs `HOOK REFUSED` and the bars keep showing your own
//! character. That is the whole of this DLL's footprint in the game image; it is recorded in
//! `scripts/me3-dll-conflicts.toml`, and no other shell in the suite hooks that function or its
//! caller.
//!
//! One more hook exists and is not a game prologue: hudhook's DX12 `Present`, which [`picker`]
//! installs the first time the creature list is opened and never otherwise. That is a swapchain
//! vtable slot, arbitrated by `er_build_watermark_core::overlay_host` so the process keeps exactly
//! one of them however many shells want to draw.
//!
//! The one thing it does that is dangerous is write `ChrCtrl+0x3b0`, because `ChrCtrl::Unref`
//! DLPanics on a non-null value there. Every path that can end a possession -- the hotkey, the
//! creature dying, the creature despawning, and `DLL_PROCESS_DETACH` below -- goes through
//! `possess::teardown`, which clears it whatever else failed.

// The config, settings, engine and edge-state modules are ungated on purpose: they are pure logic,
// so their tests run on the host where the windows-only game bindings do not exist.
mod camera;
mod config;
mod engine;
mod hud;
mod input;
mod log;
mod moveset;
#[cfg(windows)]
mod overlay;
mod picker;
mod possess;
mod settings;
mod spawn;
mod toml;

#[cfg(windows)]
use std::sync::{
    Mutex, Once,
    atomic::{AtomicUsize, Ordering},
};

#[cfg(windows)]
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp},
    fd4::FD4TaskData,
};
#[cfg(windows)]
use er_hotkey_config::{chord_name, pad::pad_chord_name};
#[cfg(windows)]
use fromsoftware_shared::{FromStatic, SharedTaskImpExt};
#[cfg(windows)]
use windows::Win32::{
    Foundation::HINSTANCE,
    System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH},
};

#[cfg(windows)]
use crate::{input::Edges, log::possess_log};

const DLL_MAIN_SUCCESS: i32 = 1;

/// Frames between status lines. At 60fps this is roughly every ten seconds.
#[cfg(windows)]
const STATUS_LOG_TICKS: usize = 600;

#[cfg(windows)]
static START: Once = Once::new();
#[cfg(windows)]
static TICKS: AtomicUsize = AtomicUsize::new(0);
/// Presses seen, whatever the engine did with them. The status line carries this so "nothing
/// happened" separates into "the key never fired" and "the key fired and there was nobody to tell".
#[cfg(windows)]
static PRESSES_SEEN: AtomicUsize = AtomicUsize::new(0);
/// The edge latches. A `Mutex` rather than a `static mut`: the FrameBegin task is the only writer,
/// so it is uncontended, and it costs one uncontended lock per frame to avoid the unsafe.
#[cfg(windows)]
static EDGES: Mutex<Option<Edges>> = Mutex::new(None);

#[cfg(windows)]
fn wait_for_task_instance() -> Option<&'static CSTaskImp> {
    // Bounded. An unbounded `loop { yield_now() }` here starved the wineserver on 1.17 when the
    // singleton did not turn up promptly; see er_game_base::wait for the measurement.
    er_game_base::wait::poll_until(|| unsafe { CSTaskImp::instance() }.ok())
}

/// One `FrameBegin` tick: sample the devices, take a reload if the engine will allow one, and turn
/// a press into a possession request.
#[cfg(windows)]
fn tick() {
    // Sample the pad before the reload, so a rebind can seed its latch from the buttons that are
    // down at that instant instead of clearing it and manufacturing a press.
    let buttons = input::read_pad_buttons();

    // The reload gate. A config reload must not land mid-animation -- the mapping tables are read
    // while an attack is playing, and swapping them under it would finish one character's swing
    // with another's. Only the engine knows whether the body is neutral, so it decides when the
    // file may be consumed. With no engine installed this is always open; see `engine`.
    if engine::accepts_reload()
        && let Some(update) = config::poll_reload()
    {
        config::log_update(&update);
        if update.bindings_moved() {
            // The latches are re-seated below, from the pad sample taken this frame. This is only
            // the reason the "now listening" line is about to appear.
            possess_log(format_args!(
                "config: a hotkey binding moved; re-seating the latches"
            ));
        }
    }

    let bindings = config::bindings();
    let ticks = TICKS.fetch_add(1, Ordering::Relaxed);

    let mut guard = match EDGES.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let edges = guard.get_or_insert_with(|| Edges::new(&bindings));
    if edges.rebind(&bindings, buttons) {
        possess_log(format_args!(
            "hotkey: now listening for keyboard={} gamepad={} radial={}",
            bindings
                .keyboard
                .map_or_else(|| "(none)".to_owned(), chord_name),
            pad_chord_name(bindings.gamepad),
            pad_chord_name(bindings.radial)
        ));
    }
    let keyboard_pressed = match bindings.keyboard {
        Some(chord) => input::keyboard_edge(chord, edges.keyboard_latch()),
        None => false,
    };
    let sample = edges.feed(buttons, keyboard_pressed);
    // The latches are advanced above whatever `enabled` says, so turning the mod back on mid-hold
    // does not read the button that was already down as a fresh press.
    drop(guard);

    // The PICKER ticks before the master-switch return, and the master switch is folded into the
    // picker's own `enabled` rather than short-circuiting past it. It has to be: the panel is
    // drawn from a snapshot the picker republishes each frame, so a `return` here with the list
    // up would freeze that snapshot on screen with no key left that could clear it -- `enabled =
    // false` also makes `take_confirm` unreachable, so neither the picker hotkey nor the possess
    // hotkey could dismiss it. Routing through `[picker] enabled` instead reuses the close path
    // that already exists, and keeps advancing the picker's latches for the same reason the
    // possess latches are advanced above.
    let mut picker_settings = config::picker();
    picker_settings.enabled &= bindings.enabled;
    if picker::tick(picker_settings, buttons) {
        // The list just opened, so the overlay now has to exist. Installed here rather than
        // inside the picker because the install waits on the game's window, takes a named mutex
        // and may end in `Hudhook::apply()`; none of that may happen while the picker's own lock
        // is held. It spawns a thread and returns immediately.
        overlay::install_once();
    }

    // `[hud] pages`, mirrored where the render thread can read it without the config lock. Every
    // tick rather than once at possession start, so the switch is as live as every other key in
    // the file -- a player who sets it to false while wearing something sees the panel go on the
    // next frame, not on the next possession.
    moveset::banner::set_enabled(config::hud().pages);

    if !bindings.enabled {
        return;
    }

    if sample.radial_changed {
        possess_log(format_args!(
            "radial: {} ({} held) -- RESERVED, there is no wheel to open until the mapping layer lands",
            if sample.radial_held { "DOWN" } else { "UP" },
            pad_chord_name(bindings.radial)
        ));
    }

    if let Some(source) = sample.possess_source() {
        PRESSES_SEEN.fetch_add(1, Ordering::Relaxed);
        // The PICKER gets this press first. While the creature list is up, the possess hotkey
        // chooses rather than possesses -- one key to learn instead of two, and "the key that
        // starts a possession also picks what to possess" is a sentence the config file can
        // print. `take_confirm` returns `None` whenever the list is closed, which is every frame
        // the picker is not in use, so the possession path below is unchanged.
        if let Some(creature) = picker::take_confirm() {
            let (from, to) = config::pick_target(creature.chr_id);
            possess_log(format_args!(
                "picker: chose {} (c{:04}) -- [target] staged {from} -> {to}. It applies at the \
                 NEXT press of the possess hotkey. To keep it past your next edit of \
                 {config}, put this in that file:  [target] mode = \"chr_id\"  chr_id = {}",
                creature.label(),
                creature.chr_id,
                creature.chr_id,
                config = config::CONFIG_FILE_NAME_FOR_LOG,
            ));
        } else {
            // Taking the request and promoting a staged `[target]` happen under one lock, so the
            // request the engine is handed is the one the log line describes.
            let (request, adopted) = config::take_request();
            if let Some((from, to)) = adopted {
                possess_log(format_args!("config: [target] ADOPTED {from} -> {to}"));
            }
            let report = engine::on_hotkey_edge(source, request);
            let binding = if source == "keyboard" {
                bindings
                    .keyboard
                    .map_or_else(|| "(none)".to_owned(), chord_name)
            } else {
                pad_chord_name(bindings.gamepad)
            };
            possess_log(format_args!("{}", report.line(&binding)));
        }
        // Not a `return` in the picker branch. `engine::tick_engine()` below has to run on every
        // frame -- it is what notices a possessed character dying or despawning -- so skipping it
        // on the one frame a pick was confirmed would leave a dead body possessed for a frame.
    }

    // The possession itself, one frame of it. After the hotkey edge, so a press and its first
    // frame land in that order; unconditional, because the engine has to notice a possessed
    // character dying or despawning on a frame nobody pressed anything.
    engine::tick_engine();

    // The second install trigger, and it is not redundant with the picker's. A possession does
    // not require the creature list -- `[target] mode = "lock_on"` is the default and never opens
    // it -- so an attack-set panel gated on the picker's install would draw nothing at all in
    // exactly the session that needs it. The live run of 2026-09-02 was that session:
    // `picker_overlay=false` for its whole length while `state=active` wore a creature.
    // Idempotent after the first call, which is one atomic swap.
    if moveset::banner::showing() {
        overlay::install_once();
    }

    if ticks.is_multiple_of(STATUS_LOG_TICKS) {
        let (state, presses) = engine::snapshot();
        // Every counter here names what it counts, and that is a correction rather than a style
        // choice. This line used to read `picker_overlay=true picker_draws=3747 picker_rows=0`,
        // where `picker_draws` was Present frames (not picker frames) and `picker_rows` is 0
        // whenever the list is closed -- so the honest reading of a closed picker looked exactly
        // like a picker that had rendered 3747 empty frames, and it was diagnosed as one. The
        // counters are now split: `overlay_frames` is the swapchain, `picker_panel_draws` and
        // `banner_draws` are the two panels, and each is zero for a reason its own name gives.
        possess_log(format_args!(
            "status: enabled={} keyboard={} gamepad={} radial={} engine_installed={} state={} presses_seen={} presses_handled={} pad_buttons=0x{buttons:04x} picker_open={} overlay_installed={} overlay_frames={} picker_panel_draws={} picker_rows={} banner_showing={} banner_draws={}",
            bindings.enabled,
            bindings
                .keyboard
                .map_or_else(|| "(none)".to_owned(), chord_name),
            pad_chord_name(bindings.gamepad),
            pad_chord_name(bindings.radial),
            engine::engine_installed(),
            state.name(),
            PRESSES_SEEN.load(Ordering::Relaxed),
            presses,
            picker::is_open(),
            overlay::installed(),
            overlay::frames(),
            picker::render::panel_draws(),
            picker::render::last_rows(),
            moveset::banner::showing(),
            overlay::banner_draws(),
        ));
    }
}

#[cfg(windows)]
fn spawn_game_task() {
    let _ = std::thread::Builder::new()
        .name("er-npc-possess-task".to_owned())
        .spawn(move || {
            possess_log(format_args!("game task thread waiting for CSTaskImp"));
            let Some(task) = wait_for_task_instance() else {
                possess_log(format_args!(
                    "CSTaskImp never appeared; this shell stays inert rather than spinning"
                ));
                return;
            };
            possess_log(format_args!("game task registering FrameBegin tick"));
            task.run_recurring(
                move |_data: &FD4TaskData| tick(),
                CSTaskGroupIndex::FrameBegin,
            );
        });
}

#[cfg(windows)]
fn install() {
    log::reset_log_file();
    let config = config::init_config();
    let bindings = config.bindings();
    possess_log(format_args!(
        "er-npc-possess attach: enabled={} hotkey={:?} ({}) gamepad_hotkey={:?} ({}) radial={:?} ({}) config={} -- edits to that file take effect while the game runs, no restart",
        bindings.enabled,
        config.keyboard_text,
        bindings
            .keyboard
            .map_or_else(|| "(none)".to_owned(), chord_name),
        config.gamepad_text,
        pad_chord_name(bindings.gamepad),
        config.radial_text,
        pad_chord_name(bindings.radial),
        config.config_path.display()
    ));
    possess_log(format_args!(
        "settings in force: [target] {} | {}",
        config.target().summary(),
        config.tables.summary()
    ));
    possess_log(format_args!(
        "moveset table: {} creatures classified offline. {} is rewritten on every possession and \
         lists what the one you are wearing can do, plus the reason for anything withheld -- it \
         is OUTPUT, so corrections go in {} under [chr.cNNNN] instead",
        moveset::table::chr_ids().count(),
        config::DERIVED_CONFIG_FILE_NAME,
        config::CONFIG_FILE_NAME_FOR_LOG,
    ));
    possess_log(format_args!(
        "spawn: [target] mode = \"spawn\" CREATES c{:04} in front of you rather than looking for a \
         character to borrow. Any four-digit creature works -- assets load on demand and nothing \
         checks the id against the current map -- but an id with no chrbnd on disk does not fail, \
         it WAITS, so the {} ms deadline is what turns a bad pick into a message. {} names the \
         stage it died at",
        config.target().spawn.chr_id,
        config.target().spawn.readiness_ms,
        config::DERIVED_CONFIG_FILE_NAME,
    ));

    // The HUD layer, and the crate'S only detour. Installed here rather than lazily on the first
    // possession: patching the game image is a thing to do once, on our own install thread, not on
    // a game task the first time somebody presses a key. `[hud] enabled = false` skips it
    // entirely, so a player who does not want the feature carries no patched bytes at all.
    hud::install(config.tables.hud.enabled);

    // Stack layers 2, 3 and 4. Pressing the key writes a forwarding thunk into the target's
    // `ChrCtrl+0x3b0`, points `WorldChrManDbg+0xb8` at it, and co-locates the player's own
    // (invisible, silent, invincible, non-attacking) body with it every frame; the four face
    // inputs fire that creature's own attacks out of the offline-classified table; and in
    // `mode = "spawn"` the creature is one this mod asked the game to create.
    let installed = engine::install_engine(Box::new(possess::NpcPossessionEngine::new()));
    possess_log(format_args!(
        "possession engine: {} -- build={} | attacks fire by writing \
         CSChrEventModule::requestAnimationId, so this layer still resolves no game function \
         address. Lock-on and aiming are BOTH one field write each now: the worn creature goes \
         on teamType Charmed, which is what makes real enemies lockable instead of the player's \
         own body, and its wantToMoveTo is aimed from the camera every frame, which is the only \
         way to point an EnemyIns's attacks (the camera-to-aim path is gated on IsPlayerIns, a \
         vtable predicate the camera override cannot move). NOT in this layer: untargetable \
         (IsLockOnDisabled reads the SpEffect-accumulated modifier block, so it needs a SpEffect \
         row rather than a field write), and range is measured to the nearest enemy rather than \
         to a lock-on target",
        if installed {
            "INSTALLED"
        } else {
            "REFUSED -- one was already installed, and two engines writing one ChrIns is not \
             something to resolve by install order"
        },
        er_game_base::game_build::describe_build()
    ));
    spawn_game_task();
}

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
/// Standard Windows `DllMain`; on attach it only starts an installer thread.
pub unsafe extern "system" fn DllMain(
    module: HINSTANCE,
    reason: u32,
    reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // Stashed, not used: the overlay needs a module handle to hand hudhook, and it installs
        // only when the creature list is first opened or the first possession publishes an
        // attack-set panel. A session that does neither therefore still hooks nothing at all.
        overlay::arm(module.0 as usize);
        // A `rust_panic` in a cdylib loaded into the game is otherwise anonymous: the message goes
        // to a stderr nobody reads, and what survives is a 0xe06d7363 record naming the module and
        // nothing else. Every cdylib links its own copy of er-game-base, so this is per-DLL.
        //
        er_game_base::panic_report::report_panics_to("er-npc-possess", crate::possess_log);
        // The refusal sink, and it must be installed before anything resolves an address. Every
        // cdylib statically links its own copy of `er-hook` and `er-game-base`, so the logger they
        // call through is a per-DLL static and an uninstalled one is silent per DLL. The two lines
        // it carries are the ones that say a feature just went inert -- `HOOK REFUSED` and
        // `ADDRESS REFUSED` -- and without it `MH_ERROR_UNSUPPORTED_FUNCTION` is ambiguous between
        // "MinHook cannot hook this" and "the build gate refused the address", which are different
        // problems with different fixes. One call installs both sinks.
        er_hook::set_hook_logger(crate::possess_log);
        START.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-npc-possess-install".to_owned())
                .spawn(install);
        });
    }
    // The one TEARDOWN that is not optional. A possession leaves a pointer to our memory in the
    // possessed character's `ChrCtrl+0x3b0`, and `ChrCtrl::Unref` DLPanics on a non-null slot -- so
    // a DLL that unloads without clearing it arms a crash for whenever that character is next torn
    // down, in a process where our code is no longer present to explain it.
    if reason == DLL_PROCESS_DETACH {
        engine::shutdown_engine();
        // ...AND the detour, which is the other pointer to our memory the game is holding. The
        // release above disarmed the post-pass; this decides whether the five patched bytes come
        // back out. `lpReserved` is how `DllMain` says which kind of detach this is: NULL means a
        // real `FreeLibrary` -- the game keeps running with our code unmapped, so a detour still
        // pointing into it would jump into nothing on the next frame and the bytes must be
        // reverted. Non-NULL means the process is exiting, where MinHook's thread suspension under
        // the loader lock is the bigger hazard and there is nothing left to protect.
        hud::shutdown(!reserved.is_null());
    }
    DLL_MAIN_SUCCESS
}

// If this module wins the IMGUI context, every other overlay in the process has to be able to
// find it by name. `overlay_host::register_with_host` locates the host by looking this export up
// on each loaded module, so a host that does not define it is a host nobody can register with --
// and every other overlay in the profile silently draws nothing. That is the #336 regression the
// arbitration exists to prevent, and omitting this line reintroduces it. The picker installs
// lazily, so this DLL normally loses the claim to a shell that installs at attach; "normally" is
// a timing accident, not a guarantee.
#[cfg(windows)]
er_build_watermark_core::export_overlay_host!();

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_npc_possess_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}
