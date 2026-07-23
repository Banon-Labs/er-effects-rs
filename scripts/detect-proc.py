#!/usr/bin/env python3
"""Adaptive, fleet-portable process detector.

Replaces the fragile single-boundary `pgrep -x <name>` used in runtime preflights.
The problem it solves: this repo runs on several machines with DIFFERENT process
topologies, so no single command reliably answers "is Steam up?" / "is the game up?":

  * canonical Linux box   -- Steam + Proton game are native Linux procs; `pgrep` works.
  * WSL2 / nested-container box -- the shell lives in a sysbox Docker container whose PID
    namespace sees ~70 infra procs and NONE of Steam/the game; Steam is a *Windows*
    process (visible only via Windows interop `tasklist.exe`), and the Proton game runs
    in a Linux session in a different, unreachable namespace.

So a robust detector must probe EVERY boundary that exists on the current box and report
which one (if any) saw the target. This script does that with zero third-party deps:

  boundaries (each used only if available):
    - local-proc  : scan /proc/<pid>/comm + /cmdline directly (no pgrep, immune to the
                    repo's rtk/OPA grep guard which mangles bare grep/ps).
    - windows     : `tasklist.exe` (Windows process table) when WSL interop is present.
    - hyprland    : `hyprctl clients` window classes, when a Hyprland session is reachable
                    (lets us see the ER window `steam_app_1245620` even if the PID is not).

Usage:
    detect-proc.py                      # default targets: steam + the game
    detect-proc.py steam eldenring      # explicit targets (case-insensitive substring/regex)
    detect-proc.py --json steam
    detect-proc.py --list-boundaries    # show which boundaries this box exposes

Exit code = number of requested targets NOT found on any boundary (0 == all present).
Targets are matched as case-insensitive regexes against process/image/window names AND
full command lines, so `steam`, `eldenring`, `steam_app_1245620` all work.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys

# Default targets when the user names none. Keyed name -> regex. These are the two the
# runtime preflight actually cares about; the game regex matches the Proton exe, the me3
# launcher, and the Hyprland window class.
DEFAULT_TARGETS = {
    "steam": r"\bsteam(\.exe)?\b",
    "game": r"eldenring(\.exe)?|start_protected_game|steam_app_1245620|me3-launcher",
}


# Every boundary helper is a fast local OS query; a module-constant timeout keeps them bounded
# (and satisfies scripts/check-no-timeouts.py, which requires a literal/constant <=30s, not a
# variable). WSL interop (tasklist.exe/reg.exe) can be sluggish on a cold call, hence 10s not less.
_SUBPROCESS_TIMEOUT_SECONDS = 10.0


def _run(cmd: list[str]) -> str | None:
    """Run a helper command, returning decoded stdout or None on any failure.

    Never raises: a missing/blocked boundary must degrade to "not probed", not crash the
    whole detector, so the boundaries that DO work still answer.
    """
    try:
        out = subprocess.run(
            cmd,
            capture_output=True,
            timeout=_SUBPROCESS_TIMEOUT_SECONDS,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    # Windows tools emit UTF-16/CP-1252 with \r and stray NULs; decode leniently. This is
    # display/matching text from a trusted local OS tool, not untrusted content, and the
    # no-lossy-utf8 lint targets in-DLL parsing, so a tolerant decode here is appropriate.
    return out.stdout.decode("utf-8", "replace").replace("\x00", "")


# --------------------------------------------------------------------------- boundaries
# Each boundary is a function name -> callable that returns a list of (pid_or_none, name,
# detail) tuples describing everything it can see, or None if the boundary is unavailable
# on this box. Matching against the target regex happens once, centrally, below.


def boundary_local_proc() -> list[tuple[str | None, str, str]] | None:
    """Scan /proc directly. Present on any Linux; on a nested container it only sees the
    container namespace (still correct for that boundary)."""
    if not os.path.isdir("/proc"):
        return None
    rows: list[tuple[str | None, str, str]] = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        base = f"/proc/{entry}"
        comm = ""
        try:
            with open(f"{base}/comm", encoding="utf-8", errors="replace") as fh:
                comm = fh.read().strip()
        except OSError:
            continue
        # Use argv[0] (the actual executable path), NOT the full command line: matching a
        # generic word like "steam" against the whole cmdline catches any shell that merely
        # MENTIONS it (e.g. this detector's own invocation). argv[0] is the real binary, and
        # a path-anchored game exe (".../eldenring.exe") still matches; a wine-reparented
        # process whose argv[0] was rewritten is still caught by `comm` above.
        argv0 = comm
        try:
            with open(f"{base}/cmdline", "rb") as fh:
                raw = fh.read()
            first = raw.split(b"\x00", 1)[0]
            if first:
                argv0 = first.decode("utf-8", "replace").strip()
        except OSError:
            pass
        rows.append((entry, comm, argv0))
    return rows


def boundary_windows() -> list[tuple[str | None, str, str]] | None:
    """Windows process table via WSL interop `tasklist.exe`."""
    if shutil.which("tasklist.exe") is None:
        return None
    txt = _run(["tasklist.exe", "/fo", "csv", "/nh"])
    if txt is None:
        return None
    rows: list[tuple[str | None, str, str]] = []
    for line in txt.splitlines():
        line = line.strip()
        if not line:
            continue
        # CSV: "Image Name","PID","Session Name","Session#","Mem Usage"
        fields = [f.strip('"') for f in line.split('","')]
        if len(fields) < 2:
            continue
        name = fields[0].strip('"')
        pid = fields[1].strip('"')
        rows.append((pid, name, name))
    return rows


def boundary_hyprland() -> list[tuple[str | None, str, str]] | None:
    """Hyprland window list -- catches the ER window class even when the PID is unreachable."""
    if shutil.which("hyprctl") is None:
        return None
    txt = _run(["hyprctl", "-j", "clients"])
    if txt is None:
        return None
    try:
        clients = json.loads(txt)
    except (json.JSONDecodeError, ValueError):
        return None
    rows: list[tuple[str | None, str, str]] = []
    for c in clients:
        cls = str(c.get("class", ""))
        title = str(c.get("title", ""))
        pid = c.get("pid")
        rows.append((str(pid) if pid else None, cls, f"{cls} :: {title}"))
    return rows


BOUNDARIES = {
    "local-proc": boundary_local_proc,
    "windows": boundary_windows,
    "hyprland": boundary_hyprland,
}


# ----------------------------------------------------------------- steam readiness oracle
# "Is Steam up?" is the wrong question for a runtime preflight. The right question is: is
# there a Steam that is (1) running, (2) signed in, and (3) has the target game installed --
# because that is the Steam the game actually launches against. On a mixed box (Windows
# Steam + WSL) the answer must pick the READY one, not just any process named steam.

ER_APPID = "1245620"


def _winpath_to_wsl(winpath: str) -> str:
    """C:\\SteamLibrary -> /mnt/c/SteamLibrary (best-effort; only meaningful under WSL)."""
    m = re.match(r"^([A-Za-z]):[\\/](.*)$", winpath.replace("\\\\", "\\"))
    if not m:
        return winpath
    drive, rest = m.group(1).lower(), m.group(2).replace("\\", "/")
    return f"/mnt/{drive}/{rest}"


def _reg_query(path: str, value: str) -> str | None:
    if shutil.which("reg.exe") is None:
        return None
    txt = _run(["reg.exe", "query", path, "/v", value])
    if not txt:
        return None
    for line in txt.splitlines():
        line = line.strip()
        if line.lower().startswith(value.lower()):
            # e.g. "ActiveUser    REG_DWORD    0x18fa4be"
            return line.split()[-1]
    return None


def _acf_field(path: str, key: str) -> str | None:
    try:
        txt = open(path, encoding="utf-8", errors="replace").read()
    except OSError:
        return None
    m = re.search(r'"' + re.escape(key) + r'"\s+"([^"]*)"', txt)
    return m.group(1) if m else None


def _game_in_library(steamapps_dir: str, appid: str) -> dict | None:
    manifest = os.path.join(steamapps_dir, f"appmanifest_{appid}.acf")
    if not os.path.isfile(manifest):
        return None
    state = _acf_field(manifest, "StateFlags")
    return {
        "manifest": manifest,
        "name": _acf_field(manifest, "name"),
        "state_flags": state,
        # StateFlags 4 == fully installed (bit 2). Anything else = partial/updating.
        "fully_installed": state == "4",
        "installdir": _acf_field(manifest, "installdir"),
    }


def _library_roots_from_vdf(vdf_path: str) -> list[str]:
    try:
        txt = open(vdf_path, encoding="utf-8", errors="replace").read()
    except OSError:
        return []
    return re.findall(r'"path"\s+"([^"]+)"', txt)


def steam_readiness_windows(appid: str) -> dict | None:
    """Windows Steam readiness via interop (reg.exe + tasklist + library scan)."""
    if shutil.which("reg.exe") is None and shutil.which("tasklist.exe") is None:
        return None
    running = False
    win = boundary_windows()
    if win:
        running = any(re.search(r"steam(\.exe)?$", n, re.IGNORECASE) for _, n, _ in win)
    active_user = _reg_query(r"HKCU\Software\Valve\Steam\ActiveProcess", "ActiveUser")
    signed_in = active_user is not None and active_user not in ("0x0", "0")
    steam_path_win = _reg_query(r"HKCU\Software\Valve\Steam", "SteamPath")
    game = None
    if steam_path_win:
        primary = _winpath_to_wsl(steam_path_win)
        roots = [primary]
        roots += [_winpath_to_wsl(r) for r in
                  _library_roots_from_vdf(os.path.join(primary, "steamapps", "libraryfolders.vdf"))]
        seen = set()
        for root in roots:
            if root in seen:
                continue
            seen.add(root)
            hit = _game_in_library(os.path.join(root, "steamapps"), appid)
            if hit:
                game = hit
                break
    return {
        "platform": "windows",
        "running": running,
        "signed_in": signed_in,
        "active_user": active_user,
        "game_installed": bool(game and game["fully_installed"]),
        "game": game,
    }


def steam_readiness_linux(appid: str) -> dict | None:
    """Native-Linux Steam readiness (proc + local Steam config + library scan)."""
    steam_roots = [
        os.path.expanduser("~/.local/share/Steam"),
        os.path.expanduser("~/.steam/steam"),
        os.path.expanduser("~/.var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ]
    steam_roots = [r for r in steam_roots if os.path.isdir(r)]
    if not steam_roots:
        return None
    rows = boundary_local_proc() or []
    running = any(re.search(r"(^|/)steam$", n) or re.search(r"(^|/)steam$", d)
                  for _, n, d in rows)
    # Signed in: loginusers.vdf with a MostRecent "1" entry.
    signed_in = False
    for root in steam_roots:
        vdf = os.path.join(root, "config", "loginusers.vdf")
        try:
            txt = open(vdf, encoding="utf-8", errors="replace").read()
        except OSError:
            continue
        if re.search(r'"MostRecent"\s+"1"', txt):
            signed_in = True
            break
    game = None
    for root in steam_roots:
        roots = [root] + _library_roots_from_vdf(
            os.path.join(root, "steamapps", "libraryfolders.vdf"))
        for lib in roots:
            hit = _game_in_library(os.path.join(lib, "steamapps"), appid)
            if hit:
                game = hit
                break
        if game:
            break
    return {
        "platform": "linux",
        "running": running,
        "signed_in": signed_in,
        "game_installed": bool(game and game["fully_installed"]),
        "game": game,
    }


def steam_readiness(appid: str) -> list[dict]:
    """All reachable Steam installs on this box, each with running/signed-in/installed."""
    out = []
    for fn in (steam_readiness_windows, steam_readiness_linux):
        r = fn(appid)
        if r is not None:
            out.append(r)
    return out


def gather() -> dict[str, list[tuple[str | None, str, str]] | None]:
    return {name: fn() for name, fn in BOUNDARIES.items()}


def match_target(regex: re.Pattern[str], rows: list[tuple[str | None, str, str]]):
    hits = []
    for pid, name, detail in rows:
        if regex.search(name) or regex.search(detail):
            hits.append((pid, name, detail))
    return hits


def main() -> int:
    ap = argparse.ArgumentParser(description="Adaptive cross-boundary process detector.")
    ap.add_argument("targets", nargs="*", help="regex(es) to find; default: steam + game")
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    ap.add_argument("--list-boundaries", action="store_true",
                    help="show which boundaries are available on this box and exit")
    ap.add_argument("--steam-ready", nargs="?", const=ER_APPID, metavar="APPID",
                    help="report Steam readiness (running+signed-in+game-installed); "
                         f"APPID defaults to {ER_APPID} (Elden Ring). Exit 0 iff a ready "
                         "Steam exists.")
    args = ap.parse_args()

    if args.steam_ready is not None:
        installs = steam_readiness(args.steam_ready)
        ready = [i for i in installs
                 if i["running"] and i["signed_in"] and i["game_installed"]]
        if args.json:
            print(json.dumps({"appid": args.steam_ready, "installs": installs,
                              "ready": bool(ready)}, indent=2))
            return 0 if ready else 1
        if not installs:
            print(f"[DOWN] no Steam install reachable on this box (appid {args.steam_ready})")
            return 1
        for i in installs:
            flags = []
            flags.append("running" if i["running"] else "NOT-running")
            flags.append("signed-in" if i["signed_in"] else "NOT-signed-in")
            g = i.get("game")
            if g and i["game_installed"]:
                flags.append(f"game-installed ({g['name']})")
            elif g:
                flags.append(f"game-PARTIAL (StateFlags={g['state_flags']})")
            else:
                flags.append("game-NOT-installed")
            verdict = "READY" if (i["running"] and i["signed_in"] and i["game_installed"]) else "not-ready"
            print(f"[{verdict:>9}] {i['platform']:<8} {', '.join(flags)}")
            if g and g.get("manifest"):
                print(f"            manifest: {g['manifest']}")
        return 0 if ready else 1

    snapshot = gather()

    if args.list_boundaries:
        avail = {b: (rows is not None) for b, rows in snapshot.items()}
        counts = {b: (len(rows) if rows is not None else None) for b, rows in snapshot.items()}
        if args.json:
            print(json.dumps({"available": avail, "counts": counts}, indent=2))
        else:
            for b in BOUNDARIES:
                state = f"visible ({counts[b]} entries)" if avail[b] else "unavailable"
                print(f"  {b:<12} {state}")
        return 0

    if args.targets:
        targets = {t: t for t in args.targets}
    else:
        targets = DEFAULT_TARGETS

    result: dict[str, dict] = {}
    missing = 0
    for label, pattern in targets.items():
        regex = re.compile(pattern, re.IGNORECASE)
        found: dict[str, list] = {}
        for bname, rows in snapshot.items():
            if rows is None:
                continue
            hits = match_target(regex, rows)
            if hits:
                found[bname] = hits
        if not found:
            missing += 1
        result[label] = {"pattern": pattern, "found": found}

    if args.json:
        print(json.dumps(result, indent=2))
        return missing

    for label, info in result.items():
        found = info["found"]
        if not found:
            probed = [b for b, r in snapshot.items() if r is not None]
            print(f"[DOWN] {label:<8} not found (probed: {', '.join(probed) or 'none'})")
            continue
        boundaries = ", ".join(found)
        print(f"[ UP ] {label:<8} via {boundaries}")
        for bname, hits in found.items():
            for pid, name, _ in hits[:6]:
                pids = f"pid {pid}" if pid else "no-pid"
                print(f"         {bname}: {name} ({pids})")
            if len(hits) > 6:
                print(f"         {bname}: (+{len(hits) - 6} more)")
    return missing


if __name__ == "__main__":
    sys.exit(main())
