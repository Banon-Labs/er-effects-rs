# shellcheck shell=bash
# Shared one-shot user gate for real Elden Ring runtime launches.
#
# A real launch must pass a per-run token file through ER_EFFECTS_USER_RUNTIME_GO_TOKEN.
# The token file must contain the exact current run identity (RUN_DIR preferred,
# else ARTIFACT_DIR, else ME3_PROFILE) and is consumed before the launch boundary.
# One user "go" authorizes one token for one run. A retry/relaunch needs a new
# user "go" and a new token. Dry-runs/help/preflight-only paths should not call it.

require_user_runtime_go() {
  local token="${ER_EFFECTS_USER_RUNTIME_GO_TOKEN:-}"
  local run_id="${ER_EFFECTS_USER_RUNTIME_RUN_ID:-${RUN_DIR:-${ARTIFACT_DIR:-${ME3_PROFILE:-}}}}"
  if [[ -z "$token" || -z "$run_id" || ! -f "$token" ]]; then
    cat >&2 <<'EOF'
user-runtime-gate: refusing real Elden Ring runtime launch.
This requires a one-shot per-run token from the current user go:
  ER_EFFECTS_USER_RUNTIME_GO_TOKEN=<file containing this run's RUN_DIR/ARTIFACT_DIR/ME3_PROFILE>
The token is consumed before launch. Every retry/relaunch needs a fresh user go.
EOF
    return 2
  fi
  local token_text
  # `read` returns NON-ZERO when it hits EOF without a trailing newline, having already stored the
  # line it read. The old `|| token_text=""` threw that value away, so a perfectly valid token file
  # written with `printf '%s'` was refused and the diagnostic below printed an empty `token:` --
  # which reads as "the file is empty" and sends the next person hunting the wrong fault (observed
  # 2026-08-07). Keep what was read; an actually-empty file still fails the exact-match test below.
  local token_text_status=0
  IFS= read -r token_text < "$token" || token_text_status=$?
  if [[ $token_text_status -ne 0 && -z "$token_text" ]]; then
    token_text=""
  fi
  if [[ "$token_text" != "$run_id" ]]; then
    cat >&2 <<EOF
user-runtime-gate: refusing real Elden Ring runtime launch.
Token does not match this run.
expected: $run_id
token:    $token_text
Every retry/relaunch needs a fresh user go and a token for that exact run.
EOF
    return 2
  fi
  rm -f -- "$token"
}
