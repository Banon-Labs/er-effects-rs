//! ERSC entry points, session-object layout and state codes -- one [`Abi`] per Seamless Co-op
//! build, chosen at runtime by byte-checking the module that is actually loaded.
//!
//! # Why this is a table rather than four constants
//!
//! `ersc.dll` is third-party. The user installs it and updates it whenever they like, and the last
//! update moved every address this module pins and the fields those addresses operate on.
//! A single constant set can only ever be right about one build, and the build it is right about
//! changes without warning -- so the addresses live in an [`Abi`] record that
//! [`super::resolve_ersc_abi`] selects by fingerprint, refusing anything it cannot identify.
//!
//! **[`SUPPORTED`] holds exactly one entry, and that is a product decision: this mod drives the
//! latest Seamless Co-op only.** The table is not vestigial -- Seamless will update again, and the
//! next build replaces this entry, measured the way this one was. A build that is no longer the
//! latest leaves nothing behind: no second entry, no fingerprint, no pinned addresses. The
//! refusal does not need to name which stale build it found, only that it is not the supported
//! one.
//!
//! # How each address here was identified
//!
//! A byte match was not accepted as an identification for any of these. Every one is reproducible
//! with `scripts/ersc-disas.py` against the installed DLL, read in place.
//!
//! | | RVA | what identifies it |
//! |---|---|---|
//! | `show` | `0x241a0` | unique masked-body match; `.pdata` index 301; function size `0xa0a` |
//! | invade action | `0x25850` | `.pdata` index 319, on a shift that holds unbroken over 78 consecutive functions; the only action of its shape writing `0xe`, at `0x25886`, out of 4903 functions |
//! | cancel action | `0x258d0` | unique masked-body match, and `.pdata` index 320 on that same shift, and size `0x64` |
//! | `BuildLobbyKey` | `0xad6e0` | `.pdata` index 1782, size `0x2d1`; loads the SHA-256 IV and a 32-byte `.rdata` salt `0x23` bytes apart; its two callers sit in one function that calls it twice `0xaf` apart; its opening decodes to `movzx edx,[rcx+CTX]; shr edx,2; and edx,1` |
//!
//! Two lessons from that measurement are worth keeping, because they are about method and will
//! apply again the next time Seamless ships:
//!
//! * **A masked body signature can miss a function that has not moved.** The compiler inverted the
//!   invade action's idle guard -- `cmp [rdi+STATE],IDLE; je <continue>` became
//!   `cmp [rdi+STATE],IDLE; jne <return>` -- and `je`/`jne` differ in the OPCODE byte, which a
//!   masked search keeps. One byte at offset 21 defeated every signature length on the ladder.
//! * **A unique prologue hit is a code shape, not a function.** `BuildLobbyKey`'s 19-byte prologue
//!   matched exactly one address and it was the wrong one: `0x1a6d0` merely happens to frame
//!   `0x148` too, and sits at `.pdata` index 193 -- 1589 entries from the real function.
//!   Big-frame MSVC functions in one source file look alike from the top; the resolution has to
//!   come from reading, not from picking.
//!
//! # The session ABI moves as one block
//!
//! The mutex sub-object, the guard and the state field keep their relative spacing
//! (`guard = mutex+0x4c`, `state = mutex+0x50`), so they move together. That is one finding with
//! three consequences rather than three separate measurements, and it is the shape to re-check
//! first when the fields move again.
//!
//! # The state enum renumbers wholesale, so leaving a code alone is the dangerous option
//!
//! Scanning every `mov dword [reg+STATE], imm32` in the real code finds seven distinct values.
//! When Seamless last shifted them, all seven moved by a uniform `+1` and a new state was inserted
//! at the bottom -- including the "idle" sentinel the guarded actions compare against. A state code
//! carried across a build unchanged is therefore not the conservative choice: it is how a progress
//! marker silently becomes a fast-fail state.
//!
// Every `*_PROLOGUE` below is assembled from named `iced-x86` instructions by this crate's
// `build.rs`, which additionally checks each one against a real copy of the build it claims to
// describe -- located by the Seamless version string inside the file rather than by which
// directory it sits in. Hand-typing them is what the generator exists to prevent: one wrong
// byte and every check here silently fails closed, which looks exactly like "Seamless is not
// loaded".
include!(concat!(env!("OUT_DIR"), "/generated_ersc_prologues.rs"));

/// Everything a Seamless build's ABI consists of, in one record.
///
/// Splitting it up is what made the previous breakage silent-adjacent: a correct new address
/// used with the previous build's field offsets would read and write the wrong fields of a
/// live multiplayer session, and this module's failure mode is cancelling other players'
/// invasions. Address, layout and codes travel together or not at all.
pub struct Abi {
    /// Human-readable build name, for the log line that says which one was recognised.
    pub version: &'static str,
    /// `show(void* OSM, int groupId)` -- the option-menu builder. Read to learn OSM, and the
    /// one function in Seamless this module hooks.
    pub show_rva: usize,
    /// The "Invade world" option action. Reads `rcx` only.
    pub invade_action_rva: usize,
    /// The "Cancel search" option action. Reads `rcx` only.
    pub cancel_action_rva: usize,
    /// `BuildLobbyKey(ctx, std::string* out)` -- produces the `lobby_key` string.
    ///
    /// # Why this one matters more than it looks
    ///
    /// Seamless finds worlds with a Steam lobby-list query carrying exactly two match terms:
    /// `lobby_type == "yknx3_seamless_master_lobby"` and `lobby_key == <this string>`, the
    /// latter with `k_ELobbyComparisonEqual`. An exhaustive decode of the XOR-obfuscated
    /// string idiom across every plaintext function in `ersc.dll` finds only those two keys in
    /// the whole module -- no map, block, region, coordinate or radius anywhere. So two
    /// players who derive different `lobby_key` values are simply invisible to each other, no
    /// matter how well their levels, weapon levels or locations line up.
    ///
    /// The value is [`LOBBY_KEY_HEX_LEN`] lowercase hex characters -- a SHA-256 digest, from
    /// an IV this function loads out of `.rdata` together with a 32-byte salt. **Seamless
    /// rotates that salt on release** -- measured 2026-09-06, no two of the last three builds
    /// share one -- which by itself makes clients of different builds invisible to each other,
    /// worth knowing before anyone reads a key mismatch as a bug in this module.
    ///
    /// The moment it re-derives is jittered, so a comparison between two machines must use the
    /// key that was live when the offers arrived, not merely the first one printed. Observed,
    /// never altered: publishing a key of our own would change what every other Seamless
    /// client matches, which is not ours to do.
    pub build_lobby_key_rva: usize,
    pub show_prologue: &'static [u8],
    /// Also the version DISCRIMINATOR -- see [`SUPPORTED`].
    pub invade_prologue: &'static [u8],
    pub cancel_prologue: &'static [u8],
    pub build_lobby_key_prologue: &'static [u8],
    /// Session state, the field every option action writes.
    pub session_state_offset: usize,
    /// Guard field. [`SESSION_GUARD_POISON`] is the value every action refuses to proceed
    /// past -- they take a fatal-error branch instead -- so the filter refuses too.
    pub session_guard_offset: usize,
    /// Idle: the state a cancelled search settles back to, and the state `invade` requires.
    pub state_idle: u32,
    /// The state the "Invade world" action writes -- the one and only site in the whole
    /// unpacked `.text` that puts this value in the field.
    pub state_searching: u32,
    /// The state "Cancel search" writes.
    ///
    /// Not usable as a "the user cancelled" signal, which is what it was briefly used for: the
    /// static scan found seven sites writing this value and only one is the Cancel action, so
    /// every internal abort looked like a user cancel -- and it was still seven sites after the
    /// last renumber. It survives as a label for the trace.
    pub state_cancelling: u32,
    /// The first state past the fast-fail path, our own progress marker for the restart
    /// backoff. See `note_attempt_progress`, its only reader, for what it gates.
    ///
    /// The one number in this table that is inferred rather than read out of an instruction: it
    /// was measured at runtime on the preceding build and carried across the proven enum-wide
    /// `+1`. Leaving it unshifted would have been worse than moving it -- under that renumber the
    /// stale value lands on a fast-fail state, so the marker would clear the backoff penalty on
    /// exactly the attempts that earned it.
    pub state_offer_received: u32,
}

/// Every Seamless build this module knows how to drive: the latest one.
///
/// [`super::resolve_ersc_abi`] requires exactly one of these to match the loaded module and
/// refuses otherwise -- zero matches means a build this repo has not measured, and two would mean
/// the discriminator does not discriminate. With a single entry the "two matched" arm cannot fire;
/// it stays because the entry after the next Seamless update has to earn its place, and a pin too
/// weak to tell itself from its predecessor must fail loudly rather than silently win a race.
///
/// The discriminator is the invade action rather than `show` for two independent reasons.
/// `show` survived the last update byte-identical at a different address, so it could not tell
/// one build from the other at all. And this module hooks `show`, so once the detour is
/// installed those bytes are our own; a fingerprint taken there measures our patch and concludes
/// Seamless is a stranger, which is a bug this module has already had once.
pub const SUPPORTED: &[Abi] = &[Abi {
    // Not a literal. The runtime armed against v2.0.1 on 2026-09-02 while this field still read
    // "v2.0.0", so the log line announced the wrong Seamless build beside correctly re-pinned
    // addresses -- the exact second-copy-of-the-version-number that AGENTS.md forbids, and the
    // most misleading possible place for it, since this string is what a reader trusts to say
    // which build the addresses on the same line belong to.
    version: SUPPORTED_VERSION,
    show_rva: V201_SHOW_RVA,
    invade_action_rva: V201_INVADE_ACTION_RVA,
    cancel_action_rva: V201_CANCEL_ACTION_RVA,
    build_lobby_key_rva: V201_BUILD_LOBBY_KEY_RVA,
    show_prologue: V201_SHOW_PROLOGUE,
    invade_prologue: V201_INVADE_PROLOGUE,
    cancel_prologue: V201_CANCEL_PROLOGUE,
    build_lobby_key_prologue: V201_BUILD_LOBBY_KEY_PROLOGUE,
    session_state_offset: V201_SESSION_STATE_OFFSET,
    session_guard_offset: V201_SESSION_GUARD_OFFSET,
    state_idle: 0x01,
    state_searching: 0x0e,
    state_cancelling: 0x23,
    state_offer_received: 0x13,
}];

// The addresses and field offsets are named constants rather than literals inside the table
// above, and that is not a style choice: `scripts/check-expression-constants.py` derives its
// gated population from `const` declarations whose name carries `RVA`, and a value written as
// a bare struct-literal field is invisible to it. Inlining these once already dropped six
// names out of that population, which the coverage floor caught. They keep the version prefix so
// that a re-pin lands as a new set of names beside the old ones for exactly one commit, rather
// than as an in-place edit of numbers nobody diffed.

/// Seamless v2.0.1, the supported build. See this module's docs for what identifies each one.
pub const V201_SHOW_RVA: usize = 0x2_41a0;
pub const V201_INVADE_ACTION_RVA: usize = 0x2_5850;
pub const V201_CANCEL_ACTION_RVA: usize = 0x2_58d0;
pub const V201_BUILD_LOBBY_KEY_RVA: usize = 0xa_d6e0;
pub const V201_SESSION_STATE_OFFSET: usize = 0x150;
pub const V201_SESSION_GUARD_OFFSET: usize = 0x14c;

/// `LOBBY_KEY_HEX_LEN` lowercase hex characters: a SHA-256 digest, so 32 bytes rendered as 64
/// characters. The digest width, so it survives an update; the SHA-256 IV has not moved.
pub const LOBBY_KEY_HEX_LEN: usize = 64;
/// MSVC `std::string`: `{ union { char buf[16]; char* ptr; }; size_t size; size_t capacity; }`.
/// A capacity of 16 or more means the bytes are on the heap and the first field is a pointer --
/// which is always the case here, because a 64-character value cannot fit the inline buffer.
pub const STD_STRING_SIZE_OFFSET: usize = 0x10;
pub const STD_STRING_CAPACITY_OFFSET: usize = 0x18;
pub const STD_STRING_HEAP_CAPACITY: usize = 0x10;

/// `OSM+0x58` is the session object, and it has survived every update so far: all five option
/// actions still open `mov rdi,[rcx+0x58]`.
///
/// A `.data` singleton holding OSM would have let this module hook nothing in Seamless. One
/// was looked for and not found: the only `.data` global that is loaded and then dereferenced
/// at `+0x58` is `ersc+0x21b228`, and it is read at 121 sites, written by a pair of adjacent
/// CRT-shaped setters, and non-zero in the file -- a locale/allocator global that the search
/// matched by coincidence, not the session. The `seamless` tag likewise lives inside longer
/// strings (`seamless buddy system`, source paths), so there is no constructor to trace back
/// to a singleton either. Recorded here so the next reader does not repeat the hunt.
pub const NEXT_OBJECT_OFFSET: usize = 0x58;
/// OSM carries the ASCII tag `seamless` here. Measured live 2026-08-04, reported as
/// a diagnostic and believed by nothing -- see `show_observer` for the day it was a gate.
pub const OSM_TAG_OFFSET: usize = 0x68;
pub const OSM_TAG: &[u8] = b"seamless";
/// The value the guard field holds when the session is unusable. Every option action still
/// compares against it.
pub const SESSION_GUARD_POISON: u32 = 0x7fff_ffff;
/// The highest plausible session state, used to reject a pointer that is not a session at all.
/// Comfortably above the largest code the build writes (`0x24`).
pub const SESSION_STATE_MAX: u32 = 0xff;
