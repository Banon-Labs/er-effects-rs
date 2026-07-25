#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/git-write-with-lock-retry.sh <git-args...>

Runs a git write command after handling transient/stale .git/index.lock files safely:
  - waits while another process owns the lock
  - removes only ownerless locks older than GIT_WRITE_LOCK_STALE_SECONDS (default: 5)
  - retries commands that fail specifically because index.lock reappeared

This is for agent/operator write operations such as add, commit, restore, checkout, merge, and rebase.
Use GIT_OPTIONAL_LOCKS=0 directly for read-only git inspection.
EOF
}

if [[ $# -eq 0 || "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

STALE_SECONDS="${GIT_WRITE_LOCK_STALE_SECONDS:-5}"
WAIT_SECONDS="${GIT_WRITE_LOCK_WAIT_SECONDS:-20}"
RETRIES="${GIT_WRITE_LOCK_RETRIES:-2}"

if ! [[ "$STALE_SECONDS" =~ ^[0-9]+$ && "$WAIT_SECONDS" =~ ^[0-9]+$ && "$RETRIES" =~ ^[0-9]+$ ]]; then
  echo "git-write-with-lock-retry: timeout/retry env vars must be non-negative integers" >&2
  exit 2
fi

repo_root="$(GIT_OPTIONAL_LOCKS=0 git rev-parse --show-toplevel)"
lock_path="$repo_root/.git/index.lock"

wait_for_index_lock() {
  python3 - "$lock_path" "$STALE_SECONDS" "$WAIT_SECONDS" <<'PY'
from __future__ import annotations

import os
import sys
import time
from pathlib import Path

lock = Path(sys.argv[1])
stale_seconds = int(sys.argv[2])
wait_seconds = int(sys.argv[3])
deadline = time.time() + wait_seconds

def owner_pids() -> list[int]:
    resolved = str(lock.resolve(strict=False))
    owners: list[int] = []
    proc = Path('/proc')
    for pid_dir in proc.iterdir():
        if not pid_dir.name.isdigit():
            continue
        fd_dir = pid_dir / 'fd'
        try:
            for fd in fd_dir.iterdir():
                try:
                    if os.path.realpath(fd) == resolved:
                        owners.append(int(pid_dir.name))
                        break
                except OSError:
                    continue
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            continue
    return sorted(set(owners))

while True:
    if not lock.exists():
        raise SystemExit(0)
    owners = owner_pids()
    try:
        age = time.time() - lock.stat().st_mtime
    except FileNotFoundError:
        raise SystemExit(0)
    if owners:
        if time.time() >= deadline:
            print(
                f"git-write-with-lock-retry: lock still owned after {wait_seconds}s: {lock} owners={owners}",
                file=sys.stderr,
            )
            raise SystemExit(75)
        time.sleep(0.25)
        continue
    if age >= stale_seconds:
        try:
            lock.unlink()
            print(
                f"git-write-with-lock-retry: removed ownerless stale lock {lock} age={age:.1f}s",
                file=sys.stderr,
            )
            raise SystemExit(0)
        except FileNotFoundError:
            raise SystemExit(0)
    if time.time() >= deadline:
        print(
            f"git-write-with-lock-retry: ownerless lock is too young and did not age out within {wait_seconds}s: {lock} age={age:.1f}s",
            file=sys.stderr,
        )
        raise SystemExit(75)
    time.sleep(0.25)
PY
}

attempt=0
while true; do
  wait_for_index_lock
  err_file="$(mktemp)"
  if git "$@" 2>"$err_file"; then
    rm -f "$err_file"
    exit 0
  fi
  status=$?
  cat "$err_file" >&2
  if grep -q "index.lock" "$err_file" && (( attempt < RETRIES )); then
    rm -f "$err_file"
    attempt=$((attempt + 1))
    continue
  fi
  rm -f "$err_file"
  exit "$status"
done
