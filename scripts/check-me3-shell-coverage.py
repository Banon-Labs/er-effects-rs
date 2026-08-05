#!/usr/bin/env python3
"""Every ME3-loadable DLL must be LINKED by a gate. This proves the list that does it is complete.

`scripts/check-rust-build.sh` carries an `me3_shells` array of `package:artifact` pairs and links
each one, because `cargo xwin check` stops at metadata and never invokes the linker, and the plain
`cargo xwin build` honours `default-members` (= `crates/er-effects-rs`) and so builds only the
product. Before that array existed, 13 of the cdylibs a user can list in an me3 `[[natives]]` entry
were never linked by ANY gate: a shell that could not link -- a missing `#[no_mangle] DllMain`, a
wrong crate-type, an unresolved import inside a `cfg(windows)` block -- passed the whole suite
while being unloadable.

The array closed that. What it did NOT close is the array itself going stale: it was kept correct
by a comment saying "keep this list in sync", which is not enforcement. Add a new DLL crate and the
gate stays green while never linking it -- the exact hole, one level up.

So this checks three things:

1. **Coverage.** Every cdylib crate that defines a `DllMain` is either in the array or is the
   product (which the default build already links) -- or is explicitly exempt here WITH a reason.
2. **Artifact names.** The `.dll` a package produces is NOT reliably its name with dashes swapped
   for underscores: four crates override `[lib] name`, and deriving the filename instead of listing
   it silently skipped all four. Each pair's artifact must match what Cargo will actually emit.
3. **No dead entries.** A listed package must exist and still be a cdylib, so a rename or a
   crate-type change cannot leave the array pointing at nothing while looking maintained.

Usage:
    python3 scripts/check-me3-shell-coverage.py
    python3 scripts/check-me3-shell-coverage.py --selftest

Exit status is 1 on any failure, so this can gate.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
BUILD_SCRIPT = REPO_ROOT / "scripts" / "check-rust-build.sh"
CRATES_DIR = REPO_ROOT / "crates"

# The product DLL. `cargo xwin build --release` builds it via `default-members`, so it is linked
# without appearing in the shells array.
PRODUCT_PACKAGE = "er-effects-rs"

# cdylib crates that define no `DllMain` and are therefore not ME3-loadable natives. Each needs a
# reason: an unexplained exemption is how a real shell gets quietly dropped from the gate.
EXEMPT: dict[str, str] = {
    "er-ags-stub": "AMD AGS shim loaded by the game as amd_ags_x64.dll, not an me3 native",
    "er-flver": "FLVER tooling library; the cdylib is for host tooling, not for injection",
    "er-hook": "shared MinHook union consumed by the shells, never loaded on its own",
    "er-save-suppress": "save-suppression helper library, not an me3 native",
    "er-shaderkit": "shader tooling library, not an me3 native",
}


def parse_me3_shells(script_text: str) -> list[tuple[str, str]]:
    """Pull the `package:artifact` pairs out of the `me3_shells=( ... )` array."""
    match = re.search(r"me3_shells=\(\s*(.*?)\s*\)", script_text, re.S)
    if not match:
        return []
    pairs = []
    for token in match.group(1).split():
        token = token.strip()
        if not token or token.startswith("#"):
            continue
        if ":" not in token:
            continue
        package, artifact = token.split(":", 1)
        pairs.append((package, artifact))
    return pairs


def cdylib_crates(crates_dir: Path) -> dict[str, str]:
    """Map package name -> the artifact stem Cargo will emit, for every cdylib crate."""
    found: dict[str, str] = {}
    for manifest in sorted(crates_dir.glob("*/Cargo.toml")):
        text = manifest.read_text(encoding="utf-8", errors="replace")
        if "cdylib" not in text:
            continue
        package_match = re.search(r"^name\s*=\s*\"([^\"]+)\"", text, re.M)
        if not package_match:
            continue
        package = package_match.group(1)
        # A `[lib] name` override decides the artifact; otherwise Cargo derives it from the
        # package name with dashes turned into underscores.
        lib_match = re.search(r"\[lib\](.*?)(?:\n\[|\Z)", text, re.S)
        artifact = package.replace("-", "_")
        if lib_match:
            lib_name = re.search(r"^\s*name\s*=\s*\"([^\"]+)\"", lib_match.group(1), re.M)
            if lib_name:
                artifact = lib_name.group(1)
        found[package] = artifact
    return found


def defines_dllmain(crates_dir: Path, package: str) -> bool:
    """Whether any source file in the crate defines a `DllMain` entry point."""
    crate_dir = crates_dir / package
    if not crate_dir.is_dir():
        # Package name and directory name can differ; fall back to a manifest scan.
        for manifest in crates_dir.glob("*/Cargo.toml"):
            text = manifest.read_text(encoding="utf-8", errors="replace")
            if re.search(rf"^name\s*=\s*\"{re.escape(package)}\"", text, re.M):
                crate_dir = manifest.parent
                break
        else:
            return False
    for source in crate_dir.glob("src/**/*.rs"):
        if re.search(r"fn\s+DllMain", source.read_text(encoding="utf-8", errors="replace")):
            return True
    return False


def check(crates_dir: Path, script_text: str) -> list[str]:
    """Return a list of failures; empty means the shells array is complete and accurate."""
    failures: list[str] = []
    listed = parse_me3_shells(script_text)
    if not listed:
        return ["could not parse the me3_shells array out of check-rust-build.sh"]
    listed_packages = {package for package, _ in listed}
    crates = cdylib_crates(crates_dir)

    # 1. Coverage.
    for package in sorted(crates):
        if package in listed_packages or package == PRODUCT_PACKAGE:
            continue
        if not defines_dllmain(crates_dir, package):
            if package not in EXEMPT:
                failures.append(
                    f"{package}: cdylib with no DllMain and no exemption -- add it to EXEMPT "
                    f"with a reason, or to me3_shells if it is loadable"
                )
            continue
        failures.append(
            f"{package}: defines DllMain but is NOT in me3_shells, so no gate links it. "
            f"Add `{package}:{crates[package]}` to the array in check-rust-build.sh."
        )

    # 2. Artifact names.
    for package, artifact in listed:
        expected = crates.get(package)
        if expected is None:
            continue  # reported by check 3
        if artifact != expected:
            failures.append(
                f"{package}: me3_shells says the artifact is `{artifact}.dll` but Cargo emits "
                f"`{expected}.dll` -- the existence assertion would look for the wrong file"
            )

    # 3. No dead entries.
    for package, _ in listed:
        if package not in crates:
            failures.append(
                f"{package}: listed in me3_shells but is not a cdylib crate (renamed, removed, "
                f"or its crate-type changed)"
            )

    # An exemption for a crate that has since grown a DllMain is worse than no exemption: it reads
    # as considered and silently drops a real shell.
    for package in sorted(EXEMPT):
        if package in crates and defines_dllmain(crates_dir, package):
            failures.append(
                f"{package}: EXEMPT here, but it now defines DllMain -- it is loadable and must "
                f"move into me3_shells"
            )
    return failures


def selftest() -> int:
    """Prove the checks actually fire, using synthetic inputs rather than the live tree."""
    import tempfile

    failures = 0

    def case(name: str, condition: bool) -> None:
        nonlocal failures
        if not condition:
            print(f"selftest FAIL: {name}", file=sys.stderr)
            failures += 1

    case(
        "parses pairs",
        parse_me3_shells("me3_shells=(\n\ta-dll:a_dll\n\tb-dll:b_lib\n)")
        == [("a-dll", "a_dll"), ("b-dll", "b_lib")],
    )
    case("empty array parses to nothing", parse_me3_shells("me3_shells=(\n)") == [])
    case("missing array is detected", parse_me3_shells("nothing here") == [])

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        # A loadable shell that the array forgot.
        (root / "forgotten-dll" / "src").mkdir(parents=True)
        (root / "forgotten-dll" / "Cargo.toml").write_text(
            '[package]\nname = "forgotten-dll"\n[lib]\ncrate-type = ["cdylib"]\n'
        )
        (root / "forgotten-dll" / "src" / "lib.rs").write_text(
            "#[no_mangle]\npub extern \"system\" fn DllMain() -> i32 { 1 }\n"
        )
        problems = check(root, "me3_shells=(\n\tother-dll:other_dll\n)")
        case(
            "an unlisted DllMain cdylib fails",
            any("forgotten-dll" in p and "NOT in me3_shells" in p for p in problems),
        )

        # An artifact-name override the array got wrong -- the trap that silently skipped four.
        (root / "renamed-dll" / "src").mkdir(parents=True)
        (root / "renamed-dll" / "Cargo.toml").write_text(
            '[package]\nname = "renamed-dll"\n[lib]\nname = "renamed_artifact"\ncrate-type = ["cdylib"]\n'
        )
        (root / "renamed-dll" / "src" / "lib.rs").write_text(
            "#[no_mangle]\npub extern \"system\" fn DllMain() -> i32 { 1 }\n"
        )
        problems = check(root, "me3_shells=(\n\trenamed-dll:renamed_dll\n)")
        case(
            "a wrong artifact name fails",
            any("renamed-dll" in p and "Cargo emits" in p for p in problems),
        )

        problems = check(root, "me3_shells=(\n\trenamed-dll:renamed_artifact\n)")
        case(
            "the correct artifact name passes that check",
            not any("Cargo emits" in p for p in problems),
        )

        problems = check(root, "me3_shells=(\n\tghost-dll:ghost_dll\n)")
        case(
            "a dead entry fails",
            any("ghost-dll" in p and "not a cdylib crate" in p for p in problems),
        )

    if failures:
        print(f"selftest: {failures} case(s) failed", file=sys.stderr)
        return 1
    print("[check-me3-shell-coverage] selftest ok (7 cases)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    if not BUILD_SCRIPT.exists():
        print(f"missing {BUILD_SCRIPT}", file=sys.stderr)
        return 1
    failures = check(CRATES_DIR, BUILD_SCRIPT.read_text(encoding="utf-8", errors="replace"))
    if failures:
        print("[check-me3-shell-coverage] FAIL:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1
    listed = len(parse_me3_shells(BUILD_SCRIPT.read_text(encoding="utf-8", errors="replace")))
    print(
        f"[check-me3-shell-coverage] ok -- {listed} me3 shells listed and linked, "
        f"{len(EXEMPT)} cdylibs exempt with reasons, 0 loadable DLLs missing from the gate"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
