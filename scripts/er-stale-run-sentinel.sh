#!/usr/bin/env bash
# Tear down a live Elden Ring run the moment an edited file CONTRIBUTES TO A DLL THAT RUN LOADED.
#
# WHY
# ---
# A live run has DLLs loaded from a particular state of this tree. The instant a file that feeds one
# of those DLLs changes, that run is STALE: its loaded code no longer matches the source, so anything
# observed in it is evidence about a build that no longer exists. Two concrete ways that bit us:
#
#   * a newly built DLL "validated" inside a process that had loaded the previous one;
#   * two live processes appending to the SAME log files next to the game exe, so one run's
#     lines land in the other's evidence -- and a per-process counter reading "#2" gets
#     misread as one process doing something twice.
#
# Advisory rules did not prevent either. AGENTS.md said to tear down; bd memories said to tear
# down; it kept happening, because an advisory rule only fires if it is recalled at the right
# moment. This sentinel does not rely on anyone remembering: it runs from the PostToolUse hook
# that already fires on Edit/MultiEdit/Write, and kills the stale run automatically.
#
# WHY "GIT-TRACKED" WAS THE WRONG TEST
# ------------------------------------
# The first version tore down whenever the edited path was inside the repo and not gitignored. The
# invariant it enforced is right; the test was far too broad, because "the repo cares about this
# file" is not "this file is compiled into a DLL this run loaded". Every one of these killed a run
# mid-measurement on 2026-08-04, each costing a user-driven invasion, and NONE of them changes a
# single byte of any loaded DLL:
#
#   * scripts/frida-trace-ersc.py, scripts/frida-steam-matchmaking-trace.py -- HOST-side frida
#     scripts, never linked into anything;
#   * scripts/er-launch-gate.py -- a host-side gate that runs BEFORE a launch, not inside one;
#   * .cupcake/policies/claude/*.rego -- agent policy, not code at all (and a background subagent
#     writing one tore down a run the main agent had just launched, after which the teardown was
#     misattributed to the subagent).
#
# THE TEST NOW
# ------------
# Tear down only if the edited file plausibly contributes to a DLL the CURRENTLY RUNNING profile
# loads. In order:
#
#   1. outside the repo, or gitignored inside it   -> SKIP (build output and logs never make a run
#      stale; that is what stops a log line killing a run)
#   2. nothing live                                -> SKIP (nothing at stake, and no cargo run)
#   3. the loaded-DLL set cannot be determined     -> FALLBACK: tear down (see FAIL SAFE below)
#   4. inside a workspace crate                    -> tear down IFF that package is in the
#      dependency closure of the loaded cdylibs, else SKIP
#   5. under an inert top-level directory          -> SKIP
#   6. anything else                               -> FALLBACK: tear down
#
# The loaded-DLL set is ground truth, not a guess: the live `me3` process's command line carries
# `-p <profile>`, the profile is TOML whose `[[natives]]` entries name the exact DLLs loaded, and
# `cargo metadata` maps each of those DLL filenames back to the package that emits it. Deriving the
# filename from the package name would be WRONG -- four crates override `[lib] name`
# (er-ags-stub -> amd_ags_x64.dll, er-inventory-sort-dll -> er_inventory_sort.dll, ...), which is
# the same trap scripts/check-me3-shell-coverage.py exists to catch -- so the cdylib TARGET name
# from cargo metadata is used instead. Nothing here is hardcoded.
#
# DEPENDENCY CLOSURE IS THE LOAD-BEARING PART. Editing crates/er-invasion-warp/src/*.rs changes
# er_invasion_warp_dll.dll even though the crate names differ, and crates/er-game-base changes both
# product DLLs. The closure is walked over cargo metadata's path-dependency edges, ALL kinds
# included (normal, build and dev): a dev-dependency cannot really reach the cdylib, but counting it
# only over-triggers, and over-triggering is the safe direction.
#
# COST, MEASURED NOT GUESSED
# --------------------------
# This runs on EVERY edit, so the added latency was measured rather than assumed (15 runs each,
# this machine, 2026-08-04):
#
#   gate 1 short-circuit (outside repo / gitignored)   ~25 ms   -- unchanged from the old sentinel
#   full classify, nothing else running                ~57 ms   -- +~32 ms
#   full classify, game running and eating CPU        ~125 ms   -- +~95 ms
#
# `cargo metadata --no-deps` is 22 ms of that. Crucially the classify step runs ONLY when a run is
# actually live: with nothing running the hook short-circuits before cargo is ever invoked, which is
# the overwhelmingly common case. So no cache. A cached crate map would also fail in the WRONG
# direction -- a map stale by one Cargo.toml edit reports "not in the closure" for a crate that now
# is, i.e. it fails open, silently, which is the exact failure mode this whole file exists to avoid.
#
# FAIL SAFE, NOT OPEN
# -------------------
# If the profile cannot be read, the crate map cannot be built, or the path cannot be classified,
# fall back to the ORIGINAL behaviour and tear down. A sentinel that silently stops protecting is
# worse than one that is occasionally too eager, because its failure mode is contaminated evidence
# that nobody notices. Every decision is logged with the branch that produced it.
#
# THE LOG
# -------
# Twice on 2026-08-04 "what tore this run down?" could not be answered after the fact -- the reason
# went to the hook's stdout and nowhere else -- and both guesses were wrong (frida-gadget once, a
# subagent once). Every check now appends one tab-separated line to $ER_SENTINEL_LOG (default
# ${XDG_STATE_HOME:-$HOME/.local/state}/er-effects-rs/stale-run-sentinel.log, outside the repo):
# timestamp, verdict, branch, edited path, reason, live profiles, killed pids. "Which edit killed
# the run" is now a lookup.
#
# USAGE
#   scripts/er-stale-run-sentinel.sh check <path>              # tear down if <path> feeds a loaded DLL
#   scripts/er-stale-run-sentinel.sh classify <path> [prof...] # print the verdict, kill nothing
#   scripts/er-stale-run-sentinel.sh teardown                  # unconditional teardown + verify
#   scripts/er-stale-run-sentinel.sh status
#   scripts/er-stale-run-sentinel.sh --selftest
#
# `--selftest` covers the CLASSIFIER in both directions and never calls teardown (scripts/check.sh
# runs it, and a real game may be live). The other half -- /proc discovery of the live profile, and
# the kill itself -- is proven against a decoy process by
# scripts/test-er-stale-run-sentinel-e2e.sh, which is destructive by design and so is run by hand.
#
# As a hook it reads the tool payload as JSON on stdin and extracts the edited path itself.
set -uo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

# Outside the repo on purpose: a log the sentinel writes must not itself look like a repo edit, and
# it has to survive `git clean`. Env-overridable and current-user-aware -- no /home/<someone>
# literals (AGENTS.md "Reusable Tooling / Hard-Coded Path Corrections").
SENTINEL_LOG="${ER_SENTINEL_LOG:-${XDG_STATE_HOME:-$HOME/.local/state}/er-effects-rs/stale-run-sentinel.log}"

# Matched EXACTLY against /proc/<pid>/comm -- never a substring match on the full command line,
# which is how `rsi`-matches-`version` style false positives happen.
#
# The kernel caps comm at TASK_COMM_LEN-1 = 15 characters, so a longer executable name NEVER
# appears in full. `start_protected_game.exe` is 24 characters: comparing it verbatim could not
# match any process that has ever existed, so that entry silently protected nothing. Each name is
# therefore compared against its 15-character truncation as well.
GAME_COMMS=("eldenring.exe" "me3" "start_protected_game.exe")

list_live() {
  python3 - "${GAME_COMMS[@]}" <<'PY'
import os, sys, glob

# TASK_COMM_LEN - 1. A name longer than this is stored truncated, so match both forms.
COMM_MAX = 15
names = set()
for raw in sys.argv[1:]:
    names.add(raw)
    names.add(raw[:COMM_MAX])
for d in glob.glob('/proc/[0-9]*'):
    try:
        comm = open(os.path.join(d, 'comm')).read().strip()
    except OSError:
        continue
    if comm in names:
        print(os.path.basename(d), comm)
PY
}

# Prefix each line of stdin with the sentinel tag. A shell loop rather than `sed` so the script
# stays free of shellcheck findings -- it is itself gated by scripts/check.sh, and a gate that
# tolerates its own noise stops being read.
prefix_lines() {
  local line
  while IFS= read -r line; do
    printf '[er-sentinel]   %s\n' "$line"
  done
}

# One tab-separated line per check. Never fatal: a hook that dies because a log directory is
# unwritable would be a worse failure than the missing line.
log_event() {
  local verdict="$1" branch="$2" path="$3" detail="$4" profiles="$5" killed="$6"
  local dir
  dir="$(dirname -- "$SENTINEL_LOG")"
  mkdir -p -- "$dir" 2>/dev/null || return 0
  printf '%s\t%s\t%s\t%s\t%s\tprofiles=%s\tkilled=%s\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$verdict" "$branch" "$path" "$detail" \
    "${profiles:--}" "${killed:--}" >>"$SENTINEL_LOG" 2>/dev/null || true
}

# Escalating teardown, then VERIFY. "I sent a signal" is not "it is gone", and every
# contamination so far came from assuming it was.
teardown() {
  local pids
  pids="$(list_live | awk '{print $1}')"
  if [[ -z "$pids" ]]; then
    return 0
  fi
  # shellcheck disable=SC2086
  kill -TERM $pids 2>/dev/null || true
  for _ in 1 2 3 4 5 6; do
    [[ -z "$(list_live)" ]] && break
    read -r -t 1 </dev/null 2>/dev/null || true
  done
  pids="$(list_live | awk '{print $1}')"
  if [[ -n "$pids" ]]; then
    # shellcheck disable=SC2086
    kill -KILL $pids 2>/dev/null || true
    for _ in 1 2 3 4 5 6; do
      [[ -z "$(list_live)" ]] && break
      read -r -t 1 </dev/null 2>/dev/null || true
    done
  fi
  [[ -z "$(list_live)" ]]
}

# Is this a path the repo cares about at all? Unchanged from the original sentinel, and still the
# FIRST gate -- it is what keeps build output and logs from killing a run.
#
# It deliberately does NOT use `git ls-files --error-unmatch` alone. That only matches files already
# committed, so CREATING a new source file -- which invalidates a loaded DLL exactly as much as
# editing an existing one -- slipped straight through: a new `.rs` is untracked until it is `git
# add`ed, the check returned false, and a live run was left running against a tree that had already
# changed. Observed 2026-08-04, on the very first edit after the sentinel was written.
# `git check-ignore` answers the question that was actually meant: is this a file the repo cares
# about, tracked yet or not.
is_repo_source() {
  local path="$1"
  [[ -n "$path" ]] || return 1
  # Resolve to a repo-relative path; anything outside the repo is not our source.
  local abs
  abs="$(cd -- "$(dirname -- "$path")" 2>/dev/null && pwd)/$(basename -- "$path")" || return 1
  case "$abs" in
    "$REPO_ROOT"/*) ;;
    *) return 1 ;;
  esac
  # Already committed -> unambiguously source.
  if git -C "$REPO_ROOT" ls-files --error-unmatch -- "$abs" >/dev/null 2>&1; then
    return 0
  fi
  # Not committed yet. A BRAND NEW source file is still source. The discriminator is whether git
  # ignores it: `check-ignore` exits 0 when the path IS ignored (target/, logs, scratch), which is
  # exactly the set that must NOT trigger a teardown.
  if git -C "$REPO_ROOT" check-ignore -q -- "$abs" 2>/dev/null; then
    return 1
  fi
  return 0
}

# Does <path> feed a DLL the live run loaded? Prints one tab-separated line:
#
#   VERDICT \t BRANCH \t DETAIL \t PROFILES
#
# VERDICT is TEARDOWN or SKIP. Any failure to answer prints TEARDOWN with a `fallback-*` branch --
# never SKIP, because a wrong SKIP silently contaminates evidence.
#
# With no profile arguments the live profiles are discovered from /proc. Explicit profile paths make
# the classification testable (and debuggable) without a game running.
classify_path() {
  local path="$1"
  shift
  local mode="discover"
  if [[ $# -gt 0 ]]; then
    mode="explicit"
  fi
  python3 - "$REPO_ROOT" "$path" "$mode" "$@" <<'PY'
import glob
import json
import os
import shutil
import subprocess
import sys
import tomllib

REPO_ROOT = os.path.realpath(sys.argv[1])
TARGET = sys.argv[2]
MODE = sys.argv[3]
EXPLICIT_PROFILES = sys.argv[4:]

# Bounded: this runs from a PostToolUse hook on every edit, and a cargo invocation that hangs would
# hang the agent. Well under the repo's 30s non-game cap (scripts/check-no-timeouts.py).
CARGO_TIMEOUT_SECONDS = 20.0
COMM_MAX = 15  # TASK_COMM_LEN - 1
ME3_COMM = "me3"
GAME_COMMS = ("eldenring.exe", "start_protected_game.exe")

# Top-level directories that cannot contribute to any DLL. Each is host-side tooling, agent
# configuration or prose -- nothing under them is compiled, linked or embedded. This list is
# CROSS-CHECKED below against cargo metadata: if a workspace crate ever appears under one of these,
# the directory stops being treated as inert and the path falls through to the fail-safe branch.
INERT_TOP_LEVEL = {
    "scripts",        # host-side helpers: frida scripts, launch gates, probe drivers, checks
    "docs",           # prose and recon notes
    "tests",          # repo-level agent-guard tests (TypeScript); builds no DLL
    ".beads",         # issue tracker data
    ".cupcake",       # agent policy (Rego) and its tests
    ".github",        # workflow files
    ".githooks",      # git hooks
    ".claude",        # Claude Code settings, skills, and agent worktrees
    ".agents",        # agent configuration
    ".pi",            # Pi harness configuration
}


def emit(verdict, branch, detail, profiles):
    print("\t".join([verdict, branch, detail, ",".join(profiles)]))
    raise SystemExit(0)


def proc_comm(pid):
    try:
        with open(f"/proc/{pid}/comm") as fh:
            return fh.read().strip()
    except OSError:
        return None


def proc_ppid(pid):
    try:
        with open(f"/proc/{pid}/stat") as fh:
            raw = fh.read()
    except OSError:
        return None
    # The comm field is parenthesised and may itself contain spaces/parens, so split after the LAST
    # ')' rather than on whitespace from the left.
    try:
        tail = raw[raw.rindex(")") + 2:].split()
        return int(tail[1])
    except (ValueError, IndexError):
        return None


def proc_argv(pid):
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as fh:
            return [a.decode("utf-8", "replace") for a in fh.read().split(b"\0") if a]
    except OSError:
        return []


def me3_profile_dir(argv):
    for i, a in enumerate(argv):
        if a == "--profile-dir" and i + 1 < len(argv):
            return argv[i + 1]
        if a.startswith("--profile-dir="):
            return a.split("=", 1)[1]
    base = os.environ.get("XDG_CONFIG_HOME") or os.path.join(os.path.expanduser("~"), ".config")
    return os.path.join(base, "me3", "profiles")


def resolve_profile(value, argv):
    """`-p` takes EITHER a path to a ModProfile OR a bare profile name in the me3 profile dir."""
    if os.path.isfile(value):
        return os.path.realpath(value)
    if os.sep in value:
        return None
    prof_dir = me3_profile_dir(argv)
    for ext in (".me3", ".toml", ".json"):
        cand = os.path.join(prof_dir, value + ext)
        if os.path.isfile(cand):
            return os.path.realpath(cand)
    return None


def discover_profiles():
    """Live me3 profiles, or (None, why) when the loaded set cannot be trusted."""
    me3_pids = set()
    game_pids = set()
    game_names = set()
    for name in GAME_COMMS:
        game_names.add(name)
        game_names.add(name[:COMM_MAX])
    for d in glob.glob("/proc/[0-9]*"):
        pid = int(os.path.basename(d))
        comm = proc_comm(pid)
        if comm is None:
            continue
        if comm == ME3_COMM:
            me3_pids.add(pid)
        elif comm in game_names:
            game_pids.add(pid)

    if not me3_pids and not game_pids:
        return [], None
    if game_pids and not me3_pids:
        # A game launched without a live me3 (direct/offline probe, or me3 already exited): there is
        # no profile to read, so the loaded DLL set is unknown.
        return None, "game-live-without-me3"

    # Every live game process must trace back to a live me3, otherwise its natives came from a
    # profile we are not looking at.
    for pid in game_pids:
        cur = pid
        hops = 0
        while cur and cur != 1 and hops < 32:
            if cur in me3_pids:
                break
            cur = proc_ppid(cur)
            hops += 1
        else:
            return None, f"game-pid-{pid}-has-no-live-me3-ancestor"
        if cur not in me3_pids:
            return None, f"game-pid-{pid}-has-no-live-me3-ancestor"

    profiles = []
    for pid in sorted(me3_pids):
        argv = proc_argv(pid)
        raw = None
        for i, a in enumerate(argv):
            if a in ("-p", "--profile") and i + 1 < len(argv):
                raw = argv[i + 1]
            elif a.startswith("--profile="):
                raw = a.split("=", 1)[1]
        if raw is None:
            return None, f"me3-pid-{pid}-has-no-profile-argument"
        resolved = resolve_profile(raw, argv)
        if resolved is None:
            return None, f"me3-pid-{pid}-profile-unresolvable"
        if resolved not in profiles:
            profiles.append(resolved)
    return profiles, None


def loaded_dll_stems(profiles):
    """Lowercased basenames (without `.dll`) of every native the profiles load."""
    stems = set()
    for prof in profiles:
        try:
            with open(prof, "rb") as fh:
                data = tomllib.load(fh)
        except (OSError, tomllib.TOMLDecodeError):
            return None, f"profile-unparseable:{os.path.basename(prof)}"
        natives = data.get("natives")
        if natives is None:
            natives = []
        if not isinstance(natives, list):
            return None, f"profile-natives-not-a-list:{os.path.basename(prof)}"
        for entry in natives:
            if not isinstance(entry, dict):
                return None, f"profile-native-not-a-table:{os.path.basename(prof)}"
            raw = entry.get("path")
            if not isinstance(raw, str) or not raw:
                return None, f"profile-native-without-path:{os.path.basename(prof)}"
            name = os.path.basename(raw.replace("\\", "/")).lower()
            if name.endswith(".dll"):
                name = name[: -len(".dll")]
            stems.add(name)
    return stems, None


def workspace_metadata():
    cargo = shutil.which("cargo")
    if cargo is None:
        cand = os.path.join(os.path.expanduser("~"), ".cargo", "bin", "cargo")
        cargo = cand if os.path.isfile(cand) else None
    if cargo is None:
        return None, "cargo-not-found"
    try:
        proc = subprocess.run(
            [cargo, "metadata", "--no-deps", "--format-version", "1",
             "--manifest-path", os.path.join(REPO_ROOT, "Cargo.toml")],
            cwd=REPO_ROOT,
            capture_output=True,
            timeout=CARGO_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.SubprocessError):
        return None, "cargo-metadata-failed"
    if proc.returncode != 0:
        return None, "cargo-metadata-nonzero"
    try:
        return json.loads(proc.stdout), None
    except json.JSONDecodeError:
        return None, "cargo-metadata-unparseable"


def main():
    target = os.path.realpath(TARGET)

    if MODE == "explicit":
        profiles, why = EXPLICIT_PROFILES, None
    else:
        profiles, why = discover_profiles()
    if profiles is None:
        emit("TEARDOWN", "fallback-profile-undiscoverable", why or "unknown", [])
    if not profiles:
        # Nothing live. `check` never reaches here (it short-circuits on an empty process list);
        # a bare `classify` can, and has nothing to measure against.
        emit("TEARDOWN", "fallback-no-live-profile", "no live me3 profile to classify against", [])

    stems, why = loaded_dll_stems(profiles)
    if stems is None:
        emit("TEARDOWN", "fallback-profile-unreadable", why, profiles)

    meta, why = workspace_metadata()
    if meta is None:
        emit("TEARDOWN", "fallback-crate-map-unavailable", why, profiles)

    pkg_dir = {}
    cdylibs = {}
    for pkg in meta.get("packages", []):
        name = pkg.get("name")
        manifest = pkg.get("manifest_path")
        if not name or not manifest:
            emit("TEARDOWN", "fallback-crate-map-unavailable", "package without name/manifest", profiles)
        pkg_dir[name] = os.path.realpath(os.path.dirname(manifest))
        cdylibs[name] = {
            t.get("name")
            for t in pkg.get("targets", [])
            if "cdylib" in (t.get("crate_types") or [])
        }
    if not pkg_dir:
        emit("TEARDOWN", "fallback-crate-map-unavailable", "no workspace packages", profiles)

    dir_to_pkg = {v: k for k, v in pkg_dir.items()}
    deps = {}
    for pkg in meta.get("packages", []):
        edges = set()
        for dep in pkg.get("dependencies", []):
            dep_path = dep.get("path")
            if not dep_path:
                continue
            owner = dir_to_pkg.get(os.path.realpath(dep_path))
            if owner:
                edges.add(owner)
        deps[pkg["name"]] = edges

    # Packages whose cdylib the live profiles actually load. The cdylib TARGET name is used, not the
    # package name: four crates override `[lib] name`, so deriving the filename would silently miss
    # them (the trap scripts/check-me3-shell-coverage.py exists to catch).
    loaded_pkgs = {name for name, libs in cdylibs.items() if libs & stems}
    if not loaded_pkgs:
        emit(
            "TEARDOWN",
            "fallback-no-workspace-dll-loaded",
            "live profile loads no DLL this workspace builds: " + ",".join(sorted(stems)),
            profiles,
        )

    # Forward closure over path-dependency edges: everything a loaded cdylib compiles in. Kept
    # PER loaded cdylib as well as unioned, so the logged reason can name the DLL a given crate
    # actually reaches instead of listing every DLL in the profile.
    def forward(seed):
        seen = set(seed)
        stack = list(seed)
        while stack:
            cur = stack.pop()
            for nxt in deps.get(cur, ()):
                if nxt not in seen:
                    seen.add(nxt)
                    stack.append(nxt)
        return seen

    per_dll = {p: forward({p}) for p in loaded_pkgs}
    closure = set().union(*per_dll.values())

    # Deepest owning crate wins, so a nested crate is attributed to itself rather than its parent.
    owner = None
    owner_dir = ""
    for name, d in pkg_dir.items():
        if target == d or target.startswith(d + os.sep):
            if len(d) > len(owner_dir):
                owner, owner_dir = name, d
    if owner is not None:
        if owner in closure:
            via = sorted(
                f"{lib}.dll"
                for p, reach in per_dll.items()
                if owner in reach
                for lib in (cdylibs[p] & stems)
            )
            return emit(
                "TEARDOWN",
                "crate-feeds-loaded-dll",
                f"pkg={owner} feeds {','.join(via)}",
                profiles,
            )
        return emit(
            "SKIP",
            "crate-builds-no-loaded-dll",
            f"pkg={owner} is not in the dependency closure of " + ",".join(sorted(stems)),
            profiles,
        )

    rel = os.path.relpath(target, REPO_ROOT)
    top = rel.split(os.sep, 1)[0]
    if top in INERT_TOP_LEVEL:
        # Cross-check the allowlist against reality: if a workspace crate has moved under this
        # directory, it is no longer inert and must not be skipped wholesale.
        crate_under = [n for n, d in pkg_dir.items() if d == os.path.join(REPO_ROOT, top)
                       or d.startswith(os.path.join(REPO_ROOT, top) + os.sep)]
        if crate_under:
            emit(
                "TEARDOWN",
                "fallback-inert-dir-holds-crates",
                f"{top}/ is allowlisted as inert but now contains: " + ",".join(sorted(crate_under)),
                profiles,
            )
        emit("SKIP", "inert-directory", f"{top}/ compiles into no DLL", profiles)

    emit(
        "TEARDOWN",
        "fallback-unclassified",
        f"{rel} is repo source but belongs to no crate and no inert directory",
        profiles,
    )


main()
PY
}

cmd_check() {
  local path="${1:-}"
  if ! is_repo_source "$path"; then
    exit 0
  fi
  local rel="${path#"$REPO_ROOT"/}"
  local live
  live="$(list_live)"
  if [[ -z "$live" ]]; then
    log_event "NOLIVE" "no-live-run" "$rel" "nothing to invalidate" "" ""
    exit 0
  fi

  local verdict branch detail profiles line
  line="$(classify_path "$path")"
  if [[ -z "$line" ]]; then
    verdict="TEARDOWN"; branch="fallback-classifier-crashed"
    detail="classifier produced no verdict"; profiles=""
  else
    IFS=$'\t' read -r verdict branch detail profiles <<<"$line"
  fi

  if [[ "$verdict" != "TEARDOWN" ]]; then
    log_event "SKIP" "$branch" "$rel" "$detail" "$profiles" ""
    exit 0
  fi

  local killed
  killed="$(echo "$live" | awk '{printf "%s%s:%s", (NR>1?",":""), $1, $2}')"
  if teardown; then
    log_event "TEARDOWN" "$branch" "$rel" "$detail" "$profiles" "$killed"
    echo "[er-sentinel] TORE DOWN the live Elden Ring run: '$rel' feeds a DLL that run loaded, so" >&2
    echo "[er-sentinel] anything observed in it would be evidence about a build that no longer" >&2
    echo "[er-sentinel] exists. Rebuild and relaunch." >&2
    echo "[er-sentinel] Reason: $branch -- $detail" >&2
    echo "[er-sentinel] Profiles: ${profiles:--}" >&2
    echo "[er-sentinel] Killed:" >&2
    echo "$live" | prefix_lines >&2
    echo "[er-sentinel] Logged to: $SENTINEL_LOG" >&2
  else
    log_event "TEARDOWN-FAILED" "$branch" "$rel" "$detail" "$profiles" "$killed"
    echo "[er-sentinel] FAILED to tear down the stale run after '$rel' was edited:" >&2
    list_live | prefix_lines >&2
    exit 1
  fi
  exit 0
}

# Hook mode: Claude Code passes the tool payload as JSON on stdin.
cmd_hook() {
  local payload path
  payload="$(cat 2>/dev/null || true)"
  [[ -n "$payload" ]] || exit 0
  path="$(printf '%s' "$payload" | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
ti = d.get("tool_input") or {}
for key in ("file_path", "notebook_path", "path"):
    v = ti.get(key)
    if isinstance(v, str) and v:
        print(v)
        break
' 2>/dev/null || true)"
  [[ -n "$path" ]] || exit 0
  cmd_check "$path"
}

# SC2317 ("command appears unreachable") fires for everything after the `cmd_hook` probe below.
# The analysis reasons that `cmd_hook` always `exit`s -- which it does -- and concludes the rest
# of this function is dead. It is not: `cmd_hook` is invoked on the right-hand side of a PIPELINE,
# so it runs in a subshell and its exit terminates only that subshell. The checker does not model
# that, so the finding is a false positive on every line here.
# shellcheck disable=SC2317
selftest() {
  local fails=0
  local tmpdir
  tmpdir="$(mktemp -d)"

  # ---- gate 1: is this a path the repo cares about at all -------------------------------------
  if is_repo_source "$REPO_ROOT/AGENTS.md"; then echo "  ok   tracked file recognised as repo source"; else
    echo "  FAIL tracked file not recognised"; fails=$((fails + 1)); fi
  # A BRAND NEW source file counts, even though git has never heard of it. This is the case that
  # actually escaped on 2026-08-04: the first file created after the sentinel was written was a
  # new `.rs`, `ls-files --error-unmatch` said no, and a live run survived an edit it should not
  # have. Creating source invalidates a loaded DLL exactly as much as editing source.
  local newsrc="$REPO_ROOT/crates/.er-sentinel-selftest-new-source.rs"
  : >"$newsrc"
  if is_repo_source "$newsrc"; then echo "  ok   brand-new untracked source file counts as repo source";
  else echo "  FAIL new source file not recognised (the 2026-08-04 escape)"; fails=$((fails + 1)); fi
  rm -f "$newsrc"
  # A gitignored path does NOT count -- this is what keeps a log line or a build artefact from
  # killing a run. `target/` is ignored in this repo.
  local ignored="$REPO_ROOT/target/.er-sentinel-selftest-ignored"
  mkdir -p "$REPO_ROOT/target" 2>/dev/null || true
  : >"$ignored"
  if is_repo_source "$ignored"; then echo "  FAIL gitignored path treated as repo source"; fails=$((fails + 1));
  else echo "  ok   gitignored/untracked path ignored"; fi
  rm -f "$ignored"
  # A path outside the repo is not.
  if is_repo_source "/etc/hostname"; then echo "  FAIL outside-repo path treated as repo source"; fails=$((fails + 1));
  else echo "  ok   outside-repo path ignored"; fi
  # An empty path is not.
  if is_repo_source ""; then echo "  FAIL empty path treated as repo source"; fails=$((fails + 1));
  else echo "  ok   empty path ignored"; fi

  # ---- gate 2: does the edit feed a DLL THIS run loaded ---------------------------------------
  # A synthetic profile stands in for a live run, so both directions are provable with no game
  # running and nothing killed. The natives need not exist on disk -- only their filenames are read.
  local prof="$tmpdir/selftest.me3"
  cat >"$prof" <<'TOML'
profileVersion = "v1"
[[supports]]
game = "eldenring"
[[natives]]
path = '/nonexistent/er_effects_rs.dll'
[[natives]]
path = '/nonexistent/er_invasion_warp_dll.dll'
[[natives]]
path = '/nonexistent/SeamlessCoop/ersc.dll'
TOML

  # `verdict <path> <expected> <label>` -- classification only; teardown is never called.
  local vt_out vt_verdict vt_branch vt_detail
  expect_verdict() {
    local vpath="$1" expected="$2" label="$3" vprof="${4:-$prof}"
    vt_out="$(classify_path "$vpath" "$vprof")"
    IFS=$'\t' read -r vt_verdict vt_branch vt_detail _ <<<"$vt_out"
    if [[ "$vt_verdict" == "$expected" ]]; then
      printf '  ok   %-58s [%s]\n' "$label" "$vt_branch"
    else
      printf '  FAIL %-58s got %s (%s: %s), want %s\n' "$label" "$vt_verdict" "$vt_branch" "$vt_detail" "$expected"
      fails=$((fails + 1))
    fi
  }

  # The logged reason is the whole point of the log, so it has to be TRUE, not merely present.
  expect_detail() {
    local vpath="$1" want="$2" label="$3"
    vt_out="$(classify_path "$vpath" "$prof")"
    IFS=$'\t' read -r vt_verdict vt_branch vt_detail _ <<<"$vt_out"
    if [[ "$vt_detail" == *"$want"* ]]; then
      printf '  ok   %-58s [%s]\n' "$label" "$vt_detail"
    else
      printf '  FAIL %-58s reason %q does not contain %q\n' "$label" "$vt_detail" "$want"
      fails=$((fails + 1))
    fi
  }

  echo "  -- must TEAR DOWN --"
  # The crate that directly builds a loaded DLL.
  expect_verdict "$REPO_ROOT/crates/er-invasion-warp-dll/src/lib.rs" TEARDOWN "crate builds a loaded DLL"
  expect_verdict "$REPO_ROOT/crates/er-effects-rs/src/lib.rs" TEARDOWN "product crate builds a loaded DLL"
  # A DIRECT dependency whose crate name does not resemble the DLL's. This is the case a
  # filename-shaped rule gets wrong: er-invasion-warp is not er_invasion_warp_dll.
  expect_verdict "$REPO_ROOT/crates/er-invasion-warp/src/lib.rs" TEARDOWN "direct dependency crate of a loaded DLL"
  # A TRANSITIVE dependency, several edges away from either loaded cdylib.
  expect_verdict "$REPO_ROOT/crates/er-game-base/src/lib.rs" TEARDOWN "transitive dependency crate of a loaded DLL"
  expect_verdict "$REPO_ROOT/crates/er-tpf/src/lib.rs" TEARDOWN "deep transitive dependency crate"
  # A crate manifest, not just its sources.
  expect_verdict "$REPO_ROOT/crates/er-gfx/Cargo.toml" TEARDOWN "manifest of a crate in the closure"
  # The logged reason must name the DLL the crate ACTUALLY reaches. er-invasion-warp is a
  # dependency of er_invasion_warp_dll ONLY -- er-effects-rs does not depend on it -- so a reason
  # that also blamed er_effects_rs.dll would send the next reader hunting the wrong DLL.
  expect_detail "$REPO_ROOT/crates/er-invasion-warp/src/lib.rs" \
    "pkg=er-invasion-warp feeds er_invasion_warp_dll.dll" "reason names only the DLL it reaches"
  expect_detail "$REPO_ROOT/crates/er-game-base/src/lib.rs" \
    "feeds er_effects_rs.dll,er_invasion_warp_dll.dll" "reason names BOTH DLLs a shared crate feeds"
  # Fail-safe: repo source that belongs to no crate and no inert directory.
  expect_verdict "$REPO_ROOT/Cargo.toml" TEARDOWN "workspace manifest (unclassified -> fail safe)"
  expect_verdict "$REPO_ROOT/data/effects.json" TEARDOWN "data/ (unclassified -> fail safe)"
  expect_verdict "$REPO_ROOT/third_party/x.c" TEARDOWN "third_party/ (unclassified -> fail safe)"
  # Fail-safe: the profile cannot be parsed.
  local badprof="$tmpdir/broken.me3"
  printf 'this is not = valid toml [[[\n' >"$badprof"
  expect_verdict "$REPO_ROOT/scripts/er-launch-gate.py" TEARDOWN "unparseable profile -> fail safe" "$badprof"
  # Fail-safe: the profile does not exist.
  expect_verdict "$REPO_ROOT/scripts/er-launch-gate.py" TEARDOWN "missing profile -> fail safe" "$tmpdir/absent.me3"
  # Fail-safe: the run loads nothing this workspace builds.
  local otherprof="$tmpdir/other.me3"
  cat >"$otherprof" <<'TOML'
profileVersion = "v1"
[[natives]]
path = '/nonexistent/SeamlessCoop/ersc.dll'
TOML
  expect_verdict "$REPO_ROOT/scripts/er-launch-gate.py" TEARDOWN "no workspace DLL loaded -> fail safe" "$otherprof"

  echo "  -- must NOT tear down --"
  # The five real false teardowns from 2026-08-04, each of which cost a user-driven invasion.
  expect_verdict "$REPO_ROOT/scripts/frida-trace-ersc.py" SKIP "scripts/frida-trace-ersc.py (host-side frida)"
  expect_verdict "$REPO_ROOT/scripts/frida-steam-matchmaking-trace.py" SKIP "scripts/frida-steam-matchmaking-trace.py"
  expect_verdict "$REPO_ROOT/scripts/er-launch-gate.py" SKIP "scripts/er-launch-gate.py (pre-launch gate)"
  expect_verdict "$REPO_ROOT/.cupcake/policies/claude/idle_hold.rego" SKIP ".cupcake policy (not code)"
  expect_verdict "$REPO_ROOT/.cupcake/tests/idle_hold_test.rego" SKIP ".cupcake policy test"
  expect_verdict "$REPO_ROOT/docs/plans/world-map-invasion-warp.md" SKIP "docs/"
  expect_verdict "$REPO_ROOT/.beads/issues.jsonl" SKIP ".beads/ issue data"
  expect_verdict "$REPO_ROOT/.github/workflows/ci.yml" SKIP ".github/ workflow file"
  expect_verdict "$REPO_ROOT/.claude/settings.json" SKIP ".claude/ settings"
  expect_verdict "$REPO_ROOT/tests/pi-continuation-guard.test.ts" SKIP "tests/ (builds no DLL)"
  # A crate that builds a DLL the live profile does NOT load. This is the discrimination the
  # whole change exists for: er_armament_icons.dll and er_input_harness_dll.dll are real, buildable,
  # me3-loadable DLLs -- they are simply not in THIS run.
  expect_verdict "$REPO_ROOT/crates/er-armament-icons/src/lib.rs" SKIP "crate builds an UNLOADED DLL"
  expect_verdict "$REPO_ROOT/crates/er-input-harness-dll/src/lib.rs" SKIP "input-harness crate not in this profile"
  expect_verdict "$REPO_ROOT/crates/er-telemetry-dll/src/lib.rs" SKIP "telemetry shell not in this profile"
  # Host-only tooling crates, reachable from no loaded cdylib.
  expect_verdict "$REPO_ROOT/tools/er-param-inspect/src/main.rs" SKIP "host-only tool crate"
  expect_verdict "$REPO_ROOT/crates/soulsformats/src/lib.rs" SKIP "host-only library crate"

  # ---- process matching ------------------------------------------------------------------------
  # Hook mode tolerates junk without erroring.
  if printf 'not json' | cmd_hook >/dev/null 2>&1; then echo "  ok   hook tolerates non-JSON stdin";
  else echo "  FAIL hook errored on non-JSON stdin"; fails=$((fails + 1)); fi
  # A name longer than the kernel's 15-char comm cap is still matched. Proven end to end against a
  # REAL process rather than by re-deriving the truncation in the test: a copy of /bin/sleep named
  # `start_protected_game.exe` gets comm `start_protected`, which the verbatim comparison this
  # replaced could never have matched. Detection only -- the process is killed by pid, never by
  # calling teardown, so this case cannot touch a live game.
  local longbin longpid
  longbin="$tmpdir/start_protected_game.exe"
  if cp -f /bin/sleep "$longbin" 2>/dev/null; then
    "$longbin" 5 &
    longpid=$!
    read -r -t 1 </dev/null 2>/dev/null || true
    local seen=0 live_pid
    while read -r live_pid _; do
      [[ "$live_pid" == "$longpid" ]] && seen=1
    done < <(list_live)
    if [[ $seen -eq 1 ]]; then
      echo "  ok   over-length executable name matched despite comm truncation"
    else
      echo "  FAIL over-length executable name not matched (comm truncation regression)"
      fails=$((fails + 1))
    fi
    kill -KILL "$longpid" 2>/dev/null || true
    wait "$longpid" 2>/dev/null || true
  else
    echo "  note comm-truncation case skipped (could not stage a test binary)"
  fi

  # Teardown with nothing running is a no-op success.
  if [[ -z "$(list_live)" ]] && teardown; then echo "  ok   teardown no-ops when nothing is live";
  else echo "  note teardown selftest skipped (a run is live)"; fi

  rm -rf "$tmpdir"
  if [[ $fails -eq 0 ]]; then echo "selftest ok"; return 0; fi
  echo "selftest FAILED ($fails)"; return 1
}

case "${1:-hook}" in
  check) shift; cmd_check "${1:-}" ;;
  classify) shift; classify_path "$@" ;;
  teardown) if teardown; then echo "[er-sentinel] teardown: verified clean"; else
      echo "[er-sentinel] teardown FAILED" >&2; exit 1; fi ;;
  status) live="$(list_live)"; if [[ -z "$live" ]]; then echo "[er-sentinel] no live run"; else
      echo "[er-sentinel] LIVE:"; echo "$live" | prefix_lines; exit 1; fi ;;
  --selftest) selftest ;;
  hook) cmd_hook ;;
  *) echo "usage: $0 {check <path>|classify <path> [profile...]|teardown|status|--selftest|hook}" >&2; exit 2 ;;
esac
