#!/usr/bin/env bash
set -euo pipefail

cat >&2 <<'EOF'
Runtime probes are disabled fail-closed from the autoresearch wrapper.
Use a deliberate manual runtime probe with the readiness watcher contract instead.
EOF
exit 2
