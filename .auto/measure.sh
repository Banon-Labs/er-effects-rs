#!/usr/bin/env bash
set -euo pipefail

python3 - <<'PY'
from __future__ import annotations

import json
import os
import re
import subprocess
from pathlib import Path

root = Path.cwd()
bd = Path(os.environ.get("BD_REAL_BIN", str(Path.home() / ".local/bin/bd")))
roadmap = (root / "docs/plans/crate-extraction-execution-roadmap.md").read_text(encoding="utf-8")

# Parse only authoritative work-package cells/bullets, not incidental cross-references in prose.
# Plus-suffixed umbrellas remain nodes until R0 expands them into child labels such as R12B1.
package_specs: set[str] = set()
for line in roadmap.splitlines():
    match = re.match(r"^\|\s*([RD]\d+[a-z]?(?:-[RD]?\d+[a-z]?)?\+?)\s*\|", line, re.I)
    if match is None:
        match = re.match(r"^- \*\*([RD]\d+[a-z]?(?:-[RD]?\d+[a-z]?)?\+?):\*\*", line, re.I)
    if match is not None:
        package_specs.add(match.group(1).upper())

base_ids: set[str] = set()
umbrella_ids: set[str] = set()
for spec in package_specs:
    plus = spec.endswith("+")
    spec = spec.removesuffix("+")
    range_match = re.fullmatch(r"([RD])(\d+)([A-Z]?)-(?:[RD])?(\d+)([A-Z]?)", spec)
    if range_match:
        prefix, start_num_raw, start_suffix, end_num_raw, end_suffix = range_match.groups()
        start_num, end_num = int(start_num_raw), int(end_num_raw)
        if start_num == end_num and start_suffix and end_suffix:
            for code in range(ord(start_suffix), ord(end_suffix) + 1):
                base_ids.add(f"{prefix}{start_num}{chr(code)}")
        elif not start_suffix and not end_suffix:
            for number in range(start_num, end_num + 1):
                base_ids.add(f"{prefix}{number}")
        continue
    base_ids.add(spec)
    if plus:
        umbrella_ids.add(spec)

# R0 scouts expanded these broad roadmap rows into exact child PR nodes even though the original
# table did not carry a `+` suffix.
umbrella_ids.update({"R0A", "R0B", "R22"})

issues = json.loads(subprocess.check_output(
    [str(bd), "list", "--all", "-n", "0", "--json"], text=True
))
roadmap_issues = [issue for issue in issues if "pr193-roadmap" in (issue.get("labels") or [])]

plan_to_issue: dict[str, list[dict]] = {}
for issue in roadmap_issues:
    for label in issue.get("labels") or []:
        match = re.fullmatch(r"roadmap-([rd]\d+[a-z0-9]*)", label, re.I)
        if match:
            plan_id = match.group(1).upper()
            plan_to_issue.setdefault(plan_id, []).append(issue)

expanded_umbrellas = {
    umbrella for umbrella in umbrella_ids
    if any(plan_id.startswith(umbrella) and plan_id != umbrella for plan_id in plan_to_issue)
}
planned_ids = (base_ids - expanded_umbrellas) | set(plan_to_issue)

prs = json.loads(subprocess.check_output([
    "gh", "pr", "list", "--state", "all", "--limit", "1000",
    "--json", "number,state,isDraft,url,headRefName,baseRefName",
], text=True))
pr_by_number = {int(pr["number"]): pr for pr in prs}

translated: dict[str, int] = {}
false_positive_details: list[str] = []
pr_to_plans: dict[int, list[str]] = {}
open_prs = merged_prs = draft_prs = 0

for plan_id, mapped_issues in sorted(plan_to_issue.items()):
    if plan_id not in planned_ids:
        false_positive_details.append(f"unknown plan label {plan_id}")
    if len(mapped_issues) != 1:
        false_positive_details.append(f"plan {plan_id} maps to {len(mapped_issues)} Beads issues")
        continue
    issue = mapped_issues[0]
    external_ref = issue.get("external_ref") or ""
    match = re.search(r"(?:github\.com/Banon-Labs/er-effects-rs/(?:pull|issues)/|repo:Banon-Labs/er-effects-rs#|github:Banon-Labs/er-effects-rs#)(\d+)", external_ref)
    if not match:
        continue
    number = int(match.group(1))
    pr = pr_by_number.get(number)
    if pr is None:
        false_positive_details.append(f"plan {plan_id} references missing PR #{number}")
        continue
    if pr["state"] == "CLOSED":
        false_positive_details.append(f"plan {plan_id} references closed-unmerged PR #{number}")
        continue
    translated[plan_id] = number
    pr_to_plans.setdefault(number, []).append(plan_id)
    if pr["state"] == "MERGED":
        merged_prs += 1
    else:
        open_prs += 1
    if pr.get("isDraft"):
        draft_prs += 1

for number, plan_ids in sorted(pr_to_plans.items()):
    if len(plan_ids) > 1:
        false_positive_details.append(f"PR #{number} maps to multiple plans: {','.join(plan_ids)}")

ready = json.loads(subprocess.check_output([str(bd), "ready", "-n", "0", "--json"], text=True))
ready_ids = {issue["id"] for issue in ready}
ready_untranslated = sum(
    1 for plan_id, mapped in plan_to_issue.items()
    if len(mapped) == 1 and mapped[0]["id"] in ready_ids and plan_id not in translated
)

for detail in false_positive_details:
    print(f"DETAIL false_positive {detail}")

planned_total = len(planned_ids)
translated_total = len(translated)
completion_pct = (100.0 * translated_total / planned_total) if planned_total else 0.0
print(f"DETAIL roadmap_ids={','.join(sorted(planned_ids))}")
print(f"DETAIL translated={','.join(f'{key}:#{translated[key]}' for key in sorted(translated))}")
print(f"METRIC plans_translated_to_prs={translated_total}")
print(f"METRIC planned_nodes_total={planned_total}")
print(f"METRIC completion_pct={completion_pct:.6f}")
print(f"METRIC open_prs={open_prs}")
print(f"METRIC merged_prs={merged_prs}")
print(f"METRIC draft_prs={draft_prs}")
print(f"METRIC ready_untranslated_plans={ready_untranslated}")
print(f"METRIC false_positives={len(false_positive_details)}")
PY
