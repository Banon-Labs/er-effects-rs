#!/usr/bin/env python3
"""Launch Elden Ring live via me3 with the freshly-built repo DLL, for manual inspection.

HOST LAYOUT IS DETECTED, NEVER ASSUMED (2026-08-11):
    This script used to hard-code a WSL2 host: a Windows `me3.exe` under one specific user's
    `/mnt/c/Users/<name>/AppData/...`, `wslpath -w` to translate the DLL path, and
    `tasklist.exe` for the Steam check. On a native Linux box every one of those is absent and
    the script died with `FileNotFoundError: 'wslpath'` before launching anything. Both layouts
    are now resolved at runtime:

    native Linux (Steam + Proton)   me3 is a NATIVE LINUX binary (PATH, or `ME3_BIN`). It runs
                                    the game through the Steam compat tool, so it wants
                                    `--steam-dir` and an explicit `-e <game exe>`. The DLL path
                                    is passed AS IS -- there is nothing to translate.
    WSL2 + Windows Steam            me3.exe is a NATIVE WINDOWS process started from WSL. It
                                    loads this repo's build output IN PLACE from the Windows
                                    spelling of the WSL path (`wslpath -w`, e.g.
                                    `\\\\wsl.localhost\\<distro>\\home\\...\\er_effects_rs.dll`).
                                    Windows LoadLibraryW over the WSL filesystem was verified
                                    reliable (5/5), so the old "must copy to a C:\\ tree first"
                                    belief does not hold. (The real UNC hazard is log WRITES from
                                    the game, not the DLL load; the DLL's debug log already lands
                                    in the game dir, a Windows path.) One build, referenced where
                                    cargo puts it.

Every machine-specific value is env-overridable with a discovered default: `ME3_BIN`,
`ME3_STEAM_DIR`, `GAME_EXE`, `ME3_TMPDIR`, `ME3_PROFILE`. `~/Elden/launch.sh` (the user's
canonical launcher) uses the same names, so an override that works there works here.

CWD IS LOAD-BEARING, not cosmetic: me3 resolves `me3-launcher.exe`/`me3_mod_host.dll` from a
CWD-relative `target/x86_64-pc-windows-msvc/release` directory when one exists, so launching
from a Rust checkout makes Proton exec a nonexistent/stale launcher and the run dies silently
inside the compat tool (bd me3-launch-cwd-must-lack-rust-target-dir). We cd to the game dir,
and refuse outright if the effective CWD carries that trap.

No-teardown stdin trick (INTENTIONAL, do not "fix"):
    me3 tears the game down when its own stdin hits EOF. We pass `stdin=PIPE` and NEVER
    close it, so me3's stdin never EOFs and me3 stays alive as the monitor with the game
    running. There is no taskkill and no runtime cap here: `p.wait()` returns only when
    the USER closes the game. This is a manual-inspection helper, not an autoresearch probe.

Note: the real DLL debug log is `er-effects-autoload-debug.log` in the game directory,
not anything this script writes. We let me3's stdout/stderr inherit to the console so
launch errors stay visible.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

# --- Repo build output (derived from THIS script's location) -----------------------------
REPO_ROOT = Path(__file__).resolve().parents[1]
BUILT_DLL = REPO_ROOT / "target" / "x86_64-pc-windows-msvc" / "release" / "er_effects_rs.dll"
STEAM_HELPER = REPO_ROOT / "scripts" / "steam-running.sh"
DETECT_PROC = REPO_ROOT / "scripts" / "detect-proc.py"

# me3's payload-resolution trap: any CWD containing this relative directory hijacks the
# launcher/mod-host lookup (see the module docstring).
ME3_CWD_HIJACK_DIR = Path("target") / "x86_64-pc-windows-msvc" / "release"

# Steam roots to probe when ME3_STEAM_DIR is unset, in preference order. The Elden Ring app
# manifest is the discriminator: a box can have several Steam roots and only one owns the game.
STEAM_ROOT_CANDIDATES = (
    "~/.local/share/Steam",
    "~/.steam/steam",
    "~/.var/app/com.valvesoftware.Steam/.local/share/Steam",
)
ER_APPMANIFEST_RELPATH = Path("steamapps") / "appmanifest_1245620.acf"
ER_GAME_EXE_RELPATH = Path("steamapps") / "common" / "ELDEN RING" / "Game" / "eldenring.exe"

# me3 install locations to fall back to when it is not on PATH. Current-user aware; the WSL
# entry globs the Windows user directories instead of naming one user's home.
NATIVE_ME3_FALLBACKS = ("~/.local/bin/me3",)
WSL_ME3_GLOB_ROOT = Path("/mnt/c/Users")
WSL_ME3_GLOB = "*/AppData/Local/garyttierney/me3/bin/me3.exe"

CARGO_BUILD_CMD = [
    "cargo",
    "xwin",
    "build",
    "--release",
    "--target",
    "x86_64-pc-windows-msvc",
]

# Bounded cap for every helper subprocess (module constant, <= the repo's 30s hard cap so
# scripts/check-no-timeouts.py is satisfied by a constant rather than a variable). These are
# local OS/queries only; WSL interop can be sluggish on a cold call, hence 15s not less.
_HELPER_TIMEOUT_SECONDS = 15.0
# A cold `cargo xwin build` can exceed the repo's 30s subprocess cap; that is handled by the
# TimeoutExpired branch in run_build(), which tells the caller to build separately.
_BUILD_TIMEOUT_SECONDS = 30.0


def md5_prefix(path: Path) -> str:
    """Return the first 8 hex chars of the file's md5 (identifies WHICH dll is loaded)."""
    h = hashlib.md5()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()[:8]


def is_wsl() -> bool:
    """True when this shell is WSL (Windows interop reachable), false on a native Linux box.

    Two independent signals, because either alone can be wrong: `wslpath` is the tool we would
    actually call (its presence is the operative fact), and `/proc/version` carries the
    "microsoft" kernel tag even in an environment where interop tools are missing from PATH.
    """
    if shutil.which("wslpath") is not None:
        return True
    try:
        return "microsoft" in Path("/proc/version").read_text(encoding="utf-8", errors="replace").lower()
    except OSError:
        return False


def resolve_me3_binary(wsl: bool) -> str:
    """Locate the me3 executable for THIS host: env override, then PATH, then known installs."""
    override = os.environ.get("ME3_BIN")
    if override:
        found = shutil.which(override)
        if found:
            return found
        candidate = Path(override).expanduser()
        if candidate.is_file():
            return str(candidate)
        # An explicit override that cannot be resolved is a configuration error, never a reason
        # to silently launch some other me3 than the one that was asked for.
        sys.exit(f"error: ME3_BIN={override!r} is not an executable me3 binary on this box")

    for name in (("me3.exe", "me3") if wsl else ("me3",)):
        found = shutil.which(name)
        if found:
            return found

    for fallback in NATIVE_ME3_FALLBACKS:
        candidate = Path(fallback).expanduser()
        if candidate.is_file():
            return str(candidate)
    if wsl and WSL_ME3_GLOB_ROOT.is_dir():
        for candidate in sorted(WSL_ME3_GLOB_ROOT.glob(WSL_ME3_GLOB)):
            if candidate.is_file():
                return str(candidate)

    sys.exit(
        "error: no me3 binary found (PATH: me3"
        + (", me3.exe" if wsl else "")
        + "; fallbacks: "
        + ", ".join(NATIVE_ME3_FALLBACKS)
        + (f", {WSL_ME3_GLOB_ROOT}/{WSL_ME3_GLOB}" if wsl else "")
        + ").\nInstall me3 or set ME3_BIN to its path."
    )


def resolve_steam_dir() -> Path | None:
    """Steam root to hand me3 via --steam-dir, or None when this host has no Linux Steam root.

    On WSL the Steam install is a Windows one that me3.exe discovers itself, so returning None
    (and omitting --steam-dir) is the correct answer there, not a failure.
    """
    override = os.environ.get("ME3_STEAM_DIR")
    if override:
        return Path(override).expanduser()
    roots = [Path(candidate).expanduser() for candidate in STEAM_ROOT_CANDIDATES]
    for root in roots:
        if (root / ER_APPMANIFEST_RELPATH).is_file():
            return root
    for root in roots:
        if root.is_dir():
            return root
    return None


def resolve_game_exe(steam_dir: Path | None) -> Path | None:
    """Explicit game executable for `me3 -e`, or None to let me3 resolve the game itself."""
    override = os.environ.get("GAME_EXE")
    if override:
        return Path(override).expanduser()
    if steam_dir is not None:
        candidate = steam_dir / ER_GAME_EXE_RELPATH
        if candidate.is_file():
            return candidate
    return None


def loader_dll_path(dll: Path, wsl: bool, me3_bin: str) -> str:
    """Spell the DLL path for whichever me3 we are driving.

    Native Linux me3 takes the path as is. A Windows me3.exe driven from WSL needs the Windows
    spelling of the WSL path (`wslpath -w`), which is the ONLY reason path translation exists
    here -- so it is keyed on actually driving a Windows binary, not on the host tag alone. A
    WSL kernel with no `wslpath` on PATH is the exact shape that used to crash this script with
    FileNotFoundError; say what is missing instead.
    """
    if not (wsl and me3_bin.lower().endswith(".exe")):
        return str(dll)
    if shutil.which("wslpath") is None:
        sys.exit(
            f"error: {me3_bin} is a Windows me3 and needs the Windows spelling of {dll}, but "
            "`wslpath` is not on PATH. Install the WSL interop tools, or set ME3_BIN to a "
            "native Linux me3."
        )
    return subprocess.run(
        ["wslpath", "-w", str(dll)],
        capture_output=True,
        text=True,
        check=True,
        timeout=_HELPER_TIMEOUT_SECONDS,
    ).stdout.strip()


def steam_running() -> tuple[bool, str]:
    """Is Steam up? Returns (running, evidence). Layered, and never a raw `pgrep -x steam`.

    `pgrep -x steam` is a false negative on any box where Steam is not a native Linux process
    (WSL2 + Windows Steam), and the repo's Cupcake policy blocks it outright. So ask the repo's
    own detectors, in descending order of information:
      1. scripts/detect-proc.py --steam-ready -- probes /proc directly, the Windows process
         table via tasklist.exe, and Hyprland, then reports running/signed-in/game-installed
         per reachable Steam install. Built expressly to replace the single-boundary pgrep.
      2. scripts/steam-running.sh -- the sanctioned shell helper (AGENTS.md), which checks the
         Linux process AND the Windows one.
    """
    if DETECT_PROC.is_file():
        try:
            run = subprocess.run(
                [sys.executable, str(DETECT_PROC), "--steam-ready", "--json"],
                capture_output=True,
                text=True,
                timeout=_HELPER_TIMEOUT_SECONDS,
                check=False,
            )
            data = json.loads(run.stdout)
        except (OSError, subprocess.SubprocessError, json.JSONDecodeError, ValueError):
            data = None
        if isinstance(data, dict):
            installs = [i for i in data.get("installs", []) if isinstance(i, dict)]
            live = [i for i in installs if i.get("running")]
            if live:
                detail = ", ".join(
                    f"{i.get('platform', '?')}("
                    f"{'signed-in' if i.get('signed_in') else 'NOT-signed-in'}, "
                    f"{'game-installed' if i.get('game_installed') else 'game-NOT-installed'})"
                    for i in live
                )
                return True, f"detect-proc.py: {detail}"

    if STEAM_HELPER.is_file():
        try:
            run = subprocess.run(
                ["bash", str(STEAM_HELPER)],
                capture_output=True,
                text=True,
                timeout=_HELPER_TIMEOUT_SECONDS,
                check=False,
            )
        except (OSError, subprocess.SubprocessError):
            run = None
        if run is not None and run.returncode == 0:
            return True, "scripts/steam-running.sh"

    return False, "detect-proc.py and scripts/steam-running.sh both report no Steam"


def run_build() -> None:
    """Run the windows-target cargo build from the repo root; abort the script on failure.

    Repo policy caps subprocess timeouts at 30s, so a cold build can exceed it and raise
    TimeoutExpired -- in that case just build separately (`cargo xwin build ...`) and re-run
    without --build. A warm rebuild is a few seconds and fits comfortably."""
    print(f"building: {' '.join(CARGO_BUILD_CMD)} (cwd={REPO_ROOT})", flush=True)
    try:
        rc = subprocess.run(CARGO_BUILD_CMD, cwd=REPO_ROOT, timeout=_BUILD_TIMEOUT_SECONDS).returncode
    except subprocess.TimeoutExpired:
        sys.exit(
            "error: build exceeded 30s (repo subprocess-timeout policy). Build separately with "
            "`cargo xwin build --release --target x86_64-pc-windows-msvc`, then re-run without --build."
        )
    if rc != 0:
        sys.exit(f"error: build failed (rc={rc}); not launching")


def build_launch_command(
    me3_bin: str,
    steam_dir: Path | None,
    game_exe: Path | None,
    profile: Path | None,
    dll_arg: str,
) -> list[str]:
    """Assemble the me3 argv, omitting the options this host cannot answer for."""
    cmd = [me3_bin]
    if steam_dir is not None:
        cmd += ["--steam-dir", str(steam_dir)]
    cmd += ["launch"]
    if profile is not None:
        cmd += ["-p", str(profile)]
    cmd += ["-g", "eldenring"]
    if game_exe is not None:
        cmd += ["-e", str(game_exe)]
    cmd += ["-n", dll_arg]
    return cmd


def quoted(cmd: list[str], cwd: Path, env_overrides: dict[str, str]) -> str:
    """The launch as a copy-pasteable shell line (what --dry-run prints)."""
    env_part = " ".join(f"{k}={shlex.quote(v)}" for k, v in sorted(env_overrides.items()))
    argv = " ".join(shlex.quote(part) for part in cmd)
    return f"cd {shlex.quote(str(cwd))} && env {env_part} {argv}"


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Deploy the freshly-built repo DLL and launch Elden Ring via me3 for inspection.",
        epilog=(
            "Env overrides (all optional; defaults are discovered): ME3_BIN, ME3_STEAM_DIR, "
            "GAME_EXE, ME3_TMPDIR, ME3_PROFILE."
        ),
    )
    parser.add_argument(
        "--build",
        action="store_true",
        help="run `cargo xwin build --release --target x86_64-pc-windows-msvc` first, abort on failure",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="resolve everything and print the exact launch command, then exit 0 without launching",
    )
    args = parser.parse_args()

    if args.build:
        run_build()

    # 1. Resolve the host layout and every machine-specific path BEFORE any gate, so a failing
    #    gate still reports against the real, fully-resolved configuration.
    wsl = is_wsl()
    me3_bin = resolve_me3_binary(wsl)
    steam_dir = resolve_steam_dir()
    game_exe = resolve_game_exe(steam_dir)
    profile_env = os.environ.get("ME3_PROFILE")
    profile = Path(profile_env).expanduser() if profile_env else None
    tmpdir = Path(os.environ.get("ME3_TMPDIR", str(Path.home() / ".cache" / "me3" / "tmp"))).expanduser()
    # me3 must not run from a Rust checkout (payload hijack, see module docstring). The game dir
    # is the right CWD anyway -- it is where the DLL writes its debug log.
    cwd = game_exe.parent if game_exe is not None else Path.cwd()

    print(f"host: {'WSL2 (Windows me3)' if wsl else 'native Linux (native me3)'}", flush=True)
    print(f"me3: {me3_bin}", flush=True)
    print(f"steam-dir: {steam_dir if steam_dir is not None else '<not passed; me3 auto-detects>'}", flush=True)
    print(f"game-exe: {game_exe if game_exe is not None else '<not passed; me3 resolves the game>'}", flush=True)
    if profile is not None:
        print(f"profile: {profile} (from ME3_PROFILE)", flush=True)
    print(f"cwd: {cwd}", flush=True)

    if (cwd / ME3_CWD_HIJACK_DIR).is_dir():
        sys.exit(
            f"error: refusing to launch from {cwd} -- it contains {ME3_CWD_HIJACK_DIR}, which hijacks "
            "me3's launcher/mod-host payload resolution and makes the run die silently inside the "
            "compat tool. Set GAME_EXE (or ME3_STEAM_DIR) so the launch runs from the game directory."
        )

    # 2. Require Steam: the offline/direct eldenring launch reuses Steam's wineprefix, save-dir,
    #    and account id. With Steam down the run is not representative, so fail closed.
    steam_up, steam_evidence = steam_running()
    print(f"steam: {'RUNNING' if steam_up else 'NOT running'} ({steam_evidence})", flush=True)
    if not steam_up and not args.dry_run:
        sys.exit(
            f"error: Steam is not running ({steam_evidence}).\n"
            "Start Steam first; Elden Ring needs it (Linux host: me3 reuses Steam's "
            "wineprefix/compat tool/save-dir; Windows host: Steam DRM)."
        )

    # 3. Require the repo build output. Never silently launch a stale/absent DLL.
    if not BUILT_DLL.is_file():
        message = (
            f"built DLL not found at {BUILT_DLL}\n"
            "Build it first with:\n"
            "  cargo xwin build --release --target x86_64-pc-windows-msvc\n"
            "or re-run this script with --build."
        )
        if not args.dry_run:
            sys.exit(f"error: {message}")
        print(f"warning: {message}", flush=True)
        dll_arg = str(BUILT_DLL)
    else:
        # 4. Spell the repo's build output for THIS me3 (loaded IN PLACE, no copy).
        dll_arg = loader_dll_path(BUILT_DLL, wsl, me3_bin)
        print(f"loading repo DLL in place: {BUILT_DLL}", flush=True)
        if dll_arg != str(BUILT_DLL):
            print(f"      -> {dll_arg}", flush=True)
        print(f"      md5[:8] = {md5_prefix(BUILT_DLL)}  <- this is the dll me3 will load", flush=True)

    cmd = build_launch_command(me3_bin, steam_dir, game_exe, profile, dll_arg)
    # A turn-scoped TMPDIR from an agent harness is deleted the moment the launching command
    # returns, but me3 writes temporary profile artifacts after that point and dies with
    # PathError/ENOENT. Give the launch tree a stable, current-user temp dir instead.
    env_overrides = {"TMPDIR": str(tmpdir)}

    if args.dry_run:
        print("dry-run (nothing launched):", flush=True)
        print(quoted(cmd, cwd, env_overrides), flush=True)
        return

    tmpdir.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ)
    env.update(env_overrides)

    # 5. Launch. stdin=PIPE is held open forever (never closed): me3 tears the game down on stdin
    #    EOF, so keeping stdin open keeps the game alive for manual inspection. stdout/stderr
    #    inherit to the console so launch errors are visible. p.wait() returns only when the USER
    #    closes the game.
    print(f"launching: {quoted(cmd, cwd, env_overrides)}", flush=True)
    p = subprocess.Popen(cmd, stdin=subprocess.PIPE, cwd=str(cwd), env=env)
    print(
        f"me3 launched pid={p.pid}; holding it alive (no teardown). waiting for game exit...",
        flush=True,
    )
    rc = p.wait()
    print(f"me3 exited rc={rc} (game closed by user)", flush=True)


if __name__ == "__main__":
    main()
