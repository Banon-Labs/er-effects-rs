#!/usr/bin/env python3
"""Launch Elden Ring live via me3 with the freshly-built repo DLL, for manual inspection.

WSL <-> Windows seam (why the paths look "mixed"):
    me3.exe is a NATIVE WINDOWS process that we start from WSL. It loads THIS repo's build
    output IN PLACE: we hand me3 the Windows spelling of the repo's WSL path (via
    `wslpath -w`, e.g. `\\\\wsl.localhost\\<distro>\\home\\...\\er_effects_rs.dll`). No copy to a
    C:\\ tree -- Windows LoadLibraryW over the WSL filesystem was verified reliable (5/5),
    so the old "must copy to a Windows drive first" belief does not hold. (The real UNC
    hazard is log WRITES from the game, not the DLL load; the DLL's debug log already lands
    in the game dir, a Windows path.) One build, referenced where cargo puts it.

No-teardown stdin trick (INTENTIONAL, do not "fix"):
    me3 tears the game down when its own stdin hits EOF. We pass `stdin=PIPE` and NEVER
    close it, so me3's stdin never EOFs and me3 stays alive as the monitor with the game
    running. There is no taskkill and no runtime cap here: `p.wait()` returns only when
    the USER closes the game. This is a manual-inspection helper, not an autoresearch probe.

Note: the real DLL debug log is `er-effects-autoload-debug.log` in the game directory,
not anything this script writes. We let me3's stdout/stderr inherit to the console so
launch errors stay visible.
"""

import argparse
import hashlib
import subprocess
import sys
from pathlib import Path

# --- Fixed tool location (Windows me3, reached through /mnt/c) --------------------------
ME3 = "/mnt/c/Users/choza/AppData/Local/garyttierney/me3/bin/me3.exe"

# --- Repo build output (WSL side; derived from THIS script's location) -------------------
REPO_ROOT = Path(__file__).resolve().parents[1]
BUILT_DLL = (
    REPO_ROOT
    / "target"
    / "x86_64-pc-windows-msvc"
    / "release"
    / "er_effects_rs.dll"
)

CARGO_BUILD_CMD = [
    "cargo",
    "xwin",
    "build",
    "--release",
    "--target",
    "x86_64-pc-windows-msvc",
]


def md5_prefix(path: Path) -> str:
    """Return the first 8 hex chars of the file's md5 (identifies WHICH dll is loaded)."""
    h = hashlib.md5()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()[:8]


def steam_running() -> bool:
    """True if Steam is running -- as a Linux/Wine process OR a native-Windows process.

    This box is WSL2 on a Windows HOST, so Steam is `steam.exe` (a Windows process) that
    Linux `pgrep` cannot see. Check both: `pgrep` for a Wine/Linux Steam, and the Windows
    `tasklist.exe` (via WSL interop) for `steam.exe`. Note tasklist prints a "No tasks..."
    line and exits 0 when nothing matches, so we test the OUTPUT for steam.exe, not the
    return code."""
    try:
        if (
            subprocess.run(
                ["pgrep", "-x", "steam"], capture_output=True, timeout=10
            ).returncode
            == 0
        ):
            return True
    except (OSError, subprocess.SubprocessError):
        pass
    try:
        out = subprocess.run(
            ["tasklist.exe", "/FI", "IMAGENAME eq steam.exe", "/NH"],
            capture_output=True,
            text=True,
            timeout=10,
        ).stdout
        if "steam.exe" in out.lower():
            return True
    except (OSError, subprocess.SubprocessError):
        pass
    return False


def run_build() -> None:
    """Run the windows-target cargo build from the repo root; abort the script on failure.

    Repo policy caps subprocess timeouts at 30s, so a cold build can exceed it and raise
    TimeoutExpired -- in that case just build separately (`cargo xwin build ...`) and re-run
    without --build. A warm rebuild is a few seconds and fits comfortably."""
    print(f"building: {' '.join(CARGO_BUILD_CMD)} (cwd={REPO_ROOT})", flush=True)
    try:
        rc = subprocess.run(CARGO_BUILD_CMD, cwd=REPO_ROOT, timeout=30).returncode
    except subprocess.TimeoutExpired:
        sys.exit(
            "error: build exceeded 30s (repo subprocess-timeout policy). Build separately with "
            "`cargo xwin build --release --target x86_64-pc-windows-msvc`, then re-run without --build."
        )
    if rc != 0:
        sys.exit(f"error: build failed (rc={rc}); not launching")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Deploy the freshly-built repo DLL and launch Elden Ring via me3 for inspection.",
    )
    parser.add_argument(
        "--build",
        action="store_true",
        help="run `cargo xwin build --release --target x86_64-pc-windows-msvc` first, abort on failure",
    )
    args = parser.parse_args()

    if args.build:
        run_build()

    # 1. Require Steam BEFORE anything else: the offline/direct eldenring launch reuses
    #    Steam's wineprefix, save-dir, and account id. With Steam down the run is not
    #    representative, so fail closed.
    if not steam_running():
        sys.exit(
            "error: Steam is not running (checked Linux `pgrep -x steam` and Windows "
            "`tasklist.exe steam.exe`).\n"
            "Start Steam first; Elden Ring needs it (Wine host: reuses Steam's "
            "wineprefix/save-dir/account; Windows host: Steam DRM)."
        )

    # 2. Require the repo build output. Never silently launch a stale/absent DLL.
    if not BUILT_DLL.is_file():
        sys.exit(
            f"error: built DLL not found at {BUILT_DLL}\n"
            "Build it first with:\n"
            "  cargo xwin build --release --target x86_64-pc-windows-msvc\n"
            "or re-run this script with --build."
        )

    # 3. Spell the repo's build output for the Windows me3 (loaded IN PLACE, no copy).
    #    `wslpath -w` gives the \\wsl.localhost\<distro>\... form; Windows LoadLibraryW over
    #    the WSL filesystem was verified reliable, so no stale-copy footgun exists.
    dll_win = subprocess.run(
        ["wslpath", "-w", str(BUILT_DLL)], capture_output=True, text=True, check=True, timeout=15
    ).stdout.strip()
    print(f"loading repo DLL in place: {BUILT_DLL}", flush=True)
    print(f"      -> {dll_win}", flush=True)
    print(f"      md5[:8] = {md5_prefix(BUILT_DLL)}  <- this is the dll me3 will load", flush=True)

    # 4. Launch me3 with the WINDOWS spelling of the repo DLL path.
    #    stdin=PIPE is held open forever (never closed): me3 tears the game down on stdin
    #    EOF, so keeping stdin open keeps the game alive for manual inspection. stdout/
    #    stderr inherit to the console so launch errors are visible. p.wait() returns only
    #    when the USER closes the game.
    cmd = [ME3, "launch", "-g", "eldenring", "-n", dll_win]
    print(f"launching: {' '.join(cmd)}", flush=True)
    p = subprocess.Popen(cmd, stdin=subprocess.PIPE)
    print(
        f"me3 launched pid={p.pid}; holding it alive (no teardown). waiting for game exit...",
        flush=True,
    )
    rc = p.wait()
    print(f"me3 exited rc={rc} (game closed by user)", flush=True)


if __name__ == "__main__":
    main()
