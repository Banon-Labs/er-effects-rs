#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

bash "$repo_root/scripts/check-no-local-main-commits.sh"
python3 "$repo_root/scripts/check-no-timeouts.py"
python3 "$repo_root/scripts/check-no-committed-build-artifacts.py" --selftest
python3 "$repo_root/scripts/check-no-committed-build-artifacts.py"
python3 "$repo_root/scripts/test-no-timeouts.py"
bash "$repo_root/scripts/test-git-pre-push-block-main.sh"
# Telemetry honesty: no counter may be READ to emit an oracle while written nowhere. Selftest first,
# so the gate is never trusted on its own say-so (er-effects-rs-56fx).
python3 "$repo_root/scripts/check-oracle-writers.py" --selftest
python3 "$repo_root/scripts/check-oracle-writers.py"
python3 "$repo_root/scripts/check-launch-guardrails.py" --audit
python3 "$repo_root/scripts/check-runtime-probe-contract.py" --audit
python3 "$repo_root/scripts/test-runtime-probe-contract.py"
python3 "$repo_root/scripts/test-er-readiness-watch.py"
python3 "$repo_root/scripts/test-save-slot-oracle.py"
python3 "$repo_root/scripts/test-detect-proc.py"
python3 "$repo_root/scripts/test-semaphore-watchdog.py"
python3 "$repo_root/scripts/test-input-harness-static.py"
python3 "$repo_root/scripts/test-wall-of-text-classifier.py"
python3 "$repo_root/scripts/check-retired-button-labels.py"
python3 "$repo_root/scripts/check-autoload-happy-path.py"
python3 "$repo_root/scripts/test-autoload-happy-path.py"
# An unresolvable staged save is terminal for the process. Pin the caller-side state transitions and
# the nonzero recurrence semaphore so the 120,959-call identical-rejection loop cannot return.
python3 "$repo_root/scripts/check-own-load-save-rejection-guard.py" --selftest
python3 "$repo_root/scripts/check-own-load-save-rejection-guard.py"
python3 "$repo_root/scripts/check-yk0j-runtime-proof.py" --selftest
python3 "$repo_root/scripts/check-user-release-package.py"
python3 "$repo_root/scripts/check-native-continue-static.py"
python3 "$repo_root/scripts/check-menu-constructor-static.py"
python3 "$repo_root/scripts/check-env-gate-comments.py"
python3 "$repo_root/scripts/test-env-gate-comments.py"
python3 "$repo_root/scripts/check-marker-file-gates.py"
python3 "$repo_root/scripts/test-marker-file-gates.py"
python3 "$repo_root/scripts/check-reload-trace-dll-policy.py" --audit
python3 "$repo_root/scripts/check-windows-proof-render.py"
python3 "$repo_root/scripts/test-windows-proof-render.py"
python3 "$repo_root/scripts/test-windows-proof-render-smoke-verdict.py"
command -v cupcake >/dev/null 2>&1 || {
	echo "missing required command: cupcake" >&2
	exit 127
}
cupcake validate --log-level error
python3 "$repo_root/scripts/test-cupcake-policies.py"
python3 "$repo_root/scripts/test-authority-agreement-signal.py"
python3 "$repo_root/scripts/test-idle-hold-signal.py"
python3 "$repo_root/scripts/test-native-ownership-vocab-signal.py"
python3 "$repo_root/scripts/test-stall-on-friction-signal.py"
command -v opa >/dev/null 2>&1 && opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/no_authority_agreement.rego" "$repo_root/.cupcake/policies/claude/no_authority_agreement_reminder.rego" "$repo_root/.cupcake/tests/no_authority_agreement_test.rego" "$repo_root/.cupcake/tests/no_authority_agreement_reminder_test.rego" "$repo_root/.cupcake/policies/claude/idle_hold.rego" "$repo_root/.cupcake/policies/claude/idle_hold_reminder.rego" "$repo_root/.cupcake/tests/idle_hold_test.rego" "$repo_root/.cupcake/tests/idle_hold_reminder_test.rego" "$repo_root/.cupcake/policies/claude/native_ownership_vocab_reminder.rego" "$repo_root/.cupcake/tests/native_ownership_vocab_reminder_test.rego" "$repo_root/.cupcake/policies/claude/block_manual_pgrep.rego" "$repo_root/.cupcake/tests/block_manual_pgrep_test.rego" "$repo_root/.cupcake/policies/claude/bash_elden_ring_launch_guard.rego" "$repo_root/.cupcake/tests/bash_elden_ring_launch_guard_test.rego" "$repo_root/.cupcake/policies/claude/block_askuserquestion.rego" "$repo_root/.cupcake/tests/block_askuserquestion_test.rego" "$repo_root/.cupcake/policies/claude/block_askuserquestion_reminder.rego" "$repo_root/.cupcake/tests/block_askuserquestion_reminder_test.rego" "$repo_root/.cupcake/policies/claude/no_stall_on_friction.rego" "$repo_root/.cupcake/tests/no_stall_on_friction_test.rego"
command -v opa >/dev/null 2>&1 && opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/git_block_main_push.rego" "$repo_root/.cupcake/tests/git_block_main_push_test.rego"
command -v opa >/dev/null 2>&1 && opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/git_block_main_commit.rego" "$repo_root/.cupcake/tests/git_block_main_commit_test.rego"
python3 "$repo_root/scripts/check-no-lossy-utf8.py"
# LINT PARITY WITH ../fromsoftware-rs. Standing user requirement (2026-08-21): this code must
# be AT LEAST as strict as the parent project. Cargo cannot inherit that -- `[lints] workspace =
# true` resolves only against THIS workspace root and lint levels never propagate from a path
# dependency -- so parity is asserted rather than inherited. The gate READS upstream's CI and
# manifests, so it goes red when upstream gets stricter instead of us finding out months later.
# It also fails if a blanket `-Awarnings` returns to .cargo/config.toml, which silently defeats
# `[workspace.lints.rust] warnings = "deny"` (measured, not theorised). Selftest first, so the
# gate is never trusted on its own say-so.
python3 "$repo_root/scripts/check-lint-parity.py" --selftest
python3 "$repo_root/scripts/check-lint-parity.py"
# FNV-1a has one zero-dependency owner below every caller. Prove the scanner catches copied
# implementations before trusting the live ownership check.
python3 "$repo_root/scripts/check-fnv1a-owner.py" --selftest
python3 "$repo_root/scripts/check-fnv1a-owner.py"
# One game address must have exactly ONE literal declaration. Divergent names for one address are
# divergent CLAIMS about what it is; three turned out to be wrong RE facts shipping in the DLL
# (bd rva-67b750-is-save-write-not-continue-load-2026-08-01,
# rva-4852f88-is-saveload2-slsystemimpl-not-fd4-io-worker-2026-08-01). Selftest first, so the gate
# is never trusted on its own say-so.
python3 "$repo_root/scripts/check-rva-alias-drift.py" --selftest
python3 "$repo_root/scripts/check-rva-alias-drift.py"
# The in-memory CS::ProfileSummary record layout is cross-cutting RAM ABI, not feature-owned data.
# Keep its typed definition in er-game-base and reject copied numeric offsets/formulas elsewhere.
python3 "$repo_root/scripts/check-profile-summary-layout.py" --selftest
python3 "$repo_root/scripts/check-profile-summary-layout.py"
# A log describes exactly ONE process run. er-invasion-warp-dll appended to a fixed filename, so
# twelve launches became one 565KB file and a count over it read as one run's behaviour. Every
# appending opener must route through er-game-base's one-shot truncation. Selftest first, so the
# gate is never trusted on its own say-so.
python3 "$repo_root/scripts/check-fresh-run-logs.py" --selftest
python3 "$repo_root/scripts/check-fresh-run-logs.py"
# The refactor/move DLL byte-identity gate (.github/workflows/refactor-byte-identical.yml) has two
# halves that can each rot silently: the trigger (which PRs it applies to) and the comparator (what
# counts as a difference). Both are tested here -- a gate whose scope and whose normalizer are
# untested is decorative.
bash "$repo_root/scripts/test-pr-refactor-scope.sh"
python3 "$repo_root/scripts/test-dll-byte-identical.py"
python3 "$repo_root/scripts/test-release-workflow.py"
python3 "$repo_root/scripts/check-rust-file-sizes.py"
python3 "$repo_root/scripts/check-experiments-rustfmt.py"
python3 "$repo_root/scripts/check-crate-extraction-roadmap.py" --selftest
python3 "$repo_root/scripts/check-crate-extraction-roadmap.py"
python3 "$repo_root/scripts/check-markdown-code-blocks.py" "$repo_root/README.md"
cargo fmt --all --manifest-path "$repo_root/Cargo.toml" -- --check
shellcheck "$repo_root/.githooks/pre-push"
shellcheck "$repo_root/scripts/check-no-local-main-commits.sh"
shellcheck "$repo_root/scripts/git-pre-push-block-main.sh"
shellcheck "$repo_root/scripts/test-git-pre-push-block-main.sh"
shellcheck "$repo_root/scripts/pr-refactor-scope.sh"
shellcheck "$repo_root/scripts/test-pr-refactor-scope.sh"
shellcheck "$repo_root/scripts/probe-dll-build-determinism.sh"
shellcheck "$repo_root/scripts/hooks/pre-push"
shellcheck "$repo_root/scripts/stage-autoload-release.sh"
shellcheck "$repo_root/scripts/run-product-continue-direct-probe.sh"
shellcheck "$repo_root/scripts/run-me3-product-smoke.sh"
shellcheck "$repo_root/scripts/run-windows-proof-render-smoke.sh"
shellcheck "$repo_root/scripts/run-portrait-dll-standalone-smoke.sh"
shellcheck "$repo_root/scripts/build-invasion-warp-profile.sh"
shellcheck "$repo_root/scripts/check-rust-build.sh"
shellcheck "$repo_root/scripts/er-stale-run-sentinel.sh"
shellcheck "$repo_root/scripts/test-er-stale-run-sentinel-e2e.sh"

# The stale-run sentinel kills a live game when an edit feeds a DLL that run loaded, so BOTH
# directions are load-bearing: a name it cannot match is a run it cannot stop, and a path it
# misclassifies is either contaminated evidence or a run killed mid-measurement. The selftest proves
# the classifier in both directions (a crate feeding a loaded DLL and its transitive dependencies
# tear down; host-side scripts, policy, docs and crates building UNLOADED DLLs do not), plus the
# `/proc/<pid>/comm` 15-character truncation handling end to end against a real process.
#
# It deliberately never calls `teardown` -- a real game may be live while this gate runs. The other
# half (/proc profile discovery + the kill itself) is proven by
# scripts/test-er-stale-run-sentinel-e2e.sh, which is NOT run here because it is destructive by
# design; run it by hand, and it refuses if a real run is live.
bash "$repo_root/scripts/er-stale-run-sentinel.sh" --selftest

# LAUNCH REACHABILITY GATE (2026-08-04). A launch takes the user's screen and yields one recording;
# spending it on a predicate that CANNOT fire returns a clean-looking run that proves nothing. The
# selftest runs first and includes the concrete regression -- the `requestCode latches 2` terminator
# that shipped and could never execute -- so the gate is never trusted on its own say-so.
python3 "$repo_root/scripts/er-launch-gate.py" --selftest

# Host-buildable GFx codec + derived-movie proof gates. These are the only place the runtime GFx
# transforms are checked (the Windows-target `cargo xwin test --lib` below cannot reach an integration
# test), and they carry the System->Quit grid-geometry gate: the two added rows are navigable only
# because the derived movie names them `Item_1_0`/`Item_1_1`. Movie-reading tests SKIP when the local
# extraction corpus is absent, so this is safe on a machine without it.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-gfx

# Scaleform's native hook owner stays host-testable at its dependency-injection seam even
# before R24 moves the first hook family. The er-gfx architecture test above enforces the
# one-way codec dependency; this test proves the narrow callback remains inert-by-default
# and install-once.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-scaleform-hooks --lib

# er-save-loader's host-portable save decoding: BND4 slot bodies + the PlayerGameData
# stats/vitals reads the loading-screen stats panel sources pre-mount. Save-byte tests are
# corpus-gated (skip when local save-files/ fixtures are absent; game-derived bytes are
# never versioned).
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-save-loader

# Host simulation for the own-load terminal-rejection state machine. It drives the preserved
# 120,959-tick churn shape and requires exactly one resolver call plus zero repeated rejections.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-save-redirect --lib

# er-loading-portrait's host-portable stats-line layer: proves the UNIFIED loading-screen
# stats layout (one five-line panel whether the values came from the save slot or live
# PlayerGameData, bd er-effects-rs-qic7). The bitmap-geometry test is corpus-gated on the
# extracted menu font (ER_FONT_GFX_PATH overridable) and skips when absent.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-loading-portrait

# The save-picker crate split (docs/plans/save-picker-crate-extraction.md). The row model
# and the quit-row resolver are pure logic, so the HOST run is their real coverage -- the
# whole point of the extraction is that state machines which today need a game launch
# become `cargo test`-able. The DLL shells' tests prove the host seam installs exactly
# once. `check-rust-build.sh` keeps all four building for the shipping target.
cargo test --manifest-path "$repo_root/Cargo.toml" \
	-p er-save-picker -p er-save-picker-dll -p er-quit-menu -p er-quit-menu-dll

# The world-map invasion-spawn warp crates (docs/plans/world-map-invasion-warp.md). The
# catalog, the block grouping, the BlockId disk/memory byte-order conversion and the on-disk
# `.aip` decoder are all pure logic, so the HOST run is their real coverage -- that
# testability is the point of the crate split. The corpus test that decodes the 365 real
# `.aip` files skips when the local extraction is absent (game-derived bytes are never
# versioned). `check-rust-build.sh` keeps both crates building for the shipping target.
cargo test --manifest-path "$repo_root/Cargo.toml" \
	-p er-invasion-warp -p er-invasion-warp-dll

# er-telemetry's host-portable logic. The workspace pins `default-members` to the DLL crate, so the
# windows-target `cargo xwin test --lib` below selects er-effects-rs ONLY and never ran these -- a
# telemetry-crate test module could be added and silently never execute in any gate. The load-count
# consistency logic is pure integer arithmetic with no platform semantics, so the host run is the
# real coverage; the cross-compile check in check-rust-build.sh keeps it building for the shipping
# target too.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-telemetry --lib

# Rust format + Windows-target BUILD of the injectable DLL (cross-compiled from Linux via
# cargo-xwin). A real build (not just `cargo check`) so codegen/link regressions -- including
# any pre-existing rust breakage -- are caught here, producing the linked er_effects_rs.dll.
# The linking gate above is only as good as its list. `check-rust-build.sh` carries an
# `me3_shells` array of every ME3-loadable cdylib and links each one, but that array was kept
# correct by a COMMENT saying "keep this list in sync" -- so adding a new DLL crate would leave
# the suite green while nothing ever linked it, which is the same hole the array closed, one
# level up. This makes the list's completeness executable.
python3 "$repo_root/scripts/check-me3-shell-coverage.py" --selftest
python3 "$repo_root/scripts/check-me3-shell-coverage.py"

# Knowing every shell exists is not knowing which of them can share a process. Five pairs
# corrupt each other -- two MinHook instances on one prologue, two D3D12 Present compositors,
# a harness that drives input every frame -- and that knowledge used to live only as prose in
# a hand-written ~/Elden/*.me3. scripts/er-dll-closure.py now reads it as data to decide what a
# generated profile may load, so the table must stay complete: a new cdylib that nobody has
# classified is exactly the one a dependency-closure walk auto-includes.
python3 "$repo_root/scripts/check-me3-dll-conflicts.py" --selftest
python3 "$repo_root/scripts/check-me3-dll-conflicts.py"

# The branch-launch pipeline. Each stage refuses rather than guessing, and each carries its own
# selftest for the refusal it exists to make -- a stale DLL, an unrankable conflict, a save with
# no decoded identity, a block printed without the DLL's testimony.
python3 "$repo_root/scripts/er_run_lib.py"
python3 "$repo_root/scripts/er-dll-closure.py" --selftest
python3 "$repo_root/scripts/er-dll-provenance.py" --selftest
python3 "$repo_root/scripts/er-pick-save.py" --selftest
python3 "$repo_root/scripts/er-gen-me3-profile.py" --selftest
python3 "$repo_root/scripts/er-run-reaper.py" --selftest
python3 "$repo_root/scripts/er-run-branch.py" --selftest

# Product D3 contract: the customized quit menu is an rlib dependency inside the one shipped
# er_effects_rs.dll. Its standalone DLL remains an explicitly-built harness and must never leak into
# the default build, staged product payload, or required ME3 native list.
python3 "$repo_root/scripts/check-single-dll-product-contract.py" --selftest
python3 "$repo_root/scripts/check-single-dll-product-contract.py"

bash "$repo_root/scripts/check-rust-build.sh"

# Dead/unused code in the save-disable DLL, on its shipping target. Scoped to that one
# crate on purpose: the repo builds with a global `-Awarnings`, so this is the narrow
# place where warning-freedom is both achievable today and load-bearing -- the crate's
# whole job is to stop saves, and two dead helpers already survived a refactor unseen.
python3 "$repo_root/scripts/check-save-disable-warnings.py"
