//! The `ChrSpawnRequest` block, built byte by byte at the offsets the retail caller writes.
//!
//! # Why a byte array and not a `#[repr(C)] struct`
//!
//! A Rust struct would put the fields wherever rustc likes and the layout would then be an
//! assumption nobody could see. Here every store names its offset out of
//! [`crate::possess::layout::chr_spawn_request`], which is a transcription of
//! `CSTalkDynamicChrCtrl`'s own stack stores -- so the layout is stated once, in the table, and
//! the tests below read the block back at those same offsets.
//!
//! # The one field that cannot be filled until the block stops moving
//!
//! `model.backingString.pointer` points at the block: at `model + MODEL_BUFFER`, thirty-two
//! `wchar_t` of inplace storage. A self-referential pointer cannot be baked into a value that is
//! about to be moved, so [`SpawnRequest::new`] leaves it zero and [`SpawnRequest::bind`] writes it
//! once the block is at the address the game will read it from. `bind` is ordinary Rust arithmetic
//! on our own memory -- no game involved -- so the test at the foot of this file proves it.

// Pure byte assembly; ungated so `cargo test` proves it on the host with no game running.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::possess::layout::chr_spawn_request as req;

/// Highest chr id the `cNNNN` name can spell. The format is `%04d` and the game's own directories
/// stop at four digits, so a fifth would produce a name no `chrbnd` exists for -- caught here
/// rather than as a creature that never leaves `LoadWait`.
pub(crate) const MAX_CHR_ID: u32 = 9999;

/// `charaInitParam` for the creature path.
///
/// `ChrInsFactory::CreateCharacter` tests `param_3->charaInitParam < 0` and takes the
/// `HeapAlloc(0x5e0)` + `EnemyIns` branch when it holds; a non-negative value takes the
/// `HeapAlloc(0x740)` + `PlayerIns` branch instead, which is a different object of a different size
/// with a different vtable. `-1` is what the retail caller writes (`OR RDI,-1`).
const CREATURE_CHARA_INIT_PARAM: i32 = -1;

/// `eventEntityId`. See [`req::EVENT_ENTITY_ID`]: zero is never inserted into the `ChrSet`'s
/// `eventEntityIdMap`, so it cannot shadow a map entity, and it is what the retail caller uses.
const NO_EVENT_ENTITY: u32 = 0;

/// What to create. Everything the game needs that this crate has to decide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SpawnSpec {
    /// The model number -- `4500` becomes `c4500`, which is the whole of how a chr id reaches the
    /// asset loader.
    pub(crate) chr_id: u32,
    /// The `NpcParam` row. Drives stats, behaviour and which resources the `ChrRes` step machine
    /// acquires; an id with no row falls back to row 0 rather than failing.
    pub(crate) npc_param_id: i32,
    /// The `NpcThinkParam` row. May be anything: the lookup pre-initialises its result and
    /// `LoadWait` treats the resulting NULL `LuaDat` caps as satisfied.
    pub(crate) npc_think_id: i32,
    /// Where the request says to put it. Not read on the creature path -- see the module docs on
    /// [`req`] -- and filled anyway because the retail caller fills it.
    pub(crate) position: [f32; 3],
    /// Likewise for the facing.
    pub(crate) yaw: f32,
}

impl SpawnSpec {
    /// The `NpcParam` row a bare chr id implies.
    ///
    /// Rows are `<chr><4 digits>`, which is the same arithmetic
    /// `crate::possess::game::Chr::npc_param_id` inverts to key the moveset table -- so a spawn
    /// configured by chr id alone lands on the row whose moveset the table already ships.
    #[must_use]
    pub(crate) const fn default_npc_param_id(chr_id: u32) -> i32 {
        (chr_id * 10_000) as i32
    }

    /// The `cNNNN` name, as UTF-16 with its terminator, or `None` for an id the format cannot
    /// spell.
    #[must_use]
    pub(crate) fn model_name(chr_id: u32) -> Option<Vec<u16>> {
        if chr_id > MAX_CHR_ID {
            return None;
        }
        let mut name: Vec<u16> = format!("c{chr_id:04}").encode_utf16().collect();
        name.push(0);
        Some(name)
    }
}

/// The 200-byte block, 16-aligned.
///
/// `align(16)` because `position` and `orientation` are `FloatVector4`s and the engine loads that
/// type with `MOVAPS`/`MOVDQA` wherever it does touch one. The creature path does not read either
/// -- so this is not load-bearing today -- and it costs nothing to be a well-formed
/// `ChrSpawnRequest` rather than one that happens to work because nobody looked.
#[repr(C, align(16))]
pub(crate) struct SpawnRequest {
    bytes: [u8; req::SIZE],
}

impl SpawnRequest {
    /// Build the block, with `model.backingString.pointer` left NULL for [`Self::bind`].
    ///
    /// `None` for a chr id `c%04d` cannot spell.
    #[must_use]
    pub(crate) fn new(spec: &SpawnSpec) -> Option<Self> {
        let name = SpawnSpec::model_name(spec.chr_id)?;
        // Cannot happen for a four-digit id, and asserting it here is cheaper than a buffer
        // overrun in the one place this writes past a fixed offset.
        if name.len() > req::MODEL_BUFFER_WCHARS {
            return None;
        }
        let mut request = Self {
            bytes: [0u8; req::SIZE],
        };
        // The vectors the retail caller writes. `w` is 1.0 for a position, 0.0 for a rotation, and
        // the scale pair is the engine's `{1,1,1,1}` constant.
        request.put_vec4(
            req::POSITION,
            [spec.position[0], spec.position[1], spec.position[2], 1.0],
        );
        request.put_vec4(req::ORIENTATION, [0.0, spec.yaw, 0.0, 0.0]);
        request.put_vec4(req::SCALE, [1.0; 4]);
        request.put_vec4(req::UNK30, [1.0; 4]);

        request.put_i32(req::NPC_PARAM_ID, spec.npc_param_id);
        request.put_i32(req::NPC_THINK_ID, spec.npc_think_id);
        request.put_i32(req::CHARA_INIT_PARAM, CREATURE_CHARA_INIT_PARAM);
        request.put_u32(req::EVENT_ENTITY_ID, NO_EVENT_ENTITY);
        request.put_i32(req::TALK_ID, 0);

        // The DLInplaceStr header, exactly as the retail caller lays it out, minus the vtable --
        // see `req::MODEL_VFTABLE` for the three consumers that were checked for a virtual call.
        let model = req::MODEL;
        request.put_usize(model + req::MODEL_VFTABLE, 0);
        request.put_usize(model + req::MODEL_BACKING, 0);
        // `len` counts characters, not the terminator.
        request.put_usize(model + req::MODEL_LEN, name.len() - 1);
        request.put_u32(model + req::MODEL_UNK18, 0);
        request.put_u16(model + req::MODEL_CHAR_SIZE, 2);
        request.bytes[model + req::MODEL_TYPE] = req::MODEL_TYPE_UTF16;
        request.bytes[model + req::MODEL_FLAGS] = 0;
        for (index, unit) in name.iter().enumerate() {
            request.put_u16(model + req::MODEL_BUFFER + index * 2, *unit);
        }
        Some(request)
    }

    /// Point `model.backingString.pointer` at this block's own inplace buffer.
    ///
    /// Must be called once the block is at its final address, and the block must not be moved
    /// afterwards -- the pointer is into itself. Calling it twice is harmless; not calling it hands
    /// the game a null name pointer, which `Format(L"%s_%04d", ptr, index)` would dereference.
    pub(crate) fn bind(&mut self) {
        let buffer = core::ptr::from_ref(&self.bytes) as usize + req::MODEL + req::MODEL_BUFFER;
        self.put_usize(req::MODEL + req::MODEL_BACKING, buffer);
    }

    /// The address the game is handed.
    #[must_use]
    pub(crate) fn as_ptr(&self) -> *const u8 {
        core::ptr::from_ref(&self.bytes).cast()
    }

    /// The name pointer currently stored, for the test and for the caller's own sanity check.
    #[must_use]
    pub(crate) fn bound_name_pointer(&self) -> usize {
        usize::from_le_bytes(
            self.bytes[req::MODEL + req::MODEL_BACKING..req::MODEL + req::MODEL_BACKING + 8]
                .try_into()
                .expect("eight bytes"),
        )
    }

    fn put_vec4(&mut self, at: usize, value: [f32; 4]) {
        for (index, component) in value.iter().enumerate() {
            self.bytes[at + index * 4..at + index * 4 + 4]
                .copy_from_slice(&component.to_le_bytes());
        }
    }

    fn put_i32(&mut self, at: usize, value: i32) {
        self.bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u32(&mut self, at: usize, value: u32) {
        self.bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u16(&mut self, at: usize, value: u16) {
        self.bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_usize(&mut self, at: usize, value: usize) {
        self.bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }

    #[cfg(test)]
    fn get_i32(&self, at: usize) -> i32 {
        i32::from_le_bytes(self.bytes[at..at + 4].try_into().expect("four bytes"))
    }

    #[cfg(test)]
    fn get_f32(&self, at: usize) -> f32 {
        f32::from_le_bytes(self.bytes[at..at + 4].try_into().expect("four bytes"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SpawnSpec {
        SpawnSpec {
            chr_id: 4500,
            npc_param_id: 45_000,
            npc_think_id: 0,
            position: [10.0, 20.0, 30.0],
            yaw: 1.5,
        }
    }

    /// The field that decides which object is allocated. A non-negative `charaInitParam` takes
    /// `CreateCharacter` down the `HeapAlloc(0x740)` + `PlayerIns` branch instead of the
    /// `HeapAlloc(0x5e0)` + `EnemyIns` one -- a different type, a different size, a different
    /// vtable -- and everything this crate then does to the result would be reading a `PlayerIns`
    /// as an `EnemyIns`.
    #[test]
    fn chara_init_param_is_negative_so_the_creature_branch_is_taken() {
        let request = SpawnRequest::new(&spec()).expect("c4500 is spellable");
        assert!(
            request.get_i32(req::CHARA_INIT_PARAM) < 0,
            "{}",
            request.get_i32(req::CHARA_INIT_PARAM)
        );
    }

    /// Zero is never inserted into `eventEntityIdMap`, so it cannot shadow a map entity. Any
    /// nonzero id we invented would be inserted, and the shadowed entity's events would start
    /// resolving to our creature.
    #[test]
    fn the_event_entity_id_is_zero() {
        let request = SpawnRequest::new(&spec()).expect("spellable");
        assert_eq!(request.get_i32(req::EVENT_ENTITY_ID), 0);
        assert_eq!(request.get_i32(req::TALK_ID), 0);
    }

    #[test]
    fn the_param_rows_land_at_their_own_offsets() {
        let request = SpawnRequest::new(&SpawnSpec {
            npc_param_id: 45_010,
            npc_think_id: 45_000,
            ..spec()
        })
        .expect("spellable");
        assert_eq!(request.get_i32(req::NPC_PARAM_ID), 45_010);
        assert_eq!(request.get_i32(req::NPC_THINK_ID), 45_000);
    }

    /// The name is the model selection; a wrong `%04d` resolves to a `chrbnd` that does not exist
    /// and the creature sits in `LoadWait` forever, which is the failure the deadline exists to
    /// bound rather than the one it should be catching.
    #[test]
    fn the_model_name_is_c_plus_four_digits_utf16_and_nul_terminated() {
        assert_eq!(
            SpawnSpec::model_name(4500).expect("spellable"),
            vec![
                b'c' as u16,
                b'4' as u16,
                b'5' as u16,
                b'0' as u16,
                b'0' as u16,
                0
            ]
        );
        // A short id is zero-padded to four, which is what `%04d` does and what the directories
        // are named.
        assert_eq!(
            SpawnSpec::model_name(80).expect("spellable"),
            vec![
                b'c' as u16,
                b'0' as u16,
                b'0' as u16,
                b'8' as u16,
                b'0' as u16,
                0
            ]
        );
        assert_eq!(SpawnSpec::model_name(MAX_CHR_ID + 1), None, "five digits");
        assert!(
            SpawnRequest::new(&SpawnSpec {
                chr_id: MAX_CHR_ID + 1,
                ..spec()
            })
            .is_none()
        );
    }

    /// ...and it is in the block, at the inplace buffer, with `len` counting characters rather than
    /// bytes or including the terminator.
    #[test]
    fn the_name_is_written_into_the_inplace_buffer_with_a_character_count() {
        let request = SpawnRequest::new(&spec()).expect("spellable");
        let at = req::MODEL + req::MODEL_BUFFER;
        let units: Vec<u16> = (0..6)
            .map(|index| {
                u16::from_le_bytes(
                    request.bytes[at + index * 2..at + index * 2 + 2]
                        .try_into()
                        .expect("two bytes"),
                )
            })
            .collect();
        assert_eq!(units, SpawnSpec::model_name(4500).expect("spellable"));
        let len = usize::from_le_bytes(
            request.bytes[req::MODEL + req::MODEL_LEN..req::MODEL + req::MODEL_LEN + 8]
                .try_into()
                .expect("eight bytes"),
        );
        assert_eq!(len, 5, "five characters, not six and not ten bytes");
        assert_eq!(
            request.bytes[req::MODEL + req::MODEL_TYPE],
            req::MODEL_TYPE_UTF16
        );
    }

    /// The self-pointer. Unbound it is null, and `Format(L"%s_%04d", ptr, index)` would dereference
    /// that; bound it must land exactly on the first character of the name.
    #[test]
    fn binding_points_the_backing_pointer_at_the_blocks_own_buffer() {
        let mut request = SpawnRequest::new(&spec()).expect("spellable");
        assert_eq!(request.bound_name_pointer(), 0, "null until bound");
        request.bind();
        let expected = request.as_ptr() as usize + req::MODEL + req::MODEL_BUFFER;
        assert_eq!(request.bound_name_pointer(), expected);
        // ...and reading through it gives the name back.
        let first = unsafe { (request.bound_name_pointer() as *const u16).read() };
        assert_eq!(first, u16::from(b'c'));
        // Binding twice is the same answer, so a defensive re-bind cannot corrupt it.
        request.bind();
        assert_eq!(request.bound_name_pointer(), expected);
    }

    /// The vectors, which the creature path does not read and which are filled anyway. `w` matters:
    /// a position vector with `w = 0` is a direction to anything that does read it.
    #[test]
    fn the_position_and_orientation_are_written_where_the_retail_caller_writes_them() {
        let request = SpawnRequest::new(&spec()).expect("spellable");
        assert_eq!(request.get_f32(req::POSITION), 10.0);
        assert_eq!(request.get_f32(req::POSITION + 4), 20.0);
        assert_eq!(request.get_f32(req::POSITION + 8), 30.0);
        assert_eq!(request.get_f32(req::POSITION + 12), 1.0, "w for a point");
        assert_eq!(request.get_f32(req::ORIENTATION + 4), 1.5, "yaw in .y");
        assert_eq!(request.get_f32(req::SCALE), 1.0);
        assert_eq!(request.get_f32(req::UNK30), 1.0);
    }

    /// The bytes the game reads must be exactly the block it writes, at an address `MOVAPS` can
    /// load from.
    ///
    /// `size_of` is 208 rather than 200 because `align(16)` pads the tail -- the game reads
    /// `0x00..0xc8` and never sees those eight bytes. The payload is what has to be exact, so that
    /// is what is asserted; asserting `size_of == 200` would only be satisfiable by dropping the
    /// alignment.
    #[test]
    fn the_payload_is_two_hundred_bytes_at_a_sixteen_byte_aligned_address() {
        let request = SpawnRequest::new(&spec()).expect("spellable");
        assert_eq!(request.bytes.len(), req::SIZE, "the block the game reads");
        assert_eq!(core::mem::align_of::<SpawnRequest>(), 16);
        assert_eq!(request.as_ptr() as usize % 16, 0);
        assert!(
            core::mem::size_of::<SpawnRequest>() >= req::SIZE,
            "tail padding may exist; a short block may not"
        );
    }

    /// A bare chr id has to imply the `NpcParam` row whose moveset the shipped table is keyed by,
    /// or a spawn configured the easy way would arrive with a moveset for somebody else.
    ///
    /// Rows are `<chr><4 digits>`: c4500 is row 45,000,000, not 45,000. The four zeroes are the
    /// variant index, and getting the magnitude wrong lands on row 45,000 -- c4's fifth variant,
    /// an entirely different creature -- which `CreateCharacter` would accept without complaint
    /// because an unknown row falls back to row 0 rather than failing.
    #[test]
    fn the_default_param_row_is_the_one_the_moveset_table_is_keyed_by() {
        assert_eq!(SpawnSpec::default_npc_param_id(4500), 45_000_000);
        assert_eq!(SpawnSpec::default_npc_param_id(80), 800_000);
        // ...and the moveset table's own key derivation inverts it, for every id the name format
        // can spell -- including the largest, which must still fit in the i32 the field is.
        for chr_id in [1u32, 80, 4500, MAX_CHR_ID] {
            let row = SpawnSpec::default_npc_param_id(chr_id);
            assert!(row > 0, "c{chr_id:04} overflowed to {row}");
            assert_eq!(u32::try_from(row).expect("positive") / 10_000, chr_id);
        }
    }

    /// Everything outside a named field must stay zero: the block is handed to the game whole, and
    /// a stray byte in an unexamined field is a value nobody chose.
    #[test]
    fn nothing_outside_the_named_fields_is_written() {
        let request = SpawnRequest::new(&spec()).expect("spellable");
        // The gap between `talkId` and `model`, and the tail past the inplace buffer.
        for at in (req::TALK_ID + 4)..req::MODEL {
            assert_eq!(request.bytes[at], 0, "{at:#x}");
        }
        let buffer_end = req::MODEL
            + req::MODEL_BUFFER
            + SpawnSpec::model_name(4500).expect("spellable").len() * 2;
        for at in buffer_end..req::SIZE {
            assert_eq!(request.bytes[at], 0, "{at:#x}");
        }
    }
}
