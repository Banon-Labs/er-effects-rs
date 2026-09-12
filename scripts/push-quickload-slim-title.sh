#!/usr/bin/env bash
# Push the slim-quickload title branch and, in the same invocation, fast-forward the stack's base
# branch onto the three gate fixes that sit above it.
#
# Both refs go in a single `git push` on purpose: the pre-push hook runs the whole gate suite, so
# two invocations would run it twice for no extra coverage.
#
# Detached on purpose too. That suite takes ~25 minutes, far past an agent shell's 30s budget, and
# killing a push mid-suite orphans check.sh's flock holders
# (bd killing-a-push-orphans-check-sh-flock-holders-2026-09-11). Launch it with
# `setsid nohup ... &` and read the log.
#
# `main` is never named here. The two refs are literal.
set -euo pipefail
cd "$(dirname "$0")/.."
base_fix="${1:-b8ca9b8c}"
git push -u origin \
	"${base_fix}:feat/quit-menu-character-rows" \
	fix/quickload-vanilla-title-after-boot
echo "PUSH-SEQUENCE-DONE"
