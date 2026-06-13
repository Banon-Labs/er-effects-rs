#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

python3 "$repo_root/scripts/check-no-timeouts.py"
python3 "$repo_root/scripts/test-no-timeouts.py"
python3 "$repo_root/scripts/check-runtime-probe-contract.py" --audit
python3 "$repo_root/scripts/test-runtime-probe-contract.py"
command -v cupcake >/dev/null 2>&1 || { echo "missing required command: cupcake" >&2; exit 127; }
cupcake validate --log-level error
python3 "$repo_root/scripts/test-cupcake-policies.py"
python3 "$repo_root/scripts/check-no-magic-numbers.py"
python3 "$repo_root/scripts/check-no-lossy-utf8.py"
cargo fmt --manifest-path "$repo_root/Cargo.toml" -- --check

if command -v powershell.exe >/dev/null 2>&1; then
  project_win=$(wslpath -w "$repo_root")
  project_ps=${project_win//\'/\'\'}
  powershell.exe -NoProfile -Command \
    "\$ErrorActionPreference = 'Stop'; \$env:CARGO_INCREMENTAL = '0'; \$env:CARGO_TARGET_DIR = Join-Path \$env:TEMP 'er-effects-rs-target'; Set-Location -LiteralPath '$project_ps'; cargo check --target x86_64-pc-windows-msvc"
elif command -v cargo-xwin >/dev/null 2>&1; then
  cargo xwin check --manifest-path "$repo_root/Cargo.toml" --target x86_64-pc-windows-msvc
else
  cargo check --manifest-path "$repo_root/Cargo.toml" --target x86_64-pc-windows-msvc
fi
