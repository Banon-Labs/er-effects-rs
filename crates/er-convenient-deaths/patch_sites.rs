// The three patched functions' 1.16.2 addresses, declared exactly once.
//
// `include!`d by both `build.rs`, which needs virtual addresses to assemble instructions
// against the image, and `src/patches.rs`, which needs RVAs to resolve on the running build.
// Neither spells an address of its own.
//
// That is not tidiness. An RVA is a game function's identity, and when one address is written
// out under several names a build-to-build correction has to be found in every one of them --
// `scripts/check-rva-alias-drift.py` exists because missing one crash-hooked this workspace
// before. It caught this crate honestly: the bloodstain writer's window starts at its entry, so
// `build.rs` and `patches.rs` each held their own literal for `0x5fc0c0`.
//
// Each function contributes exactly one address here. Everything else about it -- where its window
// starts, where its branches land -- is a byte offset from that address, so a corrected entry
// moves the whole site and no second literal can be left behind.

// Every constant here carries `#[allow(dead_code)]`, because this file has two includers with
// different needs and each compiles without the other. `build.rs` uses the image base and the
// branch destinations to assemble instructions; `src/patches.rs` uses the entries and window
// offsets to resolve and verify at runtime. Neither sees all of them, so without this the
// unused-const lint fires on whichever half the current compilation unit does not need -- and
// the fix for that must not be to move an address back into the file that uses it.

/// ELDEN RING's preferred image base. The offline images are flat, so `VA = base + RVA` for
/// every section.
#[allow(dead_code)]
const GAME_IMAGE_BASE: u64 = 0x1_4000_0000;

/// 1.16.2 entry of `FUN_1405fc0c0(CSEventBloodStainCtrl *, PlayerIns *, bool)`, the bloodstain
/// writer.
///
/// Ghidra's decompilation is the whole case for patching it: this one function copies the rune
/// count into the bloodstain (`bloodstaidRuneCount = pPVar2->runeCount`) and then clears it
/// (`pPVar2->runeCount = 0`, and `baseHeroPoint2 = 0` beside it). Nothing else in the solo death
/// path moves runes, so returning early keeps them -- and leaves no bloodstain, because the
/// bloodstain is the same write.
#[allow(dead_code)]
const BLOODSTAIN_WRITER_RVA: usize = 0x5fc0c0;

/// The bloodstain writer's window starts at its entry: the first instruction is the
/// `param_2 == null` test this patch overwrites.
#[allow(dead_code)]
const BLOODSTAIN_WRITER_WINDOW_OFFSET: usize = 0;

/// Offset from that entry to the `RET` the null test's `JZ` lands on. Naming it is half the
/// argument for patching the entry to `RET`, so it is a named offset rather than a displacement
/// buried in a jump.
#[allow(dead_code)]
const BLOODSTAIN_WRITER_EMPTY_RETURN_OFFSET: usize = 0x2da;

/// 1.16.2 entry of `FUN_14025e1e0(PlayerGameData *)`, a 19-byte leaf reached from
/// `UpdatePlayerInfo` that reads `if (trigger) { runeArcActive = false; trigger = false; }`.
///
/// Neither image declares it in `.pdata`, so the whole-image function map cannot pair it; its
/// 1.17 row is hand-derived in `docs/recon/rva-map-1162-to-1170.verified.tsv`.
#[allow(dead_code)]
const RUNE_ARC_CLEAR_RVA: usize = 0x25e1e0;

/// Offset from that entry to the `MOV` this patch rewrites: past the `CMP`/`JZ` guarding it.
#[allow(dead_code)]
const RUNE_ARC_CLEAR_WINDOW_OFFSET: usize = 0x9;

/// 1.16.2 entry of `CS::CSLuaEventScriptImitation::CSDeathRestartEvent::SoloPlayDeath`.
#[allow(dead_code)]
const SOLO_PLAY_DEATH_RVA: usize = 0x5a6fb0;

/// Offset from that entry to the fade-out lookup's window, which starts two instructions before
/// the `JZ` this patch rewrites so that it pins an address a lone `74 06` never could.
#[allow(dead_code)]
const SOLO_PLAY_DEATH_WINDOW_OFFSET: usize = 0x11d;

/// Offset from that entry to where the fade-out `JZ` lands: the `XORPS XMM3,XMM3` that supplies
/// `0.0` when the `MenuCommonParam` row is missing.
#[allow(dead_code)]
const SOLO_PLAY_DEATH_FADE_DEFAULT_OFFSET: usize = 0x12b;

/// Offset from that entry to where the taken path rejoins, past the `XORPS`, with the loaded
/// time already in `XMM3`.
#[allow(dead_code)]
const SOLO_PLAY_DEATH_FADE_JOIN_OFFSET: usize = 0x12e;
