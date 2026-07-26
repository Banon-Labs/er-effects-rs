#!/usr/bin/env python3
"""Adversarial test: does the er_launch_requires_session_auth policy catch launches
that ride a NON-`command` tool_input field on guarded non-Bash tools
(run_experiment / ctx_execute / ctx_batch_execute)?

Runs each synthetic PreToolUse event through `opa eval` and prints the deny count.
Read-only static evaluation; NEVER launches anything."""
import json, subprocess

POLICY = "/home/choza/projects/er-effects-rs/.cupcake/policies/claude/er_launch_requires_session_auth.rego"
QUERY = "count(data.cupcake.policies.claude.er_launch_requires_session_auth.deny)"

def run(event):
    p = subprocess.run(
        ["opa", "eval", "-d", POLICY, "-I", "--format", "raw", QUERY],
        input=json.dumps(event), capture_output=True, text=True, timeout=30,
    )
    out = (p.stdout or "").strip()
    if p.returncode != 0 and not out:
        return f"ERR rc={p.returncode} {p.stderr.strip()[:200]}"
    return out

def ev(tool_name, tool_input, auth="0"):
    return {
        "hook_event_name": "PreToolUse",
        "tool_name": tool_name,
        "tool_input": tool_input,
        "signals": {"er_launch_authorized": auth},
    }

LAUNCHER_SCRIPT = "bash scripts/run-vanilla-reload-fps.sh"
ME3_LAUNCH = "me3 launch -g eldenring"
ELDENRING_EXE = "./eldenring.exe"
RENDERDOC = "renderdoccmd capture ./eldenring.exe"
HUB_SCRIPT = "bash scripts/run-product-continue-direct-probe.sh"
COND_SCRIPT = "python3 scripts/install_mushroom_man.py --launch"

cases = [
    ("CTRL Bash + command=launcher", ev("Bash", {"command": LAUNCHER_SCRIPT})),
    ("CTRL run_experiment + command=launcher", ev("run_experiment", {"command": LAUNCHER_SCRIPT})),
    ("CTRL ctx_execute + command=me3launch", ev("ctx_execute", {"command": ME3_LAUNCH})),
    ("CTRL ctx_batch_execute + command=me3launch", ev("ctx_batch_execute", {"command": ME3_LAUNCH})),

    ("run_experiment + code=launcher-script", ev("run_experiment", {"code": LAUNCHER_SCRIPT})),
    ("run_experiment + code=me3-launch", ev("run_experiment", {"code": ME3_LAUNCH})),
    ("run_experiment + code=eldenring.exe", ev("run_experiment", {"code": ELDENRING_EXE})),
    ("run_experiment + code=renderdoccmd", ev("run_experiment", {"code": RENDERDOC})),
    ("run_experiment + code=hub-script", ev("run_experiment", {"code": HUB_SCRIPT})),
    ("run_experiment + code=conditional --launch", ev("run_experiment", {"code": COND_SCRIPT})),
    ("ctx_execute + code=launcher-script", ev("ctx_execute", {"code": LAUNCHER_SCRIPT})),
    ("ctx_execute + code=me3-launch", ev("ctx_execute", {"code": ME3_LAUNCH})),
    ("ctx_batch_execute + code=launcher-script", ev("ctx_batch_execute", {"code": LAUNCHER_SCRIPT})),
    ("ctx_batch_execute + code=me3-launch", ev("ctx_batch_execute", {"code": ME3_LAUNCH})),

    ("run_experiment + script=launcher", ev("run_experiment", {"script": LAUNCHER_SCRIPT})),
    ("run_experiment + cmd=launcher", ev("run_experiment", {"cmd": LAUNCHER_SCRIPT})),
    ("run_experiment + commands[]=launcher", ev("run_experiment", {"commands": [LAUNCHER_SCRIPT]})),
    ("ctx_batch_execute + commands[]=launcher", ev("ctx_batch_execute", {"commands": [LAUNCHER_SCRIPT, ME3_LAUNCH]})),
    ("ctx_execute + input=me3launch", ev("ctx_execute", {"input": ME3_LAUNCH})),

    ("run_experiment + command=echo hi + code=launcher", ev("run_experiment", {"command": "echo hi", "code": LAUNCHER_SCRIPT})),

    ("AUTHED Bash + command=launcher (auth=1)", ev("Bash", {"command": LAUNCHER_SCRIPT}, auth="1")),
]

for label, event in cases:
    print(f"{run(event):>4}  {label}")
