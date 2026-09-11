#!/usr/bin/env python3
"""Refuse a 1.16.2 -> 1.17 ledger that names the wrong kind of memory.

What this catches, and what it already caught
---------------------------------------------
Every address ledger `er-game-base/build.rs` reads licenses one of two operations, and the two
need opposite kinds of address. The function ledgers say "this FUNCTION moved here" -- callable,
and for the two that reach `detourable_pairs`, a place MinHook may write five bytes -- which is
only meaningful in EXECUTABLE memory. `rva-map-1162-to-1170.data.tsv` says "this global moved
here", which is only meaningful outside it. Nothing in the tree checked either claim against the
image's own section table until 2026-08-31.

Measured that day on `docs/recon/rva-1170-detour-audited.tsv`, since deleted. As it stood in HEAD:
444 promoted rows, of which 87 named non-EXECUTABLE destinations -- 61 in `.data`, 26 in `.rdata`
-- each carrying a prologue verdict like `6B relocatable`. Of the 85 promoted on its "unwindless
leaf" clause, all 85 were non-executable, so that clause never once fired on a real leaf.
Regenerating the file from its own current inputs added four more rows of exactly that shape,
taking it to 448 / 91 / 89.

The cause is structural and not a bug in its decoder: `.pdata` declares no function containing a
`.data` global for exactly the same reason it declares none containing an unwindless leaf, so "no
enclosing function" cannot separate the two. Those four added rows are 24 bytes of zeros in both
images, decoded as `add [rax], al` three times and reported `6B relocatable`.

The live ledgers were clean when this gate was written, and that is the point of writing it: 103
`verified.tsv` rows, 414 `needed-verified.tsv` rows and 414 `needed.tsv` rows are 100% `.text`,
and 116 `data.tsv` rows are 100% `.data`/`.rdata`. A gate that arrives green on 1047 rows and
would have been red on 87 is a gate that separates, not one that fires on everything.

Two rules
--------
  R1  code ledger    a row naming a function -- callable, and for two of the three ledgers
                     hookable -- must name a destination in an EXECUTABLE section. Applied to
                     every pair row, not only the rows `build.rs` currently admits as detourable:
                     a superset, so no admission logic is transcribed here and none can drift out
                     of sync with build.rs.
  R2  data ledger    a row in the globals ledger must name a non-EXECUTABLE destination. A code
                     address filed as a global is how a `read` becomes a call to the wrong thing.

There was a third rule, and its removal is the point
-----------------------------------------------------
R3 was a TOMBSTONE: `docs/recon/rva-1170-detour-audited.tsv` must not exist. It was enforcement
standing in for a removal that this gate's own change did not own -- `audit-1170-hook-targets.py
--promote` still wrote that path, so one command resurrected the file, and R1 could not see it
because a file `build.rs` does not declare is not a ledger this gate enumerates. `--promote` and
its two helpers were deleted on 2026-08-31, so R3 was deleted with them. A rule that guards
against a command nobody can run is not spare coverage; it is a line that reads as coverage while
matching nothing, which is the exact failure mode the rest of this file exists to refuse.

How the LEDGERS are found
-------------------------
Parsed out of `crates/er-game-base/build.rs`, never transcribed, exactly as
`check-no-duplicate-ledger-rows.py` does it and for the same reason: a copied literal is how nine
audits in this repo printed a confident zero. A ledger constant found there that the table below
does not classify stops the run (exit 2). A partial view reporting zero violations is this defect
class wearing a green tick.

The image is GITIGNORED
-----------------------
`eldenring-deobf-1.17.bin` is game-derived and not committed, so R1/R2 can only run where it
exists. When it does not, they are skipped out loud, in a line that refuses the words OK and
pass, and the summary names what was not checked. `--selftest` builds its own synthetic image and
therefore runs everywhere, which is what `check.sh` wires.

Usage
    python3 scripts/check-ledger-section-kind.py             # the gate
    python3 scripts/check-ledger-section-kind.py --rows      # also print the per-ledger tally
    python3 scripts/check-ledger-section-kind.py --selftest  # positive controls, synthetic image
"""

from __future__ import annotations

import argparse
import os
import re
import struct
import sys
import tempfile

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILD_RS = os.path.join(REPO, "crates", "er-game-base", "build.rs")
BASE = 0x140000000


def _resolve_image(env_var, filename):
    """Locate a deobf image: explicit env override, then this checkout, then the main worktree.

    Same resolution `scripts/map-rvas-1162-to-1170.py` uses, and here for the same reason. The
    image is a gitignored multi-hundred-MB reverse engineering input that lives beside the primary
    checkout and is never copied per worktree, so an agent adding a ledger row from a `git
    worktree` saw R1/R2 skip -- a gate reporting that it did not run, on the one edit it exists to
    check. `ER_DEOBF_BIN_1170` is the spelling the mapper already takes for this file.
    """
    override = os.environ.get(env_var)
    if override:
        return override
    local = os.path.join(REPO, filename)
    if os.path.exists(local):
        return local
    # `git rev-parse --git-common-dir` resolves to the primary checkout's `.git` from inside a
    # linked worktree, and to our own otherwise, so its parent is the main working tree.
    try:
        import subprocess

        common = subprocess.run(
            ["git", "-C", REPO, "rev-parse", "--git-common-dir"],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        if common.returncode == 0:
            main_root = os.path.dirname(os.path.abspath(os.path.join(REPO, common.stdout.strip())))
            candidate = os.path.join(main_root, filename)
            if os.path.exists(candidate):
                return candidate
    except Exception:
        pass
    return local


IMAGE_1170 = _resolve_image("ER_DEOBF_BIN_1170", "eldenring-deobf-1.17.bin")

# What kind of memory a ledger's destination column is allowed to name.
CODE = "code"  # rows that can license a five-byte patch: must be executable
DATA = "data"  # globals: must not be executable
NO_PAIRS = "no-pairs"  # single-column (`quarantined()` reads column 0); there is no destination

# Keyed on BASENAME, because build.rs spells the paths relative to its own crate dir.
LEDGER_KIND = {
    "rva-map-1162-to-1170.verified.tsv": CODE,
    "rva-map-1162-to-1170.needed.tsv": CODE,
    "rva-map-1162-to-1170.needed-verified.tsv": CODE,
    "rva-map-1162-to-1170.data.tsv": DATA,
    "rva-1170-quarantine.tsv": NO_PAIRS,
}

# `const NAME: &str = "../../docs/recon/whatever.tsv";` in build.rs.
LEDGER_CONST = re.compile(r'const\s+([A-Z0-9_]+)\s*:\s*&str\s*=\s*"([^"]*\.tsv)"')



class Refuse(Exception):
    """A condition under which no verdict is available, as distinct from a clean tree."""


# --------------------------------------------------------------------------------------------
# the image's own section table
# --------------------------------------------------------------------------------------------
def sections(blob: bytes) -> list[tuple[str, int, int, bool]]:
    """`[(name, rva, size, executable)]` from the PE headers of a flat image.

    Flat means file offset == RVA for every section, so the headers sit where the loader would
    have put them and no raw-pointer mapping is applied. Size is `max(virtual, raw)`: the 1.17
    `.data` declares 0xd51bc4 virtual against 0x249e00 raw, and the zero-filled tail beyond the
    raw size is exactly where the deleted ledger's four all-zero "leaf functions" lived. Taking
    the raw size alone would place them outside every section and lose the finding.
    """
    if len(blob) < 0x40:
        raise Refuse("image is too short to carry a PE header")
    e_lfanew = struct.unpack_from("<I", blob, 0x3C)[0]
    if e_lfanew + 24 > len(blob) or blob[e_lfanew : e_lfanew + 4] != b"PE\0\0":
        raise Refuse("image does not start with a PE signature at e_lfanew")
    count = struct.unpack_from("<H", blob, e_lfanew + 6)[0]
    opt_size = struct.unpack_from("<H", blob, e_lfanew + 20)[0]
    table = e_lfanew + 24 + opt_size
    out = []
    for i in range(count):
        off = table + i * 40
        if off + 40 > len(blob):
            raise Refuse(f"section header {i} runs past the end of the image")
        name = blob[off : off + 8].rstrip(b"\0").decode("ascii", "replace")
        virtual_size, rva, raw_size, _raw_ptr = struct.unpack_from("<IIII", blob, off + 8)
        characteristics = struct.unpack_from("<I", blob, off + 36)[0]
        # IMAGE_SCN_MEM_EXECUTE. Not IMAGE_SCN_CNT_CODE: what decides whether a detour can run
        # there is the page protection the loader applies, and that is this bit.
        out.append((name, rva, max(virtual_size, raw_size), bool(characteristics & 0x20000000)))
    if not out:
        raise Refuse("image declares no sections")
    return out


def classify(secs, va: int) -> tuple[str, bool]:
    """`(section name, executable)` for a VA, or `('<outside>', False)`.

    An address in no section is reported and treated as a violation for both kinds: it is not
    executable, and it is not a global either -- it is not in the image at all.
    """
    rva = va - BASE
    for name, start, size, executable in secs:
        if start <= rva < start + size:
            return name, executable
    return "<outside>", False


# --------------------------------------------------------------------------------------------
# the ledgers
# --------------------------------------------------------------------------------------------
def ledger_paths(build_rs: str = BUILD_RS):
    """`([(const, abs_path, kind)], [unclassified])`, read out of build.rs."""
    with open(build_rs, encoding="utf-8", errors="replace") as handle:
        text = handle.read()
    found, unknown = [], []
    for const_name, relative in LEDGER_CONST.findall(text):
        kind = LEDGER_KIND.get(os.path.basename(relative))
        if kind is None:
            unknown.append(f"{const_name} -> {relative}")
            continue
        found.append(
            (
                const_name,
                os.path.normpath(os.path.join(os.path.dirname(build_rs), relative)),
                kind,
            )
        )
    return found, unknown


def destinations(path: str) -> list[tuple[int, int, int]]:
    """`[(line_no, source_va, destination_va)]` for every pair row of one ledger.

    Both spellings occur and both are accepted, as `build.rs` accepts them: `verified.tsv` and
    `needed-verified.tsv` write full VAs, `needed.tsv` and `data.tsv` write RVAs. Anything at or
    above the preferred image base is already a VA.
    """
    rows = []
    if not os.path.isfile(path):
        return rows
    with open(path, encoding="utf-8", errors="replace") as handle:
        for line_no, line in enumerate(handle, 1):
            if line.startswith("#") or not line.strip():
                continue
            fields = line.rstrip("\n").split("\t")
            if len(fields) < 2:
                continue
            try:
                source = int(fields[0].strip(), 16)
                destination = int(fields[1].strip(), 16)
            except ValueError:
                continue
            rows.append(
                (
                    line_no,
                    source if source >= BASE else source + BASE,
                    destination if destination >= BASE else destination + BASE,
                )
            )
    return rows


def check(image_path: str, build_rs: str = BUILD_RS, show_rows=False):
    """`(findings, notes)`. An empty `findings` is a green gate; `notes` are printed either way."""
    findings: list[str] = []
    notes: list[str] = []

    ledgers, unknown = ledger_paths(build_rs)
    if unknown:
        raise Refuse(
            "build.rs declares a ledger this gate does not classify: "
            + ", ".join(sorted(unknown))
            + "\n  Add it to LEDGER_KIND as CODE (destinations must be executable), DATA\n"
            "  (must not be), or NO_PAIRS. Guessing would let a whole ledger go unchecked\n"
            "  while this prints a green zero."
        )
    if not ledgers:
        raise Refuse(f"no ledger constants found in {os.path.relpath(build_rs, REPO)}")

    if not os.path.isfile(image_path):
        notes.append(
            f"SKIPPED R1/R2: {os.path.relpath(image_path, REPO)} is absent (gitignored, "
            f"game-derived). {len(ledgers)} ledger(s) went unchecked against the section table."
        )
        return findings, notes

    with open(image_path, "rb") as handle:
        blob = handle.read()
    secs = sections(blob)

    for const_name, path, kind in ledgers:
        if kind == NO_PAIRS:
            continue
        shown = os.path.relpath(path, REPO)
        rows = destinations(path)
        want_executable = kind == CODE
        bad = []
        tally: dict[str, int] = {}
        for line_no, source, destination in rows:
            name, executable = classify(secs, destination)
            tally[name] = tally.get(name, 0) + 1
            if executable != want_executable:
                bad.append((line_no, source, destination, name))
        if show_rows:
            spread = ", ".join(f"{n}={c}" for n, c in sorted(tally.items(), key=lambda kv: -kv[1]))
            notes.append(f"{shown}: {len(rows)} pair row(s) [{const_name}, {kind}] -- {spread}")
        for line_no, source, destination, name in bad:
            rule = "R1 CODE" if want_executable else "R2 DATA"
            wanted = "executable" if want_executable else "non-executable"
            findings.append(
                f"{rule}  {shown}:{line_no}  0x{source:x} -> 0x{destination:x} lands in "
                f"{name}, which is not {wanted}."
            )
    return findings, notes


# --------------------------------------------------------------------------------------------
# selftest
# --------------------------------------------------------------------------------------------
def _synthetic_image(exec_rva=0x1000, exec_size=0x1000, data_rva=0x2000, data_size=0x1000) -> bytes:
    """A minimal flat PE with one executable section and one that is not.

    Built rather than borrowed so the selftest runs where the gitignored 98 MB image does not
    exist -- which is every checkout `check.sh` runs in.
    """
    e_lfanew = 0x80
    opt_size = 0xF0
    blob = bytearray(0x1000)
    blob[0:2] = b"MZ"
    struct.pack_into("<I", blob, 0x3C, e_lfanew)
    blob[e_lfanew : e_lfanew + 4] = b"PE\0\0"
    struct.pack_into("<H", blob, e_lfanew + 6, 2)  # NumberOfSections
    struct.pack_into("<H", blob, e_lfanew + 20, opt_size)
    table = e_lfanew + 24 + opt_size
    for i, (name, rva, size, characteristics) in enumerate(
        [(b".text", exec_rva, exec_size, 0x60000020), (b".data", data_rva, data_size, 0xC0000040)]
    ):
        off = table + i * 40
        blob[off : off + 8] = name.ljust(8, b"\0")
        struct.pack_into("<IIII", blob, off + 8, size, rva, size, rva)
        struct.pack_into("<I", blob, off + 36, characteristics)
    return bytes(blob)


def selftest() -> int:
    """Plant each violation this gate claims to catch, and one legitimate lookalike."""
    failures = []

    def expect(label, got, want):
        ok = got == want
        print(f"  {label:<46} {'PASS' if ok else 'FAIL'}  (got {got!r}, want {want!r})")
        if not ok:
            failures.append(label)

    with tempfile.TemporaryDirectory() as tmp:
        image = os.path.join(tmp, "image.bin")
        with open(image, "wb") as handle:
            handle.write(_synthetic_image())
        crate = os.path.join(tmp, "crates", "er-game-base")
        recon = os.path.join(tmp, "docs", "recon")
        os.makedirs(crate)
        os.makedirs(recon)
        build_rs = os.path.join(crate, "build.rs")
        code_path = os.path.join(recon, "rva-map-1162-to-1170.verified.tsv")
        data_path = os.path.join(recon, "rva-map-1162-to-1170.data.tsv")

        def write_build_rs(extra=""):
            with open(build_rs, "w", encoding="utf-8") as handle:
                handle.write(
                    'const VERIFIED_MAP: &str = "../../docs/recon/'
                    'rva-map-1162-to-1170.verified.tsv";\n'
                    'const DATA_MAP: &str = "../../docs/recon/'
                    'rva-map-1162-to-1170.data.tsv";\n' + extra
                )

        def write(path, rows):
            with open(path, "w", encoding="utf-8") as handle:
                handle.write("# header\n" + "".join(rows))

        good_code = "0x140001000\t0x140001100\tIDENTICAL-WHOLE\t1.000\t9\tX\tBOTH-ENTRIES\tP\n"
        good_data = "0x3b15008\t0x2008\tCARRIED\tagree\n"
        write_build_rs()

        # Specificity: the shapes the live tree actually has must stay green.
        write(code_path, [good_code])
        write(data_path, [good_data])
        findings, _ = check(image, build_rs)
        expect("spec/correct-kinds-green", len(findings), 0)

        # R1: a hook-licensing row pointing into non-executable memory.
        write(code_path, [good_code, "0x140001040\t0x140002040\tIDENTICAL-WHOLE\t1.0\t9\tY\tB\tP\n"])
        findings, _ = check(image, build_rs)
        expect("sens/R1-code-row-in-.data", len(findings), 1)
        expect("sens/R1-names-the-address", "0x140002040" in (findings[0] if findings else ""), True)
        write(code_path, [good_code])

        # R1 again: a destination in no section at all is not executable either.
        write(code_path, [good_code, "0x140001040\t0x140099000\tIDENTICAL-WHOLE\t1.0\t9\tY\tB\tP\n"])
        findings, _ = check(image, build_rs)
        expect("sens/R1-outside-every-section", len(findings), 1)
        write(code_path, [good_code])

        # R2: a global that is actually a code address.
        write(data_path, [good_data, "0x3b15010\t0x1200\tCARRIED\tagree\n"])
        findings, _ = check(image, build_rs)
        expect("sens/R2-data-row-in-.text", len(findings), 1)
        write(data_path, [good_data])

        # An unclassified ledger stops the run rather than being skipped.
        write_build_rs('const NEW_MAP: &str = "../../docs/recon/rva-1170-brand-new.tsv";\n')
        try:
            check(image, build_rs)
            expect("sens/unclassified-ledger-refuses", "returned", "raised Refuse")
        except Refuse as exc:
            expect("sens/unclassified-ledger-refuses", "NEW_MAP" in str(exc), True)
        write_build_rs()

        # A missing image must skip, not silently pass R1/R2 with a green tick.
        write(code_path, [good_code, "0x140001040\t0x140002040\tIDENTICAL-WHOLE\t1.0\t9\tY\tB\tP\n"])
        findings, notes = check(os.path.join(tmp, "absent.bin"), build_rs)
        expect("spec/absent-image-skips-loudly", (len(findings), len(notes)), (0, 1))
        expect(
            "spec/absent-image-says-what-it-skipped",
            bool(notes) and "SKIPPED R1/R2" in notes[0],
            True,
        )

    print(f"\n{'FAILED: ' + ', '.join(failures) if failures else 'selftest OK'}")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--rows", action="store_true", help="print the per-ledger section tally")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    try:
        findings, notes = check(IMAGE_1170, show_rows=args.rows)
    except Refuse as exc:
        print(f"check-ledger-section-kind: REFUSING TO VERDICT\n  {exc}", file=sys.stderr)
        return 2
    for note in notes:
        print(note)
    if findings:
        print(
            f"\ncheck-ledger-section-kind: {len(findings)} violation(s) -- a ledger names the "
            f"wrong kind of memory:\n",
            file=sys.stderr,
        )
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("check-ledger-section-kind: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
