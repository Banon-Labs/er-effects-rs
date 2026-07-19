#!/usr/bin/env python3
"""Regression tests for scripts/detect-proc.py.

The detector runs on machines with different process topologies (native Linux, WSL with a
Windows Steam, nested containers), so the tests must NOT depend on the live box. They pin
the pure parsing/logic (path translation, ACF/VDF parsing, target matching) with fixtures
and drive the environment-touching helpers (`_run`, `shutil.which`, filesystem) through
monkeypatching, so the same assertions hold on every box in the fleet.
"""
from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
DETECT_PATH = REPO_ROOT / "scripts" / "detect-proc.py"


def load_detect():
    spec = importlib.util.spec_from_file_location("detect_proc", DETECT_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load {DETECT_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def check_winpath_to_wsl(d) -> None:
    assert d._winpath_to_wsl(r"C:\SteamLibrary") == "/mnt/c/SteamLibrary"
    assert d._winpath_to_wsl(r"D:\Steam\steamapps") == "/mnt/d/Steam/steamapps"
    # Double-backslash escaping as it appears inside libraryfolders.vdf.
    assert d._winpath_to_wsl("C:\\\\SteamLibrary") == "/mnt/c/SteamLibrary"
    # Already-POSIX / non-Windows input passes through untouched.
    assert d._winpath_to_wsl("/home/user/.local/share/Steam") == "/home/user/.local/share/Steam"


def check_acf_and_library_parsing(d, tmp: Path) -> None:
    manifest = tmp / "appmanifest_1245620.acf"
    manifest.write_text(
        '"AppState"\n{\n'
        '\t"appid"\t\t"1245620"\n'
        '\t"name"\t\t"ELDEN RING"\n'
        '\t"StateFlags"\t\t"4"\n'
        '\t"installdir"\t\t"ELDEN RING"\n}\n',
        encoding="utf-8",
    )
    assert d._acf_field(str(manifest), "name") == "ELDEN RING"
    assert d._acf_field(str(manifest), "StateFlags") == "4"
    assert d._acf_field(str(manifest), "missing") is None

    hit = d._game_in_library(str(tmp), "1245620")
    assert hit is not None
    assert hit["name"] == "ELDEN RING"
    assert hit["fully_installed"] is True
    # A partial/updating install (StateFlags != 4) must NOT read as fully installed.
    manifest.write_text('"AppState"\n{\n\t"StateFlags"\t\t"6"\n\t"name"\t\t"ELDEN RING"\n}\n',
                        encoding="utf-8")
    partial = d._game_in_library(str(tmp), "1245620")
    assert partial is not None and partial["fully_installed"] is False
    # A different appid is simply absent from this library.
    assert d._game_in_library(str(tmp), "9999999") is None


def check_library_roots_from_vdf(d, tmp: Path) -> None:
    vdf = tmp / "libraryfolders.vdf"
    vdf.write_text(
        '"libraryfolders"\n{\n'
        '\t"0"\n\t{\n\t\t"path"\t\t"D:\\\\Steam"\n\t}\n'
        '\t"1"\n\t{\n\t\t"path"\t\t"C:\\\\SteamLibrary"\n\t}\n}\n',
        encoding="utf-8",
    )
    roots = d._library_roots_from_vdf(str(vdf))
    assert roots == ["D:\\\\Steam", "C:\\\\SteamLibrary"]
    assert d._library_roots_from_vdf(str(tmp / "nope.vdf")) == []


def check_match_target_uses_executable_not_cmdline(d) -> None:
    # local-proc rows carry (pid, comm, argv0). A generic "steam" MUST match the real
    # steam.exe image but MUST NOT match a shell whose argv0 is /usr/bin/zsh -- the earlier
    # full-cmdline match falsely flagged the detector's own invocation.
    rows = [
        ("100", "steam.exe", "steam.exe"),
        ("200", "zsh", "/usr/bin/zsh"),
        ("300", "eldenring.exe", "/games/ELDEN RING/Game/eldenring.exe"),
    ]
    import re
    steam_hits = d.match_target(re.compile(r"\bsteam(\.exe)?\b", re.IGNORECASE), rows)
    assert [h[0] for h in steam_hits] == ["100"]
    game_hits = d.match_target(re.compile(r"eldenring(\.exe)?", re.IGNORECASE), rows)
    assert [h[0] for h in game_hits] == ["300"]


def check_boundary_windows_parses_tasklist_csv(d, monkeypatch) -> None:
    csv = (
        '"steam.exe","5752","Console","1","178,396 K"\r\n'
        '"steamwebhelper.exe","21648","Console","1","175,040 K"\r\n'
    )
    monkeypatch.setattr(d.shutil, "which", lambda name: "/x/tasklist.exe" if name == "tasklist.exe" else None)
    monkeypatch.setattr(d, "_run", lambda *_a, **_k: csv)
    rows = d.boundary_windows()
    assert rows is not None
    names = {name for _, name, _ in rows}
    assert "steam.exe" in names and "steamwebhelper.exe" in names
    pid_by_name = {name: pid for pid, name, _ in rows}
    assert pid_by_name["steam.exe"] == "5752"


def check_steam_readiness_windows_ready(d, monkeypatch, tmp: Path) -> None:
    # Build a fake Windows Steam library on disk that maps through _winpath_to_wsl. We point
    # the primary SteamPath at tmp and expose ER as fully installed in a second library.
    primary = tmp / "steam"
    (primary / "steamapps").mkdir(parents=True)
    lib2 = tmp / "SteamLibrary"
    (lib2 / "steamapps").mkdir(parents=True)
    (lib2 / "steamapps" / "appmanifest_1245620.acf").write_text(
        '"AppState"\n{\n\t"name"\t\t"ELDEN RING"\n\t"StateFlags"\t\t"4"\n}\n', encoding="utf-8")
    (primary / "steamapps" / "libraryfolders.vdf").write_text(
        f'"libraryfolders"\n{{\n\t"0"\n\t{{\n\t\t"path"\t\t"{primary}"\n\t}}\n'
        f'\t"1"\n\t{{\n\t\t"path"\t\t"{lib2}"\n\t}}\n}}\n', encoding="utf-8")

    monkeypatch.setattr(d.shutil, "which",
                        lambda name: f"/x/{name}" if name in ("reg.exe", "tasklist.exe") else None)
    # _winpath_to_wsl would rewrite an absolute POSIX tmp path unchanged (no drive letter),
    # so the library roots resolve straight to the temp dirs.

    def fake_run(cmd, _timeout=8.0):
        if cmd[0] == "tasklist.exe":
            return '"steam.exe","5752","Console","1","10 K"\r\n'
        if cmd[0] == "reg.exe":
            val = cmd[4]
            if val == "ActiveUser":
                return "    ActiveUser    REG_DWORD    0x18fa4be\r\n"
            if val == "SteamPath":
                return f"    SteamPath    REG_SZ    {primary}\r\n"
        return None

    monkeypatch.setattr(d, "_run", fake_run)
    r = d.steam_readiness_windows("1245620")
    assert r is not None
    assert r["running"] is True
    assert r["signed_in"] is True
    assert r["game_installed"] is True
    assert r["game"]["name"] == "ELDEN RING"

    # Not-signed-in variant: ActiveUser 0x0 flips signed_in off and the READY verdict.
    def fake_run_logged_out(cmd, _timeout=8.0):
        if cmd[0] == "reg.exe" and cmd[4] == "ActiveUser":
            return "    ActiveUser    REG_DWORD    0x0\r\n"
        return fake_run(cmd)

    monkeypatch.setattr(d, "_run", fake_run_logged_out)
    r2 = d.steam_readiness_windows("1245620")
    assert r2 is not None and r2["signed_in"] is False


# --- tiny monkeypatch shim so the file runs standalone (python3 scripts/test-detect-proc.py)
# as well as under pytest, matching the repo's `python3 scripts/test-*.py` convention. ---
class _MonkeyPatch:
    def __init__(self) -> None:
        self._undo = []

    def setattr(self, target, name, value) -> None:
        old = getattr(target, name)
        self._undo.append((target, name, old))
        setattr(target, name, value)

    def undo(self) -> None:
        for target, name, old in reversed(self._undo):
            setattr(target, name, old)
        self._undo.clear()


def main() -> int:
    d = load_detect()
    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        check_winpath_to_wsl(d)
        check_acf_and_library_parsing(d, tmp)
        check_library_roots_from_vdf(d, tmp)
        check_match_target_uses_executable_not_cmdline(d)
        for fn in (check_boundary_windows_parses_tasklist_csv,):
            mp = _MonkeyPatch()
            try:
                fn(d, mp)
            finally:
                mp.undo()
        mp = _MonkeyPatch()
        try:
            check_steam_readiness_windows_ready(d, mp, tmp)
        finally:
            mp.undo()
    print("detect-proc: all tests passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
