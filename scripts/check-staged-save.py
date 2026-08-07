#!/usr/bin/env python3
"""Refuse a launch whose STAGED save is not the save that was configured.

WHY THIS EXISTS
---------------
2026-08-07: three launches in a row softlocked at the boot cover (~51.9%, `autoload_attempts=0`),
each costing the user their screen and ~90s of boot. Every one of them logged a clean staging line:

    save-override: direct-file staged lower=28967888 native=28967888 bytes ... 'ER0000.co2'
    save-override: ENFORCED -- staging supplied save source '...' into private native save root

and then died on:

    native-fullread: DESER slot=0 c30=0xa010000 level=9 -> GUARD
    native-fullread: GUARD FAIL -- NO continue_confirm, NO SetState5, NO save write (save-safe)

The staging genuinely succeeded. It wrote the right bytes to `ER0000.sl2`, because the save-mode
latch resolved `seamless=false` from a FILENAME (`reason=active-default-save-file-name`) even though
`ER_EFFECTS_SAVE_MODE_HINT=seamless` was in the process environment and `ersc.dll` was in the
profile. Seamless Co-op was loaded, so the game opened `er0000.co2` instead -- a leftover from the
previous day holding a level-9 Vagabond. The full read returned that character, and the autoload
guard correctly refused to continue or write.

The failure is INVISIBLE from the log: "staged N bytes" prints identically whether or not the file
the game will actually open is the file that was staged. Only hashing the stage directory shows it:

    save-files/125-Frenzy/ER0000.co2              62c21e6ac904  slot0 Maddened Bean L125
    <stage>/76561197986456766/ER0000.sl2          62c21e6ac904  <- staged, correct, NOT opened
    <stage>/76561197986456766/er0000.co2          e019cdb1bb2d  <- stale, WRONG, opened

WHAT IT CHECKS
--------------
Every container in the stage directory that the game could open holds the same CHARACTER at the
configured slot as the source does -- the name and level that the autoload guard itself tests
(`level_real`, `name_len`). A container holding somebody else is reported with what it actually
contains, because "stale" is not actionable but "this file is a level-9 Vagabond and your config
asked for Maddened Bean L125" is.

It compares IDENTITY, not bytes. Byte equality is the wrong oracle and this checker originally used
it: the first live run flagged the container the game had just written during play, because normal
progress diverges from the staged copy within seconds. Same character, advanced state, nothing
wrong. The failure that actually costs a launch is a container holding a DIFFERENT character, which
is what the guard rejects and what this reports.

It is also deliberately not a timestamp check. A wrong file can be newer than the stage (the game
writes to it during play) and a correct file can be older (nothing re-staged because nothing
changed). Only the decoded slot answers the question.

`.bak` siblings are reported but never fail the check: the game writes those itself.

USAGE
    python3 scripts/check-staged-save.py                # check, exit 1 if a stale container wins
    python3 scripts/check-staged-save.py --fix          # overwrite stale containers from the source
    python3 scripts/check-staged-save.py --selftest     # prove the check fails when it should

`--fix` is an UNBLOCK, not a repair: it makes every container name hold the configured save so that
whichever one the mode latch picks is correct. The actual defect is the latch choosing the container
from a filename rather than from the loaded ERSC module / the mode hint; see bd
`staged-save-softlock-seamless-latch-false-stale-container-2026-08-07`.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import os
import re
import shutil
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parent

# Containers the game may open. Seamless accepts BOTH; vanilla accepts only .sl2 (bd
# save-container-mode-lock-is-asymmetric-seamless-takes-both-2026-08-02). Because the mode is
# decided at runtime -- and has been decided WRONG -- this checker does not try to predict which one
# wins. It requires every candidate to be correct, which is true regardless of how the latch lands.
CONTAINER_SUFFIXES = (".sl2", ".co2")
STAGE_DIR_NAME = "er-effects-save-redirect-stage"

DEFAULT_GAME_DIR = Path(
    os.environ.get(
        "ER_GAME_DIR",
        Path.home() / ".local/share/Steam/steamapps/common/ELDEN RING/Game",
    )
)


def _load_oracle():
    """Import the evidence-bound slot decoder, or None when it is unavailable."""
    try:
        spec = importlib.util.spec_from_file_location(
            "save_slot_oracle", HERE / "save-slot-oracle.py"
        )
        if not spec or not spec.loader:
            return None
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module
    except Exception:  # noqa: BLE001 - identity text is a nicety; never fail the check on it
        return None


def wine_path_to_unix(value: str) -> Path | None:
    r"""Map a Wine path from the config onto the Linux filesystem.

    The DLL takes these paths VERBATIM into Windows file APIs, so the config holds `Z:\home\...`.
    `Z:` is the prefix's root mapping; anything else (notably `S:`, the Steam library) is not
    resolvable from here without reading the prefix's dosdevices, so it returns None rather than
    guessing -- a wrong guess would make this checker pass on a file nobody reads.
    """
    text = value.strip().strip("'\"")
    if not text:
        return None
    match = re.match(r"^([A-Za-z]):[\\/](.*)$", text)
    if match:
        drive, rest = match.group(1).upper(), match.group(2)
        if drive != "Z":
            return None
        return Path("/" + rest.replace("\\", "/"))
    # Already a unix path (the config accepts both forms).
    return Path(text.replace("\\", "/")) if text.startswith("/") else None


def read_configured_save(game_dir: Path) -> tuple[Path | None, int, str]:
    """Return `(source_path, slot, note)` for the game-dir er-effects.toml."""
    toml = game_dir / "er-effects.toml"
    if not toml.is_file():
        return None, 0, f"no er-effects.toml at {toml} -- nothing is configured, nothing to check"
    source: Path | None = None
    slot = 0
    note = "er-effects.toml sets no save_file -- the game uses its own save, nothing to check"
    for raw in toml.read_text(encoding="utf-8", errors="replace").splitlines():
        line = raw.strip()
        if line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        key = key.strip()
        if key == "slot":
            try:
                slot = int(value.strip().strip("'\""))
            except ValueError:
                pass
        elif key == "save_file":
            resolved = wine_path_to_unix(value)
            if resolved is None:
                return None, slot, f"save_file is set but not resolvable from Linux: {value.strip()}"
            source, note = resolved, ""
    return source, slot, note


def stage_containers(source: Path) -> list[Path]:
    """Every container under the stage root beside `source`, `.bak` siblings included.

    The stage root is `<source dir>/er-effects-save-redirect-stage/<eldenring>/<steamid>/`, and the
    directory components vary in case between runs (`EldenRing` vs `eldenring` have both been
    observed), so this walks rather than assuming a spelling.
    """
    root = source.parent / STAGE_DIR_NAME
    if not root.is_dir():
        return []
    found: list[Path] = []
    for dirpath, _dirnames, filenames in os.walk(root):
        for name in filenames:
            lowered = name.lower()
            if any(
                lowered.endswith(suffix) or lowered.endswith(suffix + ".bak")
                for suffix in CONTAINER_SUFFIXES
            ):
                found.append(Path(dirpath) / name)
    return sorted(found)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def slot_identity(path: Path, slot: int, oracle) -> tuple[tuple[str, object] | None, str]:
    """Return `((name, level), text)` for `slot`, or `(None, text)` when it cannot be decoded.

    The identity pair is deliberately `(name, level)` and NOT the map id: `c30` legitimately changes
    the moment the character walks through a loading screen, so including it would re-introduce the
    false positive this function exists to remove. Name and level are what the autoload guard tests.
    """
    if oracle is None:
        return None, ""
    try:
        data = path.read_bytes()
        decoded = oracle.decode_save_slot(data, path, slot).get("decoded_fields") or {}
    except Exception:  # noqa: BLE001 - a decode failure is not this checker's business
        return None, ""
    name = decoded.get("name", "")
    level = decoded.get("level")
    c30 = decoded.get("saved_map_c30")
    c30_text = f"0x{c30:x}" if isinstance(c30, int) else c30
    if decoded.get("name_empty_like", True):
        return ("", level), f"slot{slot} EMPTY-LIKE (level={level} c30={c30_text})"
    return (name, level), f"slot{slot} {name!r} level={level} c30={c30_text}"


def check(game_dir: Path, apply_fix: bool, oracle: object | None = None) -> tuple[int, list[str]]:
    """Return `(exit_code, lines)`. Exit 1 means a wrong-character container would win the launch.

    `oracle` is injectable so the selftest can exercise the real comparison logic without shipping
    game-derived save bytes into the repo (which is forbidden) and without depending on the local
    extraction corpus being present.
    """
    lines: list[str] = []
    source, slot, note = read_configured_save(game_dir)
    if source is None:
        lines.append(f"[staged-save] {note}")
        return 0, lines
    if not source.is_file():
        lines.append(f"[staged-save] REFUSED -- configured save_file does not exist: {source}")
        return 1, lines

    if oracle is None:
        oracle = _load_oracle()
    source_hash = sha256(source)
    source_key, source_text = slot_identity(source, slot, oracle)
    lines.append(f"[staged-save] source {source}")
    lines.append(f"[staged-save]   {source_hash[:12]}  {source_text or '(slot undecodable)'}")
    if source_key is None:
        lines.append(
            "[staged-save] cannot decode the configured slot from the source, so there is no "
            "identity to compare against; skipping rather than guessing"
        )
        return 0, lines
    if not source_key[0]:
        # An empty-like source slot is a REFUSAL, not a pass. Comparing "empty" against "empty"
        # made every container match and reported OK on a save that cannot autoload anything --
        # a fail-open this checker's own end-to-end test caught. The level-9 default template that
        # softlocked the 2026-08-07 runs decodes exactly this way (name_len=0), so treating it as
        # agreement would have blessed the very failure this exists to stop.
        lines.append(
            f"[staged-save] REFUSED -- the configured slot {slot} of the source has NO character "
            f"({source_text}). Nothing can autoload from it; check `slot` in er-effects.toml."
        )
        return 1, lines

    containers = stage_containers(source)
    if not containers:
        lines.append(
            "[staged-save] no stage directory yet -- the first launch creates it; nothing stale "
            "can win"
        )
        return 0, lines

    stale: list[Path] = []
    for path in containers:
        try:
            path_hash = sha256(path)
        except OSError as exc:
            lines.append(f"[staged-save]   UNREADABLE {path.name}: {exc}")
            continue
        is_backup = path.name.lower().endswith(".bak")
        key, text = slot_identity(path, slot, oracle)
        if path_hash == source_hash:
            lines.append(f"[staged-save]   ok      {path_hash[:12]}  {path.name}  (identical)")
            continue
        if key == source_key:
            # Same character, different bytes: the game wrote progress into the staged container.
            # That is normal play, not a staging fault, and calling it stale was this checker's own
            # first bug -- it fired on the live run seconds after the game started saving.
            lines.append(
                f"[staged-save]   ok      {path_hash[:12]}  {path.name}  (same character, advanced)"
            )
            continue
        if is_backup:
            # The game writes .bak itself; a differing backup is not what the loader opens.
            lines.append(
                f"[staged-save]   backup  {path_hash[:12]}  {path.name}  ({text or 'undecodable'})"
            )
            continue
        stale.append(path)
        lines.append(
            f"[staged-save]   WRONG   {path_hash[:12]}  {path.name}  ({text or 'undecodable'})"
        )

    if not stale:
        lines.append(
            f"[staged-save] OK -- every container the game could open holds {source_text}"
        )
        return 0, lines

    if apply_fix:
        for path in stale:
            shutil.copyfile(source, path)
            lines.append(f"[staged-save] FIXED -- overwrote {path.name} from the configured source")
        lines.append(
            "[staged-save] this is an UNBLOCK, not a repair: the mode latch still picks the "
            "container by filename. See bd "
            "staged-save-softlock-seamless-latch-false-stale-container-2026-08-07."
        )
        return 0, lines

    lines.append("[staged-save] REFUSED -- a container holding the WRONG CHARACTER would win this launch.")
    lines.append(
        "[staged-save] The save-mode latch picks the container at runtime and has picked it WRONG "
        "before, so whichever name it lands on must already be correct."
    )
    lines.append("[staged-save] Re-run with --fix to overwrite the stale container(s) from the source.")
    return 1, lines


def selftest() -> int:
    """A checker that only ever passes is decoration. Prove it fails on the real failure shape."""
    failures = 0

    def report(ok: bool, label: str) -> None:
        nonlocal failures
        print(f"  {'PASS' if ok else 'FAIL'}  {label}")
        if not ok:
            failures += 1

    print("[staged-save] selftest")

    # Wine path mapping: Z: resolves, other drives refuse rather than guess.
    report(wine_path_to_unix(r"Z:\home\x\ER0000.sl2") == Path("/home/x/ER0000.sl2"), "Z: maps to /")
    report(wine_path_to_unix(r"S:\steamapps\ER0000.sl2") is None, "non-Z drive refuses to guess")
    report(wine_path_to_unix("") is None, "empty value refuses")

    class StubOracle:
        """Decode the synthetic fixtures below: `b"<name>|<level>|<pad>"`.

        No game-derived save bytes are versioned (repo rule), and the local extraction corpus may be
        absent, so the selftest supplies its own decoder and exercises the REAL comparison logic.
        """

        @staticmethod
        def decode_save_slot(data: bytes, _path: Path, _slot: int) -> dict:
            name, _, rest = data.decode("utf-8", "replace").partition("|")
            level_text, _, _pad = rest.partition("|")
            return {
                "decoded_fields": {
                    "name": name,
                    "level": int(level_text or 0),
                    "saved_map_c30": 0x1C000000,
                    "name_empty_like": not name,
                }
            }

    stub = StubOracle()

    def fixture(name: str, level: int, pad: str = "a") -> bytes:
        return f"{name}|{level}|{pad * 64}".encode()

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        game = root / "Game"
        game.mkdir()
        saves = root / "save-files" / "corpus"
        stage = saves / STAGE_DIR_NAME / "eldenring" / "76561197986456766"
        stage.mkdir(parents=True)
        source = saves / "ER0000.co2"
        source.write_bytes(fixture("Maddened Bean", 125))
        game.joinpath("er-effects.toml").write_text(
            f"save_file = '{source}'\nslot = 0\n", encoding="utf-8"
        )

        # A stage holding the same bytes passes.
        (stage / "ER0000.sl2").write_bytes(source.read_bytes())
        code, _ = check(game, apply_fix=False, oracle=stub)
        report(code == 0, "byte-identical stage passes")

        # THE REGRESSION THIS CHECKER CAUSED ITSELF: the game writes progress into the staged
        # container within seconds of launch. Same character, different bytes. Byte comparison
        # called that stale and refused a healthy launch.
        (stage / "ER0000.sl2").write_bytes(fixture("Maddened Bean", 125, pad="b"))
        code, lines = check(game, apply_fix=False, oracle=stub)
        report(code == 0, "same character with advanced bytes does NOT refuse")
        report(
            any("same character, advanced" in line for line in lines),
            "advanced-progress container is labelled, not flagged",
        )

        # THE REAL FAILURE: a sibling container holding a DIFFERENT character.
        wrong = stage / "er0000.co2"
        wrong.write_bytes(fixture("Vagabond", 9))
        code, lines = check(game, apply_fix=False, oracle=stub)
        report(code == 1, "wrong-character sibling container REFUSES")
        report(
            any("WRONG" in line and "er0000.co2" in line for line in lines),
            "refusal names the offending file",
        )
        report(
            any("Vagabond" in line and "level=9" in line for line in lines),
            "refusal says what the file actually holds",
        )

        # A level-up of the SAME character is still the same character.
        wrong.write_bytes(fixture("Maddened Bean", 126))
        code, _ = check(game, apply_fix=False, oracle=stub)
        report(code == 1, "same name at a different level is treated as a mismatch")

        # A .bak holding somebody else is not what the loader opens.
        wrong.write_bytes(source.read_bytes())
        (stage / "ER0000.sl2.bak").write_bytes(fixture("Vagabond", 9))
        code, _ = check(game, apply_fix=False, oracle=stub)
        report(code == 0, "wrong-character .bak does not fail the check")

        # --fix makes every container correct, and the check then passes.
        wrong.write_bytes(fixture("Vagabond", 9))
        code, _ = check(game, apply_fix=True, oracle=stub)
        report(code == 0, "--fix reports success")
        report(wrong.read_bytes() == source.read_bytes(), "--fix actually rewrote the wrong file")
        code, _ = check(game, apply_fix=False, oracle=stub)
        report(code == 0, "check passes after --fix")

        # An empty-like slot (the level-9 default template, name_len=0) is a mismatch, not a pass.
        wrong.write_bytes(fixture("", 9))
        code, lines = check(game, apply_fix=False, oracle=stub)
        report(code == 1, "empty-like slot REFUSES")
        report(any("EMPTY-LIKE" in line for line in lines), "empty-like slot is named as such")

        # FAIL-OPEN REGRESSION (found by the end-to-end test, not by this selftest): when the SOURCE
        # slot is empty-like, every container "matched" it and the check reported OK on a save that
        # cannot autoload at all. An unusable source must refuse.
        empty_source = saves / "EMPTY.co2"
        empty_source.write_bytes(fixture("", 9))
        game.joinpath("er-effects.toml").write_text(
            f"save_file = '{empty_source}'\nslot = 0\n", encoding="utf-8"
        )
        code, lines = check(game, apply_fix=False, oracle=stub)
        report(code == 1, "empty-like SOURCE slot REFUSES (no fail-open)")
        report(
            any("NO character" in line for line in lines),
            "empty-source refusal explains that nothing can autoload",
        )
        game.joinpath("er-effects.toml").write_text(
            f"save_file = '{source}'\nslot = 0\n", encoding="utf-8"
        )

        # A configured-but-missing source must refuse, not silently pass.
        game.joinpath("er-effects.toml").write_text(
            f"save_file = '{saves / 'nope.sl2'}'\n", encoding="utf-8"
        )
        code, _ = check(game, apply_fix=False, oracle=stub)
        report(code == 1, "missing configured source REFUSES")

        # No save_file configured at all is not a failure -- the game uses its own save.
        game.joinpath("er-effects.toml").write_text("slot = 0\n", encoding="utf-8")
        code, _ = check(game, apply_fix=False, oracle=stub)
        report(code == 0, "no save_file configured is not a failure")

        # The configured slot is honoured, not hard-coded to 0.
        game.joinpath("er-effects.toml").write_text(
            f"save_file = '{source}'\nslot = 3\n", encoding="utf-8"
        )
        _src, parsed_slot, _note = read_configured_save(game)
        report(parsed_slot == 3, "slot is read from the config")

    print(f"[staged-save] selftest {'OK' if failures == 0 else f'{failures} FAILURE(S)'}")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--game-dir", type=Path, default=DEFAULT_GAME_DIR)
    parser.add_argument(
        "--fix",
        action="store_true",
        help="overwrite stale containers from the configured source (unblock, not a repair)",
    )
    parser.add_argument("--selftest", action="store_true", help="prove the check fails when it should")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    code, lines = check(args.game_dir, apply_fix=args.fix)
    stream = sys.stderr if code else sys.stdout
    for line in lines:
        print(line, file=stream)
    return code


if __name__ == "__main__":
    raise SystemExit(main())
