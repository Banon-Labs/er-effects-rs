//! Shared MinHook FFI wrapper + cross-DLL hook union.
//!
//! Extracted verbatim from `er-quickload/src/mh.rs` (consolidation only, behavior-preserving):
//! the MinHook-generic FFI (`MH_*` externs, `MH_STATUS`), the `MhHook` wrapper, and the hook union
//! (`register_union_hook` + the cross-DLL chaining) now live here so the three game cdylibs share one
//! copy and MinHook's C source is compiled once (build.rs) instead of in each crate.
//!
//! The product-specific `#[no_mangle] er_effects_union_register` C export is deliberately not here --
//! it stays defined in `er-quickload` so only `er_quickload.dll` exports that cross-DLL symbol.
// PARITY: this crate transcribes MinHook's C ABI, so its names, casing and the items it
// declares-but-does-not-call are the upstream header's shape rather than this repo's.
// A per-item allow would mean annotating essentially every line of a binding file.
#![allow(dead_code, non_snake_case, non_camel_case_types, missing_docs)]

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Whether an absolute address is a legitimate place to write into the running image, asked of
/// that image's own function table. It is what a runtime-derived address gets instead of a
/// version translation -- see the module docs for why an AOB hit needs the second question and
/// cannot answer the first.
mod detour_site;

// ============================================================================
// logging seam. `mh.rs` logged union-chain and registry-collision events through the product DLL's
// `telemetry::append_autoload_debug`. That sink is product-specific, so this shared crate calls
// through a function pointer the product installs at startup via `set_hook_logger`. Default is a
// no-op (no logger installed). `er-quickload` installs its telemetry sink in DllMain before any hook
// is registered, so every line the old in-product union code emitted is still emitted, to the same
// log. Crates that only use the raw `MH_*` externs (er-reload-trace, er-input-harness) never
// touch the union and never install a logger; the seam stays inert for them.
// ============================================================================
/// Signature of a logging sink: the union/registry code hands it `format_args!` output.
pub type HookLogFn = fn(std::fmt::Arguments<'_>);
static HOOK_LOGGER: AtomicUsize = AtomicUsize::new(0);

/// Install the sink for union/registry log lines. Call once, early (before any hook registration) to
/// preserve the exact logging the in-product `mh.rs` union produced.
///
/// It also installs the same sink for `er-game-base`'s address-resolution lines, rather than
/// leaving that a second call every caller has to remember. Every cdylib statically links its own
/// copy of both crates, so an uninstalled sink is silent per DLL -- and on 2026-08-28 that cost a
/// diagnosis: `er-armament-icons` logged `MH_ERROR_UNSUPPORTED_FUNCTION`, which is both MinHook's
/// genuine "cannot hook this" and the code `MhHook::new` returns when the build gate refuses an
/// address. With no sink installed there was no line saying which, for an address that is in the
/// verified translation table and so should not have been refused at all. One sink, one call.
pub fn set_hook_logger(logger: HookLogFn) {
    HOOK_LOGGER.store(logger as usize, Ordering::Release);
    er_game_base::game_build::set_address_logger(logger);
}

pub(crate) fn hook_log(args: std::fmt::Arguments<'_>) {
    let raw = HOOK_LOGGER.load(Ordering::Acquire);
    if raw != 0 {
        // SAFETY: `raw` is only ever a `HookLogFn` stored by `set_hook_logger`.
        let logger: HookLogFn = unsafe { std::mem::transmute::<usize, HookLogFn>(raw) };
        logger(args);
    }
}

// ============================================================================
// hook union (2026-07-16, user-directed). MinHook binds one detour per address,
// so two features hooking the same game function silently drop one -- the native-
// Windows menu race. This unions them: the first feature to hook an address installs
// a single dispatcher detour (from a fixed pool, so no runtime codegen) that owns the
// real trampoline; every feature's handler is chained by pointing its existing `orig`
// slot at the next handler, with the last handler's `orig` = the real game trampoline.
// A handler that calls its orig now calls the next handler in the chain (or the game),
// so existing handlers work unchanged and no handler is ever silently dropped.
//
// Constraint: the shared signature is `extern "system" fn(usize,usize,usize,usize)->usize`
// -- correct for the integer/pointer <=4-arg game functions we contend on (menu/dialog
// Run/activate/build). Not for float-arg targets at any arity: an integer dispatcher
// receives and forwards no `xmm` register, so a target taking a float is handed whatever the
// caller happened to leave in `xmm1`. That exclusion is unchanged by the five-argument path
// below, and `dlstring_lookat_math.rs`, `er-npc-possess/src/hud/detour.rs`,
// `er-invasion-warp/src/announce.rs` and `install_title_update_hook` record the hooks that stay
// on a bare `MhHook` because of it.
//
// # A handler is the dispatcher's arity exactly, not at most it
//
// This paragraph used to read "a handler using fewer args just ignores the extras; unused
// register args are harmless", and that is true only of a handler that is alone on its address
// -- which is the one case the union does not exist for. Chaining is what breaks it:
// `register_union_hook_resolved` stores the new handler's address into the previous handler's
// `orig` slot, so a narrow handler calling its orig through the game's own narrower signature
// leaves `r8`/`r9` unset for the next handler and returns nothing for one whose return the game
// uses. Twenty-seven handlers were written to the old sentence and twenty-three addresses ended
// up carrying handlers that disagreed about arity, four of them inside `er_quickload.dll` alone
// (`0x746e80`, `0x67b200`, `0x67b290`, `0xb0d960`) where no second module is needed to chain.
// The safety contract on [`register_shared_hook`] already stated the rule; this is the same rule
// stated where a handler author reads it first, and `scripts/check-union-hook-abi.py` enforces
// it. A game function that genuinely takes fewer arguments is unharmed: the extra registers are
// the caller's own, forwarded verbatim instead of left as the handler's scratch.
//
// # Five arguments, added 2026-09-10, as a parallel path rather than a widening
//
// [`UnionFn`] is four arguments, and where a target really takes five that alias is a silent
// lie. In the Microsoft x64 convention the fifth argument is a stack slot the caller writes
// at `[rsp+0x20]`; a four-argument dispatcher never allocates or writes it, so a
// five-argument callee reached through one reads whatever the caller happened to leave above
// its 32-byte home area. Worse than a wrong integer: for `AddCancelButton` the fifth argument
// is a function pointer the game calls. This is the same failure class as the world block
// ctor at `0x62ec00`, which `menu_trace_hooks.rs` records as runtime-proven on 2026-07-17 --
// stack args lost by a four-register forwarding hook, access violation.
//
// The `AddCancelButton` row cloner (`system_quit_duplicate_add_cancel_button_hook`, five
// arguments) is the concrete case, and it is why that hook could not use the union at all and
// installed a bare [`MhHook`] instead -- which is the trampoline-corruption hazard the union
// exists to remove.
//
// Widening [`UnionFn`] in place was rejected: 270 references across 39 files would have to be
// re-typed for one caller, putting every shipped cdylib in the blast radius. So [`UnionFn5`]
// is a second signature with its own dispatcher pool, and the four-argument handlers compile
// untouched.
//
// One address is one arity, enforced rather than raced. Both pools index the same slot table,
// so a target already union-owned at one arity is refused at the other by [`union_admission`]
// before MinHook is touched. Two dispatchers on one prologue is not a contest worth having:
// MinHook binds one detour per address, so the second `MH_CreateHook` would come back
// `MH_ERROR_ALREADY_CREATED` and one arity's handlers would silently never run -- the exact
// failure mode this whole module was built to delete.
// ============================================================================
pub type UnionFn = unsafe extern "system" fn(usize, usize, usize, usize) -> usize;
/// The five-argument shape, for a target whose fifth integer/pointer argument arrives at
/// `[rsp+0x20]`. Same chaining contract as [`UnionFn`]: what a handler finds in its `orig` slot
/// may be the next handler rather than the game trampoline, so it must call through this
/// signature and not through the game's own narrower one.
pub type UnionFn5 = unsafe extern "system" fn(usize, usize, usize, usize, usize) -> usize;

/// How many arguments a union slot's dispatcher forwards.
///
/// Recorded per entry because it is the one property two registrations on one address may not
/// disagree about. It also picks the dispatcher pool, so the tag and the installed detour cannot
/// drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnionArity {
    Four,
    Five,
}

impl UnionArity {
    /// The pooled dispatcher for `slot` at this arity. Both pools are `MAX_UNION_SLOTS` long and
    /// share the slot index space, so one slot is one address at one arity.
    fn dispatcher(self, slot: usize) -> *mut c_void {
        match self {
            UnionArity::Four => DISPATCHERS[slot] as *mut c_void,
            UnionArity::Five => DISPATCHERS5[slot] as *mut c_void,
        }
    }

    /// How the arity reads in a log line.
    fn label(self) -> &'static str {
        match self {
            UnionArity::Four => "4-argument",
            UnionArity::Five => "5-argument",
        }
    }
}
// 96 slots: this DLL's own union targets plus a companion DLL's (the log-only
// er-reload-trace routes its ~40 native load/menu hooks through this DLL's union via
// the `er_effects_union_register` export, so a single MinHook instance owns every shared
// address instead of two instances corrupting each other's trampolines). One slot per
// unique game address; chained handlers on the same address share a slot.
const MAX_UNION_SLOTS: usize = 96;

struct UnionEntry {
    target: usize,
    trampoline: usize,
    /// Which dispatcher pool holds this slot, and therefore how many arguments every handler on
    /// this address is called with. A registration at the other arity is refused.
    arity: UnionArity,
    /// handler fn ptr + its caller-owned `orig` slot, in chain order.
    handlers: Vec<(usize, &'static AtomicUsize)>,
}
static UNIONS: Mutex<Vec<UnionEntry>> = Mutex::new(Vec::new());
/// Lock-free head-handler per slot, read on every dispatch (no mutex in the hot path).
#[allow(clippy::declare_interior_mutable_const)]
static UNION_HEADS: [AtomicUsize; MAX_UNION_SLOTS] =
    [const { AtomicUsize::new(0) }; MAX_UNION_SLOTS];

unsafe extern "system" fn union_dispatch<const N: usize>(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let head = UNION_HEADS[N].load(Ordering::Acquire);
    if head == 0 {
        return 0;
    }
    let f: UnionFn = unsafe { std::mem::transmute::<usize, UnionFn>(head) };
    unsafe { f(a, b, c, d) }
}

/// The five-argument dispatcher. Same slot table and same head as [`union_dispatch`]; only the
/// signature differs, so the fifth argument the caller wrote at `[rsp+0x20]` is forwarded to the
/// head handler instead of being dropped on the floor.
unsafe extern "system" fn union_dispatch5<const N: usize>(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
    e: usize,
) -> usize {
    let head = UNION_HEADS[N].load(Ordering::Acquire);
    if head == 0 {
        return 0;
    }
    let f: UnionFn5 = unsafe { std::mem::transmute::<usize, UnionFn5>(head) };
    unsafe { f(a, b, c, d, e) }
}

/// Both dispatcher pools from one slot list, so they cannot come out different lengths and a slot
/// index cannot mean one thing in one pool and another in the other.
macro_rules! union_dispatcher_pools {
    ($($n:literal)*) => {
        static DISPATCHERS: [UnionFn; MAX_UNION_SLOTS] =
            [ $( union_dispatch::<$n> as UnionFn ),* ];
        static DISPATCHERS5: [UnionFn5; MAX_UNION_SLOTS] =
            [ $( union_dispatch5::<$n> as UnionFn5 ),* ];
    };
}
union_dispatcher_pools!(
    0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23
    24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47
    48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63 64 65 66 67 68 69 70 71
    72 73 74 75 76 77 78 79 80 81 82 83 84 85 86 87 88 89 90 91 92 93 94 95
);

/// What a registration means for the union table, decided before MinHook is touched.
///
/// Split out of [`register_union_hook_resolved_with`] for the reason [`registry_verdict`] is split
/// out of [`registry_record`]: the effects need `MH_CreateHook`, which does not exist on the host,
/// while the rule -- and in particular the arity refusal -- is pure and can be pinned by a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnionAdmission {
    /// Nothing owns this address yet. This registrant installs the dispatcher and owns the
    /// trampoline; the `usize` is the slot it takes.
    Create(usize),
    /// This address is already union-owned at the same arity; chain onto the entry at this index.
    Chain(usize),
    /// This exact handler is already on this address -- an install that ran twice. Nothing to do.
    AlreadyPresent,
    /// This address is union-owned at the other arity. Two dispatchers on one prologue means one
    /// arity's handlers never run, so the newcomer is refused instead.
    ArityConflict(UnionArity),
    /// Every slot in the pool is taken.
    Exhausted,
}

/// Classify a registration against the union table.
fn union_admission(
    unions: &[UnionEntry],
    target: usize,
    handler_addr: usize,
    arity: UnionArity,
) -> UnionAdmission {
    if let Some((index, entry)) = unions
        .iter()
        .enumerate()
        .find(|(_, entry)| entry.target == target)
    {
        if entry.arity != arity {
            return UnionAdmission::ArityConflict(entry.arity);
        }
        // A duplicate registration of the same handler is an idempotent retry, not a second
        // handler: appending it would chain the handler to itself and spin on the first dispatch.
        if entry.handlers.iter().any(|(h, _)| *h == handler_addr) {
            return UnionAdmission::AlreadyPresent;
        }
        return UnionAdmission::Chain(index);
    }
    let slot = unions.len();
    if slot >= MAX_UNION_SLOTS {
        return UnionAdmission::Exhausted;
    }
    UnionAdmission::Create(slot)
}

/// Append `handler_addr` to `entry`'s chain and wire the two `orig` slots it changes: the previous
/// last handler now calls this one, and this one calls the game trampoline. Returns the new chain
/// length.
///
/// The first registrant and the fifth take the same two stores, which is what makes the chain
/// strictly nested with the first registrant outermost. Writing it once is also what lets a host
/// test drive the real wiring: the effects around it need `MH_CreateHook`, this does not.
fn chain_append(
    entry: &mut UnionEntry,
    handler_addr: usize,
    orig_slot: &'static AtomicUsize,
) -> usize {
    if let Some((_, prev_orig)) = entry.handlers.last() {
        prev_orig.store(handler_addr, Ordering::Release); // prev -> new
    }
    orig_slot.store(entry.trampoline, Ordering::Release); // new -> game orig
    entry.handlers.push((handler_addr, orig_slot));
    entry.handlers.len()
}

/// Register `handler` on `target`, chaining through `orig_slot`. First registrant installs
/// the dispatcher + owns the trampoline; later ones append and no handler is ever dropped.
///
/// # Safety
/// `handler` must be a valid `UnionFn` matching the target's ABI; `orig_slot` must be the
/// static the handler reads to call its original.
pub unsafe fn register_union_hook(
    target: usize,
    handler: UnionFn,
    orig_slot: &'static AtomicUsize,
) -> Result<(), MH_STATUS> {
    // Resolved before anything else: `target` is the union's identity key, so a translated
    // address must be the key too -- otherwise one feature unions on the 1.16.2 address and
    // another on the 1.17 one, and MinHook ends up with two instances on the same function.
    let target = match resolve_target(target, &format!("register_union_hook 0x{target:x}")) {
        Some(resolved) => resolved,
        None => return Err(MH_STATUS::MH_ERROR_UNSUPPORTED_FUNCTION),
    };
    unsafe { register_union_hook_resolved(target, handler, orig_slot) }
}

/// [`register_union_hook`] for an address the caller derived at runtime on the running build.
///
/// The precondition, in one line: the caller found this address by scanning or reading the image
/// that is actually loaded -- an AOB hit in `.text`, a function pointer read out of a live vtable
/// -- so it is already correct for this build and there is nothing to translate.
///
/// # Why this is not [`register_union_hook`] with the gate turned off
///
/// It is a different gate, not a missing one. The translating entry point asks a table keyed by
/// 1.16.2 RVAs where an address moved to; a 1.17 address is not one of that table's keys, so the
/// honest answer for a scanned address is refused -- and on 2026-08-30 that refusal was turning
/// off `er-armament-icons`' and `er-invasion-warp`' GFx tag-parse hooks for an address the scan
/// had got right. Adding a ledger row would have been worse: the scan already returns the 1.17
/// address, so a row would translate it a second time, `+0x1e00` into the middle of a live body.
///
/// What replaces the translation is `detour_site::write_site_is_sound`, which asks the running
/// image's own `.pdata` whether this is a function entry (or an unwind-less leaf) with room for
/// MinHook's five bytes, and refuses an address inside another function's body. A wrong absolute
/// address is exactly as fatal as a stale one, so something has to ask.
///
/// # Safety
/// Same contract as [`register_union_hook`], plus: `target` must have been derived from the
/// running image. Passing a constant here is a bug this cannot detect -- it would be a 1.16.2
/// address asserted to be a 1.17 one.
pub unsafe fn register_union_hook_runtime_derived(
    target: usize,
    handler: UnionFn,
    orig_slot: &'static AtomicUsize,
) -> Result<(), MH_STATUS> {
    #[cfg(windows)]
    {
        let what = format!("register_union_hook_runtime_derived 0x{target:x}");
        if !detour_site::write_site_is_sound(target, detour_site::DETOUR_PATCH_BYTES, &what) {
            return Err(MH_STATUS::MH_ERROR_UNSUPPORTED_FUNCTION);
        }
    }
    unsafe { register_union_hook_resolved(target, handler, orig_slot) }
}

/// [`register_union_hook`] for a target whose fifth integer/pointer argument arrives at
/// `[rsp+0x20]`.
///
/// Everything about the chain is the same -- first registrant outermost, each handler's `orig`
/// slot pointing at the next, the last one at the game trampoline -- and it draws its slot from
/// the same table, so an address is one arity or the other and never both. The difference is the
/// dispatcher, which forwards five arguments instead of four.
///
/// # Safety
/// `handler` must be a valid [`UnionFn5`] matching the target's ABI (exactly five
/// integer/pointer arguments, no floats); `orig_slot` must be the static the handler reads to
/// call its original, and the handler must call that value through [`UnionFn5`] rather than
/// through the game's own signature, because it may be the next handler in the chain.
pub unsafe fn register_union_hook5(
    target: usize,
    handler: UnionFn5,
    orig_slot: &'static AtomicUsize,
) -> Result<(), MH_STATUS> {
    let target = match resolve_target(target, &format!("register_union_hook5 0x{target:x}")) {
        Some(resolved) => resolved,
        None => return Err(MH_STATUS::MH_ERROR_UNSUPPORTED_FUNCTION),
    };
    unsafe { register_union_hook5_resolved(target, handler, orig_slot) }
}

/// [`register_union_hook5`] for an address the caller derived at runtime on the running build.
///
/// The precondition and the audit that replaces the version gate are
/// [`register_union_hook_runtime_derived`]'s, unchanged -- read that one for why a scanned address
/// is refused by the translating entry point and what `.pdata` is asked instead.
///
/// # Safety
/// Same contract as [`register_union_hook5`], plus: `target` must have been derived from the
/// running image. Passing a constant here is a bug this cannot detect.
pub unsafe fn register_union_hook5_runtime_derived(
    target: usize,
    handler: UnionFn5,
    orig_slot: &'static AtomicUsize,
) -> Result<(), MH_STATUS> {
    #[cfg(windows)]
    {
        let what = format!("register_union_hook5_runtime_derived 0x{target:x}");
        if !detour_site::write_site_is_sound(target, detour_site::DETOUR_PATCH_BYTES, &what) {
            return Err(MH_STATUS::MH_ERROR_UNSUPPORTED_FUNCTION);
        }
    }
    unsafe { register_union_hook5_resolved(target, handler, orig_slot) }
}

/// [`register_union_hook`] on an address that has already been resolved for the running build.
///
/// Resolution is not IDEMPOTENT, and assuming it was is what made this split necessary. The
/// translation table is keyed by 1.16.2 RVA and its values are 1.17 RVAs, so feeding a translated
/// address back in asks "where did 0x11d0b80 move to" -- a question with no entry, whose honest
/// answer is refused. Measured 2026-08-28: `register_shared_hook` resolved, then handed the result
/// to `register_shared_hook_with_budget`, which resolved again; `er-armament-icons` lost its
/// file-open observer at 0x1411ced80 to `MH_ERROR_UNSUPPORTED_FUNCTION` even though that address is
/// in the verified table and its 1.17 prologue is byte-identical and perfectly hookable.
///
/// # It stays private, and the two ways in are the point
///
/// "Already correct for the running build" is true for two different reasons, and a caller has to
/// say which, because the checks they owe are different:
///
/// * [`register_union_hook`] resolved a 1.16.2 constant through the translation table, which is
///   also what audits the destination as a detour target;
/// * [`register_union_hook_runtime_derived`] took an address out of the running image, where there
///   is nothing to translate, and audits it against that image's own function table instead.
///
/// A `pub` un-audited entry point here would be a third way -- one that skips both -- and it would
/// look exactly like the two legitimate ones at a call site. The shared path no longer resolves
/// twice either: `register_shared_hook_with_budget` resolves once per branch, after the branch.
///
/// # Safety
/// Same contract as [`register_union_hook`], plus: `target` must already be correct for the
/// running build.
///
/// Not `#[cfg(windows)]`, because its caller `register_union_hook` is not either -- gating only the
/// callee is a host build error, not a smaller binary.
unsafe fn register_union_hook_resolved(
    target: usize,
    handler: UnionFn,
    orig_slot: &'static AtomicUsize,
) -> Result<(), MH_STATUS> {
    unsafe {
        register_union_hook_resolved_with(target, handler as usize, orig_slot, UnionArity::Four)
    }
}

/// [`register_union_hook_resolved`] for a [`UnionFn5`] handler.
///
/// # Safety
/// Same contract as [`register_union_hook_resolved`], with `handler` a [`UnionFn5`] and `target` a
/// five-argument function.
unsafe fn register_union_hook5_resolved(
    target: usize,
    handler: UnionFn5,
    orig_slot: &'static AtomicUsize,
) -> Result<(), MH_STATUS> {
    unsafe {
        register_union_hook_resolved_with(target, handler as usize, orig_slot, UnionArity::Five)
    }
}

/// The registration body both arities share, with `handler` already erased to its address.
///
/// Erased rather than generic on purpose: the table stores handlers as `usize` already, and the
/// only thing the arity decides here is which dispatcher pool `slot` is drawn from. One body means
/// the store orderings, the failure rollback and the log lines cannot come out different for the
/// two paths.
///
/// # Safety
/// `handler_addr` must be a live function of exactly `arity` arguments matching the target's ABI,
/// `orig_slot` must be the static that handler reads to call its original, and `target` must
/// already be correct for the running build.
unsafe fn register_union_hook_resolved_with(
    target: usize,
    handler_addr: usize,
    orig_slot: &'static AtomicUsize,
    arity: UnionArity,
) -> Result<(), MH_STATUS> {
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        s => return Err(s),
    }
    let mut unions = UNIONS.lock().unwrap_or_else(|e| e.into_inner());
    let slot = match union_admission(&unions, target, handler_addr, arity) {
        UnionAdmission::AlreadyPresent => return Ok(()),
        UnionAdmission::Exhausted => return Err(MH_STATUS::MH_ERROR_MEMORY_ALLOC),
        UnionAdmission::ArityConflict(existing) => {
            // Refused before any MinHook call, so nothing is patched and the incumbent keeps
            // working. Loud, because the two candidate causes -- a handler declared at the wrong
            // arity, and two features that genuinely disagree about a game signature -- are both
            // bugs a reader has to be told about rather than left to infer from a missing feature.
            hook_log(format_args!(
                "HOOK UNION ARITY CONFLICT: game addr 0x{target:x} is union-owned as {}, so the \
                 {} registration by {} is refused -- one prologue holds one dispatcher, and \
                 installing a second would leave one arity's handlers unreachable with nothing \
                 logged. One of the two has the target's signature wrong.",
                existing.label(),
                arity.label(),
                as_dll_off(handler_addr)
            ));
            // Routed through the registry as well, so it lands in the same collision channel a
            // reader already scans for a contested address. Nothing is recorded as an owner: the
            // status is not `MH_OK`, so no row is pushed.
            registry_record(
                target,
                handler_addr,
                MH_STATUS::MH_ERROR_ALREADY_CREATED,
                HookOwner::Union,
            );
            return Err(MH_STATUS::MH_ERROR_ALREADY_CREATED);
        }
        UnionAdmission::Chain(index) => {
            let chained = chain_append(&mut unions[index], handler_addr, orig_slot);
            // The registry has to see chained handlers too, or the union looks like it owns an
            // address through exactly one handler no matter how many are on it -- and a later bare
            // `MhHook` collision would name only the first.
            registry_note_union_chain(target, handler_addr);
            hook_log(format_args!(
                "HOOK UNION: game addr 0x{target:x} now chains {chained} handlers (added {}, {})",
                as_dll_off(handler_addr),
                arity.label()
            ));
            return Ok(());
        }
        UnionAdmission::Create(slot) => slot,
    };
    let mut trampoline = null_mut();
    let create_status = unsafe {
        MH_CreateHook(
            target as *mut c_void,
            arity.dispatcher(slot),
            &mut trampoline,
        )
    };
    // Recorded as the handler, not as the dispatcher. `DISPATCHERS[slot]` is a pool entry whose
    // offset says nothing to a reader; the handler is the feature. This is also the mirror case of
    // the empty-owner-set bug: when a bare detour already holds this prologue, MinHook answers
    // `MH_ERROR_ALREADY_CREATED` here and, before 2026-08-31, the union simply returned the error
    // with no registry line at all -- the union losing to a bare hook was as anonymous as a bare
    // hook losing to the union.
    registry_record(target, handler_addr, create_status, HookOwner::Union);
    create_status.ok()?;
    // Arm the slot before enabling the detour. These two stores used to happen after
    // `MH_EnableHook`, leaving a window in which the dispatcher was live but its head was still 0
    // -- and `union_dispatch` returns 0 for a null head without calling the game. On a rarely-hit
    // target that window is invisible; on a hot one like the Scaleform file-open wrapper (called
    // throughout boot) a single unlucky call would hand the engine a NULL File* instead of the
    // asset it asked for. The dispatcher is unreachable until the detour is enabled, so publishing
    // the head first is free.
    UNION_HEADS[slot].store(handler_addr, Ordering::Release);
    let mut entry = UnionEntry {
        target,
        trampoline: trampoline as usize,
        arity,
        handlers: Vec::new(),
    };
    chain_append(&mut entry, handler_addr, orig_slot); // sole handler -> game orig
    match unsafe { MH_EnableHook(target as *mut c_void) } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ENABLED => {}
        s => {
            // Nothing is patched, so leave no armed head behind for a later slot reuse to inherit.
            UNION_HEADS[slot].store(0, Ordering::Release);
            orig_slot.store(0, Ordering::Release);
            return Err(s);
        }
    }
    unions.push(entry);
    Ok(())
}

// ============================================================================
// cross-DLL union -- The companion side (2026-08-23).
//
// `register_union_hook` above unions handlers inside one DLL, and cannot do more than that:
// its registry, its dispatcher pool and its MinHook instance are all statics, and a statically
// linked crate's statics are per DLL. Two cdylibs that both link this crate therefore own two
// independent MinHook instances. If both detour one prologue, the second `MH_CreateHook` gets
// `MH_ERROR_ALREADY_CREATED`: the loser reports installed, never runs, and every feature behind
// it looks unimplemented -- nothing crashes and nothing logs an error.
//
// That is measured, not hypothetical. `er-quickload` and `er-armament-icons` both detour
// `TITLE_SCALEFORM_FILE_OPEN_RVA` (0x11ced80); in an eleven-native profile the product reported
// `file_open_observer_installed = true` with `file_open_hits = 0` for an entire session and every
// GFx swap it owns went silently vanilla, while the same build loaded alone reported 113 hits
// (bd armament-icons-and-product-share-scaleform-fileopen-rva-2026-08-23).
//
// The product DLL publishes its union as the `er_effects_union_register` C export, so the fix is
// for every other DLL to register through that export instead of its own instance -- one MinHook
// instance owns the prologue and both handlers chain. [`register_shared_hook`] is that call: it
// uses the product's union when the product is in the process and this DLL's own union when it is
// not, so a standalone run of the companion behaves exactly as before.
// ============================================================================

/// C-ABI shape of the product DLL's `er_effects_union_register` export
/// (`crates/er-quickload/src/mh.rs`): `(target, handler, *mut orig_slot) -> 0 ok | -1 null slot |
/// positive `MH_STATUS` on MinHook failure`.
pub type UnionRegisterFn = unsafe extern "system" fn(usize, UnionFn, *mut usize) -> i32;

/// C-ABI shape of the product DLL's `er_effects_union_register5` export: the same contract with a
/// [`UnionFn5`] handler.
///
/// # Why the arity is in the export name and not in an argument
///
/// A companion resolves this by string through `GetProcAddress`, and users install these DLLs one
/// at a time from separate releases, so a new companion routinely meets an older product. If arity
/// were an extra parameter on the one export, an older product would decode the call by its own
/// signature: it would read the five-argument handler as a [`UnionFn`], install a four-argument
/// dispatcher, and call a handler that expects a fifth stack argument without ever writing one. No
/// error, no log line, and garbage in the fifth parameter -- which for `AddCancelButton` is a
/// function pointer the game will call.
///
/// A distinct name cannot fail that way. `GetProcAddress` returns null on the older product,
/// [`resolve_product_union_register5`] answers `None`, and the caller takes the documented local
/// fallback with a line saying so.
pub type UnionRegister5Fn = unsafe extern "system" fn(usize, UnionFn5, *mut usize) -> i32;

/// Which MinHook instance a [`register_shared_hook`] call ended up on. Worth logging: it is the
/// difference between "chained onto the product's detour" and "installed a second instance that
/// may be about to lose a trampoline race".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookRoute {
    /// Chained into `er_quickload.dll`'s single union -- the product is co-loaded.
    ProductUnion,
    /// This DLL's own union -- the product is absent, or this is the product.
    LocalUnion,
}

/// The product DLL as me3 loads it, matched by base name rather than by path.
#[cfg(windows)]
const PRODUCT_DLL_NAME: &[u8] = b"er_quickload.dll\0";
// Deliberately still `er_effects_`, after the 2026-08-26 rename of the crate to `er-quickload`
// and the repo to `er-mods-rs`. This name is an ABI, not branding: seven crates resolve it out of
// the product DLL by string through GetProcAddress, and users install these DLLs one at a time
// from separate releases. Renaming it would make an already-downloaded `er_invasion_warp.dll`
// fail to find the union next to a freshly built product, fall back to its own MinHook instance,
// and corrupt the shared trampoline -- with nothing in any gate to say so. The exports that did
// move (`er_quickload_loading_screen_data`) have exactly one consumer, built in the same pass.
#[cfg(windows)]
const UNION_REGISTER_EXPORT: &[u8] = b"er_effects_union_register\0";
/// The five-argument sibling of [`UNION_REGISTER_EXPORT`]. Same `er_effects_` prefix and the same
/// reasoning: it is an ABI other images resolve by string, not branding.
#[cfg(windows)]
const UNION_REGISTER5_EXPORT: &[u8] = b"er_effects_union_register5\0";

/// Default poll budget for [`register_shared_hook`]: ~1s at 25ms.
///
/// A budget is needed rather than a single probe because me3 loads natives in profile order and
/// nothing guarantees the product comes first -- `er-dll-closure.py` emits the product first for
/// exactly this reason, but a hand-written profile need not. A companion whose install thread runs
/// before the product's `LoadLibrary` would see no module at all, take the local union, and
/// recreate the collision this API exists to remove. Both natives are loaded within a few
/// milliseconds of each other, so this budget is orders of magnitude past the real race; the
/// fallback is correct behaviour, not a failure, so overshooting costs nothing but a late arm.
#[cfg(windows)]
const PRODUCT_RESOLVE_TRIES: u32 = 40;
#[cfg(windows)]
const PRODUCT_RESOLVE_SLEEP_MS: u32 = 25;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn Sleep(ms: u32);
}

/// Resolve the product DLL's `er_effects_union_register` export, polling `tries` times at
/// `sleep_ms` intervals. `None` means the product is not in this process (a standalone companion
/// run) or this DLL *is* the product -- in both cases the caller owns the address itself.
///
/// Pass `tries = 1, sleep_ms = 0` for a non-blocking probe.
#[cfg(windows)]
pub fn resolve_product_union_register(tries: u32, sleep_ms: u32) -> Option<UnionRegisterFn> {
    let proc = resolve_product_export(UNION_REGISTER_EXPORT, tries, sleep_ms)?;
    // SAFETY: the export's C-ABI shape is fixed by the product DLL, and both images stay mapped
    // for the process lifetime, so the pointer stays valid.
    Some(unsafe { std::mem::transmute::<*mut c_void, UnionRegisterFn>(proc) })
}

/// [`resolve_product_union_register`] for the five-argument export.
///
/// `None` also covers a product that predates the export, which is the point of giving it its own
/// name -- see [`UnionRegister5Fn`].
#[cfg(windows)]
pub fn resolve_product_union_register5(tries: u32, sleep_ms: u32) -> Option<UnionRegister5Fn> {
    let proc = resolve_product_export(UNION_REGISTER5_EXPORT, tries, sleep_ms)?;
    // SAFETY: as above, for the five-argument shape.
    Some(unsafe { std::mem::transmute::<*mut c_void, UnionRegister5Fn>(proc) })
}

/// The polling `GetProcAddress` both resolvers share: find `er_quickload.dll`, ask it for `name`.
///
/// Factored so the self-resolution guard and the retry budget are written once. Two copies of that
/// guard is one copy too many: dropping it in either would send the product's own registration out
/// through a C-ABI round trip back into the table it was already holding the lock on.
#[cfg(windows)]
fn resolve_product_export(name: &[u8], tries: u32, sleep_ms: u32) -> Option<*mut c_void> {
    for attempt in 0..tries.max(1) {
        let hmod = unsafe { GetModuleHandleA(PRODUCT_DLL_NAME.as_ptr()) };
        // Resolving our own export would route right back into the local union through a C-ABI
        // round trip. Same outcome, so this is a clarity guard rather than a correctness one --
        // but it also means the product can call `register_shared_hook` without special-casing.
        if !hmod.is_null() && hmod as usize != dll_base() {
            let proc = unsafe { GetProcAddress(hmod, name.as_ptr()) };
            if !proc.is_null() {
                return Some(proc);
            }
        }
        if attempt + 1 < tries.max(1) && sleep_ms > 0 {
            unsafe { Sleep(sleep_ms) };
        }
    }
    None
}

/// Register `handler` on `target` through whichever union owns the process's MinHook instance for
/// it: the product DLL's when the product is co-loaded, this DLL's own otherwise.
///
/// Use this -- never a bare [`MhHook`] -- for any prologue a second ME3 DLL might also detour.
/// `scripts/check-shared-hook-rvas.py` is the gate that finds those addresses;
/// `scripts/me3-dll-conflicts.toml` records each one.
///
/// # Safety
/// `handler` must be a valid [`UnionFn`] matching `target`'s ABI (<=4 integer/pointer args), and
/// `orig_slot` must be the `'static` cell that handler reads to call its original. Note that the
/// value stored there may be the next handler in the chain rather than the game trampoline, so the
/// handler must call it through the 4-argument [`UnionFn`] signature, not the game's narrower one.
#[cfg(windows)]
pub unsafe fn register_shared_hook(
    target: usize,
    handler: UnionFn,
    orig_slot: &'static AtomicUsize,
) -> Result<HookRoute, MH_STATUS> {
    // Unresolved, deliberately -- see [`register_shared_hook_with_budget`], which owns the single
    // resolve and must own it after the branch, because the two branches resolve in different
    // images.
    unsafe {
        register_shared_hook_with_budget(
            target,
            handler,
            orig_slot,
            PRODUCT_RESOLVE_TRIES,
            PRODUCT_RESOLVE_SLEEP_MS,
        )
    }
}

/// [`register_shared_hook`] with an explicit resolve budget.
///
/// Pass `tries = 1, sleep_ms = 0` when the caller is driven by a game frame rather than by its own
/// install thread. The default budget exists because a companion's install thread can outrun me3's
/// `LoadLibrary` of the product; a game task tick cannot -- every native in the profile is loaded
/// long before `CSTaskImp` exists -- so one probe is already the right answer there, and the
/// polling budget would only be a stall on the game thread when the product is genuinely absent.
///
/// # the single resolve, and why it happens after the branch (2026-08-30)
///
/// `target` arrives unresolved and each branch resolves it exactly once, in the image that will
/// own the detour. This used to resolve first and hand the resolved address to both branches --
/// and the product branch then resolved it a second time, inside `er_quickload.dll`, because the
/// `er_effects_union_register` export calls [`register_union_hook`] like any other caller.
///
/// A second resolve normally misses and `already_translated_in` hands the address back unchanged,
/// which is why this survived. But a 1.17 destination can also be some other row's 1.16.2 source,
/// and then the second lookup does not miss -- it translates again, to a third, unrelated
/// function. Measured on er-reload-trace's own hook set: `native_submit` `0x7ac890 -> 0x7ad710`,
/// and `0x7ad710` is itself a tracked source, `-> 0x7ae590`. Three detour rows have that collision
/// shape (`0x6156c0`, `0x7ad710`, `0xbbbd90`), and `already_translated_in`'s own doc names two of
/// them, because from a bare address the two cases are INDISTINGUISHABLE: the table cannot tell
/// whether it is being asked about a source or about a destination that happens to look like one.
///
/// That is why the fix is structural rather than a smarter table. Resolve once, at one layer, and
/// leave no path that hands an already-resolved address to something that resolves again.
///
/// # Safety
/// Same contract as [`register_shared_hook`].
#[cfg(windows)]
pub unsafe fn register_shared_hook_with_budget(
    target: usize,
    handler: UnionFn,
    orig_slot: &'static AtomicUsize,
    tries: u32,
    sleep_ms: u32,
) -> Result<HookRoute, MH_STATUS> {
    if let Some(register) = resolve_product_union_register(tries, sleep_ms) {
        hook_log(format_args!(
            "HOOK SHARED (0x{target:x}): handing the UNRESOLVED address to er_quickload.dll's \
             union, which owns the single resolve for this branch"
        ));
        // AtomicUsize is a repr(transparent) usize, so handing the product a `*mut usize` into our
        // own static is sound; our image outlives every dispatch.
        let slot_ptr = orig_slot.as_ptr();
        return match unsafe { register(target, handler, slot_ptr) } {
            0 => Ok(HookRoute::ProductUnion),
            // -1 is the export's null-slot rejection, which cannot happen here (the pointer comes
            // from a live static) -- reported as unknown rather than silently mapped to a status.
            code if code < 0 => Err(MH_STATUS::MH_UNKNOWN),
            code => Err(mh_status_from_i32(code)),
        };
    }
    // The product is absent, so this image owns the one resolve.
    let target = match resolve_target(
        target,
        &format!("register_shared_hook_with_budget 0x{target:x}"),
    ) {
        Some(resolved) => resolved,
        None => return Err(MH_STATUS::MH_ERROR_UNSUPPORTED_FUNCTION),
    };
    unsafe { register_union_hook_resolved(target, handler, orig_slot) }
        .map(|()| HookRoute::LocalUnion)
}

/// [`register_shared_hook`] for a [`UnionFn5`] handler.
///
/// # Safety
/// Same contract as [`register_union_hook5`], and the same note about the `orig` slot: what it
/// holds may be the next handler in the chain, so the handler must call it through [`UnionFn5`].
#[cfg(windows)]
pub unsafe fn register_shared_hook5(
    target: usize,
    handler: UnionFn5,
    orig_slot: &'static AtomicUsize,
) -> Result<HookRoute, MH_STATUS> {
    unsafe {
        register_shared_hook5_with_budget(
            target,
            handler,
            orig_slot,
            PRODUCT_RESOLVE_TRIES,
            PRODUCT_RESOLVE_SLEEP_MS,
        )
    }
}

/// [`register_shared_hook5`] with an explicit resolve budget.
///
/// The single resolve happens after the branch, in the image that will own the detour, for the
/// reason spelled out in full on [`register_shared_hook_with_budget`]: an address can be both a
/// 1.17 destination and some other row's 1.16.2 source, so resolving twice can translate it again
/// into a third, unrelated function.
///
/// A product that does not export `er_effects_union_register5` sends this down the local branch,
/// which is a real downgrade rather than a neutral fallback -- two MinHook instances on one
/// prologue is the trampoline corruption this API exists to avoid -- so it gets its own line. It
/// is still the better failure: the alternative, one export carrying an arity argument, is the
/// wrong dispatcher arity and no line at all.
///
/// # Safety
/// Same contract as [`register_shared_hook5`].
#[cfg(windows)]
pub unsafe fn register_shared_hook5_with_budget(
    target: usize,
    handler: UnionFn5,
    orig_slot: &'static AtomicUsize,
    tries: u32,
    sleep_ms: u32,
) -> Result<HookRoute, MH_STATUS> {
    if let Some(register) = resolve_product_union_register5(tries, sleep_ms) {
        hook_log(format_args!(
            "HOOK SHARED 5-ARG (0x{target:x}): handing the UNRESOLVED address to \
             er_quickload.dll's union, which owns the single resolve for this branch"
        ));
        // AtomicUsize is a repr(transparent) usize, so handing the product a `*mut usize` into our
        // own static is sound; our image outlives every dispatch.
        let slot_ptr = orig_slot.as_ptr();
        return match unsafe { register(target, handler, slot_ptr) } {
            0 => Ok(HookRoute::ProductUnion),
            // -1 is the export's null-slot rejection, which cannot happen here (the pointer comes
            // from a live static) -- reported as unknown rather than silently mapped to a status.
            code if code < 0 => Err(MH_STATUS::MH_UNKNOWN),
            code => Err(mh_status_from_i32(code)),
        };
    }
    if resolve_product_union_register(1, 0).is_some() {
        // The product is here and publishes the four-argument export but not the five-argument
        // one, so it predates this path. Worth its own line: the resulting local install is the
        // two-instance hazard, and the fix is a matching product build rather than anything at
        // this call site.
        hook_log(format_args!(
            "HOOK SHARED 5-ARG (0x{target:x}): er_quickload.dll is loaded but exports no \
             er_effects_union_register5, so this handler takes its own MinHook instance -- if the \
             product also detours this prologue the two instances will corrupt each other's \
             trampolines. Rebuild the product from the same tree as this shell."
        ));
    }
    // The product is absent, so this image owns the one resolve.
    let target = match resolve_target(
        target,
        &format!("register_shared_hook5_with_budget 0x{target:x}"),
    ) {
        Some(resolved) => resolved,
        None => return Err(MH_STATUS::MH_ERROR_UNSUPPORTED_FUNCTION),
    };
    unsafe { register_union_hook5_resolved(target, handler, orig_slot) }
        .map(|()| HookRoute::LocalUnion)
}

/// Reconstruct an [`MH_STATUS`] from the `i32` the cross-DLL export returns.
fn mh_status_from_i32(code: i32) -> MH_STATUS {
    match code {
        0 => MH_STATUS::MH_OK,
        1 => MH_STATUS::MH_ERROR_ALREADY_INITIALIZED,
        2 => MH_STATUS::MH_ERROR_NOT_INITIALIZED,
        3 => MH_STATUS::MH_ERROR_ALREADY_CREATED,
        4 => MH_STATUS::MH_ERROR_NOT_CREATED,
        5 => MH_STATUS::MH_ERROR_ENABLED,
        6 => MH_STATUS::MH_ERROR_DISABLED,
        7 => MH_STATUS::MH_ERROR_NOT_EXECUTABLE,
        8 => MH_STATUS::MH_ERROR_UNSUPPORTED_FUNCTION,
        9 => MH_STATUS::MH_ERROR_MEMORY_ALLOC,
        10 => MH_STATUS::MH_ERROR_MEMORY_PROTECT,
        11 => MH_STATUS::MH_ERROR_MODULE_NOT_FOUND,
        12 => MH_STATUS::MH_ERROR_FUNCTION_NOT_FOUND,
        _ => MH_STATUS::MH_UNKNOWN,
    }
}

/// Central hook registry (2026-07-16). Every MinHook detour creation records its target game address
/// here. MinHook binds only one detour per address: when a second feature hooks an address that is
/// already claimed, MH_CreateHook returns MH_ERROR_ALREADY_CREATED and the loser's handler never runs.
/// Which detour wins depends on thread install order, so on native Windows it is a non-deterministic
/// race (Wine's scheduler happens to be consistent, which is why it looks fine there). This registry
/// turns that invisible race into an explicit logged collision at install time, naming the game offset
/// and both detours -- so a contested address (the root of the menu flakiness) is visible immediately
/// instead of surfacing as a flaky runtime bug. Idea + design credit: user, 2026-07-16.
///
/// # union-installed hooks are recorded here too (2026-08-31)
///
/// Until that date they were not, and the collision line therefore named an empty owner set in
/// exactly the configuration it exists to explain. Measured in run `br-20260831-160354-2513`: the
/// union took `0x14067c050` and `0x14067c0e0` at boot (+1172ms/+1288ms) for the menu trace's
/// `b80_loadsavedata_67b200` / `b80_deserialize_67b290` observers; the system-quit in-world load
/// guard and RequestLoadSlot guard then bare-`MhHook::new`'d those same two addresses, got
/// `MH_ERROR_ALREADY_CREATED`, and the registry reported `already hooked by detour(s) []`. The
/// counterparty was in the same log five thousand lines earlier under a different message, so the
/// one field a reader needs was blank precisely where they needed it -- and two save-safety guards
/// were silently absent from a load path with nothing naming what had taken their address.
///
/// Rows now carry [`HookOwner`], so a collision says whether the incumbent is a bare detour (a
/// genuine contest -- one of the two never runs) or the union (chainable -- the newcomer should be
/// registering through it rather than through MinHook).
static HOOK_REGISTRY: Mutex<Vec<HookRegistration>> = Mutex::new(Vec::new());

/// One recorded registration: which game address, whose detour, and by which installer.
struct HookRegistration {
    target: usize,
    detour: usize,
    owner: HookOwner,
}

/// Which INSTALLER claimed an address -- and therefore what a second claim on it means.
///
/// The distinction is the whole reason the owner is recorded. Two bare detours on one address is a
/// contest MinHook settles by silently dropping one. A bare detour arriving at an address the union
/// already owns is not a contest to be won: it is a call-site bug with a mechanical fix (register
/// through the union and chain), and naming the incumbent is what tells the two cases apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookOwner {
    /// A bare [`MhHook`] detour, holding MinHook's single slot for this address by itself.
    Bare,
    /// A handler registered through the union. The dispatcher holds the MinHook slot and every
    /// union handler on the address chains, so more of them is normal rather than a collision.
    Union,
}

impl HookOwner {
    /// How an owner is named in the collision/duplicate lines. `Bare` renders as the plain offset
    /// the pre-2026-08-31 message used, so the ordinary case reads exactly as it always did.
    fn label(self, detour: usize, off: &dyn Fn(usize) -> String) -> String {
        match self {
            HookOwner::Bare => off(detour),
            HookOwner::Union => format!("union handler {}", off(detour)),
        }
    }
}

/// What a registration means given what is already recorded at the same address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegistryVerdict {
    /// Nothing else holds this address and MinHook did not object: no line to print.
    Fresh,
    /// Every prior claim is the same detour by the same installer -- one owner installed twice.
    Duplicate,
    /// A different detour already holds this address, or MinHook says one does.
    Collision,
}

/// Classify a registration against the address's existing rows.
///
/// Split out from [`registry_record`] so the rule is testable on the host: the recording half
/// needs `dll_base`, which is a Win32 call, while the decision -- the part that was wrong -- is
/// pure. `MH_ERROR_ALREADY_CREATED` forces a collision even with no prior row, because that is
/// MinHook reporting an owner this registry never saw (a hook installed before the logger existed,
/// or by a different MinHook instance in another DLL).
fn registry_verdict(
    prior: &[(usize, HookOwner)],
    detour: usize,
    owner: HookOwner,
    create_status: MH_STATUS,
) -> RegistryVerdict {
    // A duplicate is not a collision, and conflating them costs an investigation. When every
    // prior registration at this address names the same detour from the same installer, one owner
    // registered twice -- its handler is live either way, and the fix is at the caller (an install
    // that races itself, e.g. two `Once` gates calling one install fn). A collision is two
    // different detours contesting one address, where the loser's handler genuinely never fires
    // and the fix is the shared/union registry. Measured 2026-08-30: `title-cover-part-a`'s
    // named-child binder logged the collision wording against itself at 0x14074b140 and read
    // exactly like the real `title-cover-part-b` conflict from the run before it.
    if !prior.is_empty() && prior.iter().all(|(d, o)| *d == detour && *o == owner) {
        return RegistryVerdict::Duplicate;
    }
    if !prior.is_empty() || create_status == MH_STATUS::MH_ERROR_ALREADY_CREATED {
        return RegistryVerdict::Collision;
    }
    RegistryVerdict::Fresh
}

/// Render the incumbent list for a collision line, naming each owner's INSTALLER as well as its
/// offset. An empty list here now means genuinely nothing recorded (MinHook knows an owner this
/// process never registered), rather than "a union hook that was never written down".
fn render_prior_owners(prior: &[(usize, HookOwner)], off: &dyn Fn(usize) -> String) -> String {
    prior
        .iter()
        .map(|(d, o)| o.label(*d, off))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Our DLL's load base, so detours can be reported as `dll+0xNNN` (identifiable against the map/disasm)
/// instead of an absolute pointer that shifts every launch.
fn dll_base() -> usize {
    use std::sync::OnceLock;
    static BASE: OnceLock<usize> = OnceLock::new();
    *BASE.get_or_init(|| {
        unsafe extern "system" {
            fn GetModuleHandleExW(flags: u32, addr: *const c_void, module: *mut *mut c_void)
            -> i32;
        }
        const FROM_ADDRESS: u32 = 0x4;
        const UNCHANGED_REFCOUNT: u32 = 0x2;
        let mut h: *mut c_void = null_mut();
        let anchor = dll_base as *const c_void; // any address inside our DLL
        if unsafe { GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, anchor, &mut h) } != 0 {
            h as usize
        } else {
            0
        }
    })
}

fn as_dll_off(p: usize) -> String {
    let b = dll_base();
    if b != 0 && p >= b {
        format!("dll+0x{:x}", p - b)
    } else {
        format!("0x{p:x}")
    }
}

/// Record one registration and log what it means. `owner` says which installer is claiming the
/// address; see [`HookOwner`] for why a bare-vs-union incumbent is the load-bearing distinction.
fn registry_record(target: usize, detour: usize, create_status: MH_STATUS, owner: HookOwner) {
    if let Ok(mut reg) = HOOK_REGISTRY.lock() {
        let prior: Vec<(usize, HookOwner)> = reg
            .iter()
            .filter(|row| row.target == target)
            .map(|row| (row.detour, row.owner))
            .collect();
        let verdict = registry_verdict(&prior, detour, owner, create_status);
        // A row means MINHOOK accepted a create at this address for this detour -- so a create that
        // failed must not leave one. Before 2026-08-31 every attempt was recorded, so the loser of a
        // collision became a permanent phantom "owner" and a third registrant was told the address
        // belongs to a detour that was never bound. Silence about a real owner and confidence about
        // a fictional one are the same defect from opposite ends.
        if create_status == MH_STATUS::MH_OK {
            reg.push(HookRegistration {
                target,
                detour,
                owner,
            });
        }
        drop(reg);
        let off: &dyn Fn(usize) -> String = &as_dll_off;
        match verdict {
            RegistryVerdict::Fresh => {}
            RegistryVerdict::Duplicate => hook_log(format_args!(
                "HOOK REGISTRY DUPLICATE: game addr 0x{target:x} registered again by the SAME detour {} (MH_CreateHook={create_status:?}) -- one owner installed twice, nothing is lost and the first registration is live; fix the caller, this is NOT a contested address",
                owner.label(detour, off)
            )),
            RegistryVerdict::Collision => hook_log(format_args!(
                "HOOK REGISTRY COLLISION: game addr 0x{target:x} already hooked by detour(s) [{}], NOW ALSO {} (MH_CreateHook={create_status:?}) -- only ONE binds, the loser's handler never fires (silent native-Windows race source); an incumbent named `union handler` is CHAINABLE, so register through the union instead of MinHook",
                render_prior_owners(&prior, off),
                owner.label(detour, off)
            )),
        }
    }
}

/// Record a union handler that chained onto an address the union already owns.
///
/// Deliberately silent: chaining is the union's designed behaviour and
/// [`register_union_hook_resolved`] already logs `HOOK UNION: ... now chains N handlers` for it.
/// What this adds is the row, so that a later bare `MhHook::new` on the same address can be told
/// who it is colliding with instead of reporting an empty owner set.
fn registry_note_union_chain(target: usize, handler: usize) {
    if let Ok(mut reg) = HOOK_REGISTRY.lock() {
        reg.push(HookRegistration {
            target,
            detour: handler,
            owner: HookOwner::Union,
        });
    }
}

#[allow(non_camel_case_types)]
#[must_use]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MH_STATUS {
    MH_UNKNOWN = -1,
    MH_OK = 0,
    MH_ERROR_ALREADY_INITIALIZED,
    MH_ERROR_NOT_INITIALIZED,
    MH_ERROR_ALREADY_CREATED,
    MH_ERROR_NOT_CREATED,
    MH_ERROR_ENABLED,
    MH_ERROR_DISABLED,
    MH_ERROR_NOT_EXECUTABLE,
    MH_ERROR_UNSUPPORTED_FUNCTION,
    MH_ERROR_MEMORY_ALLOC,
    MH_ERROR_MEMORY_PROTECT,
    MH_ERROR_MODULE_NOT_FOUND,
    MH_ERROR_FUNCTION_NOT_FOUND,
}

unsafe extern "system" {
    pub fn MH_Initialize() -> MH_STATUS;
    pub fn MH_CreateHook(
        pTarget: *mut c_void,
        pDetour: *mut c_void,
        ppOriginal: *mut *mut c_void,
    ) -> MH_STATUS;
    pub fn MH_QueueEnableHook(pTarget: *mut c_void) -> MH_STATUS;
    pub fn MH_QueueDisableHook(pTarget: *mut c_void) -> MH_STATUS;

    // The four that FREEZE. Renamed rather than exported, so nothing can reach MinHook's
    // thread-suspending entry points without passing through the guard below. See
    // [`freeze_guard`] for what that guard is for; the wrappers keep the original names and
    // signatures, so every existing call site is unchanged.
    #[link_name = "MH_Uninitialize"]
    fn MH_Uninitialize_unguarded() -> MH_STATUS;
    #[link_name = "MH_EnableHook"]
    fn MH_EnableHook_unguarded(pTarget: *mut c_void) -> MH_STATUS;
    #[link_name = "MH_DisableHook"]
    fn MH_DisableHook_unguarded(pTarget: *mut c_void) -> MH_STATUS;
    #[link_name = "MH_ApplyQueued"]
    fn MH_ApplyQueued_unguarded() -> MH_STATUS;
}

/// # Safety
/// Same contract as MinHook's own `MH_EnableHook`.
pub unsafe fn MH_EnableHook(pTarget: *mut c_void) -> MH_STATUS {
    let _guard = freeze_guard::hold("MH_EnableHook");
    unsafe { MH_EnableHook_unguarded(pTarget) }
}

/// # Safety
/// Same contract as MinHook's own `MH_DisableHook`.
pub unsafe fn MH_DisableHook(pTarget: *mut c_void) -> MH_STATUS {
    let _guard = freeze_guard::hold("MH_DisableHook");
    unsafe { MH_DisableHook_unguarded(pTarget) }
}

/// # Safety
/// Same contract as MinHook's own `MH_ApplyQueued`.
pub unsafe fn MH_ApplyQueued() -> MH_STATUS {
    let _guard = freeze_guard::hold("MH_ApplyQueued");
    unsafe { MH_ApplyQueued_unguarded() }
}

/// # Safety
/// Same contract as MinHook's own `MH_Uninitialize`.
pub unsafe fn MH_Uninitialize() -> MH_STATUS {
    let _guard = freeze_guard::hold("MH_Uninitialize");
    unsafe { MH_Uninitialize_unguarded() }
}

/// One process-wide lock around every MinHook entry point that suspends threads.
///
/// # The deadlock this exists to prevent
///
/// MinHook's `Freeze()` takes a `CreateToolhelp32Snapshot` and calls `SuspendThread` on every
/// other thread in the process before it writes a detour, then resumes them. That is correct for
/// one MinHook. This workspace ships twenty-one cdylibs, each of which statically links its OWN
/// MinHook instance -- deliberately, since the hook union owns exactly one instance per DLL -- so
/// there are twenty-one independent freezers with twenty-one independent locks, and MinHook's own
/// critical section serialises none of them against each other. Each shell then installs its hooks
/// from a thread it spawned out of `DllMain`, so those twenty-one installer threads run at the
/// same time by construction.
///
/// Two of them overlapping is the bug: thread A's `Freeze` suspends thread B while B is itself
/// inside `SuspendThread`, and on Wine every one of those calls is a wineserver round trip, so the
/// suspended requester never returns and its request never completes. Measured on 2026-09-04 and
/// again on 2026-09-08 (run br-20260908-195845-d694, wedged at +576ms during hook installation,
/// every DLL's log frozen at the same instant): 62 threads all in state `S`, wchan
/// `anon_pipe_read` x48 including the leader, and wineserver itself idle in `do_epoll_wait` --
/// nothing pending on its side, because the threads that would have had requests outstanding were
/// suspended before they could make them. Open issue er-effects-rs-1742.
///
/// It is intermittent for the reason the mechanism predicts: it needs two freeze windows to
/// overlap, and each is short. That is also why a lock is the whole fix -- there is nothing wrong
/// with any single freeze.
///
/// # Why a named mutex rather than a `static`
///
/// A Rust `static` is per-DLL here, exactly like MinHook's own lock, so it would serialise each
/// shell against itself and nothing else -- the same non-fix twenty-one times over. A named
/// kernel mutex is one object no matter how many modules open it, which is the property required.
/// It is scoped to this process by name, so two Elden Ring instances do not serialise against each
/// other, and it is reentrant for the owning thread, so a hook installed from inside another
/// hook's callback cannot self-deadlock.
///
/// Every failure path here proceeds unguarded rather than refusing. A hook that is not installed
/// is a feature that is silently missing for the whole run; an unserialised freeze is a boot that
/// usually works. Neither is good and the first one is worse.
#[cfg(windows)]
mod freeze_guard {
    use core::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    unsafe extern "system" {
        fn CreateMutexW(
            attributes: *mut c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn ReleaseMutex(handle: *mut c_void) -> i32;
        fn GetCurrentProcessId() -> u32;
    }

    const WAIT_OBJECT_0: u32 = 0x0000_0000;
    /// The holder died without releasing. The lock is ours and the protected state is MinHook's
    /// own, which a dead thread cannot have left half-written here -- so this is as good as
    /// acquiring.
    const WAIT_ABANDONED: u32 = 0x0000_0080;
    /// Long enough that a real freeze (a snapshot plus a suspend/resume pair per thread, roughly
    /// sixty threads, on Wine) is never cut off, short enough that a wedged holder costs one boot
    /// rather than hanging the process forever.
    const FREEZE_LOCK_TIMEOUT_MS: u32 = 5_000;

    /// The opened mutex, or `usize::MAX` once opening has failed and should not be retried.
    static HANDLE: AtomicUsize = AtomicUsize::new(0);
    const UNAVAILABLE: usize = usize::MAX;

    fn handle() -> Option<*mut c_void> {
        let cached = HANDLE.load(Ordering::Acquire);
        if cached == UNAVAILABLE {
            return None;
        }
        if cached != 0 {
            return Some(cached as *mut c_void);
        }
        // Per process, so a second running game does not serialise against this one.
        let name: Vec<u16> = format!("Local\\er-mods-rs-minhook-freeze-{}", unsafe {
            GetCurrentProcessId()
        })
        .encode_utf16()
        .chain(core::iter::once(0))
        .collect();
        let opened = unsafe { CreateMutexW(core::ptr::null_mut(), 0, name.as_ptr()) };
        if opened.is_null() {
            HANDLE.store(UNAVAILABLE, Ordering::Release);
            return None;
        }
        // A race here opens the same named object twice and leaks one handle for the life of the
        // process. Both handles name one mutex, so the guarantee holds either way; closing the
        // loser would be the bug, since another thread may already be waiting on it.
        match HANDLE.compare_exchange(0, opened as usize, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => Some(opened),
            Err(winner) => Some(winner as *mut c_void),
        }
    }

    /// Held for the duration of one freezing MinHook call.
    pub(super) struct Held(Option<*mut c_void>);

    impl Drop for Held {
        fn drop(&mut self) {
            if let Some(handle) = self.0 {
                unsafe { ReleaseMutex(handle) };
            }
        }
    }

    pub(super) fn hold(what: &str) -> Held {
        let Some(handle) = handle() else {
            return Held(None);
        };
        match unsafe { WaitForSingleObject(handle, FREEZE_LOCK_TIMEOUT_MS) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Held(Some(handle)),
            other => {
                // Proceeding is the lesser risk, but it is the shape of the deadlock this guard
                // exists to prevent, so it must not be silent.
                crate::hook_log(format_args!(
                    "HOOK FREEZE LOCK: {what} waited {FREEZE_LOCK_TIMEOUT_MS}ms for the \
                     process-wide MinHook lock and got {other:#x}; proceeding UNSERIALISED. \
                     Another module has been inside a thread freeze for longer than any real \
                     freeze takes -- see er-effects-rs-1742."
                ));
                Held(None)
            }
        }
    }
}

/// Host half: nothing to serialise, because nothing freezes.
#[cfg(not(windows))]
mod freeze_guard {
    pub(super) struct Held;
    pub(super) fn hold(_what: &str) -> Held {
        Held
    }
}

impl MH_STATUS {
    pub fn ok_context(self, _context: &str) -> Result<(), MH_STATUS> {
        self.ok()
    }

    pub fn ok(self) -> Result<(), MH_STATUS> {
        if self == MH_STATUS::MH_OK {
            Ok(())
        } else {
            Err(self)
        }
    }
}

// ============================================================================
// build gate (2026-08-28). Every game address in this workspace is a 1.16.2 RVA. ELDEN RING 1.17
// moved code, and a detour installed at a stale RVA does not fail -- it lands mid-function and
// corrupts the game: `0x1407ada40` is a real prologue in 1.16.2 and `xor r15d, r15d` in 1.17, and
// hooking it killed a boot with an access violation whose backtrace blames game code.
//
// MinHook cannot catch this. It refuses only what it cannot decode (several hooks did come back
// MH_ERROR_UNSUPPORTED_FUNCTION on 1.17); mid-function bytes that happen to decode are installed
// happily. So the check has to be "is this the build these addresses came from", asked once, here,
// where every detour in every DLL of this workspace passes through.
//
// Scope is the detour installers plus the two RVA-taking byte primitives. `patch_3byte_stub` and
// `apply_xor_ret_stub` were ungated until 2026-08-30 on the theory that validating the overwritten
// byte was gate enough; it is not. They take a 1.16.2 `rva`, and on 1.17 all three call sites hit a
// byte that is simply different, so each aborted reporting a signature mismatch while the map knew
// exactly where the function had gone. They now resolve first and refuse when nothing knows.
//
// `write_code_byte` stays ungated on purpose: it takes an absolute address that its callers
// discover themselves, so there is no RVA to translate and a gate could only refuse work that is
// already version-agnostic. The caller that established this was `er-ersc-sigshim`, retired
// 2026-09-03 with support for old Seamless builds; the property is about the argument, not it.
// ============================================================================

// Verified 1.16.2 -> 1.17 address pairs, generated by `build.rs` from
// `docs/recon/rva-map-1162-to-1170.verified.tsv`.

/// Where a detour should actually go, given the build that is running.
///
/// Three answers, and the middle one is the point of this whole migration:
///
/// * the address as given -- the running build is the one the RVA came from, or the address is
///   outside the game image (a Win32 detour, correct on every build);
/// * a translated address -- the running build moved the function, and this pair was verified as
///   the same function: `scripts/map-rvas-1162-to-1170.py` found it by masked signature and
///   `scripts/verify-rva-map-1170.py` then confirmed the normalised instruction sequences are
///   identical over the body, not just the prologue;
/// * `None` -- the build moved the function and nothing here knows where to. Refusing is the only
///   safe answer: `0x1407ada40` is a real prologue in 1.16.2 and `xor r15d, r15d` in 1.17, and
///   detouring it killed a boot.
///
/// A translation is logged with both addresses, because a hook silently landing somewhere other
/// than where the source says is exactly the kind of thing a reader of a crash log needs told.
fn resolve_target(target: usize, what: &str) -> Option<usize> {
    // The table and the decision both live in `er-game-base`, because a stale address is just as
    // reachable as a direct call as it is as a detour, and one copy of the rule is the only way
    // both paths can agree. The hook log keeps its own line so a reader of the hook log is not
    // sent to a second file to find out that an address was moved.
    // The detour resolver, not the call one. A row good enough to call is not automatically a
    // safe place for MinHook to write five bytes; see `resolve_detour_address`.
    let resolved = er_game_base::game_build::resolve_detour_address(target, what);
    match resolved {
        Some(address) if address != target => {
            hook_log(format_args!(
                "HOOK TRANSLATED ({what}): 0x{target:x} -> 0x{address:x}"
            ));
        }
        None => hook_log(format_args!(
            "HOOK REFUSED ({what}): {} -- this address has no verified mapping for the running \
             build, so installing here would detour whatever code now occupies it",
            er_game_base::game_build::describe_build()
        )),
        Some(_) => {}
    }
    resolved
}

/// Original address, hook function address, and trampoline for a given hook.
pub struct MhHook {
    addr: *mut c_void,
    hook_impl: *mut c_void,
    trampoline: *mut c_void,
}

impl MhHook {
    /// # Safety
    ///
    /// Installs native code detours; caller must ensure ABI and lifetime are valid.
    pub unsafe fn new(addr: *mut c_void, hook_impl: *mut c_void) -> Result<Self, MH_STATUS> {
        let addr =
            match resolve_target(addr as usize, &format!("MhHook::new 0x{:x}", addr as usize)) {
                Some(resolved) => resolved as *mut c_void,
                None => return Err(MH_STATUS::MH_ERROR_UNSUPPORTED_FUNCTION),
            };
        unsafe { Self::create(addr, hook_impl) }
    }

    /// [`MhHook::new`] for an address the caller derived at runtime on the running build.
    ///
    /// The precondition, in one line: the caller found this address by scanning or reading the
    /// image that is actually loaded -- an AOB hit in `.text`, a function pointer read out of a
    /// live vtable -- so it is already correct for this build and there is nothing to translate.
    ///
    /// This is the [`MhHook`] half of [`register_union_hook_runtime_derived`], and the reasoning
    /// is all there: translation is refused for a scanned address rather than skipped, adding a
    /// ledger row for one would translate it a second time, and what stands in for the version
    /// gate is the running image's own `.pdata` -- entry or unwind-less leaf with room for
    /// MinHook's five bytes, never an address inside another function's body.
    ///
    /// # Safety
    ///
    /// Same contract as [`MhHook::new`], plus: `addr` must have been derived from the running
    /// image. Passing a constant here is a bug this cannot detect -- it would be a 1.16.2 address
    /// asserted to be a 1.17 one.
    pub unsafe fn new_runtime_derived(
        addr: *mut c_void,
        hook_impl: *mut c_void,
    ) -> Result<Self, MH_STATUS> {
        #[cfg(windows)]
        {
            let what = format!("MhHook::new_runtime_derived 0x{:x}", addr as usize);
            if !detour_site::write_site_is_sound(
                addr as usize,
                detour_site::DETOUR_PATCH_BYTES,
                &what,
            ) {
                return Err(MH_STATUS::MH_ERROR_UNSUPPORTED_FUNCTION);
            }
        }
        unsafe { Self::create(addr, hook_impl) }
    }

    /// The MinHook call itself, shared by both entry points so they can differ only in how `addr`
    /// was established. Duplicating these four lines is how the two would drift apart.
    ///
    /// # Safety
    ///
    /// `addr` must already be correct for the running build, by whichever of the two routes.
    unsafe fn create(addr: *mut c_void, hook_impl: *mut c_void) -> Result<Self, MH_STATUS> {
        let mut trampoline = null_mut();
        let status = unsafe { MH_CreateHook(addr, hook_impl, &mut trampoline) };
        registry_record(addr as usize, hook_impl as usize, status, HookOwner::Bare);
        status.ok_context("MH_CreateHook")?;

        Ok(Self {
            addr,
            hook_impl,
            trampoline,
        })
    }

    pub fn trampoline(&self) -> *mut c_void {
        self.trampoline
    }

    /// # Safety
    ///
    /// Enables a native detour through MinHook's queued API.
    pub unsafe fn queue_enable(&self) -> Result<(), MH_STATUS> {
        unsafe { MH_QueueEnableHook(self.addr) }.ok_context("MH_QueueEnableHook")
    }

    /// # Safety
    ///
    /// Disables a native detour through MinHook's queued API.
    pub unsafe fn queue_disable(&self) -> Result<(), MH_STATUS> {
        unsafe { MH_QueueDisableHook(self.addr) }.ok_context("MH_QueueDisableHook")
    }
}

// ============================================================================
// raw code-patch PRIMITIVES (moved from `er-quickload/src/experiments/mem.rs`,
// docs/plans/experiments-crate-targets.md S5). Behaviour-preserving move: the bodies are the
// product's, and every log string is unchanged. They belong here because they are the same
// "reach into the game image and rewrite bytes" capability MinHook itself provides, and both
// consumers (the product DLL and er-title-flow) already depend on this crate -- so hosting them
// here deletes the two `TitleFlowHost` fn-pointer seams that existed only to reach back into the
// product for them.
//
// Kept as two functions rather than one because their log text differs and this is a move, not a
// redesign. `apply_xor_ret_stub` is `patch_3byte_stub` plus a success line and an
// "online-disable"-prefixed abort line; deduping them changes what a diagnostic log says and is
// deliberately left for a separate slice.
//
// The `windows` crate is not pulled in for this -- er-hook has zero `[dependencies]` and keeps it
// that way, following the raw-extern pattern already used above for `GetModuleHandleExW` and the
// `MH_*` family.
// ============================================================================

/// Init value for the `VirtualProtect` out-params; overwritten by the call.
const PAGE_PROTECT_UNSET: u32 = 0;
/// `PAGE_EXECUTE_READWRITE` (winnt.h), the protection a code patch needs.
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
/// Win32 `BOOL` false; `VirtualProtect` returns zero on failure.
const WIN32_FALSE: i32 = 0;
/// `-1` cast to a handle: the current-process pseudo-handle `FlushInstructionCache` accepts
/// without an `OpenProcess` round-trip.
const CURRENT_PROCESS_PSEUDO_HANDLE: isize = -1;
/// Both primitives write exactly the 3 bytes of a `[u8; 3]` stub.
const STUB_LEN: usize = 3;
const BYTE_STEP: usize = 1;
const BYTE_START: usize = 0;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualProtect(
        addr: *mut c_void,
        size: usize,
        new_protect: u32,
        old_protect: *mut u32,
    ) -> i32;
    /// Flush the CPU instruction cache after patching executable code so other threads see the
    /// new bytes (current-process pseudo-handle -1).
    fn FlushInstructionCache(process: isize, base: *const c_void, size: usize) -> i32;
}

/// Bytes touched by [`write_code_byte`]. Named so the protection, the store, and the cache flush
/// visibly agree on one length.
const ONE_CODE_BYTE: usize = 1;

/// The page operations a code-byte write performs, behind a seam. [`Win32CodePage`] is the only
/// production implementation; the seam exists because the two ways this primitive can be wrong are
/// both invisible to a compile check -- a page left `PAGE_EXECUTE_READWRITE` after the write, and a
/// refused protection change that stores the byte anyway -- so the sequence is asserted on the host
/// instead of only in a game. `er-scaleform-hooks` keeps its native hook owner testable the same
/// way.
trait CodePageOps {
    /// `VirtualProtect`: returns whether the protection change was allowed, writing the previous
    /// protection into `old_protect`.
    fn protect(&mut self, addr: usize, len: usize, new_protect: u32, old_protect: &mut u32)
    -> bool;

    /// # Safety
    ///
    /// `addr` must be writable for the duration of the call.
    unsafe fn store(&mut self, addr: usize, value: u8);

    /// Flush the instruction cache so threads already inside this code see the new byte.
    fn flush(&mut self, addr: usize, len: usize);
}

/// Shared body of [`write_code_byte`]: unlock, store, relock to the previous protection, flush.
///
/// Returns whether the protection change was allowed. A refused change returns before the store,
/// so nothing is written and no protection is left changed.
///
/// # Safety
///
/// With [`Win32CodePage`], `address` must be a byte of currently-mapped code in this process that
/// is safe to overwrite; the store is an unsynchronised write into executable memory.
unsafe fn write_code_byte_with<O: CodePageOps>(ops: &mut O, address: usize, value: u8) -> bool {
    let mut old_protect = PAGE_PROTECT_UNSET;
    if !ops.protect(
        address,
        ONE_CODE_BYTE,
        PAGE_EXECUTE_READWRITE,
        &mut old_protect,
    ) {
        hook_log(format_args!(
            "write_code_byte: VirtualProtect failed at 0x{address:x}"
        ));
        return false;
    }
    unsafe { ops.store(address, value) };
    let mut restored = PAGE_PROTECT_UNSET;
    ops.protect(address, ONE_CODE_BYTE, old_protect, &mut restored);
    ops.flush(address, ONE_CODE_BYTE);
    true
}

/// The production [`CodePageOps`]: Win32 `VirtualProtect` + `FlushInstructionCache` against the
/// current process.
#[cfg(windows)]
struct Win32CodePage;

#[cfg(windows)]
impl CodePageOps for Win32CodePage {
    fn protect(
        &mut self,
        addr: usize,
        len: usize,
        new_protect: u32,
        old_protect: &mut u32,
    ) -> bool {
        let allowed = unsafe { VirtualProtect(addr as *mut c_void, len, new_protect, old_protect) };
        allowed != WIN32_FALSE
    }

    unsafe fn store(&mut self, addr: usize, value: u8) {
        unsafe { *(addr as *mut u8) = value };
    }

    fn flush(&mut self, addr: usize, len: usize) {
        unsafe { FlushInstructionCache(CURRENT_PROCESS_PSEUDO_HANDLE, addr as *const c_void, len) };
    }
}

/// Write a single byte of executable code at `address`, with the protection dance the write needs:
/// `PAGE_EXECUTE_READWRITE`, the store, the original protection back, then an instruction-cache
/// flush so threads already inside that code see the new byte.
///
/// Returns whether `VirtualProtect` allowed the write. It deliberately does not report whether the
/// byte landed: a caller patching game code should read it back, because another mod can own the
/// same address, and a successful `VirtualProtect` says nothing about that.
///
/// Unlike [`patch_3byte_stub`] and [`apply_xor_ret_stub`], this neither resolves the address for
/// the running build nor validates the byte it overwrites. Those two take a 1.16.2 RVA and so can
/// do both; this one takes an absolute address its caller discovered at runtime -- often in a
/// foreign module -- so there is nothing to translate, and the caller owns the check.
///
/// # Safety
///
/// `address` must be a byte of currently-mapped code in this process that is safe to overwrite.
/// The store is unsynchronised: it is a single byte, so it cannot tear, but a thread may execute
/// the patched instruction at any point during the call.
#[cfg(windows)]
pub unsafe fn write_code_byte(address: usize, value: u8) -> bool {
    unsafe { write_code_byte_with(&mut Win32CodePage, address, value) }
}

/// Write a self-contained 3-byte return stub at `base+rva` after validating the expected first
/// byte. RWX via VirtualProtect, write, restore, icache flush. Returns true on success. Shared by
/// the gate-force patches (foreground / sign-in / user-index).
///
/// # Why the address is resolved first (2026-08-30)
///
/// `rva` is a 1.16.2 RVA like every other address in this workspace, and the expected-first-byte
/// check was doing double duty as a version gate. It is not one. Measured against
/// `eldenring-deobf-1.17.bin`: at the stale 1.16.2 RVAs the three callers use, 1.17 holds `40 53`,
/// `02 00` and `d5 00` where `0x40`, `0x40` and `0x4c` were expected -- so all three patches abort
/// and report `byte ... is 0x02, expected 0x40`, which reads as a stale signature and sends the
/// reader hunting for a changed prologue. The real cause is that the function moved, and the map
/// already knows where: 0xe56310 -> 0xe58110, 0x24129b0 -> 0x24151c0, 0x240f490 -> 0x2411ca0, each
/// `IDENTICAL` over 71-90 instructions, and each destination starts with the byte the caller
/// expects. Resolving first turns three silently dead features back on and makes an unmappable
/// address say refused instead of impersonating a signature change.
///
/// The byte check stays and still earns its place: it is what confirms the resolved destination is
/// the entry the caller means. `resolve_game_address` (not `resolve_detour_address`) is the right
/// question here -- this writes three self-contained bytes and relocates nothing, so it does not
/// need MinHook's five-relocatable-bytes audit.
///
/// It is no longer the only check, though, because on its own it is far too weak to be one: the
/// resolved address is also audited by `detour_site::write_site_is_sound` for three bytes, which
/// refuses an address inside another function's declared body. See the comment at that call for
/// why a single REX prefix passes by coincidence.
#[cfg(windows)]
pub fn patch_3byte_stub(
    base: usize,
    rva: usize,
    expected_first: u8,
    stub: [u8; STUB_LEN],
    label: &str,
) -> bool {
    let Some(address) = er_game_base::game_build::resolve_game_address(base + rva, label) else {
        hook_log(format_args!(
            "{label}: REFUSED -- rva 0x{rva:x} has no verified mapping for the running build, so \
             writing a 3-byte stub there would overwrite whatever now occupies it"
        ));
        return false;
    };
    // One byte is not a signature, so the site is audited before it is trusted. `expected_first`
    // is `0x48`, `0x40`, `0x40` and `0x4c` at the four live call sites -- REX prefixes, which open
    // a large fraction of the image, so on a build that moved the function the check passes by
    // coincidence far more often than it fails and three bytes go into unrelated code. Measured
    // 2026-08-30: at their stale 1.16.2 RVAs on 1.17, all four targets are MID-FUNCTION.
    if !detour_site::write_site_is_sound(address, STUB_LEN as u32, label) {
        return false;
    }
    let target = address as *mut u8;
    let existing = unsafe { *target };
    if existing != expected_first {
        hook_log(format_args!(
            "{label}: ABORT -- byte at 0x{address:x} is 0x{existing:x}, expected 0x{expected_first:x}"
        ));
        return false;
    }
    let mut old_protect = PAGE_PROTECT_UNSET;
    let protect_ok = unsafe {
        VirtualProtect(
            target as *mut c_void,
            STUB_LEN,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if protect_ok == WIN32_FALSE {
        hook_log(format_args!("{label}: VirtualProtect failed"));
        return false;
    }
    let mut i = BYTE_START;
    while i < STUB_LEN {
        unsafe { *target.add(i) = stub[i] };
        i += BYTE_STEP;
    }
    let mut restored = PAGE_PROTECT_UNSET;
    unsafe { VirtualProtect(target as *mut c_void, STUB_LEN, old_protect, &mut restored) };
    unsafe {
        FlushInstructionCache(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            target as *const c_void,
            STUB_LEN,
        )
    };
    true
}

/// Patch a 0x48-prologue function body to `xor eax,eax; ret` (return 0) at `base+rva`. Validates
/// the expected first byte, VirtualProtects RWX, writes the 3-byte stub, restores protection, and
/// flushes the icache. Used to force-offline the IsOnlineMode getter + login-readiness predicate.
///
/// `expected_first` and `stub` were `ONLINE_DISABLE_EXPECTED_FIRST` / `ONLINE_DISABLE_STUB` read
/// from product constants; they are parameters now because this crate cannot see the product's
/// constant tree. Callers pass the same two values, so the rendered log text is unchanged.
#[cfg(windows)]
pub fn apply_xor_ret_stub(
    base: usize,
    rva: usize,
    expected_first: u8,
    stub: [u8; STUB_LEN],
    label: &str,
) {
    // Resolved first, for the reason spelled out on `patch_3byte_stub`: the expected-first-byte
    // check is an entry-point confirmation, not a version gate, and on a build that moved the
    // function it reports a byte mismatch that reads as a changed signature.
    let Some(address) = er_game_base::game_build::resolve_game_address(base + rva, label) else {
        hook_log(format_args!(
            "online-disable: REFUSED {label} -- rva 0x{rva:x} has no verified mapping for the \
             running build, so writing xor eax,eax;ret there would neuter whatever now occupies it"
        ));
        return;
    };
    // Audited before the byte check, for the reason spelled out on [`patch_3byte_stub`]: one REX
    // prefix is not a signature, and a mid-function address passes it routinely.
    if !detour_site::write_site_is_sound(address, STUB_LEN as u32, label) {
        return;
    }
    let target = address as *mut u8;
    let existing = unsafe { *target };
    if existing != expected_first {
        hook_log(format_args!(
            "online-disable: ABORT {label} -- byte at 0x{address:x} is 0x{existing:x}, expected 0x{expected_first:x}"
        ));
        return;
    }
    let mut old_protect = PAGE_PROTECT_UNSET;
    let protect_ok = unsafe {
        VirtualProtect(
            target as *mut c_void,
            STUB_LEN,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if protect_ok == WIN32_FALSE {
        hook_log(format_args!(
            "online-disable: VirtualProtect failed for {label}"
        ));
        return;
    }
    let mut i = BYTE_START;
    while i < STUB_LEN {
        unsafe { *target.add(i) = stub[i] };
        i += BYTE_STEP;
    }
    let mut restored = PAGE_PROTECT_UNSET;
    unsafe { VirtualProtect(target as *mut c_void, STUB_LEN, old_protect, &mut restored) };
    unsafe {
        FlushInstructionCache(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            target as *const c_void,
            STUB_LEN,
        )
    };
    hook_log(format_args!(
        "online-disable: patched {label} 0x{address:x} -> xor eax,eax;ret (forces offline)"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page operation, recorded rather than performed.
    #[derive(Debug, PartialEq, Eq)]
    enum Op {
        Protect {
            addr: usize,
            len: usize,
            new_protect: u32,
        },
        Store {
            addr: usize,
            value: u8,
        },
        Flush {
            addr: usize,
            len: usize,
        },
    }

    /// Stands in for a real code page. `original_protect` is what it reports as the page's previous
    /// protection, so a test can assert that exact value is handed back on the second call.
    struct FakePage {
        ops: Vec<Op>,
        original_protect: u32,
        protect_allowed: bool,
    }

    impl FakePage {
        fn allowing(original_protect: u32) -> Self {
            Self {
                ops: Vec::new(),
                original_protect,
                protect_allowed: true,
            }
        }

        fn refusing() -> Self {
            Self {
                ops: Vec::new(),
                original_protect: 0,
                protect_allowed: false,
            }
        }
    }

    impl CodePageOps for FakePage {
        fn protect(
            &mut self,
            addr: usize,
            len: usize,
            new_protect: u32,
            old_protect: &mut u32,
        ) -> bool {
            self.ops.push(Op::Protect {
                addr,
                len,
                new_protect,
            });
            if !self.protect_allowed {
                return false;
            }
            *old_protect = self.original_protect;
            true
        }

        unsafe fn store(&mut self, addr: usize, value: u8) {
            self.ops.push(Op::Store { addr, value });
        }

        fn flush(&mut self, addr: usize, len: usize) {
            self.ops.push(Op::Flush { addr, len });
        }
    }

    const PAGE_EXECUTE_READ: u32 = 0x20;
    const TEST_ADDR: usize = 0x1234_5678;
    const TEST_BYTE: u8 = 0xcc;

    /// The whole sequence, in order: unlock to RWX, store, relock, flush.
    #[test]
    fn writes_between_unlocking_and_relocking_then_flushes() {
        let mut page = FakePage::allowing(PAGE_EXECUTE_READ);

        let wrote = unsafe { write_code_byte_with(&mut page, TEST_ADDR, TEST_BYTE) };

        assert!(wrote);
        assert_eq!(
            page.ops,
            vec![
                Op::Protect {
                    addr: TEST_ADDR,
                    len: ONE_CODE_BYTE,
                    new_protect: PAGE_EXECUTE_READWRITE,
                },
                Op::Store {
                    addr: TEST_ADDR,
                    value: TEST_BYTE,
                },
                Op::Protect {
                    addr: TEST_ADDR,
                    len: ONE_CODE_BYTE,
                    new_protect: PAGE_EXECUTE_READ,
                },
                Op::Flush {
                    addr: TEST_ADDR,
                    len: ONE_CODE_BYTE,
                },
            ]
        );
    }

    /// The hazard: a patched page left writable-and-executable for the rest of the process. The
    /// relock must name the protection the page actually had, not a guess and not RWX.
    #[test]
    fn does_not_leave_the_page_executable_and_writable() {
        for original in [PAGE_EXECUTE_READ, 0x02, 0x04, 0x80] {
            let mut page = FakePage::allowing(original);

            unsafe { write_code_byte_with(&mut page, TEST_ADDR, TEST_BYTE) };

            let last_protect = page
                .ops
                .iter()
                .filter_map(|op| match op {
                    Op::Protect { new_protect, .. } => Some(*new_protect),
                    _ => None,
                })
                .next_back()
                .expect("a protection change");
            assert_eq!(
                last_protect, original,
                "page relocked to the wrong protection"
            );
            assert_ne!(last_protect, PAGE_EXECUTE_READWRITE, "page left RWX");
        }
    }

    /// A refused protection change must abort before the store. Writing anyway would fault, or
    /// worse, succeed on a page that was already writable and hide the refusal.
    #[test]
    fn refused_protection_change_writes_nothing() {
        let mut page = FakePage::refusing();

        let wrote = unsafe { write_code_byte_with(&mut page, TEST_ADDR, TEST_BYTE) };

        assert!(!wrote);
        assert_eq!(
            page.ops,
            vec![Op::Protect {
                addr: TEST_ADDR,
                len: ONE_CODE_BYTE,
                new_protect: PAGE_EXECUTE_READWRITE,
            }],
            "nothing may follow a refused VirtualProtect"
        );
    }

    /// What the `ONE_CODE_BYTE` doc claims: the protection, the store and the flush cover the same
    /// one byte at the same address. A length that disagreed would unlock or flush a range the
    /// caller never asked about.
    #[test]
    fn every_page_operation_covers_the_same_single_byte() {
        let mut page = FakePage::allowing(PAGE_EXECUTE_READ);

        unsafe { write_code_byte_with(&mut page, TEST_ADDR, TEST_BYTE) };

        assert_eq!(ONE_CODE_BYTE, size_of::<u8>());
        for op in &page.ops {
            let (addr, len) = match op {
                Op::Protect { addr, len, .. } | Op::Flush { addr, len } => (*addr, *len),
                Op::Store { addr, .. } => (*addr, ONE_CODE_BYTE),
            };
            assert_eq!(addr, TEST_ADDR, "{op:?} touched a different address");
            assert_eq!(len, ONE_CODE_BYTE, "{op:?} covered a different length");
        }
    }

    // ------------------------------------------------------------------
    // Registry ownership. The recording half needs `dll_base` (a Win32 call), so these drive the
    // pure decision + rendering halves and inject the offset formatter. What they pin is the
    // defect from run `br-20260831-160354-2513`: a bare detour colliding with a union-owned
    // address reported `already hooked by detour(s) []`.
    // ------------------------------------------------------------------

    /// Stand-in for `as_dll_off` that does not touch `GetModuleHandleExW`.
    fn fake_off(p: usize) -> String {
        format!("dll+0x{p:x}")
    }

    const INCUMBENT_DETOUR: usize = 0xdef60;
    const NEWCOMER_DETOUR: usize = 0xdf1d0;

    #[test]
    fn a_union_incumbent_is_named_rather_than_reported_as_an_empty_set() {
        let prior = [(INCUMBENT_DETOUR, HookOwner::Union)];
        assert_eq!(
            registry_verdict(
                &prior,
                NEWCOMER_DETOUR,
                HookOwner::Bare,
                MH_STATUS::MH_ERROR_ALREADY_CREATED,
            ),
            RegistryVerdict::Collision
        );
        let off: &dyn Fn(usize) -> String = &fake_off;
        assert_eq!(
            render_prior_owners(&prior, off),
            "union handler dll+0xdef60",
            "the incumbent's installer must be named, not just its offset"
        );
    }

    #[test]
    fn a_bare_incumbent_still_renders_as_the_plain_offset() {
        let prior = [(INCUMBENT_DETOUR, HookOwner::Bare)];
        let off: &dyn Fn(usize) -> String = &fake_off;
        assert_eq!(render_prior_owners(&prior, off), "dll+0xdef60");
    }

    #[test]
    fn one_owner_installing_twice_is_a_duplicate_not_a_collision() {
        let prior = [(INCUMBENT_DETOUR, HookOwner::Bare)];
        assert_eq!(
            registry_verdict(
                &prior,
                INCUMBENT_DETOUR,
                HookOwner::Bare,
                MH_STATUS::MH_ERROR_ALREADY_CREATED,
            ),
            RegistryVerdict::Duplicate
        );
    }

    #[test]
    fn the_same_detour_under_a_different_installer_is_a_collision() {
        // Same function pointer, but one came through the union and one through a bare `MhHook`:
        // they are contesting the MinHook slot, so calling it a duplicate would say "nothing is
        // lost" about a case where something is.
        let prior = [(INCUMBENT_DETOUR, HookOwner::Union)];
        assert_eq!(
            registry_verdict(
                &prior,
                INCUMBENT_DETOUR,
                HookOwner::Bare,
                MH_STATUS::MH_ERROR_ALREADY_CREATED,
            ),
            RegistryVerdict::Collision
        );
    }

    #[test]
    fn an_uncontested_first_registration_logs_nothing() {
        assert_eq!(
            registry_verdict(&[], NEWCOMER_DETOUR, HookOwner::Bare, MH_STATUS::MH_OK),
            RegistryVerdict::Fresh
        );
    }

    #[test]
    fn already_created_with_no_recorded_owner_is_still_a_collision() {
        // MinHook knows an owner this registry never saw -- another DLL's instance, or a hook
        // installed before the log sink existed. An empty owner list now means exactly that.
        assert_eq!(
            registry_verdict(
                &[],
                NEWCOMER_DETOUR,
                HookOwner::Bare,
                MH_STATUS::MH_ERROR_ALREADY_CREATED,
            ),
            RegistryVerdict::Collision
        );
    }

    // ------------------------------------------------------------------
    // The five-argument union. Everything below runs on the host, where `MH_CreateHook` does not
    // exist, so it drives the two halves that do not need it: [`union_admission`] (the decision,
    // including the arity refusal) and [`union_dispatch5`] + [`chain_append`] (the wiring and the
    // call itself). What no host test can prove is the Microsoft x64 stack forwarding of the fifth
    // argument -- `extern "system"` on this host is SysV, which passes five arguments entirely in
    // registers. The protection against getting that wrong is not a test but the type: the
    // dispatcher and the handler are both declared `fn(usize, usize, usize, usize, usize)`, so
    // rustc emits the caller side, and there is no hand-written `[rsp+0x20]` anywhere to be wrong.
    //
    // `UNION_HEADS` is process-wide and the harness runs tests in parallel threads, so each test
    // below owns a distinct slot index and its own statics.
    // ------------------------------------------------------------------

    /// A test handler's address, taken the way the registrar takes one: through the fn-pointer
    /// type the union will call it by. A direct fn-item cast is a lint error, and rightly -- it is
    /// the step at which an arity could be lost with no diagnostic at all.
    fn addr_of(f: UnionFn5) -> usize {
        f as usize
    }

    // Deliberately not a real game address. This used to be `0x1_4092_0c90`, the Quit row
    // cloner's `AddCancelButton` -- flavour, since these tests only manipulate the admission
    // table and never hook anything. It was also a second literal declaration of an address
    // `er-title-flow` already owns, which `scripts/check-rva-alias-drift.py` reads as two claims
    // about one function. A value below the image's `.text` cannot be either.
    const FAKE_TARGET: usize = 0x1_4000_0c90;
    const OTHER_TARGET: usize = 0x1_4074_6e80;
    const HANDLER_A: usize = 0xaaa0;
    const HANDLER_B: usize = 0xbbb0;
    const FAKE_TRAMPOLINE: usize = 0x7ffe_0000;

    /// A table entry as `register_union_hook_resolved_with` would have left it after one
    /// registration, without the `MH_CreateHook` that produced the trampoline.
    fn entry_with(target: usize, arity: UnionArity, handler: usize) -> UnionEntry {
        static SOLE_ORIG: AtomicUsize = AtomicUsize::new(0);
        let mut entry = UnionEntry {
            target,
            trampoline: FAKE_TRAMPOLINE,
            arity,
            handlers: Vec::new(),
        };
        chain_append(&mut entry, handler, &SOLE_ORIG);
        entry
    }

    #[test]
    fn an_unclaimed_address_takes_the_next_slot_at_either_arity() {
        assert_eq!(
            union_admission(&[], FAKE_TARGET, HANDLER_A, UnionArity::Five),
            UnionAdmission::Create(0)
        );
        let taken = [entry_with(OTHER_TARGET, UnionArity::Four, HANDLER_B)];
        assert_eq!(
            union_admission(&taken, FAKE_TARGET, HANDLER_A, UnionArity::Five),
            UnionAdmission::Create(1),
            "a five-argument target draws from the same slot index space as a four-argument one"
        );
    }

    #[test]
    fn a_second_handler_at_the_same_arity_chains() {
        let unions = [entry_with(FAKE_TARGET, UnionArity::Five, HANDLER_A)];
        assert_eq!(
            union_admission(&unions, FAKE_TARGET, HANDLER_B, UnionArity::Five),
            UnionAdmission::Chain(0)
        );
    }

    #[test]
    fn the_same_five_argument_handler_registering_twice_is_a_no_op() {
        let unions = [entry_with(FAKE_TARGET, UnionArity::Five, HANDLER_A)];
        assert_eq!(
            union_admission(&unions, FAKE_TARGET, HANDLER_A, UnionArity::Five),
            UnionAdmission::AlreadyPresent,
            "appending a handler to its own chain would make it call itself"
        );
    }

    /// The rule this whole arity split exists to hold: one prologue, one dispatcher. A second
    /// arity on a claimed address must be refused where it can still be reported, not discovered
    /// as an `MH_ERROR_ALREADY_CREATED` after one of the two has already lost.
    #[test]
    fn a_target_claimed_at_one_arity_is_refused_at_the_other() {
        let four = [entry_with(FAKE_TARGET, UnionArity::Four, HANDLER_A)];
        assert_eq!(
            union_admission(&four, FAKE_TARGET, HANDLER_B, UnionArity::Five),
            UnionAdmission::ArityConflict(UnionArity::Four)
        );
        let five = [entry_with(FAKE_TARGET, UnionArity::Five, HANDLER_A)];
        assert_eq!(
            union_admission(&five, FAKE_TARGET, HANDLER_B, UnionArity::Four),
            UnionAdmission::ArityConflict(UnionArity::Five),
            "the refusal has to run in both directions or it is just install-order luck"
        );
    }

    #[test]
    fn a_full_table_is_exhausted_rather_than_indexing_past_the_pool() {
        let unions: Vec<UnionEntry> = (0..MAX_UNION_SLOTS)
            .map(|i| entry_with(FAKE_TARGET + i, UnionArity::Four, HANDLER_A))
            .collect();
        assert_eq!(
            union_admission(&unions, OTHER_TARGET, HANDLER_B, UnionArity::Five),
            UnionAdmission::Exhausted
        );
    }

    // -------- the dispatcher, called for real --------

    /// Slot 95 belongs to `forwards_all_five_arguments`, 94 to `a_null_head_returns_zero`, 93 to
    /// the chaining test. Production never reaches them in a test binary, which installs no hooks.
    const DISPATCH_SLOT: usize = 95;
    const NULL_HEAD_SLOT: usize = 94;
    const CHAIN_SLOT: usize = 93;

    static SEEN: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5];
    const SOLE_HANDLER_RETURN: usize = 0x5011;

    unsafe extern "system" fn record_five(
        a: usize,
        b: usize,
        c: usize,
        d: usize,
        e: usize,
    ) -> usize {
        for (cell, value) in SEEN.iter().zip([a, b, c, d, e]) {
            cell.store(value, Ordering::SeqCst);
        }
        SOLE_HANDLER_RETURN
    }

    /// The fifth argument is the whole point: a four-argument dispatcher drops it, and for
    /// `AddCancelButton` the dropped value is the keyguide function pointer the game then calls.
    #[test]
    fn the_five_argument_dispatcher_forwards_all_five_arguments() {
        UNION_HEADS[DISPATCH_SLOT].store(addr_of(record_five), Ordering::Release);

        let returned = unsafe { union_dispatch5::<DISPATCH_SLOT>(11, 22, 33, 44, 55) };

        assert_eq!(returned, SOLE_HANDLER_RETURN);
        let seen: Vec<usize> = SEEN.iter().map(|c| c.load(Ordering::SeqCst)).collect();
        assert_eq!(seen, vec![11, 22, 33, 44, 55]);
        UNION_HEADS[DISPATCH_SLOT].store(0, Ordering::Release);
    }

    /// An unarmed slot returns 0 without calling anything, exactly as [`union_dispatch`] does.
    #[test]
    fn a_null_five_argument_head_returns_zero() {
        UNION_HEADS[NULL_HEAD_SLOT].store(0, Ordering::Release);
        assert_eq!(
            unsafe { union_dispatch5::<NULL_HEAD_SLOT>(1, 2, 3, 4, 5) },
            0
        );
    }

    // -------- two handlers on one five-argument target --------

    static OUTER_ORIG: AtomicUsize = AtomicUsize::new(0);
    static INNER_ORIG: AtomicUsize = AtomicUsize::new(0);
    static CALL_ORDER: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    static GAME_SAW: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5];

    const GAME_RETURN: usize = 0x6a3e;

    unsafe extern "system" fn fake_game(a: usize, b: usize, c: usize, d: usize, e: usize) -> usize {
        CALL_ORDER.lock().unwrap().push("game");
        for (cell, value) in GAME_SAW.iter().zip([a, b, c, d, e]) {
            cell.store(value, Ordering::SeqCst);
        }
        GAME_RETURN
    }

    unsafe extern "system" fn inner_handler(
        a: usize,
        b: usize,
        c: usize,
        d: usize,
        e: usize,
    ) -> usize {
        CALL_ORDER.lock().unwrap().push("inner");
        let orig: UnionFn5 =
            unsafe { std::mem::transmute::<usize, UnionFn5>(INNER_ORIG.load(Ordering::SeqCst)) };
        unsafe { orig(a, b, c, d, e) }
    }

    unsafe extern "system" fn outer_handler(
        a: usize,
        b: usize,
        c: usize,
        d: usize,
        e: usize,
    ) -> usize {
        CALL_ORDER.lock().unwrap().push("outer");
        let orig: UnionFn5 =
            unsafe { std::mem::transmute::<usize, UnionFn5>(OUTER_ORIG.load(Ordering::SeqCst)) };
        let from_chain = unsafe { orig(a, b, c, d, e) };
        CALL_ORDER
            .lock()
            .unwrap()
            .push(if from_chain == GAME_RETURN {
                "outer-saw-game-return"
            } else {
                "outer-saw-something-else"
            });
        // The outermost handler writes the return value last, so this is what the game's caller
        // gets. Deliberately not `from_chain`, or the assertion below could not tell the two apart.
        OUTER_RETURN
    }

    const OUTER_RETURN: usize = 0x0075e4;

    /// Two handlers on one five-argument address: strictly nested, first registrant outermost,
    /// every argument reaching the game unchanged, and the outermost handler's return value the
    /// one that survives.
    ///
    /// The wiring is done by [`chain_append`], the same function the registrar calls -- only the
    /// `MH_CreateHook` that would have produced the trampoline is stood in for.
    #[test]
    fn two_handlers_on_one_five_argument_target_nest_with_the_first_outermost() {
        let mut entry = UnionEntry {
            target: FAKE_TARGET,
            trampoline: addr_of(fake_game),
            arity: UnionArity::Five,
            handlers: Vec::new(),
        };
        // First registrant: becomes the head, and its orig is the game.
        assert_eq!(
            chain_append(&mut entry, addr_of(outer_handler), &OUTER_ORIG),
            1
        );
        UNION_HEADS[CHAIN_SLOT].store(addr_of(outer_handler), Ordering::Release);
        // Second registrant: the first now calls it, and its own orig becomes the game.
        assert_eq!(
            chain_append(&mut entry, addr_of(inner_handler), &INNER_ORIG),
            2
        );

        assert_eq!(
            OUTER_ORIG.load(Ordering::SeqCst),
            addr_of(inner_handler),
            "the first registrant must now call the second, not the game"
        );
        assert_eq!(INNER_ORIG.load(Ordering::SeqCst), addr_of(fake_game));
        assert_eq!(
            UNION_HEADS[CHAIN_SLOT].load(Ordering::SeqCst),
            addr_of(outer_handler),
            "chaining must not move the head off the first registrant"
        );

        let returned = unsafe { union_dispatch5::<CHAIN_SLOT>(0x11, 0x22, 0x33, 0x44, 0x55) };

        assert_eq!(
            *CALL_ORDER.lock().unwrap(),
            vec!["outer", "inner", "game", "outer-saw-game-return"],
            "the chain must be strictly nested, not a fan-out"
        );
        let game_saw: Vec<usize> = GAME_SAW.iter().map(|c| c.load(Ordering::SeqCst)).collect();
        assert_eq!(
            game_saw,
            vec![0x11, 0x22, 0x33, 0x44, 0x55],
            "all five arguments must survive two handlers"
        );
        assert_eq!(
            returned, OUTER_RETURN,
            "the first registrant writes the return value last"
        );
        UNION_HEADS[CHAIN_SLOT].store(0, Ordering::Release);
    }
}
