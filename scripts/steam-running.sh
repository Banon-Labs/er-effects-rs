#!/usr/bin/env bash
# WSL-aware "is Steam running?" check. Exits 0 if Steam is up, 1 otherwise.
#
# Why this exists (2026-07-18): on a WSL2 + native-Windows-Steam box, Steam runs as the WINDOWS
# process steam.exe (visible only via tasklist.exe), so the naive `pgrep -x steam` ALWAYS returns
# no-match = a false negative that reports "Steam is down" when it is up. That false negative once
# blocked an entire overnight runtime session. Check BOTH the Linux process (native Steam / Proton
# on a Linux-Steam box) AND the Windows process list (WSL + Windows Steam). See bd
# steam-detection-wsl-false-negative-2026-07-18.
steam_running() {
  # 1. native Linux Steam
  pgrep -x steam >/dev/null 2>&1 && return 0
  pgrep -x steamwebhelper >/dev/null 2>&1 && return 0
  # 2. Windows Steam seen from WSL via tasklist.exe
  if command -v tasklist.exe >/dev/null 2>&1; then
    tasklist.exe /FI "IMAGENAME eq steam.exe" 2>/dev/null | grep -qi 'steam.exe' && return 0
  fi
  return 1
}

# When executed directly (not sourced), behave as a predicate + print a line.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  if steam_running; then echo "steam: RUNNING"; exit 0; else echo "steam: NOT running"; exit 1; fi
fi
