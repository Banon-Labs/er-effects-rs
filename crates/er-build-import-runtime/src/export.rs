//! Turning the live character into an `er-build-planner` share link.
//!
//! The mirror image of [`crate::request`]/[`crate::tick`], and deliberately the same SHAPE: a press
//! latches a request, the game-thread step does the part that must touch game memory, and a worker
//! does the part that must not run on the game thread. What differs is which half is slow.
//!
//! * The importer's slow half is at the front -- a blocking HTTPS get -- so its worker runs first
//!   and its game-thread step applies the result.
//! * The exporter has no network at all. The `?i=` share format carries the whole build in the URL
//!   (LZUTF8 over base64 over JSON), so nothing is fetched and no account is minted on someone
//!   else's free hobby service. Its slow half is at the BACK: `ShellExecuteW` spawns
//!   `winebrowser`, which spawns `xdg-open`, which spawns a browser. So the game-thread step runs
//!   first -- reading the character is a few dozen native getter calls, microseconds -- and the
//!   worker takes the finished document away to encode, copy and open.
//!
//! # The in-flight latch must lose to the player
//!
//! A press while an export is running has to be refused, or two workers race for the clipboard. But
//! a latch that can only say "busy" is a latch that can strand the row: the link field next door
//! went dead for three consecutive presses because an active-flag survived a rebuilt dialog with
//! nothing left alive to clear it.
//!
//! So [`export_latch_is_stale`] answers from live evidence, never from a flag alone:
//!
//! * [`Phase::Reading`] is claimed by the game-thread step, which runs every frame. A request that
//!   is still `Reading` after the step has run [`STALE_TICKS`] times was not picked up and never
//!   will be -- the tick's own counter is the witness, not a clock.
//! * [`Phase::Opening`] belongs to a worker thread that increments `WORKERS_ALIVE` on entry and
//!   decrements it from a `Drop` guard, so a panicking worker still releases it. `Opening` with no
//!   worker alive is a phase nobody owns.
//!
//! When it cannot prove the export is live, the press wins and the latch is cleared. The worst case
//! of being wrong that way is two browser tabs; the worst case of the other way is a row that never
//! works again until the game restarts.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::read_character::CharacterRead;

/// How many game-thread ticks may pass with a request unclaimed before it is considered stranded.
///
/// This is a count of ticks that did run, not a duration: the step re-checks its preconditions
/// every frame and claims any `Reading` request whose preconditions hold, so a request that has
/// survived this many of its own opportunities is one the step is refusing, not one it has not
/// reached yet. Generous because the first frames after a menu opens are the busiest.
pub const STALE_TICKS: usize = 240;

/// Where an export is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
pub enum Phase {
    /// Nothing in flight.
    Idle = 0,
    /// A press has asked; the game-thread step has not picked it up yet.
    Reading = 1,
    /// A worker holds the document and is encoding / copying / opening.
    Opening = 2,
    /// The last export finished. Not "busy".
    Done = 3,
    /// The last export failed; [`take_error`] says why, once.
    Failed = 4,
}

impl Phase {
    fn from_code(code: usize) -> Phase {
        match code {
            1 => Phase::Reading,
            2 => Phase::Opening,
            3 => Phase::Done,
            4 => Phase::Failed,
            _ => Phase::Idle,
        }
    }

    /// Whether a press may claim the machine from here.
    fn accepts_request(self) -> bool {
        matches!(self, Phase::Idle | Phase::Done | Phase::Failed)
    }
}

static PHASE: AtomicUsize = AtomicUsize::new(Phase::Idle as usize);
/// Ticks the game-thread step has run, ever. The witness `Reading` staleness is measured against.
static TICKS: AtomicUsize = AtomicUsize::new(0);
/// [`TICKS`] as it stood when the pending request was latched.
static REQUEST_TICK: AtomicUsize = AtomicUsize::new(0);
/// Workers currently inside [`spawn_worker`]. Released by a `Drop` guard, so a panic still clears
/// it -- which is what makes "Opening with no worker" a sound staleness test rather than a guess.
static WORKERS_ALIVE: AtomicUsize = AtomicUsize::new(0);
/// The finished URL, for the caller that wants to log or show it.
static LAST_URL: Mutex<Option<String>> = Mutex::new(None);
/// Why the last export failed.
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// Current phase.
pub fn phase() -> Phase {
    Phase::from_code(PHASE.load(Ordering::SeqCst))
}

/// Take the reason the last export failed, clearing it. Taken rather than peeked for the same
/// reason the importer's is: the failure is asynchronous, so the only way anyone learns of it is by
/// polling, and a peek would re-report one failure on every frame.
pub fn take_error() -> Option<String> {
    LAST_ERROR.lock().ok().and_then(|mut slot| slot.take())
}

/// Take the URL the last export produced.
pub fn take_url() -> Option<String> {
    LAST_URL.lock().ok().and_then(|mut slot| slot.take())
}

fn set_error(reason: String) {
    crate::log_line(&format!("[build-export] FAILED: {reason}"));
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = Some(reason);
    }
    PHASE.store(Phase::Failed as usize, Ordering::SeqCst);
}

/// Whether a busy-looking latch cannot be shown to be live.
///
/// Returns `true` only when there is positive evidence that nothing owns the phase. See the module
/// header for why the answer defaults toward letting the player through.
pub fn export_latch_is_stale() -> bool {
    match phase() {
        // A request nobody claimed, after the claimer had this many chances.
        Phase::Reading => {
            TICKS
                .load(Ordering::SeqCst)
                .saturating_sub(REQUEST_TICK.load(Ordering::SeqCst))
                >= STALE_TICKS
        }
        // A phase whose only owner is a thread that is not running.
        Phase::Opening => WORKERS_ALIVE.load(Ordering::SeqCst) == 0,
        // Not busy at all.
        _ => false,
    }
}

/// One line describing the latch, for the debug log. A refusal that does not say what it was
/// refusing on behalf of is a refusal nobody can diagnose.
pub fn export_latch_state() -> String {
    format!(
        "phase={:?} ticks={} request_tick={} workers_alive={}",
        phase(),
        TICKS.load(Ordering::SeqCst),
        REQUEST_TICK.load(Ordering::SeqCst),
        WORKERS_ALIVE.load(Ordering::SeqCst),
    )
}

/// Force the machine back to idle. Called when a press finds a stale latch, and when the Quit
/// dialog is rebuilt.
pub fn reset() {
    PHASE.store(Phase::Idle as usize, Ordering::SeqCst);
    REQUEST_TICK.store(TICKS.load(Ordering::SeqCst), Ordering::SeqCst);
}

/// Why [`request`] refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RequestError {
    /// An export is genuinely still running -- proven, not assumed.
    Busy,
}

impl core::fmt::Display for RequestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RequestError::Busy => write!(f, "an export is already running"),
        }
    }
}

/// Ask for an export. Returns as soon as the request is latched; the game thread must keep calling
/// [`tick`] for anything to happen.
///
/// Safe to call from any thread, including a menu action handler: nothing here touches game state.
pub fn request() -> Result<(), RequestError> {
    if !phase().accepts_request() {
        return Err(RequestError::Busy);
    }
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = None;
    }
    REQUEST_TICK.store(TICKS.load(Ordering::SeqCst), Ordering::SeqCst);
    PHASE.store(Phase::Reading as usize, Ordering::SeqCst);
    Ok(())
}

/// What one completed export did. Every field is measured from the document that was actually
/// encoded, never from a call that returned.
#[derive(Clone, Debug, Default)]
pub struct ExportReport {
    /// The character's name, as the link will carry it.
    pub character: String,
    /// Every category the link carries, counted off the encoded document -- see
    /// [`er_build_export::model::WrittenCategories`] for why it is not counted off the read.
    pub written: er_build_export::model::WrittenCategories,
    /// Equipment slots holding an item whose id resolved to no name, so a short build says so.
    pub unnamed: usize,
    /// Carried items the export deliberately leaves out, so an omission by design is never
    /// mistaken for one by accident. See `read_character::read_carried`.
    pub skipped_goods: usize,
    pub skipped_ammunition: usize,
    /// Characters in the finished URL.
    pub url_len: usize,
    /// Whether the URL reached the clipboard, and whether a browser accepted it.
    pub clipboard: bool,
    pub opened: bool,
}

impl ExportReport {
    /// One line for a menu help field or a log.
    ///
    /// # Every category is named, including the empty ones
    ///
    /// A summary that lists only what it found cannot distinguish a build with no quickbar from an
    /// exporter that never wrote one -- which is the exact question "only the physick came
    /// through" was, and it was answered on the website rather than in this line because the line
    /// did not mention the quickbar, the pouch or the ammunition at all. So each is printed with
    /// its count whether or not the count is zero, and the two categories that are skipped on
    /// purpose are named as skipped rather than omitted.
    pub fn summary(&self) -> String {
        let written = &self.written;
        format!(
            "wrote {} armaments, {} armour, {} talismans, {} spells, {} quickbar, {} pouch, \
             {} ammo, {} physick{}{}; skipped {} carried goods and {} carried ammunition rows \
             (not exported by design) -> {} char link{}{}",
            written.armaments,
            written.protectors,
            written.talismans,
            written.spells,
            written.quickbar,
            written.pouch,
            written.ammo,
            written.physick,
            if written.great_rune {
                ", great rune"
            } else {
                ""
            },
            if written.sliders { ", face" } else { "" },
            self.skipped_goods,
            self.skipped_ammunition,
            self.url_len,
            if self.clipboard { ", copied" } else { "" },
            if self.opened { ", opened" } else { "" },
        )
    }
}

/// What the worker must be able to do, supplied by the caller.
///
/// The read and the encode belong to this crate; the clipboard and the browser belong to Windows,
/// and this crate is linked into more than one DLL. Handing them in as function pointers keeps the
/// OS surface at the call site, where the `windows` crate features that back it are already
/// declared, instead of forcing every consumer to carry them.
#[derive(Clone, Copy)]
pub struct Sinks {
    /// Put the URL on the clipboard. Returns whether it landed. `None` when the caller has no
    /// clipboard at all.
    pub clipboard: Option<fn(&str) -> bool>,
    /// Hand the URL to the OS to open. Returns whether it was accepted. `None` when the caller has
    /// no browser to offer -- which is not the same as a browser that refused, and must not be
    /// reported as a failed export.
    pub open: Option<fn(&str) -> bool>,
}

impl Sinks {
    /// Sinks for a caller whose only output is the log: the harness DLL, whose entire job is to
    /// put one link in front of a reader without a menu press or a browser window.
    #[must_use]
    pub const fn log_only() -> Self {
        Self {
            clipboard: None,
            open: None,
        }
    }
}

/// One frame of the exporter. Does nothing until a press has asked and the game can be read.
///
/// Returns the report only once the whole export is done, which is a worker later -- so a caller
/// polling this sees `None` for the frames in between rather than a half-finished answer.
///
/// # Safety
///
/// Game task thread. Every step is precondition-checked, but the thread itself is not something
/// this function can verify.
pub unsafe fn tick(sinks: Sinks) -> Option<ExportReport> {
    TICKS.fetch_add(1, Ordering::SeqCst);
    if phase() != Phase::Reading {
        return None;
    }
    if !crate::catalog::params_ready() {
        return None;
    }
    // Safety: game thread; the helper is fault-checked and returns false at the title screen.
    if !unsafe { crate::grant::player_present() } {
        return None;
    }
    let msg = crate::catalog::msg_repository()?;
    // Safety: game thread, character present.
    let egd = unsafe { crate::grant::equip_game_data() }?;
    let module_base = crate::module_base();

    // Safety: game task thread, params streamed, character in the world -- all three just checked.
    let Some(read) = (unsafe { crate::read_character::read_character(module_base, msg, egd) })
    else {
        set_error("the character could not be read".to_owned());
        return None;
    };
    crate::log_line(&format!(
        "[build-export] read character={:?} class={:?} armaments={} armour={} talismans={} \
         spells={} quickbar={} pouch={} worn_ammo={} tears={} rune={:?} 2h={} flasks={}+{} \
         upgrade={:?} unnamed={} whole_inventory={} ammunition_skipped={} \
         goods_carried_not_exported={}",
        read.name,
        read.character_class,
        read.armaments.len(),
        read.protectors.len(),
        read.talismans.len(),
        read.spells.len(),
        read.quickbar.iter().flatten().count(),
        read.pouch.iter().flatten().count(),
        read.ammo.iter().flatten().count(),
        read.crystal_tears.iter().flatten().count(),
        read.great_rune,
        read.two_handing,
        read.flask_crimson,
        read.flask_cerulean,
        read.weapon_upgrade,
        read.unnamed_slots,
        read.read_whole_inventory,
        read.carried_ammunition,
        read.carried_goods,
    ));
    // The per-armament levels and the appearance, both of which used to be absent from the link
    // entirely. Logged as their own line because they are the two things a reader of the last
    // export's evidence most needs to check.
    crate::log_line(&format!(
        "[build-export] worn or ash-carrying armaments (name, +level, equip slot, ash, gaitem \
         handle) {:?}; face data {}",
        read.armaments
            .iter()
            .filter(|item| item.weapon_art.is_some() || item.equip_index.is_some())
            .map(|item| {
                (
                    item.name.as_str(),
                    item.upgrade,
                    item.equip_index,
                    item.weapon_art.as_deref(),
                    item.gaitem_handle.map(|handle| format!("{handle:#010x}")),
                )
            })
            .collect::<Vec<_>>(),
        match read.face_data.as_deref() {
            Some(bytes) => format!("{} bytes", bytes.len()),
            None => "NOT READ".to_owned(),
        }
    ));

    PHASE.store(Phase::Opening as usize, Ordering::SeqCst);
    spawn_worker(read, sinks);
    None
}

/// Hand the finished document to a worker for the parts that must not run on the game thread.
fn spawn_worker(read: CharacterRead, sinks: Sinks) {
    /// Releases the alive count on every exit path, panic included. Without this a panicking worker
    /// would leave `Opening` looking owned forever, which is exactly the stranded-latch failure the
    /// module header exists to prevent.
    struct Alive;
    impl Drop for Alive {
        fn drop(&mut self) {
            WORKERS_ALIVE.fetch_sub(1, Ordering::SeqCst);
        }
    }

    WORKERS_ALIVE.fetch_add(1, Ordering::SeqCst);
    std::thread::spawn(move || {
        let _alive = Alive;
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| export_inner(read, sinks)))
            .is_err()
        {
            set_error("export worker PANICKED".to_owned());
        }
    });
}

/// The encode, the clipboard and the browser. Runs on the worker; touches no game state.
fn export_inner(read: CharacterRead, sinks: Sinks) {
    let doc = crate::export_doc::document_from(&read);
    let written = doc.written_categories();
    // The document'S own account of itself, before it is encoded. Every category is named with
    // its count -- zeros included -- and the two that are left out on purpose are named as left
    // out, so a category that silently stopped being written shows up as a `0` in a log line
    // instead of as a missing row on somebody else's website.
    crate::log_line(&format!(
        "[build-export] document WRITES armaments={} armour={} talismans={} spells={} \
         quickbar={} pouch={} tools_unassigned={} ammo={} physick={} greatRune={} face={}; \
         SKIPS BY DESIGN carried goods={} carried ammunition rows={}",
        written.armaments,
        written.protectors,
        written.talismans,
        written.spells,
        written.quickbar,
        written.pouch,
        written.tools_unassigned,
        written.ammo,
        written.physick,
        written.great_rune,
        written.sliders,
        read.carried_goods,
        read.carried_ammunition,
    ));
    let (url, stored) = share_link(&doc);
    let mut report = ExportReport {
        character: read.name.clone(),
        written,
        unnamed: read.unnamed_slots,
        skipped_goods: read.carried_goods,
        skipped_ammunition: read.carried_ammunition,
        url_len: url.chars().count(),
        ..ExportReport::default()
    };
    crate::log_line(&format!(
        "[build-export] {} link, {} characters, for {:?}",
        if stored {
            "STORED ?b="
        } else {
            "self-contained ?i="
        },
        report.url_len,
        report.character
    ));
    // The whole URL, on its own line, so the offline oracle can decode exactly what the game
    // produced instead of a description of it.
    crate::log_line(&format!("[build-export] URL {url}"));

    report.clipboard = sinks.clipboard.is_some_and(|copy| copy(&url));
    report.opened = sinks.open.is_some_and(|open| open(&url));
    crate::log_line(&format!(
        "[build-export] clipboard={} browser={}",
        match sinks.clipboard {
            Some(_) => report.clipboard.to_string(),
            None => "n/a".to_owned(),
        },
        match sinks.open {
            Some(_) => report.opened.to_string(),
            None => "n/a".to_owned(),
        }
    ));

    if let Ok(mut slot) = LAST_URL.lock() {
        *slot = Some(url);
    }
    // A browser that refused is a failure the player has to be told about. A caller with no
    // browser at all is not: the harness's link went to the log, which is where it was asked to go.
    if sinks.open.is_some() && !report.opened {
        // A link that was built but never reached a browser is a failure the player must be told
        // about -- silently succeeding here is how "I pressed it and nothing happened" happens.
        set_error(format!(
            "the link was built ({} characters){} but no browser would open it",
            report.url_len,
            if report.clipboard {
                " and copied to the clipboard"
            } else {
                ""
            },
        ));
    } else {
        PHASE.store(Phase::Done as usize, Ordering::SeqCst);
    }
    if let Ok(mut slot) = LAST_REPORT.lock() {
        *slot = Some(report);
    }
}

/// The link to hand the player: the self-contained one when it fits, a stored one when it does not.
///
/// # A real inventory does not fit in a URL
///
/// The `?i=` form carries the whole document, and the whole document is what the player asked for
/// -- every copy of every armament. One live character came to 87 KB of JSON and a
/// 22,663-character link, which no browser sends and the planner never sees. So past
/// [`crate::upload::MAX_SELF_CONTAINED_URL_CHARS`] the build is stored on the planner instead and
/// the link becomes a short `?b=<id>`.
///
/// A store that fails falls back to the long link rather than to nothing: it may be too long for
/// this player's browser, but it is still the build, and it is still on their clipboard.
fn share_link(doc: &er_build_export::BuildExportDoc) -> (String, bool) {
    let self_contained = er_build_export::share_url(doc);
    if self_contained.chars().count() <= crate::upload::MAX_SELF_CONTAINED_URL_CHARS {
        return (self_contained, false);
    }
    crate::log_line(&format!(
        "[build-export] the self-contained link is {} characters, past the {} a browser will \
         carry -- storing the build on the planner instead",
        self_contained.chars().count(),
        crate::upload::MAX_SELF_CONTAINED_URL_CHARS,
    ));
    match crate::upload::store(doc) {
        Ok(id) => (
            format!("{}{id}", er_build_import_core::BUILD_URL_PREFIX),
            true,
        ),
        Err(err) => {
            crate::log_line(&format!(
                "[build-export] the upload FAILED ({err}); falling back to the long \
                 self-contained link, which this browser may refuse"
            ));
            (self_contained, false)
        }
    }
}

/// The finished report, waiting for whichever thread asks next.
static LAST_REPORT: Mutex<Option<ExportReport>> = Mutex::new(None);

/// Take the report of the last completed export.
pub fn take_report() -> Option<ExportReport> {
    LAST_REPORT.lock().ok().and_then(|mut slot| slot.take())
}
