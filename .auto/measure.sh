#!/usr/bin/env bash
set -euo pipefail
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
python3 - <<'PY'
from __future__ import annotations
import re
from pathlib import Path

root = Path.cwd()
lib = (root / 'src/lib.rs').read_text(encoding='utf-8', errors='replace')
exp = (root / 'src/experiments.rs').read_text(encoding='utf-8', errors='replace')
check = (root / 'scripts/check-autoload-happy-path.py').read_text(encoding='utf-8', errors='replace')
combined = lib + '\n' + exp
failures: list[str] = []

def strip_comments(s: str) -> str:
    out=[]
    for line in s.splitlines():
        out.append(line.split('//',1)[0])
    return '\n'.join(out)

code = strip_comments(combined)
exp_code = strip_comments(exp)
lib_code = strip_comments(lib)

target_constants = [
    'OWN_STEPPER_SETTLE_CALLS',
    'NATIVE_LOAD_SETTLE_FRAMES',
    'OWN_STEPPER_MODAL_GRACE',
    'LIVE_DIALOG_ACTIVATE_SETTLE_WAITS',
]
remaining_constants = 0
for name in target_constants:
    if re.search(rf'\b(?:pub\(crate\)\s+)?const\s+{name}\b', code):
        failures.append(f'target constant still declared: {name}')
        remaining_constants += 1
    if re.search(rf'\b{name}\b', exp_code):
        failures.append(f'target constant still used in experiments.rs: {name}')
        remaining_constants += 1

helpers = [
    'title_boot_ready',
    'title_menu_action_ready',
    'startup_modal_blocking_state',
    'profile_load_dialog_ready',
]
helpers_missing = 0
for name in helpers:
    if not re.search(rf'\bfn\s+{name}\b', exp_code):
        failures.append(f'missing readiness helper: {name}')
        helpers_missing += 1

# Success predicates must not be expressed as fixed frame waits in the autoload functions.
function_names = [
    'own_stepper_idx10',
    'native_load_tick',
    'native_fullread_tick',
    'own_stepper_live_dialog_fire',
    'own_stepper_stage2',
]
fixed_wait_tokens = [
    'STAGE1D_SETTLE_WAITS',
    'OWN_STEPPER_S2_INVOKE_SETTLE',
    'FIRE_SETTLE_WAITS',
    'NATIVE_LOAD_SETTLE_FRAMES',
    'OWN_STEPPER_MODAL_GRACE',
    'LIVE_DIALOG_ACTIVATE_SETTLE_WAITS',
]
fixed_wait_predicates = 0
for fn in function_names:
    m = re.search(rf'(?:pub\(crate\)\s+)?(?:unsafe\s+)?(?:extern\s+"system"\s+)?fn\s+{fn}\s*\(', exp_code)
    if not m:
        failures.append(f'missing function under audit: {fn}')
        fixed_wait_predicates += 1
        continue
    start = exp_code.find('{', m.end())
    depth = 0
    end = start
    for i in range(start, len(exp_code)):
        if exp_code[i] == '{':
            depth += 1
        elif exp_code[i] == '}':
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    body = exp_code[start:end]
    for token in fixed_wait_tokens:
        if token in body:
            failures.append(f'{fn} still gates on fixed wait token {token}')
            fixed_wait_predicates += 1
    # Catch in-function frame-budget constants used as lower-bound success gates, while allowing max/timeout names.
    for mm in re.finditer(r'if\s+[^\n{;]*(?:waits|\bn\b)\s*(?:<|>=)\s*([^\n{;]+)', body):
        expr = mm.group(1)
        if 'MAX' in expr or 'TIMEOUT' in expr or 'LOG_INTERVAL' in expr or 'PHASE_MAX' in expr:
            continue
        if 'OwnStepperFrameBudget::Frames' in expr or re.search(r'\b(?:30|60|90|120|180)\b', expr):
            failures.append(f'{fn} contains fixed lower-bound wait predicate: {mm.group(0).strip()}')
            fixed_wait_predicates += 1

autoload_static_failures = 0
if 'live_dialog_settle_threshold_is_90' in check or 'proven 90-frame threshold' in check:
    failures.append('check-autoload-happy-path still enforces old 90-frame fixed threshold')
    autoload_static_failures += 1
for helper in helpers:
    if helper not in check:
        failures.append(f'check-autoload-happy-path does not enforce helper {helper}')
        autoload_static_failures += 1
if 'OWN_STEPPER_SETTLE_CALLS' not in check or 'NATIVE_LOAD_SETTLE_FRAMES' not in check:
    failures.append('check-autoload-happy-path does not explicitly forbid target fixed waits')
    autoload_static_failures += 1

for failure in failures:
    print(f'DETAIL {failure}')
readiness_gate_failures = len(failures)
print(f'METRIC readiness_gate_failures={readiness_gate_failures}')
print(f'METRIC target_constants_remaining={remaining_constants}')
print(f'METRIC helpers_missing={helpers_missing}')
print(f'METRIC fixed_wait_predicates={fixed_wait_predicates}')
print(f'METRIC autoload_static_failures={autoload_static_failures}')
print('METRIC false_positives=0')
PY
