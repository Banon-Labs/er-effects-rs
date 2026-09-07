//! Shared test fixtures: a representative build, and the decoders that reverse the pipeline.
//!
//! The decoders live here and **not** in the crate. `er-build-export` writes links; reading
//! them is `er-build-import-core`'s job, and a decoder shipped alongside the encoder is a decoder
//! that will be quietly kept in agreement with it. These are written from the reference
//! implementation's decompressor, so they are an independent check rather than a mirror --
//! a bug in `lzutf8::compress` has to be a bug in `rotemdan/lzutf8.js` too to slip past them.

use er_build_export::model::{
    Ammo, BuildExportDoc, Flasks, Items, Protectors, Slot, SlotList, SpellList,
};

// `mod common;` is compiled separately into every integration-test binary, and each of them
// uses a different subset of what is here, so every item below is unused in at least one of
// them by construction. Per-item rather than a file-level `#![allow]` so a genuinely orphaned
// helper still shows up.
/// A build with something in every category, modelled on a real shared build.
///
/// Not a minimal fixture on purpose: a link that survives the round trip for an empty
/// document proves almost nothing, because the payload is then short enough to contain no
/// long-form matches, no near-duplicate item names, and no equipped slots.
#[allow(dead_code)]
pub fn representative_build() -> BuildExportDoc {
    let mut doc = BuildExportDoc::with_level(150, false);

    doc.name = "Occult Mage".to_string();
    doc.description =
        "Arcane/Intelligence hybrid built around Great Oracular Bubble and Occult Dryleaf Arts."
            .to_string();
    doc.tags = vec![
        "Intelligence".to_string(),
        "Caster".to_string(),
        "DLC: SotE".to_string(),
    ];

    doc.great_rune = Some("Great Rune of the Unborn".to_string());

    doc.stats.arcane = 47;
    doc.stats.dexterity = 13;
    doc.stats.faith = 7;
    doc.stats.intelligence = 36;
    doc.stats.mind = 23;
    doc.stats.strength = 12;
    doc.stats.vigor = 56;
    doc.stats.endurance = 35;

    doc.inventory = SlotList::new(vec![
        Slot::carried("Albinauric Staff", 0)
            .with_infusion("Standard")
            .with_weapon_art("No Skill"),
        Slot::carried("Mis\u{e9}ricorde", 1)
            .with_infusion("Magic")
            .with_weapon_art("Bloodhound's Step")
            .equipped_at(2),
        Slot::carried("Dryleaf Arts", 2)
            .with_infusion("Occult")
            .with_weapon_art("Palm Blast"),
        Slot::carried("Poisoned Hand", 3).with_infusion("Standard"),
        Slot::carried("Chilling Perfume Bottle", 4)
            .with_infusion("Standard")
            .with_weapon_art("Rolling Sparks"),
        Slot::carried("Star Fist", 5)
            .with_infusion("Magic")
            .with_upgrade(10),
    ]);

    doc.talismans = SlotList::new(vec![
        Slot::carried("Shard of Alexander", 0).equipped_at(1),
        Slot::carried("Godfrey Icon", 1).equipped_at(2),
        Slot::carried("Magic Scorpion Charm", 2),
        Slot::carried("Radagon's Soreseal", 3),
    ]);

    doc.spells = SpellList::new(vec![
        Slot::carried("Great Oracular Bubble", 0),
        Slot::carried("Collapsing Stars", 1),
        Slot::carried("Miriam's Vanishing", 2),
        Slot::carried("Cherishing Fingers", 3),
        Slot::carried("Glintstone Nail", 4),
        Slot::carried("Night Maiden's Mist", 5),
        Slot::carried("Unseen Form", 6),
        Slot::carried("Law of Causality", 7),
        Slot::carried("Bayle's Flame Lightning", 8),
    ]);

    doc.protectors = Protectors {
        head: SlotList::new(vec![
            Slot::carried("High Priest Hat", 0),
            Slot::carried("Silver Tear Mask", 1).equipped_at(1),
            Slot::carried("Mushroom Crown", 2),
        ]),
        body: SlotList::new(vec![Slot::carried("Snow Witch Robe", 0).equipped_at(1)]),
        arms: SlotList::new(vec![Slot::carried("Snow Witch Skirt", 0).equipped_at(1)]),
        legs: SlotList::new(vec![Slot::carried("Sorcerer Leggings", 0).equipped_at(1)]),
    };

    // Ammunition and the assignable surfaces, so the site's own decoder is what proves they
    // survive rather than this repository's opinion of the format. Two arrows and two bolts with
    // distinguishable names, because the interleave (`Arrow1, Bolt1, Arrow2, Bolt2`) is the part
    // that can be wrong while the document still looks right.
    let mut ammo = Ammo::default();
    for (key, name) in [
        ("arrow1", "Bone Arrow"),
        ("bolt1", "Bolt"),
        ("arrow2", "Great Arrow"),
        ("bolt2", "Ballista Bolt"),
    ] {
        assert!(ammo.set(key, name), "{key} is a planner ammo position");
    }

    doc.items = Items {
        ammo,
        // A quickbar position, a pouch position, and a carried tool on neither -- three states,
        // because a fixture that equips everything cannot show an unequipped row surviving.
        tools: SlotList::new(vec![
            Slot::carried("Fingerprint Nostrum", 0).equipped_without_set(0),
            Slot::carried("Bewitching Branch", 1),
            Slot::carried("Blessing of Marika", 2).equipped_without_set(10),
        ]),
        crystal_tears: vec![
            Some("Magic-Shrouding Cracked Tear".to_string()),
            Some("Thorny Cracked Tear".to_string()),
        ],
        flasks: Flasks {
            crimson: 10,
            cerulean: 4,
            total: 14,
            level: 12,
        },
    };

    doc
}

/// Reverse [`er_build_export::to_url_alphabet`].
///
/// The prefix rewrites come last here because they came first there: `uwu`/`UWU` are not
/// base64, so they have to be restored before anything tries to decode.
#[allow(dead_code)]
pub fn from_url_alphabet(payload: &str) -> String {
    let restored = payload.replace('-', "+").replace('_', "/");
    if let Some(rest) = restored.strip_prefix("uwu") {
        return format!("eyI{rest}");
    }
    if let Some(rest) = restored.strip_prefix("UWU") {
        return format!("eyJ{rest}");
    }
    restored
}

/// Decode padded standard base64.
///
/// # Panics
///
/// Panics on any character outside the alphabet, which in a test means the encoder emitted
/// something it should not have.
#[allow(dead_code)]
pub fn base64_decode(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut accumulator = 0u32;
    let mut sextets = 0usize;

    for symbol in text.bytes() {
        if symbol == b'=' {
            break;
        }
        let value = ALPHABET
            .iter()
            .position(|candidate| *candidate == symbol)
            .unwrap_or_else(|| panic!("{:?} is not base64", char::from(symbol)));
        accumulator = (accumulator << 6) | value as u32;
        sextets += 1;
        if sextets == 4 {
            out.push((accumulator >> 16) as u8);
            out.push((accumulator >> 8) as u8);
            out.push(accumulator as u8);
            accumulator = 0;
            sextets = 0;
        }
    }

    // A trailing group of 2 or 3 sextets carries 1 or 2 whole bytes; the leftover bits are
    // padding and are required to be zero.
    match sextets {
        0 => {}
        2 => out.push((accumulator >> 4) as u8),
        3 => {
            out.push((accumulator >> 10) as u8);
            out.push((accumulator >> 2) as u8);
        }
        other => panic!("base64 length leaves {other} dangling sextets"),
    }

    out
}

/// Decompress one LZ-UTF8 block.
///
/// A direct transcription of the reference `Decompressor.decompressBlock`, including its
/// literal/match dispatch: a byte `>= 0xC0` is a match header only when the byte after it has
/// its top bit clear.
///
/// The reference's trailing-partial-sequence rollback is deliberately not reproduced. It
/// exists to hold back a multi-byte character split across two blocks, and this crate emits
/// exactly one block; if a stream ever ended mid-sequence here, that would be the bug, and
/// silently trimming it is how the test would fail to notice.
///
/// # Panics
///
/// Panics on a truncated header or an out-of-range distance -- both of which mean the encoder
/// produced an invalid stream, which is what these tests are for.
#[allow(dead_code)]
pub fn lzutf8_decompress(stream: &[u8]) -> Vec<u8> {
    const MATCH_HEADER_MASK: u8 = 0b1100_0000;
    const LONG_MATCH_HEADER: u8 = 0b1110_0000;
    const LENGTH_MASK: u8 = 0b0001_1111;

    let mut out: Vec<u8> = Vec::with_capacity(stream.len() * 4);
    let mut index = 0usize;

    while index < stream.len() {
        let header = stream[index];

        // Anything below the lead-byte range is unambiguously a literal.
        if header & MATCH_HEADER_MASK != MATCH_HEADER_MASK {
            out.push(header);
            index += 1;
            continue;
        }

        // Neither reading is possible for a trailing byte: a match header is always written
        // with its distance, and valid UTF-8 never ends on a lead byte. Either way, a bug.
        assert!(
            index + 1 < stream.len(),
            "stream ends on a lead byte at {index}"
        );

        // A UTF-8 continuation byte after a lead byte means the lead byte was a literal.
        if stream[index + 1] >= 0x80 {
            out.push(header);
            index += 1;
            continue;
        }

        let long_form = header >= LONG_MATCH_HEADER;
        let needed = if long_form { 3 } else { 2 };
        assert!(
            index + needed <= stream.len(),
            "truncated match header at {index}"
        );

        let length = usize::from(header & LENGTH_MASK);
        let distance = if long_form {
            (usize::from(stream[index + 1]) << 8) | usize::from(stream[index + 2])
        } else {
            usize::from(stream[index + 1])
        };
        index += needed;

        assert!(
            (1..=out.len()).contains(&distance),
            "match at {index} reaches {distance} back into {} bytes",
            out.len()
        );
        let start = out.len() - distance;
        for offset in 0..length {
            // Read one at a time: an overlapping match legitimately reads bytes this very
            // loop just wrote.
            let byte = out[start + offset];
            out.push(byte);
        }
    }

    out
}

/// Reverse the whole pipeline, from `?i=` payload back to the character JSON.
///
/// # Panics
///
/// Panics when any stage produces something the next cannot read.
#[allow(dead_code)]
pub fn decode_payload(payload: &str) -> String {
    let compressed = base64_decode(&from_url_alphabet(payload));
    let inner = lzutf8_decompress(&compressed);
    let inner = String::from_utf8(inner).expect("the compressed text was base64, so it is UTF-8");
    let restored = inner
        .replace('\u{1}', "},{")
        .replace('\u{5}', "[{")
        .replace('\u{6}', "}]");
    let json = base64_decode(&restored);
    String::from_utf8(json).expect("the document json is ASCII by construction")
}
