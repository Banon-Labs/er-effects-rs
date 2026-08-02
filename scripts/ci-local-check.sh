#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

# SELF-HEAL THE WORKTREE CASE INSTEAD OF MAKING EVERY AGENT DO IT BY HAND. `vendor/` is
# gitignored, so a `git worktree` checkout (an agent sandbox under `.claude/worktrees/`, a
# `.worktrees/` lab) starts without MinHook while the MAIN checkout it was created from already has
# it -- and this script runs from the pre-push hook, so the first thing a worktree ever learns is
# that its push is refused. The shared git dir names that main checkout exactly, so link its vendor
# tree in rather than re-cloning ~1MB of C per worktree. Fail closed and unchanged when the link
# cannot be established, because a missing MinHook still breaks the cross-compile below.
if [[ ! -f vendor/minhook/src/buffer.c ]]; then
  main_checkout=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)
  main_checkout=${main_checkout%/.git}
  if [[ -n "$main_checkout" ]] && [[ "$main_checkout" != "$repo_root" ]] &&
    [[ -f "$main_checkout/vendor/minhook/src/buffer.c" ]]; then
    mkdir -p vendor
    ln -sfn "$main_checkout/vendor/minhook" vendor/minhook
    echo "[ci-local-check] git worktree: linked vendor/minhook -> $main_checkout/vendor/minhook" >&2
  fi
fi

if [[ ! -f vendor/minhook/src/buffer.c ]]; then
  cat >&2 <<'EOF'
missing vendor/minhook/src/buffer.c

CI checks out MinHook with:
  git clone --depth 1 --branch v1.3.4 https://github.com/TsudaKageyu/minhook.git vendor/minhook

A git worktree normally self-heals here by linking the main checkout's vendor directory; reaching
this message from one means that checkout has no MinHook either, so clone it there first.
EOF
  exit 2
fi

python3 scripts/check-no-lossy-utf8.py
python3 scripts/check-no-timeouts.py
python3 scripts/test-no-timeouts.py
bash scripts/test-git-pre-push-block-main.sh
# One game address must have exactly ONE literal declaration. Divergent names for one address
# are divergent CLAIMS about what it is, and three of them turned out to be wrong RE facts
# shipping in the DLL (bd rva-67b750-is-save-write-not-continue-load-2026-08-01,
# rva-4852f88-is-saveload2-slsystemimpl-not-fd4-io-worker-2026-08-01). Selftest first, so the
# gate is never trusted on its own say-so.
python3 scripts/check-rva-alias-drift.py --selftest
python3 scripts/check-rva-alias-drift.py
cupcake validate --log-level error
python3 scripts/test-cupcake-policies.py
cargo fmt --all -- --check
cargo test -p er-soulsformats -p er-param-inspect

if command -v cargo-xwin >/dev/null 2>&1; then
  cargo xwin check --target x86_64-pc-windows-msvc
else
  cargo check --target x86_64-pc-windows-msvc
fi
