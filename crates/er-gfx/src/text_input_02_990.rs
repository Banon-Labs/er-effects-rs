//! Inline layout derivation for `data0:/menu/win/02_990_textinput.gfx`.
//!
//! The native movie places its 400px editor at its own `(100, 100)` origin. Loaded as a separate
//! `SoftwareKeyboardJob` over ProfileSelect, that origin lands at the upper-left of the screen.
//! This edit keeps the native input/caret/IME implementation but moves its `TextInput` sprite over
//! the drive row's `CurrentPath` field, widens the real edit box to the same width, and hides the
//! text-input movie's own chrome so the existing CurrentPath button remains the sole frame.

use crate::profile_05_010_layout::Profile05_010Layout;
use crate::{CxformWithAlpha, GfxError, Movie, TWIPS_PER_PIXEL, Tag};
use er_game_base::fnv1a::fnv1a64;

pub const VANILLA_LEN: usize = 1141;
pub const VANILLA_FNV1A64: u64 = 0xe896_37d7_2af0_a2c8;
/// Installed 1.16.2 MemoryFile payload. It differs from the July extraction corpus by 11 bytes but
/// retains the same tag/character structure used by the structural edit below.
pub const RUNTIME_VANILLA_LEN: usize = 1152;
pub const RUNTIME_VANILLA_FNV1A64: u64 = 0x8803_6987_5f1e_8e98;
pub const INLINE_LEN: usize = 1160;
pub const INLINE_FNV1A64: u64 = 0xcea6_6846_d53b_edc5;

const TEXT_INPUT_SPRITE_ID: u16 = 8;
const TEXT_FIELD_CHARACTER_ID: u16 = 7;

/// Instance name of the root placement of [`TEXT_INPUT_SPRITE_ID`], and of the editable field
/// ([`TEXT_FIELD_CHARACTER_ID`]) inside it. Both are authored in the vanilla movie, so the runtime can
/// reach the live field by name (`root -> TextInput -> Text_0`) instead of walking display lists.
/// Read out of the corpus movie itself (`02_990_textinput.gfx`, 1141 bytes), whose only other strings
/// are the font and the four chrome bitmaps.
pub const TEXT_INPUT_SPRITE_NAME: &str = "TextInput";
pub const TEXT_FIELD_INSTANCE_NAME: &str = "Text_0";
const PROFILE_LIST_CENTER_X_PX: f32 = 960.0;
const PROFILE_LIST_CENTER_Y_PX: f32 = 540.0;
const FIRST_COMPACT_ROW_CENTER_Y_PX: f32 = -216.0;
const NATIVE_TEXT_INPUT_ORIGIN_PX: f32 = 100.0;
const TEXT_FIELD_LOCAL_X_PX: f32 = -8.0;
const TEXT_FIELD_LOCAL_Y_PX: f32 = -2.0;

/// Position the *external* MenuWindow root so the vanilla child TextInput bounds land over the
/// ProfileSelect CurrentPath field. The native menu reconstructs/positions its own child after GFx
/// parsing, so changing the authored child matrix alone is not an effective placement boundary.
pub fn path_editor_window_position() -> (f32, f32) {
    path_editor_window_position_for_layout(&Profile05_010Layout::default())
}

pub fn path_editor_window_position_for_layout(layout: &Profile05_010Layout) -> (f32, f32) {
    let editor = &layout.path_editor;
    let target_x = PROFILE_LIST_CENTER_X_PX + editor.x;
    let target_y = PROFILE_LIST_CENTER_Y_PX + FIRST_COMPACT_ROW_CENTER_Y_PX + editor.y;
    (
        target_x - (NATIVE_TEXT_INPUT_ORIGIN_PX + TEXT_FIELD_LOCAL_X_PX),
        target_y - (NATIVE_TEXT_INPUT_ORIGIN_PX + TEXT_FIELD_LOCAL_Y_PX),
    )
}

#[derive(Debug)]
pub enum InlineTextInputError {
    Parse(GfxError),
    Write(GfxError),
    UnknownInput { len: usize, fnv: u64 },
    MissingStructure(&'static str),
    KnownInputBadOutput { len: usize, fnv: u64 },
}

impl core::fmt::Display for InlineTextInputError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Parse(error) => write!(f, "parse: {error}"),
            Self::Write(error) => write!(f, "write: {error}"),
            Self::UnknownInput { len, fnv } => {
                write!(f, "unknown 02_990 input len={len} fnv=0x{fnv:016x}")
            }
            Self::MissingStructure(name) => write!(f, "missing 02_990 structure {name}"),
            Self::KnownInputBadOutput { len, fnv } => write!(
                f,
                "known 02_990 input derived len={len} fnv=0x{fnv:016x}; expected len={INLINE_LEN} fnv=0x{INLINE_FNV1A64:016x}"
            ),
        }
    }
}

impl std::error::Error for InlineTextInputError {}

pub fn is_known_vanilla(bytes: &[u8]) -> bool {
    let fingerprint = (bytes.len(), fnv1a64(bytes));
    matches!(
        fingerprint,
        (VANILLA_LEN, VANILLA_FNV1A64) | (RUNTIME_VANILLA_LEN, RUNTIME_VANILLA_FNV1A64)
    )
}

fn alpha_zero(tag: &mut Tag) {
    let Tag::PlaceObject2 {
        flags,
        color_transform,
        ..
    } = tag
    else {
        return;
    };
    *flags |= 0x08;
    *color_transform = Some(CxformWithAlpha {
        has_add: false,
        has_mult: true,
        nbits: 10,
        mult: Some([256, 256, 256, 0]),
        add: None,
    });
}

pub fn inline_current_path_editor(vanilla: &[u8]) -> Result<Vec<u8>, InlineTextInputError> {
    let corpus_variant = vanilla.len() == VANILLA_LEN && fnv1a64(vanilla) == VANILLA_FNV1A64;
    if !is_known_vanilla(vanilla) {
        return Err(InlineTextInputError::UnknownInput {
            len: vanilla.len(),
            fnv: fnv1a64(vanilla),
        });
    }
    let layout = Profile05_010Layout::default();
    let path = &layout.path_editor;
    let mut movie = Movie::parse(vanilla).map_err(InlineTextInputError::Parse)?;
    let mut found_root = false;
    let mut resized_field = false;
    let mut hidden_chrome = 0usize;

    for tag in &mut movie.tags {
        match tag {
            Tag::PlaceObject2 {
                name: Some(name),
                character_id: Some(TEXT_INPUT_SPRITE_ID),
                matrix: Some(matrix),
                ..
            } if name == "TextInput" => {
                // Keep the native authored child matrix. Runtime positions the owning MenuWindow's
                // root SceneObjProxy; the native controller rewrites this child after parsing.
                let _ = matrix;
                found_root = true;
            }
            Tag::DefineEditText {
                character_id: TEXT_FIELD_CHARACTER_ID,
                bounds,
                font_height,
                ..
            } => {
                bounds.nbits = 15;
                bounds.x_min = -2 * TWIPS_PER_PIXEL;
                bounds.x_max = (path.width - 2) * TWIPS_PER_PIXEL;
                bounds.y_min = -2 * TWIPS_PER_PIXEL;
                bounds.y_max = (path.clip_height - 2) * TWIPS_PER_PIXEL;
                *font_height = Some((path.font_height * TWIPS_PER_PIXEL) as u16);
                resized_field = true;
            }
            Tag::DefineSprite { id, tags, .. } if *id == TEXT_INPUT_SPRITE_ID => {
                for child in tags {
                    if matches!(
                        child,
                        Tag::PlaceObject2 {
                            character_id: Some(5) | Some(6),
                            ..
                        }
                    ) {
                        alpha_zero(child);
                        hidden_chrome += 1;
                    }
                }
            }
            _ => {}
        }
    }

    if !found_root {
        return Err(InlineTextInputError::MissingStructure(
            "root TextInput placement",
        ));
    }
    if !resized_field {
        return Err(InlineTextInputError::MissingStructure(
            "DefineEditText character 7",
        ));
    }
    if hidden_chrome != 3 {
        return Err(InlineTextInputError::MissingStructure(
            "three native chrome placements in sprite 8",
        ));
    }
    let out = movie.write().map_err(InlineTextInputError::Write)?;
    let out_fnv = fnv1a64(&out);
    if corpus_variant && (out.len() != INLINE_LEN || out_fnv != INLINE_FNV1A64) {
        return Err(InlineTextInputError::KnownInputBadOutput {
            len: out.len(),
            fnv: out_fnv,
        });
    }
    Ok(out)
}
