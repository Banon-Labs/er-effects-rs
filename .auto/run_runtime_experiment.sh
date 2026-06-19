#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$REPO_ROOT"

cat >&2 <<'EOF'
Runtime probes are disabled fail-closed for autoresearch.

Why: the runtime harness previously allowed an unbounded Elden Ring wait. Static
measurement remains available through .auto/measure.sh. Re-enable runtime
probing only through the reviewed Rego contract: explicit opt-in, readiness
watcher, clean teardown, and timeout_seconds no greater than 60.
EOF
exit 2
