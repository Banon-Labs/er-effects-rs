#!/usr/bin/env bash
# Cupcake signal: last_assistant_fix_claim_without_runtime_evidence
#
# Scans the most recently completed assistant turn and emits one facts line when that turn changed
# code that ships inside the game, called the change a fix, and never opened anything a run wrote:
#
#   FIXFACTS|claim=..|changed=..|evidence=..|hedged=..|blocked=..|hostobject=..
#
# A clean turn emits nothing (fail-open, like every neighbouring signal).
#
# The failure this exists to refuse
# ---------------------------------
# User, 2026-09-11: "If someone ever says real fix to me and doesn't put it in airquotes, I look at
# them like this" -- and then, when nothing had stopped it: "We are supposed to have a rego policy
# that stops you from saying 'fix' without runtime evidence". The turn had edited a crate that ships
# inside a loaded DLL, built it, never launched the game, and closed on "the real fix". It failed
# live on the next launch, which is the cost the word had already spent.
#
# What counts as runtime evidence, and what does not
#   counts     -- the turn opened something a run of the game produced, at or after the edit: an
#                 `er-<name>.log` written by a loaded DLL, an `er-<name>telemetry<name>.json`, one
#                 of the streamed `er-<name>.jsonl` records, a path under the `er-me3-runs` run
#                 root, a `br-<date>-<time>-<id>` run id, a named `oracle_<field>`, a live read
#                 through `scripts/er-live-fields.py`, `scripts/er-teardown.py --status`,
#                 `scripts/er-readiness-watch.py`, or `scripts/er-frida-watch.py`.
#   never       -- a build, however green. `cargo xwin build`, `cargo test`, `cargo check`,
#                 `scripts/er-build-dlls.sh` and a `sha256sum` of the artifact all say the code
#                 compiles. A launch says the game starts: `scripts/er-run-branch.py` and
#                 `~/Elden/launch.sh` are not in the allowlist either, because a process appearing
#                 is not a behaviour being observed.
#
# Ordering is half the rule. Evidence counts only from the first edit onward, because a log read
# before the change describes the code that was there before it. Measured from the first such edit
# rather than the last, so a turn that reads the log and then makes one more small edit still counts
# as having looked -- the lenient reading of a rule whose false positive gags an honest report.
#
# The exemptions, each a case where the word is being used honestly
#   hedged     -- the closing prose says somewhere that the change is unverified, untested, has not
#                 run, needs a run, or is ready for the next one. An honest hedge is the behaviour
#                 being asked for and must never be punished. Scanned over the whole closing run,
#                 not the matched sentence, so a claim in one paragraph and its hedge in the next
#                 still passes.
#   blocked    -- a dependency the agent cannot dissolve: sudo, a credential, an observation only
#                 the user can make, or a decision handed back to them.
#   hostobject -- the sentence says a gate, a check, a test or a lint was fixed and names nothing
#                 that lives inside the game. A green gate is the proof of host work, and demanding
#                 a game launch for a clippy lint is how a guard earns a reputation for being wrong.
#   changed=0  -- the turn edited no crate that can reach a DLL. Host work under `scripts/`, `docs/`
#                 or `.cupcake/`, and the two host-only crate directories, are all outside the rule:
#                 a run cannot show any of them working or failing.
#
# Why the neighbouring Stop guards do not catch it
#   * `last_assistant_proof_without_observation` reads one word, `proven`, measured against the
#     corpus. A turn that says "the real fix" never says "proven", which is the whole reason the
#     stronger word gets avoided.
#   * `last_assistant_unbacked_claim` asks whether an artifact the turn claims to have built exists.
#     Here it does exist -- the edit was real, and only the claim about its effect is unbacked.
#   * `last_assistant_diagnosis_without_fix` turns on whether a file changed. One did.
#   * `git_require_runtime_evidence` gates `git push`, not prose, so a turn that says it and never
#     pushes is invisible to it.
#
# Fenced code, backtick spans and double-quoted spans are stripped before matching, so quoting this
# file, the policy, or the user's own words cannot trip it -- and a scare-quoted "fix" stops being a
# claim for free, which is the airquotes the directive asks for. The shared half (transcript
# discovery, turn bucketing, prose runs) comes from `scripts/cupcake_turn_scan.py` and the
# classification from `scripts/cupcake_fix_claim.py`, so the guards cannot drift into disagreeing
# about the same turn. Fail-open (empty output) on any error.
#
# Measured before it shipped, over the 2,936 real turn boundaries in
# ~/.claude/projects/-home-banon-projects-er-mods-rs*/*.jsonl: 197 closing messages call something a
# fix, and one of them halts -- a hook change reported as fixed and committed with no run behind it.
# The four the exemptions cleared are in `scripts/test-fix-claim-classifier.py` as negatives.
set -uo pipefail
CUPCAKE_SIGNAL_REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")/../.." && pwd)"
export CUPCAKE_SIGNAL_REPO_ROOT
python3 - <<'PY' 2>/dev/null || true
import os, sys

repo_root = os.environ.get("CUPCAKE_SIGNAL_REPO_ROOT", ".")
sys.path.insert(0, os.path.join(repo_root, "scripts"))
try:
    import cupcake_turn_scan as scan
    import cupcake_fix_claim as fc
except Exception:
    sys.exit(0)  # fail open: a missing helper must never wedge a session

path = scan.latest_transcript()
if not path:
    sys.exit(0)
turn = scan.last_text_turn(scan.split_turns(scan.load_events(path)))
if turn is None:
    sys.exit(0)

# A turn that kept going past its last prose did not end on that prose, so the sentence was a
# working note between two tool calls rather than a claim delivered to anyone.
if turn.last_text_index < 0 or turn.tool_after(turn.last_text_index):
    sys.exit(0)

runs = turn.text_runs
if not runs:
    sys.exit(0)
closing = runs[-1]

claim = fc.fix_claim(closing)
if not claim:
    sys.exit(0)

changed, evidence = fc.runtime_evidence(turn.blocks, fc.runtime_crates(repo_root))
# The prose citing a run artifact counts as evidence on its own. Searched unscrubbed over the whole
# turn, the way the proof-without-observation guard searches its own evidence half: a run id most
# often arrives inside a fenced block, and a generous evidence search fails toward silence.
cited = evidence or fc.cites_run_artifact(turn.text)

print(
    "FIXFACTS|claim={}|changed={}|evidence={}|hedged={}|blocked={}|hostobject={}".format(
        claim,
        int(changed),
        int(cited),
        int(fc.hedged(closing)),
        int(fc.externally_blocked(closing)),
        int(fc.host_object(claim)),
    )
)
PY
