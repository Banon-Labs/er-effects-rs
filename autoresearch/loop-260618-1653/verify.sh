#!/usr/bin/env bash
# Metric: total count of hex literals (0x...) across src/*.rs. Lower is better.
# Each hand-rolled RVA/offset replaced by an upstream eldenring typed accessor
# removes its literal(s). Legitimate non-game literals (Windows CONTEXT/PE/
# DInput/VK constants) have no upstream equivalent and stay -- the metric floor
# is well above zero by design.
set -euo pipefail
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
rtk grep -o "0x[0-9a-fA-F]+" \
  "$repo_root/src/lib.rs" \
  "$repo_root/src/experiments.rs" \
  "$repo_root/src/telemetry.rs" \
  "$repo_root/src/hooks.rs" \
  "$repo_root/src/ffi.rs" \
  "$repo_root/src/crashlog.rs" | wc -l
