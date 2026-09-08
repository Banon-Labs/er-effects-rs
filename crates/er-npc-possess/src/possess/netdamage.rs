//! Why a possessed creature cannot hurt another player, and why they cannot hurt it back.
//!
//! The user's report was "I can't damage other players in a seamless invasion", then "they can't
//! damage me either -- neither HP bar goes down". Both halves have an answer in the binary, and
//! they are not the same answer. This module carries the first one as data, because it is a
//! decision the game makes from a table and a table can be transcribed and tested.
//!
//! # The routing matrix
//!
//! A chr-vs-chr hit that survives the team/immunity filter reaches the victim's
//! `CSChrDamageModule` vtable slot 7. For an ordinary character that is `FUN_140445060` (1.16.2
//! `0x140445060`, 1.17 candidate `0x1404455c0`); the player's `CSPlayerDamageModule` overrides it
//! with `FUN_14044caf0` (1.17 `0x14044d050`), which applies the hit immediately when the victim is
//! the local main player and otherwise falls through to the same generic routine.
//!
//! That routine classifies both characters with `FUN_14044a1b0` (1.17 candidate `0x14044a710`)
//! into one of seven categories -- see [`Category`] -- and looks the pair up in a 7x7 table of
//! `u32` modes:
//!
//! ```text
//! mode = table[victimCategory * 7 + attackerCategory]
//! table = DAT_142a36400   when AttackDamageInfo+0x11c == 0     (1.17: 0x142a39430)
//!         DAT_142a364d0   otherwise                            (1.17: 0x142a39500)
//! ```
//!
//! Both tables were located in `eldenring-deobf-1.17.bin` by an exact 196-byte content match with
//! a unique hit, so the installed build routes damage the same way 1.16.2 does. What each mode
//! does is a second, tiny table (`DAT_142a36598`) plus the `if/else` chain around it -- transcribed
//! into [`Route`].
//!
//! # The one cell that decides the feature
//!
//! ```text
//! victim = a REMOTE player (category 2), attacker = an NPC (category 3 or 4)  ->  mode 0
//! ```
//!
//! Mode 0 is [`Route::ComputeOnly`]: it calls `FUN_140445f00`, which resolves the guard behaviour,
//! looks up the `AtkParam` row and computes the damage numbers into the `AttackDamageInfo` -- and
//! then stops. `DAT_142a36598[0]` has its high byte set, so `HitChr` is not called, so no HP is
//! subtracted; `cVar14` stays zero, so neither `Packet15` nor the vitals packet is built. The one
//! remaining call is `victim->GetManipulator()->vtable[+0x70](info)`, and that slot is
//! `FUN_1403cd970` -- a bare `ret` -- in both the `PadManipulator` vtable (`0x142a2b778`) and the
//! `NetworkManipulator` vtable (`0x142a2b1a8`). The hit is computed and dropped.
//!
//! **The only attacker category that can put damage on a remote player is category 1,
//! `CS::PlayerIns::IsMainPlayerIns` -- the local player's own `PlayerIns`.** That cell is mode 4,
//! [`Route::SendPvpDamage`], which calls `CSPlayerDamageModule` slot 21 (`0x14044ce40`, 1.17
//! candidate `0x14044d3a0`): build a `Packet15`, fetch the victim's `PlayerNetworkSession` and
//! `SendHitPacket`. Player-versus-player damage is resolved on the attacker's client and shipped
//! to the victim; enemy-versus-player damage is resolved on the victim's own client and never
//! sent at all (their row 1 column 3/4 is mode 1, apply locally).
//!
//! So a creature's attack has no path to another player's HP from the possessing player's machine,
//! by construction, for every creature. This is not creature-dependent, and it is not the bullet
//! path either: `FUN_14038b2f0`, the bullet-versus-chr resolver, ends in the same
//! `victim->damage->vtable[7](victimModule, bulletOwner, info)` call and therefore the same table,
//! with the bullet's owning `ChrIns` as the attacker.
//!
//! # Nor can it be forged
//!
//! Even a hit that reached mode 4 would not land. The `Packet15` receiver
//! (`WorldChrManImp` pump `FUN_14050e4a0`) resolves both handles before it touches anything:
//!
//! ```text
//! victim  = GetChrInsByP2PEntityHandle(packet.victim);   if (victim  == null) skip
//! dealer  = GetChrInsByP2PEntityHandle(packet.attacker); if (dealer  == null) skip
//! HitChr(victim->damage, dealer, info, true); SetHP(hp - packet.damage); SetStamina(...)
//! ```
//!
//! A creature this crate spawned has no handle the other client can resolve, and that is not an
//! inference. `CS::ChrSet`'s dynamic spawn (`FUN_140492a90`, reached from the crate's
//! [`crate::spawn::game::SPAWN_DYNAMIC_CHR_RVA`]) builds the character's `P2PEntityHandle` as
//! `P2PEntityHandle(&handle, blockId = -1, chrSetIndex, index)` -- the invalid `0xFFFFFFFF`
//! blockId the damage forwarder itself tests for (`if (handle.blockId != 0xffffffff)`). The
//! creature is a local identity in a local ChrSet slot, so `GetChrInsByP2PEntityHandle` on the
//! other machine has nothing to return and the packet would be discarded on arrival.
//!
//! Making the hit belong to the player would mean the attack itself being player-owned -- the
//! game's own mechanism for that is a player-owned bullet, not a relabelled NPC swing.
//!
//! Possessing a creature the map placed does not change the verdict either. That character has a
//! real handle, but the swing does not: this crate fires attacks by writing
//! `CSChrEventModule+0x18 requestAnimationId`, a field the engine consumes locally and publishes
//! nowhere. The other client's copy of that enemy is animating whatever its own owner drives it
//! to, so the attack the possessing player is watching exists on one machine.
//!
//! # The incoming half is a different mechanism, and the obvious suspect is not it
//!
//! It is tempting to blame this crate's own body neuter: `NpcPossessionEngine::neuter` sets
//! `ChrIns.chrFlags1c5 |= 0x10`, and `ChrIns::IsImmuneToAttack` reads exactly that bit. But the
//! bit lives in the possessing player's process, and the receive path quoted above calls `SetHP`
//! **unconditionally** -- there is no `IsImmuneToAttack`, no `FUN_1404443e0`, no team check on an
//! arriving `Packet15`. So the invincibility bit does not make a possessing player unkillable by
//! another player; what it refuses is every hit resolved locally, which for a player victim means
//! NPCs, environment and falls. Saying otherwise gets the fix wrong.
//!
//! What the other player has to hit is the player's own body, which co-location keeps at the
//! creature's root and which their client sees as an ordinary Tarnished standing there.
//!
//! # The team byte is not the blocker either
//!
//! Possession writes `ChrIns+0x6c teamType = 15 (Charmed)` to fix lock-on, and there are two
//! independent 79x79 `CSTeamTypeRelation` matrices, not one:
//!
//! | table | consulted by | 1.16.2 | 1.17 |
//! |---|---|---|---|
//! | damage | `CanTeamTypeDamageEachOther` / `canTeamTypeHitAnother` | `0x143b180e0` | `0x143b1c0e0` |
//! | targeting | `CS::ChrIns::CanTargetTeamType` | `0x143b243f0` | `0x143b283f0` |
//!
//! In the damage table, Charmed is `CSTeamTypeRival` against every player team (1, 2 and 4) in
//! both directions, and `Rival::Validate` returns true for an attack that sets `opposeTarget` --
//! which every ordinary attack does. In the targeting table those same cells are
//! `CSTeamTypeFriend`. Identical on both builds; `scripts/ghidra/team-relation-report.py`
//! reproduces it. So the team write costs lock-on against players and costs no damage.

// Pure: a transcription of two game tables plus a state machine over observations. Ungated so
// `cargo test -p er-npc-possess` proves it on the host with no game running.
#![cfg_attr(not(windows), allow(dead_code))]

/// How `FUN_14044a1b0` sorts a character for the purpose of routing damage.
///
/// The seven values are that function's seven returns, in its own order, so an index into
/// [`ROUTE_TABLE`] is `category as usize` with nothing in between to get wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Category {
    /// `param_1 == null`. An attack with no owning character -- environment, or a bullet whose
    /// owner has already gone.
    None = 0,
    /// `CS::PlayerIns::IsMainPlayerIns`. The player at this keyboard.
    MainPlayer = 1,
    /// The vtable predicate the dump names `isChrEventIdlessThan9998`, tested immediately after
    /// `IsMainPlayerIns` on every path that asks "is this a player at all" -- `FUN_1404443e0`,
    /// `FUN_14044caf0` and `CS::ChrIns::InitTeamType` all pair the two. Another human's character.
    RemotePlayer = 2,
    /// `IsNpc` and `FUN_1403f3e30`: an NPC this client is the network authority for.
    ///
    /// `FUN_1403f3e30` is true when the session is not networked at all, when the character is the
    /// main player or their mount, when `debugFlags & 0x10000000` is set, or when
    /// `FUN_140508b70(WorldChrMan, p2pHandle)` says the handle is locally owned.
    LocalNpc = 3,
    /// `IsNpc` and not the above: an NPC some other client owns.
    RemoteNpc = 4,
    /// Neither player nor NPC, locally owned.
    LocalOther = 5,
    /// Neither player nor NPC, remotely owned.
    RemoteOther = 6,
}

impl Category {
    /// Every category, for exhaustive tests and for iterating the table.
    pub(crate) const ALL: [Self; 7] = [
        Self::None,
        Self::MainPlayer,
        Self::RemotePlayer,
        Self::LocalNpc,
        Self::RemoteNpc,
        Self::LocalOther,
        Self::RemoteOther,
    ];

    /// What the log calls it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::None => "no-owner",
            Self::MainPlayer => "local player",
            Self::RemotePlayer => "remote player",
            Self::LocalNpc => "NPC (locally owned)",
            Self::RemoteNpc => "NPC (remotely owned)",
            Self::LocalOther => "other (locally owned)",
            Self::RemoteOther => "other (remotely owned)",
        }
    }

    /// Is this one of the two NPC categories?
    ///
    /// A possessed creature is always one of them, and which one does not matter to any cell this
    /// crate cares about -- columns 3 and 4 of the remote-player row are both mode 0. That is why
    /// this crate never has to resolve `FUN_1403f3e30`'s address to answer the user's question.
    pub(crate) const fn is_npc(self) -> bool {
        matches!(self, Self::LocalNpc | Self::RemoteNpc)
    }
}

/// What the game does with a hit, once the matrix has chosen a mode.
///
/// Transcribed from the `if/else` chain in `FUN_140445060` together with `DAT_142a36598`, whose
/// entry per mode is `(hitChrArgument, skipHitChr)`:
/// `01 01 | 00 00 | 00 00 | 00 00 | 01 00 | 01 00`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// Mode 0. `FUN_140445f00` computes the numbers; `HitChr` is skipped and no packet is built.
    /// **No HP moves anywhere.**
    ComputeOnly,
    /// Mode 1. `HitChr(.., false)`. Applied here and nowhere else.
    ApplyLocal,
    /// Mode 2. `HitChr(.., false)`, then `FUN_140c9f3d0` publishes the victim's super-armour and
    /// HP to the session.
    ApplyAndSyncVitals,
    /// Mode 3. `HitChr(.., false)`, then a `Packet15` to the victim's owner -- but only when
    /// `CS::ChrIns::IsValidForThrowNetworking` accepts the attacker.
    ApplyAndSendDamage,
    /// Mode 4. `CSPlayerDamageModule` slot 21 -> `Packet15` -> `PlayerNetworkSession::SendHitPacket`,
    /// then `HitChr(.., true)`. The player-versus-player path, and the only route that puts damage
    /// on another human's character.
    SendPvpDamage,
    /// Mode 5. `HitChr(.., true)` with no packet: a local prediction on a character somebody else
    /// owns.
    ApplyPredicted,
}

impl Route {
    /// Does the victim's HP change anywhere as a result of this hit?
    pub(crate) const fn reaches_victim(self) -> bool {
        !matches!(self, Self::ComputeOnly)
    }

    /// Does a damage message leave this machine?
    pub(crate) const fn sends_message(self) -> bool {
        matches!(
            self,
            Self::ApplyAndSyncVitals | Self::ApplyAndSendDamage | Self::SendPvpDamage
        )
    }

    /// What the log calls it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::ComputeOnly => "computed-and-dropped",
            Self::ApplyLocal => "applied locally",
            Self::ApplyAndSyncVitals => "applied + vitals published",
            Self::ApplyAndSendDamage => "applied + damage packet",
            Self::SendPvpDamage => "PvP damage packet sent",
            Self::ApplyPredicted => "applied as a local prediction",
        }
    }

    /// The mode number, for a log line that can be checked against the binary.
    const fn mode(self) -> u32 {
        match self {
            Self::ComputeOnly => 0,
            Self::ApplyLocal => 1,
            Self::ApplyAndSyncVitals => 2,
            Self::ApplyAndSendDamage => 3,
            Self::SendPvpDamage => 4,
            Self::ApplyPredicted => 5,
        }
    }

    const fn from_mode(mode: u32) -> Option<Self> {
        Some(match mode {
            0 => Self::ComputeOnly,
            1 => Self::ApplyLocal,
            2 => Self::ApplyAndSyncVitals,
            3 => Self::ApplyAndSendDamage,
            4 => Self::SendPvpDamage,
            5 => Self::ApplyPredicted,
            _ => return None,
        })
    }
}

/// `DAT_142a36400`, row-major as `[victimCategory][attackerCategory]`.
///
/// Read out of `eldenring-deobf.bin` at `0x142a36400` and byte-identical at `0x142a39430` in
/// `eldenring-deobf-1.17.bin`.
const ROUTE_TABLE: [[u32; 7]; 7] = [
    [0, 0, 0, 0, 0, 0, 0],
    [1, 1, 0, 1, 1, 1, 1],
    [5, 4, 0, 0, 0, 0, 0],
    [2, 2, 0, 2, 0, 2, 0],
    [2, 3, 0, 3, 0, 3, 0],
    [2, 2, 0, 2, 0, 2, 0],
    [2, 3, 0, 3, 0, 3, 0],
];

/// `DAT_142a364d0`, the variant selected when `AttackDamageInfo+0x11c` is non-zero.
///
/// It differs from [`ROUTE_TABLE`] in exactly one cell -- a remote-player victim struck by the
/// local player is mode 1 rather than mode 4 -- and in none of the cells this crate depends on.
const ROUTE_TABLE_ALT: [[u32; 7]; 7] = [
    [0, 0, 0, 0, 0, 0, 0],
    [1, 1, 0, 1, 1, 1, 1],
    [5, 1, 0, 0, 0, 0, 0],
    [2, 2, 0, 2, 0, 2, 0],
    [2, 3, 0, 3, 0, 3, 0],
    [2, 2, 0, 2, 0, 2, 0],
    [2, 3, 0, 3, 0, 3, 0],
];

/// Which mode the game would choose for this pair.
///
/// `alt` selects `DAT_142a364d0`, i.e. `AttackDamageInfo+0x11c != 0`. This crate always asks with
/// `false`, because the two tables agree on every cell it reads and guessing the flag would be
/// inventing a fact.
pub(crate) fn route(attacker: Category, victim: Category, alt: bool) -> Route {
    let table = if alt { &ROUTE_TABLE_ALT } else { &ROUTE_TABLE };
    let mode = table[victim as usize][attacker as usize];
    // Every cell in both shipped tables is 0..=5, which the test below asserts, so the fallback is
    // unreachable rather than a silent default -- and it is `ComputeOnly` so an impossible mode
    // reports "nothing happened" rather than claiming damage landed.
    Route::from_mode(mode).unwrap_or(Route::ComputeOnly)
}

/// The sentence the log prints for one attacker/victim pair.
///
/// It names the mode number so the line can be checked against the table in the binary, and it
/// says both of the things the report asked to be readable: whether the victim's HP moves, and
/// whether a damage message leaves this machine.
pub(crate) fn verdict(attacker: Category, victim: Category) -> String {
    let route = route(attacker, victim, false);
    format!(
        "{} -> {}: mode {} ({}), victim HP changes={}, damage message sent={}",
        attacker.name(),
        victim.name(),
        route.mode(),
        route.name(),
        yes_no(route.reaches_victim()),
        yes_no(route.sends_message()),
    )
}

const fn yes_no(value: bool) -> &'static str {
    if value { "YES" } else { "NO" }
}

/// Which attacker kinds can put damage on `victim`'s authoritative HP, derived from the table
/// rather than asserted.
///
/// Printed once per possession because it is the answer to "why can I not hurt them": the list
/// for a remote-player victim has exactly one entry, and it is not the category a possessed
/// creature is in.
///
/// "Authoritative" is what makes this different from [`Route::reaches_victim`], and the
/// distinction is not pedantry -- it is the reason a naive reading of the table gets the wrong
/// answer. A character somebody else's machine owns has its HP decided there, so a local
/// `HitChr` on our copy of it is a prediction that their next sync overwrites; only a damage
/// message changes the number they see. That is why a null attacker (mode 5, `ApplyPredicted`)
/// shows up as reaching a remote player and still cannot hurt one. For a character this machine
/// owns -- the local player above all -- the local application is the damage.
pub(crate) fn attackers_that_can_reach(victim: Category) -> Vec<Category> {
    Category::ALL
        .into_iter()
        .filter(|attacker| {
            let route = route(*attacker, victim, false);
            match victim {
                Category::RemotePlayer | Category::RemoteNpc | Category::RemoteOther => {
                    route.sends_message()
                }
                _ => route.reaches_victim(),
            }
        })
        .collect()
}

/// One character seen in the world this frame, reduced to what the routing question needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Seen {
    /// `ChrIns` address, the identity the ledger keys on.
    pub(crate) address: usize,
    /// Whether `WorldChrMan.main_player` names this character.
    pub(crate) is_main_player: bool,
    /// `ChrIns.chr_type`, as the raw value, so an unmapped one is reported rather than dropped.
    pub(crate) chr_type: i32,
    /// `ChrIns+0x6c teamType`, or `None` when the byte did not read.
    pub(crate) team: Option<u8>,
    /// `CSChrDataModule+0x138 hp`, or `None` when the module did not read.
    pub(crate) hp: Option<i32>,
}

impl Seen {
    /// Which routing category this character is, as far as this crate can tell without resolving
    /// `FUN_1403f3e30`'s address.
    ///
    /// Everything in `WorldChrMan.player_chr_set` that is not the main player is another human's
    /// character, which is exactly category 2.
    pub(crate) const fn category(self) -> Category {
        if self.is_main_player {
            Category::MainPlayer
        } else {
            Category::RemotePlayer
        }
    }
}

/// The last player-versus-player damage packet this process received, as the game's own pump
/// left it in its static buffer.
///
/// See [`crate::possess::layout::packet15_receive`] for where it lives and why it can be read.
/// This is the instrument that splits the remaining question in two: their damage never arrived,
/// or it arrived and something here refused it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct ReceivedPacket {
    /// The attacker's `P2PEntityHandle`, both halves as one `u64`.
    pub(crate) dealer: u64,
    /// The victim's handle. On a packet aimed at us, ours.
    pub(crate) victim: u64,
    /// HP the pump subtracted. `i16` because the pump reads it with `MOVSX .., word ptr`.
    pub(crate) damage: i16,
    /// Stamina the pump subtracted.
    pub(crate) stamina: i16,
}

impl ReceivedPacket {
    /// Has anything ever landed in the buffer?
    ///
    /// The pump zeroes the sixteen-byte block containing the damage on init and only ever writes
    /// it from a dequeued packet, so an all-zero reading means no PvP damage packet has been
    /// received in this process -- which is a finding, not a missing measurement.
    pub(crate) const fn is_empty(self) -> bool {
        self.dealer == 0 && self.victim == 0 && self.damage == 0 && self.stamina == 0
    }

    /// The log form: what arrived, or that nothing has.
    pub(crate) fn describe(self) -> String {
        if self.is_empty() {
            return "NOTHING -- no PvP damage packet has reached this process at all".to_owned();
        }
        format!(
            "dealer={:#018x} victim={:#018x} damage={} stamina={}",
            self.dealer, self.victim, self.damage, self.stamina
        )
    }
}

/// How often the ledger re-reads the player set, in frames.
///
/// The set holds at most a handful of characters so the walk is cheap, but the log opens and
/// closes its file per line and a per-frame HP report on six players would be several hundred
/// writes a second. Deltas are still exact: the ledger compares against its own last sample, so a
/// slower cadence loses resolution in time, never in total.
const SAMPLE_FRAMES: u64 = 30;

/// One remembered player, so the next sample can be a delta rather than a number.
#[derive(Clone, Copy, Debug)]
struct Tracked {
    address: usize,
    hp: Option<i32>,
}

/// The per-possession ledger: who else is in the session, and whether their HP is moving.
///
/// It cannot observe a hit -- that would need a detour on the damage path, and this crate claims
/// no prologue. What it observes is the consequence, which is the thing the user actually reported
/// as missing: "neither HP bar goes down". A line saying a remote player's HP has not moved across
/// N samples, next to the routing verdict that says it cannot, turns "I can't damage them" into
/// something readable.
#[derive(Default)]
pub(crate) struct Ledger {
    tracked: Vec<Tracked>,
    /// Has the standing verdict been printed for this possession?
    reported_verdict: bool,
    /// The frame the last sample was taken on.
    last_sample: Option<u64>,
    /// How many samples have been taken, so a report can say "unchanged across N".
    samples: u64,
    /// The receive oracle'S last reading, so an arrival is reported as a change.
    ///
    /// `None` until the first sample, and `Some` of an empty packet when the buffer has never
    /// been written -- different states, not to be collapsed.
    last_packet: Option<ReceivedPacket>,
    /// Has the "their damage is not arriving" line been said?
    reported_silence: bool,
}

impl Ledger {
    pub(crate) const fn new() -> Self {
        Self {
            tracked: Vec::new(),
            reported_verdict: false,
            last_sample: None,
            samples: 0,
            last_packet: None,
            reported_silence: false,
        }
    }

    /// Whether this frame is a sampling frame.
    pub(crate) fn due(&self, frame: u64) -> bool {
        match self.last_sample {
            None => true,
            Some(last) => frame.saturating_sub(last) >= SAMPLE_FRAMES,
        }
    }

    /// Fold one observation of the world into the ledger and return the lines to log.
    ///
    /// `creature` is the category of the character being worn -- always one of the two NPC ones,
    /// and the caller says which only so the printed verdict names the right column.
    pub(crate) fn observe(
        &mut self,
        creature: Category,
        world: &[Seen],
        packet: Option<ReceivedPacket>,
        frame: u64,
    ) -> Vec<String> {
        self.last_sample = Some(frame);
        self.samples = self.samples.saturating_add(1);
        let mut lines = Vec::new();

        let remotes = world.iter().filter(|seen| !seen.is_main_player).count();
        if !self.reported_verdict {
            self.reported_verdict = true;
            lines.push(format!(
                "net-damage: {remotes} other player(s) in the session. The worn creature is {}. {}",
                if creature.is_npc() {
                    "an NPC as far as the game's damage router is concerned"
                } else {
                    "NOT resolving as an NPC, which is not a state this crate expects"
                },
                verdict(creature, Category::RemotePlayer),
            ));
            lines.push(format!(
                "net-damage: the ONLY attacker kind that can put damage on another player is \
                 [{}] -- the routing table gives every other kind mode 0, which computes the \
                 numbers and drops them. Their client resolves its own enemy hits, so the \
                 creature's swing exists only on this machine. See the module docs on \
                 possess::netdamage for the table and the addresses.",
                attackers_that_can_reach(Category::RemotePlayer)
                    .into_iter()
                    .map(Category::name)
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
            lines.push(format!(
                "net-damage: incoming is a different mechanism -- {}, and the arriving Packet15 \
                 is applied by WorldChrManImp's pump with SetHP directly, with no immunity or team \
                 check. The possession's invincibility bit refuses hits resolved HERE (NPCs, \
                 falls); it does not refuse another player's damage packet.",
                verdict(Category::MainPlayer, Category::RemotePlayer),
            ));
            lines.push(
                "net-damage: if NOBODY's HP moves below, read the team bytes. In the DAMAGE \
                 relation matrix the player teams 1, 2 and 4 are CSTeamTypeFriend to each other, \
                 so two players on any of them cannot hurt each other at all -- that is a session \
                 setting, not this mod. The worn creature's Charmed team is CSTeamTypeRival to \
                 all three, which DOES permit damage: the team write costs lock-on against \
                 players, never damage."
                    .to_owned(),
            );
        }

        // The receive oracle, reported as a CHANGE: the buffer holds the last packet forever, so
        // a repeated reading is not a repeated arrival.
        let local_hp_moved = world.iter().any(|seen| {
            seen.is_main_player
                && self
                    .tracked
                    .iter()
                    .find(|tracked| tracked.address == seen.address)
                    .and_then(|tracked| tracked.hp)
                    .zip(seen.hp)
                    .is_some_and(|(before, now)| before != now)
        });
        match (self.last_packet, packet) {
            (_, None) => {
                if !self.reported_silence {
                    self.reported_silence = true;
                    lines.push(
                        "net-damage: the Packet15 receive buffer could not be read on this \
                         build, so a damage packet that never arrived and one that arrived and \
                         was ignored cannot be told apart. See layout::packet15_receive_rva."
                            .to_owned(),
                    );
                }
            }
            (None, Some(now)) => lines.push(format!(
                "net-damage: PvP damage packets received by this process so far: {}",
                now.describe(),
            )),
            (Some(before), Some(now)) if before != now => lines.push(format!(
                "net-damage: A PvP DAMAGE PACKET ARRIVED -- {}. The local player's HP {} across \
                 this sample, and the pump subtracts the packet's damage unconditionally, so an \
                 unmoved HP means the packet named somebody else as its victim.",
                now.describe(),
                if local_hp_moved {
                    "MOVED"
                } else {
                    "did NOT move"
                },
            )),
            _ => {}
        }
        if let Some(now) = packet {
            self.last_packet = Some(now);
        }

        for seen in world {
            let previous = self
                .tracked
                .iter()
                .find(|tracked| tracked.address == seen.address);
            match (previous.and_then(|tracked| tracked.hp), seen.hp) {
                (Some(before), Some(now)) if before != now => lines.push(format!(
                    "net-damage: {} at {:#x} (chrType {}, team {}) HP {before} -> {now} ({:+})",
                    seen.category().name(),
                    seen.address,
                    seen.chr_type,
                    seen.team
                        .map_or_else(|| "unread".to_owned(), |team| team.to_string()),
                    now - before,
                )),
                (None, _) if previous.is_none() => lines.push(format!(
                    "net-damage: {} joined the watch at {:#x} (chrType {}, team {}, HP {})",
                    seen.category().name(),
                    seen.address,
                    seen.chr_type,
                    seen.team
                        .map_or_else(|| "unread".to_owned(), |team| team.to_string()),
                    seen.hp
                        .map_or_else(|| "unread".to_owned(), |hp| hp.to_string()),
                )),
                _ => {}
            }
        }

        self.tracked = world
            .iter()
            .map(|seen| Tracked {
                address: seen.address,
                hp: seen.hp,
            })
            .collect();
        lines
    }

    /// The closing line: what the watch saw over the whole possession.
    ///
    /// The receive oracle's final reading is the load-bearing half. A possession that ends with
    /// the buffer still empty, after the player was visibly being hit, says the other client's
    /// damage never reached this process -- which points at their hit resolution or at what we
    /// publish, and away from anything this crate does after a packet lands.
    pub(crate) fn summary(&self) -> String {
        format!(
            "net-damage: {} sample(s) taken over the possession, {} character(s) still tracked. \
             Last PvP damage packet received: {}",
            self.samples,
            self.tracked.len(),
            self.last_packet
                .map_or_else(|| "not sampled".to_owned(), ReceivedPacket::describe),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Category, Ledger, ROUTE_TABLE, ROUTE_TABLE_ALT, Route, Seen, route, verdict};

    fn seen(address: usize, is_main_player: bool, hp: i32) -> Seen {
        Seen {
            address,
            is_main_player,
            chr_type: 0,
            team: Some(1),
            hp: Some(hp),
        }
    }

    /// The cell the whole bug is. Both NPC categories, against a remote player, drop the hit.
    #[test]
    fn a_possessed_creature_cannot_reach_another_players_hp() {
        for attacker in [Category::LocalNpc, Category::RemoteNpc] {
            assert!(attacker.is_npc());
            let route = route(attacker, Category::RemotePlayer, false);
            assert_eq!(route, Route::ComputeOnly, "{attacker:?}");
            assert!(!route.reaches_victim());
            assert!(!route.sends_message());
            // ...and the alternate table agrees, so the `+0x11c` flag cannot rescue it.
            assert_eq!(
                route,
                super::route(attacker, Category::RemotePlayer, true),
                "{attacker:?}"
            );
        }
    }

    /// ...and the only column of that row that can is the local player's own `PlayerIns`.
    #[test]
    fn only_the_local_players_own_body_can_damage_a_remote_player() {
        let reaching: Vec<Category> = Category::ALL
            .into_iter()
            .filter(|attacker| route(*attacker, Category::RemotePlayer, false).sends_message())
            .collect();
        assert_eq!(reaching, vec![Category::MainPlayer]);
        assert_eq!(
            route(Category::MainPlayer, Category::RemotePlayer, false),
            Route::SendPvpDamage
        );
    }

    /// The mirror image: an NPC hurts the local player, and it does so entirely locally.
    ///
    /// This is why enemy damage never needed a packet, and why the possession's invincibility bit
    /// is the thing that stops enemies hurting the possessing player.
    #[test]
    fn an_npc_hurts_the_local_player_locally_and_sends_nothing() {
        for attacker in [Category::LocalNpc, Category::RemoteNpc] {
            let route = route(attacker, Category::MainPlayer, false);
            assert_eq!(route, Route::ApplyLocal, "{attacker:?}");
            assert!(route.reaches_victim());
            assert!(!route.sends_message());
        }
    }

    /// A remote player's hit on the local player is dropped here, because it arrives as a packet.
    #[test]
    fn a_remote_players_hit_on_the_local_player_is_resolved_on_their_machine() {
        assert_eq!(
            route(Category::RemotePlayer, Category::MainPlayer, false),
            Route::ComputeOnly
        );
    }

    /// Every shipped cell is a mode this module models. A seventh mode would silently become
    /// `ComputeOnly` in [`route`], so the assertion is what keeps that fallback unreachable.
    #[test]
    fn every_cell_of_both_tables_is_a_known_mode() {
        for table in [&ROUTE_TABLE, &ROUTE_TABLE_ALT] {
            for row in table {
                for mode in row {
                    assert!(Route::from_mode(*mode).is_some(), "mode {mode}");
                }
            }
        }
    }

    /// The two tables differ in exactly one cell, and it is not one this crate depends on.
    #[test]
    fn the_two_tables_differ_only_where_the_local_player_hits_a_remote_one() {
        let mut differences = Vec::new();
        for (victim, (row, alt)) in ROUTE_TABLE.iter().zip(ROUTE_TABLE_ALT.iter()).enumerate() {
            for (attacker, (mode, alt_mode)) in row.iter().zip(alt.iter()).enumerate() {
                if mode != alt_mode {
                    differences.push((victim, attacker));
                }
            }
        }
        assert_eq!(
            differences,
            vec![(
                Category::RemotePlayer as usize,
                Category::MainPlayer as usize
            )]
        );
    }

    #[test]
    fn the_verdict_names_the_mode_and_answers_both_questions() {
        let text = verdict(Category::LocalNpc, Category::RemotePlayer);
        assert!(text.contains("mode 0"), "{text}");
        assert!(text.contains("victim HP changes=NO"), "{text}");
        assert!(text.contains("damage message sent=NO"), "{text}");
        let pvp = verdict(Category::MainPlayer, Category::RemotePlayer);
        assert!(pvp.contains("mode 4"), "{pvp}");
        assert!(pvp.contains("victim HP changes=YES"), "{pvp}");
        assert!(pvp.contains("damage message sent=YES"), "{pvp}");
    }

    /// The list the log prints, derived rather than asserted -- and it has exactly one entry.
    ///
    /// A null attacker is the trap this test pins: mode 5 does apply locally to a remote player's
    /// ghost, and that is a prediction on our own copy, not damage on the character they are
    /// playing. Counting it would make the log claim two ways to hurt somebody when there is one.
    #[test]
    fn exactly_one_attacker_kind_can_reach_a_remote_player() {
        assert_eq!(
            super::attackers_that_can_reach(Category::RemotePlayer),
            vec![Category::MainPlayer]
        );
        assert!(route(Category::None, Category::RemotePlayer, false).reaches_victim());
        assert!(!route(Category::None, Category::RemotePlayer, false).sends_message());
        // ...while the local player can be reached by almost everything, which is why enemies
        // hurting the possessing player was never the networked half of this problem.
        let local = super::attackers_that_can_reach(Category::MainPlayer);
        assert!(local.contains(&Category::LocalNpc), "{local:?}");
        assert!(local.contains(&Category::RemoteNpc), "{local:?}");
        assert!(!local.contains(&Category::RemotePlayer), "{local:?}");
    }

    /// A character in `player_chr_set` that is not the main player is a remote player, which is
    /// the category the whole verdict turns on.
    #[test]
    fn everyone_in_the_player_set_but_the_main_player_is_a_remote_player() {
        assert_eq!(seen(1, true, 100).category(), Category::MainPlayer);
        assert_eq!(seen(2, false, 100).category(), Category::RemotePlayer);
    }

    #[test]
    fn the_first_sample_states_the_verdict_and_introduces_everybody() {
        let mut ledger = Ledger::new();
        assert!(ledger.due(0));
        let lines = ledger.observe(
            Category::LocalNpc,
            &[seen(0x10, true, 800), seen(0x20, false, 1200)],
            None,
            0,
        );
        assert!(
            lines.iter().any(|line| line.contains("mode 0")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("1 other player(s) in the session")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("joined the watch")),
            "{lines:?}"
        );
        // Said once: the second sample repeats neither the verdict nor the introductions.
        let quiet = ledger.observe(
            Category::LocalNpc,
            &[seen(0x10, true, 800), seen(0x20, false, 1200)],
            None,
            60,
        );
        assert!(quiet.is_empty(), "{quiet:?}");
    }

    #[test]
    fn a_moving_hp_bar_is_reported_as_a_delta() {
        let mut ledger = Ledger::new();
        ledger.observe(Category::LocalNpc, &[seen(0x20, false, 1200)], None, 0);
        let lines = ledger.observe(Category::LocalNpc, &[seen(0x20, false, 900)], None, 60);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("HP 1200 -> 900 (-300)"), "{lines:?}");
    }

    #[test]
    fn the_sampler_waits_its_cadence_between_reads() {
        let mut ledger = Ledger::new();
        ledger.observe(Category::LocalNpc, &[], None, 100);
        assert!(!ledger.due(101));
        assert!(!ledger.due(129));
        assert!(ledger.due(130));
    }

    /// An unread receive buffer is reported as a gap, once, rather than silently read as "no
    /// packets" -- the two answers point at opposite halves of the problem.
    #[test]
    fn an_unreadable_receive_buffer_is_reported_as_a_gap_and_only_once() {
        let mut ledger = Ledger::new();
        let first = ledger.observe(Category::LocalNpc, &[], None, 0);
        assert!(
            first
                .iter()
                .any(|line| line.contains("could not be read on this build")),
            "{first:?}"
        );
        let second = ledger.observe(Category::LocalNpc, &[], None, 60);
        assert!(
            !second
                .iter()
                .any(|line| line.contains("could not be read on this build")),
            "{second:?}"
        );
    }

    /// An empty buffer is a FINDING: no PvP damage packet has reached this process.
    #[test]
    fn an_empty_receive_buffer_says_nothing_has_arrived() {
        let mut ledger = Ledger::new();
        let lines = ledger.observe(
            Category::LocalNpc,
            &[],
            Some(super::ReceivedPacket::default()),
            0,
        );
        assert!(
            lines.iter().any(|line| line.contains("NOTHING")),
            "{lines:?}"
        );
    }

    /// An arrival is a change, and it is paired with whether our HP moved -- which is the whole
    /// discriminator: the pump subtracts unconditionally, so an arrival with a still HP bar means
    /// the packet was not addressed to us.
    #[test]
    fn an_arriving_packet_is_reported_once_with_whether_our_hp_moved() {
        let mut ledger = Ledger::new();
        ledger.observe(
            Category::LocalNpc,
            &[seen(0x10, true, 800)],
            Some(super::ReceivedPacket::default()),
            0,
        );
        let arrival = super::ReceivedPacket {
            dealer: 0x1111_2222_3333_4444,
            victim: 0x5555_6666_7777_8888,
            damage: 250,
            stamina: 30,
        };
        let hit = ledger.observe(
            Category::LocalNpc,
            &[seen(0x10, true, 550)],
            Some(arrival),
            60,
        );
        let arrived: Vec<&String> = hit
            .iter()
            .filter(|line| line.contains("A PvP DAMAGE PACKET ARRIVED"))
            .collect();
        assert_eq!(arrived.len(), 1, "{hit:?}");
        assert!(arrived[0].contains("damage=250"), "{arrived:?}");
        assert!(arrived[0].contains("HP MOVED"), "{arrived:?}");
        // The same reading again is the same packet, not a second arrival.
        let quiet = ledger.observe(
            Category::LocalNpc,
            &[seen(0x10, true, 550)],
            Some(arrival),
            120,
        );
        assert!(
            !quiet
                .iter()
                .any(|line| line.contains("A PvP DAMAGE PACKET ARRIVED")),
            "{quiet:?}"
        );
    }

    /// The damage is an `i16` because the pump reads it with `MOVSX .., word ptr`. A negative
    /// value is a heal in the game's own arithmetic and must survive the round trip.
    #[test]
    fn the_packet_damage_is_signed_sixteen_bit() {
        let packet = super::ReceivedPacket {
            damage: -500,
            ..super::ReceivedPacket::default()
        };
        assert!(!packet.is_empty());
        assert!(packet.describe().contains("damage=-500"), "{packet:?}");
    }

    #[test]
    fn the_summary_counts_what_was_watched() {
        let mut ledger = Ledger::new();
        ledger.observe(Category::LocalNpc, &[seen(0x20, false, 1200)], None, 0);
        ledger.observe(Category::LocalNpc, &[seen(0x20, false, 1200)], None, 60);
        let summary = ledger.summary();
        assert!(summary.contains("2 sample(s)"), "{summary}");
        assert!(summary.contains("1 character(s)"), "{summary}");
    }
}
