//! The seam. Everything that actually possesses a character plugs in here.
//!
//! Stack layer 1 shipped the config file, the hotkeys and this module with a [`NullEngine`] behind
//! it that refused politely. Layer 2 filled it in: [`crate::possess::NpcPossessionEngine`] is
//! installed at attach and this module is the boundary between it and everything above -- which
//! did not have to change when it arrived, which was the point of cutting the seam here.
//!
//! # Who calls what
//!
//! * **The frame task calls [`on_hotkey_edge`]**, once per rising edge of the possess hotkey --
//!   see `crate::tick`. It drives the state machine and forwards to the installed engine.
//! * **...and [`tick_engine`] every frame**, which drives the possession itself and reconciles the
//!   toggle with an engine that let go on its own.
//! * **`install` calls [`install_engine`]**, once, before the first frame task tick.
//! * **`DllMain` calls [`shutdown_engine`]** on `DLL_PROCESS_DETACH`, because an armed
//!   `ChrCtrl+0x3b0` outliving this DLL is a DLPanic nobody will be able to attribute.
//! * **Nothing calls [`PossessionEngine`] directly.** It is behind the state machine on purpose,
//!   because "the player pressed the key" and "we are now possessing something" are different
//!   facts and the failure mode worth designing against is treating them as one.
//!
//! # Why `accepts_reload` is on the engine and not on the config
//!
//! A config reload must never land mid-animation: the mapping tables are read while an attack is
//! playing, and swapping them under it would finish one character's swing with another's. Only the
//! engine knows whether the possessed body is in a neutral state, so it -- not the file poller --
//! decides when a reload may be consumed. Layer 1's engine has no animation state and always says
//! yes; the gate is wired up now so a later layer only has to answer the question, not also find
//! the place to ask it. `crate::tick` is the caller.

// Windows-only crate in practice; this module is pure state handling and stays ungated so its
// tests run on the host.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::Mutex;

use crate::settings::{ButtonSettings, MappingSettings, TargetSettings};

/// Everything an engine needs to start a possession, snapshotted at the instant the key was
/// pressed.
///
/// A snapshot rather than a borrow of the live config, because the config lock is held by the
/// file poller on the same thread and because a possession must be reproducible: the log line
/// naming what was requested has to describe what the engine was actually handed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PossessionRequest {
    /// The `[target]` table in force -- which is not necessarily the one on disk. See
    /// `crate::config::PossessConfig::adopt_staged_target`.
    pub(crate) target: TargetSettings,
    pub(crate) mapping: MappingSettings,
    /// Which bucket each of the four fixed inputs draws from. Snapshotted with the rest, so the
    /// layout cannot change under a possession that is already reporting it.
    pub(crate) buttons: ButtonSettings,
}

impl PossessionRequest {
    pub(crate) fn summary(&self) -> String {
        format!(
            "target[{}] mapping[{}] buttons[{}]",
            self.target.summary(),
            self.mapping.summary(),
            self.buttons.summary()
        )
    }
}

/// What an engine did with a request.
///
/// `Accepted` and `Refused` are constructed by [`crate::possess::NpcPossessionEngine`], which
/// landed in stack layer 2; the `#[allow(dead_code)]` that used to sit here while layer 1 shipped
/// only a `NullEngine` has been deleted, and the lint now keeps it deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PossessionOutcome {
    /// The engine took it. The state machine advances.
    Accepted,
    /// The engine looked and declined -- no valid target, the player is in a cutscene, the
    /// character is already possessed by something else. Carries the reason for the log line.
    Refused(String),
    /// There is no engine. Layer 1's answer, and the only one it can honestly give.
    NoEngine,
}

impl PossessionOutcome {
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Accepted => "accepted".to_owned(),
            Self::Refused(reason) => format!("refused({reason})"),
            Self::NoEngine => "no-engine".to_owned(),
        }
    }
}

/// The possession engine. Implement this in a later layer and hand it to [`install_engine`].
pub(crate) trait PossessionEngine: Send {
    /// Take control of the character the request selects.
    fn possess(&mut self, request: &PossessionRequest) -> PossessionOutcome;

    /// Give it back, whatever state it is in. Must be safe to call when nothing is possessed.
    fn release(&mut self) -> PossessionOutcome;

    /// May a config reload be consumed on this frame?
    ///
    /// `false` while an animation is playing. The default is `true` because an engine that does
    /// not track animation state has no reason to hold a reload back -- and a default of `false`
    /// would silently freeze the config of any engine that forgot to override it.
    fn accepts_reload(&self) -> bool {
        true
    }

    /// One frame. Called from the FrameBegin task whether or not anything is possessed.
    ///
    /// Added when the real engine landed. Layer 1's design note said an engine only had to call
    /// `install_engine` -- that was true of everything above this line and false of possession
    /// itself, which is a per-frame job: the player's body has to be re-neutered (the alpha has a
    /// decay modifier, the sound mute is cleared by four respawn paths), co-located, and the
    /// creature's position cached before a death makes it unreadable.
    fn tick(&mut self) {}

    /// Is a possession in flight right now?
    ///
    /// Not the same question as [`Possession::state`], and the difference is the reason this
    /// exists: an engine can let go on its own -- the creature died, or despawned -- with nobody
    /// having pressed anything. Without this the state machine would still believe it was
    /// `Active`, and the player's next press would "release" a possession that had already ended,
    /// costing them a second press to get anywhere. [`tick_engine`] reconciles the two.
    fn is_active(&self) -> bool {
        false
    }

    /// The process is going away. Release whatever is held, now.
    ///
    /// Distinct from [`Self::release`] because a shutdown release is not optional and not a
    /// player action: an engine that has written `ChrCtrl+0x3b0` must clear it before this DLL
    /// unloads, or the game DLPanics inside `ChrCtrl::Unref` the next time that character is torn
    /// down -- with our code no longer present to explain why.
    fn shutdown(&mut self) {
        let _ = self.release();
    }
}

/// The engine layer 1 ships: it refuses everything, and says why.
///
/// It is not a stub in the "unimplemented!()" sense -- it is the honest answer to the hotkey until
/// a real engine is installed, and it keeps the whole path above it (config, binding, edge
/// detection, logging) exercised and provable in the meantime.
struct NullEngine;

impl PossessionEngine for NullEngine {
    fn possess(&mut self, _request: &PossessionRequest) -> PossessionOutcome {
        PossessionOutcome::NoEngine
    }

    fn release(&mut self) -> PossessionOutcome {
        PossessionOutcome::NoEngine
    }
}

/// Where the possession is, as far as this crate knows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PossessionState {
    /// Nobody is possessed and nothing has been asked for.
    #[default]
    Idle,
    /// The player pressed the key and no engine took it. Layer 1 lives here.
    ///
    /// Deliberately distinct from `Active`: the difference between "the hotkey works" and "the
    /// mod works" is the entire question this layer exists to answer, and one state for both
    /// would make it unanswerable from a log.
    Requested,
    /// An engine accepted. Only reachable once one is installed.
    Active,
}

impl PossessionState {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Requested => "requested",
            Self::Active => "active",
        }
    }

    /// Is the player trying to be somebody else right now?
    const fn engaged(self) -> bool {
        matches!(self, Self::Requested | Self::Active)
    }
}

/// One press, and what it did. This is the structured log line's whole content.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HotkeyReport {
    pub(crate) source: &'static str,
    pub(crate) from: PossessionState,
    pub(crate) to: PossessionState,
    pub(crate) outcome: PossessionOutcome,
    pub(crate) request: PossessionRequest,
}

impl HotkeyReport {
    /// `key=... source=... state=idle->requested outcome=no-engine target[...] mapping[...]`
    pub(crate) fn line(&self, binding: &str) -> String {
        format!(
            "possess-hotkey: key={binding} source={} state={}->{} outcome={} {}",
            self.source,
            self.from.name(),
            self.to.name(),
            self.outcome.describe(),
            self.request.summary()
        )
    }
}

/// The toggle, independent of any engine so it can be driven from a host test.
#[derive(Debug, Default)]
pub(crate) struct Possession {
    state: PossessionState,
    presses: u64,
}

impl Possession {
    pub(crate) const fn state(&self) -> PossessionState {
        self.state
    }

    pub(crate) const fn presses(&self) -> u64 {
        self.presses
    }

    /// One rising edge of the hotkey. Possess when idle, release when not.
    ///
    /// A refusal leaves the state alone. An engine that declines because there is no valid target
    /// must not leave the mod believing it possessed something -- the next press would then
    /// "release" a possession that never happened, and the player would have to press twice to get
    /// anywhere. `NoEngine` is the exception and is deliberately not a refusal: layer 1 has to be
    /// able to demonstrate the state flipping, which is the whole of its runtime evidence.
    pub(crate) fn on_hotkey(
        &mut self,
        engine: &mut dyn PossessionEngine,
        source: &'static str,
        request: PossessionRequest,
    ) -> HotkeyReport {
        self.presses = self.presses.saturating_add(1);
        let from = self.state;
        let outcome = if from.engaged() {
            engine.release()
        } else {
            engine.possess(&request)
        };
        self.state = match (from, &outcome) {
            (PossessionState::Idle, PossessionOutcome::Accepted) => PossessionState::Active,
            (PossessionState::Idle, PossessionOutcome::NoEngine) => PossessionState::Requested,
            (PossessionState::Idle, PossessionOutcome::Refused(_)) => PossessionState::Idle,
            // Releasing always returns to idle, even on a refusal: an engine that cannot release
            // cleanly is a worse thing to be stuck inside than one that thinks it is idle, and
            // the alternative is a mod the player cannot get out of.
            (_, _) => PossessionState::Idle,
        };
        HotkeyReport {
            source,
            from,
            to: self.state,
            outcome,
            request,
        }
    }
}

/// The installed engine, or `None` for layer 1's [`NullEngine`].
///
/// A `Mutex` rather than an atomic slot: unlike a hotkey chord this is not read from a detour on
/// the game's own keyboard poll, only from the FrameBegin task, so there is no lock-free
/// requirement to satisfy and a trait object needs somewhere to live.
static ENGINE: Mutex<Option<Box<dyn PossessionEngine>>> = Mutex::new(None);

/// The global toggle the frame task drives.
static POSSESSION: Mutex<Possession> = Mutex::new(Possession {
    state: PossessionState::Idle,
    presses: 0,
});

/// Plug the possession engine in. The entry point for every later layer.
///
/// Call once, from the owning DLL's init, before the first frame task tick. Returns `false` if an
/// engine was already installed and the new one was dropped -- two engines writing the same
/// `ChrIns` is the failure this refuses rather than resolving by install order.
pub(crate) fn install_engine(engine: Box<dyn PossessionEngine>) -> bool {
    let mut slot = lock(&ENGINE);
    if slot.is_some() {
        return false;
    }
    *slot = Some(engine);
    true
}

/// Is an engine installed? The status line reports this, so "nothing happened" separates into
/// "the key never fired" and "the key fired and there was nobody to tell".
pub(crate) fn engine_installed() -> bool {
    lock(&ENGINE).is_some()
}

/// May a config reload be consumed right now? See the module docs.
pub(crate) fn accepts_reload() -> bool {
    lock(&ENGINE)
        .as_ref()
        .is_none_or(|engine| engine.accepts_reload())
}

/// Drive one hotkey press. Called by the frame task; see the module docs.
pub(crate) fn on_hotkey_edge(source: &'static str, request: PossessionRequest) -> HotkeyReport {
    let mut engine = lock(&ENGINE);
    let mut possession = lock(&POSSESSION);
    match engine.as_deref_mut() {
        Some(engine) => possession.on_hotkey(engine, source, request),
        None => possession.on_hotkey(&mut NullEngine, source, request),
    }
}

/// One frame. Drives the installed engine and reconciles the state machine with it.
///
/// The reconciliation is the point. An engine can let go on its own -- the possessed creature died
/// or despawned -- and without this the toggle would still read `Active`, so the player's next
/// press would spend itself "releasing" something that had already ended.
pub(crate) fn tick_engine() {
    let mut engine = lock(&ENGINE);
    let Some(engine) = engine.as_deref_mut() else {
        return;
    };
    engine.tick();
    if engine.is_active() {
        return;
    }
    let mut possession = lock(&POSSESSION);
    if possession.state == PossessionState::Active {
        possession.state = PossessionState::Idle;
    }
}

/// Release whatever is held, because the process is going away. See
/// [`PossessionEngine::shutdown`] for why this is not merely a release.
pub(crate) fn shutdown_engine() {
    if let Some(engine) = lock(&ENGINE).as_deref_mut() {
        engine.shutdown();
    }
    lock(&POSSESSION).state = PossessionState::Idle;
}

/// The state and press count, for the status line.
pub(crate) fn snapshot() -> (PossessionState, u64) {
    let possession = lock(&POSSESSION);
    (possession.state(), possession.presses())
}

/// A poisoned lock here means a previous holder panicked. The state inside is still a valid state,
/// and refusing to read it would disable the hotkey over a fault that already happened.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> PossessionRequest {
        PossessionRequest {
            target: TargetSettings::default(),
            mapping: MappingSettings::default(),
            buttons: ButtonSettings::default(),
        }
    }

    /// A fake engine, so the state machine is provable without a game.
    struct FakeEngine {
        answer: PossessionOutcome,
        neutral: bool,
        possessed: u32,
        released: u32,
    }

    impl FakeEngine {
        fn accepting() -> Self {
            Self {
                answer: PossessionOutcome::Accepted,
                neutral: true,
                possessed: 0,
                released: 0,
            }
        }
    }

    impl PossessionEngine for FakeEngine {
        fn possess(&mut self, _request: &PossessionRequest) -> PossessionOutcome {
            self.possessed += 1;
            self.answer.clone()
        }

        fn release(&mut self) -> PossessionOutcome {
            self.released += 1;
            PossessionOutcome::Accepted
        }

        fn accepts_reload(&self) -> bool {
            self.neutral
        }
    }

    /// Layer 1'S whole runtime CLAIM: the press is seen and the state flips. Nothing is possessed,
    /// and the report says exactly that rather than implying otherwise.
    #[test]
    fn with_no_engine_a_press_flips_to_requested_and_says_no_engine() {
        let mut possession = Possession::default();
        let report = possession.on_hotkey(&mut NullEngine, "keyboard", request());
        assert_eq!(report.from, PossessionState::Idle);
        assert_eq!(report.to, PossessionState::Requested);
        assert_eq!(report.outcome, PossessionOutcome::NoEngine);
        assert_eq!(possession.state(), PossessionState::Requested);
        assert_eq!(possession.presses(), 1);

        // ...and pressing again gets back out, so the state is a toggle and not a trap.
        let report = possession.on_hotkey(&mut NullEngine, "keyboard", request());
        assert_eq!(report.from, PossessionState::Requested);
        assert_eq!(report.to, PossessionState::Idle);
        assert_eq!(possession.presses(), 2);
    }

    #[test]
    fn an_accepting_engine_reaches_active_and_releases_back_to_idle() {
        let mut engine = FakeEngine::accepting();
        let mut possession = Possession::default();
        assert_eq!(
            possession.on_hotkey(&mut engine, "gamepad", request()).to,
            PossessionState::Active
        );
        assert_eq!(
            possession.on_hotkey(&mut engine, "gamepad", request()).to,
            PossessionState::Idle
        );
        assert_eq!(engine.possessed, 1);
        assert_eq!(engine.released, 1);
    }

    /// A refusal must not leave the mod believing it possessed something -- otherwise the next
    /// press "releases" a possession that never happened and the player presses twice for nothing.
    #[test]
    fn a_refusal_leaves_the_state_where_it_was() {
        let mut engine = FakeEngine {
            answer: PossessionOutcome::Refused("no target".to_owned()),
            ..FakeEngine::accepting()
        };
        let mut possession = Possession::default();
        let report = possession.on_hotkey(&mut engine, "keyboard", request());
        assert_eq!(report.to, PossessionState::Idle);
        assert_eq!(
            report.outcome,
            PossessionOutcome::Refused("no target".to_owned())
        );
        assert_eq!(possession.state(), PossessionState::Idle);

        // The very next press must still be a possess attempt, not a release.
        possession.on_hotkey(&mut engine, "keyboard", request());
        assert_eq!(engine.possessed, 2);
        assert_eq!(engine.released, 0);
    }

    /// The reload gate. Layer 1 always says yes; an engine mid-animation says no and the file
    /// poller holds off.
    #[test]
    fn the_reload_gate_defaults_to_open_and_an_engine_can_close_it() {
        assert!(NullEngine.accepts_reload());
        let mut engine = FakeEngine::accepting();
        assert!(engine.accepts_reload());
        engine.neutral = false;
        assert!(!engine.accepts_reload());
        // The process-wide gate with nothing installed is the layer-1 answer.
        assert!(accepts_reload());
        assert!(!engine_installed());
    }

    /// The log line has to carry every fact the runtime evidence needs: which binding fired, which
    /// device, both ends of the state move, the outcome, and what was requested.
    #[test]
    fn the_report_line_names_the_binding_the_move_and_the_request() {
        let mut possession = Possession::default();
        let report = possession.on_hotkey(&mut NullEngine, "keyboard", request());
        let line = report.line("F9");
        for fragment in [
            "key=F9",
            "source=keyboard",
            "state=idle->requested",
            "outcome=no-engine",
            "mode=lock_on",
            "model=context",
        ] {
            assert!(line.contains(fragment), "{fragment} missing from {line}");
        }
    }

    /// Two engines writing one `ChrIns` is not something to resolve by install order.
    #[test]
    fn a_second_engine_is_refused_rather_than_installed_over_the_first() {
        // Uses its own slot rather than the process-wide one, so the test does not depend on
        // whether another test in this binary installed something first.
        let slot: Mutex<Option<Box<dyn PossessionEngine>>> = Mutex::new(None);
        let install = |engine: Box<dyn PossessionEngine>| {
            let mut guard = lock(&slot);
            if guard.is_some() {
                return false;
            }
            *guard = Some(engine);
            true
        };
        assert!(install(Box::new(FakeEngine::accepting())));
        assert!(!install(Box::new(FakeEngine::accepting())));
    }

    /// An engine that let go on its own must not leave the toggle believing it is possessing --
    /// otherwise the next press is spent releasing something that already ended.
    #[test]
    fn an_engine_that_lets_go_on_its_own_is_reconciled_with_the_toggle() {
        struct SelfReleasing {
            live: bool,
        }
        impl PossessionEngine for SelfReleasing {
            fn possess(&mut self, _request: &PossessionRequest) -> PossessionOutcome {
                self.live = true;
                PossessionOutcome::Accepted
            }
            fn release(&mut self) -> PossessionOutcome {
                self.live = false;
                PossessionOutcome::Accepted
            }
            fn tick(&mut self) {
                // The creature died.
                self.live = false;
            }
            fn is_active(&self) -> bool {
                self.live
            }
        }
        let mut engine = SelfReleasing { live: false };
        let mut possession = Possession::default();
        assert_eq!(
            possession.on_hotkey(&mut engine, "keyboard", request()).to,
            PossessionState::Active
        );
        // One frame, and the engine has let go.
        engine.tick();
        assert!(!engine.is_active());
        // This is what `tick_engine` does with that fact.
        if !engine.is_active() && possession.state == PossessionState::Active {
            possession.state = PossessionState::Idle;
        }
        assert_eq!(possession.state(), PossessionState::Idle);
        // ...so the very next press possesses again rather than releasing nothing.
        possession.on_hotkey(&mut engine, "keyboard", request());
        assert!(engine.is_active());
    }

    /// The default `shutdown` must release, because an engine that forgot to override it would
    /// otherwise leave `ChrCtrl+0x3b0` armed in a process that is unloading this DLL.
    #[test]
    fn the_default_shutdown_releases() {
        let mut engine = FakeEngine::accepting();
        engine.shutdown();
        assert_eq!(engine.released, 1);
        // ...and the layer-1 engine's tick and liveness are the honest no-answer.
        NullEngine.tick();
        assert!(!NullEngine.is_active());
    }

    #[test]
    fn outcomes_describe_themselves_for_the_log() {
        assert_eq!(PossessionOutcome::Accepted.describe(), "accepted");
        assert_eq!(PossessionOutcome::NoEngine.describe(), "no-engine");
        assert_eq!(
            PossessionOutcome::Refused("cutscene".to_owned()).describe(),
            "refused(cutscene)"
        );
    }
}
