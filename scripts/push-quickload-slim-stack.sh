#!/usr/bin/env bash
# Push the er-quickload slimming stack, base first, so each pull request's base branch exists
# before the branch that targets it. Never pushes `main`; the three refs are named literally.
#
# Detached on purpose. The pre-push hook runs the whole gate suite, which takes far longer than
# an agent shell's 30s budget, and killing a push mid-suite orphans check.sh's flock holders
# (bd killing-a-push-orphans-check-sh-flock-holders-2026-09-11). Let it finish and read the log.
set -euo pipefail
cd "$(dirname "$0")/.."
git push origin feat/quit-menu-character-rows
git push -u origin feat/quickload-feature-bite-gate
git push -u origin refactor/quickload-quit-menu-demux
echo "PUSH-SEQUENCE-DONE"
