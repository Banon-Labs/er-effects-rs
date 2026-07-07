#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

if [[ ! -f vendor/minhook/src/buffer.c ]]; then
  cat >&2 <<'EOF'
missing vendor/minhook/src/buffer.c

CI checks out MinHook with:
  git clone --depth 1 --branch v1.3.4 https://github.com/TsudaKageyu/minhook.git vendor/minhook

For local ignored worktrees, either clone it there or symlink the main checkout's vendor directory.
EOF
  exit 2
fi

python3 scripts/check-no-lossy-utf8.py
python3 scripts/check-no-timeouts.py
python3 scripts/test-no-timeouts.py
cupcake validate --log-level error
python3 scripts/test-cupcake-policies.py
cargo fmt --all -- --check
cargo test -p er-soulsformats -p er-param-inspect

if command -v cargo-xwin >/dev/null 2>&1; then
  cargo xwin check --target x86_64-pc-windows-msvc
else
  cargo check --target x86_64-pc-windows-msvc
fi
