#!/usr/bin/env python3
"""Runtime proof for the b78-guard / fd4io stand-down (bd er-effects-rs-9jbe, PR #127).

THE DEFECT. `GameMan+0xb78` (CS::GameMan::GetRequestedSaveSlotLoad) has two owners. The b78 guard in
er-title-flow forces it to -1 every frame inside its active window, because a slot written there
IN-WORLD makes FUN_140afb970 spin RequestLoadSlot. The switch-reload fd4io machine parks the WARP
TARGET there across SUBMIT -> DRAIN -> COMMIT. On the user's PR-117 eval (save 150-Banon, switch #2,
cross-save) they raced: fd4io committed with b78 armed, the guard rewrote -1 ~650ms later, finalize
consumed BlockId 0xffffffff, and the world tore down with nothing armed -> black screen, mms=-1, a
defaulted level-9 character, loading bar frozen at 1/500.

THE GATE THIS DRIVER ENFORCES. Two independent things must both hold, and only the first proves the
fix ENGAGED:

  ENGAGEMENT  oracle_switch_b78_guard_standdowns > 0. The counter increments on every frame the
              guard would have forced b78=-1 and now does not. The softlock is timing-dependent and
              has been observed exactly once, so a clean switch on its own proves NOTHING about the
              fix -- it is non-regression evidence only. Only a non-zero stand-down count shows the
              new condition fired against a live fd4io overlap.

  IDENTITY    every switch reaches the world as the EXPECTED character (name + level read out of the
              source save offline, before the run). This is the black-screen detector: the observed
              failure produced a defaulted level-9 Vagabond and no map, so "player present" alone is
              not enough -- the wrong character IS the defect.

Fully agent-driven, no menu navigation and no simulated input: each switch is armed by writing the
product's own control files, the same code path the user's ProfileSelect click reaches
(poll_switch_slot_control_file -> switch_slot_arm_programmatic):
    er-effects-switch-save-file.txt   the target save FILE (Windows path) -- cross-save only
    er-effects-switch-slot.txt        the target slot, mtime-triggered -- this is "the menu click"

load1 is the game-dir er-effects.toml boot autoload, left untouched.

Per the loading-screen-portrait protocol, scripts/capture-er-window.py fires at the portrait moment
of every switch. The agent never reads those images; they are user evidence. Stop/continue decisions
come only from the RAM oracles above, telemetry-freeze detection, process exit, or the runtime cap.

Usage:
  python3 scripts/run-b78-guard-standdown-proof.py \
      --chain '0,/home/banon/projects/er-effects-rs/save-files/150-Banon/ER0000.sl2:0' \
      --label pr127
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import re
import select
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
GAME_DIR = Path(
    os.environ.get(
        "ER_GAME_DIR",
        str(Path.home() / ".local/share/Steam/steamapps/common/ELDEN RING/Game"),
    )
)
LAUNCHER = Path(os.environ.get("ER_LAUNCHER", str(Path.home() / "Elden/launch.sh")))
TELEMETRY = GAME_DIR / "er-effects-telemetry.json"
DEBUG_LOG = GAME_DIR / "er-effects-autoload-debug.log"
SWITCH_SLOT = GAME_DIR / "er-effects-switch-slot.txt"
SWITCH_SAVE = GAME_DIR / "er-effects-switch-save-file.txt"
BOOT_TOML = GAME_DIR / "er-effects.toml"
CAPTURE = REPO / "scripts/capture-er-window.py"
CAP_FILE = REPO / ".auto/runtime_timeout_cap_seconds"
SLOT_DUMPER = REPO / "scripts/dump-save-slots.py"

# Everything the verdict reasons about. The b78 keys come first because they are the point.
KEYS = [
    "oracle_switch_b78_guard_standdowns",
    "oracle_switch_reload_phase",
    "oracle_switch_reload_drain_waits",
    "oracle_switch_reload_committed",
    "oracle_switch_arm_count",
    "oracle_switch_teardown_count",
    "oracle_switch_deferred_count",
    "oracle_switch_last_slot",
    "oracle_switch_player_present",
    "oracle_switch_menu_job_present",
    "oracle_switch_slot_control_primed",
    "oracle_char_name",
    "oracle_char_level",
    "oracle_player_present",
    "oracle_now_loading",
    "oracle_saved_map_c30",
    "system_quit_quickload_phase",
    "system_quit_quickload_selected_slot",
    "system_quit_continue_confirm_fresh_deser_count",
]


class GameDirWatch:
    """inotify on the game dir: the DLL rewrites telemetry every frame, so a file event is the
    deterministic 'new state exists' primitive. Never sleep-as-synchronization."""

    IN_MODIFY = 0x00000002
    IN_CREATE = 0x00000100
    IN_NONBLOCK = 0o4000

    def __init__(self, directory: Path) -> None:
        self._libc = ctypes.CDLL("libc.so.6", use_errno=True)
        self.fd = self._libc.inotify_init1(self.IN_NONBLOCK)
        if self.fd < 0:
            raise OSError("inotify_init1 failed")
        if (
            self._libc.inotify_add_watch(
                self.fd, str(directory).encode(), self.IN_MODIFY | self.IN_CREATE
            )
            < 0
        ):
            raise OSError(f"inotify_add_watch failed on {directory}")

    def wait(self, budget_s: float) -> bool:
        ready, _, _ = select.select([self.fd], [], [], budget_s)
        if not ready:
            return False
        try:
            os.read(self.fd, 1 << 16)
        except BlockingIOError:
            pass
        return True

    def close(self) -> None:
        try:
            os.close(self.fd)
        except Exception:
            pass


def quiet_wait(budget_s: float) -> None:
    select.select([], [], [], budget_s)


def cap_seconds() -> int:
    """The canonical runtime cap. Single source of truth; fallback pinned to the same value."""
    try:
        return int(CAP_FILE.read_text().strip())
    except Exception:
        return 300


def log(msg: str) -> None:
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


def steam_running() -> bool:
    """Sanctioned helper only -- raw pgrep false-negatives here and is guard-blocked."""
    helper = REPO / "scripts/steam-running.sh"
    if not helper.exists():
        return True
    try:
        r = subprocess.run(
            ["bash", "-c", f'source "{helper}" && steam_running'], capture_output=True, timeout=15
        )
    except subprocess.TimeoutExpired:
        return False
    return r.returncode == 0


def er_pids() -> list[int]:
    out = []
    for d in Path("/proc").iterdir():
        if not d.name.isdigit():
            continue
        try:
            if "eldenring.exe" in (d / "comm").read_text():
                out.append(int(d.name))
        except Exception:
            pass
    return out


def kill_er() -> None:
    for pid in er_pids():
        try:
            os.kill(pid, signal.SIGTERM)
        except Exception:
            pass
    for _ in range(20):
        if not er_pids():
            return
        quiet_wait(0.5)
    for pid in er_pids():
        try:
            os.kill(pid, signal.SIGKILL)
        except Exception:
            pass


def read_telemetry() -> dict:
    try:
        return json.loads(TELEMETRY.read_text())
    except Exception:
        return {}


def snap(t: dict) -> dict:
    return {k: t.get(k) for k in KEYS}


def player_in_world(t: dict) -> bool:
    name = t.get("oracle_char_name") or ""
    return (
        t.get("oracle_player_present") is True
        and bool(name)
        and not name.startswith("�")
        and (t.get("oracle_char_level") or 0) > 0
    )


def capture(out: Path) -> None:
    try:
        subprocess.run([sys.executable, str(CAPTURE), str(out)], capture_output=True, timeout=25)
        log(f"screenshot attempt -> {out.name} (.txt beside it if it fail-closed)")
    except subprocess.TimeoutExpired:
        log("screenshot helper timed out (non-fatal)")


def win_path(linux_path: str) -> str:
    """Wine sees the Linux root as Z:. The DLL opens the override with std::fs under Proton, so the
    control file must carry a path the game can open (the boot TOML uses the same form)."""
    p = str(Path(linux_path).resolve())
    return "Z:" + p.replace("/", "\\")


def linux_path_from_win(w: str) -> str:
    w = w.strip().strip("'\"")
    if len(w) > 2 and w[1] == ":":
        w = w[2:]
    return w.replace("\\", "/")


SLOT_RE = re.compile(r"^slot (\d+): \[(\w+)\s*\] name='([^']*)'\s+level=(\d+)")


def slot_identity(save: str, slot: int) -> tuple[str, int]:
    """Read the EXPECTED character out of the source save OFFLINE, before the run. Deriving the
    expectation from the save (not from what the game reports) is what makes the identity check a
    real black-screen detector rather than a tautology."""
    r = subprocess.run(
        [sys.executable, str(SLOT_DUMPER), save], capture_output=True, text=True, timeout=30
    )
    for line in r.stdout.splitlines():
        m = SLOT_RE.match(line.strip())
        if m and int(m.group(1)) == slot:
            return m.group(3), int(m.group(4))
    raise SystemExit(f"could not read slot {slot} identity from {save}\n{r.stdout}\n{r.stderr}")


def boot_target() -> tuple[str, int]:
    """The boot autoload comes from the game-dir er-effects.toml, which this driver never edits."""
    save, slot = None, 0
    for line in BOOT_TOML.read_text().splitlines():
        if line.strip().startswith("save_file"):
            save = linux_path_from_win(line.split("=", 1)[1])
        elif line.strip().startswith("slot"):
            slot = int(line.split("=", 1)[1].strip())
    if not save:
        raise SystemExit(f"no save_file in {BOOT_TOML}")
    return save, slot


def parse_chain(spec: str, boot_save: str) -> list[dict]:
    """'0' = within-file switch to slot 0; '/path/ER0000.sl2:0' = cross-save switch."""
    out = []
    for item in [s for s in spec.split(",") if s.strip()]:
        item = item.strip()
        if ":" in item:
            path, slot = item.rsplit(":", 1)
            out.append({"save": str(Path(path).resolve()), "slot": int(slot), "cross": True})
        else:
            out.append({"save": boot_save, "slot": int(item), "cross": False})
    return out


def rotate_outputs(art: Path) -> None:
    """A previous run's telemetry reads as satisfied preconditions. Move it aside BEFORE launch."""
    for src in (TELEMETRY, DEBUG_LOG):
        if src.exists():
            try:
                shutil.move(str(src), str(art / (src.name + ".pre-run")))
            except Exception:
                pass


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--chain",
        required=True,
        help="comma-separated switch targets: 'SLOT' (within the boot save) or 'SAVE_PATH:SLOT' "
        "(cross-save). The defect reproduces on a cross-save switch #2.",
    )
    ap.add_argument("--label", default="b78")
    args = ap.parse_args()

    cap = cap_seconds()
    art = (
        REPO
        / "target/runtime-probe"
        / f"b78-standdown-{time.strftime('%Y%m%d-%H%M%S')}-{args.label}"
    )
    art.mkdir(parents=True, exist_ok=True)
    series_f = open(art / "b78-series.jsonl", "w")
    verdict: dict = {"label": args.label, "cap_s": cap}

    if not steam_running():
        log("FATAL: Steam is DOWN (sanctioned helper). Agent probe requires Steam.")
        return 2
    if er_pids():
        log("FATAL: an eldenring.exe is ALREADY running; refusing to disturb it.")
        return 2

    boot_save, boot_slot = boot_target()
    boot_name, boot_level = slot_identity(boot_save, boot_slot)
    chain = parse_chain(args.chain, boot_save)
    for step in chain:
        step["expect_name"], step["expect_level"] = slot_identity(step["save"], step["slot"])
    verdict["boot"] = {
        "save": boot_save,
        "slot": boot_slot,
        "expect_name": boot_name,
        "expect_level": boot_level,
    }
    verdict["chain"] = chain
    log(f"boot autoload: {boot_save} slot={boot_slot} expect='{boot_name}' lvl={boot_level}")
    for i, step in enumerate(chain, 1):
        kind = "CROSS-SAVE" if step["cross"] else "within-file"
        log(
            f"switch {i}: {kind} slot={step['slot']} expect='{step['expect_name']}' "
            f"lvl={step['expect_level']} ({step['save']})"
        )

    for f in (SWITCH_SLOT, SWITCH_SAVE):
        if f.exists():
            f.unlink()
    rotate_outputs(art)

    t0 = time.time()
    proc = subprocess.Popen(
        ["bash", str(LAUNCHER)],
        cwd=str(LAUNCHER.parent),
        stdout=open(art / "launcher.log", "w"),
        stderr=subprocess.STDOUT,
    )
    log(f"launched {LAUNCHER} pid={proc.pid} (product me3 profile; game-dir toml untouched)")
    watch = GameDirWatch(GAME_DIR)

    def teardown(reason: str) -> None:
        (art / "teardown.txt").write_text(
            f"reason={reason}\nat={time.strftime('%Y-%m-%dT%H:%M:%S%z')}\n"
            f"er_pids={er_pids()}\nlauncher_pid={proc.pid}\n"
        )
        for src in (TELEMETRY, DEBUG_LOG):
            if src.exists():
                try:
                    shutil.copyfile(src, art / src.name)
                except Exception:
                    pass
        kill_er()
        try:
            proc.terminate()
        except Exception:
            pass
        for f in (SWITCH_SLOT, SWITCH_SAVE):
            if f.exists():
                f.unlink()
        log(f"teardown complete ({reason})")

    def finish(rc: int) -> int:
        (art / "verdict.json").write_text(json.dumps(verdict, indent=1))
        log(f"verdict -> {art / 'verdict.json'}")
        return rc

    try:
        # ---- boot -> load1 -------------------------------------------------------------
        deadline = t0 + cap
        t: dict = {}
        while time.time() < deadline:
            watch.wait(2.0)
            t = read_telemetry()
            if player_in_world(t):
                break
            if proc.poll() is not None and not er_pids():
                verdict["result"] = "FAIL:game-exited-during-boot"
                teardown("boot-exit")
                return finish(1)
        if not player_in_world(t):
            verdict["result"] = "FAIL:boot-timeout"
            teardown("boot-timeout")
            return finish(1)
        log(
            f"LOAD1 in world after {time.time() - t0:.0f}s char='{t.get('oracle_char_name')}' "
            f"lvl={t.get('oracle_char_level')}"
        )
        # Settle so switch eligibility (PlayerIns present) is genuine, not a first-frame artifact.
        settle_until = time.time() + 8.0
        while time.time() < settle_until:
            watch.wait(1.0)
        boot_snap = snap(read_telemetry())
        verdict["load1"] = boot_snap
        verdict["load1_identity_ok"] = (
            boot_snap.get("oracle_char_name") == boot_name
            and boot_snap.get("oracle_char_level") == boot_level
        )
        (art / "load1-baseline.json").write_text(json.dumps(boot_snap, indent=1))

        # ---- the switch chain ----------------------------------------------------------
        switches = []
        all_ok = True
        for idx, step in enumerate(chain, start=1):
            sw: dict = {
                "index": idx,
                "slot": step["slot"],
                "cross_save": step["cross"],
                "save": step["save"],
                "expect_name": step["expect_name"],
                "expect_level": step["expect_level"],
            }
            base_s = snap(read_telemetry())
            # Order matters: the FILE override must be readable before the slot write triggers the
            # mtime poll, because own_load_read_sl2_bytes consults the override first.
            SWITCH_SAVE.write_text((win_path(step["save"]) + "\n") if step["cross"] else "")
            SWITCH_SLOT.write_text(f"{step['slot']}\n")
            log(
                f"[switch {idx}/{len(chain)}] armed: save_override="
                f"{win_path(step['save']) if step['cross'] else '(none, within-file)'} slot={step['slot']}"
            )
            switch_t0 = time.time()
            deadline = switch_t0 + cap
            window_seen_at = 0.0
            shot_window = False
            result = "FAIL:load-cap"
            last_mtime = TELEMETRY.stat().st_mtime if TELEMETRY.exists() else 0.0
            stall_since = time.time()
            max_standdowns = int(base_s.get("oracle_switch_b78_guard_standdowns") or 0)
            while time.time() < deadline:
                watch.wait(1.0)
                if TELEMETRY.exists():
                    m = TELEMETRY.stat().st_mtime
                    if m != last_mtime:
                        last_mtime = m
                        stall_since = time.time()
                # A frozen telemetry file means the game stopped ticking: the black-screen end
                # state, or a hang. Either way the run is over.
                if time.time() - stall_since > 60.0:
                    result = "FAIL:telemetry-frozen-60s"
                    break
                t = read_telemetry()
                if not t:
                    continue
                s = snap(t)
                s["t_s"] = round(time.time() - switch_t0, 1)
                s["switch_index"] = idx
                series_f.write(json.dumps(s) + "\n")
                series_f.flush()
                max_standdowns = max(
                    max_standdowns, int(t.get("oracle_switch_b78_guard_standdowns") or 0)
                )
                phase = t.get("system_quit_quickload_phase") or 0
                loading = t.get("oracle_now_loading") is True or (
                    t.get("oracle_player_present") is not True
                )
                in_window = phase >= 2 and loading
                if in_window and window_seen_at == 0.0:
                    window_seen_at = time.time()
                    log(f"[switch {idx}] load window OPEN at +{s['t_s']}s (phase={phase})")
                # Protocol: capture the loading-screen-portrait moment -- the exact view where the
                # USER can see the feature failing. Never delayed to make the artifact prettier.
                if in_window and not shot_window and time.time() - window_seen_at > 3.0:
                    capture(art / f"loading-screen-portrait-screenshot-sw{idx}.jpg")
                    shot_window = True
                if proc.poll() is not None and not er_pids():
                    result = "FAIL:game-exited-during-switch"
                    break
                if window_seen_at != 0.0 and player_in_world(t) and phase >= 2:
                    result = "LOADED"
                    break
            sw["result"] = result
            sw["load_seconds"] = round(time.time() - switch_t0, 1)

            # ---- handoff completion: the switch machine must actually FINISH, not just reach a
            # world. phase back to IDLE, the in-game menu job rebuilt, now_loading cleared.
            completed = False
            comp_deadline = time.time() + 45.0
            while result == "LOADED" and time.time() < comp_deadline:
                watch.wait(1.0)
                t = read_telemetry()
                if not t:
                    continue
                if (
                    (t.get("system_quit_quickload_phase") or 0) == 0
                    and t.get("oracle_switch_menu_job_present") == 1
                    and not t.get("oracle_now_loading")
                ):
                    completed = True
                    break
            final = snap(read_telemetry())
            sw["handoff_complete"] = completed
            sw["standdown_delta"] = max_standdowns - int(
                base_s.get("oracle_switch_b78_guard_standdowns") or 0
            )
            sw["standdowns_total"] = max_standdowns
            sw["reload_committed_delta"] = int(final.get("oracle_switch_reload_committed") or 0) - int(
                base_s.get("oracle_switch_reload_committed") or 0
            )
            sw["drain_waits"] = final.get("oracle_switch_reload_drain_waits")
            sw["reload_phase"] = final.get("oracle_switch_reload_phase")
            sw["char"] = final.get("oracle_char_name")
            sw["level"] = final.get("oracle_char_level")
            sw["map_c30"] = final.get("oracle_saved_map_c30")
            # THE BLACK-SCREEN DETECTOR. The observed failure landed in a defaulted level-9
            # character with no map, so "a player exists" is not the bar -- it must be the character
            # the source save actually holds at that slot.
            sw["identity_ok"] = (
                sw["char"] == step["expect_name"] and sw["level"] == step["expect_level"]
            )
            switches.append(sw)
            log(
                f"[switch {idx}] RESULT={result} handoff={completed} "
                f"char='{sw['char']}' lvl={sw['level']} (expect '{step['expect_name']}' "
                f"lvl={step['expect_level']}) identity_ok={sw['identity_ok']} "
                f"b78_standdowns +{sw['standdown_delta']} (total {sw['standdowns_total']}) "
                f"reload_phase={sw['reload_phase']} committed+{sw['reload_committed_delta']} "
                f"map={sw['map_c30']}"
            )
            if result != "LOADED" or not completed or not sw["identity_ok"]:
                all_ok = False
                break

        verdict["switches"] = switches
        verdict["result"] = switches[-1]["result"] if switches else "FAIL:no-switch-ran"
        settle_until = time.time() + 5.0
        while time.time() < settle_until:
            watch.wait(1.0)
        final_t = read_telemetry()
        verdict["final"] = snap(final_t)
        total_standdowns = max(
            int(final_t.get("oracle_switch_b78_guard_standdowns") or 0),
            max((s["standdowns_total"] for s in switches), default=0),
        )
        verdict["b78_standdowns_total"] = total_standdowns
        # The two gates are reported separately on purpose. no_regression alone is the weak claim
        # the ticket warns about; engagement is what makes the run product proof.
        verdict["engagement_pass"] = total_standdowns > 0
        verdict["no_regression_pass"] = all_ok and bool(switches)
        log(
            f"[verdict] b78_standdowns_total={total_standdowns} "
            f"engagement_pass={verdict['engagement_pass']} "
            f"no_regression_pass={verdict['no_regression_pass']}"
        )
        if not verdict["engagement_pass"]:
            log(
                "[verdict] standdowns == 0: the guard never stood down, so this run is "
                "NON-REGRESSION EVIDENCE ONLY and does not prove the fix engaged."
            )
        teardown(f"done:{verdict['result']}")
    finally:
        series_f.close()
        watch.close()
        (art / "verdict.json").write_text(json.dumps(verdict, indent=1))
        log(f"artifacts -> {art}")
    return 0 if (verdict.get("engagement_pass") and verdict.get("no_regression_pass")) else 1


if __name__ == "__main__":
    raise SystemExit(main())
