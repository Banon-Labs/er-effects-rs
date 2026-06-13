#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$REPO_ROOT"

if [[ $# -gt 0 ]]; then
  case "$1" in
    direct|direct-protected|steam|attach-existing)
      export LAUNCH_MODE="$1"
      shift
      ;;
  esac
fi

if [[ $# -gt 0 ]]; then
  echo "unknown arguments: $*" >&2
  exit 2
fi

printf '%s\n' "event-driven-runtime-probe" > .auto/run-runtime-once
export AUTO_ALLOW_RUNTIME_PROBE=1
exec ./.auto/measure.sh
