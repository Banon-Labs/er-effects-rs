#!/usr/bin/env bash
# Install the repo's version-controlled git hooks by pointing core.hooksPath at scripts/hooks.
# Idempotent; run once per clone. bd static-guards-run-in-build-format-cycle-precommit-hook-2026-07-19.
set -euo pipefail
repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"
chmod +x scripts/hooks/* 2>/dev/null || true
git config core.hooksPath scripts/hooks
echo "installed: core.hooksPath -> scripts/hooks (pre-commit runs the fast static guards)"
echo "verify:    git config --get core.hooksPath"
echo "bypass:    git commit --no-verify   (emergency only)"
