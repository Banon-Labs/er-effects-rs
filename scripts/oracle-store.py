#!/usr/bin/env python3
"""TOOL 3 of the real oracle (user 2026-07-20): a VERSIONED store mapping the DLL-package SHAs -> the
set of named-phase imprints.

An imprint set is tied to the EXACT build that produced it: the sha256 of the product + trace + input-
harness DLLs. Each named PHASE (boot_to_control, control_to_confirm, continue, load2_to_control, ...)
is a column holding that phase's imprint JSON. New phases (e.g. a 3rd load) are added via MIGRATIONS
(append to MIGRATIONS below -> `ALTER TABLE ... ADD COLUMN phase_<name> TEXT`), so the schema grows
without losing prior data. The comparator (oracle-compare.py) looks up the imprint for the current
build's shas (or a chosen reference build) and phase, then checks a live run against it.

DB: data/oracle/imprints.db (override with --db).

Usage:
  oracle-store.py init
  oracle-store.py put  --product <dll> --trace <dll> --harness <dll> --phase boot_to_control \\
                       --imprint imprint.json [--git-head <sha>]
  oracle-store.py get  --product <dll> --trace <dll> --harness <dll> --phase boot_to_control [-o out.json]
  oracle-store.py list
  oracle-store.py phases
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import sqlite3
import sys
from pathlib import Path

DEFAULT_DB = Path(__file__).resolve().parent.parent / "data" / "oracle" / "imprints.db"

# Phase columns created at schema v1. ADD NEW PHASES via a new migration tuple below -- never edit an
# applied migration. Each phase column holds that phase's imprint JSON (or NULL if not captured).
MIGRATIONS: list[tuple[int, list[str]]] = [
    (
        1,
        [
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY)",
            """CREATE TABLE IF NOT EXISTS imprint_sets (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   created_at TEXT,
                   git_head TEXT,
                   product_sha TEXT NOT NULL,
                   trace_sha TEXT NOT NULL,
                   harness_sha TEXT NOT NULL,
                   phase_boot_to_control TEXT,
                   phase_control_to_confirm TEXT,
                   phase_continue TEXT,
                   phase_load2_to_control TEXT,
                   UNIQUE(product_sha, trace_sha, harness_sha)
               )""",
        ],
    ),
    # Example future migration (uncomment / append when the 3rd load phase exists):
    # (2, ["ALTER TABLE imprint_sets ADD COLUMN phase_load3_to_control TEXT"]),
]


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def connect(db: Path) -> sqlite3.Connection:
    db.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(db)
    conn.row_factory = sqlite3.Row
    return conn


def migrate(conn: sqlite3.Connection) -> int:
    conn.execute("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY)")
    row = conn.execute("SELECT MAX(version) AS v FROM schema_version").fetchone()
    current = row["v"] or 0
    applied = current
    for version, statements in MIGRATIONS:
        if version <= current:
            continue
        for stmt in statements:
            conn.execute(stmt)
        conn.execute("INSERT OR REPLACE INTO schema_version(version) VALUES (?)", (version,))
        applied = version
    conn.commit()
    return applied


def phase_columns(conn: sqlite3.Connection) -> list[str]:
    cols = [r["name"] for r in conn.execute("PRAGMA table_info(imprint_sets)")]
    return [c for c in cols if c.startswith("phase_")]


def phase_col(conn: sqlite3.Connection, phase: str) -> str:
    col = phase if phase.startswith("phase_") else f"phase_{phase}"
    if col not in phase_columns(conn):
        raise SystemExit(
            f"unknown phase '{phase}'. Known: {[c[len('phase_'):] for c in phase_columns(conn)]}. "
            f"Add a migration to create it."
        )
    return col


def shas(args) -> tuple[str, str, str]:
    return (
        sha256_file(Path(args.product)),
        sha256_file(Path(args.trace)),
        sha256_file(Path(args.harness)),
    )


def cmd_init(args) -> int:
    conn = connect(args.db)
    v = migrate(conn)
    print(f"store: {args.db}  schema_version={v}  phases={[c[6:] for c in phase_columns(conn)]}")
    return 0


def cmd_put(args) -> int:
    conn = connect(args.db)
    migrate(conn)
    col = phase_col(conn, args.phase)
    p, t, h = shas(args)
    imprint = Path(args.imprint).read_text(encoding="utf-8")
    # Validate it is JSON so we never store garbage.
    json.loads(imprint)
    # Timestamp: real datetime is fine (standalone script, not the Workflow sandbox).
    now = datetime.datetime.now(datetime.timezone.utc).isoformat()
    conn.execute(
        """INSERT INTO imprint_sets (created_at, git_head, product_sha, trace_sha, harness_sha)
           VALUES (?, ?, ?, ?, ?)
           ON CONFLICT(product_sha, trace_sha, harness_sha) DO NOTHING""",
        (now, args.git_head or "", p, t, h),
    )
    conn.execute(
        f"UPDATE imprint_sets SET {col}=? WHERE product_sha=? AND trace_sha=? AND harness_sha=?",
        (imprint, p, t, h),
    )
    conn.commit()
    print(f"stored phase '{args.phase}' for set product={p[:12]} trace={t[:12]} harness={h[:12]}")
    return 0


def cmd_get(args) -> int:
    conn = connect(args.db)
    migrate(conn)
    col = phase_col(conn, args.phase)
    p, t, h = shas(args)
    row = conn.execute(
        f"SELECT {col} AS imp FROM imprint_sets WHERE product_sha=? AND trace_sha=? AND harness_sha=?",
        (p, t, h),
    ).fetchone()
    if row is None or row["imp"] is None:
        print(
            f"NO imprint for phase '{args.phase}' at product={p[:12]} trace={t[:12]} harness={h[:12]}",
            file=sys.stderr,
        )
        return 1
    if args.out:
        Path(args.out).write_text(row["imp"], encoding="utf-8")
        print(f"-> {args.out}")
    else:
        sys.stdout.write(row["imp"])
    return 0


def cmd_list(args) -> int:
    conn = connect(args.db)
    migrate(conn)
    pcols = phase_columns(conn)
    rows = conn.execute("SELECT * FROM imprint_sets ORDER BY id").fetchall()
    if not rows:
        print("(no imprint sets yet)")
        return 0
    for r in rows:
        have = [c[6:] for c in pcols if r[c] is not None]
        print(
            f"#{r['id']} {r['created_at']} git={r['git_head'][:10]} "
            f"product={r['product_sha'][:12]} trace={r['trace_sha'][:12]} harness={r['harness_sha'][:12]}"
        )
        print(f"     phases captured: {have or '(none)'}")
    return 0


def cmd_phases(args) -> int:
    conn = connect(args.db)
    migrate(conn)
    print("phases:", [c[6:] for c in phase_columns(conn)])
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", type=Path, default=DEFAULT_DB)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("init")
    sub.add_parser("list")
    sub.add_parser("phases")
    for name in ("put", "get"):
        sp = sub.add_parser(name)
        sp.add_argument("--product", required=True)
        sp.add_argument("--trace", required=True)
        sp.add_argument("--harness", required=True)
        sp.add_argument("--phase", required=True)
        if name == "put":
            sp.add_argument("--imprint", required=True)
            sp.add_argument("--git-head", default="")
        else:
            sp.add_argument("-o", "--out")
    args = ap.parse_args()
    return {
        "init": cmd_init,
        "put": cmd_put,
        "get": cmd_get,
        "list": cmd_list,
        "phases": cmd_phases,
    }[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
