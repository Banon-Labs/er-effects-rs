//! Generates each patched window from named `iced-x86` instructions.
//!
//! See `build-support/prologue_build.rs` for why these are generated rather than hand-typed: a
//! window is the only thing standing between a one-byte write and the middle of an unrelated
//! function, so the bytes it compares against must not be a number somebody copied.

#[allow(dead_code)]
mod prologue_build {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../build-support/prologue_build.rs"
    ));
}

use prologue_build::{Image, PrologueSpec, Shape, generate};

const SUPPORT: &str = "../../build-support/prologue_build.rs";
/// The one place any address in this crate is written down. Included rather than duplicated --
/// see its own docs, and `scripts/check-rva-alias-drift.py`, which caught this crate holding two
/// literals for the bloodstain writer.
const SITES: &str = "patch_sites.rs";

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/patch_sites.rs"));

// Each virtual address below is written as `GAME_IMAGE_BASE + (entry + offset) as u64` rather
// than through a helper. A `const fn` reads better and was the first draft, but
// `scripts/rva_symbols.py` evaluates arithmetic and not calls, so every constant built through
// one became unresolved residue -- and residue that wide makes every `.text` address look
// possibly-claimed, which broke that gate's `proven_unclaimed` selftest.

/// `FUN_1405fc0c0` -- the bloodstain writer. Its first instruction is the `param_2 == null`
/// test, so the window starts at the entry.
const BLOODSTAIN_WRITER_VA: u64 =
    GAME_IMAGE_BASE + (BLOODSTAIN_WRITER_RVA + BLOODSTAIN_WRITER_WINDOW_OFFSET) as u64;
/// Where that test's `JZ` lands: the function's own `RET`.
const BLOODSTAIN_WRITER_EMPTY_RETURN_VA: u64 =
    GAME_IMAGE_BASE + (BLOODSTAIN_WRITER_RVA + BLOODSTAIN_WRITER_EMPTY_RETURN_OFFSET) as u64;

/// The `MOV` inside `FUN_14025e1e0(PlayerGameData *)` that clears the rune arc.
///
/// One 16-bit store covers two adjacent fields -- `runeArcActive` at `0xff` and the trigger flag
/// at `0x100` -- which is why the patch changes an immediate rather than removing an instruction.
const RUNE_ARC_CLEAR_VA: u64 =
    GAME_IMAGE_BASE + (RUNE_ARC_CLEAR_RVA + RUNE_ARC_CLEAR_WINDOW_OFFSET) as u64;
/// Offset of `runeArcActive` within `PlayerGameData`. The store is 16-bit, so it also covers
/// `0x100`.
const PLAYER_GAME_DATA_RUNE_ARC_ACTIVE: i64 = 0xff;

/// The `MenuCommonParam` null-check inside `SoloPlayDeath`, at the `MOV` that loads the row
/// pointer -- two instructions before the `JZ` this patch rewrites, so the window pins an address
/// that a lone `74 06` never could.
const SOLO_PLAY_DEATH_FADE_LOOKUP_VA: u64 =
    GAME_IMAGE_BASE + (SOLO_PLAY_DEATH_RVA + SOLO_PLAY_DEATH_WINDOW_OFFSET) as u64;
/// Where that `JZ` lands: the `XORPS XMM3,XMM3` that supplies `0.0` when the row is missing.
const SOLO_PLAY_DEATH_FADE_DEFAULT_VA: u64 =
    GAME_IMAGE_BASE + (SOLO_PLAY_DEATH_RVA + SOLO_PLAY_DEATH_FADE_DEFAULT_OFFSET) as u64;
/// Where the taken path rejoins: past the `XORPS`, with the loaded time already in `XMM3`.
const SOLO_PLAY_DEATH_FADE_JOIN_VA: u64 =
    GAME_IMAGE_BASE + (SOLO_PLAY_DEATH_RVA + SOLO_PLAY_DEATH_FADE_JOIN_OFFSET) as u64;
/// `MenuCommonParam::soloPlayDeath_ToFadeOutTime` sits at the start of the row, so the load is a
/// bare `[RCX]`. Named so the zero is a field offset rather than a bare literal.
const SOLO_PLAY_DEATH_TO_FADE_OUT_TIME: i64 = 0;

fn main() {
    prologue_build::declare_rerun(SUPPORT);
    prologue_build::declare_rerun(SITES);
    generate(
        &[
            (
                PrologueSpec {
                    name: "KEEP_RUNES_WINDOW",
                    doc: "1.16.2 entry of the bloodstain writer:\n\
                          `TEST RDX,RDX; JZ +0x2d1`.\n\
                          The `JZ` is in the window because it is the argument for the patch: the\n\
                          function already returns without touching anything when `RDX` is null.",
                    visibility: "pub(crate)",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: BLOODSTAIN_WRITER_VA,
                    take: 0,
                    pin: &[0x48, 0x85, 0xd2, 0x0f, 0x84, 0xd1, 0x02, 0x00, 0x00],
                },
                (|asm| {
                    use iced_x86::code_asm::*;
                    asm.test(rdx, rdx)?;
                    asm.jz(BLOODSTAIN_WRITER_EMPTY_RETURN_VA)?;
                    Ok(())
                }) as prologue_build::Assemble,
            ),
            (
                PrologueSpec {
                    name: "KEEP_RUNE_ARC_WINDOW",
                    doc: "1.16.2 bytes of the rune-arc clear:\n\
                          `MOV word [RCX+0xff],0`.\n\
                          A 16-bit store over `runeArcActive` and the trigger flag beside it, so\n\
                          the patch raises the immediate's low byte instead of removing the store\n\
                          -- the flag still has to be cleared or the caller re-enters here.",
                    visibility: "pub(crate)",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: RUNE_ARC_CLEAR_VA,
                    take: 0,
                    pin: &[0x66, 0xc7, 0x81, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00],
                },
                (|asm| {
                    use iced_x86::code_asm::*;
                    asm.mov(word_ptr(rcx + PLAYER_GAME_DATA_RUNE_ARC_ACTIVE), 0)?;
                    Ok(())
                }) as prologue_build::Assemble,
            ),
            (
                PrologueSpec {
                    name: "INSTANT_FADE_OUT_WINDOW",
                    doc: "1.16.2 bytes of the fade-out time lookup in `SoloPlayDeath`:\n\
                          `MOV RCX,[RAX]; TEST RCX,RCX; JZ +6; MOVSS XMM3,[RCX]; JMP +3`.\n\
                          Both arms are in the window because they are the argument for the\n\
                          patch: the taken arm is the game's own `0.0` for a missing param row.",
                    visibility: "pub(crate)",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SOLO_PLAY_DEATH_FADE_LOOKUP_VA,
                    take: 0,
                    pin: &[
                        0x48, 0x8b, 0x08, 0x48, 0x85, 0xc9, 0x74, 0x06, 0xf3, 0x0f, 0x10, 0x19,
                        0xeb, 0x03,
                    ],
                },
                (|asm| {
                    use iced_x86::code_asm::*;
                    asm.mov(rcx, qword_ptr(rax))?;
                    asm.test(rcx, rcx)?;
                    asm.jz(SOLO_PLAY_DEATH_FADE_DEFAULT_VA)?;
                    asm.movss(xmm3, dword_ptr(rcx + SOLO_PLAY_DEATH_TO_FADE_OUT_TIME))?;
                    asm.jmp(SOLO_PLAY_DEATH_FADE_JOIN_VA)?;
                    Ok(())
                }) as prologue_build::Assemble,
            ),
        ],
        "generated_convenient_death_windows.rs",
    );
}
