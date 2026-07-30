#!/usr/bin/env python3
"""Mean-luminance sampler for the OS-picker dim overlay, as a PIXEL oracle.

WHY THIS IS NOT `capture-er-window.py`. That helper deliberately dispatches
`focuswindow` + `alterzorder top` at the ER window before it captures, because its job is to get a
readable artifact of the game no matter what is in front of it. Doing that here would RAISE the game
above the very overlay under test and above the OS dialog, i.e. it would destroy the thing being
measured. This sampler is strictly passive: it queries geometry and captures, and never dispatches
anything at the compositor.

WHAT IT MEASURES. `grim -g` captures a SCREEN REGION, not a window backing store, so the numbers it
returns are what is actually composited and visible -- which is exactly the question ("does a window
we own composite above the fullscreen Proton game?"). Mean luminance over the game's region must drop
sharply while the dim is up and return afterwards. The comparison is arithmetic, done here; no human
and no model looks at the image to decide.

PRIVACY. Only windows that identify as Elden Ring are ever considered, and nothing but their geometry
is read or written. The user's other windows are never enumerated, named, or captured. Several
top-level windows of the game process can match at once (the game, this overlay, comdlg32's dialog);
the LARGEST is taken, which is the fullscreen game region, and since every capture is a screen region
anyway the choice only affects the rectangle.

WINDOW CLASS. `steam_app_1245620` is what a Steam-launched session reports, and every capture helper
in `scripts/` hard-codes it. That is WRONG for the direct/offline me3 Proton launch the runtime probes
actually use: in run `picker-dim-bringup` the Win32 class was `ELDEN RING(tm)` and Hyprland reported
NO client of class `steam_app_1245620` at all, so a sample taken during a live dialog came back empty.
Hence the candidate set below rather than one literal. `ER_WINDOW_CLASS` overrides it outright when a
future launch path reports something new.

Usage:
  picker-dim-luma-probe.py sample <out-dir> <label>   # one timestamped sample, appended to luma.jsonl
  picker-dim-luma-probe.py watch <out-dir> <seconds>  # sample until the deadline (no sleep; paces on grim)
  picker-dim-luma-probe.py verdict <out-dir> <dim-start-ms> <dim-end-ms>  # score an interval
  picker-dim-luma-probe.py --selftest                # no compositor needed
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

# Exact classes accepted as "this is Elden Ring", plus a substring test for the Wine/Proton spellings.
# Kept deliberately narrow: every entry can only ever match the game, never one of the user's own
# applications. `ER_WINDOW_CLASS` (env) replaces the whole set when a launch path reports something
# else again.
WINDOW_CLASSES = ("steam_app_1245620", "eldenring.exe")
WINDOW_CLASS_SUBSTRINGS = ("eldenring", "elden ring")
# A dim at alpha 150/255 leaves ~41% of the original light through. Requiring the sampled mean to
# fall to at most 75% of the undimmed baseline is far looser than that, so the threshold survives a
# mostly-dark menu screen while still being impossible to pass by noise.
DIM_LUMA_CEILING = 0.75
# Every subprocess is bounded well under the repo's 30s non-game cap, so a wedged compositor call
# fails fast instead of stalling a probe.
CALL_TIMEOUT_SECONDS = 8


def is_er_window(client: dict) -> bool:
    """Whether this Hyprland client is an Elden Ring window.

    Checks `class` AND `initialClass`, because Wine can rewrite a window's class after mapping. An
    explicit `ER_WINDOW_CLASS` wins outright.
    """
    override = os.environ.get("ER_WINDOW_CLASS")
    names = [str(client.get(key) or "") for key in ("class", "initialClass")]
    if override:
        return any(name == override for name in names)
    for name in names:
        if name in WINDOW_CLASSES:
            return True
        folded = name.casefold()
        if any(token in folded for token in WINDOW_CLASS_SUBSTRINGS):
            return True
    return False


def er_region(hyprctl: str) -> tuple[int, int, int, int] | None:
    """Largest Elden Ring window's geometry as (x, y, w, h), or None."""
    try:
        out = subprocess.run(
            [hyprctl, "clients", "-j"], text=True, capture_output=True, timeout=CALL_TIMEOUT_SECONDS
        ).stdout
        clients = json.loads(out)
    except Exception:
        return None
    best: tuple[int, int, int, int] | None = None
    for client in clients:
        if not isinstance(client, dict) or not is_er_window(client):
            continue
        if client.get("mapped") is False or client.get("hidden") is True:
            continue
        at, size = client.get("at") or [], client.get("size") or []
        if len(at) != 2 or len(size) != 2:
            continue
        try:
            x, y, w, h = int(at[0]), int(at[1]), int(size[0]), int(size[1])
        except (TypeError, ValueError):
            continue
        if w <= 0 or h <= 0:
            continue
        if best is None or w * h > best[2] * best[3]:
            best = (x, y, w, h)
    return best


def mean_luma_of_png(path: Path) -> float | None:
    """Mean perceptual luminance in 0..1, via imagemagick so no image library is required."""
    magick = shutil.which("magick") or shutil.which("convert")
    if not magick:
        return None
    try:
        # -colorspace Gray then a 1x1 resize collapses the region to its average grey level, which
        # `txt:` prints as a single pixel -- cheaper and more exact than decoding the image here.
        out = subprocess.run(
            [magick, str(path), "-colorspace", "Gray", "-resize", "1x1!", "-depth", "16", "txt:-"],
            text=True,
            capture_output=True,
            timeout=CALL_TIMEOUT_SECONDS,
        ).stdout
    except Exception:
        return None
    return parse_magick_grey(out)


def parse_magick_grey(text: str) -> float | None:
    """Pull the 0..1 grey level out of imagemagick's `txt:` dump of a 1x1 image.

    Split out from the subprocess call so the parsing -- the part that silently differs between
    imagemagick builds -- is covered by `--selftest` without needing a compositor or a game.

    The real output is two lines:

        # ImageMagick pixel enumeration: 1,1,0,65535,gray
        0,0: (32896)  #808080808080  gray(128)

    The value in PARENTHESES is the quantum, scaled against the maximum declared in the header. The
    trailing `gray(128)` is the same pixel expressed in 8 bits, and reading THAT while dividing by
    the 16-bit maximum is the obvious mistake -- it would report mid-grey as 0.2% and make every run
    look like the screen went black. Hence: header first, parentheses second, `gray(...)` only as a
    fallback for builds that print a percentage there.
    """
    maximum = 65535.0
    for line in text.splitlines():
        if line.startswith("#") and "pixel enumeration:" in line:
            fields = line.split("pixel enumeration:", 1)[1].strip().split(",")
            if len(fields) >= 4:
                try:
                    maximum = float(fields[3]) or 65535.0
                except ValueError:
                    maximum = 65535.0
            continue
        if not line.startswith("0,0:"):
            continue
        if "(" in line:
            inner = line.split("(", 1)[1].split(")", 1)[0]
            first = inner.split(",")[0].strip()
            if first.endswith("%"):
                try:
                    return float(first[:-1]) / 100.0
                except ValueError:
                    return None
            try:
                return float(first) / maximum
            except ValueError:
                return None
    return None


def sample(out_dir: Path, label: str) -> int:
    out_dir.mkdir(parents=True, exist_ok=True)
    record: dict[str, object] = {"label": label, "epoch_ms": int(time.time() * 1000)}
    hyprctl, grim = shutil.which("hyprctl"), shutil.which("grim")
    if not hyprctl or not grim:
        record["error"] = f"missing tool hyprctl={hyprctl} grim={grim}"
    else:
        region = er_region(hyprctl)
        if region is None:
            record["error"] = f"no mapped Elden Ring window (looked for {WINDOW_CLASSES} / {WINDOW_CLASS_SUBSTRINGS})"
        else:
            x, y, w, h = region
            record["geom"] = f"{x},{y} {w}x{h}"
            png = out_dir / f"luma-{label}-{record['epoch_ms']}.png"
            try:
                rc = subprocess.run(
                    [grim, "-g", f"{x},{y} {w}x{h}", str(png)],
                    text=True,
                    capture_output=True,
                    timeout=CALL_TIMEOUT_SECONDS,
                )
                if rc.returncode != 0 or not png.exists():
                    record["error"] = f"grim rc={rc.returncode} {rc.stderr.strip()[:200]}"
                else:
                    record["mean_luma"] = mean_luma_of_png(png)
                    record["png"] = png.name
            except Exception as exc:
                record["error"] = f"grim failed: {exc}"
    with (out_dir / "luma.jsonl").open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(record) + "\n")
    print(json.dumps(record))
    return 0


def watch(out_dir: Path, seconds: float) -> int:
    """Sample until the deadline.

    NO SLEEP, deliberately -- the repo bans sleep as a synchronisation primitive, and none is needed
    here. Each iteration already blocks on a full-resolution `grim` grab plus an imagemagick pass,
    both synchronous with their own hard timeouts, so the loop paces itself on real work. `seconds`
    is a safety backstop that bounds the watcher, not a schedule.
    """
    deadline = time.monotonic() + seconds
    index = 0
    while time.monotonic() < deadline:
        sample(out_dir, f"watch{index:04d}")
        index += 1
    return 0


def classify(samples: list[dict], dim_start_ms: int, dim_end_ms: int) -> dict:
    """Score the dim interval against the samples outside it.

    Pure, so `--selftest` can prove the verdict logic without a game: a run whose inside-samples are
    not darker than its outside-samples must NOT come back as a pass, however many frames the overlay
    thread claims to have pushed.
    """
    inside = [
        s["mean_luma"]
        for s in samples
        if s.get("mean_luma") is not None and dim_start_ms <= int(s["epoch_ms"]) <= dim_end_ms
    ]
    outside = [
        s["mean_luma"]
        for s in samples
        if s.get("mean_luma") is not None
        and not (dim_start_ms <= int(s["epoch_ms"]) <= dim_end_ms)
    ]
    verdict: dict[str, object] = {
        "inside_count": len(inside),
        "outside_count": len(outside),
    }
    if not inside or not outside:
        verdict["result"] = "inconclusive"
        verdict["why"] = "need at least one sample inside the dim interval and one outside it"
        return verdict
    inside_mean = sum(inside) / len(inside)
    outside_mean = sum(outside) / len(outside)
    verdict["inside_mean_luma"] = inside_mean
    verdict["outside_mean_luma"] = outside_mean
    verdict["ratio"] = inside_mean / outside_mean if outside_mean else None
    if outside_mean <= 0:
        verdict["result"] = "inconclusive"
        verdict["why"] = "the baseline region was already black, so a dim cannot be distinguished"
    elif inside_mean / outside_mean <= DIM_LUMA_CEILING:
        verdict["result"] = "dim_visible"
        verdict["why"] = (
            f"the game region fell to {inside_mean / outside_mean:.0%} of its undimmed brightness, "
            "so a window we own is compositing above the fullscreen game"
        )
    else:
        verdict["result"] = "dim_not_visible"
        verdict["why"] = (
            f"the game region stayed at {inside_mean / outside_mean:.0%} of its undimmed brightness; "
            "the overlay did not reach the screen"
        )
    return verdict


def verdict(out_dir: Path, dim_start_ms: int, dim_end_ms: int) -> int:
    path = out_dir / "luma.jsonl"
    samples = []
    if path.exists():
        for line in path.read_text(encoding="utf-8").splitlines():
            try:
                samples.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    result = classify(samples, dim_start_ms, dim_end_ms)
    (out_dir / "luma-verdict.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))
    return 0 if result.get("result") == "dim_visible" else 1


def selftest() -> int:
    # Exactly what imagemagick prints here, header included. Mid-grey must read as ~0.5 -- reading
    # the trailing 8-bit `gray(128)` against the 16-bit maximum would give 0.002 instead.
    mid = parse_magick_grey(
        "# ImageMagick pixel enumeration: 1,1,0,65535,gray\n0,0: (32896)  #808080808080  gray(128)"
    )
    assert mid is not None and abs(mid - 0.502) < 0.01, mid
    black = parse_magick_grey(
        "# ImageMagick pixel enumeration: 1,1,0,65535,gray\n0,0: (0)  #000000000000  gray(0)"
    )
    assert black is not None and abs(black) < 1e-9, black
    white = parse_magick_grey(
        "# ImageMagick pixel enumeration: 1,1,0,65535,gray\n0,0: (65535)  #FFFFFFFFFFFF  gray(255)"
    )
    assert white is not None and abs(white - 1.0) < 1e-9, white
    # A build that declares an 8-bit quantum must not be read against 65535.
    eight_bit = parse_magick_grey(
        "# ImageMagick pixel enumeration: 1,1,0,255,gray\n0,0: (128)  #808080  gray(128)"
    )
    assert eight_bit is not None and abs(eight_bit - 0.502) < 0.01, eight_bit
    assert parse_magick_grey("nothing here") is None

    now = 1_000_000
    dark = [
        {"epoch_ms": now + 100, "mean_luma": 0.20},
        {"epoch_ms": now + 200, "mean_luma": 0.18},
    ]
    bright = [
        {"epoch_ms": now - 500, "mean_luma": 0.60},
        {"epoch_ms": now + 900, "mean_luma": 0.62},
    ]
    passing = classify(dark + bright, now, now + 300)
    assert passing["result"] == "dim_visible", passing

    # The verdict must FAIL when nothing darkened -- this is the case a frame counter alone would
    # happily call a success.
    flat = [{"epoch_ms": now + 100, "mean_luma": 0.60}, {"epoch_ms": now - 500, "mean_luma": 0.61}]
    assert classify(flat, now, now + 300)["result"] == "dim_not_visible"

    # A one-sided run is inconclusive, never a pass.
    assert classify(dark, now, now + 300)["result"] == "inconclusive"
    assert classify(bright, now, now + 300)["result"] == "inconclusive"

    # An already-black baseline cannot prove anything either way.
    black = [{"epoch_ms": now + 100, "mean_luma": 0.0}, {"epoch_ms": now - 500, "mean_luma": 0.0}]
    assert classify(black, now, now + 300)["result"] == "inconclusive"
    # The class matcher is what silently broke the first live sample, so it is covered here.
    assert is_er_window({"class": "steam_app_1245620"})
    assert is_er_window({"class": "eldenring.exe"})
    assert is_er_window({"class": "", "initialClass": "ELDEN RING\u2122"})
    assert is_er_window({"class": "elden ring"})
    assert not is_er_window({"class": "firefox"})
    assert not is_er_window({"class": "kitty", "initialClass": "kitty"})
    os.environ["ER_WINDOW_CLASS"] = "something-else"
    assert is_er_window({"class": "something-else"})
    assert not is_er_window({"class": "steam_app_1245620"}), "an explicit override must win outright"
    del os.environ["ER_WINDOW_CLASS"]

    print("picker-dim-luma-probe selftest: OK")
    return 0


def main() -> int:
    args = sys.argv[1:]
    if not args or args[0] in {"-h", "--help"}:
        print(__doc__)
        return 0
    if args[0] == "--selftest":
        return selftest()
    if args[0] == "sample" and len(args) >= 3:
        return sample(Path(args[1]), args[2])
    if args[0] == "watch" and len(args) >= 3:
        return watch(Path(args[1]), float(args[2]))
    if args[0] == "verdict" and len(args) >= 4:
        return verdict(Path(args[1]), int(args[2]), int(args[3]))
    print(__doc__)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
