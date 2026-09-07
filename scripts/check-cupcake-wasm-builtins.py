#!/usr/bin/env python3
"""Every Rego builtin our policies use must survive Cupcake's WASM runtime.

Why this gate exists (2026-08-22). Every Stop-event guard in this repo was inert for 36 days and
nothing noticed, because a policy that cannot execute does not fail -- it returns nothing, and
"nothing" is indistinguishable from "clean turn".

The mechanism: `cupcake eval` does not run policies in the OPA interpreter. It shells out to
`opa build -t wasm` and executes the compiled module in its own embedded WASM runtime. OPA's WASM
target implements many builtins inside the module, but the rest are compiled as *host-dispatched*
calls -- the module emits the builtin's name into a dispatch table and calls back out, and the
embedding host must supply an implementation. `sprintf` is host-dispatched, and cupcake 0.5.2 does
not implement it (the string `sprintf` does not appear anywhere in the cupcake binary). A missing
host builtin does not raise: the call yields undefined, undefined propagates up through the rule
body, the decision set comes back empty, and cupcake logs "Synthesized allow decision (no policies
triggered)" with exit code 0.

That is why the failure sorted itself by event type and looked like a routing bug. It never was:
    * PreToolUse guards build their reason strings with `concat` or plain literals -> they worked.
    * every Stop guard interpolated a captured signal phrase with `sprintf`  -> all silently dead.
The `walk()` aggregation in .cupcake/system/evaluate.rego was never at fault and is proven fine.

What this gate does. It refuses to take any builtin on trust:
  1. it extracts every builtin actually called by .cupcake/policies/** and .cupcake/system/**
     (comments and string literals stripped, so prose mentioning a name is not a call);
  2. every such builtin must have a probe recipe below. A builtin with no recipe is a hard failure,
     not a pass -- that is the anti-rot property: a new policy reaching for an unverified builtin
     breaks this gate until someone adds a probe and the probe passes;
  3. it then generates one throwaway halt policy per builtin, aggregates them through the repo's own
     evaluate.rego, and runs the real `cupcake eval` binary. A builtin whose halt does not come back
     is one the WASM runtime cannot execute, and the gate names it.

Step 3 is the load-bearing half. A hand-maintained blocklist of known-bad builtins would have to be
correct in advance about a runtime nobody here controls; this executes the runtime and asks it.

Run with --selftest to prove the detector still catches the original defect.
"""
from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CUPCAKE_DIR = REPO_ROOT / ".cupcake"
# .cupcake/tests holds Rego unit tests, which run in the OPA interpreter and never reach WASM.
SCAN_DIRS = [CUPCAKE_DIR / "policies", CUPCAKE_DIR / "system"]

# A Rego expression per builtin, written so the builtin is genuinely called and its result reaches
# the emitted decision (a probe whose builtin is optimised away would prove nothing).
# `str:` recipes produce the halt's reason string. `cond:` recipes are boolean conditions guarding it.
PROBES: dict[str, str] = {
    "array.concat": 'str:concat("", array.concat(["a"], ["b"]))',
    "array.slice": 'str:concat("", array.slice(["a", "b", "c"], 1, 3))',
    "concat": 'str:concat("", ["a", "b"])',
    "contains": 'cond:contains("abc", "b")',
    "count": "str:format_int(count([1, 2, 3]), 10)",
    "endswith": 'cond:endswith("abc", "c")',
    "format_int": "str:format_int(255, 16)",
    "indexof": 'str:format_int(indexof("abc", "c"), 10)',
    "is_object": "cond:is_object({})",
    "is_string": 'cond:is_string("a")',
    "json.marshal": 'str:json.marshal({"a": [1, "b"]})',
    "lower": 'str:lower("AB")',
    "object.get": 'str:object.get({"k": "v"}, "k", "d")',
    "regex.find_all_string_submatch_n": 'str:concat("", regex.find_all_string_submatch_n("a(b)", "ab", 1)[0])',
    "regex.find_n": 'str:concat("", regex.find_n("a", "aa", 2))',
    "regex.match": 'cond:regex.match("^a.c$", "abc")',
    "replace": 'str:replace("aXa", "X", "-")',
    "split": 'str:concat("|", split("a,b", ","))',
    "startswith": 'cond:startswith("abc", "a")',
    "substring": 'str:substring("abcdef", 1, 3)',
    "trim": 'str:trim("xxaxx", "x")',
    "trim_left": 'str:trim_left("xxa", "x")',
    "trim_prefix": 'str:trim_prefix("Pa", "P")',
    "trim_right": 'str:trim_right("axx", "x")',
    "trim_space": 'str:trim_space("  a  ")',
    "walk": 'str:concat("", [v | walk({"o": {"k": "v"}}, [_, v]); is_string(v)])',
    # Present only so --selftest can assert the detector still catches the original defect.
    # No policy may use it; if one does, step 3 fails and names it.
    "sprintf": 'str:sprintf("%s", ["a"])',
}

# Strip block/line comments and double-quoted or backtick-quoted strings, so a builtin name appearing
# in prose or inside a reason string is never mistaken for a call.
_STRIP = re.compile(
    r'"(?:[^"\\\n]|\\.)*"'  # double-quoted string
    r"|`[^`]*`"  # raw string
    r"|#.*$",  # comment to end of line
    re.MULTILINE,
)


def opa_builtin_names() -> set[str]:
    """Authoritative builtin list from the installed OPA, not a hand-copied table."""
    out = subprocess.run(
        ["opa", "capabilities", "--current"], capture_output=True, text=True, timeout=25
    )
    if out.returncode != 0:
        raise SystemExit("check-cupcake-wasm-builtins: `opa capabilities --current` failed")
    return {b["name"] for b in json.loads(out.stdout).get("builtins", [])}


def builtins_used(names: set[str]) -> dict[str, set[str]]:
    """Map builtin -> set of files calling it. Longest-first so `json.marshal` beats `json`."""
    ordered = sorted(names, key=len, reverse=True)
    pat = re.compile(
        r"(?<![A-Za-z0-9_.])(" + "|".join(re.escape(n) for n in ordered) + r")\s*\("
    )
    used: dict[str, set[str]] = {}
    for d in SCAN_DIRS:
        for f in sorted(d.rglob("*.rego")):
            code = _STRIP.sub(" ", f.read_text(encoding="utf-8"))
            for m in pat.finditer(code):
                used.setdefault(m.group(1), set()).add(str(f.relative_to(REPO_ROOT)))
    return used


def probe_policy(name: str, recipe: str) -> tuple[str, str]:
    """Build a throwaway Stop policy that halts iff `name` executes. Returns (package_slug, source)."""
    slug = re.sub(r"[^a-z0-9]", "_", name.lower())
    rule_id = f"WASMPROBE-{slug.upper()}"
    kind, _, expr = recipe.partition(":")
    # The rule_id is folded into the reason because cupcake renders a lone decision as a bare reason
    # string with no [rule_id] prefix; without this a single surviving probe would look like a miss.
    body = (
        f'\t{expr}\n\treason := "{rule_id}|ok"'
        if kind == "cond"
        else f'\treason := concat("", ["{rule_id}|", {expr}])'
    )
    src = f"""# METADATA
# scope: package
# custom:
#   routing:
#     required_events: ["Stop"]
package cupcake.policies.claude.wasmprobe_{slug}

import rego.v1

halt contains decision if {{
\tinput.hook_event_name == "Stop"
{body}
\tdecision := {{"rule_id": "{rule_id}", "reason": reason, "severity": "HIGH"}}
}}
"""
    return rule_id, src


def run_probes(targets: dict[str, str]) -> set[str]:
    """Run every probe through the real cupcake WASM runtime. Returns the set of rule_ids that fired."""
    with tempfile.TemporaryDirectory(prefix="cupcake-wasm-probe-") as tmp:
        root = Path(tmp) / ".cupcake"
        (root / "policies" / "claude").mkdir(parents=True)
        (root / "system").mkdir(parents=True)
        # Isolation, and it has to be an empty directory. This probe measures one thing -- does
        # the WASM runtime execute this builtin -- so the only policies in the evaluation must be
        # the throwaway ones generated below. Left to itself cupcake discovers a global config at
        # ${XDG_CONFIG_HOME:-$HOME/.config}/cupcake, which on a developer machine carries live
        # custom policies; one of them denying could put a second `rule_id|` line in stdout and
        # make a probe's presence or absence depend on the user's personal setup.
        #
        # This used to pass a file (`root / "rulebook.yml"`) and it did isolate the probe -- but
        # only by accident. cupcake 0.5.2 rejects any --global-config that is not a directory,
        # logs "Global config path must be a directory" at DEBUG, and continues project-only
        # without falling back to discovery; the isolation was a side effect of a rejection, and
        # would evaporate the day a bad path became fatal or started falling back. An empty
        # directory is instead accepted and loaded -- measured against 0.5.2: "Global
        # configuration discovered at ..." / "Global configuration initialization complete" --
        # and contributes no policy, so the isolation is now what the argument says it is.
        global_root = Path(tmp) / "empty-global-config"
        global_root.mkdir()
        # The repo's own aggregator, so the probe exercises the real evaluation path.
        shutil.copy(CUPCAKE_DIR / "system" / "evaluate.rego", root / "system" / "evaluate.rego")
        (root / "rulebook.yml").write_text("signals: {}\nbuiltins: {}\n", encoding="utf-8")

        expected: set[str] = set()
        for name, recipe in sorted(targets.items()):
            rule_id, src = probe_policy(name, recipe)
            slug = re.sub(r"[^a-z0-9]", "_", name.lower())
            (root / "policies" / "claude" / f"wasmprobe_{slug}.rego").write_text(src, encoding="utf-8")
            expected.add(rule_id)

        event = json.dumps(
            {
                "session_id": "wasm-builtin-probe",
                "transcript_path": "/dev/null",
                "cwd": tmp,
                "hook_event_name": "Stop",
                "stop_hook_active": False,
            }
        )
        out = subprocess.run(
            [
                "cupcake", "eval",
                "--harness", "claude",
                "--policy-dir", str(root),
                "--global-config", str(global_root),
            ],
            input=event, capture_output=True, text=True, timeout=25,
        )
        return {rule_id for rule_id in expected if f"{rule_id}|" in out.stdout}


def check() -> int:
    names = opa_builtin_names()
    used = builtins_used(names)

    missing_recipe = sorted(set(used) - set(PROBES))
    if missing_recipe:
        print("check-cupcake-wasm-builtins: FAIL -- builtin used with no WASM probe recipe", file=sys.stderr)
        for n in missing_recipe:
            print(f"  {n}  used by: {', '.join(sorted(used[n]))}", file=sys.stderr)
        print(
            "\nA builtin nobody has verified against Cupcake's WASM runtime is exactly how every Stop\n"
            "guard went inert for 36 days: an unsupported builtin returns UNDEFINED, the rule silently\n"
            "never fires, and cupcake reports a clean ALLOW. Add a probe recipe to PROBES in\n"
            "scripts/check-cupcake-wasm-builtins.py and re-run -- if the probe does not come back, that\n"
            "builtin cannot be used in a policy.",
            file=sys.stderr,
        )
        return 1

    # `sprintf` is stocked only as the selftest's specimen; a policy using it is the original defect.
    targets = {n: PROBES[n] for n in used}
    fired = run_probes(targets)
    dead = sorted(
        rid_name
        for rid_name in used
        if f"WASMPROBE-{re.sub(r'[^a-z0-9]', '_', rid_name.lower()).upper()}" not in fired
    )
    if dead:
        print("check-cupcake-wasm-builtins: FAIL -- builtin does not execute in Cupcake's WASM runtime", file=sys.stderr)
        for n in dead:
            print(f"  {n}  used by: {', '.join(sorted(used[n]))}", file=sys.stderr)
        print(
            "\nThese calls return UNDEFINED at runtime, so every rule whose body reaches one silently\n"
            "never fires and cupcake reports ALLOW with exit code 0. Replace them with a builtin the\n"
            "runtime implements (string building: `concat`; any-value rendering: `json.marshal`;\n"
            "numbers: `format_int`).",
            file=sys.stderr,
        )
        return 1

    print(f"check-cupcake-wasm-builtins: OK ({len(used)} builtins verified against the live WASM runtime)")
    return 0


def selftest() -> int:
    """Prove the detector still catches the original defect, rather than trusting its own say-so."""
    fired = run_probes({"sprintf": PROBES["sprintf"], "concat": PROBES["concat"]})
    if "WASMPROBE-CONCAT" not in fired:
        print("selftest FAIL: the control probe (concat) did not fire -- the harness itself is broken", file=sys.stderr)
        return 1
    if "WASMPROBE-SPRINTF" in fired:
        print(
            "selftest FAIL: sprintf executed. Either Cupcake gained a sprintf host builtin (good --\n"
            "re-verify and update this file's rationale), or the probe is not exercising WASM.",
            file=sys.stderr,
        )
        return 1
    # The scanner must also see through prose/strings: a name in a comment is not a call.
    decoy = _STRIP.sub(" ", '# sprintf(x)\nmsg := concat("", ["use sprintf(y) instead"])\n')
    if "sprintf" in decoy:
        print("selftest FAIL: comment/string stripping missed a decoy `sprintf` mention", file=sys.stderr)
        return 1
    print("check-cupcake-wasm-builtins: selftest OK (concat executes, sprintf silently does not)")
    return 0


def main() -> int:
    for tool in ("cupcake", "opa"):
        if shutil.which(tool) is None:
            print(f"check-cupcake-wasm-builtins: SKIP ({tool} not installed)")
            return 0
    if "--selftest" in sys.argv:
        return selftest()
    return check()


if __name__ == "__main__":
    raise SystemExit(main())
