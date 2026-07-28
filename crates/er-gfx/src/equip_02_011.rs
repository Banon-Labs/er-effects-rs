//! Runtime-derived Ash-of-War badge enablement for `data0:/menu/02_011_equip.gfx`.
//!
//! This does **not** ship a game-derived GFx file. The DLL reads the game's own
//! Scaleform MemoryFile for the equip menu, applies the structural edit below in
//! memory, and serves the derived movie for that process (bd er-effects-rs-pe98).
//!
//! WHY AUTO-REPLENISH: the equip tile (`DefineSprite 71`) already places a bottom-left
//! `ArtsIcon` container, but the game never instantiates its subtree on grid tiles.
//! The native tile binder already has a bottom-left `AutoReplenish/IconImage` slot for
//! the same visual real estate used by replenishable materials in storage/sort views.
//! Weapons are never autorefillable, so this edit adds that missing named subtree to
//! weapon tiles: `AutoReplenish` at `ArtsIcon`'s matrix, containing an `IconImage`
//! clip with a real 160px placeholder rect. The DLL then drives that native-style slot
//! only for weapon/armament tiles; vanilla autorefillable material tiles keep it.

use crate::title_05_000::fnv1a64;
use crate::{GfxError, Movie, PO2_HAS_CHARACTER, PO2_HAS_NAME, Tag};

/// Fresh (unused) character id for the injected AutoReplenish parent clip. Max id in
/// vanilla `02_011` is 110; 250 is safely free (verified).
pub const BADGE_CLIP_ID: u16 = 250;
/// Instance name of the injected tile child the badge DLL binds and draws into.
pub const BADGE_INSTANCE_NAME: &str = "AutoReplenish";
/// Nested instance name the native slot binder and icon setter target.
pub const BADGE_ICONIMAGE_INSTANCE_NAME: &str = "IconImage";
/// The equip tile sprite that places `ItemIcon`/`AttributeIcon`/`ArtsIcon`.
const TILE_SPRITE_ID: u16 = 71;
/// `ItemIcon`'s `IconImage` clip; its frame-0 child is the 160px placeholder shape we reuse.
const ITEM_ICONIMAGE_SPRITE_ID: u16 = 44;
/// The 160px placeholder `DefineShape` (bounds_twips [0,3200,0,3200]).
const PLACEHOLDER_SHAPE_ID: u16 = 43;

/// Vanilla `02_011_equip.gfx` fingerprint (UXM-unpacked 1.16.2).
pub const VANILLA_LEN: usize = 18393;
pub const VANILLA_FNV1A64: u64 = 0xf40f_9505_3a6e_f33c;
/// Edited length + fingerprint (self-consistency gate for the known vanilla input).
/// Derived and verified by `tests/equip_02_011.rs`.
pub const EDITED_LEN: usize = 18455;
pub const EDITED_FNV1A64: u64 = 0x6507_db92_c60d_6ffd;

pub fn is_known_vanilla(bytes: &[u8]) -> bool {
    bytes.len() == VANILLA_LEN && fnv1a64(bytes) == VANILLA_FNV1A64
}

#[derive(Clone, Debug)]
pub enum EquipBadgeError {
    Parse(GfxError),
    Write(GfxError),
    /// The vanilla movie did not have the structure the edit expects (a game update or a
    /// different asset): the named sprite/placement/shape was missing.
    Structure(&'static str),
    KnownInputBadOutput {
        out_len: usize,
        out_fnv1a64: u64,
    },
}

impl core::fmt::Display for EquipBadgeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EquipBadgeError::Parse(e) => write!(f, "parse: {e}"),
            EquipBadgeError::Write(e) => write!(f, "write: {e}"),
            EquipBadgeError::Structure(w) => write!(f, "unexpected movie structure: {w}"),
            EquipBadgeError::KnownInputBadOutput {
                out_len,
                out_fnv1a64,
            } => write!(
                f,
                "known vanilla input but output len={out_len} fnv=0x{out_fnv1a64:016x} != expected len={EDITED_LEN} fnv=0x{EDITED_FNV1A64:016x}"
            ),
        }
    }
}

impl std::error::Error for EquipBadgeError {}

/// Immutable ref to a top-level `DefineSprite`'s child tag stream.
fn sprite_tags<'m>(movie: &'m Movie, id: u16) -> Option<&'m Vec<Tag>> {
    movie.tags.iter().find_map(|t| match t {
        Tag::DefineSprite { id: sid, tags, .. } if *sid == id => Some(tags),
        _ => None,
    })
}

fn placement_named<'t>(tags: &'t [Tag], want: &str) -> Option<&'t Tag> {
    tags.iter()
        .find(|t| matches!(t, Tag::PlaceObject2 { name: Some(n), .. } if n == want))
}

/// Derive the badge-enabled equip movie from the game's own vanilla `02_011` payload.
/// Adds a single-frame `AutoReplenish/IconImage` subtree to the tile sprite. All-or-
/// nothing: any missing structure fails cleanly and the caller serves the untouched
/// vanilla movie.
pub fn arts_badge(vanilla: &[u8]) -> Result<Vec<u8>, EquipBadgeError> {
    let mut movie = Movie::parse(vanilla).map_err(EquipBadgeError::Parse)?;

    // 1. Reuse ItemIcon/IconImage's frame-0 placement transform as an identity-ish
    //    template for the nested IconImage child. We place the existing IconImage clip
    //    (char44) inside our parent so native `AutoReplenish/IconImage` binding has a
    //    real MovieClip with the same stable placeholder rect the icon setter expects.
    let mut iconimage_place = sprite_tags(&movie, ITEM_ICONIMAGE_SPRITE_ID)
        .and_then(|tags| {
            tags.iter()
                .find(|t| matches!(t, Tag::PlaceObject2 { character_id: Some(c), .. } if *c == PLACEHOLDER_SHAPE_ID))
                .cloned()
        })
        .ok_or(EquipBadgeError::Structure("char44 placeholder-shape placement"))?;
    if let Tag::PlaceObject2 {
        flags,
        character_id,
        name,
        depth,
        ..
    } = &mut iconimage_place
    {
        *character_id = Some(ITEM_ICONIMAGE_SPRITE_ID);
        *name = Some(BADGE_ICONIMAGE_INSTANCE_NAME.to_owned());
        *depth = 1;
        // The flags byte governs field presence on write; the vanilla char43
        // placement is 0x06 (no HasName), so the injected name is silently
        // dropped unless its bit is set here.
        *flags |= PO2_HAS_CHARACTER | PO2_HAS_NAME;
    } else {
        return Err(EquipBadgeError::Structure(
            "char44 placeholder placement is not PlaceObject2",
        ));
    }

    // 2. Build a SINGLE-FRAME AutoReplenish parent containing the IconImage child.
    let badge_clip = Tag::DefineSprite {
        id: BADGE_CLIP_ID,
        frame_count: 1,
        tags: vec![
            iconimage_place,
            Tag::ShowFrame { force_long: false },
            Tag::End,
        ],
        force_long: false,
    };

    // 3. Clone the tile's ArtsIcon placement (its bottom-left matrix is the intended badge
    //    slot) into a sibling named `AutoReplenish` that places the new clip at a fresh depth.
    let tile_idx = movie
        .tags
        .iter()
        .position(|t| matches!(t, Tag::DefineSprite { id, .. } if *id == TILE_SPRITE_ID))
        .ok_or(EquipBadgeError::Structure("tile DefineSprite 71"))?;

    let (mut badge_place, arts_pos, max_depth) = {
        let Tag::DefineSprite { tags, .. } = &movie.tags[tile_idx] else {
            return Err(EquipBadgeError::Structure("tile is not a DefineSprite"));
        };
        let arts_pos = tags
            .iter()
            .position(|t| matches!(t, Tag::PlaceObject2 { name: Some(n), .. } if n == "ArtsIcon"))
            .ok_or(EquipBadgeError::Structure("ArtsIcon placement in tile 71"))?;
        let max_depth = tags
            .iter()
            .filter_map(|t| match t {
                Tag::PlaceObject2 { depth, .. } => Some(*depth),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        (
            placement_named(tags, "ArtsIcon")
                .cloned()
                .ok_or(EquipBadgeError::Structure("ArtsIcon placement clone"))?,
            arts_pos,
            max_depth,
        )
    };
    if let Tag::PlaceObject2 {
        flags,
        character_id,
        name,
        depth,
        ..
    } = &mut badge_place
    {
        *character_id = Some(BADGE_CLIP_ID);
        *name = Some(BADGE_INSTANCE_NAME.to_owned());
        *depth = max_depth + 1;
        *flags |= PO2_HAS_CHARACTER | PO2_HAS_NAME;
    } else {
        return Err(EquipBadgeError::Structure(
            "ArtsIcon clone is not PlaceObject2",
        ));
    }

    // 4a. Insert the sibling placement right after the ArtsIcon placement in tile 71.
    if let Tag::DefineSprite { tags, .. } = &mut movie.tags[tile_idx] {
        tags.insert(arts_pos + 1, badge_place);
    }
    // 4b. Define the new clip before the tile that places it (dictionary order).
    movie.tags.insert(tile_idx, badge_clip);

    let out = movie.write().map_err(EquipBadgeError::Write)?;
    if is_known_vanilla(vanilla)
        && EDITED_LEN != 0
        && EDITED_FNV1A64 != 0
        && (out.len() != EDITED_LEN || fnv1a64(&out) != EDITED_FNV1A64)
    {
        return Err(EquipBadgeError::KnownInputBadOutput {
            out_len: out.len(),
            out_fnv1a64: fnv1a64(&out),
        });
    }
    Ok(out)
}
