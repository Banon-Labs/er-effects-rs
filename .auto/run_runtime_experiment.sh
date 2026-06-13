#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$REPO_ROOT"

cat >&2 <<'EOF'
Runtime probes are disabled fail-closed for autoresearch.

Why: the runtime harness previously allowed an outer autoresearch/tool timeout to
become an idle Elden Ring wait. Static measurement remains available through
.auto/measure.sh. Re-enable runtime probing only by changing
scripts/check-runtime-probe-contract.py, its regression tests, and the Rego
policy in the same reviewed change.
EOF
exit 2
