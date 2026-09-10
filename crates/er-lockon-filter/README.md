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
the point's owner is a hostile phantom and you are one too. `0` is that function's own answer for a point with no character behind it,
so every caller takes a branch it already has, and all eleven of its call sites are lock-on code:
five in the update above, two building the `NetSyncData` for the target you hold, two
`LockTgtMan` accessors reading flags off the owner.

Who counts as a hostile phantom is the game's own answer, not a list somebody typed. Elden Ring
carries a `CharacterTypeProperties` table (1.16.2 `0x143b17c00`, byte-identical at 1.17.1
`0x143b1bc00`) whose `isHostilePhantom` byte `CS::CharacterTypeProperties::IsHostilePhantom`
(`0x1404c7d10`) reads, and it answers yes for `Duelist` (2), `BloodyFinger` (15), `Recusant` (16),
`FesteringBloodyFinger` (18) and the three npc kinds `20`, `21`, `22`. The npc kinds are dropped
-- the game spawned them, and an invader may want to lock one -- and the other four are the set.

Whether you are invading is asked of two fields, and either is enough. `ChrIns::chr_type` is the
one the engine derives; `GameMan::summonParamType` is what it derives it from, and the
`MultiplayProperties` table (`0x143b11230`, 1.17.1 `0x143b15230`) is the derivation: each row
pairs a `SummonParamType` with the `CharacterType` it produces, so every role landing on a
hostile phantom is a role that means you are invading. That is 19 values, not 3, and the
difference is the feature -- a live Seamless Co-op session measured the local player at
`chr_type` 2 with `summonParamType` -12 (`MultiplayProperties` role 12, `アノールマップ守護`),
and the old three-value lists held neither, so the filter could never arm. Reading both fields is
cheap insurance against a session layer that writes one and not the other, and widening this half
alone hides nobody -- the candidate still has to be a hostile phantom.

`scripts/er-character-type-tables.py` prints both tables and re-derives both sets; its
`--selftest` fails if the game's answer ever stops being the one these constants were written
from.

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
| `census: a candidate with chr_type N has team_type T` | the team byte behind each kind, once per pairing |
| `hidden: a chr_type N character is out of the lock-on candidate set ...` | the filter fired |

The census is there because the feature can only be exercised by two players invading one
world, which no offline check can produce. If the filter never fires, those lines say whether
your own kind or the other invader's was something other than a hostile phantom -- which is the
question Seamless Co-op makes worth asking, since it runs its own session layer and has been
measured typing remote players `Local` (0). It has already earned its place once: the 2026-09-07
run's two census lines are what showed the rule of the day could not arm, and are what the
current rule was derived against.

The `team_type` line is there to settle a second question with the same log, and it needs no
invasion at all -- an ordinary co-op session already has a remote player to read. In the vanilla
path that byte is derived from `chr_type`: `CS::ChrIns::InitTeamType` takes it from a `RoleParam`
row that `CalculateRoleParamId` keys as `(vowType * 10000) + chrType`, so if every player reads
`chr_type` 0 they would all read the same team as well and the byte adds nothing. But Seamless
ships an explicit PvP-teams option, so its session layer may write teams itself. Two characters
sharing a `chr_type` with different team bytes means the team byte is the discriminator this
filter is missing; team bytes that track `chr_type` exactly mean it is derived, and the question
is closed rather than re-argued. Nothing in the rule reads it yet -- it is a measurement, not a
behaviour change.

## Status

Never yet run in a live double invasion. What changed on 2026-09-09 is that the rule can now
arm at all in the sessions this user plays: the 2026-09-07 run measured the local player at
`chr_type` 2 / `summonParamType` -12, the rule of the day required 15/16/18 or -3/-4/-5, and so
the filter was inert no matter who was standing in the world. Both values are members now, and
both are members because the game's own tables say they are hostile-phantom roles, not because
they were added to make a run fire.

What is still unmeasured is the candidate half. Remote players in an ordinary Seamless session
read `chr_type` 0, which is `Local` and is also what the host reads, so if a fellow invader also
reads 0 the filter still has nobody to hide and the candidate set must stay strict rather than
grow to cover it -- hiding 0 would hide the host, which is the one failure worse than doing
nothing. The census answers it on the first double invasion: a `census: first candidate with
chr_type N` line naming 2, 15, 16 or 18 means the filter has what it needs, and a `hidden:` line
means it fired.

The widened invading half carries one risk worth naming rather than burying: `Duelist` (2) is
now a member, and a Seamless session was measured putting the LOCAL player on it during ordinary
co-op. So the gate can arm when nobody is invading. That hides nobody by itself -- the candidate
still has to be a hostile phantom, and every candidate that session asked about was `Local` (0),
`Npc` (5) or `Unk7` (7) -- but if Seamless ever types a co-op partner `Duelist`, they would stop
being a lock-on target. `the_measured_seamless_session_hides_nobody` pins the measured case; the
partner case is the one to watch for in the log.

The candidate walk, the detour target, the offsets it reads (`ChrIns::chr_type` at `+0x68`,
`WorldChrManImp::mainPlayer` at `+0x1e508`, `GameMan::summonParamType` at `+0xd84`), the two
classification tables and the 1.16.2 -> 1.17 pair are all byte-proven in both images.
