//! Runtime-derived Ash-of-War badge enablement for `data0:/menu/02_011_equip.gfx`.
//!
//! This does **not** ship a game-derived GFx file. The DLL reads the game's own
//! Scaleform MemoryFile for the equip menu, applies the structural edit below in
//! memory, and serves the derived movie for that process (bd er-effects-rs-pe98).
//!
//! WHY A SIBLING, NOT ArtsIcon: the equip tile (`DefineSprite 71`) already places a
//! bottom-left `ArtsIcon` container (`char 53`), but the game never instantiates its
//! subtree -- `ArtsIcon/IconImage` does not bind at runtime and `ArtsIcon`'s rect is
//! [0,0,0,0] (runtime rect trace 20260723-162855), so editing inside `ArtsIcon` is
//! futile. Instead we ADD our own child to the tile: a single-frame clip `ArtsBadge`
//! placed at `ArtsIcon`'s exact matrix (scale 0.65, bottom-left) whose sole content is
//! the same sized 160px placeholder shape (`char 43`) that makes `ItemIcon` render.
//! The badge DLL binds `ArtsBadge` and drives the ash icon into it with the game's own
//! icon setter, additive to the vanilla infusion/can't-wield/qty badges.

use crate::title_05_000::fnv1a64;
use crate::{GfxError, Movie, Tag};

/// Fresh (unused) character id for the injected single-frame badge clip. Max id in
/// vanilla `02_011` is 110; 250 is safely free (verified).
pub const BADGE_CLIP_ID: u16 = 250;
/// Instance name of the injected tile child the badge DLL binds and draws into.
pub const BADGE_INSTANCE_NAME: &str = "ArtsBadge";
/// The equip tile sprite that places `ItemIcon`/`AttributeIcon`/`ArtsIcon`.
const TILE_SPRITE_ID: u16 = 71;
/// `ItemIcon`'s `IconImage` clip; its frame-0 child is the 160px placeholder shape we reuse.
const ITEM_ICONIMAGE_SPRITE_ID: u16 = 44;
/// The 160px placeholder `DefineShape` (bounds_twips [0,3200,0,3200]).
const PLACEHOLDER_SHAPE_ID: u16 = 43;

/// Vanilla `02_011_equip.gfx` fingerprint (UXM-unpacked 1.16.1).
pub const VANILLA_LEN: usize = 18400;
pub const VANILLA_FNV1A64: u64 = 0xf043_fc28_76e5_4c54;
/// Edited length + fingerprint (self-consistency gate for the known vanilla input).
/// Derived and verified by `tests/equip_02_011.rs`.
pub const EDITED_LEN: usize = 18473;
pub const EDITED_FNV1A64: u64 = 0xdf8b_273d_509b_515e;

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
/// Adds a single-frame `ArtsBadge` clip child to the tile sprite. All-or-nothing: any
/// missing structure fails cleanly and the caller serves the untouched vanilla movie.
pub fn arts_badge(vanilla: &[u8]) -> Result<Vec<u8>, EquipBadgeError> {
    let mut movie = Movie::parse(vanilla).map_err(EquipBadgeError::Parse)?;

    // 1. Reuse ItemIcon/IconImage's frame-0 placement of the 160px placeholder shape as
    //    the badge clip's content (proven to give a real, stable rect).
    let placeholder_place = sprite_tags(&movie, ITEM_ICONIMAGE_SPRITE_ID)
        .and_then(|tags| {
            tags.iter()
                .find(|t| matches!(t, Tag::PlaceObject2 { character_id: Some(c), .. } if *c == PLACEHOLDER_SHAPE_ID))
                .cloned()
        })
        .ok_or(EquipBadgeError::Structure("char44 placeholder-shape placement"))?;

    // 2. Build a SINGLE-FRAME clip that always shows the placeholder shape (no free-run).
    let badge_clip = Tag::DefineSprite {
        id: BADGE_CLIP_ID,
        frame_count: 1,
        tags: vec![
            placeholder_place,
            Tag::ShowFrame { force_long: false },
            Tag::End,
        ],
        force_long: false,
    };

    // 3. Clone the tile's ArtsIcon placement (its bottom-left matrix is the intended badge
    //    slot) into a sibling named `ArtsBadge` that places the new clip at a fresh depth.
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
        character_id,
        name,
        depth,
        ..
    } = &mut badge_place
    {
        *character_id = Some(BADGE_CLIP_ID);
        *name = Some(BADGE_INSTANCE_NAME.to_owned());
        *depth = max_depth + 1;
    } else {
        return Err(EquipBadgeError::Structure(
            "ArtsIcon clone is not PlaceObject2",
        ));
    }

    // DIAGNOSTIC sibling: a second placement of an EXISTING dictionary clip (ItemIcon's
    // IconImage char 44) named "ZReachProbe". Comparing its runtime bind to ArtsBadge's
    // (which places the NEW char 250) isolates whether the swap fails to reach the parse
    // (both unbound) vs the new char id not being instantiated by this AS3 movie
    // (ZReachProbe binds, ArtsBadge does not).
    let mut probe_place = badge_place.clone();
    if let Tag::PlaceObject2 {
        character_id,
        name,
        depth,
        ..
    } = &mut probe_place
    {
        *character_id = Some(ITEM_ICONIMAGE_SPRITE_ID);
        *name = Some("ZReachProbe".to_owned());
        *depth = max_depth + 2;
    }

    // 4a. Insert the sibling placements right after the ArtsIcon placement in tile 71.
    if let Tag::DefineSprite { tags, .. } = &mut movie.tags[tile_idx] {
        tags.insert(arts_pos + 1, probe_place);
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
