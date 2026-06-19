#!/usr/bin/env bash
# Loop guard for the upstream-replacement autoresearch run.
#
# The user-chosen guard is `scripts/check.sh`, but that script is RED at the
# branch baseline for a reason ORTHOGONAL to this loop: a pre-existing
# cupcake-policy-dev regression (`test-cupcake-policies.py` case
# `deny-semicolon-split` expects the bash_no_timeouts policy to DENY
# `echo one; echo two`, but the policy allows it). This failure exists at
# origin (81a191a) and cannot be caused or fixed by `src/*.rs` edits, and
# "fixing" the policy would force semicolon-splitting on the agent's own
# commands -- out of scope for replacing hand-rolled code with upstream.
#
# This loop only edits `src/*.rs`. The ONLY check.sh stages those edits can
# affect are the 4 Rust-correctness gates below, all GREEN at baseline. We run
# exactly those, so the guard is baseline-relative: it passes iff a kept change
# introduces no NEW failure. Run the full `scripts/check.sh` before pushing.
set -euo pipefail
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo_root"
python3 scripts/check-no-magic-numbers.py
python3 scripts/check-no-lossy-utf8.py
cargo fmt --manifest-path Cargo.toml -- --check
cargo xwin check --manifest-path Cargo.toml --target x86_64-pc-windows-msvc
