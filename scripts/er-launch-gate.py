#!/usr/bin/env python3
"""Refuse an Elden Ring launch whose changed code path has never been shown to execute.

WHY THIS EXISTS
---------------
2026-08-04: a fix for the load2 `warpRequested` clear shipped, the user's screen was taken for a
launch, and the fix did nothing. Not because it failed to build -- it built, the gate was green, and
the loaded DLL's md5 matched the build byte for byte. It did nothing because its release condition
was *unreachable by construction*: it disarmed when `requestCode` latched 2, and the very state it
was scoped to (the load2 park) is DEFINED by `requestCode` staying 1 forever. The previous run's
telemetry already said so -- `oracle_stepfinish_request_code = 1`, `ig_d8 == 1` in every sample --
and reading it cost nothing.

A compile proves a predicate is well-typed. It says nothing about whether the state it names ever
occurs. That is the gap this gate closes, and it closes it OFFLINE, against evidence that already
exists, before anyone's screen is taken.

WHAT IT CHECKS
--------------
1. STALENESS -- every built DLL is newer than the sources it was built from. A launch that validates
   a DLL older than the tree is measuring a build that no longer exists.
2. REACHABILITY -- every registered predicate relevant to the selected named probe scope was
   OBSERVED true in a recorded run. A relevant predicate nobody has ever seen fire becomes an
   explicit obligation; a relevant predicate contradicted by a current run blocks the launch.

Registering a predicate is the point of contact: when you write a new release/disarm/gate condition,
add it here with the feature it protects and the oracle field or log pattern that proves it fired.
Named scopes select whole features, never hand-written predicate exclusion lists, so a scope cannot
silently omit one predicate while retaining another predicate for the same feature.

USAGE
  python3 scripts/er-launch-gate.py                      # full-product gate (fail-closed default)
  python3 scripts/er-launch-gate.py --scope save-load-continue
  python3 scripts/er-launch-gate.py --run <dir>          # score a specific recorded run
  python3 scripts/er-launch-gate.py --selftest           # prove the gate itself works
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import re
import sys
import tempfile
from dataclasses import dataclass, field

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

TELEMETRY_NAME = "er-effects-telemetry.json"
# Every DLL log a predicate may name. Each product DLL writes its OWN file next to the
# executable, and reading only the first one makes the gate structurally blind to any predicate
# owned by another DLL -- it would score that predicate against a log its evidence can never
# appear in, and report NEVER OBSERVED forever. The legacy-converter census lives in
# er-invasion-warp-dll.log, so a gate that reads only the autoload log can never pass it.
DEBUG_LOG_NAMES = (
    "er-effects-autoload-debug.log",
    "er-invasion-warp-dll.log",
)

# Evidence a run produces OUTSIDE the game directory. Frida-gadget traces are driven from the
# host and write where they are told, which is deliberately not the repo and not next to the
# executable -- so without this the gate is blind to them exactly as it was blind to
# er-invasion-warp-dll.log, and any predicate they own would read NEVER OBSERVED forever.
# Missing files are skipped silently: absence is what "no run has looked yet" looks like.
EXTRA_EVIDENCE_PATHS = (
    "/tmp/claude-1000/-home-banon-projects-er-effects-rs/"
    "fdd5f467-bf36-402d-bbcd-6defe1f4d0b7/scratchpad/steam-matchmaking-trace.jsonl",
    "/tmp/claude-1000/-home-banon-projects-er-effects-rs/"
    "fdd5f467-bf36-402d-bbcd-6defe1f4d0b7/scratchpad/steam-vtable-trace.jsonl",
    "/tmp/claude-1000/-home-banon-projects-er-effects-rs/"
    "fdd5f467-bf36-402d-bbcd-6defe1f4d0b7/scratchpad/ersc-session-trace.jsonl",
)

def _extra_evidence_paths() -> tuple[str, ...]:
    """Host-side evidence files, indirected so the selftest can switch them off.

    Reading them directly meant the selftest's synthetic fixtures absorbed whatever the last LIVE
    run had written, so a genuine runtime contradiction could fail a test about fixture logic.
    A test that depends on the machine's current state is not a test.
    """
    return EXTRA_EVIDENCE_PATHS


# Where a live run drops its artifacts, and where this session archives them.
DEFAULT_RUN_DIRS = [
    os.path.expanduser("~/.local/share/Steam/steamapps/common/ELDEN RING/Game"),
]


@dataclass(frozen=True)
class Predicate:
    """A runtime condition some code path depends on, plus how to prove it has ever been true.

    `oracle_all` are telemetry fields that must ALL hold the given values in one recorded run.
    `log_any` are regexes of which at least one must match a line of that run's debug log.
    A predicate with neither cannot be proven and is rejected at registration time.

    `informative_if` is what separates "a run DISAGREED with this" from "no run has ever had an
    opinion". Without it the gate treats both as failure, and refuses the very launch that would
    produce the evidence -- so a brand-new code path can never be proven and the gate becomes an
    unconditional no, which is a gate nobody can use and everybody skips. A run counts against a
    predicate only when it got far enough to have an opinion: reached the reload, opened the map,
    whatever the predicate is about. A run that never got there is silent, not contradicting.

    Leaving it empty means "every recorded run has an opinion", which is right for predicates
    about states a run always reaches.
    """

    name: str
    why: str
    owner: str
    feature: str
    oracle_all: dict[str, object] = field(default_factory=dict)
    log_any: tuple[str, ...] = ()
    informative_if: tuple[str, ...] = ()
    informative_oracle: dict[str, object] = field(default_factory=dict)

    def is_informative(self, telemetry: dict, log_text: str) -> bool:
        """Whether this run reached the state the predicate is about.

        `informative_oracle` exists because a LOG regex is a poor precondition for a state the
        telemetry names exactly. `epoch [1-9]` matched somewhere in a combined multi-DLL log on a
        BOOT-ONLY run, so three reload predicates read that run as a contradiction and refused
        every launch -- the precondition has to be the field that actually says a reload
        happened, not a substring that can appear anywhere.
        """
        if not self.informative_if and not self.informative_oracle:
            return True
        for key, want in self.informative_oracle.items():
            if key not in telemetry or not _values_agree(telemetry[key], want):
                return False
        if self.informative_oracle and not self.informative_if:
            return True
        return any(re.search(pattern, log_text) for pattern in self.informative_if)

    def check(self, telemetry: dict, log_text: str) -> tuple[bool, str]:
        for key, want in self.oracle_all.items():
            if key not in telemetry:
                return False, f"telemetry has no field {key!r}"
            got = telemetry[key]
            if not _values_agree(got, want):
                return False, f"{key} = {got!r}, needed {want!r}"
        for pattern in self.log_any:
            if re.search(pattern, log_text):
                return True, f"log matched /{pattern}/"
        if self.log_any:
            return False, "no log line matched " + " or ".join(f"/{p}/" for p in self.log_any)
        return True, "all oracle fields agree"


def _values_agree(got: object, want: object) -> bool:
    """Compare a telemetry reading against a wanted value.

    `want` may be a callable predicate, which is how "any epoch >= 1" is expressed without
    hard-coding an epoch number that only happens to be right for one recording.
    """
    if callable(want):
        try:
            return bool(want(got))
        except Exception:
            return False
    if isinstance(want, bool) or isinstance(got, bool):
        return bool(got) == bool(want)
    return got == want


# --- the register -------------------------------------------------------------------------------
#
# THE LOAD2 WARP-CLEAR RELEASE. The clear zeroes GameMan+0x10 every frame of a map move at epoch >= 1
# and must stop once the load it protects has produced a playable world -- otherwise no warp, ours or
# vanilla's, can ever complete again. Its release predicate has to be a signal that transitions WHILE
# THE LOAD IS PARKED, because the park is the steady state: `oracle_stepfinish_mms_state` sits at 18
# and `oracle_stepfinish_request_code` at 1 indefinitely. Anything phrased as "the load finished" is
# therefore unreachable, which is exactly the bug this gate was written after.
PREDICATES: tuple[Predicate, ...] = (
    Predicate(
        name="warp_clear_window_opened",
        why=(
            "without this, the release predicate below is VACUOUS -- a run that never reloaded into "
            "the park satisfies it without ever exercising the code path"
        ),
        owner=(
            "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/system_quit_hooks.rs "
            "(maybe_force_finish_stuck_testnet_step)"
        ),
        feature="reload-system-quit",
        oracle_all={"oracle_current_load_epoch": lambda v: isinstance(v, int) and v >= 1},
        log_any=(r"cvar10-warp-clear: load2 epoch [1-9]\d* mms=1[3-8] fin=[0-4]",),
        # A boot-only run never reloaded, so it has nothing to say about a reload-time clear.
        # Keyed on the telemetry field rather than a log substring: `epoch [1-9]` matched inside
        # an unrelated DLL's log on an epoch-0 run and turned a silence into a refusal.
        informative_oracle={"oracle_current_load_epoch": lambda v: isinstance(v, int) and v >= 1},
    ),
    Predicate(
        name="case7_gate_clear_at_release",
        why=(
            "THE ASSERTION THAT WOULD HAVE CAUGHT THE FREEZE. Releasing GameMan+0x10 lets cVar10 "
            "reach 1, which fades to black and walks the finalize to substate 7; substate 7 advances "
            "only when GameMan+0xb72 and +0xb73 are clear. In the 2026-08-04 run both were latched "
            "and nothing could clear them, so the release would have parked the game on a black "
            "screen -- strictly worse than the silent no-op it replaced. Block the launch until a "
            "satisfier that can actually run is in the build."
        ),
        owner=(
            "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/system_quit_hooks.rs "
            "(case7-savedrain-satisfy) / crates/er-title-flow/src/title_tick_cover.rs "
            "(reload-drain-b80)"
        ),
        feature="reload-system-quit",
        # Satisfied either by the gate demonstrably not being blocked, or by a satisfier having been
        # OBSERVED to run at a reload epoch. Both are recorded facts, not predictions.
        oracle_all={"oracle_current_load_epoch": lambda v: isinstance(v, int) and v >= 1},
        log_any=(
            r"case7-savedrain-satisfy: epoch [1-9]",
            r"reload-drain-b80",
        ),
        # A boot-only run never reloaded, so it has nothing to say about a reload-time clear.
        # Keyed on the telemetry field rather than a log substring: `epoch [1-9]` matched inside
        # an unrelated DLL's log on an epoch-0 run and turned a silence into a refusal.
        informative_oracle={"oracle_current_load_epoch": lambda v: isinstance(v, int) and v >= 1},
    ),
    Predicate(
        name="warp_clear_release_world_live",
        why=(
            "the load2 warpRequested clear must disarm once this epoch's world clock is live, or "
            "every warp after the first reload stays dead"
        ),
        owner=(
            "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/system_quit_hooks.rs "
            "(maybe_force_finish_stuck_testnet_step)"
        ),
        feature="reload-system-quit",
        oracle_all={
            # The latch itself, and that it named a RELOAD epoch rather than the boot epoch --
            # epoch 0 is never touched by the clear, so a boot-only observation proves nothing.
            "oracle_play_time_live": True,
            "oracle_current_load_epoch": lambda v: isinstance(v, int) and v >= 1,
            "oracle_boot_view_epoch_live": lambda v: isinstance(v, int) and v >= 1,
            "oracle_player_present": True,
        },
        # A boot-only run never reloaded, so it has nothing to say about a reload-time clear.
        # Keyed on the telemetry field rather than a log substring: `epoch [1-9]` matched inside
        # an unrelated DLL's log on an epoch-0 run and turned a silence into a refusal.
        informative_oracle={"oracle_current_load_epoch": lambda v: isinstance(v, int) and v >= 1},
    ),
    Predicate(
        name="legacy_converter_tree_readable",
        why=(
            "the whole 'markers without visiting' feature rests on ONE unverified read: that the "
            "std::map at WorldMapLegacyConverter+0x08 walks to real entries. Every failure mode -- "
            "wrong offset, head-vs-root confusion, converter with no legacy table -- produces the "
            "SAME visible result as a working feature on a save that has already been everywhere: "
            "no new markers. Without this the launch cannot tell 'nothing to add' from 'the walk "
            "found nothing', which is exactly the ambiguity that cost a run on 2026-08-04."
        ),
        owner=(
            "crates/er-invasion-warp/src/legacy_map_regions.rs (walk_tree) / "
            "crates/er-invasion-warp-dll/src/map_hooks.rs (legacy_map_regions_for_view)"
        ),
        feature="world-map-markers",
        # A non-zero block count is the only reading that proves the walk reached real nodes.
        # Deliberately NOT satisfied by the marker count: a save that has visited every dungeon
        # legitimately yields zero markers while the walk is working perfectly.
        log_any=(
            r"map-inject: legacy-dungeon table: [1-9]\d* block\(s\) known to the world map",
        ),
        # Only a run that actually built a world-map ViewModel has an opinion on the tree walk.
        informative_if=(r"map-inject:",),
    ),
    # RETIRED: steam_matchmaking_reached. It was ANSWERED, in the negative, and a predicate whose
    # question is settled must not sit here refusing launches forever. Measured 2026-08-04: 33
    # steam_api64 flat exports hooked at BOOT -- 18 ISteamMatchmaking, 10 ISteamNetworking*, 5
    # ISteamFriends rich presence -- and a complete invasion produced ZERO calls. Seamless does
    # not reach Steam through the flat C API. That is why the predicate below exists instead.
    Predicate(
        name="steam_vtable_call_observed",
        why=(
            "THE ONLY REMAINING ROUTE TO SEAMLESS TARGETING. Static reading is out -- the "
            "deciding functions (ersc 0x18006a2d0, 0x18006a1e0) are Themida-VIRTUALIZED, proven "
            "by a live dump whose jump targets are themselves chains of `e9` jumps, so no dump "
            "recovers them. The flat Steam C API is out -- 33 exports hooked at boot, zero calls "
            "across a full invasion. What is left is the interface VTABLE: a C++ mod fetches the "
            "pointer once and calls slots directly. If a vtable call is never observed either, "
            "Steam is not the layer at all and the next one is ERSC's own sockets -- and a run "
            "that cannot tell those apart is a run wasted, which is what this gate is for."
        ),
        owner="scripts/frida-steam-vtable-trace.py",
        feature="seamless-session-tracing",
        # A vtable CALL, not merely an interface handed out. Capturing the pointer proves the
        # accessor fired; it says nothing about whether Seamless ever calls through it, and
        # conflating the two is the same "hooked is not called" error the retired predicate hit.
        log_any=(r'"type":\s*"vcall"',),
        # Only a run whose vtable trace produced SOMETHING has an opinion. An empty file means
        # the tracer never attached or attached too late, which is a silence, not a refutation.
        informative_if=(r'"type":\s*"(vcall|iface)"',),
    ),
    Predicate(
        name="steam_interface_version_resolved",
        why=(
            "WITHOUT THIS THE 10068 RECORDED VTABLE CALLS ARE ANONYMOUS. All 5 interfaces came "
            "back through SteamInternal_FindOrCreateUserInterface with version=None, because the "
            "capture used a prefix allowlist that matched nothing -- so every interface was "
            "labelled by its accessor and none could be told from another. Slot indices without "
            "an interface identity name no mechanism: slot[29] firing 6908 times is a transport, "
            "and the targeting call is one of the slots that fired twice. The filter is now any "
            "'<Name><3 digits>' string, and this run has to show it resolving."
        ),
        owner="scripts/frida-steam-vtable-trace.py",
        feature="seamless-session-tracing",
        # A non-null version on an iface record. NOT satisfied by vcalls: those were already
        # plentiful while every interface stayed unidentified, which is the exact failure here.
        # Satisfied by an interface being IDENTIFIABLE, which is the actual requirement -- either
        # the decoded version field, or the raw argument bytes carrying one. The bytes route is
        # not a loophole: 'Steam'/'STEAM' in hex at the head of an accessor argument decoded
        # offline to SteamUser021 and STEAMUSERSTATS_INTERFACE_VERSION, which answered the
        # question the field was only ever a convenience for. Testing for the FIELD when the
        # requirement is the ANSWER is how a gate refuses a launch over settled ground.
        log_any=(
            r'"version":\s*"[A-Za-z][A-Za-z0-9_]{4,40}\d{3}"',
            r'"bytes":\s*"5374 ?65 ?61 ?6d'.replace(' ', ''),
            r'"bytes":\s*"535445414d',
        ),
        informative_if=(r'"type":\s*"iface"',),
    ),
    Predicate(
        name="ersc_session_state_observed",
        why=(
            "THE LAST UNREAD LINK. What sets session state 0x15 -- the only state offering 'Seek "
            "opponent' -- and what consumes the opponent handle that option latches at S+0x1F0 "
            "both live inside the Themida-virtualized seamless_session_manager dispatcher and "
            "CANNOT be read. They can only be watched. If OSM is never captured, the tracer saw "
            "nothing and the run proves nothing about the state machine; that must not be "
            "mistaken for 'the state never changed'."
        ),
        owner="scripts/frida-ersc-session-trace.py",
        feature="seamless-session-tracing",
        # A real session reading. NOT satisfied by an 'osm' capture alone: capturing the object
        # proves a hook fired, not that S+0x110 was ever readable through it -- the same
        # hooked-is-not-called conflation that made two earlier predicates look promising.
        log_any=(r'"type":\s*"session"',),
        informative_if=(r'"type":\s*"(session|osm|menu-open|action)"',),
    ),
    Predicate(
        name="hunt_hook_lands_on_steamclient",
        why=(
            "THE ONE FACT ABOUT HUNT MODE NO OFFLINE WORK CAN ESTABLISH. Every other detour this "
            "repo installs targets the game image or ersc; hunt's goes onto vtable slot 4 of "
            "ISteamMatchmaking, which lives in steamclient64.dll -- a different module, loaded by "
            "Steam, with its own page protections. Whether the union dispatcher can take that "
            "target is answerable only in a live process. If it cannot, hunt is INERT: every query "
            "goes out unfiltered and the player sees a perfectly ordinary search, which is exactly "
            "what 'the filter is working and nobody is there' looks like. The DLL logs the failure "
            "and the oracle carries it, so a run that comes back with this false has told us "
            "something; a run that never looks has not."
        ),
        owner="crates/er-invasion-warp-dll/src/lobby_publish.rs (install_hunt_hook)",
        feature="invasion-hunt",
        oracle_all={"oracle_invasion_warp_hunt_hooked": True},
        log_any=(r"hunt: asking Steam for hosts at m\d\d_\d\d_\d\d_\d\d only",),
        # A run with hunt off, or one that never reached a lobby query, has no opinion about
        # whether the hook can land. Only a run where the DLL published its oracle document at all
        # counts -- absence of the field means the feature never got that far.
        informative_oracle={
            "oracle_invasion_warp_hunt_hooked": lambda v: isinstance(v, bool),
        },
    ),
    Predicate(
        name="hunt_filter_reaches_the_wire",
        why=(
            "Installing the detour is NOT the same as narrowing a query. `hunt_target` declines "
            "whenever hunt is off, the player's block is unreadable, or several locations are "
            "marked -- a Steam string filter is one equality test with no OR, so the multi-mark "
            "case refuses on purpose. All three declines leave a hooked, silent run that looks "
            "identical to a working one from outside. Only a non-zero filter count says Seamless's "
            "own outgoing search actually carried our key."
        ),
        owner="crates/er-invasion-warp-dll/src/lobby_publish.rs (request_lobby_list_hook)",
        feature="invasion-hunt",
        oracle_all={
            "oracle_invasion_warp_hunt_filters": lambda v: isinstance(v, int) and v >= 1
        },
        log_any=(r"hunt: asking Steam for hosts at m\d\d_\d\d_\d\d_\d\d only",),
        informative_oracle={
            "oracle_invasion_warp_hunt_hooked": lambda v: v is True,
        },
    ),
    Predicate(
        name="own_load_save_rejection_bounded",
        why=(
            "a save/load/Continue probe must publish the terminal-rejection guard and must never "
            "repeat an identical unresolvable staged-source decision; zero means either no "
            "rejection occurred or the first rejection stayed terminal instead of becoming the "
            "YK0J per-frame loop"
        ),
        owner=(
            "crates/er-effects-rs/src/experiments/save_redirect/path_hooks.rs "
            "(OWN_LOAD_SAVE_REJECTION) / experiments/own_load/drive.rs"
        ),
        feature="save-load-continue",
        oracle_all={
            "oracle_own_load_save_rejection_state": lambda v: isinstance(v, int) and v in (0, 1),
            "oracle_own_load_save_repeated_identical_rejections": 0,
        },
        # Old builds do not publish this field and therefore have no opinion. A run from the new
        # build does, including the ordinary state=0 path where no rejection was needed.
        informative_oracle={
            "oracle_own_load_save_rejection_state": lambda v: isinstance(v, int) and v in (0, 1),
        },
    ),
)


@dataclass(frozen=True)
class ProbeScope:
    """A named launch contract expressed as whole product features."""

    name: str
    features: frozenset[str] | None


FULL_PRODUCT_SCOPE = "full-product"
SAVE_LOAD_CONTINUE_SCOPE = "save-load-continue"
PROBE_SCOPES: dict[str, ProbeScope] = {
    # None means every registered predicate. This is deliberately the default and stays fail-closed
    # as new features and predicates are registered.
    FULL_PRODUCT_SCOPE: ProbeScope(FULL_PRODUCT_SCOPE, None),
    SAVE_LOAD_CONTINUE_SCOPE: ProbeScope(
        SAVE_LOAD_CONTINUE_SCOPE, frozenset({"save-load-continue"})
    ),
}
# Keep the minimum contract independent of the selector definition. If a future edit accidentally
# removes a required feature from a named scope, resolution fails rather than silently weakening it.
REQUIRED_SCOPE_FEATURES: dict[str, frozenset[str]] = {
    SAVE_LOAD_CONTINUE_SCOPE: frozenset({"save-load-continue"}),
}


def predicates_for_scope(
    scope_name: str,
    predicates: tuple[Predicate, ...] | None = None,
    scopes: dict[str, ProbeScope] | None = None,
) -> tuple[Predicate, ...]:
    """Resolve a named scope, rejecting typo, empty, and underdeclared contracts."""
    registered = PREDICATES if predicates is None else predicates
    available_scopes = PROBE_SCOPES if scopes is None else scopes
    if not scope_name or scope_name != scope_name.strip():
        raise ValueError("probe scope must be a non-empty exact name")
    scope = available_scopes.get(scope_name)
    if scope is None:
        known = ", ".join(sorted(available_scopes))
        raise ValueError(f"unknown probe scope {scope_name!r}; known scopes: {known}")
    if scope.name != scope_name:
        raise ValueError(
            f"scope registry key {scope_name!r} does not match declaration {scope.name!r}"
        )
    if scope.features is None:
        if scope.name != FULL_PRODUCT_SCOPE:
            raise ValueError(f"scope {scope.name!r} may not use the full-product wildcard")
        if not registered:
            raise ValueError("full-product scope has no registered predicates")
        return registered
    if not scope.features:
        raise ValueError(f"scope {scope.name!r} declares no product features")
    missing_required = REQUIRED_SCOPE_FEATURES.get(scope.name, frozenset()) - scope.features
    if missing_required:
        raise ValueError(
            f"scope {scope.name!r} is underdeclared; missing required feature(s): "
            + ", ".join(sorted(missing_required))
        )
    known_features = {predicate.feature for predicate in registered}
    unknown_features = scope.features - known_features
    if unknown_features:
        raise ValueError(
            f"scope {scope.name!r} declares feature(s) with no predicates: "
            + ", ".join(sorted(unknown_features))
        )
    selected = tuple(
        predicate for predicate in registered if predicate.feature in scope.features
    )
    if not selected:
        raise ValueError(f"scope {scope.name!r} selects no predicates")
    # Selection is feature-derived rather than a predicate allowlist. This assertion makes a future
    # edit that accidentally drops one same-feature predicate fail closed.
    expected_names = {
        predicate.name for predicate in registered if predicate.feature in scope.features
    }
    selected_names = {predicate.name for predicate in selected}
    if selected_names != expected_names:
        raise ValueError(f"scope {scope.name!r} is underdeclared")
    return selected


@dataclass
class RunEvidence:
    directory: str
    telemetry: dict
    log_text: str
    recorded_at: float = 0.0

    def predates(self, source_mtime: float) -> bool:
        """Whether this run was produced before the current sources existed.

        A run is evidence about the build that produced it, not about the tree as it stands now.
        After a fix, the recorded run still shows the OLD failure -- and scoring it as a
        contradiction refuses the launch that would prove the fix, permanently. That is the same
        "cannot tell a disagreement from a silence" defect this gate already corrects once; a
        stale run is a third category, and it is a silence.
        """
        return self.recorded_at < source_mtime


def load_run(directory: str) -> RunEvidence | None:
    telemetry_path = os.path.join(directory, TELEMETRY_NAME)
    if not os.path.exists(telemetry_path):
        return None
    try:
        with open(telemetry_path, encoding="utf-8", errors="replace") as handle:
            telemetry = json.load(handle)
    except (OSError, ValueError):
        return None
    chunks = []
    paths = [os.path.join(directory, name) for name in DEBUG_LOG_NAMES]
    paths.extend(_extra_evidence_paths())
    for path in paths:
        if not os.path.exists(path):
            continue
        try:
            with open(path, encoding="utf-8", errors="replace") as handle:
                chunks.append(handle.read())
        except OSError:
            continue
    try:
        recorded_at = os.path.getmtime(telemetry_path)
    except OSError:
        recorded_at = 0.0
    return RunEvidence(
        directory=directory,
        telemetry=telemetry,
        log_text="\n".join(chunks),
        recorded_at=recorded_at,
    )


def newest_source_mtime() -> tuple[float, str]:
    """Newest mtime across the Rust sources that end up in a game DLL."""
    newest = 0.0
    newest_path = ""
    for pattern in ("crates/**/*.rs", "crates/**/Cargo.toml", "Cargo.toml", "data/effects.json"):
        for path in glob.glob(os.path.join(REPO_ROOT, pattern), recursive=True):
            try:
                stamp = os.path.getmtime(path)
            except OSError:
                continue
            if stamp > newest:
                newest, newest_path = stamp, path
    return newest, newest_path


def stale_dlls() -> list[str]:
    """Report whether the tree has been edited since the last build.

    Deliberately NOT per-DLL-vs-newest-source: that flags `er_armament_icons.dll` for an edit to an
    unrelated crate, and a check that cries wolf is a check nobody reads. Without a dependency graph
    the honest question is the coarse one -- did ANY build happen after the last edit? If the newest
    artifact postdates the newest source, whatever the launch loads was built from this tree.
    """
    newest_src, newest_src_path = newest_source_mtime()
    if newest_src == 0.0:
        return []
    out_dir = os.path.join(REPO_ROOT, "target", "x86_64-pc-windows-msvc", "release")
    built = []
    for dll in glob.glob(os.path.join(out_dir, "*.dll")):
        # Only crates this repo builds; vendored/copied blobs carry unrelated mtimes.
        if not os.path.basename(dll).startswith(("er_", "mushroom")):
            continue
        try:
            built.append((os.path.getmtime(dll), os.path.basename(dll)))
        except OSError:
            continue
    if not built:
        return ["no built DLLs under target/x86_64-pc-windows-msvc/release"]
    newest_dll, newest_dll_name = max(built)
    if newest_dll < newest_src:
        rel = os.path.relpath(newest_src_path, REPO_ROOT)
        return [
            f"{rel} was edited after the newest build ({newest_dll_name}); "
            f"run `cargo xwin build --release --target x86_64-pc-windows-msvc` first"
        ]
    return []


def evaluate(
    runs: list[RunEvidence],
    source_mtime: float = 0.0,
    predicates: tuple[Predicate, ...] | None = None,
) -> tuple[bool, list[str], list[str]]:
    """Score every selected predicate against every recorded run.

    Returns `(ok, refusals, obligations)`. A predicate becomes a REFUSAL only when some run got
    far enough to have an opinion and disagreed -- that is a code path a run has actually shown
    cannot execute. A predicate no run has an opinion on is an OBLIGATION: the launch proceeds,
    and this is what it has to come back having shown.
    """
    selected = PREDICATES if predicates is None else predicates
    refusals: list[str] = []
    obligations: list[str] = []
    for predicate in selected:
        if not predicate.oracle_all and not predicate.log_any:
            refusals.append(f"{predicate.name}: registered with no evidence to check")
            continue
        proven = False
        contradiction = None
        for run in runs:
            ok, reason = predicate.check(run.telemetry, run.log_text)
            if ok:
                proven = True
                break
            if run.predates(source_mtime):
                # Produced by a build that no longer exists; says nothing about this tree.
                continue
            if predicate.is_informative(run.telemetry, run.log_text):
                contradiction = f"{os.path.basename(run.directory)}: {reason}"
        if proven:
            continue
        if contradiction is not None:
            refusals.append(
                f"{predicate.name}: A RUN REACHED THIS STATE AND IT WAS NOT TRUE ({contradiction})\n"
                f"      needed because {predicate.why}\n"
                f"      owner: {predicate.owner}"
            )
        else:
            obligations.append(
                f"{predicate.name}: no recorded run has reached this state, so this launch must "
                f"be the one that shows it\n"
                f"      needed because {predicate.why}\n"
                f"      owner: {predicate.owner}"
            )
    return (not refusals), refusals, obligations


def gate(run_dirs: list[str], scope_name: str = FULL_PRODUCT_SCOPE) -> int:
    try:
        selected = predicates_for_scope(scope_name)
    except ValueError as error:
        print(f"[launch-gate] REFUSED -- {error}", file=sys.stderr)
        return 1

    runs = [run for run in (load_run(d) for d in run_dirs) if run is not None]
    print(f"[launch-gate] probe scope: {scope_name} ({len(selected)} predicate(s))")
    print(f"[launch-gate] recorded runs found: {len(runs)}")
    for run in runs:
        print(f"[launch-gate]   {run.directory}")

    failures = []
    obligations: list[str] = []

    stale = stale_dlls()
    if stale:
        failures.append("stale build -- a launch would validate a DLL older than the tree:")
        failures.extend(f"      {item}" for item in stale)

    if not runs:
        failures.append(
            "no recorded run to check predicates against. Reachability cannot be established "
            "offline, so this launch cannot validate anything it claims to."
        )
    else:
        ok, problems, obligations = evaluate(
            runs, newest_source_mtime()[0], predicates=selected
        )
        if not ok:
            failures.append("unreachable predicate(s) -- the code path cannot execute:")
            failures.extend(f"      {item}" for item in problems)
        if obligations:
            print("[launch-gate] THIS RUN MUST PROVE:")
            for item in obligations:
                print(f"[launch-gate]   {item}")

    if failures:
        print("[launch-gate] REFUSED", file=sys.stderr)
        for line in failures:
            print(f"[launch-gate]   {line}", file=sys.stderr)
        print(
            "[launch-gate] A launch takes the user's screen. Prove the path executes first.",
            file=sys.stderr,
        )
        return 1

    proven = len(selected) - len(obligations) if runs else 0
    if obligations:
        print(
            f"[launch-gate] OK -- build is current; {proven}/{len(selected)} predicate(s) "
            f"already observed true, {len(obligations)} unproven and listed above. Nothing "
            f"CONTRADICTS them, so the launch proceeds -- but it is only worth taking the screen "
            f"if it comes back having shown them."
        )
    else:
        print(
            f"[launch-gate] OK -- {len(selected)} predicate(s) observed true, build is current"
        )
    return 0


def selftest() -> int:
    """The gate must FAIL on an unreachable predicate; a gate that only ever passes is decoration."""
    fails = 0
    # Fixtures only. See _extra_evidence_paths.
    globals()["_extra_evidence_paths"] = lambda: ()

    def report(ok: bool, label: str) -> None:
        nonlocal fails
        print(f"  {'ok  ' if ok else 'FAIL'} {label}")
        if not ok:
            fails += 1

    with tempfile.TemporaryDirectory() as tmp:
        # A run where the real predicate holds.
        good = os.path.join(tmp, "good")
        os.makedirs(good)
        with open(os.path.join(good, TELEMETRY_NAME), "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "oracle_play_time_live": True,
                    "oracle_current_load_epoch": 1,
                    "oracle_boot_view_epoch_live": 1,
                    "oracle_player_present": True,
                },
                handle,
            )
        with open(os.path.join(good, DEBUG_LOG_NAMES[0]), "w", encoding="utf-8") as handle:
            # The window actually opened, and a case-7 satisfier actually ran at a reload epoch --
            # the two facts that make the release predicate non-vacuous and non-fatal.
            handle.write(
                "[+182370ms] cvar10-warp-clear: load2 epoch 1 mms=13 fin=0 warpRequested was set\n"
                "[+199001ms] case7-savedrain-satisfy: epoch 1 world-live mms=18 fin=6\n"
                "[+201455ms] map-inject: legacy-dungeon table: 113 block(s) known to the world "
                "map's legacy converter -> 109 whole-dungeon marker(s) for dungeons not yet "
                "entered\n"
            )

        # The run that actually happened on 2026-08-04, where the SHIPPED predicate was
        # unreachable: mms parked at 18 and requestCode never left 1.
        parked = os.path.join(tmp, "parked")
        os.makedirs(parked)
        with open(os.path.join(parked, TELEMETRY_NAME), "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "oracle_stepfinish_mms_state": 18,
                    "oracle_stepfinish_request_code": 1,
                    "oracle_current_load_epoch": 1,
                    "oracle_play_time_live": True,
                    "oracle_boot_view_epoch_live": 1,
                    "oracle_player_present": True,
                },
                handle,
            )
        with open(os.path.join(parked, DEBUG_LOG_NAMES[0]), "w", encoding="utf-8") as handle:
            handle.write("cvar10-warp-clear: load2 epoch 1 mms=13 fin=0 warpRequested was set\n")

        # Exact YK0J dry-run shape: the save rejection is terminal and non-repeating, while the
        # latest general game-dir recording still contradicts unrelated reload predicates. The
        # focused gate must pass this; full-product must keep refusing it.
        yk0j = os.path.join(tmp, "yk0j-dry-run")
        os.makedirs(yk0j)
        with open(os.path.join(yk0j, TELEMETRY_NAME), "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "oracle_current_load_epoch": 1,
                    "oracle_play_time_live": False,
                    "oracle_boot_view_epoch_live": 0,
                    "oracle_player_present": False,
                    "oracle_own_load_save_rejection_state": 1,
                    "oracle_own_load_save_repeated_identical_rejections": 0,
                },
                handle,
            )
        with open(os.path.join(yk0j, DEBUG_LOG_NAMES[0]), "w", encoding="utf-8") as handle:
            handle.write(
                "cvar10-warp-clear: load2 epoch 1 mms=13 fin=0 warpRequested was set\n"
                "own-load: TERMINAL fail-closed rejection observation=First -- no retry\n"
            )

        good_run = load_run(good)
        parked_run = load_run(parked)
        yk0j_run = load_run(yk0j)
        report(
            good_run is not None and parked_run is not None and yk0j_run is not None,
            "recorded runs load",
        )

        ok, _, _ = evaluate([good_run])
        report(ok, "a run where the predicate held passes")

        focused = predicates_for_scope(SAVE_LOAD_CONTINUE_SCOPE)
        report(
            {predicate.name for predicate in focused}
            == {"own_load_save_rejection_bounded"},
            "save/load/Continue scope contains its complete relevant predicate set",
        )
        report(
            predicates_for_scope(FULL_PRODUCT_SCOPE) == PREDICATES,
            "default full-product scope contains every registered predicate",
        )
        focused_ok, focused_refusals, _ = evaluate([yk0j_run], predicates=focused)
        full_ok, full_refusals, _ = evaluate(
            [yk0j_run], predicates=predicates_for_scope(FULL_PRODUCT_SCOPE)
        )
        report(
            focused_ok and not focused_refusals,
            "exact YK0J dry-run fixture passes the save/load/Continue scope",
        )
        report(
            not full_ok
            and any("case7_gate_clear_at_release" in item for item in full_refusals)
            and any("warp_clear_release_world_live" in item for item in full_refusals),
            "exact YK0J fixture still fails closed under full-product scope",
        )
        for bad_scope in ("", "save-load-contine"):
            try:
                predicates_for_scope(bad_scope)
            except ValueError:
                rejected = True
            else:
                rejected = False
            report(rejected, f"unknown/empty scope {bad_scope!r} is refused")
        underdeclared = {
            SAVE_LOAD_CONTINUE_SCOPE: ProbeScope(
                SAVE_LOAD_CONTINUE_SCOPE, frozenset({"world-map-markers"})
            ),
        }
        try:
            predicates_for_scope(SAVE_LOAD_CONTINUE_SCOPE, scopes=underdeclared)
        except ValueError:
            rejected_underdeclared = True
        else:
            rejected_underdeclared = False
        report(rejected_underdeclared, "underdeclared named scope is refused")

        # THE REGRESSION THIS GATE EXISTS FOR: the terminator that shipped and could not fire.
        unreachable = Predicate(
            name="disarm_on_request_code_latched_done",
            why="the shipped-and-failed terminator: disarm when the world load latches requestCode 2",
            owner="system_quit_hooks.rs",
            feature="reload-system-quit",
            oracle_all={"oracle_stepfinish_request_code": 2},
        )
        proven, reason = unreachable.check(parked_run.telemetry, parked_run.log_text)
        report(
            not proven and "oracle_stepfinish_request_code" in reason,
            "the shipped unreachable terminator is REJECTED against the run that exposed it",
        )

        # An empty evidence set must not silently pass.
        empty = Predicate(name="no_evidence", why="x", owner="y", feature="test")
        ok_empty, problems, _ = evaluate([good_run])
        report(ok_empty, "register with evidence still passes")
        saved = globals()["PREDICATES"]
        try:
            globals()["PREDICATES"] = (empty,)
            ok_none, problems_none, _ = evaluate([good_run])
            report(
                not ok_none and any("no evidence" in p for p in problems_none),
                "a predicate registered with no evidence is refused",
            )
        finally:
            globals()["PREDICATES"] = saved

        # No runs at all must refuse, not pass by default. With nothing recorded no predicate can
        # be CONTRADICTED, so the refusal has to come from the gate's own no-evidence check
        # rather than from scoring -- which is exactly what `gate()` does.
        report(gate([]) != 0, "no recorded run refuses rather than passes")

        # THE STEAM-TRACE DISTINCTION: hooks INSTALLING is not calls HAPPENING. A trace that
        # attached to every export and then recorded nothing during an invasion means the
        # approach is dead, and it must not read as proof.
        steam_pred = [p for p in PREDICATES if p.name == "steam_vtable_call_observed"][0]
        called = '{"type": "vcall", "iface": "SteamMatchMaking009", "slot": 12}'
        ok_called, _ = steam_pred.check({}, called)
        report(ok_called, "a recorded vtable call proves the interface route")

        # THE DISTINCTION THAT RETIRED THE PREVIOUS PREDICATE: capturing an interface pointer
        # proves the accessor fired, NOT that anything is ever called through it.
        iface_only = '{"type": "iface", "version": "SteamMatchMaking009", "slotsHooked": 20}'
        ok_iface, _ = steam_pred.check({}, iface_only)
        report(
            not ok_iface and steam_pred.is_informative({}, iface_only),
            "an interface captured but never called is informative and NOT satisfied",
        )
        report(
            not steam_pred.is_informative({}, "nothing here"),
            "no vtable trace at all is a silence, not a contradiction",
        )

        # A run from BEFORE the current sources is a silence, not a disagreement. Without this
        # every bug fix is unprovable: the recorded run still shows the old failure, so the gate
        # refuses the launch that would demonstrate the fix, forever.
        stale_predicate = Predicate(
            name="fixed_since_that_run",
            why="a predicate whose code was corrected after the recorded run",
            owner="x",
            feature="test",
            log_any=(r"evidence-only-the-new-build-emits",),
        )
        saved_stale = globals()["PREDICATES"]
        try:
            globals()["PREDICATES"] = (stale_predicate,)
            future = good_run.recorded_at + 10_000
            ok_stale, refusals_stale, obligations_stale = evaluate([good_run], future)
            report(
                ok_stale and not refusals_stale and len(obligations_stale) == 1,
                "a run older than the sources is a silence, not a contradiction",
            )
            ok_fresh, refusals_fresh, _ = evaluate([good_run], 0.0)
            report(
                not ok_fresh and len(refusals_fresh) == 1,
                "a run newer than the sources still contradicts",
            )
        finally:
            globals()["PREDICATES"] = saved_stale

        # THE DISTINCTION THIS SPLIT EXISTS FOR. A gate that cannot tell "a run disagreed" from
        # "no run ever looked" refuses every launch on a new code path -- including the launch
        # that would produce the evidence -- so it becomes an unconditional no and gets skipped.
        never_looked = Predicate(
            name="state_no_run_reached",
            why="a brand-new path",
            owner="x",
            feature="test",
            log_any=(r"brand-new-marker",),
            informative_if=(r"a-line-no-run-has",),
        )
        looked_and_failed = Predicate(
            name="state_a_run_reached",
            why="a path a run actually exercised",
            owner="x",
            feature="test",
            log_any=(r"brand-new-marker",),
            # The good run's log DOES contain this, so that run has an opinion -- and disagrees.
            informative_if=(r"cvar10-warp-clear",),
        )
        saved = globals()["PREDICATES"]
        try:
            globals()["PREDICATES"] = (never_looked,)
            ok_new, refusals_new, obligations_new = evaluate([good_run])
            report(
                ok_new and not refusals_new and len(obligations_new) == 1,
                "a predicate no run has an opinion on is an obligation, not a refusal",
            )
            globals()["PREDICATES"] = (looked_and_failed,)
            ok_seen, refusals_seen, obligations_seen = evaluate([good_run])
            report(
                not ok_seen and len(refusals_seen) == 1 and not obligations_seen,
                "a predicate a run reached and disagreed with still refuses",
            )
        finally:
            globals()["PREDICATES"] = saved

    if fails:
        print(f"selftest FAILED ({fails})")
        return 1
    print("selftest ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", action="append", default=[], help="recorded run directory")
    parser.add_argument(
        "--scope",
        default=FULL_PRODUCT_SCOPE,
        help="named probe scope (default: full-product)",
    )
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    run_dirs = args.run or DEFAULT_RUN_DIRS
    return gate([d for d in run_dirs if os.path.isdir(d)], scope_name=args.scope)


if __name__ == "__main__":
    sys.exit(main())
