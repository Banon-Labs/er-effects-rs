#!/usr/bin/env bash
# Set eldenring.exe (a NATIVE WINDOWS process) to High priority (+ optional dedicated affinity) so the
# WSL2 parallel-cargo/agent contention cannot starve ER's single-core asset-loading / physics. No admin
# needed for an own-user process's priority/affinity (WSL powershell.exe can set .PriorityClass /
# .ProcessorAffinity). Deterministic-triage lever for the load2/load3 movement + FPS proof (bd
# runs-coinflip-under-parallel-cargo-contention / deterministic-control-boost-er-windows-priority).
#
# ONE-SHOT by design: priority persists for the process lifetime and a reload (System->Quit->Load) does
# NOT restart eldenring.exe, so a single set holds -- no poll/sleep loop (which the no-timeouts gate bans;
# the caller already has ER-readiness state). Prints the resulting priority/affinity, or NOPROC if ER is
# not running yet (call again once it is). Priority is ALWAYS raised to High; affinity is OPT-IN via
# ER_AFFINITY_MASK (decimal core bitmask, e.g. 61440 = 0xF000 = cores 12-15 on a 16-core box) -- off by
# default so ER is not over-confined.
set -uo pipefail

AFFINITY_MASK="${ER_AFFINITY_MASK:-}"

powershell.exe -NoProfile -Command "
  \$p = Get-Process -Name eldenring -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not \$p) { Write-Output 'NOPROC'; exit 0 }
  try { \$p.PriorityClass = 'High' } catch {}
  \$mask = '${AFFINITY_MASK}'
  if (\$mask -ne '') { try { \$p.ProcessorAffinity = [IntPtr][int64]\$mask } catch {} }
  Write-Output ('ER PID=' + \$p.Id + ' prio=' + \$p.PriorityClass + ' aff=' + \$p.ProcessorAffinity)
" 2>/dev/null | tr -d '\r' | grep -E 'ER PID=|NOPROC' | head -1
