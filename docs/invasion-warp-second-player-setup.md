# Setting up `er_invasion_warp_dll.dll` for a two-player location test

Two roles. They need different things, and the host's side is the easy one.

## If you are the HOST (the one being invaded)

**Configure nothing.** Load the DLL and play.

The DLL publishes your current map onto your Steam lobby every tick, and that path reads no
config at all -- it is not gated on `enabled`, not gated on `hunt`, and it does not need the
`er-invasion-warp.toml` file to exist. A config file will appear next to the DLL on first run;
you can ignore it.

If your loader is me3, add the DLL as a native alongside Seamless:

```toml
profileVersion = "v1"

[[supports]]
game = "eldenring"

[[natives]]
path = 'C:\path\to\SeamlessCoop\ersc.dll'

[[natives]]
path = 'C:\path\to\er_invasion_warp_dll.dll'
```

Any injector works -- it is an ordinary native DLL -- but it must load into the same process as
Seamless, and Seamless must be present or the filter half never arms.

### What must match, and what famously does not

These are Seamless's own matchmaking rules, not ours, and they will make you invisible to each
other no matter what this DLL does:

1. **The same `regulation.bin`.** Invaders filter on `lobby_key`, and that key is a SHA-256 over a
   fingerprint of your PARAM TABLES plus the Seamless build -- reversed out of `ersc.dll` v1.9.9
   (`BuildLobbyKey` @ RVA `0x0ABC20`). So any mod that edits `regulation.bin` makes you invisible
   to everyone who has not made the byte-identical edit. That is the single most common reason two
   people who "did everything right" never see each other.
2. **The same matchmaking bracket.** There is a `matchmaking_breakin_lobby_...` term carrying a
   value like `5_3` -- measured, both sides asked for and carried `5_3`. It tracks character
   level/weapon level. A wildly different character may never match.

**Your co-op passwords do NOT need to match.** This doc previously said they did, and that was
wrong. The password is parsed and is mandatory -- Seamless refuses to start without one -- but it
is AES-encrypted into a different object that feeds the session manager, and it never enters the
lobby key. Falsified two ways: the captured key does not equal `sha256(password + salt)`, and a
player running a different password invaded us live on 2026-08-06.

### Being invadable at all

Measured on a live host: the lobby key `lobby_breakin_lobby_ykssr_199_6` must read `true`, and
that is set by opening your world -- a Tiny Great Pot / "open to wanderers", or being in co-op.
A closed solo world reads `false` and no invader's query returns you.

### Checking it worked

`er-invasion-warp-dll.log`, next to the game exe, should show:

```
lobby-publish: er_invasion_warp_map = m12_02_00_00 on lobby 0x186000016dfa0f7 (#1, read back)
```

`read back` means the write survived the server round trip. If instead you see `REFUSED`, the
line says which check failed and why -- no guessing required.

## If you are the INVADER -- invading someone at the place you are standing

This is the path that is **proven working against real hosts** (2026-08-06). Edit
`er-invasion-warp.toml` in your `ELDEN RING\Game` folder and change exactly two lines:

```toml
[local_invasion]
enabled = true      # master switch -- OFF by default, because this cancels real matches
mode = "area"       # this place, or anywhere sharing its name
```

Leave `hunt = false`. See the note below for why.

Two optional extras, both off by default:

```toml
reject_notice   = true    # put each rejection on screen: "Rejected m60_42_36_00 (elsewhere)"
dll_users_only  = false   # see the warning below before turning this on
```

`reject_notice` is worth turning on the first time you use the mod. It names WHY a match was
refused, and one of the reasons -- `(open your map)` -- is a mistake that otherwise looks exactly
like "nobody is around": until you open the world map once per session, no destination has a name,
so every name-based judgement fails closed and you silently reject everyone.

It writes to the game's **own auto-closing announcement line** -- the one that says "Grace
discovered". It appears, scrolls, and expires on its own. No dialog and no button.

> **If you are on a build before 2026-08-06, leave this OFF.** Earlier builds routed this through
> `showPopupMenu`, which is a blocking modal: you got a dialog to dismiss for *every* rejection,
> showing empty boxes rather than text, and leaving it unattended stalled the Seamless handshake
> long enough for the mod to cancel the attempt. It defaults to `false`, so you are only exposed if
> you turned it on deliberately.

Then, in game:

1. **Walk to the place you want to invade.** Where you are standing IS the target -- there is
   nothing to mark or type. The filter compares every incoming match against your own location.
2. **Open your world map once.** `mode = "area"` reads place names off the map's own rows; before
   you have opened it no location has a name and every name-based judgement fails closed. Once per
   session is enough. (`mode = "exact"` never needs the map, but it compares raw blocks, so a large
   area split across several blocks can reject someone standing near you.)
3. **Search for invasions** the normal Seamless way.
4. **Expect it to take several tries, and let it.** Each match at the wrong place is cancelled and
   the search restarts by itself. A real run took five matches to land: four elsewhere, refused,
   then one at the right block. Cancelling is safe -- the session returns to idle and searching
   continues.
5. **To stop, press Seamless's own "Cancel search."** Your cancel beats the loop; it will not
   restart behind you.

Marking is optional and only WIDENS what is allowed: `Insert` on a spot adds it to
`allowed_blocks`, so it is accepted from then on wherever you later stand. `Delete` excludes a
place, and an exclusion beats everything.

### What this guarantees, and what it does not

It guarantees **location, not identity.** The accept reason is that the destination equalled your
location. If a stranger is standing where your friend is, you will invade the stranger and the
result is identical. Location-targeted, not person-targeted.

### Why `hunt` stays off

`hunt = true` narrows the Steam query itself, so the right host arrives on the first try instead
of the fifth. Two reasons it is not the recommended path:

* It filters on a key only this DLL publishes, so while it is on you will **not** see hosts who
  are not running the DLL -- and almost nobody is.
* As of 2026-08-06 the two halves are proven separately but never joined: a host on a second
  machine was captured publishing the correct block (`er_invasion_warp_map = m60_43_34_00`) onto
  the lobby invaders query, and the filter is known to reach the wire -- but nobody has yet run a
  query *filtered* on that key and had that host come back.

The reject loop needs none of it. It reads the destination Seamless pushes to *you*, so it works
against completely vanilla hosts who have never heard of this DLL.

## The two switches are independent, which gives you three modes

`enabled` filters by LOCATION. `dll_users_only` changes WHO IS IN YOUR POOL. They do not gate each
other, so:

| `enabled` | `dll_users_only` | what you get |
|---|---|---|
| `true` | `false` | Filter by location, meet everybody. The default use, and the one proven live. |
| `false` | `true` | **Invade anywhere exactly as unmodded -- but only ever meet other DLL users.** A private global community with ordinary invasion inside it. |
| `true` | `true` | Only DLL users, and only at the place you are standing. |
| `false` | `false` | The DLL does nothing to matchmaking. |

The middle row costs you nothing in gameplay terms: no rejections, no grind, no waiting for the
right location. It only narrows the population.

Whichever you pick, `dll_users_only` is **symmetric and absolute**. While it is on, vanilla players
cannot see you and you cannot see them, for hosting as much as for invading -- because Seamless
finds worlds with a key we rewrite, and one value drives both the search and the advertisement.

### Measured live, 2026-08-06

Toggled mid-session on a real Seamless search, with nobody else in the world running this DLL:

| `dll_users_only` | searches | no match | matched |
|---|---|---|---|
| `false` (before) | 4 | 0 | **3** |
| `true` | 5 | **6** | 0 |
| `false` (after) | 1 | 0 | **1** |

The key rewrite was visible on the wire (`16ca67264987...` became `a23fc38f8a79...`), and turning
it back off restored matching on the very next query -- which is what makes this a measurement
rather than a coincidence. With the option on and no other DLL user alive, "only DLL users"
correctly resolved to "nobody".

**It takes effect on the NEXT search, not instantly.** The rewrite happens when a query is issued,
so a match already in flight keeps running -- one held for 33 seconds after the toggle. Set it
while idle, not mid-search, and have both people set it before either starts looking.

One more measured number, since it decides how patient to be: Seamless's retry between failed
searches is a **15-second wall-clock timer** (15003 ms across six samples, +-0.22%). It is not
frame-based, so running the game faster does not speed it up.

## Who actually needs the DLL

**Only the invader.** Measured 2026-08-06: every host rejected and the one accepted were ordinary
Seamless players -- twelve queries, five distinct lobbies, not one carrying our key. Host-side
publishing is not on the critical path for location-targeted invasion; it is an optimization for
cutting down the number of tries.
