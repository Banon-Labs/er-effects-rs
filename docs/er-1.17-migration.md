# ELDEN RING 1.17 migration status

## The installed game is 1.17.1 since 2026-09-08, not 1.17.0

Everything below this section describes the 1.16.2 to **1.17.0** gap, and that is still the hard
part -- 1.17.0 moved code by an amount that changes over spans as short as 0xb00 bytes, which is
why the mapper hunts masked signatures. Read it with one correction applied: the game on this
machine is now `FileVersion` **2.7.1.0**, and 1.17.0 no longer exists here, because Steam
overwrote it.

The 1.17.0 to 1.17.1 step is nothing like the one below it. It is a single constant, read
exhaustively out of both de-Arxan'd images' `.pdata`:

* every function entry **below rva `0xafefe9`** keeps its address, and
* every one of the **174,389** entries at or above it moves by exactly **`+0x70`**, with none left
  over and none needing a signature match.

One function grew -- `0xafeea0`, from 329 bytes to 441 -- and everything downstream slid by the
difference. One other changed without moving anything: `0x82dc20` grew `0x126` to `0x12e` out of
its own alignment padding, so its address is unchanged and its body is not. Nothing else moved at
all: the section table is byte-identical between the two builds, so `.rdata`, `.data` and `.pdata`
keep their addresses, and `u"%s/EldenRing/%s/"` is at rva `0x2bdd9b8` in both. The shift is
therefore bounded to the primary `.text`; applied to anything above the boundary it would move
every vtable and every global, which did not move. Only the Arxan section at `0x4c13000` changed
size (`+0x4400`), and it re-randomises per build, so nothing in it carries forward.

```bash
python3 scripts/map-rvas-1170-to-1171.py 0x140b00000     # one address, or a whole bundle
python3 scripts/map-rvas-1170-to-1171.py --selftest      # re-derives the model from both images
python3 scripts/audit-1171-stale-constants.py            # product constants the shift invalidated
```

**Do not diff the two images byte-for-byte to check this.** 8.87% of `.text` differs even between
the two de-Arxan'd images: Arxan re-randomises its inline obfuscation every build and `dearxan`
neutralises stubs rather than un-mutating bodies. `.pdata` is the structural evidence, and
`dearxan` does not rewrite it. Comparing a raw `.exe` against a deobf image is worse again -- that
is deobfuscated against obfuscated, and it reports ~50% of functions as changed.

What has been carried forward, and what has not:

| | state |
| --- | --- |
| `eldenring-deobf-1.17.1.bin` | generated (1597 stubs, all ok; 1347 regions; 88,875 bytes -- the same shape as 1.17.0, so no new obfuscation technique). Gitignored, like every other game image |
| `fromsoftware-rs` RVA bundle | `rva_ww_271.rs` generated from `rva_ww_270.rs` by the mapper and wired to a `(LANG_ID_EN, "2.7.1.0")` arm. 5 of its 96 fields moved; the rest are below the boundary or are `.rdata`/`.data`. `scripts/check-game-version-supported.py` passes |
| `er-game-base` address translation | carries the map's 1.17.0 destinations onto the running build, and now **refuses every game address on a build it has not been told about** rather than handing out 1.17.0 addresses to whatever is installed, which is what it did before 1.17.1 shipped |
| Constants spelled at 1.17.0 addresses | **nothing to repair.** Exactly three `PrologueSpec`s carry `image: Image::EldenRing1170` -- the only addresses that bypass the map -- and all three are below the boundary, so the patch did not move any of them: `SAVE_REQUEST_RETRACT_B72_SIG` `0x140679590`, `SAVE_REQUEST_RETRACT_B73_SIG` `0x140679560`, `QUIT_PHASE_SETTLE_SIG` `0x14067b7d0`. `scripts/audit-1171-stale-constants.py` re-derives that. Two earlier answers here were wrong and are worth knowing about, because both look convincing: classifying by FILE said 13 constants were stale, which is what happens when a `build.rs` holding one 1.17.0 spec and twenty 1.16.2 ones is read as a 1.17.0 file; classifying by ledger membership missed the real ones, because an address can be a function start in both builds at once. The `image` field of the spec is the only thing that settles it |

ELDEN RING updated to **1.17** on 2026-08-27 (PE `FileVersion` 2.7.0.0, Steam buildid 23850278).
Every game address in this workspace was reverse-engineered against **1.16.2** (2.6.2.0). This file
is the state of that gap: what has been repaired, what is stale, and what each remaining item is
blocked on. Update it as items close rather than starting a second list somewhere else.

## What already works on 1.17

| Component | Evidence |
| --- | --- |
| me3 0.11.0 + dearxan | A zero-native profile boots 1.17 normally; the loader was never the problem. |
| Seamless Co-op v2.0.0 | The only supported Seamless build (2026-09-03). Whether it boots 1.17 unaided is NOT established here -- the AOB strings are packed in the shipped DLL, so a static check cannot answer it, and the decisive evidence is a run. What IS established: nothing in this repo can help it, which supersedes the `er-ersc-sigshim` route, which rebuilt two AOB landmarks so that v1.9.9 could start on 1.17 -- that crate was RETIRED 2026-09-03 when the mod dropped support for older Seamless builds, and it could never have fired on v2.0.0 anyway: it refused any build other than the one its fixups were measured against. |
| The build gate (`er-hook` + `er-game-base::game_build`) | 51 game-image detours refuse with a logged reason instead of corrupting the image; boot survives. |
| Win32 / DirectInput detours | Resolved through `GetProcAddress`, never version-sensitive. 15 install normally. |
| `eldenring-deobf-1.17.bin` | Generated offline by dearxan (1597 stubs, 1371 decrypted regions). Bit-reproducible from the installed `eldenring.exe`, and 99.909% of it is byte-identical to that executable -- every one of the 89,309 changed bytes lies inside a region dearxan declared, and no byte outside one moved. See "Did 1.17 add a new obfuscation technique" below. |
| `docs/recon/rva-map-1162-to-1170.tsv` | 32 of the 51 refused addresses carried forward with evidence; the other 19 are named, not guessed. |
| 27 translated detour targets | Three independent passes agree: signature re-occurrence, normalised instruction comparison, and `scripts/audit-1170-hook-targets.py`, which finds every one is a real function entry carrying the SAME reference profile it had in 1.16.2 (same call count, same kind). The control is decisive -- 24 of the 27 STALE addresses have zero references in the 1.17 image. |

Two of those rows carry counts from a specific run rather than from a file, so they date:
the 51 refused detours and the 15 Win32 detours were counted out of one 2026-08-28 launch log.
The 51/32/19 split is still reproducible offline -- `docs/recon/rva-map-1162-to-1170.tsv` holds
exactly 51 rows of which 19 are `UNRESOLVED` (re-counted 2026-08-31) -- but the ledgers have grown
far past it since (`needed.tsv` 412 rows, `needed-verified.tsv` 411, `verified.tsv` 102), so do not
read 51 as the size of the detour set today. `me3 0.11.0` is confirmed current on PATH.

## What is still stale

This table is re-measured, not remembered. Every row carries the command that produced its
number, so the next agent can find out it has moved instead of trusting a date. Audited
2026-08-31; six rows were false and are corrected below.

| # | Item | State | Blocked on |
| --- | --- | --- | --- |
| 1 | ~~Ghidra dump / MCP is 1.16.2 only~~ **RESOLVED 2026-08-30** | A 1.17 dump is imported as `ermaporch1170` and served on **:8767**, alongside 1.16.2 on :8765. Verified 2026-08-31: `getContext` returns `pc_eldenring_runtime.1.17.0.exe`, 366,673 functions, against 367,183 on :8765. It carries **zero curated symbols** -- `searchFunctionsByName` (the parameter is `query`, not `searchTerm`) totals 1.16.2 vs 1.17: Scadutree 5/0, CSFeManImp 3/0, MoveMap 23/0, FreeList 6/0, TitleTopDialog 1/0. Everything is `FUN_<addr>` | nothing for STRUCTURE. **Names, types and RTTI are still 1.16.2-only** and must be carried across by pairing -- that is the residue, and it is a different problem from "there is no dump" |
| 2 | 19 unresolved addresses in the RVA map | Still 19, of 51 rows in `docs/recon/rva-map-1162-to-1170.tsv` (re-counted 2026-08-31). Shape-matched but ambiguous, deliberately left blank | hand RE per address. No longer blocked on #1: :8767 gives the call graph these need |
| 3 | ~~1 mapping refused on evidence: `MOVEMAPSTEP_STEP_MOVEMAP_RVA`~~ **RESOLVED 2026-08-31** | `PATCH-SITE-IDENTICAL`, in both `VERIFIED_1162_TO_1170` and `DETOUR_SAFE_1162_TO_1170`. The two inserted instructions are `mov rcx,rbx; call CS::MoveMapStep::_UpdateHorseType` at index 873 of 975, 0x1055 bytes past a prologue that is `48 8b c4 55 56 57 41 54` in both builds -- see "Function lengths" below | nothing; pinned in `PATCH_SITE_ACKNOWLEDGED` (`er-game-base/build.rs`) so the next insertion fails the build |
| 4 | Struct layouts | two confirmed drifts: `PlayerGameData` +8 (`+0xab5` -> `+0xabd`, corroborated by the retired `er-ersc-sigshim` fixups, whose Scadutree rewrite depended on it), the Wwise settings object +0x38. The rest is unaudited | hand RE. :8767 does NOT close this: it has structure, not types -- a field name or a struct layout still only exists on :8765, for the previous build |
| 5 | `fromsoftware-rs` bindings (path dependency) | field offsets are 1.16.2-shaped. The RVA half IS done: `rva_ww_270.rs` exists with 96 fields and **zero** of them are `0` (checked 2026-08-31 -- see the zero-field trap below), and `scripts/check-game-version-supported.py` passes: `installed game 2.7.0.0 is in the RVA bundle's supported set ['2.6.2.0', '2.6.2.1', '2.7.0.0']` | #4 |
| 6 | Generated prologue windows (`build.rs` + `check-prologue-bytes`) | mostly fine: the sweep that found exactly ONE breakage, `er-save-suppress::QUIT_PHASE_SETTLE_SIG` (now respelled), covered **18 specs in `er-quickload/build.rs`** -- not the whole set. Re-counted 2026-09-08: **42 `PrologueSpec` sites** across five `build.rs` files (er-quickload 18, er-save-suppress 11, er-invasion-warp 6, er-seamless-bugfixes 4, er-player-name-filter 3). The 84 recorded here on 2026-08-31 was a double-count, not drift: the same five files held 42 at `1427730` too, and every one of the six numbers was exactly twice the real one. `Image::EldenRing1170` is the `image` of **3** specs, not 4 -- the fourth occurrence of that name is a doc-comment reference at `crates/er-quickload/build.rs:32`. All three sit below the 1.17.1 boundary and none of them moved (`scripts/audit-1171-stale-constants.py`). The rest are register-only prologues whose encoding is version-invariant | re-running the sweep over all 84, plus 5 specs whose 1.16.2 RVA is in no map |
| 7 | `dump-exec.bin` + `scripts/dump-deobf-shift.py` | **RETIRED. Do not run it.** Its dump side is 1.16.1, so it maps 1.16.1-dump onto a 1.16.2 image whose real shift is zero, and it invents a nonzero one. Re-verified 2026-08-31 against `.pdata`: `0x142413860` IS a function start in `eldenring-deobf.bin`, and the `+0x10` answer `0x142413870` is 16 bytes into its prologue; `0x142410830`, which the tool flagged as a "+0x10 estimate", is also already a function start. Both of its published answers land mid-instruction | nothing. Use `map-rvas-1162-to-1170.py`, or read :8767 directly |
| 8 | `regulation.bin`, `data/effects.json`, `effect-master-catalog.json` | 1.17 shipped new params; row ids unverified | re-validate with `tools/er-param-inspect` |
| 9 | Save containers / `ProfileSummary` reader | RVA-stale; whether the format itself changed is unknown | a save-format diff. Not blocked on #1 any more |
| 10 | ~~160 game addresses CALLED without resolving~~ **RESOLVED** | `python3 scripts/check-stale-rva-calls.py --list` prints `0 TOTAL across 0 crates` (run 2026-08-31), and `scripts/audit-1170-readiness.py` agrees per-cdylib: **ungated EXEC 0, WRITE 0** across all 27. The number 160 in this row was a snapshot that outlived the work; the sections below already said 0 while this row said 160 | nothing. The remaining bucket is 136 ungated read/compare sites, which are fault-safe by construction |
| 11 | ~~4 byte-patch stub sites all REFUSE~~ **3 of 4 now apply** | Byte-verified 2026-08-31 against `eldenring-deobf-1.17.bin`. `patch_3byte_stub`/`apply_xor_ret_stub` resolve through `resolve_game_address` BEFORE the opcode check, so a mapped site reaches its 1.17 destination: `menu-online-mode` `0xe56310 -> 0xe58110` (byte `0x40` = expected), `signin-force` `0x24129b0 -> 0x24151c0` (`0x40`), `userindex-force` `0x240f490 -> 0x2411ca0` (`0x4c`) -- all three match and are written. Only `online-disable` `0x67a030` has no row in `needed.tsv` and still refuses. Reading this row as "all four are inert" sends you to re-RE three functions that already work | re-RE `ONLINE_DISABLE_RVA` (`0x67a030`) on 1.17, and nothing else |
| 12 | ~2.7k `bd` memories carrying 1.16.2 RVAs | 2704 memories total (`bd memories`, 2026-08-31). Correct for the build they were written against, silently wrong now. Worse than 1.16.2-scoped in places: several predate even that and carry **1.16.1-dump** addresses stated as fact | nothing -- treat every RVA memory as scoped to a build it does not name, and re-verify before use |

## How to carry an address forward

```bash
# One address, or the whole work list straight out of a runtime log.
uv run --with capstone python3 scripts/map-rvas-1162-to-1170.py 0x1407ada40
uv run --with capstone python3 scripts/map-rvas-1162-to-1170.py \
    --from-refusal-log "<game dir>/er-quickload-autoload-debug.log" \
    --tsv docs/recon/rva-map-1162-to-1170.tsv
uv run --with capstone python3 scripts/map-rvas-1162-to-1170.py --selftest
```

The mapper reports how each answer was reached, and the distinction matters:

* `unique, NNB signature` -- one masked match in the whole image. Strong.
* `nearest-anchor delta (unique)` -- the shape occurs more than once and the winner was chosen
  because its delta matches the closest address THIS RUN mapped uniquely. Weaker; the shift
  changes over spans as short as 0xb00 bytes, so an anchor further away than that is not evidence.
* `nearest-anchor delta (ledger)` -- same, but the delta came from a pair in
  `docs/recon/rva-map-1162-to-1170.verified.tsv`, which has been compared instruction by
  instruction over its whole body. Better evidence than a same-run byte match, and available even
  when only one address is being mapped.
* `UNRESOLVED` -- shape matches exist but none agrees with the local delta. Left blank on purpose:
  a blank costs a Ghidra lookup, a wrong address costs a mid-function detour and a dead boot.

**Anchors are why a region resolves, and the ledger is where they live** (2026-09-01,
`er-effects-rs-4uw5.13`). Pass 2 used to draw anchors only from other addresses in the same
invocation, so mapping ONE address could never use one. It now reads the verified ledger as well.
Measured over every `.pdata` entry of three regions, one address at a time
(`scripts/measure-1170-anchor-coverage.py`, which reproduces this for any range):

| region | before | after | what changed |
| --- | --- | --- | --- |
| `0x87xxxx` | 67/278 (24.1%) | 271/278 (97.5%) | four anchor rows added; the region had none |
| `0x92xxxx` | 37/501 (7.4%) | 355/501 (70.9%) | four anchor rows added; the region had none |
| `0x9axxxx` | 24/342 (7.0%) | 265/342 (77.5%) | no new rows -- six were already there, unread |
| `0x964000-0x969000` | 7/102 (6.9%) | 82/102 (80.4%) | 2026-09-10, one anchor `0x1409650a0 -> 0x140966240`; the region had none |
| `0x999000-0x99e000` | 51/115 (44.3%) | 87/115 (75.7%) | 2026-09-10, one anchor `0x140999070 -> 0x14099a210`; the region had none of its own, only 0x9axxxx rows reaching in |

The last two rows were added carrying the Quit-panel portrait pair, and the greedy search is why
each is a single row: `--suggest` named one anchor per region that resolved 75 and 36 further
entries respectively, and no second one resolved anything. Both are unique masked-signature matches
needing no anchor themselves, and both agree at `+0x11a0` with the pair they were cut for.

The residue is not short of anchors, and a greedy search over every address the matcher resolves
alone confirms it: in the first three regions, no further anchor resolves even one more entry. 227 of the
230 remaining addresses produce a masked signature matching the `CANDIDATE_CEILING` of 2048 places
-- compiler-generated unwind funclets and vtable stubs, whose shape is the compiler's rather than
the function's. The mapper refuses those outright. It could resolve them from the region delta
alone, and deliberately does not: when a shape occurs thousands of times, "the bytes at the
predicted address have that shape too" is nearly free and would corroborate nothing while appearing
to. `docs/recon/rva-map-1162-to-1170.functions.tsv` pairs by region delta and says so in its own
header; that is where an address of this shape belongs.

`--selftest` asserts three things: that the mapper re-derives the two mappings this repo
established by hand from the live 1.17 process (`GetScadutreeBlessing` and the `GetWwiseSettings`
allocator landmark); that `0x1409a5810` is the 39th of 51 shape matches for `0x1409a4670`, which is
why the candidate list is no longer cut at 9; and that the four ProfileSelect/System-Quit addresses
resolve ONE AT A TIME, which they can only do from the ledger's anchor rows. A matcher that cannot
reproduce a known answer has no business proposing an unknown one.

A pair headed for the ledger gets a second opinion that shares none of the above machinery:

```bash
# Both Ghidra dumps, asked whether each half is a declared function ENTRY and whether the
# call graph carries across. Skips at exit 0 if either daemon is down.
python3 scripts/confirm-1170-pair-in-dumps.py 0x1409a4670:0x1409a5810
python3 scripts/confirm-1170-pair-in-dumps.py --selftest
```

## Three private address helpers that looked gated and were not

Each of these resolved a game address for callers that transmute and CALL it, and none checked
anything. They are fixed; they are recorded because the shape recurs -- a helper whose NAME or doc
comment implies a check, wrapping a bare addition.

| Where | What it claimed | What it did |
| --- | --- | --- |
| `er-save-loader` `load_methods::game_rva` | nothing, but it is the crate's address resolver and its results are transmuted | rejected a null base, then added |
| `er-invasion-path` `sfx::function` | "refusing anything that is not inside the game image" in its own doc comment | added |
| `er-build-import-runtime` `catalog` getters | `name_of`'s safety comment: "the caller guarantees the address is one of the verified getters" | added |

All three now go through `er_game_base::game_build::resolve_game_address`, so the doc comments are
true and an unresolvable address costs a feature rather than the process.

## What a "verified" address does and does not buy

Three questions, and it is worth keeping them apart, because each was answered by a different pass
and conflating them is how a wrong address gets installed with confidence:

| Question | Answered by | What it cannot see |
| --- | --- | --- |
| Where did this function move to? | `scripts/map-rvas-1162-to-1170.py` -- masked signature re-occurrence | whether the recurrence is the function or an inlined copy of its prologue |
| Is the code there the same function? | `scripts/verify-rva-map-1170.py` -- normalised instruction sequences | what the functions it CALLS now do |
| Is it a function ENTRY, and is it safe to patch? | `scripts/audit-1170-hook-targets.py` -- references the image makes to it, plus MinHook's patch window | whether OUR HANDLER is still right |

The last column of the last row is the standing gap: a handler reads struct fields and calls
virtuals by slot, and 1.17 moved plenty of both. No offline pass can close it. Only a fatal record
naming the handler can, which is what `docs/recon/rva-1170-quarantine.tsv` is for and why it is
still empty -- across every run of 2026-08-28 the unhandled-exception filter fired zero times.

A note on how the audit's entry check was built, because it is the reusable part: the first version
decoded BACKWARDS from the address looking for int3 padding or a terminator. Run against the 1.16.2
image at the 27 addresses this project has hooked successfully for months, it called 20 of them
mid-function. Backward decoding desynchronises, and a de-Arxan'd image does not carry the padding
it assumed. The replacement uses forward-derived evidence -- calls, jumps and pointers the image
makes TO the address -- and passes 27/27 on that same known-good input while still rejecting a
deliberate mid-function control. Calibrate a new check on data whose answer you already know
before you point it at data you do not.

## The sibling checkout is part of this migration, and git does not know it

`eldenring::rva::get()` in `../fromsoftware-rs` resolves a `LazyLock<RvaBundle>`
through `ERGameVersion::detect()`, and until 2026-08-29 that function accepted
exactly two strings: `"2.6.2.0"` and `"2.6.2.1"`. The installed game is
`2.7.0.0`, so it took the `.unwrap_or_else(|e| panic!("{e}"))` arm.

That one line of data was the whole 1.17 boot death. **Eight** product DLLs --
er-quickload, er-invasion-warp, er-inventory-sort, er-refill-all,
er-enemynpc-effects, er-telemetry, er-invasion-path, er-net-effects -- each threw
a `rust_panic` within five milliseconds of each other at
`ms_since_install=869..898`, each on its own thread, each with an identical
frame shape. The panic happens inside a `LazyLock`, on whichever thread first
touches any singleton, so nothing logged the message and nothing named the
DLL: the crash record says `cpp_throw_type=rust_panic` and stops. It cost
eight game launches to find, and the shape of it -- "a companion DLL is
involved, because er-quickload alone gets further" -- was a red herring
produced by nothing more than which thread happened to touch a singleton
first.

The fix is three edits in the sibling checkout:

| file | change |
|---|---|
| `crates/eldenring/mapper-profile.toml` | `title_step_state_table`'s pattern, see below |
| `crates/eldenring/src/rva/rva_ww_270.rs` | new, generated, 96 RVAs |
| `crates/eldenring/src/rva.rs` | `Ww270` variant accepting `(LANG_ID_EN, "2.7.0.0")` |

Regenerate the bundle with the project's own generator rather than by diffing
addresses. 62 of the 96 fields are vtables resolved by RTTI class name, which
is version-independent and needs no work at all; the rest come from AOB
patterns. For comparison, `scripts/map-rvas-1162-to-1170.py` resolved 2 of the
first 5 code addresses -- byte-diffing is the wrong tool for a whole bundle.

```bash
cd ../fromsoftware-rs
# `windows-future` cannot build for a Linux target at all, so the generator is
# cross-compiled and run under wine rather than built natively.
cargo xwin build --release -p binary-mapper --target x86_64-pc-windows-msvc
WINEDEBUG=-all wine target/x86_64-pc-windows-msvc/release/binary-mapper.exe map \
  --profile 'Z:\home\banon\projects\fromsoftware-rs\crates\eldenring\mapper-profile.toml' \
  --exe 'Z:\home\banon\.local\share\Steam\steamapps\common\ELDEN RING\Game\eldenring.exe' \
  --output rust
```

**Check every field for zero before believing the output.** `MapperProfilePattern::find`
records a miss as RVA `0`, prints no warning, and `0` resolves at runtime to the
PE header -- a silent wrong answer that looks like a successful generation.
On the first 1.17 run exactly one field came back zero, `title_step_state_table`,
because 1.17 emits a `nop` between the `call` and the vftable store that the
pattern's fixed `[4]` skip could not cross. Two things about the replacement
are worth keeping: the `mov byte [rdi+0xb8], 0` suffix is load-bearing, since
without it fifteen sibling state-machine constructors match on both versions
and `find` silently takes the first; and pelite 0.10's range skip `[4-5]`
parses fine but does not match here, so the pattern uses an alternation
`( 90 48 8d 05 | 48 8d 05 )` instead. The result is calibrated rather than
fitted: on 2.6.2.0 the new pattern still yields `0x3d71580`, the value already
checked in.

`scripts/check-game-version-supported.py` is the gate that makes this
discoverable without a game. It reads the PE product version off
`eldenring.exe`, reads the accepted versions out of the sibling's `rva.rs`, and
fails when they disagree -- printing the exact panic text the mismatch will
produce. It skips, rather than fails, when either side is absent.

None of the sibling's edits are tracked by this repository. A fresh clone of
`fromsoftware-rs` reintroduces the boot death, and the gate above is what will
say so.

## The order to do the rest in

1. ~~Capture a 1.17 runtime dump and stand up `ermaporch1170`~~ **DONE 2026-08-30, serving on :8767.**
   It did NOT reduce items 2, 4, 5 and 9 to lookups the way this step predicted, and the reason is
   worth keeping: the dump has structure and no semantics. It answers "is this a function, what
   calls it, how long is it" and cannot answer "what is this field" -- so the 19 blanks became
   answerable and the struct work did not.
2. Verify and re-point addresses feature by feature, cheapest first: each one that lands turns
   a `HOOK REFUSED` line back into a working feature, and the gate keeps the rest safe meanwhile.
3. Flip `eldenring-deobf.bin` to the 1.17 image and regenerate the prologue windows (#6) in one
   commit, once enough addresses are re-pointed that the gates are meaningful again.
4. Re-validate the param/save data (#8, #9), which is the only part that can change what a player
   sees without any address being involved.

## The wedge: er-quickload kills the game's main thread (CLOSED 2026-08-29; kept for method)

**This section is history, not a work item.** It is marked here because it opened with `(open, 2026-08-29)`
for two days after the bug was fixed, and an open header is an instruction. The cause was a stale 1.16.2
address executing on 1.17; the fix was the address gate; the proof is two sections down -- er_quickload
boots with zero fault lines and 792 refusals. Read the rest for the measurement technique, not for a lead.

The crash classes are closed -- the full eighteen-DLL profile loads with zero panics and three
detour refusals -- and what is left is not a crash. With `er_quickload.dll` loaded, ELDEN RING
1.17's **initial thread exits** a few seconds into boot. `/proc/<pid>/stat` shows the thread-group
leader in state `Z` while ~81 workers stay alive and asleep, so the process lingers as a husk with
a mapped window and ~0 CPU until it finally goes away. That husk is the "black screen".

### What is measured, and what it rules out

| profile | result |
|---|---|
| no natives at all | 112 threads, 784 CPU ticks/3s -- healthy |
| `ersc.dll` + `er_ersc_sigshim` | 106 threads, ~350 ticks/3s -- healthy, so **Seamless is not it** |
| the same plus `er_quickload.dll` | leader `Z`, workers idle -- **wedged** |

Both healthy profiles and the wedged one were launched the same way, through `~/Elden/launch.sh`
with hand-written profiles in `~/Elden/` (`control-no-natives.me3`, `control-ersc-only.me3`,
`control-quickload-only.me3`). That matters: for eight runs every death was staged by
`scripts/er-run-branch.py` and every survival came from `launch.sh`, so the launcher and the DLL
set were changing together and neither could be blamed. The third profile broke the tie and
exonerated the runner.

### Six hypotheses, each falsified by its own launch -- do not re-test these

1. the boot pump's foreign-thread self-present (`self_present_safe` forced false)
2. the early foreign-thread window move (`apply_startup_window_final_geometry`)
3. the swapchain VMT swap (`try_install_game_present_hook` made a no-op)
4. our ~200 game-image detours (`DETOUR_SAFE_1162_TO_1170` emptied in `er-game-base/build.rs`)
5. `er-crash-logging`'s vectored handler (excluded from the profile)
6. `scripts/er-stale-run-sentinel.sh` -- its log records `killed=-` on every line; it never fired

### The trap that cost four stack captures

`/proc/<pid>/mem` **is** `/proc/<pid>/task/<leader>/mem`, so a zombie leader makes it unopenable:
the open fails with `ESRCH` "No such process" while the process is alive and burning 195 ticks per
three seconds. Read `/proc/<pid>/task/<live-tid>/mem` instead. Both
`scripts/er-wedge-stacks.py` and `scripts/wine-thread-death-watch.py` now do, and the former
prints the leader state as its headline so the state is never inferred from a failed read again.

### Where it stands, and the next step

`scripts/wine-thread-death-watch.py` caught the death: over its last 400 samples the initial
thread was in state **R -- running in userspace -- for 368 of them**, CPU still climbing, and then
it was gone. Not blocked, and no fatal record from the vectored handler in the runs that carried
one. A thread that runs hot and then exits cleanly points at a deliberate thread termination, not
a fault.

### The death site, captured

It is a **SIGSEGV inside `er_quickload.dll`**, and Wine aborts the thread because it cannot even
build the exception frame -- which is why no vectored handler ever recorded it. `abort_thread`
bypasses the SEH/VEH path entirely.

```
#0 abort_thread            (ntdll.so)
#1 virtual_setup_exception (ntdll.so)   <- could not build the exception frame
#2 setup_raise_exception   (ntdll.so)
#3 segv_handler            (ntdll.so)
#4 <signal handler called>
#5 0x6ffff9cd2326 in er_quickload.dll
#6 0x0
```

The PE stack scan at the abort (module base `0x6ffff9cd0000`) names the shape:

| rva | times on the stack | symbol |
|---|---|---|
| `0x2326` | 3 | none -- below where Rust code starts, so a linker import/jump thunk |
| `0x1cf0c0` | 2 | `core::fmt::num::impl$8::fmt` |
| `0x392fd8` | 1 | none |

Alternating repeated frames, plus `virtual_setup_exception` failing, is **recursion into stack
overflow** -- not a wild pointer. That much held.

**The explanation offered next to it did not, and the follow-up it prescribed was deleted rather
than left standing.** It read: an import thunk with `core::fmt` beside it means a log write
re-entering a hooked Win32 file call, so disassemble `er_quickload.dll` at rva `0x2326` and read
that hook's re-entrancy guard. The re-entrancy hypothesis was implemented and falsified by its own
oracle in the very next section -- the descent reproduced with `oracle_veh_reentrant_refusals = 0`,
and gdb counted 218 independent top-level raises. The recursion is in Wine's exception dispatch,
and the thing that STARTED it was a stale address executing. Anyone who followed the deleted
instruction would have spent the day reading a guard that was already correct.

### How to reproduce the capture

This is the part that cost the most, so it is written down rather than rediscovered:

* me3 has `--suspend` ("Suspend the game until a debugger is attached"). Pass it through the user
  launcher as `ME3_PROFILE=... bash ~/Elden/launch.sh -s -- --suspend`. **The bare `--` matters**;
  without it `getopts` eats `--suspend` and the script just prints its usage.
* Then attach `python3 scripts/wine-thread-death-watch.py --gdb --max-seconds 240 --out <file>`.
  Attaching after the fact cannot work -- gdb refuses a zombie leader -- and without `--suspend`
  the thread dies within about six seconds, far too early to attach by hand.
* Run the watcher under `setsid`/`nohup`: an agent tool call is killed at two minutes and the
  capture needs longer than that.
* Under gdb the death is DELAYED, because every breakpoint trap slows the process down. An 80
  second window was not enough and 240 was.

## Which DLL actually breaks: a five-profile split (2026-08-29)

The boot death was being blamed on "the mod set". It is one DLL, and the crash log named the
fault all along -- 216 `access-violation` lines of which exactly ONE matters:

| | fault |
|---|---|
| 1 x | EXECUTE at `0x3cb67a0` -- a heap object whose vtable is game rva `0x2b6d728`, RTTI `CS::CSFadeImp` |
| 215 x | identical NULL read at `ntdll+0x3969c`, `rsp` marching DOWN `0x1260` a line, `0x2fc50` -> `0x13a50` |

4704 bytes a level against a 1 MiB stack is dead after ~220 levels, and `MAX_AV_LOG_LINES` is 256,
so the line budget could never stop it. The thread then takes a SIGSEGV Wine cannot report --
`virtual_setup_exception` needs stack to build the exception frame, has none, and calls
`abort_thread` -- which is why the process died with no fatal record and no minidump.

**The amplifier is NOT our handler re-entering itself.** That hypothesis was implemented
(`er_game_base::reentry::ReentryLatch` on both `crash_vectored_handler`s) and falsified by its own
oracle in one run: the descent reproduced exactly with `oracle_veh_reentrant_refusals = 0`. A gdb
catch on `KiUserExceptionDispatcher` then counted **218 raises on the initial thread with `rsp`
descending a constant `0x12c0` each** -- every one a fresh top-level raise, our previous invocation
already returned. The recursion is in Wine's exception dispatch. The latch stays as correct
defensive code for a real hazard; it is not this bug.

### The split -- nine launches

Each row is one launch of one `.me3` profile through `~/Elden/launch.sh -s`, watching
`/proc/<pid>/stat` field 3 for the thread-group leader.

| profile | crates | outcome | fault signature |
|---|---|---|---|
| sigshim + ersc + quickload | -- | leader `Z` | `0xc0000005` EXECUTE at `0x3cb67a0` (`CS::CSFadeImp`), +1.7s |
| **er_quickload ALONE** | 23 | leader `Z` | **identical** -- same address, same chain |
| er_telemetry | 6 | boots | -- |
| er_quit_menu | 12 | boots | -- |
| er_loading_bar | 5 | boots | -- |
| er_save_picker | 9 | boots | -- |
| er_save_disable | 4 | boots | -- |
| er_loading_portrait | 11 | vanishes ~4s after its first Present | NO crash record at all |
| er_armament_icons | 4 | vanishes ~+38s | `0xc000001d` ILLEGAL_INSTRUCTION at `game+0x32ee2b5`; 3 hooks correctly REFUSED by the gate first |

**"Died" is not a usable signal on 1.17 -- the fault SIGNATURE is.** Three shells die and they die
three different ways; only er_quickload produces the CSFadeImp execute fault. A bisect that scores
rows as live/dead instead of comparing signatures would have convicted the wrong crate twice.

Exonerated outright: Seamless Co-op 1.9.9; the shared base (`er-game-base`, `er-hook`,
`er-crash-logging-core`, `er-telemetry-core` -- er_telemetry links all of it and boots); the shared
title/quit feature crates (er_quit_menu); the Present hook (er_loading_bar hooks Present and boots,
which kills the tempting 2x2 where both Present-hooking shells died); `er-save-redirect`
(er_save_picker); `er-save-suppress` (er_save_disable). Also cleared by reading rather than
running: quickload's own VMT-swap path, whose `swapchain_vtable_matches` demands an exact
slot-8-and-22 match against addresses resolved from a dummy swapchain, and whose run log records
`exact vtable match` -- it cannot have patched a CSFadeImp.

### What is left, and why the shell axis is now exhausted

Crate-set arithmetic (`cargo metadata` closures, dying minus living) leaves these carried by NO
other cdylib, so no further profile can test them without editing quickload itself:

    er-quickload (379 files of experiments)   er-title-flow      er-scaleform-hooks
    er-boot-profiler                          er-tpf             er-profile-summary-core

and three shared only with the OTHER dying shell, so unproven either way: `er-gfx`,
`er-loading-portrait-core`, `erpx-rs`. (`er_armament_icons` carries er-gfx and does die -- but with
a different signature, so it neither convicts nor clears it.) `CS::CSFadeImp` is title/fade
machinery, which points at er-title-flow and er-scaleform-hooks first.

### Reading the evidence without being misled

* `modbt=[...]` in an `access-violation` line is a **stack SCAN**, not an unwind. It mixes dead
  frames with live ones. Chasing its frame 0 here led to `eldenring.exe+0xb3d2e8`, whose preceding
  call is a three-instruction atomic increment that cannot fault -- a stale frame. Do not chain it.
* What IS trustworthy in that line: `raw=[...]` (the actual qwords at `rsp`), the `vt=` probe (feed
  the rva to the RTTI reader below), and the register values.
* Resolving a vtable rva to a class name, offline, in both images -- `vtable[-1]` is the
  RTTICompleteObjectLocator, whose `pTypeDescriptor` rva + 16 is the mangled name:

  ```python
  col = struct.unpack_from('<Q', image, vt_rva - 8)[0] - 0x140000000
  ptd = struct.unpack_from('<IIIII', image, col)[3]
  name = image[ptd+16:ptd+16+120].split(b'\0')[0]
  ```

  This is how `0x2b6d728` was named `CS::CSFadeImp`, and it also proves the region MOVED: in 1.16.2
  that same rva is not a vtable at all, and `0x2b63bb0` -- which the tree hard-codes as
  `TITLE_OWNER_VTABLE_RVA` in three crates -- is `CS::TitleStep` in 1.16.2 and nothing in 1.17.
  Those scans now silently find nothing. Read-only and fault-safe, so not a crash, but the features
  behind them are dead until the constants are re-pointed.

### Function lengths as a cheap map audit

Comparing `.pdata` extents for every pair in `rva-map-1162-to-1170.needed-verified.tsv`: exactly
one changed size, `0x140af7cf0 -> 0x140af9000` (`MOVEMAPSTEP_STEP_MOVEMAP_RVA`), `0x120b ->
0x1213`. That was 1 of 218 when it was first run; re-counted 2026-08-31 it is **1 of 411**, plus
0 of 102 in `verified.tsv` -- the ledgers nearly doubled and the answer did not move. Both extents
were re-read straight out of the two images' `.pdata` on 2026-08-31 and match.

**The conclusion originally drawn here was wrong, and the way it was wrong is the point.** It said
the 8 bytes were "elsewhere in the tail" and `IDENTICAL` was the right verdict. They are two
INSERTED INSTRUCTIONS at index 873 of 975 -- a Torrent destroy-and-recreate -- and `IDENTICAL` was
what the verifier said after comparing 120 instructions and stopping. A cheap signal was noticed,
explained away, and the explanation was never checked against the thing it explained.

Since 2026-08-30 the extent length is a first-class signal rather than an audit someone remembers
to run: a differing length blocks an `IDENTICAL` verdict outright, the verifier decodes the WHOLE
declared extent instead of the first 120 instructions, and every row carries an `extent` column
(`PDATA:0x120b/0x1213+8`).

The pair verdicts `PATCH-SITE-IDENTICAL`, not `NEAR`: the difference is a single pure-insertion
hunk 0x1055 bytes past a prologue that is `48 8b c4 55 56 57 41 54` in both builds, so MinHook's
five bytes and the three instructions it relocates are identical code in both images. The callee,
`CS::MoveMapStep::_UpdateHorseType`, is NEW in 1.17 -- its prologue signature has 0 hits in
`eldenring-deobf.bin` and 1 in `eldenring-deobf-1.17.bin`, and the whole-image pairing's local
delta steps `0x1490 -> 0x16a0` across it, which is exactly its 514 bytes plus alignment. It never
dereferences the `MoveMapStep`: the caller's `mov rcx,rbx` is a DEAD STORE (`rcx` is written
before any read in the callee and in the first thing the callee calls), and the callee reloads
`rbx` from `GameDataMan`. So the fields er-title-flow's after-original detour holds -- `+0x100`,
`+0x270`, `+0x4b8`, `+0x4c`, `+0x50` -- are untouched by the insertion, and the gate read it sits
two instructions in front of is unchanged.

## How much of the migration is actually left

**Re-measured 2026-08-31 with the repo's own instrument, and the answer moved by two orders of
magnitude.** Run it rather than reading a number:

    python3 scripts/audit-1170-coverage-inventory.py --report

    488 unique GAME addresses; 484 resolvable, 4 UNMAPPED

Four. Not 185. This section previously carried a 2026-08-29 snapshot -- `367` constants, `182`
mapped, `185` unmapped, split `141` data / `29` no-unique-pair / `11` mid-function-fixable / `4`
mid-function-unmapped -- and every subsection under it was written against that split. The work it
describes was then done and the prose was not. It is deleted rather than dated, because a work
list that is 97% finished reads as a work list.

Two footnotes the inventory prints that are worth not rediscovering:

* 6 of the resolvable addresses translate for a CALL and are refused for a DETOUR. Mapping cannot
  fix those; they need entry/verdict evidence.
* 4 of the addresses are **not `eldenring.exe` addresses at all** -- `0x22d30`, `0x243e0`,
  `0x24460`, `0xabc20` belong to `ersc.dll`. None of them is ELDEN RING migration work. Three of
  the four are `SHOW_RVA`, `INVADE_ACTION_RVA` and `CANCEL_ACTION_RVA`, which the "eleven
  mechanical ones" table below flagged with "check what module they are relative to before
  touching them". That question is answered: `ersc.dll`.

  **This bullet used to end "No ELDEN RING patch moves them", and that was read as "they do not
  move". Seamless Co-op v2.0.0 moved every one of them on 2026-09-02.** The clause was true and
  useless: `ersc.dll` has its own release schedule, so being immune to a GAME patch says nothing
  about stability. Measured against the v1.9.9 the pins came from -- `show` `0x22d30` ->
  `0x241a0`, `cancel` `0x24460` -> `0x258d0`, `invade` `0x243e0` -> **no match at any signature
  length**, `BuildLobbyKey` `0xabc20` -> **ambiguous, three candidates**. The session ABI moved
  with them (`S+0x110` -> `S+0x150`, `S+0x10c` -> `S+0x14c`, the option sub-object `+0xc0` ->
  `+0x100`), cancel now writes `0x23` where it wrote `0x22`, and nothing in v2.0.0 writes `0xd`
  at all. The lesson to carry forward is that a foreign module needs a version check, not an
  exemption from one: `scripts/ersc_identify.py` reports which build is installed and
  `scripts/locate-ersc-entry-points.py` maps a v1.9.9 function body onto it.

### The eleven mechanical ones -- eight of them are done

A mid-function address cannot be mapped, but the function that contains it can, and the offset
within it survives the move. So the fix is to declare the FUNCTION as the `*_RVA` constant (which
puts it in front of `scripts/select-needed-1170-rows.py`) and add the offset at the use site.
That technique is the durable part of this section. The work list is not: re-read against the
tree 2026-08-31, only two of the eleven are still in the shape described.

| constant | state on 2026-08-31 |
|---|---|
| `TITLE_GFX_VISIBLE_TITLE_FADEIN_CALLER_RVA` | **gone** -- no declaration anywhere under `crates/` |
| `TITLE_NATIVE_MENU_VISUAL_WINDOW_FADEIN_RUN_CALLER_RVA` | **gone** |
| `SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA` | **gone** |
| `SYSTEM_QUIT_SECOND_ROW_TARGET_RETURN_RVA` | **gone** |
| `TITLE_NATIVE_MENU_VISUAL_FACTORY_RVA` | **converted** -- now derives from `MENU_WINDOW_JOB_NATIVE_CTOR_B_RVA` |
| `FREELIST_SHUTDOWN_ASSERT_RVA` | **converted** -- `FREELIST_SHUTDOWN_ASSERT_FN_RVA + FREELIST_SHUTDOWN_ASSERT_WINDOW_OFFSET` |
| `SHOW_RVA` / `INVADE_ACTION_RVA` / `CANCEL_ACTION_RVA` | **answered, then moved** -- they are `ersc.dll` RVAs (`0x2_2d30`, `0x2_43e0`, `0x2_4460`), not `eldenring.exe`. Immune to ELDEN RING patches, NOT to Seamless ones: v2.0.0 (2026-09-02) moved show to `0x2_41a0` and cancel to `0x2_58d0`, and `invade` matches nothing. See the footnote above |
| `GX_COMMAND_QUEUE_RVA` | **still a bare mid-function literal**, `0x8012a8` in `er-loading-portrait-core/src/resource_readback.rs`. Containing fn `0x8012a0 -> 0x802120`, offset `+0x8` |
| `GX_CMD_QUEUE_WRAPPER_RVA_MIN` | **still a bare mid-function literal**, `0x1aea900` in `er-title-flow/src/constants_autoload_state.rs`. Containing fn `0x1aea880 -> 0x1aec680`, offset `+0x80` |

### Worked example: `SPLASH_SKIP_RVA`, done

It was `0xb0c35d`, a byte in the middle of `STEP_BeginLogo`, patched `je(0x74)` -> `jg(0x7f)` with a
raw `VirtualProtect` + store that never went near the address gate. On 1.17 that byte is `0x4a` --
the first displacement byte of a `lea` -- so the write would have corrupted an unrelated
instruction in the title path. **It corrupted nothing only because `apply_splash_skip` compares the
opcode before it writes**, and on 2026-08-29 it logged:

    splash-skip: ABORT -- byte at 0x140b0c35d is 0x4a, expected 0x74

That is the pattern to copy for every raw byte patch: a signature check makes a stale address a
refusal instead of corruption. The address is now `SPLASH_SKIP_FN_RVA = 0xb0c2a0` plus
`SPLASH_SKIP_JE_OFFSET = 0xbd`, resolved through `resolve_game_address`, with the resolver living in
`er-title-flow` beside the constants so the patcher and the telemetry oracle that reads the byte
back cannot disagree about which byte it is. The pair is corroborated four ways: the whole-image
map already carried `0xb0c2a0 -> 0xb0d940`; both functions are `0x294` bytes in `.pdata`; the byte
at `+0xbd` is `0x74` in both images behind the same `bf b8 00 00 00 00` prefix; and the verifier
scores it `IDENTICAL 1.000 over 120 insns, BOTH-ENTRIES`. The `+0x16a0` delta is the same one the
`CS::TitleStep` vtable slots move by, so the region shifted as a block.

## Per-DLL 1.17 readiness, and the one property worth guarding hardest (2026-08-29)

`scripts/audit-1170-readiness.py` scores every cdylib on a single question: **can a stale 1.16.2
address still reach ELDEN RING 1.17 from this DLL without the gate having a say?** Three ways it
can, and they are not equally dangerous, which is the whole reason the tool separates them:

| bucket | what happens on 1.17 |
|---|---|
| **EXEC** | `transmute(base + SOME_RVA)` then call it. Control transfers into whatever now occupies the address. Nothing refuses, nothing logs. WORST. |
| **WRITE** | a raw store at `base + SOME_RVA`. Corrupts the image for every later reader, not just this caller. |
| read/compare | `safe_read_usize(base + rva)`, or `vt != base + SOME_VTABLE_RVA`. Fault-safe: a wrong answer, never a fault. The SILENT class -- the feature quietly stops working. |

**A MAPPED constant is not safe when used this way.** The map knows exactly where the function
went; `base + rva` simply never asks it. Counting only the unmapped ones undercounted the hazard
by half, which is how the first pass of this audit got it wrong.

### Where it stands

    ungated EXEC   210  ->  0    ALL 27 cdylibs
    ungated WRITE    0  ->  0    ALL 27 cdylibs, before and after

**Every game call in this tree now goes through the 1.17 gate.** A function the patch moved and
nothing verified produces a refusal and a named log line, not a jump into whatever now occupies
the address. That is a property of the whole DLL set, not of the DLLs someone remembered to fix.

**0 ungated WRITEs is the property worth guarding hardest**: it means no DLL in this tree can
corrupt the running 1.17 image with a stale address. `--check` is wired into `scripts/check.sh`
and fails when any per-cdylib count RISES, so that stays true by construction rather than by
memory. It divides with `scripts/check-stale-rva-calls.py`, which is the authority on the repo-wide
CALL set; this one adds per-cdylib attribution (a flat repo total hides one DLL regressing while
another improves) and the WRITE/read buckets that nothing else measures. The two independently
written detectors agree to within one site, which is the closest thing to a cross-check available.

### What the conversion looks like

`scripts/gate-stale-rva-calls.py` did 197 of them mechanically, and its value is mostly in what it
REFUSES to touch:

* never inside an `extern "system"` function -- those are detours, and a detour that returns early
  never calls its original, which deletes the game's own behaviour instead of adding ours.
  `hud_weapon_update_hook` needed `return ret`, not `return`;
* only where the return type has an obvious did-nothing value (`()`, `bool`, an integer, `f32`);
  `Result<String, String>` and `Option<(f32, f32)>` are printed for a human rather than guessed at.

The 13 it refused were then done by hand, and the refusals were right every time: three
`Result<_, String>` functions wanted an `Err` carrying a reason, and two were detours where the
correct degraded behaviour is to skip only the ADDED work and still let the game's own body run.
A blind transform would have deleted vanilla behaviour in both detours.

### Runtime evidence lives in a file, never in the audit's head

`docs/recon/dll-1170-runtime-results.json` records the launch outcome per cdylib, and the audit
prints it as a column. It is populated only by an actual launch: a DLL with zero ungated sites is
NOT thereby "working on 1.17", and the tool never infers one from the other. Rows tagged
`(pre-gating)` were measured before their crate was converted and are due a re-run.

## The silent class: data addresses, and how a vtable proves its own identity (2026-08-29)

Data addresses -- vtables, globals, tables -- are not in `.text`, so the function gate cannot see
them, and a stale one does not crash: the reads are fault-safe, so it yields a wrong answer and the
feature behind it quietly stops working. `TITLE_OWNER_VTABLE_RVA` is `CS::TitleStep` in 1.16.2 and
not a vtable at all in 1.17, and its three scans had been finding no owner, forever, without a log
line. (Re-verified 2026-08-31 by reading RTTI out of both flat images: `0x2b63bb0` resolves to
`.?AVTitleStep@CS@@` in 1.16.2 and to nothing in 1.17; `0x2b66c60` resolves to it in 1.17 and to
nothing in 1.16.2.)

This section opened with "141 of the unmapped constants" until 2026-08-31. That count belonged to
the deleted snapshot above; `audit-1170-coverage-inventory.py --report` now reports 4 unmapped
addresses in total. The mechanism below is what still matters, not the size of the pile.

`scripts/map-data-rvas-1162-to-1170.py` already carries data addresses by VOTING: every
rip-relative reference in 1.16.2 `.text` is mapped onto its 1.17 function and the same instruction
re-read there. It withholds any row under two agreeing references, because a confident wrong
address once cost a boot.

### RTTI rescues what voting withholds

A vtable carries its own name. MSVC puts a CompleteObjectLocator at `vtable[-1]` whose type
descriptor holds the mangled class, and **a name that occurs once per image is stronger evidence
than any number of agreeing displacements**. `rtti_confirms()` accepts a withheld row when the same
mangled name sits at the source in 1.16.2 and the destination in 1.17 **and at neither crossed
position** -- that last condition is what stops a region which happens not to have moved from
passing by accident. Rescued rows carry an `rtti` suffix in the votes column.

The rescue worked and kept working. `data.tsv` now carries 111 rows, of which **6** are
`1/1 rtti` (re-counted 2026-08-31 -- this passage used to say "76 -> 81 usable rows, 5 carried by
RTTI" and then list four, which is how a count and its evidence drift apart):

      SAVE_RETRY_DIALOG_VTABLE_RVA                0x2aaabf8 -> 0x2aadc78
      FUNCTOR_VTABLE_RVA                          0x2ac3ea8 -> 0x2ac6f28   std::_Func_impl<lambda_e1e7fa74...>
      MENU_ITEM_LOADGAME_FUNCTOR_VTABLE_RVA       0x2ac3ea8 -> 0x2ac6f28   (same vtable, second name)
      DEPOSITORY_DIALOG_VFTABLE_RVA               0x2aebba0 -> 0x2aeec20   CS::DepositoryDialog
      SYSTEM_QUIT_RETURN_TITLE_ACTION_VTABLE_RVA  0x2b12b48 -> 0x2b15bc8   std::_Func_impl<lambda_6698ebbb...>
      MEMBERFUNCJOB_VTABLE_RVA                    0x2b265d0 -> 0x2b29650   CS::MenuMemberFuncJob<TitleTopDialog>

Every one of those six pairs was re-read out of the two flat images on 2026-08-31 and holds,
including the condition that makes the method sound: the mangled name is present at the source in
1.16.2 and at the destination in 1.17, and at NEITHER crossed position.

### Two methods, no shared assumption, same answer

Worth doing for any address you are going to trust. The title vtables were derived twice over:

| | `TITLE_OWNER_VTABLE_RVA` | `TITLE_TOP_DIALOG_VTABLE_RVA` |
|---|---|---|
| RTTI ordinal + slot delta (`find-vtable-rva.py`) | `0x2b63bb0 -> 0x2b66c60` (#16 of 21 TitleStep vtables in both images) | `0x2b26468 -> 0x2b294e8` (all three shift the same `0x3080`) |
| reference voting (`map-data-rvas`) | `0x2b63bb0 -> 0x2b66c60`, 2/2 | `0x2b26468 -> 0x2b294e8`, 2/2 |

All 32 use sites now go through `er_game_base::mem::game_data_addr(base, RVA, "RVA")`, which
resolves for the running build and returns `0` on a refusal -- so an unmapped address takes the
caller's existing "not the object I wanted" branch and says so, instead of silently comparing
against a 1.16.2 value that can never match.

**When refreshing the data map, diff by (constant -> destination), never by line.** A refresh
rewrote 37 of 77 rows and changed ZERO destinations -- only the vote counts moved. Read by line,
that looks like a mass remap.

### Score against all three maps, not one

The resolver's table is fed by three files -- `data.tsv`, `needed-verified.tsv` and `verified.tsv`
(see `crates/er-game-base/build.rs`) -- so "is this constant mapped?" has to be asked of the union.
Asking only the data map said 167 sites needed a new row. Against the union it is 110, and the
57-site difference were free wins: the address was already known and the code was still building it
by hand. `scripts/gate-stale-rva-data.py` now reads all three.

    151 + 57 = 208 data/compare sites routed through `game_data_addr`   (2026-08-29)

That second line used to read "110 sites across 61 constants still need a map row earned first".
Re-measured 2026-08-31: `audit-1170-readiness.py` reports **136** ungated read/compare sites and
`audit-1170-coverage-inventory.py` reports **4** unmapped addresses repo-wide. Ungated is not the
same claim as unmapped -- an ungated site is one that builds `base + RVA` by hand and never asks
the map, whether or not the map knows the answer -- and it is the ungated 136 that is now the
work, not a hunt for missing rows.

Note that several of those constants are `.text` addresses used as DATA -- `MENU_ITEM_ACCEPT_NATIVE_RVA`
(`0x7ad810`) is compared against, not called. `map-data-rvas-1162-to-1170.py` skips `.text` by
design, so those rows come from the FUNCTION map instead; a constant missing from one map is not
missing from the gate.

## The gate turned the boot crash into 792 refusals (2026-08-29)

`er_quickload` boots on ELDEN RING 1.17. It had been dying at +1.7s with an execute access
violation on a `CS::CSFadeImp` object; on the fully-gated tree its crash log carries **zero fault
lines** and its debug log carries **792 `ADDRESS REFUSED` entries**. That is the whole mechanism in
one number: 792 stale 1.16.2 addresses that used to be called blind are now refused by name, and
one of them was the fault. A refused address costs a feature; calling it cost the process.

### Per-DLL runtime verdicts, one launch each, on one settled build

    26 boot   1 dies   (27 cdylibs, docs/recon/dll-1170-runtime-results.json)

The single holdout is `er-reload-trace`, which vanishes with no crash record. It installs ~40
diagnostic hooks and its log shows two `create failed status=8` (`MH_ERROR_UNSUPPORTED_FUNCTION`)
before the end -- a tracing shell, not a product one, and its own bug rather than a shared defect.

Two shells that DIED before this work now boot: `er_armament_icons` (was `0xc000001d`
ILLEGAL_INSTRUCTION at `game+0x32ee2b5`) and `er_loading_portrait` (was vanishing ~4s after its
first Present). Both had ungated `transmute(base + rva)` sites; both are clean now.

### Two ways this measurement can lie, and what stops each

* **A mixed build.** The first attempt at this sweep was thrown away because DLLs were rebuilt ten
  minutes into it, so the early entries tested one binary and the rest another. The sweep now
  fingerprints every DLL at the start and ABORTS the instant one changes underneath it.
* **Scoring liveness instead of signature.** Three shells once died three different ways, and
  ranking rows live/dead convicted the wrong crate twice. The results file records the crash
  signature, and `not-built` is never read as a pass -- `er-ags-stub` reported it for a whole run
  because its DLL is `amd_ags_x64.dll`, not `er_ags_stub.dll`; the sweep now reads the real `[lib]
  name` from cargo metadata.

### What this does and does not prove

It proves each DLL can be loaded into 1.17 and the game survives boot, and that no stale address
can execute or corrupt. It does NOT prove any FEATURE works: 792 refusals is 792 things not
happening. Feature-level 1.17 correctness is per-DLL work behind this, and the refusal log is
exactly the to-do list for it.

## Twenty-eight `functions.tsv` pairs two methods refute, and why none was written (2026-08-30)

Two independent passes each found `docs/recon/rva-map-1162-to-1170.functions.tsv` rows they
could refute: the call-graph topology pairing adjudicated by whole-body byte equality
(13 rows), and the leaf pass's caller vote adjudicated by masked extent bytes plus region
delta (19 rows, 4 shared). The negative controls are strong -- the whole-body test accepted
0 of 400 random wrong destinations while accepting 399 of 400 correct ones -- and 16 of the
leaf pass's 19 were independently reproduced by a topology run that did not seed them, with
0 contradicted.

**None of them was written to any ledger, and that is the finding, not an omission.** All 28
are absent from `verified.tsv`, `needed.tsv`, `needed-verified.tsv` and `data.tsv`, and no
literal in `crates/` names any of them. They exist only in `functions.tsv`, which is
gitignored AND is not what `build.rs` reads -- its `FUNCTION_MAP` is `needed.tsv`. So no
wrong address reaches either map from these rows, `refuted_sources()` is not in play, and a
correction written into a tracked ledger would be a dead row.

They are recorded here because `functions.tsv` is regenerated and untracked, so this table is
the only place the derivation survives. **If one of these addresses is ever named by a new
`const *_RVA`, put the corrected pair into `needed.tsv` before running
`select-needed-1170-rows.py --refresh`**: the refresh then REFUSES rather than importing the
wrong value. That guard is real -- planting `0x116c70 -> 0xdeadbe0` on a scratch copy exits 1
with `CONFLICT: ... but functions.tsv now pairs it with 0x116c70`, and writes nothing.

| 1.16.2 | `functions.tsv` says | refuted to | evidence | 1.16.2 name |
|---|---|---|---|---|
| `0x140536630` | `0x140a7e5c0` | `0x140537480` | topology + whole-body bytes | thunk_FUN_145ac79fd |
| `0x1406ab480` | `0x140e56250` | `0x1406ac2d0` | caller vote + masked bytes + region delta + unseeded topology | -- |
| `0x1409f5e70` | `0x140a96e80` | `0x1409f7150` | caller vote + masked bytes + region delta + unseeded topology | -- |
| `0x140b0f3f0` | `0x140bbab60` | `0x140b10a90` | caller vote + masked bytes + region delta + unseeded topology | -- |
| `0x140b2d250` | `0x141d2b520` | `0x140b2e8f0` | topology + whole-body bytes + caller vote + region delta | thunk_FUN_1459837f8 |
| `0x140d86bc0` | `0x141c99210` | `0x140d88900` | caller vote + masked bytes + region delta + unseeded topology | -- |
| `0x140db28d0` | `0x140e4dd80` | `0x140db4630` | caller vote + masked bytes + region delta + unseeded topology | -- |
| `0x140e44850` | `0x1401b9430` | `0x140e46650` | caller vote + masked bytes + region delta + unseeded topology | -- |
| `0x140e46020` | `0x140e54040` | `0x140e47e20` | caller vote + masked bytes + region delta (topology had no opinion) | -- |
| `0x140e4ac30` | `0x1426d51a0` | `0x140e4ca30` | caller vote + masked bytes + region delta + unseeded topology | -- |
| `0x140e4c550` | `0x1406c8190` | `0x140e4e350` | caller vote + masked bytes + region delta + unseeded topology | -- |
| `0x140e4c6d0` | `0x140ae1a80` | `0x140e4e4d0` | topology + whole-body bytes + caller vote + region delta | Game.Network.GetOpenFieldMaxDistFromHostPlayer |
| `0x140e4e3e0` | `0x140654b70` | `0x140e501e0` | caller vote + masked bytes + region delta + unseeded topology | -- |
| `0x140e51e50` | `0x140540150` | `0x140e53c50` | caller vote + masked bytes + region delta (topology had no opinion) | -- |
| `0x140e530b0` | `0x140bf7f30` | `0x140e54eb0` | caller vote + masked bytes + region delta (topology had no opinion) | -- |
| `0x140e5dc80` | `0x1406830c0` | `0x140e5fa80` | topology + whole-body bytes + caller vote + region delta | thunk_FUN_1402b9ccd |
| `0x140e5df10` | `0x140e4a8e0` | `0x140e5fd10` | caller vote + masked bytes + region delta + unseeded topology | -- |
| `0x141125370` | `0x140dfde10` | `0x141127170` | topology + whole-body bytes | SysAllocStatic |
| `0x14181e140` | `0x1404d3780` | `0x14181ff40` | topology + whole-body bytes + caller vote + region delta | thunk_FUN_145333383 |
| `0x141a597c0` | `0x140b125b0` | `0x141a5b5c0` | caller vote + masked bytes + region delta + unseeded topology | -- |
| `0x141ec5fe0` | `0x141ec7de0` | `0x141ec7e30` | topology + whole-body bytes | FUN_141ec5fe0 |
| `0x141ecf9a0` | `0x140e45e80` | `0x141ed17a0` | caller vote + masked bytes + region delta + unseeded topology | -- |
| `0x142028860` | `0x1414c146f` | `0x14202a660` | topology + whole-body bytes | thunk_FUN_144c45a14 |
| `0x142217ae0` | `0x1422198e0` | `0x142219830` | topology + whole-body bytes | FUN_142217ae0 |
| `0x14223f8f0` | `0x14296e345` | `0x142242080` | topology + whole-body bytes | SetSwitch |
| `0x142417b80` | `0x14241a390` | `0x14241a3c0` | topology + whole-body bytes | FUN_142417b80 |
| `0x14296b5a9` | `0x14296dd69` | `0x14296e7f5` | topology + whole-body bytes | Unwind@14296b5a9 |
| `0x14299c180` | `0x14299edb0` | `0x14299ed20` | topology + whole-body bytes | FUN_14299c180 |

Three of the leaf pass's nineteen are NOT in the reproduced set: `0x140e46020`,
`0x140e51e50` and `0x140e530b0`. In the split that withheld each one topology never reached
it, and in the other split it is a seed that merely echoes `functions.tsv`, so topology has
no independent opinion. What would decide them is a third complementary seed split that
both withholds them and anchors their neighbourhood -- NOT a wider ORDER residue, which is
the setting whose cross-run error is 1.881% rather than 0.181%.

Several are adjacent-sibling SWAPS, where `functions.tsv` crossed two same-shape neighbours
(`0x14299c0f0`/`0x14299c180` onto `0x14299ed20`/`0x14299edb0`, and `0x141ec5fe0`/`0x141ec6030`
onto `0x141ec7de0`/`0x141ec7e30`). Masked signature identity cannot separate those; a caller
set can. Apply such a pair as a pair or not at all -- half a swap leaves one address
pointing at the other's function.

## Did 1.17 add a new obfuscation technique? No (2026-08-31)

Everything this workspace does on 1.17 rests on `eldenring-deobf-1.17.bin` being a faithful
de-Arxan'd rendering of the installed game. Until now the only evidence for that was one
sentence -- "byte-identical to live memory at three independently known sites" -- which is a spot
check across a 43 MB `.text`, not coverage. A new protection technique living anywhere those three
samples did not fall would leave the image quietly wrong there, and every address, ledger row and
hook target derived from those bytes would inherit the error without anything going red.

**The verdict: no new technique.** The two builds' Arxan profiles are identical in every structural
count, the deobfuscated image is bit-reproducible from the shipped executable, and a
whole-image scan finds no undecrypted code. What that measurement can and cannot see is spelled
out at the end, because it is the part worth remembering.

### The profile comparison

`scripts/dearxan-profile.rs` (built as a dearxan example, the same way
`scripts/dearxan-deobfuscate.rs` is) reports the stub population broken down by KIND at every
stage of dearxan's pipeline, so a new technique shows up as a new SHAPE rather than as a count
moving. dearxan models three ciphers -- `Tea`, `Rmx`, `Sub` -- and a fourth would surface either as
an analysis error bucket or as a region list whose "plaintext" is noise.

The 1.16.2 `eldenring.exe` no longer exists on this machine (the game updated over it), so the two
builds are compared in the form both still exist in: the DEOBFUSCATED flat image. That is a valid
comparison because Arxan's stubs are never themselves encrypted, so stub discovery and region
DECLARATION read identically from either form -- proven by running both forms of 1.17 and getting
the same declared table.

| measured from the deobfuscated flat image | 1.16.2 | 1.17 |
| --- | --- | --- |
| raw `test rsp, 15` candidates | 1619 | 1619 |
| stubs analyzed | 1614 | 1614 |
| analyzed ok / analysis errors | 1614 / **0** | 1614 / **0** |
| stubs declaring encrypted regions | 169 | 169 |
| inert stubs (checksum / anti-tamper / CFG) | 1445 | 1445 |
| `Tea` region lists | 83 | 83 |
| `Rmx` region lists | 39 | 39 |
| `Sub` region lists | 47 | 47 |
| `Tea` regions / plaintext bytes | 2781 / 280,452 | 2864 / 284,861 |
| `Rmx` regions / plaintext bytes | 11,038 / 1,664,629 | 10,957 / 1,658,733 |
| `Sub` regions / plaintext bytes | 14,165 / 2,619,834 | 14,159 / 2,626,737 |
| lists whose plaintext is ALREADY in the image | 129 (39 Rmx, 47 Sub, 43 Tea) | 129 (39 Rmx, 47 Sub, 43 Tea) |
| `.pdata` RUNTIME_FUNCTION entries | 235,862 | 235,904 |
| Shannon entropy `.text` / Arxan `.text` / `.interpr` | 6.582 / 7.279 / 6.458 | 6.582 / 7.281 / 6.464 |

Every structural count is the same number. Only the per-kind region and byte totals move, by under
0.5%, which is what a build with 11 KB more code looks like. `.text` entropy is unchanged to three
decimals -- a new encryption layer over any meaningful span would raise it.

Measured instead from the installed 1.17 `.exe` (the ciphertext form, which is the only form the
older build can no longer be read in): 1602 raw candidates, 1597 stubs, 1597 ok, 0 errors, 169
declaring regions, 1428 inert, and the identical declared per-kind table. The +17 stub difference
between the two forms is fully accounted for: decryption introduces 17 new `48 f7 c4 0f 00 00 00`
byte sequences in Arxan's own `.text` (883 candidates there before, 900 after), and all 17 analyze
as inert. So the exe-form count for 1.16.2 would have been 1597/1428 too -- which is exactly what
`bd deobfuscated-er-image-tooling-2026` recorded for the run that produced `eldenring-deobf.bin`.

**Only `Tea` is ever applied.** All 39 `Rmx` and all 47 `Sub` lists decrypt to bytes that are
already at their RVAs -- they are self-verifying integrity data, not at-rest encryption -- and the
resolver eliminates them on the entropy comparison. That holds identically in both builds. Of the
83 `Tea` lists, 3 are likewise already-present and 40 are applied; the remaining 43 are the paired
re-encryption stubs whose "plaintext" is the random filler that hides the code again.

That subtraction is also how the 1.16.2 applied numbers survive without the executable. On 1.17,
lists-present-in-the-deobf-image minus lists-present-in-the-cipher-image is
`43/1493/195,552 - 3/122/106,243 = 40/1371/89,309`, which IS the applied set. The 1.16.2 deobf
image gives `43/1451/193,088` for the first term; combined with the recorded 1330 regions / 87,364
bytes applied, its second term must have been 121 regions / 105,724 bytes -- one region and 519
bytes away from 1.17's. Internally consistent, so the old figure is corroborated rather than
merely quoted.

### The byte-identity evidence, widened from three sites to the whole image

1. **Bit-reproducible.** Re-running `deobfuscate` on the currently installed `eldenring.exe`
   produces sha256 `bc1daf4838d3dc2757719fb69855b1aa95f88825165636529b137e27888c1a76`, identical to
   the `eldenring-deobf-1.17.bin` already on disk. The image is the tool's output for the build
   that is installed right now -- not a stale artefact of some earlier copy, and not hand-touched.
2. **Exhaustive diff against the shipped executable.** Mapping the installed `.exe` with dearxan's
   own mapping rule and comparing all 98,604,544 bytes: exactly **1344 differing runs, 89,309
   bytes**. The union of the 1371 applied regions is exactly those 1344 disjoint intervals and
   exactly those 89,309 bytes. **Zero changed bytes fall outside a declared applied region, and
   zero bytes inside one were left unchanged.** So 98,515,235 bytes -- 99.909% of the image,
   including all of `.rdata`, `.data`, `.pdata` and `.reloc` -- are the shipped bytes untouched,
   and the deobfuscator's entire footprint is accounted for region by region.
3. **Residual-ciphertext scan** (`scripts/arxan-residual-scan.py`), the 2026-07-01 completeness
   scan re-run against 1.17 and widened. Function entries come from `.pdata` (175,227 merged
   extents) UNIONED with the 1.17 Ghidra dump's 366,189 functions -- which matters, because
   `.pdata` is blind to unwindless leaves across 146,715 holes. Ghidra covers all but one of the
   `.pdata` starts, so the union is 366,190 and 338,454 of them are long enough to judge.
   **392 flagged as not-code (0.1158%), and 0 of them inside a region dearxan decrypted.**
4. **The same scan on 1.16.2**: 338,688 scanned, **384 flagged (0.1134%)**, same species, same
   clustering. The rate moves by 0.0024 percentage points between builds.

The 392 are not defects and not missed decryptions. 365 begin `E9` (`jmp rel32`) and 12 begin `EB`
(`jmp rel8`) -- 96% are Arxan control-flow trampolines whose declared extent runs past the jump
into filler. The other 15 open with clean prologues or Arxan's `lea rsp,[rsp+8]; jmp [rsp-8]`
return gadget. That is the same conclusion the 2026-07-01 scan reached on the previous build, and
it still holds: the residue is Arxan CONTROL-FLOW obfuscation, a separate layer that is out of
dearxan's decryption scope and is present in a runtime dump too. Arxan's own `.text` section
(14,856 Ghidra functions in 1.17, 14,883 in 1.16.2) was scanned as well and flagged **zero** in
either build.

The scan's thresholds were calibrated, not chosen. 1.17 supplies a labelled dataset for free: the
same 1371 spans exist as ciphertext in the `.exe` and as plaintext in the deobfuscated image. Over
the 831 spans of at least 32 bytes the rule flags **88.2% of ciphertext, 0.0% of plaintext**, and
0.225% of 4000 random control functions. Sensitivity is per function; a missed region covers a run
of them, and three consecutive escapes have probability 0.0017.

### Two premises corrected along the way

* **The two `.text` sections are not a 1.17 asymmetry.** Both builds carry eleven sections with the
  same names in the same order, including two executable `.text`: the game's at RVA `0x1000`, and
  Arxan's at `0x4c0e000` (1.16.2) / `0x4c13000` (1.17). Arxan's grew by `0x2e00` bytes, in
  proportion with the game's `0x2c00`. Nothing about the section layout changed.
* **`find-deobf-bytes.py` does search the second `.text`** -- it scans the whole flat file. Its
  blind spot is `MAX_HITS = 64`: results come out in ascending address order, so a pattern common
  in the game's `.text` fills the budget in the first half-megabyte and the tool returns before
  reaching Arxan's section. `4883ec28` reports 64 hits, all below `0x14007b000`. That is a
  truncation to be aware of when reading a negative result, not a section restriction.
* `eldenring-deobf.bin` is **2.6.2.0** -- confirmed from its own `VS_VERSIONINFO`, against
  2.7.0.0 for both the installed `.exe` and `eldenring-deobf-1.17.bin`. The bd memory that recorded
  its generation labels the run "1.16.1"; the label is wrong, the image is 1.16.2, and its
  1597-stub / 1330-region figures are therefore the 1.16.2 baseline.

### What this measurement would have missed

The most useful sentence here, because a clean negative is only worth what its blind spots allow.

* **Anything that does not change bytes at rest.** A runtime-only integrity check, a new anti-debug
  or timing probe, a stub that decrypts and re-encrypts within a single call -- all of them leave
  the static image identical to a build that has none of them. Every number above is a statement
  about the file on disk. Nothing here reads the running game.
* **A cipher whose stub does not open with `test rsp, 15`.** That one 7-byte sequence is dearxan's
  entire discovery seed, so a protection introduced with a different stub prologue is invisible to
  the profiler AND to dearxan itself. The residual scan is the backstop for exactly that case --
  it sees the EFFECT (code that is not code) without needing to find the cause -- but only at 88%
  per function, and only where a function is declared.
* **Encrypted DATA that no function declares.** The scan is function-entry driven, so an encrypted
  `.rdata` blob or vtable with no `.pdata` entry and no Ghidra function would not be examined.
* **The 27,736 functions (7.6%) shorter than 8 bytes**, where there is not enough signal to judge.
* **Live memory is still three sites.** Comparing the static image against the running process
  would need a game launch, and none was made. If Arxan's runtime behaviour diverges from its
  at-rest layout by design, that divergence is invisible offline by construction.

### Reproducing it, cheaply, at the next game update

```bash
cp -f scripts/dearxan-profile.rs ../dearxan/examples/profile.rs
cd ../dearxan && cargo build --release --example profile --no-default-features --features rayon
./target/release/examples/profile "<game>/eldenring.exe" --regions /tmp/regions.tsv
./target/release/examples/profile <old-deobf>.bin --mapped     # the previous build's baseline
python3 scripts/dump-ghidra-function-list.py --port 8767 --out /tmp/funcs.tsv
uv run --with capstone python3 scripts/arxan-residual-scan.py \
    --image eldenring-deobf-<ver>.bin --functions /tmp/funcs.tsv --regions /tmp/regions.tsv
uv run --with capstone python3 scripts/arxan-residual-scan.py --selftest
```

Each profile run takes 0.3s and the residual scan a few minutes. Compare against the 1.16.2 and
1.17 columns above; a new technique is a new row shape, not a moved count.
