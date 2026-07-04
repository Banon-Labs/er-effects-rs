#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

python3 "$repo_root/scripts/check-no-timeouts.py"
python3 "$repo_root/scripts/test-no-timeouts.py"
python3 "$repo_root/scripts/check-launch-guardrails.py" --audit
python3 "$repo_root/scripts/check-runtime-probe-contract.py" --audit
python3 "$repo_root/scripts/test-runtime-probe-contract.py"
python3 "$repo_root/scripts/test-er-readiness-watch.py"
python3 "$repo_root/scripts/test-save-slot-oracle.py"
python3 "$repo_root/scripts/check-autoload-happy-path.py"
python3 "$repo_root/scripts/test-autoload-happy-path.py"
python3 "$repo_root/scripts/check-native-continue-static.py"
python3 "$repo_root/scripts/check-menu-constructor-static.py"
python3 "$repo_root/scripts/check-env-gate-comments.py"
python3 "$repo_root/scripts/test-env-gate-comments.py"
command -v cupcake >/dev/null 2>&1 || { echo "missing required command: cupcake" >&2; exit 127; }
cupcake validate --log-level error
python3 "$repo_root/scripts/test-cupcake-policies.py"
python3 "$repo_root/scripts/check-no-lossy-utf8.py"
python3 "$repo_root/scripts/check-rust-file-sizes.py"
cargo fmt --manifest-path "$repo_root/Cargo.toml" -- --check
shellcheck "$repo_root/scripts/stage-autoload-release.sh"
shellcheck "$repo_root/scripts/run-product-continue-direct-probe.sh"
shellcheck "$repo_root/scripts/run-me3-product-smoke.sh"

# Windows-target check, cross-compiled from Linux via cargo-xwin (preferred). Falls back to
# a plain cargo check only if cargo-xwin is unavailable (which needs an MSVC toolchain on
# PATH and will otherwise fail at the C-dependency link step).
if command -v cargo-xwin >/dev/null 2>&1; then
  cargo xwin check --manifest-path "$repo_root/Cargo.toml" --target x86_64-pc-windows-msvc
else
  cargo check --manifest-path "$repo_root/Cargo.toml" --target x86_64-pc-windows-msvc
fi
