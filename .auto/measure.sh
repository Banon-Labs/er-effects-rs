#!/usr/bin/env bash
set -euo pipefail
python3 - <<'PY'
from __future__ import annotations
import json
import re
from pathlib import Path

MAX_SCORE = 1400
root = Path.cwd()
lib = (root / 'src/lib.rs').read_text(encoding='utf-8', errors='replace')
exp = (root / 'src/experiments.rs').read_text(encoding='utf-8', errors='replace')
check = (root / 'scripts/check-autoload-happy-path.py').read_text(encoding='utf-8', errors='replace')
prompt = (root / '.auto/prompt.md').read_text(encoding='utf-8', errors='replace') if (root / '.auto/prompt.md').exists() else ''
combined = lib + '\n' + exp


def strip_comments(s: str) -> str:
    out=[]
    for line in s.splitlines():
        out.append(line.split('//',1)[0])
    return '\n'.join(out)


def function_body(name: str, source: str) -> str | None:
    m = re.search(rf'(?:pub\(crate\)\s+)?(?:unsafe\s+)?(?:extern\s+"system"\s+)?fn\s+{name}\s*\(', source)
    if not m:
        return None
    start = source.find('{', m.end())
    if start == -1:
        return None
    depth = 0
    for i in range(start, len(source)):
        if source[i] == '{':
            depth += 1
        elif source[i] == '}':
            depth -= 1
            if depth == 0:
                return source[start:i + 1]
    return None


def doc_text() -> str:
    chunks=[prompt]
    for path in [root / 'docs/file-extraction-tooling.md']:
        if path.exists():
            chunks.append(path.read_text(encoding='utf-8', errors='replace'))
    for path in sorted((root / 'docs').glob('**/*')):
        if path.is_file() and path.suffix.lower() in {'.md', '.txt'} and 'recon' in path.parts:
            try:
                chunks.append(path.read_text(encoding='utf-8', errors='replace')[:200_000])
            except OSError:
                pass
    return '\n'.join(chunks)


code = strip_comments(combined)
exp_code = strip_comments(exp)
lib_code = strip_comments(lib)
legacy_failures: list[str] = []

# Legacy semantic-readiness regression checks from the previous prompt.
target_constants = [
    'OWN_STEPPER_SETTLE_CALLS',
    'NATIVE_LOAD_SETTLE_FRAMES',
    'OWN_STEPPER_MODAL_GRACE',
    'LIVE_DIALOG_ACTIVATE_SETTLE_WAITS',
]
forbidden_budget_tokens = [
    'OwnStepperFrameBudget',
]
remaining_constants = 0
for name in target_constants:
    if re.search(rf'\b(?:pub\(crate\)\s+)?const\s+{name}\b', code):
        legacy_failures.append(f'target constant still declared: {name}')
        remaining_constants += 1
    if re.search(rf'\b{name}\b', exp_code):
        legacy_failures.append(f'target constant still used in experiments.rs: {name}')
        remaining_constants += 1
for token in forbidden_budget_tokens:
    if token in code:
        legacy_failures.append(f'forbidden frame-budget token still present: {token}')
        remaining_constants += 1

helpers = [
    'product_core_autoload_ready',
    'product_continue_action_ready',
    'title_boot_ready',
    'title_menu_action_ready',
    'title_live_dialog_fire_ready',
    'startup_modal_blocking_state',
    'profile_load_dialog_ready',
]
helpers_missing = 0
autoload_static_failures = 0
for name in helpers:
    if not re.search(rf'\bfn\s+{name}\b', exp_code):
        legacy_failures.append(f'missing readiness helper: {name}')
        helpers_missing += 1

if 'product_core_autoload_tick' not in lib_code:
    legacy_failures.append('product autoload no longer routes through product_core_autoload_tick')
    autoload_static_failures += 1
else:
    product_core_pos = lib_code.find('product_core_autoload_tick')
    for later in ['own_stepper_patch_once', 'title_accept_tick']:
        later_pos = lib_code.find(later)
        if later_pos != -1 and product_core_pos > later_pos:
            legacy_failures.append(f'product_core_autoload_tick appears after legacy path {later}')
            autoload_static_failures += 1

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
    body = function_body(fn, exp_code)
    if body is None:
        legacy_failures.append(f'missing function under audit: {fn}')
        fixed_wait_predicates += 1
        continue
    for token in fixed_wait_tokens:
        if token in body:
            legacy_failures.append(f'{fn} still gates on fixed wait token {token}')
            fixed_wait_predicates += 1
    for mm in re.finditer(r'if\s+[^\n{;]*(?:waits|\bn\b)\s*(?:<|>=)\s*([^\n{;]+)', body):
        expr = mm.group(1)
        if 'MAX' in expr or 'TIMEOUT' in expr or 'LOG_INTERVAL' in expr or 'PHASE_MAX' in expr:
            continue
        if 'OwnStepperFrameBudget::Frames' in expr or re.search(r'\b(?:30|60|90|120|180)\b', expr):
            legacy_failures.append(f'{fn} contains fixed lower-bound wait predicate: {mm.group(0).strip()}')
            fixed_wait_predicates += 1

product_body = function_body('product_core_autoload_tick', exp_code)
product_continue_body = function_body('product_continue_autoload_tick', exp_code) or ''
submit_body = function_body('submit_native_continue_item_action', exp_code) or ''
continue_item_body = function_body('product_continue_item_action', exp_code) or ''
product_related = '\n'.join([product_body or '', product_continue_body, submit_body, continue_item_body])
if product_body is None:
    legacy_failures.append('missing product_core_autoload_tick under native-path audit')
    autoload_static_failures += 1
else:
    if 'own_stepper_direct_build' in product_body:
        legacy_failures.append('product_core_autoload_tick still calls broken direct_build path')
        autoload_static_failures += 1
    if 'product_continue_autoload_tick' not in product_body or 'product_continue_action_ready' not in product_body:
        legacy_failures.append('product_core_autoload_tick no longer routes through product Continue action readiness')
        autoload_static_failures += 1

if 'live_dialog_settle_threshold_is_90' in check or 'proven 90-frame threshold' in check:
    legacy_failures.append('check-autoload-happy-path still enforces old 90-frame fixed threshold')
    autoload_static_failures += 1
for helper in helpers:
    if helper not in check:
        legacy_failures.append(f'check-autoload-happy-path does not enforce helper {helper}')
        autoload_static_failures += 1
if 'OWN_STEPPER_SETTLE_CALLS' not in check or 'NATIVE_LOAD_SETTLE_FRAMES' not in check:
    legacy_failures.append('check-autoload-happy-path does not explicitly forbid target fixed waits')
    autoload_static_failures += 1

asset_failures: list[str] = []
docs = doc_text()
asset_requirements = {
    'data archive source': [r'Data\*\.bhd/bdt', r'Data\*\.bhd', r'Data0\.bdt', r'Data\*\.bdt'],
    'menu message bundle path': [r'msg/engus/menu\.msgbnd\.dcx'],
    'FMG/resource identity': [r'\bFMG\b', r'text ID', r'resource ID'],
    'extraction tooling': [r'Nuxe', r'WitchyBND'],
    'not regulation.bin': [r'not `?regulation\.bin`?', r'not regulation\.bin'],
}
for label, patterns in asset_requirements.items():
    if not any(re.search(pattern, docs, re.IGNORECASE) for pattern in patterns):
        asset_failures.append(f'asset chain missing {label}')

native_failures: list[str] = []
for name in ['product_continue_item_action', 'submit_native_continue_item_action']:
    if not re.search(rf'\bfn\s+{name}\b', exp_code):
        native_failures.append(f'missing native Continue helper {name}')
for token in [
    'MENU_WINDOW_JOB_VTABLE_RVA',
    'MENU_TITLE_CONTINUE_DOCALL_RVA',
    'MENU_ITEM_DIALOG_RESULT_130_OFFSET',
    'MENU_ITEM_SUBMIT_RVA',
    'MENU_ITEM_RESULT_EVENT_SLOT_60_OFFSET',
    'FD4_EVENT_CONSTRUCTOR_RVA',
]:
    if token not in product_related:
        native_failures.append(f'native Continue path missing token {token}')
if continue_item_body and 'MENU_TITLE_CONTINUE_DOCALL_RVA' not in continue_item_body:
    native_failures.append('Continue item action does not validate Continue docall')
if submit_body and 'MENU_ITEM_SUBMIT_RVA' not in submit_body and 'MENU_ITEM_RESULT_EVENT_SLOT_60_OFFSET' not in submit_body:
    native_failures.append('submit helper does not use native submit/event dispatch')

field58_failures: list[str] = []
if 'MENU_ITEM_RESULT_MODE_58_OFFSET' in submit_body and re.search(r'if\s+mode\s*==', submit_body):
    field58_failures.append('submit helper still gates on result+0x58/mode')
if re.search(r'native Continue MenuWindowJob result rejected[^\n]*mode=', exp):
    field58_failures.append('product logs still reject native Continue solely as mode-gated result')

direct_failures: list[str] = []
direct_tokens = [
    'CONTINUE_LOAD_RVA',
    'CONTINUE_CONFIRM_RVA',
    'B80_DESERIALIZE_RVA',
    'drive_product_continue_post_click_dispatchers',
    'menu_continue_wrapper(',
    'b80_deserialize_67b290(',
]
for token in direct_tokens:
    if token in product_related:
        direct_failures.append(f'product/native submit body contains direct shortcut token {token}')

input_failures: list[str] = []
for token in ['input_probe_enabled', 'inject_nav_enabled', 'menu_input_probe', 'set_injected_key', 'SAFE_INPUT_CONFIRM', 'DIK_DOWN', 'XInput']:
    if token in product_related:
        input_failures.append(f'product/native submit body contains input path token {token}')
if re.search(r'Down \+ accept.*product proof', prompt, re.IGNORECASE):
    input_failures.append('prompt still frames Down+accept as product proof')

dll_failures: list[str] = []
if 'er_effects_rs.dll' not in prompt and 'chainload DLL' not in prompt and 'DLL' not in prompt:
    dll_failures.append('prompt does not make DLL product vehicle explicit')
for token in ['eldenring.exe patch', 'patch eldenring.exe', 'loose asset edits as product']:
    if token in prompt.lower() and 'do not' not in prompt.lower():
        dll_failures.append(f'prompt may allow forbidden product vehicle: {token}')
if not product_continue_body:
    dll_failures.append('missing product_continue_autoload_tick implementation')

runtime_failures: list[str] = []
required_runtime = ['ready', 'product_submit', 'continue_load', 'deserialize', 'confirm', 'world', 'zero_input']
best_runtime: tuple[int, Path | None, dict[str, bool]] = (0, None, {key: False for key in required_runtime})
rt_root = root / 'target/runtime-probe'
if rt_root.exists():
    candidates = sorted((p for p in rt_root.glob('product-core-*') if p.is_dir()), key=lambda p: p.stat().st_mtime, reverse=True)[:30]
    for d in candidates:
        proof = {key: False for key in required_runtime}
        for name in ['readiness-result.json', 'max-oracle-result.json', 'telemetry.json']:
            p = d / name
            if not p.exists():
                continue
            try:
                data = json.loads(p.read_text(encoding='utf-8', errors='replace'))
            except Exception:
                continue
            if data.get('ready') is True or data.get('success') is True:
                proof['ready'] = True
            raw = json.dumps(data)
            if 'simulated_button_presses_total' in raw and re.search(r'"simulated_button_presses_total"\s*:\s*0', raw):
                proof['zero_input'] = True
            if re.search(r'world[-_ ]?stable|max oracle|SetState5', raw, re.IGNORECASE):
                proof['world'] = True
        for name in ['autoload-debug-live.final.log', 'continue-trace-game.final.log', 'continue-trace-game.log']:
            p = d / name
            if not p.exists():
                continue
            text = p.read_text(encoding='utf-8', errors='replace')[-200_000:]
            if 'SUBMITTED native Continue' in text or 'native Continue submit' in text:
                proof['product_submit'] = True
            if 'continue_load_67b750' in text:
                proof['continue_load'] = True
            if 'b80_deserialize_67b290' in text or 'CAP b80_deserialize' in text:
                proof['deserialize'] = True
            if 'CAP continue_confirm' in text or 'continue_confirm' in text:
                proof['confirm'] = True
            if 'simulated_button_presses_total=0' in text or 'simulated_button_presses_total": 0' in text:
                proof['zero_input'] = True
            if 'world-stable' in text or 'SetState5' in text or 'max oracle' in text:
                proof['world'] = True
        count = sum(proof.values())
        if count > best_runtime[0]:
            best_runtime = (count, d, proof)
best_count, best_dir, proof = best_runtime
if best_dir is None:
    runtime_failures.append('runtime proof missing product-core artifact directory')
else:
    missing = [key for key in required_runtime if not proof[key]]
    if missing:
        runtime_failures.append(f'runtime proof best artifact {best_dir.name} missing {",".join(missing)}')

false_positives = 0
all_detail_failures = []
for group in [legacy_failures, asset_failures, dll_failures, native_failures, field58_failures, direct_failures, input_failures, runtime_failures]:
    all_detail_failures.extend(group)

weights = {
    'readiness': 15,
    'asset': 35,
    'dll': 45,
    'native': 45,
    'field58': 100,
    'direct': 85,
    'input': 85,
    'runtime': 80,
    'false_positive': 100,
}
penalty = (
    len(legacy_failures) * weights['readiness']
    + len(asset_failures) * weights['asset']
    + len(dll_failures) * weights['dll']
    + len(native_failures) * weights['native']
    + len(field58_failures) * weights['field58']
    + len(direct_failures) * weights['direct']
    + len(input_failures) * weights['input']
    + len(runtime_failures) * weights['runtime']
    + false_positives * weights['false_positive']
)
score = max(0, MAX_SCORE - penalty)

for failure in all_detail_failures:
    print(f'DETAIL {failure}')
print(f'DETAIL autoload_re_score_penalty={penalty}')
print(f'METRIC autoload_re_score={score}')
print(f'METRIC readiness_gate_failures={len(legacy_failures)}')
print(f'METRIC target_constants_remaining={remaining_constants}')
print(f'METRIC helpers_missing={helpers_missing}')
print(f'METRIC fixed_wait_predicates={fixed_wait_predicates}')
print(f'METRIC autoload_static_failures={autoload_static_failures}')
print(f'METRIC asset_chain_failures={len(asset_failures)}')
print(f'METRIC dll_patch_failures={len(dll_failures)}')
print(f'METRIC native_continue_failures={len(native_failures)}')
print(f'METRIC field58_gate_failures={len(field58_failures)}')
print(f'METRIC direct_shortcut_failures={len(direct_failures)}')
print(f'METRIC input_path_failures={len(input_failures)}')
print(f'METRIC runtime_proof_failures={len(runtime_failures)}')
print(f'METRIC false_positives={false_positives}')
PY
