#!/usr/bin/env bash
# Cupcake signal: er_launch_authorized
#
# Emits "1" when the CURRENT agent session has been authorized by the user to launch Elden Ring, else
# "0". Consumed by the PreToolUse policy er_launch_requires_session_auth, which DENIES approved ER-launch
# commands (me3 launch / eldenring.exe / renderdoccmd capture / the repo's run-*/launch-*/capture-*
# launcher scripts) unless this signal is "1". It does NOT touch the existing forbidden-form guard
# (bash_elden_ring_launch_guard) -- Steam/EAC/start_protected_game/ersc.dll stay unconditionally blocked.
#
# WHY (user directive 2026-07-23): the prose-only "gate every ER launch on explicit user approval"
# directive (bd er-effects-rs-pe98) FAILED -- an agent auto-launched a vanilla-refresh run without
# asking. The user asked for EXECUTABLE enforcement plus a per-session keyphrase escape hatch. This
# mirrors the transcript-scan pattern of last_assistant_idle_hold / last_assistant_authority_agreement:
# the CURRENT session is the newest transcript jsonl in ~/.claude/projects/<cwd-key>/, and authorization
# is a keyphrase the USER typed in that session's own prompts.
#
# PER-LAUNCH BY CONSTRUCTION (user directive 2026-07-24 "gate each and every launch"): the signal reads
# ONLY the user's MOST RECENT prompt. The keyphrase authorizes launches for that prompt/turn ONLY; it
# does NOT persist across the session -- the next user prompt without the keyphrase re-locks launches.
# Session-bounded (a brand-new session starts unauthorized). Superseded the session-scoped scan where the
# keyphrase anywhere in the transcript authorized the whole session.
#
# KEYPHRASES (rename here in ONE place; matched case-insensitively; quoted/backticked spans are stripped
# first so that DISCUSSING the phrase does not authorize a launch):
#   AUTH   : "LAUNCH CLEARANCE GRANTED"  -> authorize the launch(es) driven from THIS prompt
#   REVOKE : "LAUNCH CLEARANCE REVOKED"  -> explicit deny (a later occurrence in the same prompt wins)
#
# FAIL-CLOSED: any error, missing transcript, or absent keyphrase -> "0" (deny). A launch gate must
# never fail open. The `|| printf 0` backstops a missing/broken python; python itself try/excepts to 0.
set -uo pipefail
python3 - <<'PY' 2>/dev/null || printf '0'
import glob
import json
import os
import re
import sys

AUTH = "launch clearance granted"
REVOKE = "launch clearance revoked"


def emit(v):
    sys.stdout.write("1" if v else "0")


try:
    # TEST-ONLY diagnostic override (NEVER set in production): point the scan at a fixture transcript so
    # the keyphrase gate can be unit-tested deterministically. Does not change product behavior -- when
    # unset (always, in real sessions) the signal reads the current session's real transcript below.
    override = os.environ.get("ER_LAUNCH_AUTH_TRANSCRIPT_FILE")
    if override is not None:
        files = [override] if override and os.path.exists(override) else []
    else:
        cwd = os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd()
        key = cwd.replace("/", "-")
        tdir = os.path.join(os.path.expanduser("~/.claude/projects"), key)
        files = sorted(
            glob.glob(os.path.join(tdir, "*.jsonl")),
            key=lambda p: os.path.getmtime(p),
            reverse=True,
        )
        if not files:
            # cupcake runs signals in a cwd/env where CLAUDE_PROJECT_DIR/getcwd does NOT resolve to
            # this session's transcript dir (verified 2026-07-24: the signal returns "1" run directly
            # with cwd=repo but "0" via cupcake even with CLAUDE_PROJECT_DIR=repo set). Fall back to the
            # newest transcript across ALL projects -- the actively-written current session is the
            # most-recently-modified .jsonl anywhere under ~/.claude/projects/.
            files = sorted(
                glob.glob(os.path.join(os.path.expanduser("~/.claude/projects"), "*", "*.jsonl")),
                key=lambda p: os.path.getmtime(p),
                reverse=True,
            )
    if not files:
        emit(False)
        sys.exit(0)

    def user_prompt_text(ev):
        """Text of a GENUINE user prompt, or None. Tool-result carrier 'user' events (the harness
        handing tool output back mid-turn) are NOT prompts and must not carry authorization."""
        if ev.get("type") != "user":
            return None
        content = ev.get("message", {}).get("content")
        if isinstance(content, str):
            return content
        if isinstance(content, list):
            parts = []
            for block in content:
                if isinstance(block, dict):
                    if block.get("type") == "tool_result":
                        return None  # tool-result carrier, not a real prompt
                    if block.get("type") == "text" and block.get("text"):
                        parts.append(block["text"])
            return "\n".join(parts) if parts else None
        return None

    # PER-LAUNCH gating (user directive 2026-07-24: "gate each and every launch"): authorize ONLY when
    # the keyphrase is in the user's MOST RECENT prompt, so every launch requires a fresh clearance in
    # the latest message. A one-time grant does NOT persist across the session -- once the user sends any
    # later prompt without the keyphrase, launches are denied again. (Superseded the session-scoped scan
    # where the keyphrase anywhere in the transcript authorized the whole session.)
    last_prompt = None
    with open(files[0], encoding="utf-8", errors="replace") as fh:
        for line in fh:
            try:
                ev = json.loads(line)
            except ValueError:
                continue
            txt = user_prompt_text(ev)
            if txt is not None:
                last_prompt = txt
    authorized = False
    if last_prompt is not None:
        # Strip quoted/backticked spans so DISCUSSING the phrase does not authorize a launch.
        scrubbed = re.sub(r'"[^"]*"', " ", last_prompt)
        scrubbed = re.sub(r"'[^']*'", " ", scrubbed)
        scrubbed = re.sub(r"`[^`]*`", " ", scrubbed)
        low = scrubbed.lower()
        ai = low.rfind(AUTH)
        ri = low.rfind(REVOKE)
        authorized = ai != -1 and ai > ri  # granted, and no revoke later in the same prompt
    emit(authorized)
except Exception:
    emit(False)
PY
