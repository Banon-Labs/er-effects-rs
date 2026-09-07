#!/usr/bin/env python3
"""Audit address constants whose doc comment makes a *mechanism* claim.

The failure mode this hunts is a correct address carrying a wrong STORY: a doc that
says "pointer that is dereferenced" for something whose single reader immediately
``call``s it (a call slot), a "guard/flag" that is only ever written once at init, a
"vtable" nothing indirect-calls through, or a "table of N entries" whose second qword
belongs to a different object.

The cheapest falsifier is the cross-reference shape, so that is what this collects:
for every address behind such a constant it records ``getXrefsTo`` totalCount, the
reference kinds, and -- when there is exactly one reference, the strongest signal --
the instruction at that site and the one after it.

Usage:
    python3 scripts/audit-const-mechanism-claims.py --scan   # extract constants
    python3 scripts/audit-const-mechanism-claims.py --xref   # query Ghidra (slow)
    python3 scripts/audit-const-mechanism-claims.py --report

Ghidra: :8765 is 1.16.2 (the only image with curated names/types/RTTI), which is why
semantic questions go there. Both dump/flat-image/runtime shifts are zero.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
MCP = REPO / "scripts" / "ghidra" / "mcp_query.py"
PORT_1162 = 8765

# A constant declaration whose name looks like an address.
DECL_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+([A-Z][A-Z0-9_]*)\s*:\s*([^=]+?)\s*=\s*(.*)$"
)
ADDRISH_RE = re.compile(r"(RVA|_VA\b|ADDR|SLOT|VADDR)")
HEX_RE = re.compile(r"0x([0-9a-fA-F_]{5,})")

# Doc phrases that assert a mechanism, i.e. that are falsifiable by xref shape.
MECHANISM_RE = re.compile(
    r"\b(vtable|v-table|vftable|guard|flag|status|dereferenc\w*|deref\w*|pointer to|"
    r"ptr to|table of|entries|slot table|is read by|used by|read by|singleton|"
    r"global pointer|instance pointer)\b",
    re.I,
)

SRC_GLOBS = ("crates/**/*.rs", "src/**/*.rs", "tools/**/*.rs")


def scan() -> list[dict]:
    """Extract address-shaped constant declarations plus their doc comments."""
    out: list[dict] = []
    files: list[Path] = []
    for g in SRC_GLOBS:
        files.extend(sorted(REPO.glob(g)))
    for f in files:
        try:
            lines = f.read_text(encoding="utf-8", errors="replace").split("\n")
        except OSError:
            continue
        for i, line in enumerate(lines):
            m = DECL_RE.match(line)
            if not m:
                continue
            name, ty, val = m.groups()
            if not ADDRISH_RE.search(name):
                continue
            doc: list[str] = []
            j = i - 1
            while j >= 0 and (
                lines[j].strip().startswith("///")
                or lines[j].strip().startswith("//!")
                or lines[j].strip().startswith("#[")
            ):
                doc.append(lines[j].strip())
                j -= 1
            doc.reverse()
            hexm = HEX_RE.search(val)
            num = None
            if hexm:
                num = int(hexm.group(1).replace("_", ""), 16)
                if num >= 0x140000000:
                    num -= 0x140000000
            out.append(
                {
                    "file": str(f.relative_to(REPO)),
                    "line": i + 1,
                    "name": name,
                    "ty": ty.strip(),
                    "val": val.strip()[:120],
                    "doc": " ".join(doc),
                    "rva": num,
                }
            )
    return out


def mcp(method: str, params: dict, port: int = PORT_1162) -> dict:
    """One lock-free daemon call. Bounded well under the 30s shell cap."""
    proc = subprocess.run(
        [sys.executable, str(MCP), method, "--params", json.dumps(params), "--port", str(port)],
        capture_output=True,
        text=True,
        timeout=25,
    )
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError:
        return {"error": proc.stdout[:200] + proc.stderr[:200]}


def xref_profile(rva: int, port: int = PORT_1162) -> dict:
    """Cross-reference shape for one address: count, kinds, and the lone site if unique."""
    va = f"{rva + 0x140000000:x}"
    r = mcp("getXrefsTo", {"address": va}, port)
    res = r.get("result", {})
    items = res.get("items", []) or []
    prof = {
        "va": va,
        "totalCount": res.get("totalCount"),
        "kinds": sorted({str(i.get("refType")) for i in items}),
        "callers": sorted({str(i.get("fromFunction")) for i in items})[:8],
        "sites": [i.get("fromAddress") for i in items][:8],
        "error": r.get("error"),
    }
    return prof


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--scan", action="store_true")
    ap.add_argument("--xref", action="store_true")
    ap.add_argument("--report", action="store_true")
    ap.add_argument("--state", default=os.environ.get("ER_CONST_AUDIT_STATE", ""))
    ap.add_argument("--start", type=int, default=0)
    ap.add_argument("--count", type=int, default=10_000)
    args = ap.parse_args()

    state = Path(args.state) if args.state else REPO / "target" / "const-mechanism-audit.json"
    state.parent.mkdir(parents=True, exist_ok=True)

    if args.scan:
        decls = scan()
        claims = [d for d in decls if d["rva"] and MECHANISM_RE.search(d["doc"])]
        payload = {"decls": decls, "claims": claims, "xrefs": {}}
        if state.exists():
            old = json.loads(state.read_text())
            payload["xrefs"] = old.get("xrefs", {})
        state.write_text(json.dumps(payload, indent=1))
        print(f"decls={len(decls)} mechanism-claims={len(claims)} -> {state}")
        return 0

    if args.xref:
        payload = json.loads(state.read_text())
        rvas = sorted({d["rva"] for d in payload["claims"]})
        todo = [r for r in rvas if str(r) not in payload["xrefs"]][args.start : args.start + args.count]
        for rva in todo:
            payload["xrefs"][str(rva)] = xref_profile(rva)
            state.write_text(json.dumps(payload, indent=1))
        print(f"xrefed={len(todo)} remaining={len([r for r in rvas if str(r) not in payload['xrefs']])}")
        return 0

    if args.report:
        payload = json.loads(state.read_text())
        by_rva: dict[int, list[dict]] = {}
        for d in payload["claims"]:
            by_rva.setdefault(d["rva"], []).append(d)
        rows = []
        for rva, ds in sorted(by_rva.items()):
            x = payload["xrefs"].get(str(rva), {})
            rows.append((x.get("totalCount"), rva, ds, x))
        rows.sort(key=lambda r: (r[0] is None, r[0]))
        for tc, rva, ds, x in rows:
            names = ",".join(sorted({d["name"] for d in ds}))
            print(f"{tc!s:>6}  0x{rva + 0x140000000:x}  {','.join(x.get('kinds') or [])!s:<20} {names}")
        return 0

    ap.print_help()
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
