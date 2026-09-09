# er-lockon-filter

While you are invading, other invaders stop being lock-on targets. Your lock-on, and the
left/right target switch, skip them and find the host and their phantoms instead.

There is no hotkey, no toggle and no config file. Loading the DLL is the feature.

## Load it

```toml
[[natives]]
path = 'target/x86_64-pc-windows-msvc/release/er_lockon_filter.dll'
```

Build it with `bash scripts/er-build-dlls.sh er-lockon-filter`, which is the same cargo
invocation as a bare `cargo xwin build -p er-lockon-filter` plus the provenance record every
launch script gates on.

`scripts/me3-dll-conflicts.toml` lists it under `[always]`, so `scripts/er-run-branch.py` loads
it into every launch whether or not the branch touched this crate -- on by user directive,
2026-09-08. It was `[opt_in_only]` for the first day of its life, on the argument that changing
what happens when the player presses lock-on needs consent a dependency-closure walk cannot
give. The consent was given; the mechanics never objected. `--without er-lockon-filter` still
takes it back out of a single run.

## What it actually does

`CS::LockTgtMan`'s per-frame update walks the act-point list and admits each point on four
tests before any distance or angle test runs:

```text
point+0x75 & 1                             the point is enabled
owner = PointOwner(point)                  and has a character behind it
CS::ChrIns::CanTargetTeamType(me, owner)   whom my team may target
owner != me                                and who is not me
```

This DLL detours `PointOwner` -- 1.16.2 `0x140713db0`, 1.17 `0x140714c00` -- and answers `0` when
the point's owner is one of `BloodyFinger` (15), `Recusant` (16) or `FesteringBloodyFinger` (18)
and you are invading. `0` is that function's own answer for a point with no character behind it,
so every caller takes a branch it already has, and all eleven of its call sites are lock-on code:
five in the update above, two building the `NetSyncData` for the target you hold, two
`LockTgtMan` accessors reading flags off the owner.

Whether you are invading is asked of two fields, and either is enough. `ChrIns::chr_type` is the
one the engine derives; `GameMan::summonParamType` is what it derives it from, and it names the
invasion roles directly (`RedInvasionA` -3, `RedInvasionALimited` -4, `RedInvasionB` -5). Reading
both is cheap insurance against a session layer that writes one and not the other, and widening
this half alone hides nobody -- the candidate still has to be typed as an invader.

No game memory is written, no param is patched, damage in both directions is untouched, and
nameplates are untouched.

One thing beyond the candidate list does change, and it is the same statement rather than a side
effect: `CSChrAutoHomingModule` is fed from inside that same admitted-candidate block, so a
fellow invader is no longer an auto-homing target either. Your swings stop bending toward them
for the same reason your reticle stops offering them.

## Why not somewhere more obvious

`CanTargetTeamType` is the test that actually rejects a candidate, but 21 call sites across ai
targeting and damage reach it, so a detour there decides far more than lock-on. Rewriting the
team-relation matrix cell for two invaders to `Friend` reaches those same 21 sites through the
data instead of the code, and would stop two invaders damaging each other as well.

## Reading the log

`er-lockon-filter.log`, beside the game executable, fresh per run.

| line | what it means |
| --- | --- |
| `install: hooked the lock-on point-owner resolver ...` | armed |
| `REFUSED ...` | the address has no detour-safe mapping for the running build; nothing was read or installed |
| `DISARMED ...` | the address resolved but the bytes there are not the verified prologue -- most likely another mod detoured the same entry first |
| `census: first candidate with chr_type N; ...` | each character kind the lock-on system asked about, named once |
| `census: GameMan summon param type is N ...` | your multiplayer role, once per change |
| `hidden: a chr_type N character is out of the lock-on candidate set ...` | the filter fired |

The census is there because the feature can only be exercised by two players invading one
world, which no offline check can produce. If the filter never fires, those lines say whether
your own kind or the other invader's was something other than 15/16/18 -- which is the question
Seamless Co-op makes worth asking, since it runs its own session layer and has been measured
typing remote players `Local` (0).

## Status

Statically settled, never yet run in a live double invasion, and being on by default does not
change that. There is a specific way it can be quietly absent: under Seamless Co-op the local
player has been measured typing as `Local` (0) or `Duelist` (2) with `summonParamType` 0 or
-12, none of which is an invader kind, so the gate may never arm. If you invade and the other
red is still under your reticle, the census lines say which of the two fields read what.

The candidate walk, the detour
target, the offsets it reads (`ChrIns::chr_type` at `+0x68`, `WorldChrManImp::mainPlayer` at
`+0x1e508`, `GameMan::summonParamType` at `+0xd84`) and the 1.16.2 -> 1.17 pair are all
byte-proven in both images; what no offline evidence can settle is which `ChrType` the game and
Seamless give two players invading the same world. The log answers that on the first invasion.
