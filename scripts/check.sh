#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

bash "$repo_root/scripts/check-no-local-main-commits.sh"
python3 "$repo_root/scripts/check-no-timeouts.py"
python3 "$repo_root/scripts/test-no-timeouts.py"
python3 "$repo_root/scripts/check-launch-guardrails.py" --audit
python3 "$repo_root/scripts/check-runtime-probe-contract.py" --audit
python3 "$repo_root/scripts/test-runtime-probe-contract.py"
python3 "$repo_root/scripts/test-er-readiness-watch.py"
python3 "$repo_root/scripts/test-save-slot-oracle.py"
python3 "$repo_root/scripts/test-detect-proc.py"
python3 "$repo_root/scripts/test-semaphore-watchdog.py"
python3 "$repo_root/scripts/test-input-harness-static.py"
python3 "$repo_root/scripts/check-autoload-happy-path.py"
python3 "$repo_root/scripts/test-autoload-happy-path.py"
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
command -v opa >/dev/null 2>&1 && opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/no_authority_agreement.rego" "$repo_root/.cupcake/policies/claude/no_authority_agreement_reminder.rego" "$repo_root/.cupcake/tests/no_authority_agreement_test.rego" "$repo_root/.cupcake/tests/no_authority_agreement_reminder_test.rego" "$repo_root/.cupcake/policies/claude/idle_hold.rego" "$repo_root/.cupcake/policies/claude/idle_hold_reminder.rego" "$repo_root/.cupcake/tests/idle_hold_test.rego" "$repo_root/.cupcake/tests/idle_hold_reminder_test.rego" "$repo_root/.cupcake/policies/claude/native_ownership_vocab_reminder.rego" "$repo_root/.cupcake/tests/native_ownership_vocab_reminder_test.rego" "$repo_root/.cupcake/policies/claude/block_manual_pgrep.rego" "$repo_root/.cupcake/tests/block_manual_pgrep_test.rego" "$repo_root/.cupcake/policies/claude/bash_elden_ring_launch_guard.rego" "$repo_root/.cupcake/tests/bash_elden_ring_launch_guard_test.rego" "$repo_root/.cupcake/policies/claude/block_askuserquestion.rego" "$repo_root/.cupcake/tests/block_askuserquestion_test.rego"
command -v opa >/dev/null 2>&1 && opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/git_block_main_push.rego" "$repo_root/.cupcake/tests/git_block_main_push_test.rego"
command -v opa >/dev/null 2>&1 && opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/git_block_main_commit.rego" "$repo_root/.cupcake/tests/git_block_main_commit_test.rego"
python3 "$repo_root/scripts/check-no-lossy-utf8.py"
python3 "$repo_root/scripts/check-rust-file-sizes.py"
python3 "$repo_root/scripts/check-markdown-code-blocks.py" "$repo_root/README.md"
cargo fmt --all --manifest-path "$repo_root/Cargo.toml" -- --check
shellcheck "$repo_root/scripts/check-no-local-main-commits.sh"
shellcheck "$repo_root/scripts/git-pre-push-block-main.sh"
shellcheck "$repo_root/scripts/stage-autoload-release.sh"
shellcheck "$repo_root/scripts/run-product-continue-direct-probe.sh"
shellcheck "$repo_root/scripts/run-me3-product-smoke.sh"
shellcheck "$repo_root/scripts/run-windows-proof-render-smoke.sh"
shellcheck "$repo_root/scripts/check-rust-build.sh"

# Host-buildable GFx codec + derived-movie proof gates. These are the only place the runtime GFx
# transforms are checked (the Windows-target `cargo xwin test --lib` below cannot reach an integration
# test), and they carry the System->Quit grid-geometry gate: the two added rows are navigable only
# because the derived movie names them `Item_1_0`/`Item_1_1`. Movie-reading tests SKIP when the local
# extraction corpus is absent, so this is safe on a machine without it.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-gfx

# er-save-loader's host-portable save decoding: BND4 slot bodies + the PlayerGameData
# stats/vitals reads the loading-screen stats panel sources pre-mount. Save-byte tests are
# corpus-gated (skip when local save-files/ fixtures are absent; game-derived bytes are
# never versioned).
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-save-loader

# er-loading-portrait's host-portable stats-line layer: proves the UNIFIED loading-screen
# stats layout (one five-line panel whether the values came from the save slot or live
# PlayerGameData, bd er-effects-rs-qic7). The bitmap-geometry test is corpus-gated on the
# extracted menu font (ER_FONT_GFX_PATH overridable) and skips when absent.
cargo test --manifest-path "$repo_root/Cargo.toml" -p er-loading-portrait

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
bash "$repo_root/scripts/check-rust-build.sh"

# Dead/unused code in the save-disable DLL, on its shipping target. Scoped to that one
# crate on purpose: the repo builds with a global `-Awarnings`, so this is the narrow
# place where warning-freedom is both achievable today and load-bearing -- the crate's
# whole job is to stop saves, and two dead helpers already survived a refactor unseen.
python3 "$repo_root/scripts/check-save-disable-warnings.py"
