# Lock-on filter: the reverse engineering, after the product was removed

**The `er-lockon-filter` DLL and its crate were deleted on 2026-09-11 by user directive**
("delete the lockon filter dll/crate. We want to keep all research and findings, but the product
needs to be removed"). This note is the surviving record. Nothing here is a plan to rebuild it;
it is the set of measured facts the crate carried, written down so deleting the code does not
delete the reverse engineering.

Everything below was byte-proven against `eldenring-deobf.bin` (1.16.2),
`eldenring-deobf-1.17.bin` (1.17.0) and `eldenring-deobf-1.17.1.bin` (the installed 1.17.1),
except where a line says it was measured at runtime. The one runtime claim that matters is
marked as such and is the reason the feature's first rule was wrong.

Related records that outlive this note: the 1.16.2 -> 1.17 pair for the detour target is row
`0x140713db0 -> 0x140714c00` in `docs/recon/rva-map-1162-to-1170.verified.tsv`, and
`scripts/er-character-type-tables.py` still prints both classification tables and re-derives both
constant sets from the image (`--selftest`).

## 1. Where the lock-on system decides who is a candidate

`CS::LockTgtMan`'s per-frame update is `FUN_140716260` (1.16.2), `0x1407170b0` on 1.17. It walks
the act-point list at `GLOBAL_ActPntMan + 0x10` and admits each point on exactly four tests, in
this order, before any distance or angle test runs:

```text
point+0x75 & 1                              the point is enabled
owner = PointOwner(point)                   and has a character behind it
CS::ChrIns::CanTargetTeamType(me, owner)    whom my team may target
owner != me                                 and who is not me
```

Then a distance test, then the candidate's own `disableLockOnAng` cone, and only then is the
point inserted into the candidate set.

Two consequences worth keeping:

* A point whose owner resolves to null is skipped **before** `CanTargetTeamType` is asked. So a
  candidate removed at the `PointOwner` step is genuinely gone from the walk, not merely
  deprioritised. This is the fact that made the 2026-09-10 failure diagnosable (section 6).
* `CSChrAutoHomingModule` is fed from inside that same admitted-candidate block. Anything removed
  from the candidate set also stops being an auto-homing target, which is why a swing stops
  bending toward a character the lock-on no longer offers.

## 2. The seam: the point-owner resolver

`PointOwner` -- `ChrIns *FUN_140713db0(ActPnt *point)` -- is **1.16.2 `0x140713db0`, 1.17
`0x140714c00`** (RVA `0x713db0` / `0x714c00`). The 1.16.2 Ghidra dump has no name for it and the
1.17 dump has no names at all, so the crate named it for what it answers.

The whole function:

```c
if ((point->fieldInsHandle & 0xf0000000) != 0x10000000) return nullptr;
return CS::WorldChrManImp::GetChrInsFromHandle(GLOBAL_WorldChrMan, &point->fieldInsHandle);
```

Why this was the chosen seam:

* `nullptr` is already the function's own vocabulary for "no character owns this point", so every
  caller has a branch for it and returning it for a chosen character costs the engine nothing it
  does not already handle.
* It has **eleven call sites and all eleven are lock-on code**: five inside the update above, two
  building the `NetSyncData` for the currently held target, two `LockTgtMan` accessors reading
  flags off the owner. Nothing else in the image calls it. A detour here is therefore a statement
  about targeting alone rather than about the character.

The two alternatives, and why both were rejected:

* `CS::ChrIns::CanTargetTeamType` is the test that actually rejects a candidate, but it has **21
  call sites across ai targeting and damage**, so a detour there decides far more than lock-on.
* Rewriting the team-relation matrix cell for two invaders to `Friend` reaches those same 21 sites
  through the data instead of the code, and would additionally stop the two invaders damaging
  each other.

### Prologue

The first 13 bytes at that entry are **identical in 1.16.2 and 1.17**:

```text
40 53                push rbx        (redundant-prefix form the compiler emitted)
48 83 ec 20          sub  rsp,0x20
8b 41 78             mov  eax,[rcx+0x78]   (the 8b rm32 direction)
48 8d 59 78          lea  rbx,[rcx+0x78]
```

up to and including the load of the point's `FieldInsHandle`. The crate generated these from
named `iced-x86` instructions via `build-support/prologue_build.rs` rather than hand-typing them,
because two of the encodings have a plausible alternative spelling that differs by one byte and
would have disarmed the byte check on every launch. `rva-map-1162-to-1170.verified.tsv` records
the pair as `IDENTICAL-WHOLE` over 27 compared instructions, which is why one pin covered both
builds.

## 3. Struct offsets, each with its evidence

Every one of these was established from a getter whose whole body is unique in both images -- that
uniqueness is what says the constant inside it did not move between builds, so the field could be
read directly instead of calling through a translated address.

| field | offset | how it was proven |
| --- | --- | --- |
| `CS::ChrIns::chr_type` | `+0x68` | `CS::ChrIns::GetCharacterType` (1.16.2 `0x1403eec10`, 1.17 `0x1403eee40`) is `mov eax,[rcx+0x68]` in both images |
| `CS::ChrIns::teamType` | `+0x6c` | `CS::ChrIns::GetTeamType` opens `48 89 5c 24 10 57 48 83 ec 20 0f b6 41 6c 48 8b f9 88 02` at 1.16.2 `0x1403f1a60`; that exact 19-byte sequence has a unique hit at `0x1403f1c90` in `eldenring-deobf-1.17.1.bin`. The `0f b6 41 6c` in the middle is the load |
| `CS::GameMan::summonParamType` | `+0xd84` | `CS::GameMan::GetSummonParamType` (1.16.2 `0x140679300`, 1.17 `0x14067a150`) is a null check and one load; body byte-identical in both bar the singleton displacement (`GLOBAL_GameMan` 1.16.2 `0x143d65f88`-era, 1.17 `0x143d6d988`, the destination already recorded for `GAME_MAN_SINGLETON_RVA`) |
| `CS::WorldChrManImp::mainPlayer` | `+0x1e508` | centralised as `WORLD_CHR_MAN_PLAYER_INS_OFFSET` in `er-game-base` |
| `CS::PlayerIns::sessionManagerPlayerEntry` | `+0x6b8` | `GetSessionManagerPlayerEntry` is the whole of `mov rax,[rcx+0x6b8]; ret`; those 8 bytes `48 8b 81 b8 06 00 00 c3` occur exactly once in `eldenring-deobf.bin` (`0x140657b20`) and exactly once in `eldenring-deobf-1.17.1.bin` (`0x140658970`). `er-player-name-filter` pins the same offset |
| `CS::PlayerIns::playerGameData` | `+0x580` | `GetPlayerGameData` is the whole of `mov rax,[rcx+0x580]; ret`; `48 8b 81 80 05 00 00 c3` occurs exactly once at `0x1406563d0` (1.16.2) and once at `0x140657220` (1.17.1) |
| `PlayerGameData::multiplayRole` | `+229` | `CS::PlayerIns::GetMultiplayRole` (1.16.2 `0x140655fd0`) returns exactly this field. The **live** role, not the session entry's `preCeremonyMultiplayRole` |
| `PlayerGameData::characterType` | `+152` | Ghidra types it `CharacterType`, length **4**. A byte read is wrong: `MultiplayProperties` carries `-1` on the invalid-sign row, which a byte read reports as 255 |
| `PlayerGameData::isMainPlayer` | `+2288` | `er-player-name-filter` pins the same offset, which is what says the two crates were reading one struct |
| `SessionManagerPlayerEntry::steamId` | `+0x10` | `er-player-name-filter` pins the same layout; its `session_manager_entry_layout_matches_copy_offsets` test held the two crates together |
| `SessionManagerPlayerEntry::steamName` | `+0x18` | a `DLInplaceStr`; backing pointer at `+0x08` and length in text units at `+0x10` inside the `DLTX::DLString`, inplace capacity 64 UTF-16 units |
| `SessionManagerPlayerEntry::isHost` | `+232` | Ghidra's typed `CS::SessionManagerPlayerEntry` |
| `SessionManagerPlayerEntry::isLocalPlayer` | `+233` | as above |
| `SessionManagerPlayerEntry::preCeremonyMultiplayRole` | `+252` | as above |

### One offset that is right and whose field is useless

`PlayerGameData::usedInvasionItemType` at **`+2705` (`0xa91`)**. The offset is correct --
`MultiplayType::GetByInvasionItemType` (1.16.2 `0x1401dafb0`) is `movzx edx, byte ptr [rdx+0xa91]`
and branches on 0, 1, 2 for `BloodyFinger`, `FesteringBloodyFinger` and `Recusant`. But the live
readings were **161, 182 and 186** on three different people, none of them a member of that enum.
Seamless Co-op runs its own invasion plumbing and evidently never writes the field, so anything
read there is whatever the allocation happened to hold. Recorded here so the next person who finds
the offset does not spend the measurement again.

## 4. The two classification tables

Neither constant set below was written from a wiki or from the list of invasion items. Both are
the game's own answer, read out of tables the engine itself consults.

### `CharacterTypeProperties` -- who is a hostile phantom

**23 records of 20 bytes, 1.16.2 `0x143b17c00`, byte-identical at 1.17.1 `0x143b1bc00`.** The
`isHostilePhantom` byte sits at record `+0xa` and is what
`CS::CharacterTypeProperties::IsHostilePhantom` (1.16.2 `0x1404c7d10`) reads.

The table answers **true for `ChrType` 2, 15, 16, 18, 20, 21, 22**:

| `ChrType` | name |
| --- | --- |
| 2 | `Duelist` |
| 15 | `BloodyFinger` |
| 16 | `Recusant` |
| 18 | `FesteringBloodyFinger` |
| 20 | `BloodyFingerNpc` |
| 21 | `RecusantNpc` |
| 22 | (third npc kind) |

The crate used `{2, 15, 16, 18}`: the three npc kinds were dropped because the game spawned them
and an invader may legitimately want to lock one.

`Duelist` (2) being a member is a correction, and it is the one that mattered. An earlier
hand-written list excluded it on the reasoning that "a duelist was summoned by the host". The
game's own row disagrees, and so does the role table -- `MultiplayProperties` role 2 is `Chi Zhao Huan `,
a red summon, which fights beside the invaders. It is also the value the live census actually
measured on the local player under Seamless Co-op.

### `MultiplayProperties` -- which role produces which kind

**32 records of 64 bytes, 1.16.2 `0x143b11230`, 1.17.1 `0x143b15230`**, walked by
`GetMultiplayPropertiesByMultiplayRole` (1.16.2 `0x1401db340`). Each row pairs the
`SummonParamType` the engine matched the session on with the `CharacterType` it derives from it.
So the table is the derivation `summonParamType -> chrType`, and it can be read either way round.

Joining it against the hostile-phantom set above gives two derived lists:

* **`SummonParamType` values whose row resolves to a hostile phantom** (19 of them):
  `-2, -3, -4, -5, -8, -10, -11, -12, -16, -17, -18, -19, -21, -22, -23, -25, -26, -29, -30`.
  Named members: `-3 RedInvasionA` (Bloody Finger), `-4 RedInvasionALimited` (Festering finger),
  `-5 RedInvasionB` (Recusant), `-12` = `MultiplayProperties` role 12, debug name
  `ano-rumatsupuShou Hu `, whose `CharacterType` is `Duelist`.
  `Host` (0), `Summon` (-1) and the hunter roles are absent because their rows resolve to `Local`,
  `WhitePhantom` or `BluePhantom` -- not because anyone judged them friendly.
* **`MultiplayRole` values whose row resolves to a hostile phantom** (16 of them):
  `2, 3, 4, 5, 9, 10, 11, 12, 17, 18, 19, 20, 26, 27, 30, 31`.
  Role **0 is deliberately not a member**: it is what a `PlayerGameData` holds before anyone is
  assigned a role, so treating it as an invader takes the host off the lock-on list -- the one
  outcome worse than the filter doing nothing. Role 3 is `Luan Ru Chi _A`, the Bloody Finger invasion.

Friendly/neutral roles confirmed as non-members: `0` (none assigned), `1` white summon, `6`
berserker white, `7` red hunter, `8` sinner hero white, `13` avatar battle, `15` ceremony summon,
`21` white NPC summon, `25` NPC pseudo-multi white, `29` red hunter 2. Also non-invading:
`-1 Summon` (WhitePhantom), `-9` red hunter (BluePhantom), `-14` battle royale, `-28` red hunter 2
(BluePhantom).

`scripts/er-character-type-tables.py` prints both tables and re-derives both sets; its
`--selftest` fails if the game's answer stops being the one these numbers were written from. That
script is kept.

## 5. The rule the crate encoded

Hide a lock-on candidate when **all three** hold:

1. the candidate is a `CS::PlayerIns` (a hard precondition, see below);
2. the local player is invading -- `chr_type` in the hostile-phantom set **or** `summonParamType`
   in the 19-value set. Either alone is enough;
3. the candidate is a hostile phantom -- `chr_type` in the hostile-phantom set **or**
   `multiplayRole` in the 16-value set. Either alone is enough.

The asymmetry between (2) and (3) was deliberate and is the part worth keeping. A false yes on the
*local* half hides nobody, because the candidate still has to be a hostile phantom; a false yes on
the *candidate* half takes a character off the lock-on list. So the local half was widened freely
and the candidate half stayed keyed to the two derived tables.

**Why a non-player candidate was never hidden.** The sets are `chr_type` numbers, and nothing
stops the game giving a non-player character a number the hostile-phantom set holds. Without the
precondition, an ordinary enemy becomes untargetable -- a far worse fault than the one being
fixed, and one that shows up in normal play rather than only during an invasion.

**How "is this a player" was answered**, and this is reusable: a **vtable identity test**, not a
`chr_type` test. Every player in the world is a `CS::PlayerIns` -- the RTTI carries one class for
all of them (`.?AVPlayerIns@CS@@`, 1.17.1 `0x143c85de8`), with no separate kind for a remote or a
main player -- while a non-player character is a `CS::EnemyIns` (`0x143c84e70`). So the main
player's own vtable word identifies the class for the whole session: capture it once from
`WorldChrManImp+0x1e508`, then compare each candidate's first word against it. That costs one load
and one compare, needs no pinned address, and cannot drift between builds. `chr_type` cannot do
this job at all, because it is exactly the field a session layer rewrites.

**Reading `mainPlayer` directly rather than calling `GetMainPlayerIns`** was also deliberate: the
getter answers the debug-camera override first, and with a possession mod loaded that override is
the creature being worn, not the person invading.

## 6. Why `chr_type` alone is wrong under Seamless Co-op

This is the substantive finding, and it cost a whole live invasion to learn. Tracked as bd
`er-effects-rs-hgfy`.

**Run of 2026-09-10.** The invader at the keyboard was `chr_type` 2 with `summonParamType` 0. The
gate armed and `hidden:` fired **4096 times** -- and every fellow invader in the world was still
lockable. Those two facts fit together exactly one way. Because the candidate walk skips a
null-owner point *before* `CanTargetTeamType` (section 1), a hidden candidate is genuinely gone;
so the players who were being locked had never matched the `ChrType` set in the first place, and
the 4096 hidden candidates were somebody else.

**The cause.** Under Seamless Co-op a remote player reads `chr_type` **0** (`Local`) -- which is
also what the host reads. No `chr_type` rule can separate a fellow invader from the host, because
the field says the same thing about both.

**The fix, measured.** Read `PlayerGameData::multiplayRole` per candidate as well. In run
`br-20260910-162621-9e8e`, same character, same Seamless session:

```text
hidden: a chr_type 0 multiplay_role 3 character is out of the lock-on candidate set
        while you are chr_type 2, summon param type 0
```

Role 3 is `Luan Ru Chi _A`, the Bloody Finger invasion. In the same session the host read role 0 and
their co-op phantom read role 1, and neither was hidden. So the per-person role **does**
discriminate where `chr_type` does not, and that is the durable finding.

**An earlier identity census that failed completely.** Before the role term, the crate read
`isHost`, `isLocalPlayer` and `preCeremonyMultiplayRole` off `SessionManagerPlayerEntry`. All
three offsets are right -- Ghidra's typed struct puts them exactly there -- and all three were
useless: **six distinct entries in one invasion each reported `is_host=true`,
`pre_ceremony_role=0`, and most of them `is_local_player=true`.** Under Seamless those booleans
say the same thing about everybody. The Steam ID at `+0x10` is the only field in that entry that
reliably names a specific human.

**The earlier three-value lists could never arm at all.** A live Seamless session on 2026-09-07
measured the local player at `chr_type` 2 / `summonParamType` **-12**, and the hand-written lists
(three invasion `ChrType`s and their three `SummonParamType`s) held neither value. That
measurement is bd `seamless-types-the-local-player-duelist-2-summonparam-minus12-2026-09-07`.

**The residual risk, named rather than buried.** `Duelist` (2) is a member of the hostile-phantom
set, and a Seamless session was measured putting the **local** player on it during ordinary co-op.
So the "am I invading" gate can arm when nobody is invading. That hides nobody by itself -- the
candidate still has to be a hostile phantom -- but if Seamless ever types a co-op partner
`Duelist`, or hands them an invader role, that partner would stop being a lock-on target.

## 7. The team byte: an open question, with the evidence for both sides

`ChrIns::teamType` (`+0x6c`) was read for the census and never by the rule. It is the obvious
candidate for a discriminator that survives Seamless, and the static evidence is genuinely
two-sided. Recorded here because the question was never closed:

* **Against it.** In the vanilla path the byte is *derived from* `chrType`, not independent of it.
  `CS::ChrIns::InitTeamType` (`0x1403f7580`) resolves the character's `RoleParam` row and takes
  `RoleParam::GetTeamType`, and `CalculateRoleParamId` (`0x1404d8320`) keys that row as
  `(vowType * 10000) + chrType`. A session that leaves every player at `chrType` 0 gives every
  player the same team byte, and the byte adds nothing.
* **For it.** Seamless ships an explicit `OPTIONSELECT_TOGGLEPVPTEAMS` option, so its session layer
  may write the byte itself rather than let the game derive it. Note that `CanTargetTeamType` and
  the damage table calling two player teams `Friend` is a statement about the *relation* between
  teams, not about whether two characters carry different *values* -- only the second question
  matters here.

**The measurement that would settle it needs no invasion**: an ordinary co-op session already has
a remote player to read. Two characters sharing a `chr_type` with different team bytes means the
team byte is the discriminator; team bytes that track `chr_type` exactly mean it is derived and
the question is closed.

## 8. Engineering notes worth keeping

* **Hand the hook an unresolved address.** `er_hook::register_union_hook` resolves the target for
  the running build itself. Passing it an already-resolved address resolves twice -- silently --
  and lands the detour on a third function whenever a region's shift equals the local spacing
  between two entries. `scripts/check-double-resolved-hook-targets.py` is the gate for this. The
  byte-check of the prologue, by contrast, *must* read the resolved address.
* **Fail closed in three distinguishable ways**, so a log line says which: `REFUSED` (no
  detour-safe mapping for the running build; nothing read, nothing installed), `DISARMED` (the
  address resolved but the bytes are not the verified prologue -- most likely another mod detoured
  the same entry first), and a refusal to install when the `WorldChrMan` singleton has no address.
  A missing `GameMan` singleton stood down only the `summonParamType` half rather than the whole
  filter.
* **A game-directory log is single-slot.** `er_game_base::log::begin_fresh_run` rotates `<name>` to
  `<name>.prev` and truncates on the first write of each process, so two launches destroy the run
  before last. This crate wrote straight into the game directory until 2026-09-09: reconstructed
  from the launcher logs afterwards, **43 separate runs had loaded the DLL and every one of their
  logs was gone except the last two** -- neither of which was an invasion. The knob
  (`ER_QUICKLOAD_LOCKON_FILTER_LOG_PATH`) had to be spelled inline at the call site rather than
  behind a constant, because `scripts/er-artifact-redirect-audit.py` discovers launcher knobs by
  reading the Rust for that exact call shape with a literal.
* **Read `chr_type` as a raw `i32`, never as a constructed enum.** A session layer may type a
  character in a way the vanilla enum never anticipated; a raw integer can be surprised where a
  constructed enum would be undefined behaviour.
* **A per-role census budget, not a shared one.** A single shared budget was spent by the local
  player: every world reload gives a fresh `SessionManagerPlayerEntry`, so a session that invaded,
  returned and invaded again burned four of eight slots on rows that all said the same thing, and
  the census came within two people of going silent before the row it existed to capture could
  arrive.

## 9. Status at deletion

The feature was **never proven to work end to end**. The seam, the offsets, the two tables and the
1.16.2 -> 1.17 pair were all byte-proven in both images, and the role term was observed firing
correctly on one candidate in run `br-20260910-162621-9e8e`. What was never observed is the whole
statement -- an invader unable to lock on to other invaders while the host and their phantoms
remain lockable -- because that needs two players invading one world and holding it long enough to
check both halves.
