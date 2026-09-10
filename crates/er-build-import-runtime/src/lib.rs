//! Import an Elden Ring build from an `er-build-planner` share link, into the character that is
//! already in the world.
//!
//! # Shape
//!
//! Two threads, split by what they are allowed to touch:
//!
//! * a **fetch worker** does the blocking HTTPS get and parses the payload. It touches no game
//!   state at all, which is why it may block.
//! * a **game-thread step**, [`tick`], does everything else. Building the catalog reads the param
//!   tables and the message repository; granting mutates the inventory and the `CSGaitemImp`
//!   singleton; stats, spells and equipment all call native functions. All of that belongs on the
//!   thread the game runs its own tasks on.
//!
//! Neither sleeps. The worker blocks in WinHTTP; [`tick`] re-checks its preconditions once a frame
//! and does nothing until they hold, which is the natural shape here and also what
//! `scripts/check-no-timeouts.py` requires.
//!
//! # Why this is a library and not just the DLL
//!
//! There are two callers with the same needs and different triggers:
//!
//! * `er-build-import` -- a standalone ME3 shell that imports the build named in
//!   `er-quickload.toml` once, as soon as a character is in the world.
//! * `er-quickload` -- the product DLL, whose System>Quit **Load Build from URL** row imports on
//!   demand, as many times as the player asks.
//!
//! The second is why [`request`] exists at all: the original code ran exactly once per process,
//! latched by an `AtomicBool`. That latch is now a phase machine ([`Phase`]) whose terminal states
//! -- [`Phase::Done`] and [`Phase::Failed`] -- are ones [`request`] accepts, so a second import is
//! an ordinary state transition rather than a special case. Nothing resets the machine on a timer:
//! it stays on its last outcome until someone asks for another build.

#![cfg(windows)]

pub mod catalog;
pub mod character;
pub mod chr_name;
pub mod equip_native;
pub mod evict;
pub mod export;
pub mod export_doc;
pub mod gaitem;
pub mod gem_mount;
pub mod grant;
pub mod native;
pub mod read_character;
pub mod reorder;
pub mod storage;
pub mod upload;

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use er_build_import_core::equip::{
    CHR_ASM_SLOT_QUICK_BASE, Capacity, EquipLedger, PositionKind, PositionResult, equip_plan,
};
use er_build_import_core::{API_HOST, BuildDoc, build_path, class, model, plan::plan, stats};

use windows::Win32::System::LibraryLoader::GetModuleHandleW;

/// Config key naming the planner build to import, re-exported so callers naming it in a log line
/// or a menu message cannot drift from the parser.
pub use er_build_import_core::BUILD_URL_KEY;

/// The config file, beside the game executable.
pub const CONFIG_FILE_NAME: &str = "er-quickload.toml";

/// Identifies this client to the API owner, who runs the service for free.
pub(crate) const USER_AGENT: &str = "er-mods-rs build-import (+github.com/Banon-Labs)";

/// Log file name, written next to the game executable.
const LOG_NAME: &str = "er-build-import.log";

// ------------------------------------------------------------------ state

/// Where the importer is. Encoded as a `usize` so the whole machine is one lock-free atomic that a
/// menu row, a fetch worker and the game task can all read without ordering games.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
pub enum Phase {
    /// Nothing in flight. The only phase [`request`] accepts.
    Idle = 0,
    /// A worker is blocked in WinHTTP.
    Fetching = 1,
    /// A build is parsed and waiting for the game thread to be able to apply it.
    Ready = 2,
    /// [`tick`] is inside the import.
    Importing = 3,
    /// The last import finished. The report was returned to whoever called [`tick`]; this phase is
    /// only "not busy, and the last thing that happened was a success".
    Done = 4,
    /// The last request failed before anything was applied; [`take_error`] says why, once.
    Failed = 5,
}

impl Phase {
    fn from_code(code: usize) -> Phase {
        match code {
            1 => Phase::Fetching,
            2 => Phase::Ready,
            3 => Phase::Importing,
            4 => Phase::Done,
            5 => Phase::Failed,
            _ => Phase::Idle,
        }
    }
}

static PHASE: AtomicUsize = AtomicUsize::new(Phase::Idle as usize);

/// The parsed build, handed from the fetch worker to the game task.
static DOC: Mutex<Option<BuildDoc>> = Mutex::new(None);

/// Why the last request failed. Kept separate from the report so a failure cannot be read as a
/// report with zero counts -- "granted 0/0" and "the fetch 404'd" are not the same event.
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// What one completed import actually did, measured by reading game memory back -- never by a call
/// having returned. Every field here is a read-back count.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// The build's own name, as the planner stored it.
    pub build_name: String,
    /// Items confirmed present in the inventory afterwards, out of those attempted.
    pub granted: (usize, usize),
    /// Equipment slots holding the requested param id afterwards, out of those requested.
    pub equipped: (usize, usize),
    /// Spells confirmed in a memory slot afterwards, out of those wanted.
    pub spells: (usize, usize),
    /// Physick tears confirmed in the flask afterwards, out of those wanted.
    pub physick: (usize, usize),
    /// Attributes still disagreeing with the build after the read-back. Zero means the character
    /// matches.
    pub attributes_wrong: usize,
    /// Character level after the import, read back from the player's game data.
    pub level: i32,
    /// The name the character now has, when the import is what gave it one -- read back from
    /// `PlayerGameData`, not from the build document. `None` covers every other case: the build
    /// named nothing, the character already had the name, or the rename could not be performed.
    pub name: Option<String>,
    /// Names the catalog could not resolve to an item id.
    pub unresolved: usize,
    /// Items re-acquired into the build's order, out of those the character already held.
    ///
    /// Not part of [`Report::summary`]: a player reads that line to find out whether they got
    /// their build, and the inventory sort order is not what they mean by that. It is in the
    /// report so the log line and the telemetry have one source.
    pub reordered: (usize, usize),
    /// Equipment positions the build leaves empty that are empty afterwards, out of those it
    /// leaves empty.
    pub vacated: (usize, usize),
    /// Consumables destroyed to free a pot group the storage box would not take.
    pub discarded: u32,
    /// Armaments, armour and talismans the build does not name that went to the storage box.
    pub evicted: u32,
}

impl Report {
    /// Whether the import attempted nothing at all -- not whether it succeeded.
    ///
    /// The distinction is the whole point. `0/5 items` is a failure the counters already show.
    /// `0/0 items, 0/0 gear, 0/0 spells` is not a poor result, it is the absence of a result: the
    /// denominators are what the importer set out to do, and all-zero means it set out to do
    /// nothing. That is what a build whose natives are all refused looks like, and it reads
    /// exactly like a clean run of an empty build unless something says otherwise.
    ///
    /// Measured 2026-08-30 on a real 1.17 session: a 13,581-byte payload carrying 75 armaments,
    /// 70 talismans, 22 armour pieces and 8 spells produced this report, was logged as
    /// `import complete`, and the user was told nothing.
    pub fn attempted_nothing(&self) -> bool {
        self.granted.1 == 0 && self.equipped.1 == 0 && self.spells.1 == 0 && self.physick.1 == 0
    }

    /// One line for a menu help field or a log: what a player wants to know is whether it worked.
    pub fn summary(&self) -> String {
        format!(
            "{}/{} items, {}/{} gear, {}/{} spells, RL{}{}{}",
            self.granted.0,
            self.granted.1,
            self.equipped.0,
            self.equipped.1,
            self.spells.0,
            self.spells.1,
            self.level,
            if self.attributes_wrong == 0 {
                String::new()
            } else {
                format!(", {} attributes WRONG", self.attributes_wrong)
            },
            // Only when the import changed it. A character that already had the build's name is
            // not a result, and printing it every time would read as a rename that did not happen.
            match self.name.as_deref() {
                Some(name) => format!(", named {name:?}"),
                None => String::new(),
            }
        )
    }
}

/// Current phase.
pub fn phase() -> Phase {
    Phase::from_code(PHASE.load(Ordering::SeqCst))
}

/// Take the reason the last request failed, clearing it.
///
/// Taking rather than peeking, because the failure is ASYNCHRONOUS -- the fetch worker fails long
/// after [`request`] returned `Ok` -- so the only way a caller learns about it is by polling. A
/// peek would re-report the same failure on every frame; taking it reports each failure exactly
/// once, which is what a log wants.
pub fn take_error() -> Option<String> {
    LAST_ERROR.lock().ok().and_then(|mut guard| guard.take())
}

fn set_error(reason: String) {
    log_line(&format!("[build-import] FAILED: {reason}"));
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = Some(reason);
    }
    PHASE.store(Phase::Failed as usize, Ordering::SeqCst);
}

// ------------------------------------------------------------------ config

/// Read `build_url` out of the game-directory `er-quickload.toml`.
///
/// The file lookup lives here because it needs the game directory; the PARSING lives in
/// `er-build-import-core`, which `cargo test` can reach.
pub fn configured_build_url() -> Option<String> {
    let path = er_game_base::log::game_directory_path()?.join(CONFIG_FILE_NAME);
    let contents = std::fs::read_to_string(path).ok()?;
    er_build_import_core::build_url_from_config(&contents).map(str::to_owned)
}

/// Whether the game-directory `er-quickload.toml` asks the standalone shell to export one build link
/// at character load. Never consulted by the product DLL -- see
/// [`er_build_import_core::EXPORT_ON_LOAD_KEY`].
pub fn configured_export_on_load() -> bool {
    let Some(path) = er_game_base::log::game_directory_path() else {
        return false;
    };
    let Ok(contents) = std::fs::read_to_string(path.join(CONFIG_FILE_NAME)) else {
        return false;
    };
    er_build_import_core::config_flag(&contents, er_build_import_core::EXPORT_ON_LOAD_KEY)
}

// ------------------------------------------------------------------ request

/// Why [`request`] refused. A refusal never changes [`phase`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RequestError {
    /// A fetch or import is already in flight.
    Busy,
    /// The URL carries no `?b=<id>`. The self-contained `?i=` form needs no network at all and is
    /// not supported here.
    NoShareId,
}

impl core::fmt::Display for RequestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RequestError::Busy => write!(f, "a build import is already in flight"),
            RequestError::NoShareId => {
                write!(
                    f,
                    "that link carries no ?b=<id> (the ?i= form is not supported)"
                )
            }
        }
    }
}

/// Start importing the build named by `url`. Returns as soon as the worker is spawned; the game
/// thread must keep calling [`tick`] for the import to actually happen.
///
/// Safe to call from any thread, including a menu action handler: nothing here touches game state.
pub fn request(url: &str) -> Result<(), RequestError> {
    let Some(share_id) = er_build_import_core::share_id_from_url(url) else {
        return Err(RequestError::NoShareId);
    };
    // Claim Idle/Done/Failed -> Fetching atomically. Losing this race means another caller (or the
    // standalone shell's boot import) already owns the machine.
    let claimed = [Phase::Idle, Phase::Done, Phase::Failed]
        .into_iter()
        .any(|from| {
            PHASE
                .compare_exchange(
                    from as usize,
                    Phase::Fetching as usize,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
        });
    if !claimed {
        return Err(RequestError::Busy);
    }
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = None;
    }
    let share_id = share_id.to_owned();
    std::thread::spawn(move || {
        if std::panic::catch_unwind(|| fetch_inner(&share_id)).is_err() {
            set_error("fetch worker PANICKED".to_owned());
        }
    });
    Ok(())
}

/// Start importing the build configured in `er-quickload.toml`, if there is one. Returns `Ok(false)`
/// when the key is absent, which is the normal state for a player who has not set one.
pub fn request_configured() -> Result<bool, RequestError> {
    let Some(url) = configured_build_url() else {
        log_line(&format!(
            "[build-import] no `{BUILD_URL_KEY}` in {CONFIG_FILE_NAME} -- nothing to import"
        ));
        return Ok(false);
    };
    request(&url).map(|()| true)
}

/// The fetch proper. Runs on the worker thread; touches no game state.
fn fetch_inner(share_id: &str) {
    log_line(&format!("[build-import] fetch start, build {share_id}"));
    log_line(&format!(
        "[build-import] GET https://{API_HOST}{}",
        build_path(share_id)
    ));

    let body = match er_game_base::http::get(API_HOST, &build_path(share_id), USER_AGENT) {
        Ok(body) => body,
        Err(err) => return set_error(format!("fetch: {err}")),
    };
    log_line(&format!("[build-import] fetch ok, {} bytes", body.len()));

    let mut doc = match model::parse(&body) {
        Ok(doc) => doc,
        Err(err) => return set_error(format!("parse: {err}")),
    };

    let armour: usize = doc.protectors.values().map(|part| part.slots.len()).sum();
    log_line(&format!(
        "[build-import] parsed name={:?} class={:?} rl={:?} weaponUpgrade={}",
        doc.name,
        doc.character_class,
        doc.stats.get("rl"),
        doc.weapon_upgrade
    ));
    log_line(&format!(
        "[build-import] slots armaments={} spells={} talismans={} armour={} tools={}",
        doc.inventory.slots.len(),
        doc.spells.slots.len(),
        doc.talismans.slots.len(),
        armour,
        doc.items.tools.slots.len()
    ));

    // The eight attributes are the build, and the level is derived from them rather than trusted.
    // The whole derivation -- the class floor under the attributes, the sum, the `- 79`, and the
    // overwrite of the payload's `rl` -- lives in `er_build_import_core::stats`, which is
    // host-testable; this crate is not, so nothing provable by `cargo test` belongs in it. Read
    // that module for why each of those steps exists. What is left here is the reporting.
    let normalised = match stats::normalise(&mut doc) {
        Ok(normalised) => normalised,
        Err(err) => return set_error(err.to_string()),
    };

    // The floor (user-reported 2026-09-06). A starting class's base attributes are its minimum:
    // no ordinary play session produces a Vagabond with strength below 14. A payload carrying one
    // summed two points short, derived level 148 instead of the 150 it claimed, and stamped a
    // character whose stat block contradicts its own class. `ApplyMainPlayerStats` does no
    // clamping of its own (see `er_build_import_core::stats` for the decompiled evidence), so
    // nothing downstream would have caught it. Naming what was raised matters more than most
    // lines here, because the alternative is a character silently two levels off the build the
    // player asked for.
    match &normalised.floor {
        stats::Floor::Class { name, archetype } if normalised.raised.is_empty() => {
            log_line(&format!(
                "[build-import] floor: {name} (archetype {archetype}, CharaInitParam {}) -- every \
                 attribute is at or above its class base",
                class::chara_init_param_row(*archetype)
            ));
        }
        stats::Floor::Class { name, archetype } => {
            log_line(&format!(
                "[build-import] floor: {name} (archetype {archetype}, CharaInitParam {}) -- {} \
                 attribute(s) BELOW the class base, raised to it. The build is not legal as \
                 written and the level it claims already counts these points.",
                class::chara_init_param_row(*archetype),
                normalised.raised.len()
            ));
            for raised in &normalised.raised {
                log_line(&format!(
                    "[build-import]   RAISED {}: {} -> {} (class base)",
                    raised.key, raised.was, raised.now
                ));
            }
        }
        stats::Floor::Unnamed => log_line(
            "[build-import] floor: the build names no class, so there is no base to raise to. \
             The attributes stand as the payload gave them -- no default class is invented.",
        ),
        stats::Floor::Unrecognised(name) => log_line(&format!(
            "[build-import] floor: the build names {name:?}, which is not a class this build \
             knows -- most likely the game grew one. No base to raise to, so the attributes \
             stand as the payload gave them."
        )),
    }

    match normalised.claimed {
        Some(claimed) if claimed != normalised.level => log_line(&format!(
            "[build-import] level: attributes sum to {}, so RL {}. The payload CLAIMS RL \
             {claimed}, which disagrees with its own stat block by {}. Importing the \
             attributes, which are what actually get applied.",
            normalised.total,
            normalised.level,
            (claimed - normalised.level).abs()
        )),
        _ => log_line(&format!(
            "[build-import] level: {} - {} = {}, matches the payload",
            normalised.total,
            stats::CLASS_INVARIANT,
            normalised.level
        )),
    }

    match DOC.lock() {
        Ok(mut slot) => {
            *slot = Some(doc);
            PHASE.store(Phase::Ready as usize, Ordering::SeqCst);
            log_line("[build-import] build handed to the game task");
        }
        Err(_) => set_error("the build slot is poisoned".to_owned()),
    }
}

// ------------------------------------------------------------------ game thread

/// One frame of the importer. Does nothing until a build is [`Phase::Ready`] and the game can take
/// it, then runs the whole import once and returns the report.
///
/// # Safety
///
/// Game task thread only. Every step below is individually precondition-checked, but the thread
/// itself is not something this function can verify.
pub unsafe fn tick() -> Option<Report> {
    if phase() != Phase::Ready {
        return None;
    }
    if !catalog::params_ready() {
        return None;
    }
    // Safety: game thread; the helper is fault-checked and returns false at the title screen.
    if !unsafe { grant::player_present() } {
        return None;
    }
    let doc = DOC.lock().ok().and_then(|mut slot| slot.take())?;
    // Claim the run before doing it: a panic must not leave the task retrying every frame.
    PHASE.store(Phase::Importing as usize, Ordering::SeqCst);

    // Safety: the caller's contract (game task thread) carries through.
    let report = unsafe { import_now(&doc) };
    match report {
        // Nothing was attempted. Reported as a failure, with the names, rather than as a
        // completed import of nothing. See `Report::attempted_nothing`.
        Some(report) if report.attempted_nothing() => {
            let inert = native::inert();
            let named = if inert.is_empty() {
                "no native was recorded inert, so the cause is upstream of the address resolver \
                 -- an empty catalog or an unreadable payload"
                    .to_owned()
            } else {
                format!(
                    "{} game function(s) have no verified mapping for this build and every call \
                     to them was refused: {}",
                    inert.len(),
                    inert.join(", ")
                )
            };
            set_error(format!(
                "the import applied NOTHING -- {}. The build was decoded and planned; it could \
                 not be written. {}",
                report.summary(),
                named
            ));
            Some(report)
        }
        Some(report) => {
            PHASE.store(Phase::Done as usize, Ordering::SeqCst);
            log_line(&format!(
                "[build-import] import complete: {}",
                report.summary()
            ));
            Some(report)
        }
        None => {
            set_error(
                "the message repository was unavailable, so no catalog could be built".to_owned(),
            );
            None
        }
    }
}

/// The import proper: catalog, plan, grant, equip, physick, great rune, class, stats, spells.
///
/// # Safety
///
/// Game task thread, params streamed, character in the world -- all three checked by [`tick`].
unsafe fn import_now(doc: &BuildDoc) -> Option<Report> {
    let module_base = module_base();
    let msg = catalog::msg_repository()?;

    // Safety: params_ready() proved the tables are streamed and `msg` came from the singleton.
    let (catalog, stats) = unsafe { catalog::build_from_game(msg, module_base) };
    log_line(&format!(
        "[build-import] catalog: {} named, {} unnamed, {} goods rows are spells, \
         {} ashes have no gem that draws an icon (their badge renders the `ICON` placeholder)",
        stats.named, stats.unnamed, stats.spell_rows, stats.iconless_ashes
    ));
    // Both numbers are INERTNESS detectors. A zero on either is the goods param table failing to
    // read, and each failure is invisible in its own outcome: no pot group could be freed, and
    // every consumable in the build silently becomes one copy. Neither looks like a fault
    // afterwards, so they are counted here rather than inferred later.
    log_line(&format!(
        "[build-import] catalog: {} goods rows are pot-capped, {} declare a hold limit (maxNum) \
         -- a consumable is granted that many, capped at 99, and reconciled rather than added",
        stats.pot_capped_rows, stats.max_held_rows
    ));
    // Ammunition is a SUBTRACTION from the armament catalog, so this number is a denominator for
    // both kinds and a zero is not merely "no arrows". A zero means the weapon table did not
    // classify, `Kind::Ammo` is empty and every arrow is still filed under `Kind::Weapon` -- so a
    // build's ammo comes back unresolved while the same name sits one catalog over. The installed
    // 1.17 table has 73 such rows.
    log_line(&format!(
        "[build-import] catalog: {} EquipParamWeapon rows are ammunition (weaponCategory 13/14) \
         -- granted at their own EquipParamWeapon.maxArrowQuantity, not at maxNum",
        stats.ammo_rows
    ));
    // The offset alarm. Two independent fields classify the same rows; they agree on every build
    // measured. A non-zero here says one of the two is being read out of the wrong place, which
    // is the one defect in the catalog that produces no fault and no refusal.
    if stats.ammo_classification_disagreements > 0 {
        log_line(&format!(
            "[build-import] catalog: WRONG-OFFSET ALARM -- {} weapon row(s) are ammunition by \
             weaponCategory (+0xE6) but not by wepType (+0x1A6), or the reverse. Those two \
             offsets describe the same 73 rows on every build measured, so a disagreement means \
             one of them is no longer the field it is named after; treat every ammunition \
             quantity below as unverified",
            stats.ammo_classification_disagreements
        ));
    }

    // Names that resolve to more than one row. Reported at build time rather than discovered
    // later as a duplicated item: an id that cannot be told apart from its siblings by name is
    // exactly the shape that granted a second Flask of Wondrous Physick.
    let collisions = catalog.collisions();
    if collisions.is_empty() {
        log_line("[build-import] catalog: every name resolves to exactly one id");
    } else {
        log_line(&format!(
            "[build-import] catalog: {} name(s) resolve to MORE THAN ONE id -- the grant check \
             counts all of them, so holding any one counts as holding the item",
            collisions.len()
        ));
        // No cap. The first cut printed 24 of 101, sorted by (kind, name) -- which put a wall of
        // `[error]`-named placeholder rows first and cut off everything real, including the one
        // item under investigation. A truncated report is worse than none: it was read as
        // "this item does not collide" and nearly retracted a correct diagnosis.
        for (kind, name, ids) in &collisions {
            let rendered: Vec<String> = ids.iter().map(|id| format!("0x{id:08X}")).collect();
            log_line(&format!(
                "[build-import]   COLLIDING NAME [{}] {:?} -> {}",
                kind.label(),
                name,
                rendered.join(", ")
            ));
        }
    }

    let planned = plan(doc, &catalog);
    let equips = equip_plan(doc, &catalog, Capacity::default());
    log_line(&format!(
        "[build-import] planned: {} grants, {} unresolved, {} equip positions, {} spells to \
         memorise, {} rejected",
        planned.grants.len(),
        planned.unresolved.len(),
        equips.occupied(),
        equips.spells.len(),
        equips.rejected.len()
    ));
    for position in equips.positions() {
        log_line(&format!("[build-import]   PLAN {}", position.describe()));
    }
    // The commonest confusion this importer produces is "why is the thing in my inventory not on
    // my character": the answer is usually that the planner document never marked it equipped.
    // Say so up front, with the count, so nobody has to read the payload to find out.
    let carried = doc
        .inventory
        .slots
        .iter()
        .filter(|slot| slot.equip_index.is_none())
        .count();
    if carried > 0 {
        log_line(&format!(
            "[build-import] the build marks {} of {} armaments as equipped; the other {carried} \
             are CARRIED ONLY and will be granted, not worn",
            doc.inventory.slots.len() - carried,
            doc.inventory.slots.len()
        ));
    }
    for missing in planned.unresolved.iter().take(12) {
        log_line(&format!(
            "[build-import]   UNRESOLVED {} {:?}",
            missing.kind.label(),
            missing.name
        ));
    }
    // A build can name two items for one position -- the planner leaves the old row flagged when a
    // slot is re-assigned. The winner is chosen the way the planner renders it, but a dropped
    // armament is not allowed to be invisible in the log, which is where this is diagnosed from.
    for contest in equips.contested.iter().take(12) {
        log_line(&format!(
            "[build-import]   CONTESTED {} -> {:?}, dropping {:?}",
            contest.position, contest.winner, contest.losers
        ));
    }

    let mut report = Report {
        build_name: doc.name.clone(),
        unresolved: planned.unresolved.len(),
        ..Report::default()
    };

    // Safety: game thread, character loaded (checked by the caller), verified RVAs.
    let outcome = unsafe { grant::grant_all(module_base, &planned.grants) };
    report.granted = (outcome.confirmed, outcome.attempted);
    report.discarded = outcome.discarded_to_free_pots;
    log_line(&format!(
        "[build-import] GRANTED: {}/{} confirmed AT THE REQUESTED QUANTITY ({} short, {} missing \
         entirely, {} already held and left alone)",
        outcome.confirmed,
        outcome.attempted,
        outcome.short.len(),
        outcome.missing.len(),
        outcome.already_held
    ));
    // The storage box, and why a number here is not a complaint. Items pulled back were the
    // player's own copies, moved instead of duplicated; items deposited were pot-group members
    // the build did not ask for, moved to raise the group's ceiling. Both are reversible by the
    // player at any grace. A run where the box was unreachable says so, because zero-and-unable
    // and zero-and-nothing-to-do are different facts.
    if !outcome.storage_available {
        log_line(
            "[build-import] STORAGE: unreachable this run -- an item already in the box was \
             granted as a new copy instead of moved back, and no pot group could be freed",
        );
    } else if outcome.pulled_from_storage > 0 || outcome.deposited_to_storage > 0 {
        log_line(&format!(
            "[build-import] STORAGE: {} item(s) moved back out of the box instead of duplicated, \
             {} deposited into it to free pot-group capacity",
            outcome.pulled_from_storage, outcome.deposited_to_storage
        ));
    }
    // On its own line, never folded into the storage counts, and printed whenever it is non-zero
    // even though the per-item lines already said it. A deposit and a discard both read as "the
    // item left your pockets" and only one of them can be walked back, so the irreversible one
    // gets its own headline or it hides inside the harmless one.
    if outcome.discarded_to_free_pots > 0 {
        log_line(&format!(
            "[build-import] DISCARDED: {} consumable(s) were DESTROYED, not stored. The storage \
             box refused them -- it was already at their maxRepositoryNum -- and their pot group \
             was what stopped the build's own pots from being carried. Nothing else this import \
             did is irreversible; this is",
            outcome.discarded_to_free_pots
        ));
    }
    // Short is not missing, and it is the one the old report could not say. Printed before the
    // missing list because it is the failure a reader will otherwise not know happened.
    for short in outcome.short.iter().take(12) {
        log_line(&format!(
            "[build-import]   SHORT {:?} item 0x{:08X}: {} of {} requested{}",
            short.label,
            short.item_id,
            short.held,
            short.requested,
            // A pot-capped item names its cause, because that shortfall is not a defect and
            // will recur on every import until the player carries more vessels.
            match short.pot_group {
                Some(group) => format!(
                    " -- pot group {group} is full; the ceiling is the number of Cracked Pots \
                     the character carries, not something the import can raise"
                ),
                None => String::new(),
            }
        ));
    }
    for id in outcome.missing.iter().take(12) {
        log_line(&format!("[build-import]   MISSING item id 0x{id:08X}"));
    }

    // ARMAMENTS, read back off the instance that was just MINTED.
    //
    // Quantity cannot see an ash or an upgrade level: both live on the gaitem, not in the item
    // id, so "71/71 present" was true and meaningless while every weapon came out bare. Each
    // line below is measured -- the arts id is what `GetSwordArtsParamForWeapon` says that
    // instance holds, not what the plan asked for.
    let wanted_ashes = planned
        .grants
        .iter()
        .filter(|grant| grant.weapon_skill != er_build_import_core::plan::NO_SKILL)
        .count();
    let ashes_mounted = outcome
        .armaments
        .iter()
        .filter(|arm| arm.wanted_gem.is_some() && arm.has_ash())
        .count();
    log_line(&format!(
        "[build-import] ARMAMENTS (read back off the minted gaitem): {ashes_mounted}/{wanted_ashes}          ashes mounted, {} armaments granted",
        outcome.armaments.len()
    ));
    for arm in &outcome.armaments {
        log_line(&format!(
            "[build-import]   ARMAMENT {:?} item 0x{:08X} invIdx {} gem {} -> arts {} | +{} -> +{}",
            arm.label,
            arm.item_id,
            arm.inventory_index,
            arm.wanted_gem
                .map_or_else(|| "none".to_owned(), |row| row.to_string()),
            arm.arts_id
                .map_or_else(|| "NONE".to_owned(), |id| id.to_string()),
            arm.wanted_level,
            arm.level,
        ));
    }
    for arm in outcome.armaments_missing_their_ash() {
        log_line(&format!(
            "[build-import]   ASH NOT MOUNTED on {:?}: asked for gem {:?}, the instance reports no              sword-arts row",
            arm.label, arm.wanted_gem
        ));
    }

    // Between the grant and the equip, the two passes that make a RE-import mean what it says.
    //
    // Both answer the same complaint, from opposite ends: a build imported onto a character that
    // already owns most of it used to change almost nothing the player could see. The grant
    // correctly left held items alone, the equip correctly wrote only the positions the build
    // fills, and the result was the previous build's gear still worn beside the new build's, in
    // the previous build's inventory order.
    //
    // Both run before the equip, and `apply_build_order` runs first. It has to take items off to
    // move them -- a worn entry is named by `EquipGameData.equipmentItemIdxList` and must not be
    // removed from under that -- and it does not put them back, so the equip is what dresses the
    // character afterwards. Running the vacate second costs nothing and means the positions the
    // build wants bare are cleared after everything that could have disturbed them.
    if let Some(egd) = unsafe { grant::equip_game_data() } {
        // Safety: game thread, character in the world (the caller proved it), grants applied.
        let reordered = unsafe { reorder::apply_build_order(module_base, egd, &planned.grants) };
        log_line(&format!("[build-import] {}", reordered.summary()));
        for (label, why) in reordered.declined.iter().take(12) {
            log_line(&format!("[build-import]   NOT REORDERED {label:?}: {why}"));
        }
        // An item that went into the box and did not come back is the one failure in this pass
        // that costs the player something, so it is never folded into a count.
        for (label, deposited, retrieved) in &reordered.stranded {
            log_line(&format!(
                "[build-import]   STRANDED {label:?}: {deposited} went into the storage box and                  {retrieved} came back. The rest is in the box and can be collected at any grace"
            ));
        }
        report.reordered = (reordered.restamped, reordered.attempted);

        let vacancies = equips.vacancies();
        // Safety: as above; the vacate pass reads every position back rather than counting calls.
        let vacated = unsafe { equip_native::vacate_all(module_base, egd, &vacancies) };
        log_line(&format!("[build-import] {}", vacated.summary()));
        for (kind, slot, holding) in vacated.still_occupied.iter().take(12) {
            log_line(&format!(
                "[build-import]   STILL WORN {} (slot {slot}): the build leaves this position                  empty and it holds {holding}",
                kind.label()
            ));
        }
        for (kind, slot) in vacated.unproven.iter().take(12) {
            log_line(&format!(
                "[build-import]   VACANCY UNPROVEN {} (slot {slot}): nothing on this build can                  read the position back, so whether it is empty is unknown",
                kind.label()
            ));
        }
        report.vacated = (vacated.cleared + vacated.already_empty, vacated.attempted);

        // Last of the three, and before the equip rather than after it. Every deposit shifts the
        // inventory indices the equip resolves, so the equip has to be the final word; and gear
        // the previous build wore in a position the new build also names is still on the
        // character here, because the vacate above clears only the positions the build wants
        // bare. The pass takes those off itself before depositing them.
        //
        // An allowance, not a list of ids. The build asks for a count of each thing -- one
        // Serpent Crest Shield, five Crimson Seed Talismans -- and a set of ids cannot say five,
        // so every copy of a named id survived and the sweep left 168 entries alone for a build
        // with 24 gear positions. `evict::Keep` spends one slot per copy and sweeps the rest, and
        // it takes `outcome.armaments` as well as the plan because the copies this import minted
        // are named by gaitem handle: an ash lives on the instance, so the item id cannot tell
        // the new shield from the old one it replaces.
        let mut keep = evict::Keep::new(&planned.grants, &outcome.armaments);
        // Safety: game thread, character in the world, and the vacate above has run.
        let evicted = unsafe { evict::unlisted_gear(module_base, egd, &mut keep) };
        log_line(&format!("[build-import] {}", evicted.summary()));
        // Named one by one and never folded into the summary's count, because this is the second
        // irreversible thing the importer does and the player is owed the list.
        for (item, quantity, ash) in &evicted.discarded {
            log_line(&format!(
                "[build-import]   DESTROYED {item} x{quantity}: the storage box would not take it \
                 and already holds two or more of the same item. {}",
                if *ash {
                    "Its Ash of War was taken off first and is back in the inventory"
                } else {
                    "It had no Ash of War to recover"
                }
            ));
        }
        for (item, why) in &evicted.refused {
            log_line(&format!("[build-import]   NOT EVICTED {item}: {why}"));
        }
        // The other half of "why is this still on my character": the build asked for it. Without
        // this line a kept item and a stuck item look identical from the outside.
        for item in &evicted.kept_names {
            log_line(&format!(
                "[build-import]   KEPT {item}: the build names this item, so it stays"
            ));
        }
        report.evicted = evicted.deposited_items;
    }

    // What each armament slot should be holding, computed before the equip rather than after it,
    // because it is now needed twice: it tells the equip which minted copy belongs in which hand
    // (an ash lives on the instance, so the item id alone cannot say), and it is what the
    // post-import read-back adjudicates the worn armament against. One table, both jobs -- the
    // alternative is a second opinion about what the build asked for.
    let wants = er_build_import_core::plan::equipped_armament_skills(doc, &catalog);

    // Equip only what was actually granted: equipping an item the inventory does not hold cannot
    // work, and the outcome distinguishes those from real equip failures.
    if let Some(egd) = unsafe { grant::equip_game_data() } {
        // Open the ledger over the plan, before anything is written. Every score below is
        // measured against this, so a family of positions the pass never reaches cannot leave
        // the denominator on its way out -- which is how a run that equipped ten of twelve
        // planned positions printed "10/10 verified".
        let mut ledger = EquipLedger::new(&equips);

        // The gaitem handles the grant minted, joined to the slots that should wear them.
        let mut instances = equip_native::WornInstances::new(&outcome.armaments, &wants);
        let mintable = instances.available();

        // Safety: game thread, character loaded, items granted above.
        let worn =
            unsafe { equip_native::equip_all(module_base, egd, &mut ledger, &mut instances) };

        // How each position was resolved, not just whether it was. An index found from the minted
        // handle names one specific instance; an index found from the item id names whichever
        // copy the inventory filed lowest, which for several armaments differing only by ash is
        // an arbitrary one of them. A line that does not say which question was asked cannot be
        // used to diagnose a weapon that came out with somebody else's skill on it.
        //
        // Armament fallbacks are listed one by one and everything else is counted, because only
        // an armament can have two copies the item id cannot tell apart -- a talisman or a
        // quickbar consumable has no per-instance identity to take the wrong one of.
        let armament_fallbacks = worn
            .by_item_id
            .iter()
            .filter(|(kind, ..)| *kind == PositionKind::Armament);
        log_line(&format!(
            "[build-import] EQUIP RESOLUTION: {} position(s) found by minted gaitem handle, \
             {} by item id ({} of them armaments, where the id cannot tell copies apart); \
             {mintable} armament handle(s) were available",
            worn.by_handle,
            worn.by_item_id.len(),
            armament_fallbacks.clone().count()
        ));
        for (_, slot, item_id, why) in armament_fallbacks.take(12) {
            log_line(&format!(
                "[build-import]   ARMAMENT BY ITEM ID slot {slot} item 0x{item_id:08X} -- {why}; \
                 this position may hold a copy carrying another ash"
            ));
        }
        // A substitution, named. The build asked for one row and the character owns another row
        // of the same item because they have upgraded it, so the position is filled with an id
        // the build never mentions. That is the right answer and it still has to be said out
        // loud: an unannounced id swap is indistinguishable in a log from equipping the wrong
        // thing, and the alternative -- what this replaced -- was reporting the item as
        // not-in-inventory while it sat in the player's pouch.
        for (kind, slot, wanted, held) in worn.by_upgrade_variant.iter().take(12) {
            log_line(&format!(
                "[build-import]   UPGRADED ROW {} slot {slot}: the build names 0x{wanted:08X}, \
                 the character owns 0x{held:08X} -- the same item at another upgrade level, so \
                 that is what was equipped",
                kind.label()
            ));
        }
        // One entry, one slot -- refused collisions, named. A collision is not a near-miss: the
        // equip that was refused would have stripped the slot it collided with, so the log has to
        // say which slot kept the item and which position went without.
        if worn.index_collisions.is_empty() {
            log_line(
                "[build-import] EQUIP COLLISIONS: none -- every position named its own inventory entry",
            );
        } else {
            log_line(&format!(
                "[build-import] EQUIP COLLISIONS: {} position(s) REFUSED because an earlier slot \
                 already wears that exact inventory entry; equipping it again would have stripped \
                 the earlier slot",
                worn.index_collisions.len()
            ));
            for (slot, item_id, item_idx, held_by) in worn.index_collisions.iter().take(12) {
                log_line(&format!(
                    "[build-import]   COLLISION slot {slot} item 0x{item_id:08X} invIdx {item_idx} \
                     is already worn in slot {held_by}"
                ));
            }
        }

        // After everything. Each per-position read-back ran before the positions following it, so
        // it can only prove its own write landed. This is the sweep that proves it survived.
        if worn.stripped_after_verifying.is_empty() {
            log_line(
                "[build-import] EQUIP FINAL SWEEP: no position that read back correctly was \
                 taken back off before the pass ended",
            );
        } else {
            log_line(&format!(
                "[build-import] EQUIP FINAL SWEEP: {} position(s) read back CORRECTLY and no \
                 longer hold that item -- something later in the pass took them back off",
                worn.stripped_after_verifying.len()
            ));
            for (slot, expected, actual) in worn.stripped_after_verifying.iter().take(12) {
                log_line(&format!(
                    "[build-import]   STRIPPED slot {slot} expected {expected} but holds {actual}"
                ));
            }
        }
        // The right armament, a different upgrade level. Counted as placed -- which armament is
        // in which hand is what the equip decides, and the level is the grant's, reported on its
        // own armament line -- but never silent, because "+25 imported as +0" is a complaint a
        // reader must be able to answer from this file.
        for (slot, expected, actual) in worn.level_differences.iter().take(12) {
            log_line(&format!(
                "[build-import]   LEVEL slot {slot}: the right armament at another upgrade level \
                 -- placed {expected}, holds {actual}"
            ));
        }

        if worn.no_inventory {
            log_line(
                "[build-import] EQUIP: the inventory pointer was null, so NOTHING was attempted",
            );
        }
        if !worn.unresolved_natives.is_empty() {
            // Named one by one on purpose: each one is a `docs/recon/rva-map-1162-to-1170` row
            // that does not exist yet, and a count names none of them.
            log_line(&format!(
                "[build-import] EQUIP: NOTHING WAS ATTEMPTED -- {} game function(s) have no \
                 verified mapping for the running build: {}. Every planned position is recorded \
                 as not attempted rather than dropped from the denominator.",
                worn.unresolved_natives.len(),
                worn.unresolved_natives.join(", ")
            ));
        }
        for (slot, permitted) in &worn.gate {
            log_line(&format!("[build-import]   gate(slot {slot}) = {permitted}"));
        }
        for (slot, expected, actual) in &worn.mismatches {
            log_line(&format!(
                "[build-import]   SLOT {slot} expected {expected} but holds {actual}"
            ));
        }

        // The one read-back that answers the player'S question.
        //
        // Grants and equips can both be green while the character holds a bare weapon: the grant
        // proves an instance exists, the equip proves a slot holds that item ID, and neither can
        // see which instance the slot took. A build routinely carries several copies of one
        // armament differing only by ash, so the id is not a unique name for a weapon. This walks
        // the worn armament itself -- slot -> gaitem handle -> instance -> equipped gem -> arts
        // row -- and says what is actually in the player's hands.
        let mut correct = 0usize;
        let mut asked = 0usize;
        for want in &wants {
            let Some(gem) = (want.weapon_skill != er_build_import_core::plan::NO_SKILL)
                .then_some(want.weapon_skill & !er_build_import_core::plan::GEM_ITEM_CATEGORY)
            else {
                continue;
            };
            asked += 1;
            let wanted_arts = catalog::arts_row_for_gem(gem);
            // Safety: game thread, character in the world -- the caller's own preconditions.
            let worn_arm = unsafe { read_character::worn_armament(module_base, want.slot) };
            let held = worn_arm.and_then(|arm| arm.arts_id);
            let held_name = held.and_then(|arts| {
                // Safety: `msg` is the live repository this import already read from.
                unsafe {
                    catalog::name_for(
                        er_build_import_core::catalog::Kind::AshOfWar,
                        msg,
                        module_base,
                        arts,
                    )
                }
            });
            // Which armament the plan put here, so the two ways of being wrong can be told apart
            // by the log rather than by a reader cross-referencing two sections of it. A slot
            // holding a different item id holds another armament entirely; a slot holding the
            // right id with the wrong arts row holds a different copy of the right armament,
            // which is the exact failure the gaitem-handle threading exists to prevent and the
            // only one an id-keyed equip could ever produce.
            let planned_item = er_build_import_core::equip::armament_planner_index(want.slot)
                .and_then(|index| equips.armaments.get(index as usize))
                .and_then(|entry| entry.as_ref())
                .map(|item| item.item_id);
            let verdict = match (wanted_arts, held) {
                (Some(wanted), Some(got)) if wanted == got => {
                    correct += 1;
                    "OK"
                }
                (_, None) if worn_arm.is_none() => "EMPTY -- no armament is worn in this slot",
                (_, None) => "NOT MOUNTED -- the worn armament reports no sword-arts row",
                // Compared without the upgrade level, which lives in the id's last two digits:
                // the plan names an armament, the level is a separate dimension of it, and a
                // worn +25 would otherwise read as a different weapon from the +25 that was
                // placed here.
                _ if worn_arm.map(read_character::WornArmament::armament_identity)
                    != planned_item.map(|id| id / 100 * 100) =>
                {
                    "WRONG ARMAMENT -- the worn item id is not the one the plan placed in this slot"
                }
                // Same item id, wrong arts row. Either the equip took another copy of this
                // armament (copies differing only by ash share an item id), or the armament does
                // not accept ashes at all -- `EquipParamWeapon::canGemBeChanged` gates the read,
                // so a gem mounted on such a weapon is stored and then ignored.
                _ => "WRONG COPY OR NO GEM SLOT -- the right armament, carrying the wrong ash",
            };
            // The upgrade level, read off the worn instance. Printed here because it is the
            // number the player is looking at when they say a build imported at +0, and because
            // `GetReinforcement` is not it: that field read 25 for a whole session of +0 weapons.
            let worn_item = worn_arm.map_or_else(
                || "none".to_owned(),
                |arm| format!("0x{:08X} (+{})", arm.item_id, arm.level()),
            );
            log_line(&format!(
                "[build-import]   ASH slot {} {:?} wants {:?} (gem {gem} -> arts {:?}); \
                 worn item {worn_item} holds arts {:?} {:?} -- {verdict}",
                want.slot, want.weapon, want.art, wanted_arts, held, held_name
            ));
        }
        log_line(&format!(
            "[build-import] EQUIPPED ASHES (read back from the worn armament): \
             {correct}/{asked} correct"
        ));
        for (slot, id, idx, got) in &worn.dispatch {
            // Both ids in the same base, because the whole point of this line is that a reader
            // can adjudicate it without a calculator. `-1` is the position being empty.
            log_line(&format!(
                "[build-import]   QUICK/POUCH/RUNE slot {slot} (index {}) item 0x{id:08X} \
                 invIdx {idx} -> the position reads back {} ({})",
                slot - CHR_ASM_SLOT_QUICK_BASE,
                if *got < 0 {
                    "EMPTY".to_owned()
                } else {
                    format!("0x{got:08X}")
                },
                if *got == *id as i32 { "OK" } else { "WRONG" }
            ));
        }
        for id in worn.not_in_inventory.iter().take(12) {
            log_line(&format!("[build-import]   NOT-IN-INVENTORY 0x{id:08X}"));
        }
        // What is there instead. `NOT-IN-INVENTORY` names the row the build wanted and nothing
        // else, and for the two flasks it was wrong in the same way in every run there is a log
        // of: the character owned the item, at an upgrade level whose row is a different id. The
        // one fact that diagnoses it is the id the position is holding, and this is that id.
        for (slot, wanted, actual) in worn.position_holds_instead.iter().take(12) {
            log_line(&format!(
                "[build-import]   POSITION slot {slot} wanted 0x{wanted:08X} and no row of that \
                 item is in the inventory; the position itself holds {}",
                if *actual < 0 {
                    "nothing".to_owned()
                } else {
                    format!("0x{actual:08X}")
                }
            ));
        }

        // Physick: log what the flask held before, so a pre-existing value is never mistaken for
        // something this importer wrote.
        // Safety: game thread, character loaded.
        let before = unsafe { equip_native::read_physick(module_base, egd) };
        let wanted_tears: Vec<(&str, u32)> = equips
            .physick
            .iter()
            .flatten()
            .map(|t| (t.name.as_str(), t.item_id))
            .collect();
        let filled = unsafe { equip_native::fill_physick(module_base, egd, &equips.physick) };
        let after = unsafe { equip_native::read_physick(module_base, egd) };
        report.physick = (filled.unwrap_or(0), wanted_tears.len());
        // UNREADABLE is not empty. `-1` is the flask's own "this slot holds nothing", so a
        // refusal rendered as `-1` would tell the reader the tears were not written when in fact
        // they were written and could not be read back.
        let render = |flask: Option<[i32; 2]>| match flask {
            Some(values) => format!("{:?}", values.map(|value| format!("0x{value:08X}"))),
            None => {
                "UNREADABLE (GetPhysicTearBySlot has no verified mapping for this build)".to_owned()
            }
        };
        log_line(&format!(
            "[build-import] PHYSICK: {} verified out of {}. wants {:?}; flask was {} -> now {} \
             (-1 = empty)",
            match filled {
                Some(filled) => filled.to_string(),
                None => "UNVERIFIABLE -- the tears were written, the read-back native is \
                         unavailable, so 0"
                    .to_owned(),
            },
            wanted_tears.len(),
            wanted_tears,
            render(before),
            render(after)
        ));
        // The physick is the one planned position `equip_all` does not own, so it is recorded
        // here from the same read-back the line above prints. If this loop ever stops running,
        // the tears go back to being UNACCOUNTED rather than silently disappearing.
        for (index, tear) in equips.physick.iter().enumerate() {
            let Some(tear) = tear else { continue };
            let expected = tear.item_id as i32;
            let result = match after.and_then(|flask| flask.get(index).copied()) {
                Some(actual) if actual == expected => PositionResult::Verified,
                Some(actual) => PositionResult::Mismatch { expected, actual },
                // Written, unprovable. Not `Mismatch { actual: -1 }`, which would claim the
                // engine reported an empty slot.
                None => PositionResult::NotAttempted(
                    "the tear was written but GetPhysicTearBySlot has no verified mapping for \
                     the running build, so nothing read it back",
                ),
            };
            if !ledger.record_kind(PositionKind::Physick, index, result) {
                log_line(&format!(
                    "[build-import] ACCOUNTING BUG: physick {index} was written but the plan \
                     never listed it"
                ));
            }
        }

        // Great rune: read the equipped rune back and light the rune arc.
        if let Some(rune) = equips.great_rune.as_ref() {
            // Safety: game thread; a native getter plus one bool in live save data.
            let equipped = unsafe { equip_native::equipped_great_rune(module_base, egd) };
            let active = unsafe { character::activate_rune_arc() };
            log_line(&format!(
                "[build-import] GREAT RUNE: {:?} -> GetEquippedGreatrune reports {}, \
                 runeArcActive={active}",
                rune.name,
                match equipped {
                    Some(equipped) => equipped.to_string(),
                    None => "NOTHING -- the getter has no verified mapping for the running build"
                        .to_owned(),
                }
            ));
        }

        // The one line. It reconciles against the plan, names every position that did not end up
        // holding the build's item, and is the last word on this pass -- so a partial import
        // cannot be read as a complete one no matter which family of positions went missing.
        let counts = ledger.counts();
        report.equipped = (counts.verified + counts.already, counts.planned);
        log_line(&format!(
            "[build-import] EQUIP LEDGER: {}",
            ledger.headline()
        ));
        // The SUBTRACTION nobody did. Every number needed to catch the last defect was already
        // in this file -- `25 planned`, `5 failed`, `3 position(s) no longer hold what was
        // written` -- in three separate lines, and no line differenced them, so a pass that
        // dropped a fifth of its work read as a pass that mostly worked. This one reconciles or
        // names its own casualties, and it is derived from the same ledger as the headline so
        // the two cannot disagree.
        log_line(&format!(
            "[build-import] EQUIP BALANCE: {}",
            ledger.balance(worn.stripped_after_verifying.len())
        ));
        for failure in ledger.failures() {
            log_line(&format!("[build-import]   NOT EQUIPPED: {failure}"));
        }
    }

    // Class before stats: the level-up menu derives its per-attribute floors from the archetype's
    // CharaInitParam row, so setting the class first means anything that re-reads those floors
    // already sees the right class.
    if let Some(pgd) = unsafe { character::player_game_data() } {
        match doc.character_class.as_deref() {
            Some(class) => match unsafe { character::set_class(pgd, class) } {
                Some((wanted, got)) => log_line(&format!(
                    "[build-import] CLASS: {class} -> archetype {wanted}, read back {got} ({})",
                    if wanted == got { "OK" } else { "MISMATCH" }
                )),
                None => log_line(&format!("[build-import] CLASS: unrecognised {class:?}")),
            },
            None => log_line("[build-import] CLASS: build names none, left alone"),
        }

        // Name, before the stats. Not for correctness -- nothing here depends on the order -- but
        // because `ApplyMainPlayerStats` recomputes and re-renders a pile of derived state, and a
        // rename that lands first is visible on the very next frame the stats pass causes rather
        // than one frame later.
        //
        // This is the only rename Elden Ring has: `CS::PlayerGameData::CopyChrName` is the sole
        // writer of the name, and the game exposes no UI that calls it after character creation.
        // What it does not do is write the save -- the ProfileSummary record and the `.sl2` copy
        // both come from `PlayerGameData` at the next save the game performs, which under the
        // product DLL means the System>Quit "Save Game" row and nothing else.
        //
        // Safety: game thread, character in the world (gated by the caller), `pgd` read above.
        let named = unsafe { chr_name::adopt_build_name(module_base, pgd, &doc.name) };
        report.name = named.adopted().map(str::to_owned);
        log_line(&format!("[build-import] NAME: {}", named.label()));

        // Safety: game thread, character in the world (gated by the caller).
        match unsafe { character::apply_stats(module_base, pgd, doc) } {
            Some(stats) => {
                report.level = stats.level.1;
                report.attributes_wrong = stats.wrong.len();
                log_line(&format!(
                    "[build-import] STATS: level {} -> {} ({} attributes wrong after read-back)",
                    stats.level.0,
                    stats.level.1,
                    stats.wrong.len()
                ));
                for (name, want, got) in &stats.wrong {
                    log_line(&format!(
                        "[build-import]   {name}: wanted {want}, holds {got}"
                    ));
                }
                if stats.is_correct() {
                    log_line("[build-import] STATS: every attribute matches the build");
                }
            }
            // Said, not skipped. The stat pass going inert on a build that moved the code is
            // exactly the case a silent default outcome would have reported as a clean import.
            None => log_line(
                "[build-import] STATS: NOT APPLIED -- the level/attribute natives have no \
                 verified mapping for the running build. The character keeps the stats it had.",
            ),
        }
    }

    // Spells last: ApplyMainPlayerStats recomputes the memory-slot count from Mind, so asking the
    // game for capacity before the stats are applied would use the old number.
    if let Some(egd) = unsafe { grant::equip_game_data() } {
        // Safety: game thread, character loaded.
        match unsafe { character::memorise_spells(module_base, egd, &equips.spells) } {
            Some(spells) => {
                report.spells = (spells.verified, spells.wanted);
                log_line(&format!(
                    "[build-import] SPELLS (read back): {}/{} memorised, capacity {}, {} over \
                     capacity, {} old slot(s) cleared first, {} NOT THE BUILD'S",
                    spells.verified,
                    spells.wanted,
                    spells.capacity,
                    spells.over_capacity,
                    spells.cleared,
                    spells.stale
                ));
                // The clear is the half of the pass a per-slot read-back cannot see, so both of
                // its failure shapes are said rather than left to be inferred from the counts.
                if spells.clear_declined {
                    log_line(
                        "[build-import]   SPELLS: the build names no spells, so nothing was \
                         cleared -- an absent `spells` key and an empty one are the same payload, \
                         and so is a build whose every spell was rejected by the catalog.",
                    );
                } else if spells.stale > 0 {
                    log_line(&format!(
                        "[build-import]   SPELLS: {} slot(s) past the build's list are still \
                         occupied -- the character keeps spells the build did not ask for.",
                        spells.stale
                    ));
                }
                for (slot, expected, actual) in &spells.mismatches {
                    log_line(&format!(
                        "[build-import]   SPELL slot {slot} expected {expected} but holds {actual}"
                    ));
                }
            }
            None => log_line(&format!(
                "[build-import] SPELLS: NOT MEMORISED -- the spell natives have no verified \
                 mapping for the running build. {} spell(s) in the build were left unmemorised.",
                equips.spells.len()
            )),
        }
    }

    Some(report)
}

/// Base address of the loaded game image.
fn module_base() -> usize {
    // Safety: a null module name asks for the process image, which always exists.
    unsafe { GetModuleHandleW(None) }
        .map(|handle| handle.0 as usize)
        .unwrap_or_default()
}

/// Append one line to the log beside the game executable, flushing immediately.
///
/// Per line rather than per run: a probe that dies partway is the interesting case, and a buffered
/// report loses precisely the evidence that matters when it does -- an earlier version accumulated
/// the whole report in a `String` and flushed at the end, so a worker that died left nothing at all
/// and looked exactly like a DLL that never loaded.
///
/// Routed through `er_game_base::log` so the file describes one run: the shared helper rotates the
/// previous run's log aside on first write instead of letting runs pile up in one file.
pub fn log_line(line: &str) {
    let path = er_game_base::log::game_directory_path()
        .map(|dir| dir.join(LOG_NAME))
        .unwrap_or_else(|| PathBuf::from(LOG_NAME));
    er_game_base::log::append_line(&path, format_args!("{line}"));
}
