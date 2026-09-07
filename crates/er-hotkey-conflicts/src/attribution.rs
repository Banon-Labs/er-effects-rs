//! Which module actually asked for the key -- and why a stack walk alone does not answer that.
//!
//! # The problem
//!
//! A detour on `GetAsyncKeyState` wants the caller's return address, and a stack walk gives it.
//! But this DLL registers through the cross-DLL hook union, and the union chains handlers: the
//! stack inside our detour is
//!
//! ```text
//!   [ our handler ]                       <- where the walk starts
//!   [ another mod's handler on this API ] <- zero or more of these
//!   [ union_dispatch, in whichever DLL owns the MinHook instance ]
//!   [ the real caller ]                   <- the one we want
//!   [ whatever called that ]
//! ```
//!
//! Taking "the first frame outside our own module" therefore names the union host, or another
//! mod's handler -- and the second case is worse than useless, because it would report the mod
//! that merely observes the keyboard as the one that binds every key on it. That is a confident
//! wrong answer in a tool whose entire output is an accusation.
//!
//! # The rule, in three clauses
//!
//! 1. **Strip our own frames.** The walk starts inside this DLL's detour, so the first frames are
//!    always ours. Matched by module, never by a hard-coded skip count -- which frame the unwinder
//!    calls "frame zero" differs between Windows and Wine, and a constant there would be an
//!    untested assumption sitting under every attribution the DLL makes.
//!
//! 2. **Strip the union host.** Exactly one dispatcher frame stands between the patched prologue
//!    and the first handler, and it lives in whichever DLL owns the MinHook instance. That module
//!    is known at registration time (`HookRoute`), so this clause is exact rather than inferred.
//!
//! 3. **Strip the leading frames every chain agrees on -- but only while they still look like
//!    handlers.** Other mods' handlers on the same prologue are fixed overhead too, and their
//!    modules are not known in advance. They are the leading frames every call to that API shares,
//!    so a common prefix finds them. A common prefix alone is not safe, though, so the walk stops
//!    at the first frame that stops looking like a handler; [`fixed_prefix`] lists the four tests.
//!
//! Those four tests are not caution, they are the difference between a correct answer and a
//! confident wrong one. A raw longest-common-prefix breaks in two shapes that both occur:
//!
//! * a mod that is the only caller of an API shares its whole stack with itself, so the "common
//!   prefix" is its entire stack and the frame past it is the game's task runner -- the report
//!   would accuse `eldenring.exe` of binding the mod's hotkey;
//! * a mod polling one key from two call sites has chains that agree for several frames and then
//!   diverge somewhere deeper, so the prefix eats its own frames and lands past it.
//!
//! Requiring two distinct chains kills the first. Refusing to take two frames from one module, and
//! refusing to take a frame in the game executable at all, kill the second.
//!
//! Clause 2 also handles the case a hand-written skip-list cannot: when the union host DLL is
//! itself a caller, its module appears both in the stripped prefix and past it, and the strip is
//! positional rather than a name filter, so it stops in the right place.
//!
//! # Why the raw addresses are kept
//!
//! The prefix is only correct once enough chains have been seen, and more arrive throughout a
//! run. So nothing is attributed on the hot path: raw frame addresses are tallied as they come
//! and the whole tally is attributed from scratch each time a report is rendered. An early call
//! and a late one are always scored by the same rule.

// Windows-only in practice; ungated so the attribution rule -- the part that can be confidently
// wrong -- is covered by `cargo test` on the host.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::BTreeMap;

use crate::census::{Census, InputId, Surface};

/// Return addresses captured per call.
///
/// Sized against the worst chain this DLL can find itself in, and the budget is not only the
/// handler chain: the unwinder starts inside this DLL's own detour, so a couple of frames are
/// spent on us before the interesting ones begin. Twelve leaves room for our own frames, a union
/// dispatcher, three other handlers on the same prologue, and still several frames of real caller.
/// Undersizing this is not a graceful degradation -- the true caller falls off the end of the
/// capture and the call is attributed to whichever handler was still in frame, which is a
/// confident accusation against a module that merely observes the keyboard.
pub const MAX_FRAMES: usize = 12;

/// A captured call stack, fixed-size so it can be a map key without allocating on the hot path.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Frames {
    addresses: [usize; MAX_FRAMES],
    length: usize,
}

impl Frames {
    /// Build from a captured slice, truncating at [`MAX_FRAMES`].
    pub fn from_slice(captured: &[usize]) -> Self {
        let mut frames = Self::default();
        for address in captured.iter().take(MAX_FRAMES) {
            frames.addresses[frames.length] = *address;
            frames.length += 1;
        }
        frames
    }

    /// The captured addresses, outermost-first as the unwinder produced them.
    pub fn addresses(&self) -> &[usize] {
        &self.addresses[..self.length]
    }

    /// Is the capture empty? An empty capture means the unwinder failed and the call cannot be
    /// attributed at all.
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }
}

/// Raw, unattributed observations: the only thing the hot path writes.
///
/// Keyed by the frames themselves, so it is bounded by the number of distinct call sites in the
/// process rather than by the number of calls -- a few dozen entries for a whole session, however
/// many times a second the game polls.
#[derive(Clone, Debug, Default)]
pub struct RawTally {
    rows: BTreeMap<(Surface, InputId, Frames), u64>,
    unattributable: u64,
    dropped: u64,
}

impl RawTally {
    /// An empty tally, in a form a `static Mutex` can be initialised with.
    pub const fn new() -> Self {
        Self {
            rows: BTreeMap::new(),
            unattributable: 0,
            dropped: 0,
        }
    }

    /// Record one call.
    pub fn record(&mut self, surface: Surface, input: InputId, frames: Frames) {
        if frames.is_empty() {
            self.unattributable = self.unattributable.saturating_add(1);
            return;
        }
        let entry = self.rows.entry((surface, input, frames)).or_insert(0);
        *entry = entry.saturating_add(1);
    }

    /// Record calls the hot path could not store because the tally was locked.
    pub fn record_dropped(&mut self, times: u64) {
        self.dropped = self.dropped.saturating_add(times);
    }

    /// Distinct call sites recorded.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Total calls recorded, attributable or not.
    pub fn call_count(&self) -> u64 {
        self.rows
            .values()
            .copied()
            .fold(0, u64::saturating_add)
            .saturating_add(self.unattributable)
    }

    /// Calls dropped by a contended lock.
    pub const fn dropped(&self) -> u64 {
        self.dropped
    }
}

/// A resolved call stack: one module name per frame, outermost-first.
type Chain = Vec<String>;

/// How many other handlers a shared prologue can plausibly carry, and therefore the most frames
/// clause 3 will ever strip.
///
/// Three is the number of shells in this workspace that detour the DirectInput `GetDeviceState`
/// slot besides this one, which is the busiest prologue anybody here contends. The cap matters
/// more than the exact value: past it, a "common prefix" stops being evidence of chain overhead
/// and starts being evidence that one module is the only caller -- and stripping that lands on
/// the game's task runner and blames the game for a mod's hotkey.
pub const MAX_HANDLER_FRAMES: usize = 3;

/// Chains needed before a common prefix means anything.
///
/// A prefix computed over a single chain is just that chain. Two distinct chains are the minimum
/// at which "these leading frames are the same on every call" is a statement about the API's
/// handler chain rather than about one caller's stack.
pub const MIN_CHAINS_FOR_PREFIX: usize = 2;

/// Chain overhead that `chains` all agree on -- clause 3 of the rule in this module's docs.
///
/// A common prefix is the raw signal; on its own it is not safe, because a module that is the only
/// caller of an API shares its whole stack with itself. So the walk stops at the first frame that
/// fails any of four tests, each of which is a property a genuine handler frame has:
///
/// * **the chains still agree there** -- otherwise it is where the callers diverge, i.e. the answer;
/// * **the module has not already contributed a frame** -- a module installs one handler on one
///   prologue, so a repeat is the caller's own stack, not another link in the chain. This is what
///   stops a mod that polls one key from two call sites having its own frames eaten;
/// * **the frame is not in the game executable** -- Elden Ring does not detour its own imports, so
///   a frame there is always a real caller or the task runner beneath one;
/// * **fewer than [`MAX_HANDLER_FRAMES`] have been taken**, and at least one frame is left.
///
/// Returns 0 for fewer than [`MIN_CHAINS_FOR_PREFIX`] distinct chains.
pub fn fixed_prefix(chains: &[Chain], game_module: &str) -> usize {
    let mut distinct: Vec<&Chain> = Vec::new();
    for chain in chains {
        if !distinct.contains(&chain) {
            distinct.push(chain);
        }
    }
    if distinct.len() < MIN_CHAINS_FOR_PREFIX {
        return 0;
    }
    let Some(shortest) = distinct.iter().map(|chain| chain.len()).min() else {
        return 0;
    };
    let ceiling = shortest.saturating_sub(1).min(MAX_HANDLER_FRAMES);
    let mut taken: Vec<&str> = Vec::new();
    let mut prefix = 0;
    while prefix < ceiling {
        let head = &distinct[0][prefix];
        if distinct.iter().any(|chain| &chain[prefix] != head) {
            break;
        }
        if head == game_module || taken.contains(&head.as_str()) {
            break;
        }
        taken.push(head.as_str());
        prefix += 1;
    }
    prefix
}

/// The module a chain should be attributed to, given the fixed prefix for its API.
pub fn caller_of(chain: &[String], prefix: usize) -> Option<&str> {
    chain
        .get(prefix)
        .or_else(|| chain.last())
        .map(String::as_str)
}

/// Fold a raw tally into a census, resolving each frame address to a module name.
///
/// `resolve` returns the module a code address lives in, or `None` for an address in no mapped
/// module. `own_module` is this DLL's own file name and `union_host` the module that owns the
/// MinHook instance the detours were registered through -- clauses 1 and 2 of the rule in this
/// module's docs, both stripped from the front of every chain before clause 3 runs.
///
/// Also returns one diagnostic line per distinct chain, which is the evidence that attribution
/// worked at all -- a run where every chain resolves to the same module is a broken hook, and the
/// only way to see that is to print the chains.
pub fn fold(
    raw: &RawTally,
    own_module: &str,
    union_host: Option<&str>,
    game_module: &str,
    resolve: impl Fn(usize) -> Option<String>,
) -> (Census, Vec<String>) {
    let mut resolved: Vec<(Surface, InputId, Chain, usize, u64)> =
        Vec::with_capacity(raw.rows.len());
    let mut census = Census::default();
    for ((surface, input, frames), count) in &raw.rows {
        let chain: Chain = frames
            .addresses()
            .iter()
            .map(|address| resolve(*address).unwrap_or_else(|| format!("0x{address:x}")))
            .collect();
        // Clauses 1 and 2: our own detour's frames, then the union dispatcher's. Positional, not
        // a name filter -- a module that appears again later in the chain is a genuine caller and
        // must survive, which is exactly the case where the union host is itself the caller.
        let overhead = chain
            .iter()
            .take_while(|module| {
                module.as_str() == own_module || Some(module.as_str()) == union_host
            })
            .count();
        let stripped: Chain = chain[overhead..].to_vec();
        if stripped.is_empty() {
            census.record_unattributed(*count);
            continue;
        }
        resolved.push((*surface, *input, stripped, overhead, *count));
    }

    let mut by_surface: BTreeMap<Surface, Vec<Chain>> = BTreeMap::new();
    for (surface, _, chain, _, _) in &resolved {
        by_surface.entry(*surface).or_default().push(chain.clone());
    }
    // Clause 3, per API: a prefix found on one surface says nothing about another.
    let prefixes: BTreeMap<Surface, usize> = by_surface
        .iter()
        .map(|(surface, chains)| (*surface, fixed_prefix(chains, game_module)))
        .collect();

    let mut diagnostics = Vec::with_capacity(resolved.len());
    for (surface, input, chain, overhead, count) in &resolved {
        let prefix = prefixes.get(surface).copied().unwrap_or(0);
        match caller_of(chain, prefix) {
            Some(module) => {
                census.record(module, *input, *surface, *count);
                diagnostics.push(format!(
                    "  {} {} x{count} own+host={overhead} handlers={prefix} -> {module}  \
                     chain=[{}]",
                    surface.label(),
                    input.describe(),
                    chain.join(" <- ")
                ));
            }
            None => census.record_unattributed(*count),
        }
    }
    census.record_unattributed(raw.unattributable);
    census.record_dropped(raw.dropped);
    (census, diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(modules: &[&str]) -> Chain {
        modules.iter().map(|m| (*m).to_string()).collect()
    }

    const OURS: &str = "er_hotkey_conflicts.dll";
    const HOST: &str = "er_effects_rs.dll";
    const GAME: &str = "eldenring.exe";
    const VK_F7: u16 = 0x76;
    const VK_F8: u16 = 0x77;

    /// Clause 3 on the shape it exists for: two other handlers chained ahead of us on one
    /// prologue, present on every call, absent from the answer.
    #[test]
    fn frames_every_chain_shares_are_treated_as_chain_overhead() {
        let chains = vec![
            chain(&["er_net_effects.dll", "er_charm_enemies.dll", GAME]),
            chain(&[
                "er_net_effects.dll",
                "er_charm_enemies.dll",
                "er_invasion_warp.dll",
            ]),
        ];
        let prefix = fixed_prefix(&chains, GAME);
        assert_eq!(prefix, 2);
        assert_eq!(caller_of(&chains[0], prefix), Some(GAME));
        assert_eq!(caller_of(&chains[1], prefix), Some("er_invasion_warp.dll"));
    }

    /// The failure the cap exists for, and the reason clause 3 is not just "the longest common
    /// prefix". One caller, polling from its own game task: every frame of its stack is common to
    /// every chain, so an uncapped prefix walks past the mod entirely and lands on the game's task
    /// runner -- and the report would then accuse `eldenring.exe` of binding the mod's hotkey.
    #[test]
    fn a_lone_callers_whole_stack_is_not_mistaken_for_chain_overhead() {
        let lone = chain(&[
            "er_invasion_warp.dll",
            "er_invasion_warp.dll",
            GAME,
            GAME,
            GAME,
        ]);
        let chains = vec![lone.clone(), lone.clone(), lone.clone()];
        assert_eq!(
            fixed_prefix(&chains, GAME),
            0,
            "one distinct chain is not evidence about a handler chain"
        );
        assert_eq!(caller_of(&lone, 0), Some("er_invasion_warp.dll"));
    }

    /// The same failure one step subtler: two call sites inside one module, differing only deep in
    /// the stack. The cap keeps the answer on the module that made the call.
    #[test]
    fn two_call_sites_in_one_module_do_not_produce_a_deep_prefix() {
        let chains = vec![
            chain(&[
                "er_invasion_warp.dll",
                "er_invasion_warp.dll",
                GAME,
                "a.dll",
            ]),
            chain(&[
                "er_invasion_warp.dll",
                "er_invasion_warp.dll",
                GAME,
                "b.dll",
            ]),
        ];
        let prefix = fixed_prefix(&chains, GAME);
        assert!(prefix <= MAX_HANDLER_FRAMES);
        assert_eq!(
            caller_of(&chains[0], prefix),
            Some("er_invasion_warp.dll"),
            "the answer must still be the module that made the call"
        );
    }

    /// The game does not detour its own imports, so a frame in `eldenring.exe` is always a real
    /// caller -- the game polling, or the task runner beneath a mod that is. It can never be a
    /// link in a handler chain, so the walk stops there however well the chains agree.
    #[test]
    fn a_frame_in_the_game_executable_is_never_chain_overhead() {
        let chains = vec![
            chain(&["er_net_effects.dll", GAME, "a.dll"]),
            chain(&["er_net_effects.dll", GAME, "b.dll"]),
        ];
        assert_eq!(fixed_prefix(&chains, GAME), 1);
        assert_eq!(caller_of(&chains[0], 1), Some(GAME));
    }

    #[test]
    fn no_chains_means_no_prefix() {
        assert_eq!(fixed_prefix(&[], GAME), 0);
    }

    /// A one-frame chain has nowhere to strip to, so the prefix is zero and the single frame is
    /// the answer.
    #[test]
    fn a_single_frame_chain_attributes_to_that_frame() {
        let chains = vec![chain(&["er_invasion_warp.dll"]), chain(&[GAME])];
        assert_eq!(fixed_prefix(&chains, GAME), 0);
        assert_eq!(caller_of(&chains[0], 0), Some("er_invasion_warp.dll"));
    }

    /// The capture has to be deep enough to hold our own frames, a dispatcher, every other
    /// handler, and still reach the caller. An undersized capture does not degrade gracefully:
    /// the caller falls off the end and the call is attributed to whichever handler was last in
    /// frame.
    #[test]
    fn the_capture_is_deeper_than_the_worst_chain_it_must_see_through() {
        const OWN_FRAMES: usize = 2;
        const DISPATCHER_FRAME: usize = 1;
        let worst_case_overhead = OWN_FRAMES + DISPATCHER_FRAME + MAX_HANDLER_FRAMES;
        assert!(
            MAX_FRAMES > worst_case_overhead + 1,
            "MAX_FRAMES={MAX_FRAMES} leaves no caller frame past {worst_case_overhead} of overhead"
        );
    }

    /// End to end through [`fold`], with a fake address-to-module map: the F7 pair comes out as
    /// two named modules on one key, which is the whole product of the DLL.
    #[test]
    fn folding_produces_the_reproducer_collision() {
        let modules = |address: usize| -> Option<String> {
            Some(
                match address {
                    0x1000 => OURS,
                    0x2000 => HOST,
                    0x3000 => "er_invasion_warp.dll",
                    0x4000 => "er_invasion_path.dll",
                    _ => GAME,
                }
                .to_string(),
            )
        };
        let mut raw = RawTally::default();
        for _ in 0..10 {
            raw.record(
                Surface::AsyncKeyState,
                InputId::Key(VK_F7),
                Frames::from_slice(&[0x1000, 0x2000, 0x3000, 0x9000]),
            );
            raw.record(
                Surface::AsyncKeyState,
                InputId::Key(VK_F7),
                Frames::from_slice(&[0x1000, 0x2000, 0x4000, 0x9000]),
            );
        }
        let (census, diagnostics) = fold(&raw, OURS, Some(HOST), GAME, modules);
        let collisions = census.key_collisions(GAME);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].input, InputId::Key(VK_F7));
        assert_eq!(
            collisions[0].modules,
            vec!["er_invasion_path.dll", "er_invasion_warp.dll"]
        );
        assert_eq!(diagnostics.len(), 2, "one line per distinct call site");
        assert!(diagnostics.iter().all(|line| line.contains("own+host=2")));
    }

    /// Prefixes are computed per API. A busy surface must not shift a quiet one's answer.
    #[test]
    fn each_surface_gets_its_own_prefix() {
        let modules = |address: usize| -> Option<String> {
            Some(
                match address {
                    0x2000 => HOST,
                    0x3000 => "er_invasion_warp.dll",
                    0x4000 => "er_invasion_path.dll",
                    _ => GAME,
                }
                .to_string(),
            )
        };
        let mut raw = RawTally::default();
        // Two callers behind a dispatcher, which clause 2 strips by module.
        raw.record(
            Surface::AsyncKeyState,
            InputId::Key(VK_F7),
            Frames::from_slice(&[0x2000, 0x3000, 0x9000]),
        );
        raw.record(
            Surface::AsyncKeyState,
            InputId::Key(VK_F8),
            Frames::from_slice(&[0x2000, 0x4000, 0x9000]),
        );
        // A different API this DLL reached without any dispatcher in front of it.
        raw.record(
            Surface::RegisterHotKey,
            InputId::Key(VK_F8),
            Frames::from_slice(&[0x4000, 0x9000]),
        );
        raw.record(
            Surface::RegisterHotKey,
            InputId::Key(VK_F7),
            Frames::from_slice(&[0x3000, 0x9000]),
        );
        let (census, _) = fold(&raw, OURS, Some(HOST), GAME, modules);
        let rows: Vec<_> = census.rows().collect();
        assert!(rows.contains(&(
            "er_invasion_warp.dll",
            InputId::Key(VK_F7),
            Surface::AsyncKeyState,
            1
        )));
        assert!(rows.contains(&(
            "er_invasion_path.dll",
            InputId::Key(VK_F8),
            Surface::RegisterHotKey,
            1
        )));
    }

    /// An unwinder that returns nothing must not be silently discarded: it is the difference
    /// between "nothing collided" and "this DLL could not see anything".
    #[test]
    fn an_empty_capture_counts_as_unattributable() {
        let mut raw = RawTally::default();
        raw.record(
            Surface::AsyncKeyState,
            InputId::Key(VK_F7),
            Frames::from_slice(&[]),
        );
        assert_eq!(raw.call_count(), 1);
        let (census, _) = fold(&raw, OURS, Some(HOST), GAME, |_| None);
        assert_eq!(census.unattributed_calls(), 1);
        assert_eq!(census.attributed_calls(), 0);
    }

    /// A chain that is nothing but our own frames means the API was called from inside this DLL,
    /// which is never anybody's binding.
    #[test]
    fn a_chain_entirely_inside_this_dll_is_not_attributed() {
        let mut raw = RawTally::default();
        raw.record(
            Surface::AsyncKeyState,
            InputId::Key(VK_F7),
            Frames::from_slice(&[0x1000, 0x1000]),
        );
        let (census, _) = fold(&raw, OURS, Some(HOST), GAME, |_| Some(OURS.to_string()));
        assert_eq!(census.unattributed_calls(), 1);
        assert_eq!(census.attributed_calls(), 0);
    }

    /// An address in no mapped module is rendered as a bare address rather than dropped, so a
    /// caller in a manually mapped image still shows up as *something* distinct.
    #[test]
    fn an_unmapped_address_is_kept_as_a_hex_frame() {
        let mut raw = RawTally::default();
        raw.record(
            Surface::AsyncKeyState,
            InputId::Key(VK_F7),
            Frames::from_slice(&[0xdead_beef]),
        );
        let (census, _) = fold(&raw, OURS, Some(HOST), GAME, |_| None);
        assert!(census.modules().contains("0xdeadbeef"));
    }

    #[test]
    fn frames_truncate_at_the_capture_limit() {
        let captured: Vec<usize> = (1..=MAX_FRAMES + 4).collect();
        let frames = Frames::from_slice(&captured);
        assert_eq!(frames.addresses().len(), MAX_FRAMES);
        assert_eq!(frames.addresses()[0], 1);
    }
}
