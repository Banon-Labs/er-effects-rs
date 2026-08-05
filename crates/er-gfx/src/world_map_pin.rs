//! Give the world map a red pin icon, by putting one on a frame it never uses.
//!
//! # Why this is a `.gfx` edit and not a colour write
//!
//! A world-map pin's icon id is not a texture id and not a tint. `CS::WorldMapPinData::SetTo`
//! (`0x14087ae20`) resolves it to `Icon_0.gotoAndStop(id)`, where `Icon_0` is a **348-frame
//! MovieClip** -- sprite [`ICON_SPRITE_ID`] in `menu:/02_120_WorldMap.gfx` -- and frame `N`
//! places exactly one `MENU_MAP_*` bitmap. So the icon id is a 1-based frame number, the set of
//! achievable pin looks is exactly the set of populated frames, and there is no colour input
//! anywhere on the path: the engine's own red-vs-blue player markers are a frame switch too
//! (`WorldMapPlayRegionData::SetTo` calls `gotoAndStop(1)` for allies and `(2)` for enemies).
//!
//! That leaves two ways to get a red pin. Hooking `SetTo` and driving the engine's GFx colour
//! transform (`FUN_140d838d0`) would tint the existing sprite -- but `SetTo` is shared by every
//! pin class that does not override it, so a filter mistake tints the whole map, and the exact
//! multiply-versus-add layout of the Cxform it builds is unproven. Adding a frame is
//! structurally safer: it cannot affect a single shipped pin, because nothing else ever asks
//! for this frame number.
//!
//! # What gets placed, and why that bitmap
//!
//! [`RED_MARKER_CHARACTER`] is `MENU_MAP_Enemy_02`, the hostile-player marker. It is chosen by
//! measurement rather than by name (`scripts/map-icon-colors.py` decodes the BC7 atlas and
//! reduces each subtexture to alpha-weighted channel means): it is the reddest bitmap in the
//! map atlas at RGB `234/99/48`, against `170/144/81` for the Site-of-Grace icon the pins
//! currently render as. At 146x146 declared it is also the same scale as the grace's 156x156,
//! so it needs no bespoke matrix -- it reuses the placement shape the shipped frames use.
//!
//! It is already a character in this same movie, so nothing new is defined and no texture is
//! shipped: the edit is one `RemoveObject2` and one `PlaceObject3` on a dead frame.
//!
//! # Fail-closed
//!
//! Every structural assumption is checked before anything is written, and a mismatch returns an
//! error rather than an edited movie: the wrong sprite, a frame that already has content, or a
//! movie with fewer frames than expected all abort. A map with the old icon is a disappointment;
//! a corrupted menu movie is a crash on the title screen.

use crate::{GfxError, Matrix, Movie, Tag};

/// The movie that owns the world-map pin icons.
pub const WORLD_MAP_MOVIE_FILE_NAME: &str = "02_120_worldmap.gfx";

/// Sprite id of the `Icon_0` clip whose frames ARE the icon-id space.
pub const ICON_SPRITE_ID: u16 = 171;

/// Frames the icon clip declares. Frame numbers the engine passes are 1-based.
pub const ICON_SPRITE_FRAME_COUNT: u16 = 348;

/// The spare frame the red marker is installed on.
///
/// Sprite 171 declares 348 frames but populates only 118 of them; 300 sits inside a long
/// unpopulated stretch, so no shipped `BonfireWarpParam` icon id can collide with it.
pub const RED_PIN_FRAME: u16 = 300;

/// `MENU_MAP_Enemy_02` -- the hostile-player marker, and the reddest bitmap in the map atlas.
pub const RED_MARKER_CHARACTER: u16 = 52;

/// Depth the icon clip places its bitmap at. Every populated frame uses depth 1.
pub const ICON_DEPTH: u16 = 1;

/// `PlaceObject3` `flags1`: `HasCharacter | HasMatrix`, matching every shipped icon frame.
const PLACE_FLAGS1_CHARACTER_AND_MATRIX: u8 = 0x06;
/// `PlaceObject3` `flags2`: `HasImage`. External-image placements in this movie all set it.
const PLACE_FLAGS2_HAS_IMAGE: u8 = 0x10;

/// Half of `MENU_MAP_Enemy_02`'s 146x146 at half scale, in twips: `146 / 2 / 2 * 20`.
///
/// The shipped frames centre their bitmap by translating back by half its drawn size (the grace
/// is 156x156 at scale 0.5 with a `-780` twip translate, which is exactly `156/2/2*20`), so the
/// same rule applied to 146 gives `-730` and the marker lands centred on the pin's anchor
/// instead of hanging off its corner.
const RED_MARKER_TRANSLATE_TWIPS: i32 = -730;

/// 16.16 fixed-point `0.5`, the scale every full-size icon frame uses.
const HALF_SCALE_FIXED: i32 = 0x0000_8000;
/// Bit widths for the emitted MATRIX fields. These need not match any particular shipped tag --
/// the writer re-packs from these -- but they must be wide enough to hold the values.
const SCALE_NBITS: u32 = 17;
const TRANSLATE_NBITS: u32 = 16;

/// Why a red pin frame could not be installed. Every variant means the movie was left alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RedPinError {
    /// The movie did not parse.
    Parse(GfxError),
    /// The movie did not serialise back.
    Write(GfxError),
    /// No `DefineSprite` with [`ICON_SPRITE_ID`] -- this is not the movie we reversed.
    IconSpriteMissing,
    /// The icon sprite declares a different frame count than expected.
    FrameCountMismatch { found: u16 },
    /// The sprite's tag stream held fewer frames than [`RED_PIN_FRAME`] needs.
    FrameOutOfRange { frames_found: usize },
    /// The target frame already places something, so it is not the dead frame we think it is.
    FrameNotEmpty,
}

impl core::fmt::Display for RedPinError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Parse(error) => write!(f, "world-map movie did not parse: {error:?}"),
            Self::Write(error) => write!(f, "edited world-map movie did not serialise: {error:?}"),
            Self::IconSpriteMissing => write!(
                f,
                "no DefineSprite {ICON_SPRITE_ID} (the Icon_0 clip); this is not the world-map \
                 movie the icon-frame mapping was reversed against"
            ),
            Self::FrameCountMismatch { found } => write!(
                f,
                "icon sprite declares {found} frames, expected {ICON_SPRITE_FRAME_COUNT}"
            ),
            Self::FrameOutOfRange { frames_found } => write!(
                f,
                "icon sprite's tag stream holds {frames_found} frames, too few for frame \
                 {RED_PIN_FRAME}"
            ),
            Self::FrameNotEmpty => write!(
                f,
                "frame {RED_PIN_FRAME} already places something; refusing to overwrite a shipped \
                 icon"
            ),
        }
    }
}

impl std::error::Error for RedPinError {}

/// The `PlaceObject3` that draws the red marker, centred, at the icon depth.
fn red_marker_placement() -> Tag {
    Tag::PlaceObject3 {
        flags1: PLACE_FLAGS1_CHARACTER_AND_MATRIX,
        flags2: PLACE_FLAGS2_HAS_IMAGE,
        depth: ICON_DEPTH,
        class_name: None,
        character_id: Some(RED_MARKER_CHARACTER),
        matrix: Some(Matrix {
            has_scale: true,
            scale_nbits: SCALE_NBITS,
            scale_x: HALF_SCALE_FIXED,
            scale_y: HALF_SCALE_FIXED,
            has_rotate: false,
            rotate_nbits: 0,
            rotate_skew0: 0,
            rotate_skew1: 0,
            translate_nbits: TRANSLATE_NBITS,
            translate_x: RED_MARKER_TRANSLATE_TWIPS,
            translate_y: RED_MARKER_TRANSLATE_TWIPS,
        }),
        color_transform: None,
        ratio: None,
        name: None,
        clip_depth: None,
        filters: None,
        blend_mode: None,
        bitmap_cache: None,
        visible: None,
        force_long: false,
    }
}

/// Index of the tag that ends frame `frame` (1-based) inside a sprite's tag stream.
///
/// Frames are delimited by `ShowFrame`, so frame `N` ends at the `N`th one.
fn show_frame_index(tags: &[Tag], frame: u16) -> Option<usize> {
    let mut seen = 0_u16;
    for (index, tag) in tags.iter().enumerate() {
        if matches!(tag, Tag::ShowFrame { .. }) {
            seen += 1;
            if seen == frame {
                return Some(index);
            }
        }
    }
    None
}

/// How many `ShowFrame`s a stream holds, for the out-of-range error.
fn frame_count(tags: &[Tag]) -> usize {
    tags.iter()
        .filter(|tag| matches!(tag, Tag::ShowFrame { .. }))
        .count()
}

/// Whether the frame ending at `show_frame` places or removes anything.
///
/// Walks back to the previous `ShowFrame` and reports whether the span between them is empty.
/// A populated span means [`RED_PIN_FRAME`] is not the dead frame the RE said it was, and the
/// edit must not proceed.
fn frame_is_empty(tags: &[Tag], show_frame: usize) -> bool {
    let start = tags[..show_frame]
        .iter()
        .rposition(|tag| matches!(tag, Tag::ShowFrame { .. }))
        .map_or(0, |index| index + 1);
    !tags[start..show_frame].iter().any(|tag| {
        matches!(
            tag,
            Tag::PlaceObject2 { .. } | Tag::PlaceObject3 { .. } | Tag::RemoveObject2 { .. }
        )
    })
}

/// Install the red marker on [`RED_PIN_FRAME`] of the icon clip, in place.
///
/// # Errors
///
/// Returns a [`RedPinError`] and leaves `movie` untouched when any structural expectation fails.
pub fn install_red_pin_frame(movie: &mut Movie) -> Result<(), RedPinError> {
    let sprite = movie
        .tags
        .iter_mut()
        .find_map(|tag| match tag {
            Tag::DefineSprite {
                id,
                frame_count,
                tags,
                ..
            } if *id == ICON_SPRITE_ID => Some((frame_count, tags)),
            _ => None,
        })
        .ok_or(RedPinError::IconSpriteMissing)?;
    let (declared_frames, tags) = sprite;
    if *declared_frames != ICON_SPRITE_FRAME_COUNT {
        return Err(RedPinError::FrameCountMismatch {
            found: *declared_frames,
        });
    }
    let show_frame = show_frame_index(tags, RED_PIN_FRAME).ok_or(RedPinError::FrameOutOfRange {
        frames_found: frame_count(tags),
    })?;
    if !frame_is_empty(tags, show_frame) {
        return Err(RedPinError::FrameNotEmpty);
    }
    // Insert immediately BEFORE the frame's ShowFrame, so the placement belongs to this frame.
    // The RemoveObject2 first, mirroring every shipped icon frame: the display list persists
    // across frames, so without it the previous icon stays under ours at the same depth.
    tags.splice(
        show_frame..show_frame,
        [
            Tag::RemoveObject2 {
                depth: ICON_DEPTH,
                force_long: false,
            },
            red_marker_placement(),
        ],
    );
    Ok(())
}

/// Parse `bytes`, install the red pin frame, and serialise the result.
///
/// # Errors
///
/// See [`RedPinError`]. On any error the caller should use the original bytes unchanged.
pub fn with_red_pin_frame(bytes: &[u8]) -> Result<Vec<u8>, RedPinError> {
    let mut movie = Movie::parse(bytes).map_err(RedPinError::Parse)?;
    install_red_pin_frame(&mut movie)?;
    movie.write().map_err(RedPinError::Write)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sprite_with_frames(id: u16, frames: u16, populate: &[u16]) -> Tag {
        let mut tags = Vec::new();
        for frame in 1..=frames {
            if populate.contains(&frame) {
                tags.push(Tag::PlaceObject3 {
                    flags1: PLACE_FLAGS1_CHARACTER_AND_MATRIX,
                    flags2: PLACE_FLAGS2_HAS_IMAGE,
                    depth: ICON_DEPTH,
                    class_name: None,
                    character_id: Some(169),
                    matrix: None,
                    color_transform: None,
                    ratio: None,
                    name: None,
                    clip_depth: None,
                    filters: None,
                    blend_mode: None,
                    bitmap_cache: None,
                    visible: None,
                    force_long: false,
                });
            }
            tags.push(Tag::ShowFrame { force_long: false });
        }
        tags.push(Tag::End);
        Tag::DefineSprite {
            id,
            frame_count: frames,
            tags,
            force_long: false,
        }
    }

    fn movie_with(sprite: Tag) -> Movie {
        Movie {
            header: crate::Header {
                version: 11,
                movie_rect_raw: vec![0, 0, 0, 0],
                frame_rate: 30 << 8,
                frame_count: 1,
            },
            tags: vec![sprite, Tag::End],
        }
    }

    #[test]
    fn the_red_marker_lands_on_the_target_frame_and_nowhere_else() {
        let mut movie = movie_with(sprite_with_frames(
            ICON_SPRITE_ID,
            ICON_SPRITE_FRAME_COUNT,
            &[1, 2, 3],
        ));
        install_red_pin_frame(&mut movie).expect("installs");
        let Tag::DefineSprite { tags, .. } = &movie.tags[0] else {
            panic!("sprite");
        };
        // The placement must sit in frame RED_PIN_FRAME's span, i.e. between the (N-1)th and
        // Nth ShowFrame. An off-by-one here silently gives the pins a DIFFERENT icon.
        let target = show_frame_index(tags, RED_PIN_FRAME).expect("target frame");
        let previous = show_frame_index(tags, RED_PIN_FRAME - 1).expect("previous frame");
        let span = &tags[previous + 1..target];
        assert_eq!(span.len(), 2, "one remove and one place");
        assert!(matches!(span[0], Tag::RemoveObject2 { depth, .. } if depth == ICON_DEPTH));
        assert!(matches!(
            span[1],
            Tag::PlaceObject3 {
                character_id: Some(RED_MARKER_CHARACTER),
                depth: ICON_DEPTH,
                ..
            }
        ));
    }

    #[test]
    fn the_frame_count_is_unchanged_so_no_shipped_icon_id_shifts() {
        // Adding a ShowFrame instead of inserting before one would renumber every later frame
        // and silently repoint a large block of shipped icon ids.
        let mut movie = movie_with(sprite_with_frames(
            ICON_SPRITE_ID,
            ICON_SPRITE_FRAME_COUNT,
            &[1],
        ));
        let Tag::DefineSprite { tags, .. } = &movie.tags[0] else {
            panic!("sprite");
        };
        let before = frame_count(tags);
        install_red_pin_frame(&mut movie).expect("installs");
        let Tag::DefineSprite {
            tags,
            frame_count: declared,
            ..
        } = &movie.tags[0]
        else {
            panic!("sprite");
        };
        assert_eq!(frame_count(tags), before);
        assert_eq!(*declared, ICON_SPRITE_FRAME_COUNT);
    }

    #[test]
    fn a_populated_target_frame_refuses_rather_than_overwriting_a_shipped_icon() {
        let mut movie = movie_with(sprite_with_frames(
            ICON_SPRITE_ID,
            ICON_SPRITE_FRAME_COUNT,
            &[RED_PIN_FRAME],
        ));
        assert_eq!(
            install_red_pin_frame(&mut movie),
            Err(RedPinError::FrameNotEmpty)
        );
    }

    #[test]
    fn a_movie_without_the_icon_sprite_is_refused() {
        let mut movie = movie_with(sprite_with_frames(999, ICON_SPRITE_FRAME_COUNT, &[]));
        assert_eq!(
            install_red_pin_frame(&mut movie),
            Err(RedPinError::IconSpriteMissing)
        );
    }

    #[test]
    fn a_different_frame_count_is_refused_as_a_different_movie() {
        let mut movie = movie_with(sprite_with_frames(ICON_SPRITE_ID, 100, &[]));
        assert_eq!(
            install_red_pin_frame(&mut movie),
            Err(RedPinError::FrameCountMismatch { found: 100 })
        );
    }

    #[test]
    fn the_target_frame_is_inside_the_declared_range() {
        assert!(RED_PIN_FRAME >= 1 && RED_PIN_FRAME <= ICON_SPRITE_FRAME_COUNT);
    }

    #[test]
    fn the_marker_is_centred_by_the_same_rule_the_shipped_icons_use() {
        // grace: 156 px at scale 0.5 -> -780 twips. enemy: 146 px at scale 0.5 -> -730.
        // Multiply BEFORE dividing: 146 px is an odd number of half-pixels, so `146/2/2*20`
        // floors to 36 px and lands the marker 10 twips off centre.
        assert_eq!(RED_MARKER_TRANSLATE_TWIPS, -(146 * 20 / 4));
        assert_eq!(-(156 * 20 / 4), -780);
    }
}
