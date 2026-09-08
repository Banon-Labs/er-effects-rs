#!/usr/bin/env python3
"""Sweep 2 detector: writes derived from a game-image RVA that no hook audit can see.

MinHook-shaped audits look at detour installs. These four classes never touch MinHook,
so nothing had ever checked them:

  Slot     a store into a function-pointer table / vtable slot (`*(slot as *mut usize) = ...`)
  RVA_ARG  a byte-patch primitive that takes `(base, rva)` as separate arguments -- invisible
           to any regex looking for the text `base + SOMETHING_RVA`
  sentinel `game_data_addr(..) + offset`, which destroys the `0`-means-refused sentinel:
           a refusal on row 3 yields the address 24, and `if addr != 0` waves it through
  raw      a raw store whose address expression contains `base + *_rva` in any casing

Every class has a positive control in `--selftest`, because a detector that reports zero
is indistinguishable from a detector that matches nothing: `audit-1170-readiness.py`
reported zero ungated writes while six existed, and its `HAND_BUILT` pattern required an
uppercase `RVA` so `base + spec.rva` matched 0 of 40 real sites.

Exits 1 on any finding (since 2026-08-31). It used to only report, and it reported six
findings that were every one of them false: two were the primitives' own `fn` declarations
and four were call sites of primitives that gate internally. With those two matcher faults
fixed the true count is zero, so the class can be held at zero instead of narrated.
The complementary ratchet is `scripts/audit-1170-readiness.py`.
"""
from __future__ import annotations

import glob
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rustfn import mask_source  # noqa: E402

GATED = re.compile(
    r"game_data_addr(?:_offset)?\s*\(|resolve_game_address(?:_fmt)?\s*\(|resolve_detour_address\s*\("
    r"|game_rva(?:_named)?\s*\(|write_global_u8\s*\("
)
# A store, in the spellings this tree actually uses.
STORE = re.compile(
    r"\*\s*\(?[^;=]{0,200}?as\s+\*mut\s+[A-Za-z0-9_:<> ]+\s*\)?\s*=(?!=)"
    r"|\)\s*\.\s*write(?:_volatile|_unaligned)?\s*\("
    r"|\bcopy_nonoverlapping\s*\("
)
# `base + FOO_RVA` / `base + spec.rva` / `module_base + rva` -- lowercase included on purpose.
IMAGE_ADDR = re.compile(
    r"\b(?:base|image_base|module_base|game_base|module)\s*\+\s*"
    r"((?:[A-Za-z_][A-Za-z0-9_]*(?:::|\.))*"
    r"(?:[A-Za-z0-9_]*RVA[A-Za-z0-9_]*|(?:[a-z0-9_]*_)?rva(?:_[a-z0-9_]+)?))"
)
# The byte-patch primitives, which take `(base, rva)` as arguments and so carry no `base + rva`
# text for IMAGE_ADDR to find.
RVA_ARG_CALL = re.compile(r"\b(patch_3byte_stub|apply_xor_ret_stub)\s*\(")
# `fn patch_3byte_stub(` is the definition, not a call. It matched RVA_ARG until 2026-08-31 and
# accounted for 2 of the 6 findings -- the same self-match that put four er-hook wrappers on
# audit-installer-partial-failure's list.
RVA_ARG_DEF = re.compile(r"\bfn\s+$")
# Where the gate actually lives (2026-08-31). The other four RVA_ARG findings were the four real
# call sites in `er-title-flow`, and every one of them was already safe: `patch_3byte_stub` and
# `apply_xor_ret_stub` each open by resolving `base + rva` through
# `er_game_base::game_build::resolve_game_address`, refuse on `None`, then audit the site with
# `detour_site::write_site_is_sound` and confirm the expected first byte. The gate is inside the
# primitive, which is the right place for it -- a per-call-site gate would be four copies of one
# rule, and the class as originally written could not distinguish "ungated" from "gated somewhere
# this regex cannot see", so it reported all six as findings and gated nothing.
#
# The invariant that is actually worth failing a build over is therefore not "does this call site
# gate" but "does the primitive still gate". Delete that `resolve_game_address` line and all four
# call sites silently become raw writes into a 1.17 image at 1.16.2 offsets, with no other check in
# the tree noticing. So: a call site is reported only when the primitive it calls has lost its
# gate, and a primitive whose definition is not in the scanned set is treated as ungated
# (fail-closed -- an audit that cannot see the definition must not vouch for it).
PRIMITIVE_NAMES = ("patch_3byte_stub", "apply_xor_ret_stub")
PRIMITIVE_GATE = re.compile(r"resolve_game_address(?:_fmt)?\s*\(")
SENTINEL = re.compile(r"game_data_addr\s*\(")
SLOT_LHS = re.compile(
    r"\*\s*\(?\s*\(?\s*([A-Za-z_][A-Za-z0-9_]*)(?:\s*\+[^)]*)?\)?\s*as\s+\*mut\s+[A-Za-z0-9_:<> ]+\s*\)?\s*=(?!=)"
    r"|\(\s*([A-Za-z_][A-Za-z0-9_]*)(?:\s*\+[^)]*)?\)?\s*as\s+\*mut\s+[A-Za-z0-9_:<> ]+\s*\)\s*\.\s*write"
)
SLOT_NAME = re.compile(r"vtable|vtbl|vft|vfptr|slot|table|entry|thunk|dispatch", re.I)
SKIP_DIR = re.compile(r"/(tests|examples|benches)/|/tests\.rs$|^third_party/")

WINDOW = 4  # lines: a store's address can be bound a few lines above it


def close_paren(mask: str, open_index: int) -> int:
    depth = 1
    i = open_index + 1
    while i < len(mask) and depth:
        if mask[i] == "(":
            depth += 1
        elif mask[i] == ")":
            depth -= 1
        i += 1
    return i


def primitive_gating_sources(sources) -> dict[str, bool]:
    """`{primitive_name: does its own definition resolve through the version gate}`.

    Read from the definitions themselves rather than assumed, so that removing the
    `resolve_game_address` call from either primitive turns every call site into a finding.
    """
    gated: dict[str, bool] = {}
    for src in sources:
        if not any(f"fn {n}" in src for n in PRIMITIVE_NAMES):
            continue
        mask = mask_source(src)
        for m in re.finditer(r"\bfn\s+(" + "|".join(PRIMITIVE_NAMES) + r")\s*\(", mask):
            brace = mask.find("{", m.end())
            if brace < 0:
                continue
            depth, i = 0, brace
            while i < len(mask):
                if mask[i] == "{":
                    depth += 1
                elif mask[i] == "}":
                    depth -= 1
                    if depth == 0:
                        break
                i += 1
            gated[m.group(1)] = bool(PRIMITIVE_GATE.search(mask[brace:i]))
    return gated


def primitive_gating(paths) -> dict[str, bool]:
    """`primitive_gating_sources` over the files at `paths`, skipping unreadable ones."""

    def readable(paths):
        for p in paths:
            try:
                yield open(p, encoding="utf-8", errors="replace").read()
            except OSError:
                continue

    return primitive_gating_sources(readable(paths))


def scan_text(path: str, src: str, gating: dict[str, bool] | None = None) -> list[tuple[str, int, str, str]]:
    """Return (class, line, symbol, source_line) findings for one file's text."""
    out = []
    mask = mask_source(src)
    lines = src.splitlines()
    mlines = mask.splitlines()
    gating = {} if gating is None else gating

    # RVA_ARG: a call to a primitive that takes (base, rva) separately and has lost its own
    # internal version gate. A gated primitive is the correct design, not a finding -- see the
    # comment on PRIMITIVE_GATE.
    for m in RVA_ARG_CALL.finditer(mask):
        if RVA_ARG_DEF.search(mask[: m.start()]):
            continue  # `fn patch_3byte_stub(` -- the definition, not a call site
        if gating.get(m.group(1), False):
            continue  # the primitive resolves through the version gate itself
        end = close_paren(mask, mask.index("(", m.end() - 1))
        args = mask[m.end() - 1 : end]
        line = src[: m.start()].count("\n") + 1
        out.append(("RVA_ARG", line, m.group(1), " ".join(args.split())[:120]))

    # SENTINEL: `game_data_addr(...)` immediately followed by `+`/`-`.
    for m in SENTINEL.finditer(mask):
        end = close_paren(mask, m.end() - 1)
        tail = mask[end : end + 40]
        # `-` only when it is not the `->` of a return type: `game_data_addr(..) -> usize {` is the
        # definition, not a call, and it matched here until 2026-08-30.
        if re.match(r"\s*(?:\+|-(?!>))\s*[^=]", tail):
            line = src[: m.start()].count("\n") + 1
            out.append(("SENTINEL", line, "game_data_addr(..)+offset", lines[line - 1].strip()[:120]))

    # Slot and RAW: per store, look back window lines for how the address was built.
    for i, mline in enumerate(mlines, 1):
        if not STORE.search(mline):
            continue
        lo = max(0, i - 1 - WINDOW)
        window_mask = "\n".join(mlines[lo:i])
        window_src = "\n".join(lines[lo:i])
        addr = IMAGE_ADDR.search(window_mask)
        gated = GATED.search(window_mask)
        sm = SLOT_LHS.search(mline)
        name = (sm.group(1) or sm.group(2)) if sm else None
        # Slot is tested FIRST: it is the more specific class, and every slot store is also a
        # raw store, so testing raw first would swallow it and the slot control would never fire.
        if name and SLOT_NAME.search(name) and not gated:
            if IMAGE_ADDR.search(window_mask) or re.search(
                rf"\b{re.escape(name)}\s*=\s*[^;]*(?:RVA|rva)", window_src
            ):
                out.append(("SLOT", i, name, lines[i - 1].strip()[:120]))
                continue
        if addr and not gated:
            out.append(("RAW", i, addr.group(1), lines[i - 1].strip()[:120]))
    return out


def scan(paths) -> list:
    out = []
    gating = primitive_gating(paths)
    for p in paths:
        if SKIP_DIR.search(p):
            continue
        src = open(p, encoding="utf-8", errors="replace").read()
        for cls, line, sym, text in scan_text(p, src, gating):
            out.append((cls, p, line, sym, text))
    return out


CONTROLS = {
    "SLOT": '''
fn control_slot(module_base: usize) {
    let slot = module_base + TITLE_STEP_IDX10_SLOT_RVA;
    unsafe { *(slot as *mut usize) = handler as *const () as usize };
}
''',
    "RVA_ARG": '''
fn control_rva_arg(base: usize) {
    let _ = er_hook::patch_3byte_stub(base, SIGNIN_FORCE_RVA, 0x40, STUB, "signin-force");
    er_hook::apply_xor_ret_stub(base, ONLINE_DISABLE_RVA, 0x48, STUB, "IsOnlineMode getter");
}
''',
    "SENTINEL": '''
fn control_sentinel(base: usize, idx: usize) -> usize {
    er_game_base::mem::game_data_addr(base, TABLE_RVA, "TABLE_RVA") + idx * 8
}
''',
    "RAW": '''
fn control_raw(base: usize, spec: &Spec) {
    unsafe { *((base + spec.rva) as *mut u8) = 1 };
}
''',
}
NEGATIVE = '''
fn control_gated(base: usize, idx: usize) {
    let row = er_game_base::mem::game_data_addr_offset(base, TABLE_RVA, "TABLE_RVA", idx * 8);
    if row != 0 {
        unsafe { *(row as *mut usize) = 0 };
    }
}
'''


# The RVA_ARG class keys on the primitive's gate, so it needs its own red/green pair: the same
# call site must fire when the primitive has lost `resolve_game_address` and stay silent when it
# has not. Without the green half, restoring the gate could not be told from breaking the matcher.
PRIMITIVE_GATED = '''
pub fn patch_3byte_stub(base: usize, rva: usize, label: &str) -> bool {
    let Some(address) = er_game_base::game_build::resolve_game_address(base + rva, label) else {
        return false;
    };
    write_stub(address)
}
fn caller(base: usize) {
    let _ = patch_3byte_stub(base, SIGNIN_FORCE_RVA, "signin-force");
}
'''
PRIMITIVE_UNGATED = PRIMITIVE_GATED.replace(
    "    let Some(address) = er_game_base::game_build::resolve_game_address(base + rva, label) else {\n        return false;\n    };\n",
    "    let address = base + rva;\n",
)


def selftest() -> int:
    failed = []
    for cls, text in CONTROLS.items():
        found = {c for c, _, _, _ in scan_text("control.rs", text)}
        status = "FIRES" if cls in found else "DID NOT FIRE"
        print(f"  positive control {cls:9} -> {status} (matched {sorted(found) or 'nothing'})")
        if cls not in found:
            failed.append(cls)
    neg = scan_text("control.rs", NEGATIVE)
    print(f"  negative control  gated    -> {'clean' if not neg else f'FALSE POSITIVE {neg}'}")
    if neg:
        failed.append("negative")

    # RVA_ARG red/green against the primitive's own definition.
    ungated = scan_text("control.rs", PRIMITIVE_UNGATED, primitive_gating_sources([PRIMITIVE_UNGATED]))
    print(
        f"  positive control RVA_ARG   -> {'FIRES' if ungated else 'DID NOT FIRE'} "
        f"when patch_3byte_stub loses resolve_game_address"
    )
    if not ungated:
        failed.append("RVA_ARG-ungated-primitive")
    gated = scan_text("control.rs", PRIMITIVE_GATED, primitive_gating_sources([PRIMITIVE_GATED]))
    print(
        f"  negative control RVA_ARG   -> {'clean' if not gated else f'FALSE POSITIVE {gated}'} "
        f"when patch_3byte_stub keeps it"
    )
    if gated:
        failed.append("RVA_ARG-gated-primitive")

    # A gate that cannot see the real primitives licenses every call site. Refuse to pass if the
    # definitions have moved out of the scanned corpus.
    real = primitive_gating(sorted(glob.glob("crates/**/*.rs", recursive=True)))
    missing = [n for n in PRIMITIVE_NAMES if n not in real]
    print(f"  corpus            -> primitives found: {sorted(real)}" + (f" MISSING {missing}" if missing else ""))
    if missing:
        failed.append(f"primitive-definitions-not-found:{missing}")
    if failed:
        print(f"SELFTEST FAILED: {failed}", file=sys.stderr)
        return 1
    print("audit-ungated-image-writes selftest OK: all four classes have a proven positive control")
    return 0


def main() -> int:
    if "--selftest" in sys.argv:
        return selftest()
    paths = [a for a in sys.argv[1:] if not a.startswith("-")] or sorted(
        glob.glob("crates/**/*.rs", recursive=True)
    )
    rows = scan(paths)
    for cls, p, line, sym, text in sorted(rows):
        print(f"{cls:9} {p}:{line}  [{sym}]\n          {text}")
    counts = {}
    for cls, *_ in rows:
        counts[cls] = counts.get(cls, 0) + 1
    print(f"\n{len(rows)} findings: {counts}")
    if rows:
        print(
            "\naudit-ungated-image-writes: FAILED -- each finding above writes into the game image\n"
            "from an address the running build's version gate never vetted. On 1.17 a 1.16.2 offset\n"
            "lands mid-function, so the write corrupts whatever now occupies it, silently.\n"
            "Route the address through er_game_base::game_build::resolve_game_address (or\n"
            "game_data_addr / game_rva) and honour the 0/None refusal."
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
