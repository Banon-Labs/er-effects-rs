//! The world-map seams this DLL detours, and the guard that refuses to hook a wrong build.
//!
//! # Two independent checks, and why both exist
//!
//! Every address here was byte-checked OFFLINE against `eldenring-deobf.bin` at shift 0
//! (`python3 scripts/check-dump-deobf-identity.py --count 32 0x<va>`). That proves the address
//! is right *for the image we reverse-engineered*. It says nothing about the image actually
//! running, which is why each seam also carries the first bytes of its prologue and
//! [`verify_seam`] re-reads them from live memory before anything is patched. A patch applied
//! to a differently-built game lands mid-instruction and crashes; refusing to hook merely costs
//! the feature.
//!
//! # What a prologue signature does and does not catch
//!
//! It catches DRIFT -- a different game build where the function moved or changed. It does NOT
//! identify which function this is: the three `BonfireWarp*` param lookups share a byte-identical
//! 12-byte prologue (`40 57 48 83 ec 40 48 c7 44 24 20 fe`) because they are the same
//! binary-search shape over different param tables. Only the RVA distinguishes them, and the RVA
//! is only as good as the offline byte-check that produced it. Do not "fix" a mismatch by
//! relaxing the signature.
//!
//! # Argument-count trap
//!
//! `er_hook`'s union dispatcher is a FOUR-argument `extern "system"` shape. A target taking five
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
    /// Register/stack argument count. `> 4` means this seam MUST NOT ride the four-argument
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
/// populated here and NOWHERE else, so an epilogue hook here is the injection seam for synthetic
/// rows.
///
/// LIFETIME, corrected twice and now pinned by the static call graph: the ViewModel is built ONCE
/// PER WORLD ENTRY -- not once per session, and not once per map view. This ctor's only code xref
/// is `FUN_1407ed840 @0x1407ed8d3`, whose body is `if (popupMenu->worldMapViewModel == NULL) {
/// alloc(0x450, MenuHeap); ctor(...) }`; its only caller is `FUN_140766010 @0x14076607b`, whose
/// only caller is `MoveMapStep`'s constructor `@0x140af2e47`, reached from `STEP_MoveMap_Init`.
/// Teardown mirrors it: `~MoveMapStep -> FUN_140765fa0 @0x140af3eb5 -> FUN_1407ed790`, which runs
/// the dtor, frees the block and nulls the slot. So the object's lifetime IS `MoveMapStep`'s.
///
/// Two consequences that have each cost a wrong diagnosis:
/// - A map-LAYER switch (`FUN_1409c1fc0`) does not come near this function -- it mutates
///   `dialog+0xa88` and re-sizes the clip pool. `VIEWMODEL_CTOR_HITS > 1` means the player MOVED
///   MAPS, never that they toggled a layer.
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
/// This is the seam the location filter judges at: the destination is DECIDED and the player has
/// NOT moved. Measured live 2026-08-05 -- `ServerPushJoinData+0x00` was the only offset in all 128
/// bytes whose u32 equalled the destination that landed in `GameMan+0xAC8`, so the field is
/// identified by correlation rather than by its reversed name (`matchPlayerCount`, which would
/// have pointed at the wrong dword).
pub const SET_MULTIPLAY_JOIN_DATA: MapSeam = MapSeam {
    name: "CS::SosSignMan::SetMultiplayJoinData",
    rva: 0x06f_b520,
    prologue: &[0x40, 0x53, 0x48, 0x81, 0xec, 0x80, 0x00, 0x00, 0x00],
    arg_count: 2,
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

/// `FUN_140885ed0` -- the `CS::WorldMapWarpPinData` COPY-constructor. The row owns two heap
/// regions, so this is the only safe way to duplicate one; a `memcpy` double-frees at teardown.
/// It carries NO symbol in the 1.16.2 dump -- an earlier agent-supplied name for it was
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
/// `label[0..*(u64*)(row+0x230))` then the `+0x18` MenuString. Does NOT free the row storage.
/// The temp row used to build a pin MUST be destroyed with this; `free`/`operator delete`
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

/// `FUN_14088be50` -- the FAST-TRAVEL LIST filter.
/// `(row /*RCX*/, categoryMask /*EDX*/, allowUnvisited /*R8B*/)` returns non-zero when the row
/// should be listed.
///
/// It is **NOT** the map-marker visibility gate, whatever this comment used to claim. All four
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
pub const CONVERT_MSB_COORDS_TO_MAP_COORDS: MapSeam = MapSeam {
    name: "WorldMapAreaConverter::ConvertMsbCoordsToMapCoords",
    rva: 0x087_6140,
    prologue: &[
        0x40, 0x53, 0x57, 0x48, 0x83, 0xec, 0x68, 0x48, 0x8b, 0x05, 0x62, 0x4c,
    ],
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
    /// The live bytes differ from the image these addresses were verified against.
    PrologueMismatch {
        name: &'static str,
        address: usize,
        expected: Vec<u8>,
        actual: Vec<u8>,
    },
}

impl core::fmt::Display for SeamError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ModuleBase(detail) => write!(f, "game module base unavailable: {detail}"),
            Self::Unreadable { name, address } => {
                write!(f, "{name} @0x{address:x}: prologue unreadable")
            }
            Self::PrologueMismatch {
                name,
                address,
                expected,
                actual,
            } => write!(
                f,
                "{name} @0x{address:x}: prologue mismatch (got {actual:02x?}, want {expected:02x?}) \
                 -- refusing to hook; this is not the 1.16.2 image these addresses were verified \
                 against"
            ),
        }
    }
}

impl std::error::Error for SeamError {}

#[cfg(windows)]
/// Resolve a seam to a live address, refusing when the bytes there are not what we reversed.
///
/// # Safety
///
/// Reads live process memory through the repo's fault-tolerant primitive, so an unmapped
/// address yields [`SeamError::Unreadable`] rather than a fault.
pub unsafe fn verify_seam(seam: &MapSeam) -> Result<usize, SeamError> {
    let base = er_game_base::mem::game_module_base().map_err(SeamError::ModuleBase)?;
    let address = base + seam.rva;
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
        });
    }
    Ok(address)
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
        // identifying WHICH function was found. It detects drift, nothing more.
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
        };
        let rendered = error.to_string();
        assert!(rendered.contains("refusing to hook"), "{rendered}");
        assert!(rendered.contains("cc"), "{rendered}");
        assert!(rendered.contains("48"), "{rendered}");
    }
}
