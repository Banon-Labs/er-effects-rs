#!/usr/bin/env python3
"""Every feature `er-quickload` declares must actually gate code, and the count may only be raised on purpose.

A Cargo feature that no `#[cfg(feature = "...")]` names compiles the same code whether it is on or
off. Declaring one is free and looks like progress; turning it off then changes nothing at all. That
happened here on 2026-09-11: `default` was trimmed to four features, the DLL was rebuilt and
relaunched, and the report back was "that seemed to have no effect" -- correct, because five of the
six features named zero lines between them. The build was 5 files smaller purely through the
dependency closure.

So this gate holds two numbers per feature and refuses any drift in either direction:

* measured below the baseline -- a gate was deleted, and the feature quietly got wider;
* measured above the baseline -- a gate was added, which is the point, but the new number has to
  land in the baseline in the same commit so the ratchet is in the diff a reviewer reads.

`ungated` lists the features that name nothing yet, each with the reason it is still there. That
list is the work remaining; a feature may leave it, and nothing may join it without an entry. When
it is empty every declared feature bites.

    python3 scripts/check-feature-gates-bite.py
    python3 scripts/check-feature-gates-bite.py --selftest
    python3 scripts/check-feature-gates-bite.py --update
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
import tomllib

REPO = pathlib.Path(__file__).resolve().parent.parent
CRATE = REPO / "crates" / "er-quickload"
MANIFEST = CRATE / "Cargo.toml"
SOURCE = CRATE / "src"
BASELINE = REPO / "scripts" / "quickload-feature-bite.baseline.json"

# `feature = "name"` appears in Rust source only inside a cfg predicate: the `#[cfg(...)]` and
# `#[cfg_attr(...)]` attributes and the `cfg!(...)` macro. Matching the predicate rather than the
# attribute keeps `all(...)` / `any(...)` / `not(...)` nesting out of the pattern, which is where a
# regex over attributes would start being wrong.
FEATURE_PREDICATE = re.compile(r'\bfeature\s*=\s*"([A-Za-z0-9_.+-]+)"')

# Non-vacuity floors. A walk that finds nothing prints zeros, and zeros match a baseline of zeros,
# so the all-clear and the broken walk are the same output unless the inputs are asserted first.
# The crate held 128 source files when this was written; the floor sits below that with room for
# the module moves this gate exists to track, and far above what a broken walk returns.
MIN_SOURCE_FILES = 100
MIN_TOTAL_SITES = 1


def declared_features() -> list[str]:
    """The feature names this crate offers, `default` excluded -- it selects, it does not gate."""
    manifest = tomllib.loads(MANIFEST.read_text(encoding="utf-8"))
    return sorted(name for name in manifest.get("features", {}) if name != "default")


def source_files() -> list[pathlib.Path]:
    return sorted(SOURCE.rglob("*.rs"))


def measure(files: list[pathlib.Path]) -> dict[str, int]:
    """How many cfg predicates name each feature, across the crate's own sources."""
    counts: dict[str, int] = {}
    for path in files:
        text = path.read_text(encoding="utf-8", errors="replace")
        for name in FEATURE_PREDICATE.findall(text):
            counts[name] = counts.get(name, 0) + 1
    return counts


def load_baseline() -> dict:
    """The recorded counts, or a clean refusal -- never a traceback.

    An absent baseline is the first run and starts empty: every feature then measures against
    nothing, so a feature gating zero sites is refused for want of an entry, which is the honest
    answer. An unreadable one is a different thing entirely and says so, because the blinded-read
    audit reaches this path and a `JSONDecodeError` traceback names neither the file nor the fix.
    """
    if not BASELINE.exists():
        return {"gated": {}, "ungated": {}}
    raw = BASELINE.read_text(encoding="utf-8")
    try:
        return json.loads(raw)
    except json.JSONDecodeError as err:
        raise SystemExit(
            f"[check-feature-gates-bite] {BASELINE.relative_to(REPO)} is not readable json "
            f"({err}); regenerate it with --update"
        ) from err


def evaluate(
    features: list[str], counts: dict[str, int], baseline: dict, file_count: int
) -> list[str]:
    problems: list[str] = []

    if file_count < MIN_SOURCE_FILES:
        problems.append(
            f"the source walk found {file_count} files under {SOURCE.relative_to(REPO)}; "
            f"under {MIN_SOURCE_FILES} means the walk is broken, not that the crate shrank"
        )
    total = sum(counts.get(name, 0) for name in features)
    if total < MIN_TOTAL_SITES:
        problems.append(
            f"no cfg predicate in the crate names any declared feature; a walk that finds "
            f"nothing cannot tell a gated crate from an ungated one"
        )

    gated = baseline.get("gated", {})
    ungated = baseline.get("ungated", {})

    for name in features:
        measured = counts.get(name, 0)
        if name in gated and name in ungated:
            problems.append(f"{name!r} is recorded both gated and ungated; it is one or the other")
            continue
        if measured == 0:
            if name not in ungated:
                problems.append(
                    f"{name!r} gates nothing and is not recorded in `ungated`: turning it off "
                    f"cannot change the build. Gate something with it, or record why it is "
                    f"still declared."
                )
            continue
        recorded = gated.get(name)
        if recorded is None:
            problems.append(
                f"{name!r} now gates {measured} site(s) and has no `gated` entry; add "
                f"\"{name}\": {measured} so the ratchet is in the diff (--update writes it)"
            )
        elif measured < recorded:
            problems.append(
                f"{name!r} gates {measured} site(s), down from the recorded {recorded}: a gate was "
                f"deleted and the feature silently got wider"
            )
        elif measured > recorded:
            problems.append(
                f"{name!r} gates {measured} site(s), up from the recorded {recorded}: raise the "
                f"baseline in this commit so the gain is reviewed (--update writes it)"
            )

    for name in sorted(set(gated) | set(ungated)):
        if name not in features:
            problems.append(
                f"the baseline records {name!r}, which the manifest no longer declares; drop it"
            )
    return problems


def render(features: list[str], counts: dict[str, int], baseline: dict) -> dict:
    gated = {name: counts[name] for name in features if counts.get(name, 0) > 0}
    keep = baseline.get("ungated", {})
    ungated = {
        name: keep.get(name, "declared, gates nothing yet")
        for name in features
        if counts.get(name, 0) == 0
    }
    return {"gated": dict(sorted(gated.items())), "ungated": dict(sorted(ungated.items()))}


def run() -> int:
    features = declared_features()
    files = source_files()
    counts = measure(files)
    problems = evaluate(features, counts, load_baseline(), len(files))
    if problems:
        print("[check-feature-gates-bite] FAIL:", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1
    biting = sum(1 for name in features if counts.get(name, 0) > 0)
    print(
        f"[check-feature-gates-bite] ok -- {biting} of {len(features)} declared features gate "
        f"{sum(counts.get(n, 0) for n in features)} site(s) across {len(files)} files"
    )
    return 0


def update() -> int:
    features = declared_features()
    counts = measure(source_files())
    BASELINE.write_text(
        json.dumps(render(features, counts, load_baseline()), indent=2) + "\n", encoding="utf-8"
    )
    print(f"wrote {BASELINE.relative_to(REPO)}")
    return 0


def selftest() -> int:
    failures = 0

    def case(label: str, ok: bool) -> None:
        nonlocal failures
        if not ok:
            failures += 1
            print(f"selftest FAIL: {label}", file=sys.stderr)

    features = ["autoload", "quit-rows", "menu-trace"]
    enough = MIN_SOURCE_FILES

    # The real tree is clean, and is the only input that proves the walk itself works.
    live_features = declared_features()
    live_files = source_files()
    live_counts = measure(live_files)
    case("the crate declares features to measure", len(live_features) >= 2)
    case(
        f"the source walk found {len(live_files)} files; it must clear its own floor",
        len(live_files) >= MIN_SOURCE_FILES,
    )
    case(
        "at least one declared feature is measured as gating code, or the walk proves nothing",
        sum(live_counts.get(n, 0) for n in live_features) >= MIN_TOTAL_SITES,
    )
    case("the live tree passes", evaluate(live_features, live_counts, load_baseline(), len(live_files)) == [])

    # A feature that gates nothing and is not recorded is refused.
    case(
        "an ungated, unrecorded feature fails",
        any(
            "gates nothing" in p
            for p in evaluate(features, {"menu-trace": 2}, {"gated": {"menu-trace": 2}, "ungated": {"autoload": "x"}}, enough)
        ),
    )
    # ...and is accepted once recorded, which is what lets the stack land one feature at a time.
    case(
        "an ungated feature recorded with a reason passes",
        evaluate(
            features,
            {"menu-trace": 2},
            {"gated": {"menu-trace": 2}, "ungated": {"autoload": "x", "quit-rows": "y"}},
            enough,
        )
        == [],
    )
    # A deleted gate is the regression this exists to catch.
    case(
        "losing a gate fails",
        any(
            "down from the recorded" in p
            for p in evaluate(
                features,
                {"menu-trace": 1},
                {"gated": {"menu-trace": 2}, "ungated": {"autoload": "x", "quit-rows": "y"}},
                enough,
            )
        ),
    )
    # A gained gate must be recorded in the same commit, or the number stops meaning anything.
    case(
        "gaining a gate without raising the baseline fails",
        any(
            "up from the recorded" in p
            for p in evaluate(
                features,
                {"menu-trace": 3},
                {"gated": {"menu-trace": 2}, "ungated": {"autoload": "x", "quit-rows": "y"}},
                enough,
            )
        ),
    )
    # A feature leaving `ungated` for `gated` needs its entry, not silence.
    case(
        "a newly gating feature with no entry fails",
        any(
            "has no `gated` entry" in p
            for p in evaluate(
                features,
                {"menu-trace": 2, "quit-rows": 4},
                {"gated": {"menu-trace": 2}, "ungated": {"autoload": "x", "quit-rows": "y"}},
                enough,
            )
        ),
    )
    # Vacuity: a broken walk must not read as an all-clear.
    case(
        "a walk that finds no files fails",
        any(
            "the walk is broken" in p
            for p in evaluate(features, {}, {"gated": {}, "ungated": {n: "x" for n in features}}, 3)
        ),
    )
    case(
        "a walk that finds no cfg predicate at all fails",
        any(
            "cannot tell a gated crate from an ungated one" in p
            for p in evaluate(features, {}, {"gated": {}, "ungated": {n: "x" for n in features}}, enough)
        ),
    )
    # A stale baseline entry for a deleted feature is refused rather than ignored.
    case(
        "a baseline entry the manifest no longer declares fails",
        any(
            "the manifest no longer declares" in p
            for p in evaluate(
                features,
                {"menu-trace": 2},
                {"gated": {"menu-trace": 2, "gone": 1}, "ungated": {"autoload": "x", "quit-rows": "y"}},
                enough,
            )
        ),
    )
    # The predicate pattern reads the nesting a cfg really uses.
    nested = 'a\n#[cfg(all(windows, feature = "quit-rows", not(feature = "autoload")))]\nb'
    case(
        "the predicate pattern reads features out of nested cfg predicates",
        FEATURE_PREDICATE.findall(nested) == ["quit-rows", "autoload"],
    )

    if failures:
        print(f"selftest: {failures} case(s) failed", file=sys.stderr)
        return 1
    print(
        f"[check-feature-gates-bite] selftest ok -- {len(live_features)} declared features, "
        f"{len(live_files)} source files, "
        f"{sum(live_counts.get(n, 0) for n in live_features)} gating site(s) measured"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--update", action="store_true", help="rewrite the baseline from the tree")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if args.update:
        return update()
    return run()


if __name__ == "__main__":
    sys.exit(main())
