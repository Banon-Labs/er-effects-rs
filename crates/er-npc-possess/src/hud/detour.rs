//! The crate'S first and only detour, and everything that follows from that.
//!
//! Layers 1 and 2 install no hook at all and layer 3 spends exactly one resolved address on a
//! call. This layer writes five bytes into the game image, which is a different order of claim,
//! so the reasoning is written down here rather than assumed.
//!
//! # What is hooked, and why it is the right seam
//!
//! `CS::CSFeManImp::UpdatePlayerComponents` -- 1.16.2 `0x140772a80`, 1.17 `0x140773900` -- is the
//! per-frame pass that fills the HUD's `FrontEndViewValues` from the local player. It reads the
//! raw `WorldChrManImp+0x1e508` exactly once, into `r12`, and carries it through all 1,366
//! instructions; `WorldChrManDbg+0xb8 camOverrideChrIns` (which is how possession moves the
//! camera) is not consulted, so without this the bars keep showing the abandoned body.
//!
//! The detour calls the original **unchanged** and then overwrites eight ints. That ordering is
//! the entire design:
//!
//! * Runes, equipment, the great rune and the spell slots come from `PlayerGameData`
//!   (vtable `+0x168`) and `GetWeaponGaitemHandleBySlot` (vtable `+0x230`) during the original
//!   call, and are never touched afterwards -- so they keep reading the real player, which is
//!   what a creature cannot supply. An `EnemyIns` returns 0 from `+0x230` and its `GetChrAsm`
//!   slots answer 0, so retargeting the whole function would empty the armament HUD.
//! * The post-pass calls no game code and dereferences one pointer chain the engine itself walks
//!   (`ChrIns+0x190` -> `+0x00`), so there is no vtable dispatch on a creature and nothing that
//!   could reach `CSSessionManager` voice chat or the quickmatch manager the way a swapped
//!   `r12` would.
//! * Nothing here goes near `IsMainPlayerIns`, which is what gates every save write. The
//!   dangerous `+0x1e508` readers -- `UpdateSaveRelatedData`, `AddOrRemoveItem`, `RevivePlayer`,
//!   `UploadPcInfo` and the rest -- are not on this path and must stay off it.
//!
//! Last write wins, and that is checked rather than hoped: all eleven later `Update*` passes in
//! `CSFeManImp::Update` were disassembled, and every write they make to `[reg+0x84/88/8c/90/98/
//! a4/ac/b8]` is RSP/RBP-relative stack, never a `CSFeManImp`.
//!
//! # The signature has a float in it, which rules out the hook union
//!
//! `void UpdatePlayerComponents(CSFeManImp* this /*rcx*/, float deltaTime /*xmm1*/)` -- Ghidra's
//! recovered signature, and independently visible in the prologue (`movaps xmm6,xmm1` at
//! +0x41, feeding a `subss` against a global accumulator) and at the call site (`movaps
//! xmm1,xmm6` before every call). `er_hook::register_shared_hook` takes a `UnionFn`, four
//! `usize`s, and its own docs say "Not for float-arg or >4-stack-arg targets": routing through it
//! would let the compiler clobber `xmm1` before the trampoline ran, feeding the HUD's timing
//! accumulator garbage. So this is a bare [`MhHook`] with a correctly typed detour, and the
//! address is recorded in `scripts/me3-dll-conflicts.toml` as unshared so that a second DLL
//! arriving on this prologue is a gate failure rather than a silently dropped hook.
//!
//! (The call site also loads `r8`, which is not a third parameter -- every `r8` touch in the
//! body is a destination. It is dead setup shared with the sibling call above it.)
//!
//! # Inert unless something is possessed
//!
//! [`TARGET`] is zero whenever no possession is running, and the detour's whole body in that case
//! is: call the original, load one atomic, return. Nothing is read, nothing is written, and the
//! cost is a relaxed load on a function that was already running every frame.

use core::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use eldenring::cs::CSChrDataModule;
use er_game_base::game_build::{describe_build, game_file_version};
use er_game_base::mem::{game_rva_for_hook, safe_read_f32, safe_read_i32, safe_read_usize};
use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};

use crate::hud::derived::Off;
use crate::hud::layout::Layout;
use crate::hud::vitals::Source;
use crate::log::possess_log;
use crate::possess::layout::{chr_ins, modules};

/// Compile-time cross-check of this module's source offsets against `fromsoftware-rs`'s model of
/// `CSChrDataModule`, which was derived separately. The 1.16.2 named dump agrees with both
/// (`getStructure CSChrDataModule`: `hp` `+0x138`, `hpMax` `+0x13c`, `hpMaxUncapped` `+0x140`,
/// `fp` `+0x148`, `fpMax` `+0x14c`, `stamina` `+0x154`, `staminaMax` `+0x158`,
/// `recoverableHpLeft` `+0x160`), so a failure here means one of three independently derived
/// layouts moved and somebody has to find out which.
const _: () = {
    let measured = crate::hud::layout::MEASURED.source;
    assert!(core::mem::offset_of!(CSChrDataModule, hp) == measured.hp);
    assert!(core::mem::offset_of!(CSChrDataModule, max_hp) == measured.hp_max);
    assert!(core::mem::offset_of!(CSChrDataModule, max_uncapped_hp) == measured.hp_max_uncapped);
    assert!(core::mem::offset_of!(CSChrDataModule, fp) == measured.fp);
    assert!(core::mem::offset_of!(CSChrDataModule, max_fp) == measured.fp_max);
    assert!(core::mem::offset_of!(CSChrDataModule, stamina) == measured.stamina);
    assert!(core::mem::offset_of!(CSChrDataModule, max_stamina) == measured.stamina_max);
    assert!(core::mem::offset_of!(CSChrDataModule, recoverable_hp) == measured.recoverable_hp);
};

/// `CS::CSFeManImp::UpdatePlayerComponents`, 1.16.2 RVA `0x772a80`.
///
/// The 1.17 counterpart is `0x773900`, and the pair is registered in
/// `docs/recon/rva-map-1162-to-1170.verified.tsv` with the verdict
/// `IDENTICAL-WHOLE 1.000 over 1366 insns BOTH-ENTRIES PDATA:0x16c2/0x16c2` -- an exhaustive
/// comparison of both bodies, with each image's own `.pdata` confirming the address is a function
/// start in both. That is what `er-game-base/build.rs` requires before an address may carry a
/// detour rather than merely a call, and it is what makes the translation exist at all.
///
/// The 21-byte prologue
/// `48 8b c4 55 56 57 41 54 41 55 41 56 41 57 48 8d a8 a8 fd ff ff` is byte-identical in both
/// images and its first five bytes are a clean rel32-free detour window. Extended to 51 bytes it
/// matches uniquely in each image, at exactly these two addresses.
const UPDATE_PLAYER_COMPONENTS_RVA: u32 = 0x0077_2a80;

/// MinHook's trampoline back to the real function.
///
/// Zero until the hook is armed. The detour refuses to run rather than skipping the original: not
/// calling it would blank the entire HUD, which is a far worse failure than the retarget not
/// happening.
static ORIG: AtomicUsize = AtomicUsize::new(0);

/// The possessed creature's `ChrIns`, or 0 for "not possessing".
///
/// The inert check. One relaxed load per frame decides whether any of this module's code runs.
static TARGET: AtomicUsize = AtomicUsize::new(0);

/// The measured offsets for the running build, published once at install.
///
/// A `OnceLock` rather than a `Mutex` because the detour reads it every frame and must not take a
/// lock on the game's render path; it is written exactly once, before the hook is enabled.
static LAYOUT: std::sync::OnceLock<Layout> = std::sync::OnceLock::new();

/// `MhHook` for the life of the process, so the detour can be removed if this DLL is ever
/// unloaded.
///
/// `MhHook` holds three raw pointers -- the patched site in the game image, our detour, and
/// MinHook's trampoline -- which makes it `!Send` by inference rather than by intent. None is
/// thread-affine: all three are process-lifetime addresses in mapped executable memory, and
/// MinHook's own API is called from arbitrary threads throughout this repo. The claim is spelled
/// out here rather than worked around, because the alternative is stashing the resolved address
/// separately, and the only way to obtain it is to resolve a second time -- which is exactly the
/// silent wrong-function bug `scripts/check-double-resolved-hook-targets.py` exists to prevent.
struct InstalledHook(MhHook);
// SAFETY: see the doc comment above -- the three pointers are process-lifetime, non-thread-affine
// addresses in mapped memory, and MinHook's own enable/disable API is thread-agnostic.
unsafe impl Send for InstalledHook {}

static HOOK: Mutex<Option<InstalledHook>> = Mutex::new(None);

/// What [`install`] did, so the log and the derived report can say the same thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Install {
    /// The detour is live.
    Installed,
    /// `[hud] enabled = false`; nothing was written to the game image.
    Disabled,
    /// The running build has no measured offsets; nothing was written to the game image.
    UnmeasuredBuild,
    /// The address had no verified mapping for this build, or MinHook declined. `er-hook` has
    /// already logged `HOOK REFUSED` naming the address and the build.
    Refused,
}

impl Install {
    /// How the derived report explains this state to a player whose bars did not move.
    pub(crate) const fn off_reason(self) -> Option<Off> {
        match self {
            Self::Installed => None,
            Self::Disabled => Some(Off::Disabled),
            Self::UnmeasuredBuild => Some(Off::UnmeasuredBuild),
            Self::Refused => Some(Off::HookRefused),
        }
    }
}

/// The install outcome, kept so the derived report does not need it threaded through the engine.
///
/// A plain `AtomicUsize` holding a discriminant rather than a lock: it is written once during
/// install and read only when a possession starts.
static INSTALL_OUTCOME: AtomicUsize = AtomicUsize::new(OUTCOME_NOT_ATTEMPTED);
const OUTCOME_NOT_ATTEMPTED: usize = 0;
const OUTCOME_INSTALLED: usize = 1;
const OUTCOME_DISABLED: usize = 2;
const OUTCOME_UNMEASURED: usize = 3;
const OUTCOME_REFUSED: usize = 4;

/// What [`install`] concluded. `Install::Refused` also covers "install never ran", which cannot
/// happen in the shipped DLL and is the safe reading if it ever did.
pub(crate) fn outcome() -> Install {
    match INSTALL_OUTCOME.load(Ordering::Acquire) {
        OUTCOME_INSTALLED => Install::Installed,
        OUTCOME_DISABLED => Install::Disabled,
        OUTCOME_UNMEASURED => Install::UnmeasuredBuild,
        _ => Install::Refused,
    }
}

fn record(outcome: Install) -> Install {
    INSTALL_OUTCOME.store(
        match outcome {
            Install::Installed => OUTCOME_INSTALLED,
            Install::Disabled => OUTCOME_DISABLED,
            Install::UnmeasuredBuild => OUTCOME_UNMEASURED,
            Install::Refused => OUTCOME_REFUSED,
        },
        Ordering::Release,
    );
    outcome
}

/// Install the detour. Called once, from the DLL's install thread.
///
/// Returns without touching the game image on any build whose offsets nobody has measured -- the
/// address translation would still be refused a step later, but refusing here means the reason in
/// the log is the true one ("no measured offsets") rather than the downstream one.
pub(crate) fn install(enabled: bool) -> Install {
    if !enabled {
        possess_log(format_args!(
            "hud: [hud] enabled = false, so NO detour was installed and this DLL patches no bytes \
             of the game image. The HP/FP/stamina bars will keep showing your own character while \
             you possess something"
        ));
        return record(Install::Disabled);
    }
    let Some(layout) = Layout::for_build(game_file_version()) else {
        possess_log(format_args!(
            "hud: no measured FrontEndViewValues/CSChrDataModule offsets for {} -- the HP/FP/\
             stamina retarget is DISABLED and no detour was installed. The offsets are the same \
             on 1.16.2 and 1.17, but ChrIns grew 8 bytes at +0x3b8 on 1.17 with its size \
             unchanged, so a stale offset on this game reads the neighbouring field instead of \
             faulting -- there is nothing to notice at runtime, which is why this refuses rather \
             than reuses",
            describe_build()
        ));
        return record(Install::UnmeasuredBuild);
    };
    let _ = LAYOUT.set(layout);

    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            possess_log(format_args!("hud: MH_Initialize failed: {status:?}"));
            return record(Install::Refused);
        }
    }

    // Unresolved on purpose. `MhHook::new` translates 1.16.2 -> 1.17 internally through the
    // detour map, and resolving here first would hand it an already-resolved address to resolve
    // again -- which is not merely redundant: a 1.17 destination can also be some other row's
    // 1.16.2 source, and the second lookup then lands on a third, unrelated function with no
    // error and no log line. `game_rva_for_hook` is the module-base lookup and nothing else.
    let target = match game_rva_for_hook(UPDATE_PLAYER_COMPONENTS_RVA) {
        Ok(address) => address,
        Err(why) => {
            possess_log(format_args!("hud: could not find the game module: {why}"));
            return record(Install::Refused);
        }
    };

    let hook = match unsafe {
        MhHook::new(
            target as *mut c_void,
            update_player_components_hook as *mut c_void,
        )
    } {
        Ok(hook) => hook,
        Err(status) => {
            possess_log(format_args!(
                "hud: MhHook::new(CSFeManImp::UpdatePlayerComponents @0x{target:x}) failed: \
                 {status:?} -- the HP/FP/stamina bars will keep showing your own character. On an \
                 unrecognised build this is the CORRECT outcome: er-hook refuses an address it \
                 cannot verify rather than detouring whatever code now occupies it, and logs HOOK \
                 REFUSED beside this line"
            ));
            return record(Install::Refused);
        }
    };

    // Before the enable, always. The detour can fire on the very next frame, and one that finds a
    // zero trampoline declines to call the original -- which would blank the whole HUD.
    ORIG.store(hook.trampoline() as usize, Ordering::Release);
    if let Err(status) = unsafe { hook.queue_enable() } {
        possess_log(format_args!("hud: queue_enable failed: {status:?}"));
        ORIG.store(0, Ordering::Release);
        return record(Install::Refused);
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {}
        status => {
            possess_log(format_args!("hud: MH_ApplyQueued failed: {status:?}"));
            ORIG.store(0, Ordering::Release);
            return record(Install::Refused);
        }
    }
    if let Ok(mut guard) = HOOK.lock() {
        *guard = Some(InstalledHook(hook));
    }
    possess_log(format_args!(
        "hud: CSFeManImp::UpdatePlayerComponents detour INSTALLED at 0x{target:x} -- while \
         something is possessed the HP, FP and stamina bars read that creature; runes, equipment, \
         the great rune and the spell slots are deliberately left reading your own character, \
         because a creature has no PlayerGameData to supply them"
    ));
    record(Install::Installed)
}

/// Point the bars at `chr_ins`. Idempotent, one relaxed store, safe to call every frame.
pub(crate) fn follow(chr_ins: usize) {
    TARGET.store(chr_ins, Ordering::Release);
}

/// Give the bars back to the real player.
///
/// Infallible by construction, which is why the teardown step that calls it always reports
/// success. It must run before the possession is otherwise unwound: everything after it reads
/// through a creature that is about to stop being one.
pub(crate) fn stop() {
    TARGET.store(0, Ordering::Release);
}

/// Read the possessed creature's vitals right now, for the derived report.
///
/// `None` when nothing is possessed or the module chain does not read back.
pub(crate) fn read_source(chr_ins: usize) -> Option<Source> {
    let layout = LAYOUT.get()?;
    read_vitals(chr_ins, layout)
}

/// Disarm, and remove the detour if this DLL is genuinely being unloaded.
///
/// `process_exiting` is `DllMain`'s `lpReserved != NULL`. The two cases want opposite things and
/// conflating them is how a shutdown path deadlocks:
///
/// * **Process exiting.** Other threads are already gone and the whole image is about to be torn
///   down. `MH_QueueDisableHook` suspends threads to rewrite the prologue, and doing that from
///   `DllMain` while the loader lock is held, against threads that no longer exist, buys nothing
///   and can hang the exit. Disarm and leave.
/// * **A real `FreeLibrary`.** The game keeps running with our code unmapped, so a detour still
///   pointing into it is a jump into unmapped memory on the next frame. The five bytes must come
///   back out, and the deadlock risk is the lesser one.
///
/// The disarm happens first in both cases, so no frame between here and the disable can run the
/// post-pass.
pub(crate) fn shutdown(process_exiting: bool) {
    stop();
    ORIG.store(0, Ordering::Release);
    if process_exiting {
        return;
    }
    let Ok(mut guard) = HOOK.lock() else {
        return;
    };
    let Some(hook) = guard.take() else {
        return;
    };
    if unsafe { hook.0.queue_disable() }.is_ok() {
        let _ = unsafe { MH_ApplyQueued() };
    }
}

/// `void UpdatePlayerComponents(CSFeManImp*, float)` -- see the module docs for where the float
/// comes from and why it is in the signature rather than dropped.
type UpdatePlayerComponentsFn = unsafe extern "system" fn(usize, f32);

/// The detour. Runs on the game's render path, once a frame, forever.
unsafe extern "system" fn update_player_components_hook(fe_man: usize, delta_time: f32) {
    let orig = ORIG.load(Ordering::Acquire);
    if orig == 0 {
        // Nothing to call. Returning without running the original leaves the HUD stale for this
        // frame, which is the least bad option available: the alternative is inventing a call
        // target. Reachable only between `MH_ApplyQueued` failing and the store being rolled
        // back, i.e. effectively never.
        return;
    }
    // The original, unchanged, first. Everything the creature cannot supply is filled in here.
    unsafe {
        core::mem::transmute::<usize, UpdatePlayerComponentsFn>(orig)(fe_man, delta_time);
    }

    // The inert path ends here on every frame nobody is possessing anything.
    let creature = TARGET.load(Ordering::Acquire);
    if creature == 0 || fe_man == 0 {
        return;
    }
    let Some(layout) = LAYOUT.get() else {
        return;
    };
    let Some(source) = read_vitals(creature, layout) else {
        // A despawning creature stops reading back mid-frame. Leaving the player's own values in
        // place for that frame is correct: the possession is about to end anyway, and a partial
        // write would be worse than none.
        return;
    };
    write_view(fe_man, layout, &source);
}

/// Walk `ChrIns+0x190` -> `+0x00` and read the eight fields.
///
/// Every read goes through `safe_read_*`, which is `ReadProcessMemory` against the current-process
/// pseudo-handle: a despawned character or a half-constructed chain answers `None` rather than
/// raising an access violation on the render thread.
fn read_vitals(chr_ins: usize, layout: &Layout) -> Option<Source> {
    let container = unsafe { safe_read_usize(chr_ins + chr_ins::MODULES) }?;
    let data = unsafe { safe_read_usize(container + modules::DATA) }?;
    let source = &layout.source;
    Some(Source {
        hp: unsafe { safe_read_i32(data + source.hp) }?,
        hp_max: unsafe { safe_read_i32(data + source.hp_max) }?,
        hp_max_uncapped: unsafe { safe_read_i32(data + source.hp_max_uncapped) }?,
        recoverable_hp: unsafe { safe_read_f32(data + source.recoverable_hp) }?,
        fp: unsafe { safe_read_i32(data + source.fp) }?,
        fp_max: unsafe { safe_read_i32(data + source.fp_max) }?,
        stamina: unsafe { safe_read_i32(data + source.stamina) }?,
        stamina_max: unsafe { safe_read_i32(data + source.stamina_max) }?,
    })
}

/// Overwrite the eight ints in `FrontEndViewValues`.
///
/// A write cannot be made fault-tolerant the way a read can, so this follows the same convention
/// as `possess::game`: read the address first and skip the whole pass when the read fails, which
/// turns a stale `CSFeManImp` into a missed frame rather than an access violation. One probe
/// covers all eight because they are 0x34 bytes apart inside one sub-object of one allocation.
fn write_view(fe_man: usize, layout: &Layout, source: &Source) {
    let view = layout.view;
    if unsafe { safe_read_i32(fe_man + view.player_hp) }.is_none() {
        return;
    }
    let values = source.view();
    for (offset, value) in [
        (view.player_hp, values.player_hp),
        (view.max_recoverable_hp, values.max_recoverable_hp),
        (view.hp_max, values.hp_max),
        (view.hp_max_uncapped, values.hp_max_uncapped),
        (view.fp, values.fp),
        (view.fp_max, values.fp_max),
        (view.stamina, values.stamina),
        (view.stamina_max, values.stamina_max),
    ] {
        // SAFETY: `fe_man` is the `CSFeManImp` the game just finished writing these same eight
        // fields through, and the probe above proved the sub-object is mapped. Each offset is a
        // 4-byte `int` inside it, taken from the curated `FrontEndViewValues` struct.
        unsafe { ((fe_man + offset) as *mut i32).write(value) };
    }
}
