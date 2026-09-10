//! The world-map seams this DLL detours, and the guard that refuses to hook a wrong build.
//!
//! # Two independent checks, and why both exist
//!
//! Every address here was byte-checked offline against `eldenring-deobf.bin` at shift 0
//! (`python3 scripts/check-dump-deobf-identity.py --count 32 0x<va>`). That proves the address
//! is right *for the image we reverse-engineered*. It says nothing about the image actually
//! running, which is why each seam also carries the first bytes of its prologue and
//! [`verify_seam`] re-reads them from live memory before anything is patched. A patch applied
//! to a differently-built game lands mid-instruction and crashes; refusing to hook merely costs
//! the feature.
//!
//! # What a prologue signature does and does not catch
//!
//! It catches drift -- a different game build where the function moved or changed. It does not
//! identify which function this is: the three `BonfireWarp*` param lookups share a byte-identical
//! 12-byte prologue (`40 57 48 83 ec 40 48 c7 44 24 20 fe`) because they are the same
//! binary-search shape over different param tables. Only the RVA distinguishes them, and the RVA
//! is only as good as the offline byte-check that produced it. Do not "fix" a mismatch by
//! relaxing the signature.
//!
//! # Argument-count trap
//!
//! `er_hook`'s union dispatcher is a four-argument `extern "system"` shape. A target taking five
//! or more register/stack arguments silently loses the extras -- including out-parameters the
//! callee writes through, which corrupts memory rather than failing loudly. Any seam added here
//! with more than four arguments must get its own typed `MhHook` instead of riding the union;
//! `arg_count` records the count so that decision is explicit rather than remembered.

/// One detour target: where it is, what it looks like, and how many arguments it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapSeam {
    /// Human name, used in the refusal log line.
    pub name: &'static str,
    /// `VA - 0x140000000`.
    pub rva: usize,
    /// First bytes of the function's prologue in the image the addresses were verified against.
    pub prologue: &'static [u8],
    /// Register/stack argument count. `> 4` means this seam must not ride the four-argument
    /// union dispatcher.
    pub arg_count: usize,
}

impl MapSeam {
    /// Whether this seam can ride `er_hook`'s four-argument union dispatcher without dropping
    /// arguments.
    #[must_use]
    pub const fn fits_union_dispatcher(&self) -> bool {
        self.arg_count <= UNION_DISPATCHER_ARG_COUNT
    }

    /// The absolute VA this seam was verified at, for logs and cross-referencing the RE notes.
    #[must_use]
    pub const fn verified_va(&self) -> usize {
        RE_IMAGE_BASE + self.rva
    }
}

/// Image base every VA in the RE notes is expressed against. For 1.16.2 the dump VA, the
/// `eldenring-deobf.bin` VA and the live runtime VA are identical, so `RVA = VA - this`.
pub const RE_IMAGE_BASE: usize = 0x1_4000_0000;

/// Arguments `er_hook`'s union dispatcher forwards. Exceeding this silently truncates.
pub const UNION_DISPATCHER_ARG_COUNT: usize = 4;

/// `CS::WorldMapViewModel::WorldMapViewModel` -- `0x1408855b0`. The row list at `+0x2d8` is
/// populated here and nowhere else, so an epilogue hook here is the injection seam for synthetic
/// rows.
///
/// Lifetime, corrected twice and now pinned by the static call graph: the ViewModel is built once
/// per world entry -- not once per session, and not once per map view. This ctor's only code xref
/// is `FUN_1407ed840 @0x1407ed8d3`, whose body is `if (popupMenu->worldMapViewModel == NULL) {
/// alloc(0x450, MenuHeap); ctor(...) }`; its only caller is `FUN_140766010 @0x14076607b`, whose
/// only caller is `MoveMapStep`'s constructor `@0x140af2e47`, reached from `STEP_MoveMap_Init`.
/// Teardown mirrors it: `~MoveMapStep -> FUN_140765fa0 @0x140af3eb5 -> FUN_1407ed790`, which runs
/// the dtor, frees the block and nulls the slot. So the object's lifetime is `MoveMapStep`'s.
///
/// Two consequences that have each cost a wrong diagnosis:
/// - A map-layer switch (`FUN_1409c1fc0`) does not come near this function -- it mutates
///   `dialog+0xa88` and re-sizes the clip pool. `VIEWMODEL_CTOR_HITS > 1` means the player moved
///   maps, never that they toggled a layer.
/// - Opening the map does not rebuild the list either. There is no refresh, rebuild or dirty-flag
///   path anywhere in the image: the grow helper, the row ctor and the row copy-ctor each have
///   exactly one calling function, and it is this one. Anything the list must gain later, we have
///   to put there ourselves.
///
/// Keying injection on the `this` pointer is therefore unsound: the block is freed to the same heap
/// at the same size class, so a later ViewModel can land on the same address.
pub const WORLDMAP_VIEWMODEL_CTOR: MapSeam = MapSeam {
    name: "CS::WorldMapViewModel::WorldMapViewModel",
    rva: 0x088_55b0,
    prologue: &[
        0x48, 0x8b, 0xc4, 0x55, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57,
    ],
    arg_count: 1,
};

/// `CS::SosSignMan::SetMultiplayJoinData` -- `0x1406fb520`. The single function that writes every
/// CSGameMan field the invasion destination lives in, from a 128-byte server-pushed struct in RDX:
/// `SetTargetMapId` -> `GameMan+0xAC8`, `SetMultiplayJoinTargetBlockPos` -> `+0xAA0`,
/// `SetNPCInvadeTargetEntryPoint(0)` -> `+0xAF0` (hard zero, which is why that field always read
/// 0 in the RAM sampling).
///
/// This is the seam the location filter judges at: the destination is decided and the player has
/// not moved. Measured live 2026-08-05 -- `ServerPushJoinData+0x00` was the only offset in all 128
/// bytes whose u32 equalled the destination that landed in `GameMan+0xAC8`, so the field is
/// identified by correlation rather than by its reversed name (`matchPlayerCount`, which would
/// have pointed at the wrong dword).
pub const SET_MULTIPLAY_JOIN_DATA: MapSeam = MapSeam {
    name: "CS::SosSignMan::SetMultiplayJoinData",
    rva: 0x06f_b520,
    prologue: &[0x40, 0x53, 0x48, 0x81, 0xec, 0x80, 0x00, 0x00, 0x00],
    arg_count: 2,
};

/// `CS::CSSessionManager::JoinSession` -- `0x140cae640`. The call that issues the Steam join RPC,
/// and the last instant at which a rejected match is still free.
///
/// `SosSignMan::JoinSession` calls `SetMultiplayJoinData` and then this, 87 bytes later in the same
/// function, so the seam the filter already judges at runs immediately before the join is sent.
///
/// # Why a rejection has to be refused here rather than cancelled afterwards
///
/// Once this has run, `lobbyState` is `Joining` and the engine will not unwind:
/// `CSSessionManager::LeaveSession` (`0x140cae730`) sets `disconnectRequested` and returns without
/// tearing anything down while the state is `Creating` or `Joining`, and `CSSessionManagerImp::
/// Update` (`0x140cafd10` in 1.16.2) only acts on that request under
/// `lobbyState != Creating && lobbyState != Joining`. So the disconnect is parked until the join
/// RPC resolves on its own.
///
/// Measured on run `br-20260910-012622-fd23`, four rejections: the two whose RPC resolved reached
/// `lobbyState == Closing` and were done in ~1.6s; the two whose RPC did not resolve sat at
/// `Joining` for 30.2s and 30.3s. That is the delay the player feels as the invasion item timing
/// out, and no cancel driven into Seamless can shorten it, because Seamless is not what is waiting.
///
/// Returning `false` from here without calling the original leaves the engine exactly as it was:
/// the caller only reaches this line when `lobbyState` is already one of `None`, `CreateFailed` or
/// `JoinFailed` (its `(0x25 >> lobbyState) & 1` guard), and no RPC has been issued yet, so there is
/// nothing outstanding to unwind.
pub const CS_SESSION_MANAGER_JOIN_SESSION: MapSeam = MapSeam {
    name: "CS::CSSessionManager::JoinSession",
    rva: 0x0ca_e640,
    prologue: &[0x88, 0x54, 0x24, 0x10, 0x57, 0x48, 0x83, 0xec, 0x40],
    arg_count: 4,
};

/// Offset of the destination block id within `ServerPushJoinData`.
pub const JOIN_DATA_DESTINATION_BLOCK_OFFSET: usize = 0x00;

/// `FUN_1407a04f0` -- the warp-job assembler. All five confirm routes funnel through it, which
/// makes it the single chokepoint before a MenuJob exists.
pub const WARP_JOB_ASSEMBLER: MapSeam = MapSeam {
    name: "warp job assembler (FUN_1407a04f0)",
    rva: 0x07a_04f0,
    prologue: &[
        0x40, 0x55, 0x53, 0x56, 0x57, 0x41, 0x54, 0x41, 0x56, 0x41, 0x57, 0x48,
    ],
    arg_count: 4,
};

/// `FUN_140d26390` -- `BonfireWarpTabParam` row lookup (param table index `0x2C`).
pub const BONFIRE_WARP_TAB_LOOKUP: MapSeam = MapSeam {
    name: "BonfireWarpTabParam lookup (FUN_140d26390)",
    rva: 0x0d2_6390,
    prologue: &[
        0x40, 0x57, 0x48, 0x83, 0xec, 0x40, 0x48, 0xc7, 0x44, 0x24, 0x20, 0xfe,
    ],
    arg_count: 2,
};

/// `FUN_140d26220` -- `BonfireWarpSubCategoryParam` row lookup (param table index `0x2D`).
pub const BONFIRE_WARP_SUBCATEGORY_LOOKUP: MapSeam = MapSeam {
    name: "BonfireWarpSubCategoryParam lookup (FUN_140d26220)",
    rva: 0x0d2_6220,
    prologue: &[
        0x40, 0x57, 0x48, 0x83, 0xec, 0x40, 0x48, 0xc7, 0x44, 0x24, 0x20, 0xfe,
    ],
    arg_count: 2,
};

/// `BonfireWarpParamLookup` -- `0x140d25c30` (param table index `0x2B`).
pub const BONFIRE_WARP_PARAM_LOOKUP: MapSeam = MapSeam {
    name: "BonfireWarpParamLookup",
    rva: 0x0d2_5c30,
    prologue: &[
        0x40, 0x57, 0x48, 0x83, 0xec, 0x40, 0x48, 0xc7, 0x44, 0x24, 0x20, 0xfe,
    ],
    arg_count: 2,
};

/// `CS::BonfireWarpParamLookupResult::GetBonfireEntityId` -- `0x140d25650`. Reads `param_row+0x08`
/// and answers `-1` as `0`.
pub const GET_BONFIRE_ENTITY_ID: MapSeam = MapSeam {
    name: "BonfireWarpParamLookupResult::GetBonfireEntityId",
    rva: 0x0d2_5650,
    prologue: &[
        0x48, 0x8b, 0x41, 0x08, 0x33, 0xc9, 0x48, 0x85, 0xc0, 0x74, 0x0f, 0x8b,
    ],
    arg_count: 2,
};

/// `FUN_140885ed0` -- the `CS::WorldMapWarpPinData` copy-constructor. The row owns two heap
/// regions, so this is the only safe way to duplicate one; a `memcpy` double-frees at teardown.
/// It carries no symbol in the 1.16.2 dump -- an earlier agent-supplied name for it was
/// fabricated and refuted.
pub const WORLDMAP_PIN_ROW_COPY_CTOR: MapSeam = MapSeam {
    name: "WorldMapWarpPinData copy-ctor (FUN_140885ed0)",
    rva: 0x088_5ed0,
    prologue: &[
        0x48, 0x89, 0x4c, 0x24, 0x08, 0x57, 0x48, 0x83, 0xec, 0x30, 0x48, 0xc7,
    ],
    arg_count: 2,
};

/// `FUN_14088b7b0` -- the `CS::WorldMapWarpPinData` constructor, called by the ViewModel ctor as
/// `(dst, &mapCoords, &bonfireLookupResult)`.
pub const WORLDMAP_PIN_ROW_CTOR: MapSeam = MapSeam {
    name: "WorldMapWarpPinData ctor (FUN_14088b7b0)",
    rva: 0x088_b7b0,
    prologue: &[
        0x40, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57,
    ],
    arg_count: 3,
};

/// `FUN_14088bac0` -- the `CS::WorldMapWarpPinData` non-deleting destructor. Destroys
/// `label[0..*(u64*)(row+0x230))` then the `+0x18` MenuString. Does not free the row storage.
/// The temp row used to build a pin must be destroyed with this; `free`/`operator delete`
/// instead corrupts the game heap.
pub const WORLDMAP_PIN_ROW_DTOR: MapSeam = MapSeam {
    name: "WorldMapWarpPinData dtor (FUN_14088bac0)",
    rva: 0x088_bac0,
    prologue: &[
        0x48, 0x89, 0x4c, 0x24, 0x08, 0x53, 0x48, 0x83, 0xec, 0x30, 0x48, 0xc7,
    ],
    arg_count: 1,
};

/// `FUN_140888aa0` -- the row list's reserve/grow helper.
pub const WORLDMAP_PIN_LIST_GROW: MapSeam = MapSeam {
    name: "WorldMapPinDataList grow (FUN_140888aa0)",
    rva: 0x088_8aa0,
    prologue: &[
        0x40, 0x57, 0x48, 0x83, 0xec, 0x20, 0x4c, 0x8b, 0xda, 0x4c, 0x8b, 0xd1,
    ],
    arg_count: 2,
};

/// `FUN_14088be50` -- the fast-travel list filter.
/// `(row /*RCX*/, categoryMask /*EDX*/, allowUnvisited /*R8B*/)` returns non-zero when the row
/// should be listed.
///
/// It is **not** the map-marker visibility gate, whatever this comment used to claim. All four
/// of its callers (`0x1409cef10`, `0x1408803b0`, `0x14088a6c0`, `0x14088aba0`) build the
/// fast-travel list or the bookmark dialog. A live run that saw the pins on the map while this
/// reported `ours 0/0` was reporting the truth; the earlier reading of that as a bug sent an
/// agent chasing a refuted experiment, which is why the correction is spelled out here.
///
/// The real marker gate is `WorldMapPinData::UpdateVisible` (`0x14087afa0`, pin vtable slot 3),
/// which writes the per-row draw flag `row+0xc` from four tests: `IsOpen`, the map-layer bit
/// (`(row+0x60 >> FUN_140887e90(mapId)) & 1`), an any-label-enabled test, and a zoom threshold.
pub const WORLDMAP_ROW_FILTER: MapSeam = MapSeam {
    name: "WorldMapWarpPinData row filter (FUN_14088be50)",
    rva: 0x088_be50,
    prologue: &[
        0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x6c, 0x24, 0x10, 0x48, 0x89,
    ],
    arg_count: 3,
};

/// `CS::WorldMapAreaConverter::ConvertMsbCoordsToMapCoords` -- `0x140876140`. Produces the
/// `WorldMapCoordinates{x, z}` a pin renders at.
///
/// The signature stops at 10 bytes on purpose. The eleventh byte begins the `disp32` of
/// `mov rax,[rip+disp32]`, and a RIP-relative displacement is the distance between the
/// instruction and the global it names -- so it re-encodes whenever either end moves, even when
/// the function is untouched. 1.17 moved both: this function translates cleanly to `0x140877130`
/// and its body is byte-identical there except that `disp32` (`62 4c 3e 03` -> `d2 7c 3e 03`).
/// The old 12-byte signature captured the low half of that field, so it matched nowhere in 1.17
/// and `verify_seam` -- which compares exactly, having no mask -- would have refused a correctly
/// translated address. That is the same failure the generated pins hit, where it was solved with
/// a `_MASK` that ignores exactly these bytes; a hand-written seam has no mask, so it stops short
/// of the field instead. This is the `take` idea from `build-support/prologue_build.rs`: name the
/// instructions in full, keep only the bytes that are stable.
///
/// Truncating costs nothing here, which is why this is the fix rather than a wider one: the
/// 10-byte prefix still occurs exactly once in each image (1.16.2 and 1.17 both n=1), measured by
/// `scripts/verify-prologue-coverage-1170.py --section uniqueness`. A shorter signature is only a
/// weaker signature when it actually matches more; this one does not.
pub const CONVERT_MSB_COORDS_TO_MAP_COORDS: MapSeam = MapSeam {
    name: "WorldMapAreaConverter::ConvertMsbCoordsToMapCoords",
    rva: 0x087_6140,
    prologue: &[0x40, 0x53, 0x57, 0x48, 0x83, 0xec, 0x68, 0x48, 0x8b, 0x05],
    arg_count: 3,
};

/// Every seam, for the startup self-check and the log banner.
pub const ALL_SEAMS: &[MapSeam] = &[
    WORLDMAP_VIEWMODEL_CTOR,
    WARP_JOB_ASSEMBLER,
    BONFIRE_WARP_TAB_LOOKUP,
    BONFIRE_WARP_SUBCATEGORY_LOOKUP,
    BONFIRE_WARP_PARAM_LOOKUP,
    GET_BONFIRE_ENTITY_ID,
    WORLDMAP_PIN_ROW_COPY_CTOR,
    WORLDMAP_PIN_ROW_CTOR,
    WORLDMAP_PIN_ROW_DTOR,
    WORLDMAP_PIN_LIST_GROW,
    WORLDMAP_ROW_FILTER,
    CONVERT_MSB_COORDS_TO_MAP_COORDS,
];

/// Why a seam could not be used. Every variant means "nothing was patched".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeamError {
    /// `GetModuleHandleA(NULL)` failed.
    ModuleBase(String),
    /// The prologue could not be read -- unmapped or protected.
    Unreadable { name: &'static str, address: usize },
    /// The running build moved this function and no verified mapping says where to.
    ///
    /// Separate from [`Self::PrologueMismatch`] on purpose, because the two need opposite fixes
    /// and this file reported the wrong one for the whole of the 1.17 migration. Every RVA here
    /// is 1.16.2; the installed game has been 1.17 since 2026-08-27. Byte-comparing at
    /// `base + rva` on that build reads unrelated code and answers "prologue mismatch", which
    /// blames the signature -- sending a reader to re-derive bytes that were never wrong -- when
    /// the address is what moved and the fix is to map the function.
    NoMappingForBuild {
        name: &'static str,
        /// The 1.16.2 address that was asked for.
        address: usize,
        /// `er_game_base::game_build::describe_build()` at the moment of the refusal.
        build: String,
    },
    /// The live bytes differ from the image these addresses were verified against.
    PrologueMismatch {
        name: &'static str,
        address: usize,
        expected: Vec<u8>,
        actual: Vec<u8>,
        /// `Some(stale)` when `address` is a translation of the 1.16.2 address `stale`, i.e. the
        /// build gate did find this function on the running build and its prologue still differs.
        /// That is a recompiled function, not a wrong address, and saying so keeps this message
        /// from being read as the refusal above.
        translated_from: Option<usize>,
    },
}

impl core::fmt::Display for SeamError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ModuleBase(detail) => write!(f, "game module base unavailable: {detail}"),
            Self::Unreadable { name, address } => {
                write!(f, "{name} @0x{address:x}: prologue unreadable")
            }
            Self::NoMappingForBuild {
                name,
                address,
                build,
            } => write!(
                f,
                "{name}: the 1.16.2 address 0x{address:x} has no verified detour mapping for the \
                 running build ({build}) -- refusing to hook. THE ADDRESS MOVED; the prologue \
                 signature is not in question and must not be \"fixed\""
            ),
            Self::PrologueMismatch {
                name,
                address,
                expected,
                actual,
                translated_from,
            } => match translated_from {
                Some(stale) => write!(
                    f,
                    "{name} @0x{address:x} (the verified translation of 1.16.2 0x{stale:x}): \
                     prologue mismatch (got {actual:02x?}, want {expected:02x?}) -- refusing to \
                     hook. The address resolved, so this is a RECOMPILED function, not a wrong \
                     address"
                ),
                None => write!(
                    f,
                    "{name} @0x{address:x}: prologue mismatch (got {actual:02x?}, want \
                     {expected:02x?}) -- refusing to hook; this is not the 1.16.2 image these \
                     addresses were verified against"
                ),
            },
        }
    }
}

impl std::error::Error for SeamError {}

/// How many times [`call_target`] may write its refusal line, per process.
///
/// # The refusal line is counted, and that is not fussiness
///
/// The only caller is `inject_pins`, which runs once per `CS::WorldMapViewModel` construction --
/// and that object's lifetime is `MoveMapStep`'s, so it is rebuilt on every world entry. An
/// unlatched refusal is therefore five identical lines every time the player loads in, for as
/// long as the session lasts, with no upper bound but playtime. That is precisely the shape
/// `scripts/check-no-rva-zero.py` exists to stop: one session wrote **339,764** copies of a
/// single refusal, and the volume is what made the real cause unfindable rather than the refusal
/// itself. The cap costs a reader nothing -- the first pass says everything the ten-thousandth
/// would -- and turns the log into a bounded artifact. [`ALL_SEAMS`] sizes it so that every seam
/// gets to speak once even if all of them move at the same time.
///
/// Not `#[cfg(windows)]`, unlike the counter and [`call_target`] themselves, so that the host
/// test run can assert the bound exists and is small. A cap that only compiles on the target it
/// protects is a cap nothing can regression-test.
pub const CALL_REFUSAL_LOG_LIMIT: usize = ALL_SEAMS.len();
#[cfg(windows)]
static CALL_REFUSAL_LOGS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// The address to call for `seam` on the running build, or `None` after saying why not.
///
/// # The call resolver, deliberately beside the detour one
///
/// [`verify_seam`] is for a seam about to carry a MinHook detour and uses `resolve_detour_address`.
/// This is for a seam about to be `transmute`d into a function pointer and called, and uses
/// `resolve_game_address`, which additionally accepts the whole-image `.pdata` pairing -- good
/// enough to say where a function is, not good enough to say MinHook may write five bytes there.
/// The two live together so the choice is a visible fork rather than a habit.
///
/// # Why it logs here
///
/// `er-game-base`'s own refusal goes to the sink `er_hook::set_hook_logger` installs, which is a
/// per-DLL static. This line is what makes a dead map surface readable in this DLL's log, beside
/// the injection tallies that will now read zero. It is capped by
/// [`CALL_REFUSAL_LOG_LIMIT`]; see that constant for why.
///
/// On 1.16.2 `resolve_game_address` returns its argument unchanged (`is_supported_build()`
/// short-circuits before the table is consulted), so this is a pass-through and nothing about the
/// shipped behaviour on that build changes.
#[cfg(windows)]
#[must_use]
pub fn call_target(base: usize, seam: &MapSeam) -> Option<usize> {
    let resolved = er_game_base::game_build::resolve_game_address(base + seam.rva, seam.name);
    if resolved.is_none()
        && CALL_REFUSAL_LOGS.fetch_add(1, core::sync::atomic::Ordering::SeqCst)
            < CALL_REFUSAL_LOG_LIMIT
    {
        crate::standalone_log(format_args!(
            "map-inject: REFUSED -- {} (1.16.2 0x{:x}) has NO verified address for the running \
             build ({}); it will NOT be called, so the map-pin surface is inert this session. The \
             ADDRESS moved -- map the function rather than removing the guard. This line is \
             logged at most {CALL_REFUSAL_LOG_LIMIT} times per process",
            seam.name,
            seam.verified_va(),
            er_game_base::game_build::describe_build(),
        ));
    }
    resolved
}

#[cfg(windows)]
/// Resolve a seam to a live address, refusing when the running build moved the function or when
/// the bytes there are not what we reversed.
///
/// # resolve first, byte-check second
///
/// The order is the fix. This used to byte-compare at `base + seam.rva` with no translation at
/// all, which on 1.17 reads whatever the patch put at a 1.16.2 address -- so every seam reported
/// [`SeamError::PrologueMismatch`], a message that blames the signature when the address is what
/// moved. The build gate is asked first now, and its refusal has its own variant.
///
/// # Why the detour resolver and not the call one
///
/// All four callers hand the result straight to `er_hook::register_union_hook`; every one of them
/// is installing a detour. `resolve_game_address` answers "where is this function now", which is
/// what a call needs, and it will happily return a pair carried by the whole-image `.pdata` map.
/// A detour needs the further claim that the destination is a real function entry with a
/// relocatable five-byte prologue, and only `resolve_detour_address` speaks to it -- letting the
/// weaker rows carry detours is what killed a boot on 2026-08-29. If a future caller wants one of
/// these seams as a plain call target, it must resolve with `resolve_game_address` itself rather
/// than loosening this.
///
/// # what it returns is the unresolved address, and that is the point (corrected 2026-08-30)
///
/// The byte check runs at the resolved address -- it has to, that is where the bytes are -- but
/// what comes back is `base + seam.rva`, untranslated, because `register_union_hook` resolves
/// again and must be the one resolve that decides where the detour lands.
///
/// This doc used to say the opposite: that resolving twice is idempotent for an address which is
/// already a 1.17 destination, so handing over the translated one is safe. That is true of most
/// addresses and false of exactly the ones that matter. An address can be both a 1.17 destination
/// of one row and the 1.16.2 source of a different row -- which happens whenever a region's shift
/// equals the local spacing between two functions, so `B - A == C - B`. On such an address
/// translation wins over the already-translated shortcut (it must; see `already_translated_in`),
/// and the second resolve silently returns a third, unrelated function. Three rows in the current
/// detour table have that shape (`0x6156c0`, `0x7ad710`, `0xbbbd90`), and on 2026-08-30 three live
/// detours in this workspace were measured landing on the wrong function because of it.
///
/// Resolving the same 1.16.2 input twice is harmless and is what happens now: this function
/// resolves it to read the prologue, `register_union_hook` resolves it to place the detour, and
/// both get the same answer. Resolving the output is the bug.
/// `scripts/check-double-resolved-hook-targets.py` is the gate that keeps it out.
///
/// On 1.16.2 `resolve_detour_address` returns its argument unchanged (`is_supported_build()`
/// short-circuits), so this whole step is a no-op there and the byte check runs at exactly the
/// address it always did.
///
/// # Safety
///
/// Reads live process memory through the repo's fault-tolerant primitive, so an unmapped
/// address yields [`SeamError::Unreadable`] rather than a fault.
pub unsafe fn verify_seam(seam: &MapSeam) -> Result<usize, SeamError> {
    let base = er_game_base::mem::game_module_base().map_err(SeamError::ModuleBase)?;
    let stale = base + seam.rva;
    let Some(address) =
        er_game_base::game_build::resolve_detour_address(base + seam.rva, seam.name)
    else {
        return Err(SeamError::NoMappingForBuild {
            name: seam.name,
            address: stale,
            build: er_game_base::game_build::describe_build(),
        });
    };
    let mut actual = vec![0_u8; seam.prologue.len()];
    if !unsafe { er_game_base::mem::read_bytes(address, &mut actual) } {
        return Err(SeamError::Unreadable {
            name: seam.name,
            address,
        });
    }
    if actual != seam.prologue {
        return Err(SeamError::PrologueMismatch {
            name: seam.name,
            address,
            expected: seam.prologue.to_vec(),
            actual,
            translated_from: (address != stale).then_some(stale),
        });
    }
    // `stale`, not `address`: see "WHAT IT RETURNS IS THE UNRESOLVED ADDRESS" above. The prologue
    // was verified at `address`; the caller hands `stale` to `register_union_hook`, which resolves
    // this same 1.16.2 input to that same `address` and owns the single resolve that places the
    // detour. Callers log what comes back, so their line now names the address the feature meant
    // and er-hook's own `HOOK TRANSLATED` line names where it went -- which is the pair a reader
    // needs, rather than one address twice.
    Ok(stale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_seam_carries_a_prologue_and_a_plausible_rva() {
        for seam in ALL_SEAMS {
            assert!(!seam.prologue.is_empty(), "{}", seam.name);
            // A signature shorter than the 14 bytes an absolute-jmp patch clobbers is still
            // useful for drift detection, but an empty or 1-byte one would match everything.
            assert!(
                seam.prologue.len() >= 8,
                "{} signature too short",
                seam.name
            );
            assert!(seam.rva > 0x1000, "{} rva looks like an offset", seam.name);
            assert!(seam.rva < 0x300_0000, "{} rva outside the image", seam.name);
        }
    }

    #[test]
    fn seam_rvas_reconstruct_the_byte_checked_vas() {
        // Guards against a transcription slip turning a verified VA into a crash-hook.
        for (seam, va) in [
            (WORLDMAP_VIEWMODEL_CTOR, 0x1_4088_55b0_usize),
            (WARP_JOB_ASSEMBLER, 0x1_407a_04f0),
            (BONFIRE_WARP_TAB_LOOKUP, 0x1_40d2_6390),
            (BONFIRE_WARP_SUBCATEGORY_LOOKUP, 0x1_40d2_6220),
            (BONFIRE_WARP_PARAM_LOOKUP, 0x1_40d2_5c30),
            (GET_BONFIRE_ENTITY_ID, 0x1_40d2_5650),
            (WORLDMAP_PIN_ROW_COPY_CTOR, 0x1_4088_5ed0),
            (WORLDMAP_PIN_ROW_CTOR, 0x1_4088_b7b0),
            (WORLDMAP_PIN_ROW_DTOR, 0x1_4088_bac0),
            (WORLDMAP_PIN_LIST_GROW, 0x1_4088_8aa0),
            (WORLDMAP_ROW_FILTER, 0x1_4088_be50),
            (CONVERT_MSB_COORDS_TO_MAP_COORDS, 0x1_4087_6140),
        ] {
            assert_eq!(seam.verified_va(), va, "{}", seam.name);
        }
    }

    #[test]
    fn no_seam_silently_exceeds_the_union_dispatchers_argument_count() {
        // A 5+ argument target riding the 4-argument union loses an out-parameter the callee
        // writes through -- memory corruption, not a clean failure. Anything that does not fit
        // must be given its own typed MhHook, and this test is where that decision surfaces.
        for seam in ALL_SEAMS {
            assert!(
                seam.fits_union_dispatcher(),
                "{} takes {} args and needs its own typed MhHook, not the union",
                seam.name,
                seam.arg_count
            );
        }
    }

    #[test]
    fn the_three_param_lookups_share_a_prologue_so_only_the_rva_tells_them_apart() {
        // Pinned deliberately: this is why a prologue signature must never be treated as
        // identifying which function was found. It detects drift, nothing more.
        assert_eq!(
            BONFIRE_WARP_TAB_LOOKUP.prologue,
            BONFIRE_WARP_SUBCATEGORY_LOOKUP.prologue
        );
        assert_eq!(
            BONFIRE_WARP_TAB_LOOKUP.prologue,
            BONFIRE_WARP_PARAM_LOOKUP.prologue
        );
        assert_ne!(
            BONFIRE_WARP_TAB_LOOKUP.rva,
            BONFIRE_WARP_SUBCATEGORY_LOOKUP.rva
        );
        assert_ne!(BONFIRE_WARP_TAB_LOOKUP.rva, BONFIRE_WARP_PARAM_LOOKUP.rva);
    }

    #[test]
    fn the_call_refusal_line_is_bounded_and_every_seam_still_gets_to_speak() {
        // The bound is the whole point: `call_target`'s only caller runs once per world entry,
        // so an unlatched refusal grows with playtime. 339,764 copies of one line is what that
        // looks like in practice. Asserting an upper bound here is what stops a later edit from
        // deleting the latch and re-creating it.
        // `const {}` rather than a runtime assert: both operands are compile-time constants, so
        // this becomes a build error rather than a test failure -- strictly stronger, and it is
        // what `clippy::assertions_on_constants` asks for. The lint only fires on the Windows
        // target (`cargo xwin clippy -p er-invasion-warp --all-targets`), because the module it
        // guards is `#[cfg(windows)]` and a host clippy run never compiles it.
        const {
            assert!(
                CALL_REFUSAL_LOG_LIMIT > 0,
                "a zero cap silences the refusal entirely, which is the opposite failure"
            );
            assert!(
                CALL_REFUSAL_LOG_LIMIT <= ALL_SEAMS.len(),
                "the cap must not exceed one refusal line per seam"
            );
        }
    }

    #[test]
    fn seam_names_are_unique_so_a_refusal_line_is_unambiguous() {
        for (index, seam) in ALL_SEAMS.iter().enumerate() {
            for other in &ALL_SEAMS[index + 1..] {
                assert_ne!(seam.name, other.name);
            }
        }
    }

    #[test]
    fn a_mismatch_error_shows_both_byte_strings_and_refuses_rather_than_claiming_success() {
        let error = SeamError::PrologueMismatch {
            name: "example",
            address: 0x1_4088_55b0,
            expected: vec![0x48, 0x8b],
            actual: vec![0xcc, 0xcc],
            translated_from: None,
        };
        let rendered = error.to_string();
        assert!(rendered.contains("refusing to hook"), "{rendered}");
        assert!(rendered.contains("cc"), "{rendered}");
        assert!(rendered.contains("48"), "{rendered}");
    }

    #[test]
    fn a_moved_address_is_not_reported_as_a_byte_mismatch() {
        // The defect this pair exists to stop coming back. `verify_seam` used to byte-compare at
        // the untranslated `base + rva`, so on 1.17 every seam came back "prologue mismatch" --
        // which reads as "our recorded bytes are wrong" and sends the next agent to re-derive a
        // signature that was never the problem. The two failures must not share wording.
        let moved = SeamError::NoMappingForBuild {
            name: "example",
            address: 0x1_4088_55b0,
            build: "game FileVersion 2.7.0.0 (this build supports 2.6.2.0)".to_owned(),
        }
        .to_string();
        assert!(moved.contains("no verified detour mapping"), "{moved}");
        assert!(moved.contains("THE ADDRESS MOVED"), "{moved}");
        assert!(moved.contains("2.7.0.0"), "{moved}");
        // The word a reader greps for when they think the signature is stale must not appear.
        assert!(!moved.contains("prologue mismatch"), "{moved}");

        // And the converse: a mismatch at an address that did resolve says so, so it is not read
        // as the refusal above.
        let recompiled = SeamError::PrologueMismatch {
            name: "example",
            address: 0x1_4088_65a0,
            expected: vec![0x48, 0x8b],
            actual: vec![0x40, 0x53],
            translated_from: Some(0x1_4088_55b0),
        }
        .to_string();
        assert!(recompiled.contains("prologue mismatch"), "{recompiled}");
        assert!(recompiled.contains("RECOMPILED"), "{recompiled}");
        assert!(recompiled.contains("1408855b0"), "{recompiled}");
    }

    #[test]
    fn every_seam_error_refuses_rather_than_reporting_a_partial_success() {
        // The enum's contract, asserted rather than left to the doc comment: no variant may render
        // as anything a reader could take for "patched anyway".
        for error in [
            SeamError::ModuleBase("no module".to_owned()),
            SeamError::Unreadable {
                name: "example",
                address: 0x1_4088_55b0,
            },
            SeamError::NoMappingForBuild {
                name: "example",
                address: 0x1_4088_55b0,
                build: "unreadable".to_owned(),
            },
            SeamError::PrologueMismatch {
                name: "example",
                address: 0x1_4088_55b0,
                expected: vec![0x48],
                actual: vec![0xcc],
                translated_from: None,
            },
        ] {
            let rendered = error.to_string();
            assert!(!rendered.is_empty(), "{error:?}");
            assert!(!rendered.contains("hooked"), "{rendered}");
        }
    }
}
