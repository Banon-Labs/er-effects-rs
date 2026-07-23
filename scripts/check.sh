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
command -v cupcake >/dev/null 2>&1 || {
	echo "missing required command: cupcake" >&2
	exit 127
}
cupcake validate --log-level error
python3 "$repo_root/scripts/test-cupcake-policies.py"
python3 "$repo_root/scripts/test-authority-agreement-signal.py"
python3 "$repo_root/scripts/test-idle-hold-signal.py"
python3 "$repo_root/scripts/test-native-ownership-vocab-signal.py"
command -v opa >/dev/null 2>&1 && opa test "$repo_root/.cupcake/system/commands.rego" "$repo_root/.cupcake/policies/claude/no_authority_agreement.rego" "$repo_root/.cupcake/policies/claude/no_authority_agreement_reminder.rego" "$repo_root/.cupcake/tests/no_authority_agreement_test.rego" "$repo_root/.cupcake/tests/no_authority_agreement_reminder_test.rego" "$repo_root/.cupcake/policies/claude/idle_hold.rego" "$repo_root/.cupcake/policies/claude/idle_hold_reminder.rego" "$repo_root/.cupcake/tests/idle_hold_test.rego" "$repo_root/.cupcake/tests/idle_hold_reminder_test.rego" "$repo_root/.cupcake/policies/claude/native_ownership_vocab_reminder.rego" "$repo_root/.cupcake/tests/native_ownership_vocab_reminder_test.rego" "$repo_root/.cupcake/policies/claude/block_manual_pgrep.rego" "$repo_root/.cupcake/tests/block_manual_pgrep_test.rego"
python3 "$repo_root/scripts/check-no-lossy-utf8.py"
python3 "$repo_root/scripts/check-rust-file-sizes.py"
python3 "$repo_root/scripts/check-markdown-code-blocks.py" "$repo_root/README.md"
cargo fmt --all --manifest-path "$repo_root/Cargo.toml" -- --check
shellcheck "$repo_root/scripts/check-no-local-main-commits.sh"
shellcheck "$repo_root/scripts/stage-autoload-release.sh"
shellcheck "$repo_root/scripts/run-product-continue-direct-probe.sh"
shellcheck "$repo_root/scripts/run-me3-product-smoke.sh"
shellcheck "$repo_root/scripts/check-rust-build.sh"

# Rust format + Windows-target BUILD of the injectable DLL (cross-compiled from Linux via
# cargo-xwin). A real build (not just `cargo check`) so codegen/link regressions -- including
# any pre-existing rust breakage -- are caught here, producing the linked er_effects_rs.dll.
bash "$repo_root/scripts/check-rust-build.sh"
