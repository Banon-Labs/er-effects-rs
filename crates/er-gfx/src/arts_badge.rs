//! Runtime-derived Ash-of-War badge enablement for the game's item-tile menus.
//!
//! This does **not** ship a game-derived GFx file. The DLL reads the game's own Scaleform
//! MemoryFile for each menu movie, applies the structural edit below in memory, and serves
//! the derived movie for that process (bd er-effects-rs-pe98 / er-effects-rs-jogu).
//!
//! # Which movies
//!
//! [`TARGETS`] lists every movie whose grid/slot tile carries the dormant `ArtsIcon` child:
//! the equip menu (`02_011_equip`, Right/Left Hand Armament slots), the inventory
//! (`02_020_inventory`, melee armaments / ranged weapons / shields tabs) and the sort chest
//! (`03_050_itembox`, the same tabs). The native side needs NO new hook: the tile-populate
//! function the DLL already hooks is shared -- run 20260727-224115 shows it firing on
//! inventory tiles (`inadequacy`/`StockNum` children) as well as equip tiles, and those
//! tiles were skipped only because their `ArtsIcon` was still the dormant vanilla stub.
//!
//! # WHY ArtsIcon
//!
//! Every one of those tiles already places `ArtsIcon` at (-32, +37) -- the BOTTOM-LEFT slot
//! (`AttributeIcon` is bottom-right, `ItemIcon` is centred) -- and it is the game's own
//! Ash-of-War slot, so driving it shows the AoW where the game already intended to.
//!
//! WHY NOT a new child: injecting one does not work. With the swap proven to land, an
//! injected `AutoReplenish` stayed unbound on every tile, because Scaleform instantiates a
//! named timeline child only where the parent's AS3 class declares a matching member. This
//! edit adds no name -- `ArtsIcon` is vanilla and already declared -- so that gate never
//! applies.
//!
//! `ArtsIcon` renders nothing in vanilla only because it points at a stub: a 4-frame
//! animation with no stable placeholder rect in `02_011`, and a completely EMPTY sprite in
//! `02_020`/`03_050`. The edit re-points that single character reference at a clip built to
//! be structurally identical to the icon slot the game already draws into.
//!
//! # Everything positional is read from the asset
//!
//! No per-movie constants: the tile sprite, the icon cell size, the placeholder shape and
//! the badge's position/scale are all derived from the movie being edited, so one code path
//! serves all three (and tracks the vanilla layout if an asset ever shifts).

use crate::{
    GfxError, Matrix, Movie, PO2_HAS_CHARACTER, PO2_HAS_MATRIX, PO2_HAS_NAME, PO3_HAS_VISIBLE,
    Rect, Tag,
};
use er_game_base::fnv1a::fnv1a64;

/// Instance name of the tile child the badge DLL binds and draws into. This is a VANILLA
/// child, not an injected one.
pub const BADGE_INSTANCE_NAME: &str = "ArtsIcon";
/// Nested instance name the native slot binder and icon setter target. `SetIcon` recurses
/// into this child and scales the drawn quad by ITS local rect.
pub const BADGE_ICONIMAGE_INSTANCE_NAME: &str = "IconImage";
/// The centred item icon. Its container is the structural template for the badge clip, and
/// its placeholder shape supplies the icon's native atlas cell size.
const ITEM_INSTANCE_NAME: &str = "ItemIcon";
/// The vanilla bottom-RIGHT corner badge (infusion/affinity). Its placement is the
/// authoritative reference the Ash-of-War badge mirrors: same scale and vertical offset,
/// reflected about the tile centre to land bottom-LEFT.
const ATTRIBUTE_INSTANCE_NAME: &str = "AttributeIcon";

/// Rendered size of the badge, in tile-local px before the mirrored placement scale -- what
/// `AttributeIcon/IconImage` measures at runtime, so the Ash-of-War badge matches the corner
/// badge the game already draws on these tiles.
const BADGE_RENDER_PX: f32 = 37.0;

/// The Ash-of-War backing plate: a `GFX_DefineExternalImage2` resolved by NAME out of the
/// shared `01_common` atlas (`SB_FE_01.layout` -> `MENU_FL_Arts_waku.png`, 182x184).
///
/// This is the game's own frame -- `01_000_fe.gfx` declares it as character 232 and draws it
/// UNDER its Ash-of-War icon (sprite 450: plate at depth 1, `BaseIcon` at depth 5), which is
/// why the same plate appears everywhere the game shows an ash. Name resolution is global,
/// not per-movie: `MENU_FL_Arts_waku2` is declared with different character ids in eight
/// different movies, and the movies we edit already resolve names from several different
/// atlas sheets inside the same `01_common.tpf`.
///
/// NOT a `DefineShape`: the vanilla placeholder shapes are flat placeholder FILLS, so using
/// one as a plate rendered a solid green square.
const PLATE_IMAGE_NAME: &str = "MENU_FL_Arts_waku";
/// Bitmap format word the vanilla external-image tags use for these atlas sprites.
const PLATE_IMAGE_FORMAT: u16 = 13;
/// Native size of `MENU_FL_Arts_waku`, from `SB_FE_01.layout` and the vanilla tag in
/// `01_000_fe.gfx`. The plate is rendered at the badge's box, matching the game's own
/// composition, where the plate quad (91x92) and the icon quad (92x92) are concentric.
const PLATE_NATIVE_W: f32 = 182.0;
const PLATE_NATIVE_H: u16 = 184;
/// `GFX_DefineExternalImage2` tag code.
const GFX_DEFINE_EXTERNAL_IMAGE2: u16 = 1009;

/// Depth of the plate inside the badge clip. Lower depth = further back, so the plate sits
/// behind the icon exactly as it does in the game's own HUD composition.
const PLATE_DEPTH: u16 = 1;
/// Depth of the icon child inside the badge clip.
const ICON_DEPTH: u16 = 2;

/// A menu movie whose grid/slot tile gets the badge.
#[derive(Clone, Copy, Debug)]
pub struct BadgeTarget {
    /// Corpus file name (also the fragment matched on the loader's open URL).
    pub file_name: &'static str,
    /// URL fragment identifying this movie on `FileOpener::OpenFile`.
    pub url_needle: &'static [u8],
    pub vanilla_len: usize,
    pub vanilla_fnv1a64: u64,
    /// Edited length + fingerprint (self-consistency gate for the known vanilla input).
    /// Derived and verified by `tests/arts_badge.rs`.
    pub edited_len: usize,
    pub edited_fnv1a64: u64,
    /// Extra named children a tile must ALSO place to be badge-able in this movie.
    ///
    /// Empty for the menu movies, where every `ItemIcon`+`AttributeIcon` tile is a real item
    /// slot. The HUD movie needs it: `01_000_fe` holds the quick-slot strip AND both
    /// item-acquisition banners, and the banners are populated by a path we do not hook -- a
    /// badge there would be a bare plate nothing ever fills. The quick-slots are the only
    /// tiles that also place `Dish` (their round backing), so that is the discriminator.
    ///
    /// Structural on purpose: a character-id allow-list would be wrong the moment a user's
    /// menu mod renumbers the movie, which is the whole case [`derive_unknown`] exists for.
    pub require_children: &'static [&'static str],
    /// Emit the injected (nested) badge placement with `visible = 0`.
    ///
    /// ONLY the HUD movie. There, every quick-slot shares one `ItemIcon` container, so the
    /// badge necessarily exists on slots nothing populates and must not show its un-set
    /// placeholder (run 20260728-082544: green squares on the quick-item previews, which no
    /// hook of ours binds and therefore cannot hide natively).
    ///
    /// The menus must stay VISIBLE-by-default: `02_010_equiptop` is the one menu using the
    /// nested mount, and making it hidden removed its badges outright while the three
    /// re-pointed menus kept theirs (reported 2026-07-28). Their populate path shows and hides
    /// per tile already, so they need no help and tolerate no interference.
    pub default_hidden: bool,
}

/// Every movie the badge is applied to. Fingerprints are UXM-unpacked 1.16.2.
///
/// A `static`, deliberately: a `const` slice is materialized separately at EVERY use site,
/// so callers in different crates see different addresses for "the same" entry. Identifying
/// a target by pointer across that boundary silently failed (run 20260727-231706: the
/// per-movie derived-movie cache keyed on `ptr::eq` never matched, fell back to slot 0, and
/// served the inventory movie's bytes for the equip AND sort-chest movies). The lookups
/// below therefore return an INDEX, and nothing depends on pointer identity.
pub static TARGETS: [BadgeTarget; 5] = [
    // The in-game HUD armament quick-slot strip. Its tiles are AS3 class-bound and place no
    // `ArtsIcon`, so the badge nests in the `ItemIcon` container they all share. Scoped by
    // `Dish` because this movie ALSO holds both item-acquisition banners, which no hook of
    // ours populates -- a badge there would be a plate that never fills. Populated by the
    // dedicated HUD hooks in `er-armament-icons::hud_badge`, NOT by the menu tile-populate.
    BadgeTarget {
        file_name: "01_000_fe.gfx",
        url_needle: b"01_000_fe",
        vanilla_len: 1634757,
        vanilla_fnv1a64: 0x71f7_ecb9_79e7_8040,
        edited_len: 1634920,
        edited_fnv1a64: 0x71d1_337b_b6ef_fde5,
        require_children: &["Dish"],
        default_hidden: true,
    },
    // The Equipment screen's loadout grid. Its tile (sprite 59) places NO `ArtsIcon`, so the
    // badge is nested inside the `ItemIcon` container instead -- see `BadgeMount`.
    BadgeTarget {
        file_name: "02_010_equiptop.gfx",
        url_needle: b"02_010_equiptop",
        vanilla_len: 15601,
        vanilla_fnv1a64: 0xc454_e159_e4cd_63a2,
        edited_len: 15758,
        edited_fnv1a64: 0x73da_027e_dafb_b70d,
        require_children: &[],
        default_hidden: false,
    },
    BadgeTarget {
        file_name: "02_011_equip.gfx",
        url_needle: b"02_011_equip",
        vanilla_len: 18393,
        vanilla_fnv1a64: 0xf40f_9505_3a6e_f33c,
        edited_len: 18525,
        edited_fnv1a64: 0xea36_0352_1b69_3519,
        require_children: &[],
        default_hidden: false,
    },
    BadgeTarget {
        file_name: "02_020_inventory.gfx",
        url_needle: b"02_020_inventory",
        vanilla_len: 35218,
        vanilla_fnv1a64: 0x9f3a_47d3_38bd_2ea0,
        edited_len: 35350,
        edited_fnv1a64: 0xc3fd_2a6e_fa59_2b5a,
        require_children: &[],
        default_hidden: false,
    },
    BadgeTarget {
        file_name: "03_050_itembox.gfx",
        url_needle: b"03_050_itembox",
        vanilla_len: 79267,
        vanilla_fnv1a64: 0x17a0_fa52_78fb_400f,
        edited_len: 79399,
        edited_fnv1a64: 0x2b6b_e60e_9d1f_1dc2,
        require_children: &[],
        default_hidden: false,
    },
];

/// The target whose vanilla movie is exactly `bytes`, as `(index, target)`. The index is
/// what callers must key per-movie state on -- see [`TARGETS`].
pub fn target_for_vanilla(bytes: &[u8]) -> Option<(usize, &'static BadgeTarget)> {
    let fnv = fnv1a64(bytes);
    TARGETS
        .iter()
        .position(|t| t.vanilla_len == bytes.len() && t.vanilla_fnv1a64 == fnv)
        .map(|i| (i, &TARGETS[i]))
}

/// The target whose vanilla movie declares `len` bytes in its GFX header, as
/// `(index, target)`. The cheap pre-filter before a candidate File's bytes are read in full.
pub fn target_for_declared_len(len: usize) -> Option<(usize, &'static BadgeTarget)> {
    TARGETS
        .iter()
        .position(|t| t.vanilla_len == len)
        .map(|i| (i, &TARGETS[i]))
}

#[derive(Clone, Debug)]
pub enum BadgeError {
    Parse(GfxError),
    Write(GfxError),
    /// The vanilla movie did not have the structure the edit expects (a game update or a
    /// different asset): the named sprite/placement/shape was missing.
    Structure(&'static str),
    KnownInputBadOutput {
        file_name: &'static str,
        out_len: usize,
        out_fnv1a64: u64,
        want_len: usize,
        want_fnv1a64: u64,
    },
    /// We could not reproduce the INPUT movie byte-for-byte, so we do not model every tag it
    /// contains and must not re-serialise it. Only reachable on the unknown-input path: a
    /// movie some other mod supplied through ME3 that we have no baked fingerprint for.
    NotReproducible {
        in_len: usize,
        out_len: usize,
        first_diff: Option<usize>,
    },
    /// The edit changed something it was never supposed to touch.
    NotAdditive(&'static str),
}

impl core::fmt::Display for BadgeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BadgeError::Parse(e) => write!(f, "parse: {e}"),
            BadgeError::Write(e) => write!(f, "write: {e}"),
            BadgeError::Structure(w) => write!(f, "unexpected movie structure: {w}"),
            BadgeError::KnownInputBadOutput {
                file_name,
                out_len,
                out_fnv1a64,
                want_len,
                want_fnv1a64,
            } => write!(
                f,
                "{file_name}: known vanilla input but output len={out_len} \
                 fnv=0x{out_fnv1a64:016x} != expected len={want_len} fnv=0x{want_fnv1a64:016x}"
            ),
            BadgeError::NotReproducible {
                in_len,
                out_len,
                first_diff,
            } => write!(
                f,
                "cannot reproduce input byte-for-byte (in={in_len} out={out_len} \
                 first_diff={first_diff:?}); refusing to re-serialise a movie we do not \
                 fully model"
            ),
            BadgeError::NotAdditive(w) => write!(f, "edit was not additive: {w}"),
        }
    }
}

impl std::error::Error for BadgeError {}

/// Immutable ref to a top-level `DefineSprite`'s child tag stream.
fn sprite_tags(movie: &Movie, id: u16) -> Option<&Vec<Tag>> {
    movie.tags.iter().find_map(|t| match t {
        Tag::DefineSprite { id: sid, tags, .. } if *sid == id => Some(tags),
        _ => None,
    })
}

fn placement_named<'t>(tags: &'t [Tag], want: &str) -> Option<&'t Tag> {
    tags.iter()
        .find(|t| matches!(t, Tag::PlaceObject2 { name: Some(n), .. } if n == want))
}

fn placement_char(tags: &[Tag], want: &str) -> Option<u16> {
    match placement_named(tags, want) {
        Some(Tag::PlaceObject2 {
            character_id: Some(c),
            ..
        }) => Some(*c),
        _ => None,
    }
}

/// Read a placement's uniform scale and translation, in (scale, x_px, y_px).
fn placement_transform(tag: &Tag) -> Option<(f32, f32, f32)> {
    let Tag::PlaceObject2 { matrix, .. } = tag else {
        return None;
    };
    let m = matrix.as_ref()?;
    let scale = if m.has_scale {
        m.scale_x as f32 / 65536.0
    } else {
        1.0
    };
    Some((
        scale,
        m.translate_x as f32 / 20.0,
        m.translate_y as f32 / 20.0,
    ))
}

/// A `MATRIX` with a uniform scale and a translation given in PIXELS.
///
/// The badge's placements are built explicitly rather than cloned from a vanilla one.
/// Cloning carries the SOURCE's transform, which is authored for the SOURCE's content:
/// the item `IconImage` clip is placed at (-80, -80) to centre the 160px placeholder, so
/// reusing that placement for the badge threw it off the tile entirely (run
/// 20260727-220127: `ArtsIcon_post` local `[-84,-15]` instead of `[-32,37]`).
fn placed_matrix(scale: f32, x_px: f32, y_px: f32) -> Matrix {
    // SWF scale fields are 16.16 fixed point; translation is in twips (1px = 20).
    let fixed = (scale * 65536.0) as i32;
    let (tx, ty) = ((x_px * 20.0) as i32, (y_px * 20.0) as i32);
    let translate_nbits = if tx == 0 && ty == 0 { 0 } else { 24 };
    Matrix {
        has_scale: true,
        // Width the value can round-trip through; 24 bits covers the practical range with
        // room to spare (the codec reproduces whatever width is written here).
        scale_nbits: 24,
        scale_x: fixed,
        scale_y: fixed,
        has_rotate: false,
        rotate_nbits: 0,
        rotate_skew0: 0,
        rotate_skew1: 0,
        translate_nbits,
        translate_x: tx,
        translate_y: ty,
    }
}

/// A `PlaceObject2` placing `character_id` at `depth`, optionally named, with `scale` and
/// the given pixel translation.
fn place(character_id: u16, depth: u16, name: Option<&str>, scale: f32, x: f32, y: f32) -> Tag {
    let mut flags = PO2_HAS_CHARACTER | PO2_HAS_MATRIX;
    if name.is_some() {
        flags |= PO2_HAS_NAME;
    }
    Tag::PlaceObject2 {
        flags,
        depth,
        character_id: Some(character_id),
        matrix: Some(placed_matrix(scale, x, y)),
        color_transform: None,
        ratio: None,
        name: name.map(str::to_owned),
        clip_depth: None,
        force_long: false,
    }
}

/// The same placement as [`place`], but emitted as `PlaceObject3` with `visible = 0`.
///
/// Used for every INJECTED (nested) badge, because a badge that nothing populates must be
/// invisible rather than showing its un-set placeholder. The HUD movie makes this mandatory:
/// sprite 343 is the `ItemIcon` container for the left weapon, the right weapon, the quick-item
/// slot AND its two small cycle previews (see `tests/hud_tree_probe.rs`), so injecting there
/// necessarily gives all of them a badge while only the two weapon slots are ever populated.
///
/// Hiding from the NATIVE side cannot cover them: it only reaches clips something binds, and the
/// cycle previews are bound by a path that resolves `ItemIcon/IconImage` alone -- which is why
/// run 20260728-082544 left green placeholder squares on exactly those two tiles and nowhere
/// else. Default-hidden in the movie needs no binder at all: the slot stays invisible until code
/// explicitly shows it.
///
/// Safe for the menus too -- their draw path already calls `SetVisible(true)` before drawing
/// and `SetVisible(false)` on every non-draw path.
fn place_hidden(
    character_id: u16,
    depth: u16,
    name: Option<&str>,
    scale: f32,
    x: f32,
    y: f32,
) -> Tag {
    let mut flags1 = PO2_HAS_CHARACTER | PO2_HAS_MATRIX;
    if name.is_some() {
        flags1 |= PO2_HAS_NAME;
    }
    Tag::PlaceObject3 {
        flags1,
        flags2: PO3_HAS_VISIBLE,
        depth,
        class_name: None,
        character_id: Some(character_id),
        matrix: Some(placed_matrix(scale, x, y)),
        color_transform: None,
        ratio: None,
        name: name.map(str::to_owned),
        clip_depth: None,
        filters: None,
        blend_mode: None,
        bitmap_cache: None,
        // (visible = 0, background RGBA unused when hidden)
        visible: Some((0, [0, 0, 0, 0])),
        force_long: false,
    }
}

/// `GFX_DefineExternalImage2` body: `characterId u32`, `bitmapFormat u16`,
/// `targetWidth u16`, `targetHeight u16`, then length-prefixed export and file names.
/// Byte layout read off the vanilla tags in `01_000_fe.gfx`.
fn external_image_tag(character_id: u16, name: &str, w: u16, h: u16) -> Tag {
    let file = format!("{name}.tga");
    let mut raw = Vec::with_capacity(10 + 2 + name.len() + file.len());
    raw.extend_from_slice(&(character_id as u32).to_le_bytes());
    raw.extend_from_slice(&PLATE_IMAGE_FORMAT.to_le_bytes());
    raw.extend_from_slice(&w.to_le_bytes());
    raw.extend_from_slice(&h.to_le_bytes());
    raw.push(name.len() as u8);
    raw.extend_from_slice(name.as_bytes());
    raw.push(file.len() as u8);
    raw.extend_from_slice(file.as_bytes());
    Tag::Unknown {
        code: GFX_DEFINE_EXTERNAL_IMAGE2,
        raw,
        force_long: false,
    }
}

fn rect_size_px(r: &Rect) -> (f32, f32) {
    (
        (r.x_max - r.x_min) as f32 / 20.0,
        (r.y_max - r.y_min) as f32 / 20.0,
    )
}

fn rect_origin_px(r: &Rect) -> (f32, f32) {
    (r.x_min as f32 / 20.0, r.y_min as f32 / 20.0)
}

/// How the badge clip is attached to a tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BadgeMount {
    /// The tile has the game's own dormant `ArtsIcon` child: re-point that placement. The
    /// name is already declared by the tile's AS3 class, so it is already instantiated.
    RepointArtsIcon,
    /// The tile has NO arts slot -- the equipment loadout grid (`02_010_equiptop` sprite 59)
    /// places only `ItemIcon`/`AttributeIcon`/`inadequacy`/`HitArea`/`Cursor`/`StockNum`, so
    /// there is nothing to re-point and a new child on the TILE would not instantiate (its
    /// AS3 class declares the members).
    ///
    /// So the badge goes one level down instead, as a named child of the `ItemIcon`
    /// CONTAINER. That sprite is classless -- it carries no `SymbolClass` entry -- and a
    /// classless sprite instantiates its named children normally, which is exactly why the
    /// `IconImage` we inject into our own badge clip binds and draws. The DLL drives it at
    /// the path `ItemIcon/ArtsIcon`.
    NestInItemIcon,
}

/// Everything the edit needs for ONE tile, read out of the movie being edited.
struct TileLayout {
    /// Index of the grid/slot tile `DefineSprite` in the top-level tag stream.
    tile_idx: usize,
    mount: BadgeMount,
    /// The `ItemIcon` container sprite's character id, and its index in the top-level stream.
    item_container_id: u16,
    item_container_idx: usize,
    /// `ItemIcon`'s placement transform in the tile (needed to convert a tile-space position
    /// into container space for [`BadgeMount::NestInItemIcon`]).
    item_scale: f32,
    item_x: f32,
    item_y: f32,
    /// The `ArtsIcon` placement's current character (the dormant stub), when there is one.
    arts_char: Option<u16>,
    /// The `DefineShape` the item icon's `IconImage` clip holds -- the icon's native atlas
    /// cell.
    placeholder_shape_id: u16,
    /// That shape's size and origin in px.
    cell_w: f32,
    cell_origin: (f32, f32),
    /// `AttributeIcon`'s placement -- the mirror reference.
    attr_scale: f32,
    attr_x: f32,
    attr_y: f32,
}

/// Does any `SymbolClass` bind this character to an AS3 class?
fn has_symbol_class(movie: &Movie, id: u16) -> bool {
    movie.tags.iter().any(|t| match t {
        Tag::SymbolClass { symbols, .. } => symbols.iter().any(|(tag, _)| *tag == id),
        _ => false,
    })
}

fn sprite_index(movie: &Movie, id: u16) -> Option<usize> {
    movie
        .tags
        .iter()
        .position(|t| matches!(t, Tag::DefineSprite { id: sid, .. } if *sid == id))
}

/// Every badge-able tile in the movie: a sprite placing `ItemIcon` AND `AttributeIcon` (the
/// mirror reference the badge's position is derived from). Tiles WITH `ArtsIcon` are
/// re-pointed; tiles without get the nested mount.
fn resolve_layouts(
    movie: &Movie,
    require_children: &[&str],
) -> Result<Vec<TileLayout>, BadgeError> {
    let mut out = Vec::new();
    for idx in 0..movie.tags.len() {
        let Tag::DefineSprite { tags: tile, .. } = &movie.tags[idx] else {
            continue;
        };
        if placement_named(tile, ITEM_INSTANCE_NAME).is_none() {
            continue;
        }
        // Movie-specific narrowing (see `BadgeTarget::require_children`): in the HUD movie
        // this keeps the badge on the quick-slot strip and off the item-acquisition banners,
        // which no hook of ours populates.
        if require_children
            .iter()
            .any(|c| placement_named(tile, c).is_none())
        {
            continue;
        }
        // The badge mirrors `AttributeIcon`; a tile without one gives us no authoritative
        // position, so it is not a badge-able tile (icon strips, thumbnail lists).
        let Some((attr_scale, attr_x, attr_y)) =
            placement_named(tile, ATTRIBUTE_INSTANCE_NAME).and_then(placement_transform)
        else {
            continue;
        };
        let (item_scale, item_x, item_y) = placement_named(tile, ITEM_INSTANCE_NAME)
            .and_then(placement_transform)
            .ok_or(BadgeError::Structure("ItemIcon placement transform"))?;
        let item_container_id = placement_char(tile, ITEM_INSTANCE_NAME)
            .ok_or(BadgeError::Structure("ItemIcon placement has no character"))?;

        // ItemIcon's TWO-LEVEL structure is the template: container -> child named
        // `IconImage` -> the placeholder shape. `SetIcon` recurses into `IconImage` and scales
        // the drawn quad by THAT clip's rect, so a one-level target leaves it nothing to
        // recurse into and it paints a tiny quad inside an otherwise correctly-sized slot
        // (run 20260727-215127).
        let container_tags = sprite_tags(movie, item_container_id)
            .ok_or(BadgeError::Structure("ItemIcon container sprite missing"))?;
        let item_iconimage = placement_char(container_tags, BADGE_ICONIMAGE_INSTANCE_NAME)
            .ok_or(BadgeError::Structure("ItemIcon has no IconImage child"))?;
        let iconimage_tags = sprite_tags(movie, item_iconimage)
            .ok_or(BadgeError::Structure("ItemIcon/IconImage sprite missing"))?;
        let placeholder_shape_id = iconimage_tags
            .iter()
            .find_map(|t| match t {
                Tag::PlaceObject2 {
                    character_id: Some(c),
                    ..
                } => Some(*c),
                _ => None,
            })
            .ok_or(BadgeError::Structure(
                "ItemIcon/IconImage places no placeholder",
            ))?;
        let (cell_w, cell_h, cell_origin) = movie
            .tags
            .iter()
            .find_map(|t| match t {
                Tag::DefineShape {
                    shape_id,
                    shape_bounds,
                    ..
                } if *shape_id == placeholder_shape_id => {
                    let (w, h) = rect_size_px(shape_bounds);
                    Some((w, h, rect_origin_px(shape_bounds)))
                }
                _ => None,
            })
            .ok_or(BadgeError::Structure(
                "placeholder shape is not a DefineShape",
            ))?;
        // The icon clip must keep its NATIVE cell extent: `SetIcon` maps the icon out of a
        // texture atlas using the target clip's local rect, so a rect that is not the icon's
        // cell size mis-maps the UVs and smears the whole atlas page into the quad --
        // observed as "I can see all of the ashes of war" in one badge. A cell that is not
        // square (or is implausibly small) means this is not the placeholder we think it is.
        if !(cell_w >= 16.0 && (cell_w - cell_h).abs() <= 1.0) {
            return Err(BadgeError::Structure(
                "placeholder shape is not a square icon cell",
            ));
        }

        let arts_char = placement_char(tile, BADGE_INSTANCE_NAME);
        let mount = if arts_char.is_some() {
            BadgeMount::RepointArtsIcon
        } else {
            // Fail closed if the container is class-bound: a named child added to a sprite
            // whose AS3 class declares its members would not instantiate, and we would ship a
            // silently dead edit.
            if has_symbol_class(movie, item_container_id) {
                return Err(BadgeError::Structure(
                    "ItemIcon container is class-bound; cannot nest the badge",
                ));
            }
            BadgeMount::NestInItemIcon
        };
        let item_container_idx = sprite_index(movie, item_container_id)
            .ok_or(BadgeError::Structure("ItemIcon container not a sprite"))?;

        out.push(TileLayout {
            tile_idx: idx,
            mount,
            item_container_id,
            item_container_idx,
            item_scale,
            item_x,
            item_y,
            arts_char,
            placeholder_shape_id,
            cell_w,
            cell_origin,
            attr_scale,
            attr_x,
            attr_y,
        });
    }
    if out.is_empty() {
        return Err(BadgeError::Structure("no badge-able tile in movie"));
    }
    Ok(out)
}

/// First character id not already used by the movie.
///
/// New DICTIONARY characters are safe: only instance NAMES are AS3-declaration gated, and the
/// badge clip is placed under a name that is either vanilla (`ArtsIcon` on the tile) or lives
/// inside a classless container.
fn first_free_id(movie: &Movie) -> Result<u16, BadgeError> {
    movie
        .tags
        .iter()
        .filter_map(|t| match t {
            Tag::DefineSprite { id, .. } => Some(*id),
            Tag::DefineShape { shape_id, .. } => Some(*shape_id),
            Tag::DefineEditText { character_id, .. } => Some(*character_id),
            Tag::DefineFont3 { font_id, .. } => Some(*font_id),
            Tag::Unknown { code, raw, .. } if *code == GFX_DEFINE_EXTERNAL_IMAGE2 => raw
                .get(..4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]).min(u16::MAX as u32) as u16),
            _ => None,
        })
        .max()
        .ok_or(BadgeError::Structure("movie defines no characters"))?
        .checked_add(1)
        .ok_or(BadgeError::Structure("character id space exhausted"))
}

/// Derive the badge-enabled movie from the game's own vanilla payload.
///
/// For each badge-able tile the edit defines a clip mirroring `ItemIcon`'s two-level shape
/// (container -> `IconImage` -> placeholder) with the game's own Ash-of-War plate image
/// behind it, and mounts it at the mirror of the vanilla infusion badge.
///
/// All-or-nothing: any missing structure fails cleanly and the caller serves the untouched
/// vanilla movie.
pub fn arts_badge(vanilla: &[u8]) -> Result<Vec<u8>, BadgeError> {
    arts_badge_scoped(vanilla, &[], false)
}

/// [`arts_badge`], narrowed to tiles that also place every name in `require_children`.
pub fn arts_badge_scoped(
    vanilla: &[u8],
    require_children: &[&str],
    default_hidden: bool,
) -> Result<Vec<u8>, BadgeError> {
    let mut movie = Movie::parse(vanilla).map_err(BadgeError::Parse)?;
    let layouts = resolve_layouts(&movie, require_children)?;
    let mut next_id = first_free_id(&movie)?;
    let plate_id = next_id;
    next_id += 1;

    // The new characters are inserted as ONE block with the movie's other external images,
    // which sit ahead of every sprite -- so each definition precedes the sprite that places
    // it without any per-tile index bookkeeping. Verified rather than assumed.
    let last_image_idx = movie
        .tags
        .iter()
        .rposition(
            |t| matches!(t, Tag::Unknown { code, .. } if *code == GFX_DEFINE_EXTERNAL_IMAGE2),
        )
        .ok_or(BadgeError::Structure("movie declares no external images"))?;
    if layouts
        .iter()
        .any(|l| l.tile_idx <= last_image_idx || l.item_container_idx <= last_image_idx)
    {
        return Err(BadgeError::Structure(
            "external images do not precede every badge-able sprite",
        ));
    }

    let mut new_tags: Vec<Tag> = vec![external_image_tag(
        plate_id,
        PLATE_IMAGE_NAME,
        PLATE_NATIVE_W as u16,
        PLATE_NATIVE_H,
    )];
    // A movie can hold several tiles sharing one `ItemIcon` container (detail/comparison
    // panels); the nested mount must be injected into each container exactly once.
    let mut nested_containers: Vec<u16> = Vec::new();

    for layout in &layouts {
        if layout.mount == BadgeMount::NestInItemIcon
            && nested_containers.contains(&layout.item_container_id)
        {
            continue;
        }
        let icon_clip_id = next_id;
        let badge_clip_id = next_id + 1;
        next_id = next_id
            .checked_add(2)
            .ok_or(BadgeError::Structure("character id space exhausted"))?;

        // Inner clip == the item `IconImage`'s role: holds the SAME placeholder the item icon
        // does, at identity, so its local rect is the icon's native cell size and `SetIcon`'s
        // atlas UV maths land on exactly one cell. Shrinking THIS clip is what smeared the
        // whole atlas page; the corner size comes from the PARENT placement instead.
        new_tags.push(Tag::DefineSprite {
            id: icon_clip_id,
            frame_count: 1,
            tags: vec![
                place(layout.placeholder_shape_id, 1, None, 1.0, 0.0, 0.0),
                Tag::ShowFrame { force_long: false },
                Tag::End,
            ],
            force_long: false,
        });

        // Outer clip == the item icon container's role: its single named child is `IconImage`,
        // the name `SetIcon` recurses into, plus the plate as a sibling BEHIND it. Both are
        // scaled to the badge box and anchored on the placeholder's own origin, so plate and
        // icon are concentric exactly as in the game's HUD composition.
        let icon_scale = BADGE_RENDER_PX / layout.cell_w;
        let plate_scale = BADGE_RENDER_PX / PLATE_NATIVE_W;
        let (ox, oy) = layout.cell_origin;
        new_tags.push(Tag::DefineSprite {
            id: badge_clip_id,
            frame_count: 1,
            tags: vec![
                place(
                    plate_id,
                    PLATE_DEPTH,
                    None,
                    plate_scale,
                    ox * icon_scale,
                    oy * icon_scale,
                ),
                place(
                    icon_clip_id,
                    ICON_DEPTH,
                    Some(BADGE_ICONIMAGE_INSTANCE_NAME),
                    icon_scale,
                    0.0,
                    0.0,
                ),
                Tag::ShowFrame { force_long: false },
                Tag::End,
            ],
            force_long: false,
        });

        // AUTHORITATIVE POSITION: mirror the vanilla infusion badge instead of hand-tuning.
        // `AttributeIcon` is the game's own corner badge on this exact tile, so reflecting its
        // transform about the tile's centre (x = 0, where `ItemIcon` sits) yields the
        // bottom-LEFT counterpart -- same scale, same vertical offset, same inset from its
        // edge. `AttributeIcon` spans [attr_x, attr_x + rendered]; its mirror spans the same
        // distance in from the opposite side, so the mirrored left edge is -(right edge).
        let badge_rendered = BADGE_RENDER_PX * layout.attr_scale;
        let badge_tile_x = -(layout.attr_x + badge_rendered);
        let badge_tile_y = layout.attr_y;

        match layout.mount {
            BadgeMount::RepointArtsIcon => {
                let Tag::DefineSprite { tags, .. } = &mut movie.tags[layout.tile_idx] else {
                    return Err(BadgeError::Structure("tile is not a DefineSprite"));
                };
                let slot = tags
                    .iter_mut()
                    .find(|t| matches!(t, Tag::PlaceObject2 { name: Some(n), .. } if n == BADGE_INSTANCE_NAME))
                    .ok_or(BadgeError::Structure("ArtsIcon placement in tile"))?;
                let Tag::PlaceObject2 {
                    flags,
                    character_id,
                    matrix,
                    ..
                } = slot
                else {
                    return Err(BadgeError::Structure(
                        "tile ArtsIcon placement is not PlaceObject2",
                    ));
                };
                if *character_id != layout.arts_char {
                    return Err(BadgeError::Structure("tile ArtsIcon character drifted"));
                }
                *character_id = Some(badge_clip_id);
                // Vanilla `ArtsIcon` sits at (-32, +37), which is NOT the mirror of the
                // infusion badge. Replace it with the mirrored transform so the two corner
                // badges are symmetric.
                *matrix = Some(placed_matrix(layout.attr_scale, badge_tile_x, badge_tile_y));
                // HasCharacter/HasMatrix are already set on this vanilla placement; assert
                // them via the flags byte (the writer emits each field only when its bit is
                // set).
                *flags |= PO2_HAS_CHARACTER | PO2_HAS_MATRIX;
            }
            BadgeMount::NestInItemIcon => {
                // Convert the tile-space target into the container's own space: the container
                // is placed at `item_scale` and `(item_x, item_y)`, so everything inside it is
                // divided through by that transform. The badge therefore lands on exactly the
                // same pixels as the re-pointed mount would.
                if layout.item_scale.abs() < f32::EPSILON {
                    return Err(BadgeError::Structure("ItemIcon placement has zero scale"));
                }
                let inv = 1.0 / layout.item_scale;
                let Tag::DefineSprite { tags, .. } = &mut movie.tags[layout.item_container_idx]
                else {
                    return Err(BadgeError::Structure("ItemIcon container is not a sprite"));
                };
                // Depth above the container's own `IconImage` so the badge is not painted over
                // by the item icon it sits on.
                let depth = tags
                    .iter()
                    .filter_map(|t| match t {
                        Tag::PlaceObject2 { depth, .. } | Tag::PlaceObject3 { depth, .. } => {
                            Some(*depth)
                        }
                        _ => None,
                    })
                    .max()
                    .unwrap_or(0)
                    .saturating_add(2);
                let at = tags
                    .iter()
                    .position(|t| matches!(t, Tag::ShowFrame { .. }))
                    .unwrap_or(tags.len().saturating_sub(1));
                tags.insert(
                    at,
                    (if default_hidden { place_hidden } else { place })(
                        badge_clip_id,
                        depth,
                        Some(BADGE_INSTANCE_NAME),
                        layout.attr_scale * inv,
                        (badge_tile_x - layout.item_x) * inv,
                        (badge_tile_y - layout.item_y) * inv,
                    ),
                );
                nested_containers.push(layout.item_container_id);
            }
        }
    }

    // Splice the whole block in at once, after the last vanilla external image.
    for (n, tag) in new_tags.into_iter().enumerate() {
        movie.tags.insert(last_image_idx + 1 + n, tag);
    }

    movie.write().map_err(BadgeError::Write)
}

/// Character ids defined by a movie, in stream order.
fn character_ids(movie: &Movie) -> Vec<u16> {
    movie
        .tags
        .iter()
        .filter_map(|t| match t {
            Tag::DefineSprite { id, .. } => Some(*id),
            Tag::DefineShape { shape_id, .. } => Some(*shape_id),
            Tag::DefineEditText { character_id, .. } => Some(*character_id),
            Tag::DefineFont3 { font_id, .. } => Some(*font_id),
            Tag::Unknown { code, raw, .. }
                if *code == GFX_DEFINE_EXTERNAL_IMAGE2 && raw.len() >= 4 =>
            {
                Some(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as u16)
            }
            _ => None,
        })
        .collect()
}

/// Every `AttributeIcon` placement in the movie, keyed by the sprite that holds it.
fn attribute_placements(movie: &Movie) -> Vec<(u16, Vec<String>)> {
    movie
        .tags
        .iter()
        .filter_map(|t| match t {
            Tag::DefineSprite { id, tags, .. } => Some((
                *id,
                tags.iter()
                    .filter(|t| {
                        matches!(t, Tag::PlaceObject2 { name: Some(n), .. } if n == ATTRIBUTE_INSTANCE_NAME)
                            || matches!(t, Tag::PlaceObject3 { name: Some(n), .. } if n == ATTRIBUTE_INSTANCE_NAME)
                    })
                    .map(|t| format!("{t:?}"))
                    .collect(),
            )),
            _ => None,
        })
        .collect()
}

/// The runtime form of the `arts_badge_diff` test invariant: prove the edit only ADDED, and
/// only re-pointed placements named [`BADGE_INSTANCE_NAME`].
///
/// For a movie we have baked fingerprints for this is redundant with [`derive`]'s exact-bytes
/// check. For a movie we do NOT know -- one another mod supplied through ME3 -- it is the only
/// thing standing between a structural derivation and a corrupted HUD, so it is checked
/// against the parsed output rather than trusted from the code that produced it.
pub fn validate_additive(vanilla: &[u8], edited: &[u8]) -> Result<(), BadgeError> {
    let v = Movie::parse(vanilla).map_err(BadgeError::Parse)?;
    let e = Movie::parse(edited).map_err(BadgeError::Parse)?;
    if v.header != e.header {
        return Err(BadgeError::NotAdditive("movie header changed"));
    }
    let (v_ids, e_ids) = (character_ids(&v), character_ids(&e));
    if let Some(missing) = v_ids.iter().find(|id| !e_ids.contains(id)) {
        let _ = missing;
        return Err(BadgeError::NotAdditive("a character was removed"));
    }
    if e_ids.len() <= v_ids.len() {
        return Err(BadgeError::NotAdditive("no character was added"));
    }
    // The vanilla infusion badge is the badge's POSITION reference and is only ever read.
    // Compare per PRE-EXISTING sprite: the edit legitimately adds sprites of its own, so the
    // two whole-movie lists are expected to differ in length.
    let e_attrs = attribute_placements(&e);
    for (id, want) in attribute_placements(&v) {
        let got = e_attrs.iter().find(|(sid, _)| *sid == id).map(|(_, p)| p);
        if got != Some(&want) {
            return Err(BadgeError::NotAdditive(
                "an AttributeIcon placement changed",
            ));
        }
    }
    // Every pre-existing sprite that changed may differ ONLY by placements named `ArtsIcon`.
    for vt in &v.tags {
        let Tag::DefineSprite { id, tags: vs, .. } = vt else {
            continue;
        };
        let Some(Tag::DefineSprite { tags: es, .. }) = e
            .tags
            .iter()
            .find(|t| matches!(t, Tag::DefineSprite { id: sid, .. } if sid == id))
        else {
            return Err(BadgeError::NotAdditive("a sprite lost its definition"));
        };
        if vs == es {
            continue;
        }
        let strip = |tags: &Vec<Tag>| -> Vec<Tag> {
            tags.iter()
                .filter(|t| {
                    !(matches!(t, Tag::PlaceObject2 { name: Some(n), .. } if n == BADGE_INSTANCE_NAME)
                        || matches!(t, Tag::PlaceObject3 { name: Some(n), .. } if n == BADGE_INSTANCE_NAME))
                })
                .cloned()
                .collect()
        };
        if strip(vs) != strip(es) {
            return Err(BadgeError::NotAdditive(
                "a sprite changed outside its ArtsIcon placement",
            ));
        }
    }
    Ok(())
}

/// Derive the badge for a movie we have NO baked fingerprint for -- i.e. one another mod
/// supplied through ME3.
///
/// The edit itself is already movie-agnostic: it locates tiles by their named children,
/// mirrors the tile's own `AttributeIcon` for position (so a mod that moved or rescaled the
/// tile is followed automatically), reads the atlas cell off the tile's own placeholder shape,
/// and allocates character ids above whatever the movie already uses. What it cannot assume is
/// that we UNDERSTAND the whole file, so two gates bracket it:
///
/// 1. `parse -> write` must reproduce the input byte-for-byte. If a tag we do not model is in
///    there, re-serialising would silently reshape it, so we refuse to touch the movie at all.
///    (All 106 vanilla menu movies pass this, so it is a real gate rather than a rejection.)
/// 2. The output must satisfy [`validate_additive`].
///
/// Either gate failing is a clean no-op: the caller serves the user's own bytes untouched.
pub fn derive_unknown(input: &[u8]) -> Result<Vec<u8>, BadgeError> {
    derive_unknown_scoped(input, &[], false)
}

/// [`derive_unknown`], narrowed to the target's `require_children`.
pub fn derive_unknown_scoped(
    input: &[u8],
    require_children: &[&str],
    default_hidden: bool,
) -> Result<Vec<u8>, BadgeError> {
    let reproduced = Movie::parse(input)
        .map_err(BadgeError::Parse)?
        .write()
        .map_err(BadgeError::Write)?;
    if reproduced != input {
        return Err(BadgeError::NotReproducible {
            in_len: input.len(),
            out_len: reproduced.len(),
            first_diff: reproduced
                .iter()
                .zip(input.iter())
                .position(|(a, b)| a != b),
        });
    }
    let out = arts_badge_scoped(input, require_children, default_hidden)?;
    validate_additive(input, &out)?;
    Ok(out)
}

/// [`arts_badge`] plus the known-input self-consistency gate: for a movie we have baked
/// fingerprints for, the derived bytes must match exactly.
pub fn derive(target: &BadgeTarget, vanilla: &[u8]) -> Result<Vec<u8>, BadgeError> {
    let out = arts_badge_scoped(vanilla, target.require_children, target.default_hidden)?;
    if target.edited_len != 0
        && target.edited_fnv1a64 != 0
        && (out.len() != target.edited_len || fnv1a64(&out) != target.edited_fnv1a64)
    {
        return Err(BadgeError::KnownInputBadOutput {
            file_name: target.file_name,
            out_len: out.len(),
            out_fnv1a64: fnv1a64(&out),
            want_len: target.edited_len,
            want_fnv1a64: target.edited_fnv1a64,
        });
    }
    Ok(out)
}
