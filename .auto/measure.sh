#!/usr/bin/env bash
set -euo pipefail

export RUNTIME_ONSCREEN="${RUNTIME_ONSCREEN:-1}"

if [[ -f .auto/run_profile_portrait_capture_once ]]; then
  rm -f .auto/run_profile_portrait_capture_once
  cargo xwin build --release --target x86_64-pc-windows-msvc >/tmp/er-effects-runtime-build.log 2>&1 || {
    tail -80 /tmp/er-effects-runtime-build.log >&2
    exit 1
  }
  profile_capture_dir="target/runtime-probe/profile-portrait-capture-$(date +%Y%m%d-%H%M%S)"
  set +e
  PROFILE_CAPTURE_SKIP_BUILD=1 \
  ARTIFACT_DIR="$profile_capture_dir" \
  ER_EFFECTS_GOLD_SAVE="${ER_EFFECTS_GOLD_SAVE:-/home/banon/projects/er-effects-rs/save-files/150-Banon/ER0000.sl2}" \
  ER_EFFECTS_GOLD_SLOT="${ER_EFFECTS_GOLD_SLOT:-0}" \
  RUNTIME_TIMEOUT_SECONDS="${RUNTIME_TIMEOUT_SECONDS:-45}" \
  scripts/run-profile-portrait-capture-probe.sh
  profile_capture_rc=$?
  set -e
  if (( profile_capture_rc != 0 )) && [[ ! -f "$profile_capture_dir/profile-portrait-failure.json" ]]; then
    exit "$profile_capture_rc"
  fi
fi

if [[ -f .auto/run_runtime_probe_once ]]; then
  rm -f .auto/run_runtime_probe_once
  runtime_enable_hudhook="${ER_EFFECTS_ENABLE_HUDHOOK:-0}"
  if [[ -f .auto/enable_hudhook_runtime_once ]]; then
    rm -f .auto/enable_hudhook_runtime_once
    runtime_enable_hudhook=1
  fi
  cargo xwin build --release --target x86_64-pc-windows-msvc >/tmp/er-effects-runtime-build.log 2>&1 || {
    tail -80 /tmp/er-effects-runtime-build.log >&2
    exit 1
  }
  runtime_autoload_request=$(mktemp "${TMPDIR:-/tmp}/er-effects-autoload.XXXXXX")
  runtime_expected_save_oracle=$(mktemp "${TMPDIR:-/tmp}/er-effects-save-oracle.XXXXXX.json")
  python3 scripts/save-slot-oracle.py \
    --save "${ER_EFFECTS_GOLD_SAVE:-/home/banon/projects/er-effects-rs/save-files/150-Banon/ER0000.sl2}" \
    --slot "${ER_EFFECTS_GOLD_SLOT:-0}" \
    --output "$runtime_expected_save_oracle"
  printf 'slot=%s\nmethod=direct_menu_load\nrequire_title_bootstrap=false\n' "${ER_EFFECTS_GOLD_SLOT:-0}" > "$runtime_autoload_request"
  ER_EFFECTS_AUTHORIZED_DIRECT_RUNTIME=1 \
  AUTO_ALLOW_MANUAL_RUNTIME_PROBE=1 \
  ER_EFFECTS_EXPERIMENTAL_DIRECT_MENU_LOAD=1 \
  ER_EFFECTS_ENABLE_HUDHOOK="$runtime_enable_hudhook" \
  ER_EFFECTS_GOLD_SAVE="${ER_EFFECTS_GOLD_SAVE:-/home/banon/projects/er-effects-rs/save-files/150-Banon/ER0000.sl2}" \
  ER_EFFECTS_GOLD_SLOT="${ER_EFFECTS_GOLD_SLOT:-0}" \
  RUNTIME_TIMEOUT_SECONDS="${RUNTIME_TIMEOUT_SECONDS:-35}" \
  RUNTIME_EXPECTED_MODE="${RUNTIME_EXPECTED_MODE:-vanilla}" \
  RUNTIME_EXPECTED_SAVE_ORACLE="$runtime_expected_save_oracle" \
  scripts/run-product-continue-direct-probe.sh --autoload-request "$runtime_autoload_request"
  rm -f "$runtime_autoload_request" "$runtime_expected_save_oracle"
fi

if [[ -f .auto/run_manual_profile_trace_once ]]; then
  rm -f .auto/run_manual_profile_trace_once
  ER_EFFECTS_AUTHORIZED_DIRECT_RUNTIME=1 \
  AUTO_ALLOW_MANUAL_RUNTIME_PROBE=1 \
  ER_EFFECTS_GOLD_SAVE="${ER_EFFECTS_GOLD_SAVE:-/home/banon/projects/er-effects-rs/save-files/150-Banon/ER0000.sl2}" \
  ER_EFFECTS_GOLD_SLOT="${ER_EFFECTS_GOLD_SLOT:-0}" \
  RUNTIME_ONSCREEN=1 \
  RUNTIME_TIMEOUT_SECONDS="${RUNTIME_TIMEOUT_SECONDS:-45}" \
  RUNTIME_EXPECTED_MODE="${RUNTIME_EXPECTED_MODE:-any}" \
  ER_EFFECTS_NO_AUTOLOAD=1 \
  ER_EFFECTS_TRACE_CONTINUE=1 \
  scripts/run-product-continue-direct-probe.sh
fi

python3 - <<'PY'
from __future__ import annotations
import json
import os
import re
import subprocess
import tempfile
from pathlib import Path

MAX_SCORE = 1600
root = Path.cwd()
lib = (root / 'src/lib.rs').read_text(encoding='utf-8', errors='replace')
constants_src = (root / 'src/constants.rs').read_text(encoding='utf-8', errors='replace')
_exp_dir = root / 'src/experiments'
if _exp_dir.is_dir():
    _exp_files = sorted(_exp_dir.glob('*.rs'), key=lambda p: (p.name != 'mod.rs', p.name))
    exp = '\n'.join(p.read_text(encoding='utf-8', errors='replace') for p in _exp_files)
else:
    exp = (root / 'src/experiments.rs').read_text(encoding='utf-8', errors='replace')
check = (root / 'scripts/check-autoload-happy-path.py').read_text(encoding='utf-8', errors='replace')
telemetry_src = (root / 'src/telemetry.rs').read_text(encoding='utf-8', errors='replace')
overlay_code = (root / 'src/overlay.rs').read_text(encoding='utf-8', errors='replace') if (root / 'src/overlay.rs').exists() else ''
watcher = (root / 'scripts/er-readiness-watch.py').read_text(encoding='utf-8', errors='replace')
native_static_check = (root / 'scripts/check-native-continue-static.py').read_text(encoding='utf-8', errors='replace') if (root / 'scripts/check-native-continue-static.py').exists() else ''
menu_ctor_static_check = (root / 'scripts/check-menu-constructor-static.py').read_text(encoding='utf-8', errors='replace') if (root / 'scripts/check-menu-constructor-static.py').exists() else ''
launch_guard_check = (root / 'scripts/check-launch-guardrails.py').read_text(encoding='utf-8', errors='replace') if (root / 'scripts/check-launch-guardrails.py').exists() else ''
direct_probe = (root / 'scripts/run-product-continue-direct-probe.sh').read_text(encoding='utf-8', errors='replace') if (root / 'scripts/run-product-continue-direct-probe.sh').exists() else ''
runtime_probe = (root / '.auto/runtime_probe.sh').read_text(encoding='utf-8', errors='replace') if (root / '.auto/runtime_probe.sh').exists() else ''
check_sh = (root / 'scripts/check.sh').read_text(encoding='utf-8', errors='replace')
prompt = (root / '.auto/prompt.md').read_text(encoding='utf-8', errors='replace') if (root / '.auto/prompt.md').exists() else ''
combined = lib + '\n' + constants_src + '\n' + exp


def empty_name_like(value) -> bool:
    if not isinstance(value, str):
        return True
    stripped = value.strip()
    return stripped == '' or stripped == '_'


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
    for later in ['title_accept_tick']:
        later_pos = lib_code.find(later)
        if later_pos != -1 and product_core_pos > later_pos:
            legacy_failures.append(f'product_core_autoload_tick appears after legacy path {later}')
            autoload_static_failures += 1
if (
    'BOOTSTRAP_EVENT_GAME_TASK_WAITING_INSTANCE' not in lib_code
    or 'TASK_INSTANCE_WAIT_LOG_INTERVAL' not in lib_code
    or 'attempts={wait_attempts}' not in lib_code
):
    legacy_failures.append('game task startup no longer reports bounded CSTaskImp wait progress')
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
native_fullread_body = function_body('native_fullread_tick', exp_code) or ''
product_continue_body = function_body('product_continue_autoload_tick', exp_code) or ''
submit_body = function_body('submit_native_continue_item_action', exp_code) or ''
continue_item_body = function_body('product_continue_item_action', exp_code) or ''
product_related = '\n'.join([product_body or '', product_continue_body, submit_body, continue_item_body])
product_input_audit = product_related
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
if (
    'product autoload suppressed MessageBoxDialog builder before UI allocation but counted it as oracle failure' not in exp
    or 'MSGBOX_TOTAL_BUILDS.fetch_add' not in exp
    or 'MSGBOX_LAST_ARG_RDX.store' not in exp
):
    message = 'product-mode MessageBoxDialog suppression can hide popup builder attempts from telemetry'
    legacy_failures.append(message)
    autoload_static_failures += 1
if 'product-mode MessageBoxDialog suppression must preserve/count builder args' not in check:
    legacy_failures.append('check-autoload-happy-path does not enforce counted product-mode MessageBoxDialog suppression')
    autoload_static_failures += 1
if (
    'MENU_ITEM_ACCEPT_IDLE_RVA' not in continue_item_body
    or 'MENU_ITEM_ACCEPT_NATIVE_RVA' not in continue_item_body
    or 'constant false idle predicate' not in continue_item_body
    or 'promoted candidate native Continue MenuWindowJob' not in continue_item_body
):
    legacy_failures.append('product Continue submit must reject idle rows and promote only native-accept Continue candidates')
    autoload_static_failures += 1
if 'product Continue item validation must reject the constant-false idle accept predicate before native submit' not in check:
    legacy_failures.append('check-autoload-happy-path does not enforce idle-reject/native-accept submit guard')
    autoload_static_failures += 1
menu_update_body = function_body('cap_menu_item_update_hook', exp_code) or ''
if (
    'captured semantic native Continue item' not in menu_update_body
    or 'semantic_continue_item' not in menu_update_body
    or 'captured first title item as native Continue' in menu_update_body
):
    legacy_failures.append('product Continue capture can latch the first ticked MenuWindowJob instead of a semantic Continue item')
    autoload_static_failures += 1
if 'product Continue capture must latch a semantic Continue item, not the first ticked MenuWindowJob' not in check:
    legacy_failures.append('check-autoload-happy-path does not enforce semantic Continue item capture')
    autoload_static_failures += 1
ctor_body = function_body('menu_window_job_ctor_hook', exp_code) or ''
if (
    'MENU-WINDOW-CTOR captured semantic native Continue item' not in ctor_body
    or 'MENU_WINDOW_JOB_CTOR_RVA' not in code
    or 'cap_menu_window_job_ctor_7ac8c0' not in exp_code
):
    legacy_failures.append('product Continue capture lacks constructor hook for semantic item latching')
    autoload_static_failures += 1
if 'product Continue capture must observe MenuWindowJob construction before update-time first-item latching' not in check:
    legacy_failures.append('check-autoload-happy-path does not enforce constructor hook semantic Continue latch')
    autoload_static_failures += 1
native_ctor_b_body = function_body('menu_window_job_native_ctor_b_hook', exp_code) or ''
if (
    'MENU_WINDOW_JOB_NATIVE_CTOR_B_RVA' not in code
    or 'cap_menu_window_job_native_ctor_b_7acb00' not in exp_code
    or 'MENU-WINDOW-NATIVE-CTOR-B captured semantic native Continue item' not in native_ctor_b_body
    or 'MENU_ITEM_ACCEPT_NATIVE_RVA' not in native_ctor_b_body
    or 'MENU_CONTINUE_ITEM' not in native_ctor_b_body
    or '0x007ad810' not in native_ctor_b_body
    or '0x007add70' in native_ctor_b_body
):
    legacy_failures.append('product diagnostics lack native-accept MenuWindowJob constructor B hook or can accept idle rows')
    autoload_static_failures += 1
if 'product diagnostics must hook native-accept MenuWindowJob constructor B without accepting idle rows' not in check:
    legacy_failures.append('check-autoload-happy-path does not enforce native constructor B telemetry')
    autoload_static_failures += 1
idle_ctor_body = function_body('menu_window_job_idle_ctor_hook', exp_code) or ''
if (
    'MENU_WINDOW_JOB_IDLE_CTOR_RVA' not in code
    or 'MENU_ITEM_ACCEPT_IDLE_RVA' not in exp_code
    or 'cap_menu_window_job_idle_ctor_7acf80' not in exp_code
    or 'MENU-WINDOW-IDLE-CTOR observed Continue-looking disabled item' not in idle_ctor_body
    or 'record_continue_candidate' not in idle_ctor_body
    or 'trace_first_game_caller_rva' not in idle_ctor_body
    or re.search(r'MENU_CONTINUE_ITEM\s*\.\s*(store|swap|compare_exchange|fetch)', idle_ctor_body)
):
    legacy_failures.append('product diagnostics lack passive idle MenuWindowJob constructor provenance or can promote disabled rows')
    autoload_static_failures += 1
if 'product diagnostics must passively attribute disabled Continue rows to the 0x1407acf80 idle constructor without promoting them' not in check:
    legacy_failures.append('check-autoload-happy-path does not enforce idle MenuWindowJob constructor provenance')
    autoload_static_failures += 1
title_ready_body = function_body('title_native_ready_predicate_hook', exp_code) or ''
if (
    'TITLE_NATIVE_READY_PREDICATE_RVA' not in code
    or 'TITLE_NATIVE_READY_PREDICATE_ORIG' not in code
    or 'cap_title_native_ready_733150' not in exp_code
    or 'STATE_FLAGS_20_OFFSET' not in title_ready_body
    or 'READY_MASK_8F' not in title_ready_body
    or 'TITLE_NATIVE_READY_PREDICATE_LAST_OBJECT' not in title_ready_body
    or 'TITLE_NATIVE_READY_PREDICATE_LAST_MASKED' not in title_ready_body
    or 'oracle_title_native_ready_last_masked' not in telemetry_src
    or 'oracle_title_langselect_ready_last_masked' not in telemetry_src
):
    legacy_failures.append('product diagnostics lack corrected LangSelect title-ready predicate flags telemetry')
    autoload_static_failures += 1
if 'product diagnostics must passively expose LangSelect title-ready predicate flags without treating them as Continue readiness' not in check:
    legacy_failures.append('check-autoload-happy-path does not enforce corrected LangSelect title-ready predicate telemetry')
    autoload_static_failures += 1
member_latch_body = function_body('capture_continue_member_node_candidate', exp_code) or ''
if (
    'MENU_CONTINUE_MEMBER_NODE' not in code
    or 'TRACE_MENU_CONTINUE_WRAPPER_RVA' not in member_latch_body
    or 'MEMBERFUNCJOB_VTABLE_RVA' not in member_latch_body
    or 'MEMBER_FN_18' not in member_latch_body
    or 'MEMBER_ADJ_20' not in member_latch_body
    or 'capture_continue_member_node_candidate(base, arg1' not in exp_code
    or 'capture_continue_member_node_candidate(base, result' not in exp_code
    or 'oracle_continue_member_node' not in telemetry_src
    or 'oracle_continue_task_node' not in telemetry_src
):
    legacy_failures.append('product tracing lacks passive registered TitleTopDialog Continue MenuMemberFuncJob latch/telemetry')
    autoload_static_failures += 1
if 'product tracing must passively latch registered TitleTopDialog Continue MenuMemberFuncJob nodes' not in check:
    legacy_failures.append('check-autoload-happy-path does not enforce passive Continue MenuMemberFuncJob latch')
    autoload_static_failures += 1
if 'telemetry must expose passive Continue task/member semantic latch addresses' not in check:
    legacy_failures.append('check-autoload-happy-path does not enforce passive Continue latch telemetry')
    autoload_static_failures += 1
result_event_body = function_body('result_event_handler_hook', exp_code) or ''
result_action_body = function_body('result_action_builder_hook', exp_code) or ''
if (
    'RESULT_EVENT_HANDLER_RVA' not in code
    or 'RESULT_ACTION_BUILDER_RVA' not in code
    or 'RESULT_EVENT_HANDLER_ORIG' not in code
    or 'RESULT_ACTION_BUILDER_ORIG' not in code
    or 'result_event_handler_746e80' not in exp_code
    or 'result_action_builder_746a00' not in exp_code
    or 'call_result_void2_original' not in exp_code
    or 'continue_load' in result_event_body.lower()
    or 'continue_load' in result_action_body.lower()
    or 'oracle_native_submit_hits' not in telemetry_src
    or 'oracle_result_event_handler_hits' not in telemetry_src
    or 'oracle_result_action_builder_hits' not in telemetry_src
    or 'oracle_result_event_last_raw_qword0' not in telemetry_src
    or 'oracle_result_event_last_fd4_code' not in telemetry_src
    or 'oracle_result_event_last_fd4_arg' not in telemetry_src
    or 'oracle_result_action_last_word0' not in telemetry_src
    or 'oracle_result_action_last_word1' not in telemetry_src
    or 'oracle_result_action_wrapper_builder_hits' not in telemetry_src
    or 'oracle_result_action_last_wrapper_builder_ret' not in telemetry_src
    or 'oracle_result_action_last_wrapper_builder_ret_update_rva' not in telemetry_src
    or 'oracle_policy_window_backing_flag_ptr' not in telemetry_src
    or 'oracle_policy_window_stored_backing_flag_ptr' not in telemetry_src
    or 'oracle_policy_window_backing_flag_value' not in telemetry_src
    or 'oracle_policy_window_requested_flag_value' not in telemetry_src
    or 'oracle_policy_window_caller_rva' not in telemetry_src
    or 'write_policy_oracle_snapshot' not in telemetry_src
    or 'policy_oracle_snapshot' not in telemetry_src
    or 'telemetry_snapshot_reason' not in telemetry_src
    or 'oracle_policy_ctor_wrapper_hits' not in telemetry_src
    or 'oracle_policy_ctor_wrapper_original_this' not in telemetry_src
    or 'oracle_policy_ctor_wrapper_original_vtable' not in telemetry_src
    or 'oracle_policy_ctor_wrapper_backing_flag_ptr' not in telemetry_src
    or 'oracle_policy_ctor_wrapper_caller_rva' not in telemetry_src
    or 'oracle_policy_selector_wrapper_hits' not in telemetry_src
    or 'oracle_policy_selector_wrapper_requested_flag' not in telemetry_src
    or 'oracle_policy_selector_wrapper_selector_arg' not in telemetry_src
    or 'oracle_policy_selector_wrapper_caller_rva' not in telemetry_src
    or 'oracle_policy_selector_ctor_hits' not in telemetry_src
    or 'oracle_policy_selector_ctor_requested_flag_ptr' not in telemetry_src
    or 'oracle_policy_selector_ctor_stored_requested_flag_ptr' not in telemetry_src
    or 'oracle_policy_selector_ctor_caller_rva' not in telemetry_src
    or 'oracle_policy_status_predicate_hits' not in telemetry_src
    or 'oracle_policy_status_predicate_ret' not in telemetry_src
    or 'oracle_policy_status_predicate_caller_rva' not in telemetry_src
    or 'oracle_policy_flag_setter_hits' not in telemetry_src
    or 'oracle_policy_flag_setter_after' not in telemetry_src
    or 'oracle_policy_flag_setter_caller_rva' not in telemetry_src
    or 'oracle_result_action_insert_hits' not in telemetry_src
    or 'oracle_result_action_last_insert_arg1_update_rva' not in telemetry_src
    or 'oracle_result_action_last_insert_ret_update_rva' not in telemetry_src
    or 'RESULT_ACTION_WRAPPER_BUILDER_HITS' not in code
    or 'RESULT_ACTION_LAST_WRAPPER_BUILDER_RET_UPDATE_RVA' not in code
    or 'POLICY_TOS_TITLE_LAST_BACKING_FLAG_PTR' not in code
    or 'POLICY_TOS_TITLE_LAST_STORED_BACKING_FLAG_PTR' not in code
    or 'POLICY_TOS_TITLE_LAST_BACKING_FLAG_VALUE' not in code
    or 'POLICY_TOS_TITLE_LAST_REQUESTED_FLAG_VALUE' not in code
    or 'POLICY_TOS_TITLE_LAST_CALLER_RVA' not in code
    or 'POLICY_TOS_TITLE_CTOR_WRAPPER_RVA' not in code
    or 'POLICY_TOS_TITLE_CTOR_WRAPPER_ORIG' not in code
    or 'POLICY_TOS_TITLE_WRAPPER_HITS' not in code
    or 'POLICY_TOS_TITLE_WRAPPER_THIS_ADJUST' not in code
    or 'POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_THIS' not in code
    or 'POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_VTABLE' not in code
    or 'POLICY_TOS_TITLE_WRAPPER_LAST_CALLER_RVA' not in code
    or 'POLICY_TOS_SELECTOR_WRAPPER_RVA' not in code
    or 'POLICY_TOS_SELECTOR_WRAPPER_HITS' not in code
    or 'POLICY_TOS_SELECTOR_WRAPPER_LAST_REQUESTED_FLAG' not in code
    or 'POLICY_TOS_SELECTOR_WRAPPER_LAST_SELECTOR_ARG' not in code
    or 'POLICY_TOS_SELECTOR_WRAPPER_LAST_CALLER_RVA' not in code
    or 'POLICY_TOS_SELECTOR_CTOR_RVA' not in code
    or 'POLICY_TOS_SELECTOR_CTOR_HITS' not in code
    or 'POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_PTR' not in code
    or 'POLICY_TOS_SELECTOR_CTOR_LAST_STORED_REQUESTED_FLAG_PTR' not in code
    or 'POLICY_TOS_SELECTOR_CTOR_LAST_CALLER_RVA' not in code
    or 'POLICY_TOS_STATUS_PREDICATE_RVA' not in code
    or 'POLICY_TOS_STATUS_PREDICATE_ORIG' not in code
    or 'POLICY_TOS_STATUS_LAST_CALLER_RVA' not in code
    or 'POLICY_TOS_FLAG_SETTER_RVA' not in code
    or 'POLICY_TOS_FLAG_SETTER_ORIG' not in code
    or 'POLICY_TOS_FLAG_SETTER_LAST_CALLER_RVA' not in code
    or 'RESULT_ACTION_INSERT_HITS' not in code
    or 'RESULT_ACTION_LAST_INSERT_ARG1_UPDATE_RVA' not in code
    or 'text_section_bounds' not in exp_code
    or 'update_target_in_text' not in exp_code
    or 'raw_task_node_update_rva' not in exp_code
    or 'shared_pointee' not in exp_code
    or 'PE_TEXT_SECTION_NAME' not in exp_code
    or 'policy_tos_title_ctor_wrapper_hook' not in exp_code
    or 'write_policy_oracle_snapshot("tos_title_ctor")' not in exp_code
    or 'policy_tos_record_fields' not in exp_code
    or 'let caller_rva = trace_first_game_caller_rva();' not in exp_code
    or 'trace_first_game_caller_rva' not in exp_code
    or 'backing_flag_ptr' not in exp_code
    or 'stack_arg0' not in exp_code
    or 'callstack_contains_game_rva' not in exp_code
    or 'native_submit_entered' not in watcher
    or 'native_result_chain_same_result' not in watcher
    or 'native_submit_fd4_event_match' not in watcher
    or 'native_result_chain_ready' not in watcher
    or 'native_continue_chain_stage' not in watcher
    or 'result_action_wrapper_built' not in watcher
    or 'result_action_wrapper_has_update_rva' not in watcher
    or 'result_action_inserted' not in watcher
    or 'result_action_insert_has_update_rva' not in watcher
    or 'result_chain_waiting_wrapper_builder' not in watcher
    or 'wrapper_builder_without_update_rva' not in watcher
    or 'wrapper_builder_waiting_action_insert' not in watcher
    or 'action_insert_without_update_rva' not in watcher
    or 'action_insert_waiting_continue_load' not in watcher
    or 'oracle_continue_phase' not in telemetry_src
    or 'oracle_continue_expected_slot' not in telemetry_src
    or 'oracle_continue_mount_c30' not in telemetry_src
    or 'oracle_continue_guard_waits' not in telemetry_src
):
    legacy_failures.append('product tracing lacks passive result.vtable+0x60/action-builder telemetry hooks or Continue phase telemetry')
    autoload_static_failures += 1
if 'product tracing must passively hook native submit, result.vtable+0x60, action builder, and wrapper builder without direct load shortcuts' not in check:
    legacy_failures.append('check-autoload-happy-path does not enforce passive submit/result-chain/action-builder/wrapper-builder hooks')
    autoload_static_failures += 1
if 'telemetry/watcher oracle must expose passive native submit/result-handler/action-builder/wrapper-builder/action-insert hit counts, wrapper/update-RVA proof, same-result proof, and chain stage' not in check:
    legacy_failures.append('check-autoload-happy-path does not enforce passive submit/result-chain/wrapper-builder/action-insert wrapper/update-RVA same-result telemetry/stage')
    autoload_static_failures += 1
if 'native static checker must pin wrapper-builder ABI, ToS wrapper vtable/thunk/RTTI provenance, selector requested-flag ABI, status predicate/setter/caller/requested-flag ABI, and inner finalize edge' not in check:
    legacy_failures.append('check-autoload-happy-path does not enforce native wrapper-builder/ToS vtable+RTTI+selector+predicate+setter+caller/requested-flag ABI static check')
    autoload_static_failures += 1
if 'telemetry must expose native Continue product phase/guard state for result-chain interpretation' not in check:
    legacy_failures.append('check-autoload-happy-path does not enforce Continue phase telemetry')
    autoload_static_failures += 1
if (
    'MENU_CONTINUE_WRAPPER' not in native_static_check
    or 'MENU_WINDOW_JOB_CTOR' not in native_static_check
    or 'MENU_ACCEPT_IDLE' not in native_static_check
    or 'MENU_ACCEPT_NATIVE' not in native_static_check
    or 'MENU_SUBMIT' not in native_static_check
    or 'MENU_MEMBER_FUNC_JOB_RUN' not in native_static_check
    or 'MENU_REGISTRY_INSERT_COPY' not in native_static_check
    or 'RESULT_EVENT_HANDLER' not in native_static_check
    or 'RESULT_EVENT_WRAPPER_BUILDER' not in native_static_check
    or 'POLICY_TOS_STATUS_PREDICATE' not in native_static_check
    or 'POLICY_TOS_FLAG_SETTER' not in native_static_check
    or 'POLICY_TOS_TITLE_CTOR_WRAPPER' not in native_static_check
    or 'POLICY_TOS_TITLE_CTOR_WRAPPER_VTABLE_SLOT' not in native_static_check
    or 'POLICY_TOS_TITLE_CTOR_WRAPPER_RTTI_COL' not in native_static_check
    or 'POLICY_TOS_SELECTOR_RTTI_COL' not in native_static_check
    or 'POLICY_TOS_SELECTOR_WRAPPER' not in native_static_check
    or 'POLICY_TOS_SELECTOR_CTOR' not in native_static_check
    or 'POLICY_TOS_SELECTOR_WRAPPER_VTABLE_SLOT' not in native_static_check
    or 'POLICY_TOS_TITLE_CTOR_CALLER' not in native_static_check
    or 'POLICY_TOS_FLAG_SETTER_CALLER' not in native_static_check
    or 'POLICY_TOS_REQUESTED_FLAG_INIT' not in native_static_check
    or 'POLICY_TOS_REQUESTED_FLAG_BIND' not in native_static_check
    or 'POLICY_TOS_REQUESTED_FLAG_COMMIT' not in native_static_check
    or 'policy ToS status predicate' not in native_static_check
    or 'policy ToS flag setter' not in native_static_check
    or 'owner+0x29c0' not in native_static_check
    or 'owner+0x29c8' not in native_static_check
    or 'requested-flag binder' not in native_static_check
    or 'requested-flag commit' not in native_static_check
    or '0x1409b7380' not in native_static_check
    or '0x1409b7390' not in native_static_check
    or '+0x8' not in native_static_check
    or 'CommandSelectDialog' not in native_static_check
    or 'SceneProxy' not in native_static_check
    or 'owner+0x29d0 selector argument' not in native_static_check
    or '0x1409b49f0' not in native_static_check
    or 'object+0x1260' not in native_static_check
    or 'object+0x1268' not in native_static_check
    or 'requested flag value' not in native_static_check
    or 'MENU_JOB_LIST_CONSUMER' not in native_static_check
    or 'MENU_JOB_SINGLE_CONSUMER' not in native_static_check
    or 'FD4 event code 3' not in native_static_check
    or 'FD4 event code 2' not in native_static_check
    or 'downstream action node' not in native_static_check
    or 'constructed FD4 event pointer' not in native_static_check
    or 'event+0x0' not in native_static_check
    or 'event+0x4' not in native_static_check
    or 'node+0x18' not in native_static_check
    or 'node+0x20' not in native_static_check
    or 'node+0x10' not in native_static_check
    or 'result+0x3b0' not in native_static_check
    or 'vtable +0x10 update' not in native_static_check
    or 'update return payload' not in native_static_check
    or 'DISABLED_CONTINUE_CALL' not in menu_ctor_static_check
    or 'DISABLED_CONTINUE_ENQUEUE_CALL' not in menu_ctor_static_check
    or 'NATIVE_CTOR_A_TITLE_CALL' not in menu_ctor_static_check
    or 'NATIVE_TITLE_READY_CALL' not in menu_ctor_static_check
    or 'NATIVE_TITLE_READY_SKIP_JE' not in menu_ctor_static_check
    or 'NATIVE_TITLE_REGISTER_CALL' not in menu_ctor_static_check
    or 'NATIVE_ACCEPT_PREDICATE_LEA' not in menu_ctor_static_check
    or 'IDLE_ACCEPT_PREDICATE_LEA' not in menu_ctor_static_check
    or 'LANG_SELECT_LABEL' not in menu_ctor_static_check
    or 'LANG_SELECT_COMPONENT_CTOR_CALL' not in menu_ctor_static_check
    or 'LANG_SELECT_READY_VTABLE' not in menu_ctor_static_check
    or 'LANG_SELECT_GETTER_BYTES' not in menu_ctor_static_check
    or 'CONTINUE_DOCALL_TABLE_SLOT' not in menu_ctor_static_check
    or 'find_rel32_callers' not in menu_ctor_static_check
    or 'rip_lea_target' not in menu_ctor_static_check
    or 'check-native-continue-static.py' not in check_sh
    or 'check-menu-constructor-static.py' not in check_sh
):
    legacy_failures.append('quality gates do not include native Continue/MenuWindowJob/MenuMemberFuncJob/result-consumer/static constructor provenance validation')
    autoload_static_failures += 1
if 'quality gates must include skip-safe native Continue/MenuWindowJob/MenuMemberFuncJob/result-consumer static byte-window validation' not in check:
    legacy_failures.append('check-autoload-happy-path does not enforce native Continue/MenuMemberFuncJob/result-consumer static checker wiring')
    autoload_static_failures += 1
if (
    'TitleTopDialog::open_menu writes latch and does not require Loop/TextFadeout state' not in exp
    or 'product open-menu gate must allow validated title dialog + latch-clear' not in check
    or 'ready.title_in_loop\n            && ready.menu_opened_latch' in exp
    or '!title_state.in_loop\n        && !title_state.in_textfadeout' in exp
):
    legacy_failures.append('product open-menu gate still depends on Loop/TextFadeout-only timing instead of validated dialog + latch-clear')
    autoload_static_failures += 1
if (
    'ARTIFACT_FORBIDDEN_SCAN_ROOTS' not in launch_guard_check
    or 'artifact_forbidden_launch_findings' not in launch_guard_check
    or 'artifact-forbidden-elden-ring-launch' not in launch_guard_check
    or 'ARTIFACT_FORBIDDEN_LAUNCH_TERMS' not in launch_guard_check
    or 'steam://rungameid/1245620' not in launch_guard_check
    or 'check-launch-guardrails.py' not in check_sh
):
    legacy_failures.append('quality gates do not scan generated runtime artifacts for forbidden Elden Ring Steam/protected launch forms')
    autoload_static_failures += 1
if (
    '.auto/runtime_probe.sh' not in direct_probe
    or 'eldenring.exe' not in direct_probe
    or 'terminate_runtime_pids' not in direct_probe
    or 'comm=$(<"$proc/comm")' not in direct_probe
    or '[[ "$comm" == "eldenring.exe"' not in direct_probe
    or 'ELDEN RING\\\\Game\\\\eldenring.exe' not in direct_probe
    or '[[ "$cmdline" == *"$GAME_DIR/eldenring.exe"* ]]' not in direct_probe
    or 'kill -9 "$pid"' not in direct_probe
    or 'steam://rungameid/1245620' in direct_probe
    or 'steam -applaunch 1245620' in direct_probe
    or 'start_protected_game.exe' in direct_probe
    or 'run-product-continue-direct-probe.sh' not in check_sh
):
    legacy_failures.append('approved direct/offline product probe wrapper is missing, unguarded, lacks exact runtime cleanup, or contains forbidden launch forms')
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
    'B80_DESERIALIZE_RVA',
    'drive_product_continue_post_click_dispatchers',
    'menu_continue_wrapper(',
    'b80_deserialize_67b290(',
]
for token in direct_tokens:
    if token in product_related:
        direct_failures.append(f'product/native submit body contains direct shortcut token {token}')
if 'CONTINUE_CONFIRM_RVA' in product_related and not (
    'MODAL-CONFIRM-DISABLED' in product_continue_body
    and 'modal_disable_ready' in product_continue_body
    and 'c30_loaded_sane' in product_continue_body
    and 'fp_real' in product_continue_body
):
    direct_failures.append('product/native submit body contains unguarded direct continue_confirm')

input_failures: list[str] = []
for token in ['input_probe_enabled', 'inject_nav_enabled', 'menu_input_probe', 'set_injected_key', 'SAFE_INPUT_CONFIRM', 'DIK_DOWN', 'XInput']:
    if token in product_input_audit:
        input_failures.append(f'product/native submit body contains input path token {token}')
if re.search(r'Down \+ accept.*product proof', prompt, re.IGNORECASE):
    input_failures.append('prompt still frames Down+accept as product proof')

dll_failures: list[str] = []
if 'er_effects_rs.dll' not in prompt and 'chainload DLL' not in prompt and 'DLL' not in prompt:
    dll_failures.append('prompt does not make DLL product vehicle explicit')
for token in ['eldenring.exe patch', 'patch eldenring.exe', 'loose asset edits as product']:
    if token in prompt.lower() and 'do not' not in prompt.lower():
        dll_failures.append(f'prompt may allow forbidden product vehicle: {token}')
if not native_fullread_body:
    dll_failures.append('missing native_fullread_tick implementation')

runtime_failures: list[str] = []
runtime_mode_failures: list[str] = []
eula_popup_failures: list[str] = []
save_data_popup_failures: list[str] = []
messagebox_dialog_failures: list[str] = []
server_status_failures: list[str] = []
for token in [
    'seamless_coop_loaded',
    'runtime_mode',
    'GetModuleHandleA',
    'ersc.dll',
]:
    if token not in telemetry_src:
        runtime_mode_failures.append(f'telemetry missing runtime-mode semaphore token {token}')
for token in [
    '--expected-runtime-mode',
    'runtime_mode_mismatch',
    'seamless_module_mappings',
    'SEAMLESS_MODULE_MARKERS',
    'preexisting_runtime_pids',
    'row.pid not in preexisting_runtime_pids',
    'target_window_capture_diagnostics',
    '"target_window_capture"',
    'target_window_capture_problems(selected, window_class)',
    'autoload_progress_summary',
    '"autoload_progress"',
    '"product_core_ready_blocker"',
    '"product_core_autoload_ticks"',
    'product_core_{product_core_blocker}',
    '"native_continue_chain_stage"',
    '"title_native_ready_last_masked"',
    '"title_native_ready_last_ret"',
    '"title_langselect_ready_last_masked"',
    '"title_langselect_ready_last_ret"',
]:
    if token not in watcher:
        runtime_mode_failures.append(f'readiness watcher missing runtime-mode/preexisting-pid/window-capture/autoload-progress diagnostic token {token}')
for token in [
    'PRODUCT_CORE_AUTOLOAD_TICKS',
    'PRODUCT_CORE_READY_BLOCKS',
    'PRODUCT_CORE_READY_SUCCESSES',
    'PRODUCT_CORE_OWNER_TICKS',
    'PRODUCT_CORE_LAST_OWNER',
    'PRODUCT_CORE_LAST_TITLE_IN_LOOP',
    'PRODUCT_CORE_LAST_MENU_OPENED_LATCH',
    'PRODUCT_CORE_LAST_PRESS_START_CONTEXT',
    'MENU_WINDOW_JOB_CTOR_HITS',
    'MENU_WINDOW_JOB_CTOR_SEMANTIC_HITS',
    'MENU_WINDOW_JOB_NATIVE_CTOR_B_HITS',
    'MENU_WINDOW_JOB_NATIVE_CTOR_B_CONTINUE_HITS',
    'MENU_WINDOW_JOB_IDLE_CTOR_HITS',
    'MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_HITS',
    'MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_CALLER_RVA',
    'MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_OUT_SLOT',
    'arg0_points_to_idle_item',
    'idle_continue_insert_match',
    'MENU_CONTINUE_IDLE_INSERT_HITS',
    'MENU_CONTINUE_IDLE_INSERT_LAST_CALLER_RVA',
    'MENU_CONTINUE_IDLE_INSERT_CALLER_RVA',
    'MENU_CONTINUE_IDLE_INSERT_CALLER_START_RVA',
    'MENU_CONTINUE_IDLE_INSERT_CALLER_END_RVA',
    'TASK_ENQUEUE_GENERIC_HITS',
    'TASK_ENQUEUE_GENERIC_SAMPLE0_CALLER_RVA',
    'TASK_ENQUEUE_GENERIC_SAMPLE1_ARG0_POINTEE',
    'TASK_ENQUEUE_GENERIC_IDLE_ITEM_MATCH_HITS',
    'TASK_ENQUEUE_IDLE_MATCH_ARG0_POINTEE',
    'TASK_ENQUEUE_IDLE_MATCH_ARG1_ITEM',
    'MENU_WINDOW_JOB_IDLE_CTOR_LAST_CALLER_RVA',
    'MENU_ITEM_UPDATE_HITS',
    'MENU_ITEM_UPDATE_SEMANTIC_HITS',
    'MENU_CONTINUE_CANDIDATE_ITEM',
    'MENU_CONTINUE_CANDIDATE_ACCEPT_CHANGES',
    'TITLE_NATIVE_READY_PREDICATE_HITS',
    'TITLE_NATIVE_READY_PREDICATE_LAST_OBJECT',
    'TITLE_NATIVE_READY_PREDICATE_LAST_MASKED',
    'record_continue_candidate',
    'PRODUCT_CORE_LAST_BLOCKER',
    'PRODUCT_CORE_LAST_BLOCKER.store(blocker, Ordering::SeqCst);\n        if tick % OWN_STEPPER_LOG_INTERVAL',
    'product_core_ready_blocker_label',
    'TITLE_OWNER_SCAN_ATTEMPTS',
    'TITLE_OWNER_SCAN_VTABLE_HITS',
    'TITLE_OWNER_SCAN_TABLE_REJECTS',
    'TITLE_OWNER_SCAN_STATE_REJECTS',
]:
    if token not in exp:
        runtime_mode_failures.append(f'experiments missing product-core/title-owner diagnostic token {token}')
for token in [
    'product_core_ready_blocker',
    'product_core_autoload_ticks',
    'product_core_owner_ticks',
    'product_core_last_owner',
    'product_core_last_title_in_loop',
    'product_core_last_menu_opened_latch',
    'product_core_last_press_start_context',
    'oracle_menu_window_ctor_hits',
    'oracle_menu_window_native_ctor_b_hits',
    'oracle_menu_window_native_ctor_b_last_accept',
    'oracle_menu_window_idle_ctor_hits',
    'oracle_menu_window_idle_ctor_continue_last_caller_rva',
    'oracle_menu_window_idle_ctor_continue_last_out_slot',
    'oracle_menu_continue_idle_insert_hits',
    'oracle_menu_continue_idle_insert_last_caller_rva',
    'oracle_task_enqueue_generic_hits',
    'oracle_task_enqueue_generic_sample0_caller_rva',
    'oracle_task_enqueue_generic_sample1_arg0_pointee',
    'oracle_task_enqueue_generic_idle_item_match_hits',
    'oracle_menu_window_idle_ctor_last_caller_rva',
    'oracle_menu_item_update_hits',
    'oracle_menu_continue_candidate_item',
    'oracle_menu_continue_candidate_last_accept',
    'oracle_title_native_ready_hits',
    'oracle_title_native_ready_last_object',
    'oracle_title_native_ready_last_masked',
    'oracle_title_native_ready_last_ret',
    'oracle_title_langselect_ready_last_masked',
    'oracle_title_langselect_ready_last_ret',
    'product_autoload_armed',
    'title_owner_scan_attempts',
    'title_owner_scan_vtable_hits',
    'title_owner_scan_last_candidate',
]:
    if token not in telemetry_src:
        runtime_mode_failures.append(f'telemetry missing product-core/title-owner diagnostic token {token}')
for token in [
    'oracle_msgbox_total_builds',
    'oracle_msgbox_any_seen',
]:
    if token not in telemetry_src:
        messagebox_dialog_failures.append(f'telemetry missing native MessageBoxDialog zero-oracle token {token}')
for token in [
    '--fail-on-messagebox-dialog',
    'native_messagebox_dialog_detected',
    'telemetry_messagebox_dialog_detected',
]:
    if token not in watcher:
        messagebox_dialog_failures.append(f'readiness watcher missing native MessageBoxDialog fail-fast token {token}')
for token in [
    'oracle_server_status_text_id',
    'oracle_server_status_any_seen',
]:
    if token not in telemetry_src:
        server_status_failures.append(f'telemetry missing native server/login status semaphore token {token}')
for token in [
    '--fail-on-server-status-semaphore',
    'native_server_status_semaphore_detected',
    'telemetry_server_status_semaphore_detected',
    '401120',
    '401160',
]:
    if token not in watcher:
        server_status_failures.append(f'readiness watcher missing server/login status fail-fast token {token}')
for token in [
    'oracle_msgbox_builder_args',
    'oracle_policy_window_total_builds',
    'oracle_policy_window_any_seen',
]:
    if token not in telemetry_src:
        eula_popup_failures.append(f'telemetry missing native legal-popup oracle token {token}')
for token in [
    '--fail-on-native-legal-popup',
    'native_legal_popup_detected',
    'telemetry_native_legal_popup_detected',
    'ToS_win64.fmg',
    '607200',
    'oracle_policy_window_total_builds',
]:
    if token not in watcher:
        eula_popup_failures.append(f'readiness watcher missing non-OCR legal-popup oracle token {token}')
for token in [
    '--visual-save-data-popup-check',
    'visual_save_data_popup_detected',
    'failed to load save data',
    'defer_unsafe_visual_capture_until_telemetry',
]:
    if token not in watcher:
        save_data_popup_failures.append(f'readiness watcher missing save-data-popup semaphore token {token}')
if '--defer-unsafe-visual-capture-until-telemetry' not in runtime_probe:
    save_data_popup_failures.append('runtime_probe.sh does not defer unsafe screenshot failure until native telemetry can arrive')
if any(token in product_continue_body for token in ['T_loadgame_menu_fallback', 'fire_live_loadgame_node', 'Load Game menu fallback']):
    eula_popup_failures.append('product path can still open native Load Game fallback instead of failing closed on invalid/empty Continue target')
required_runtime = ['ready', 'product_submit', 'result_chain', 'continue_load', 'deserialize', 'confirm', 'world', 'zero_input', 'expected_save', 'expected_animation', 'no_postload_popup']
legal_popup_by_dir: dict[str, list[str]] = {}
save_data_popup_by_dir: dict[str, list[str]] = {}
messagebox_by_dir: dict[str, list[str]] = {}
server_status_by_dir: dict[str, list[str]] = {}
runtime_mode_by_dir: dict[str, list[str]] = {}
title_cover_runtime_by_dir: dict[str, list[str]] = {}
runtime_artifacts_raw: list[str] = []
runtime_artifacts_raw_by_dir: dict[str, list[str]] = {}
best_runtime: tuple[int, Path | None, dict[str, bool]] = (0, None, {key: False for key in required_runtime})
latest_runtime_dir: Path | None = None
rt_root = root / 'target/runtime-probe'
if rt_root.exists():
    candidates = sorted(
        (p for pattern in ('product-core-*', 'product-continue-direct-*') for p in rt_root.glob(pattern) if p.is_dir()),
        key=lambda p: p.stat().st_mtime,
        reverse=True,
    )[:200]
    latest_runtime_dir = candidates[0] if candidates else None
    for d in candidates:
        proof = {key: False for key in required_runtime}
        for name in ['readiness-result.json', 'max-oracle-result.json', 'telemetry.json', 'er-effects-telemetry.json']:
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
            runtime_artifacts_raw.append(raw)
            runtime_artifacts_raw_by_dir.setdefault(d.name, []).append(raw)
            if (
                re.search(r'"reason"\s*:\s*"native_legal_popup_detected"', raw)
                or re.search(r'"oracle_policy_window_any_seen"\s*:\s*true', raw)
                or re.search(r'"oracle_policy_window_total_builds"\s*:\s*[1-9]\d*', raw)
            ):
                legal_popup_by_dir[d.name] = [
                    f'runtime artifact {d.name} detected EULA/legal/privacy popup from native packed-asset Text ID or TosTitle policy-window evidence'
                ]
            if re.search(r'"reason"\s*:\s*"native_messagebox_dialog_detected"', raw) or re.search(r'"oracle_msgbox_any_seen"\s*:\s*true', raw) or re.search(r'"oracle_msgbox_total_builds"\s*:\s*[1-9]\d*', raw):
                messagebox_by_dir[d.name] = [
                    f'runtime artifact {d.name} observed native CS::MessageBoxDialog build(s); product proof requires zero MessageBoxDialog popups'
                ]
            if (
                re.search(r'"reason"\s*:\s*"native_server_status_semaphore_detected"', raw)
                or re.search(r'"oracle_server_status_any_seen"\s*:\s*true', raw)
                or re.search(r'"oracle_server_status_total_seen"\s*:\s*[1-9]\d*', raw)
                or re.search(r'"oracle_server_status_text_id"\s*:\s*(401120|401150|401160|401165)', raw)
            ):
                server_status_by_dir[d.name] = [
                    f'runtime artifact {d.name} observed native server/login status semaphore; product proof requires no online status UI'
                ]
            if re.search(r'"reason"\s*:\s*"visual_save_data_popup_detected"', raw):
                save_data_popup_by_dir[d.name] = [
                    f'runtime artifact {d.name} failed with visual_save_data_popup_detected'
                ]
            seamless_observed = (
                re.search(r'"seamless_coop_loaded"\s*:\s*true', raw)
                or re.search(r'"runtime_mode_actual"\s*:\s*"seamless"', raw)
                or 'SeamlessCoop' in raw
                or 'ersc.dll' in raw
            )
            seamless_expected = re.search(r'"runtime_mode_expected"\s*:\s*"seamless"', raw)
            if seamless_observed and not seamless_expected:
                runtime_mode_by_dir.setdefault(d.name, []).append(
                    f'runtime artifact {d.name} loaded Seamless/ERSC markers while not marked runtime_mode_expected=seamless'
                )
            if 'simulated_button_presses_total' in raw and re.search(r'"simulated_button_presses_total"\s*:\s*0', raw):
                proof['zero_input'] = True
            submit_result_match = re.search(r'"oracle_native_submit_last_result"\s*:\s*"(0x[0-9a-fA-F]+)"', raw)
            event_result_match = re.search(r'"oracle_result_event_last_result"\s*:\s*"(0x[0-9a-fA-F]+)"', raw)
            action_result_match = re.search(r'"oracle_result_action_last_result"\s*:\s*"(0x[0-9a-fA-F]+)"', raw)
            same_result = bool(
                submit_result_match
                and event_result_match
                and action_result_match
                and submit_result_match.group(1) == event_result_match.group(1) == action_result_match.group(1)
            )
            fd4_submit_event_match = bool(
                re.search(r'"oracle_result_event_last_fd4_code"\s*:\s*"0x3"', raw)
                and re.search(r'"oracle_result_event_last_fd4_arg"\s*:\s*"0x0"', raw)
            )
            if (
                re.search(r'"oracle_native_submit_hits"\s*:\s*[1-9]\d*', raw)
                and re.search(r'"oracle_result_event_handler_hits"\s*:\s*[1-9]\d*', raw)
                and re.search(r'"oracle_result_action_builder_hits"\s*:\s*[1-9]\d*', raw)
                and same_result
                and fd4_submit_event_match
            ):
                proof['result_chain'] = True
            if re.search(r'world[-_ ]?stable|max oracle|SetState5', raw, re.IGNORECASE):
                proof['world'] = True
            if (
                re.search(r'"native_continue_chain_stage"\s*:\s*"world_loaded"', raw)
                or re.search(r'"oracle_player_present"\s*:\s*true', raw)
                and re.search(r'"oracle_char_name"\s*:\s*"[^"_\s][^"]*"', raw)
                and re.search(r'"oracle_saved_map_c30"\s*:\s*"?0x[0-9a-fA-F]+"?', raw)
            ):
                proof['world'] = True
            if re.search(r'"native_continue_chain_stage"\s*:\s*"world_loaded"', raw):
                proof['continue_load'] = True
                proof['deserialize'] = True
                proof['confirm'] = True
            overlay_portrait_source_cover_rendered = bool(
                re.search(r'"oracle_title_overlay_cover_rendered"\s*:\s*true', raw)
                and re.search(r'"oracle_title_overlay_cover_render_calls"\s*:\s*[1-9]\d*', raw)
                and re.search(r'"oracle_title_overlay_cover_last_display_size"\s*:\s*\[\s*(?:[2-9]\d{2,}|1[0-9]{3,})\s*,\s*(?:[2-9]\d{2,}|1[0-9]{3,})\s*\]', raw)
                and re.search(r'"oracle_title_custom_cover_profile_source_ready"\s*:\s*true', raw)
                and re.search(r'"oracle_title_custom_cover_profile_source_tex_rescap"\s*:\s*[1-9]\d*', raw)
            )
            portrait_visible_surface_bound = bool(re.search(r'"oracle_title_portrait_visible_surface_bound"\s*:\s*true', raw))
            if re.search(r'"oracle_title_profile_cover_bound_to_logo_surface"\s*:\s*false', raw) and not overlay_portrait_source_cover_rendered and not portrait_visible_surface_bound:
                # Count this runtime artifact's missing-cover semaphore once. Several JSON evidence
                # files in the same artifact dir can contain the same oracle snapshot; charging the
                # same artifact once per file over-penalizes a single product failure. The overlay
                # successor route only counts when it is both visible and gated by the live
                # CSMenuProfModelRend/CSEzOffscreenRend/SYSTEX_Menu_Profile portrait source.
                title_cover_runtime_by_dir.setdefault(d.name, [])
                missing_cover_msg = f'runtime artifact {d.name} has no custom/profile cover bound to the visible logo/title surface or RAM-backed rendered overlay successor cover'
                if missing_cover_msg not in title_cover_runtime_by_dir[d.name]:
                    title_cover_runtime_by_dir[d.name].append(missing_cover_msg)
            oracle = data.get('oracle') if isinstance(data.get('oracle'), dict) else {}
            expected_oracle = oracle.get('expected') if isinstance(oracle.get('expected'), dict) else {}
            observed_oracle = oracle.get('observed') if isinstance(oracle.get('observed'), dict) else {}
            observed_name = oracle.get('character_name')
            expected_name = expected_oracle.get('character_name')
            if (
                oracle.get('expected_save_match') is True
                and observed_name
                and not empty_name_like(observed_name)
                and not empty_name_like(expected_name)
                and observed_oracle.get('character_name_empty_like') is not True
                and expected_oracle.get('character_name_empty_like') is not True
            ):
                proof['expected_save'] = True
            if oracle.get('expected_animation_match') is True:
                proof['expected_animation'] = True
            if oracle.get('native_result_chain_ready') is True:
                # The watcher only sets this after passive native result-event/action-wrapper/insert
                # evidence agrees (outer submit hook if present, otherwise same-result event/action
                # chain). Count it as both the product submit edge and the result chain edge.
                proof['product_submit'] = True
                proof['result_chain'] = True
            if oracle.get('no_postload_popup') is True:
                proof['no_postload_popup'] = True
        legal_evidence: list[str] = []
        for legal_path in sorted(d.glob('legal-popup-check-*.json')):
            try:
                legal_data = json.loads(legal_path.read_text(encoding='utf-8', errors='replace'))
            except Exception:
                continue
            if legal_data.get('legal_popup_detected') is True:
                matches = legal_data.get('ocr_matches') or []
                legal_evidence.append(f'{legal_path.name} matches={matches}')
        readiness_detected_legal = False
        p = d / 'readiness-result.json'
        if p.exists():
            try:
                data = json.loads(p.read_text(encoding='utf-8', errors='replace'))
            except Exception:
                data = {}
            readiness_detected_legal = data.get('reason') == 'visual_legal_popup_detected'
        if legal_evidence:
            legal_popup_by_dir[d.name] = [
                f'runtime artifact {d.name} detected EULA/legal popup from captured target-window OCR evidence: {"; ".join(legal_evidence)}'
            ]
        elif readiness_detected_legal:
            legal_popup_by_dir[d.name] = [
                f'runtime artifact {d.name} failed immediately with visual_legal_popup_detected but legal OCR evidence file was missing'
            ]
        save_data_evidence: list[str] = []
        for save_popup_path in sorted(d.glob('save-data-popup-check-*.json')):
            try:
                save_popup_data = json.loads(save_popup_path.read_text(encoding='utf-8', errors='replace'))
            except Exception:
                continue
            if save_popup_data.get('save_data_popup_detected') is True:
                matches = save_popup_data.get('ocr_matches') or []
                save_data_evidence.append(f'{save_popup_path.name} matches={matches}')
        if save_data_evidence:
            save_data_popup_by_dir[d.name] = [
                f'runtime artifact {d.name} detected failed-save-data popup from captured target-window OCR evidence: {"; ".join(save_data_evidence)}'
            ]
        logo_analysis_path = d / 'logo-replacement-screenshot-analysis.json'
        if logo_analysis_path.exists():
            try:
                logo_analysis = json.loads(logo_analysis_path.read_text(encoding='utf-8', errors='replace'))
            except Exception:
                logo_analysis = {}
            if logo_analysis.get('known_false_positive_signature') is True:
                ratio = logo_analysis.get('black32_ratio')
                title_cover_runtime_by_dir.setdefault(d.name, [])
                msg = (
                    f'runtime artifact {d.name} logo-replacement screenshot has black32_ratio={ratio}; '
                    'matches the known dark LOAD GAME/ProfileSelect false-positive panel rather than a portrait-on-black cover'
                )
                if msg not in title_cover_runtime_by_dir[d.name]:
                    title_cover_runtime_by_dir[d.name].append(msg)
        for name in ['er-effects-autoload-debug.log', 'autoload-debug-live.final.log', 'continue-trace-game.final.log', 'continue-trace-game.log']:
            p = d / name
            if not p.exists():
                continue
            text = p.read_text(encoding='utf-8', errors='replace')[-200_000:]
            if 'CORRUPTED-SAVE SEMAPHORE' in text or 'GetGR_System_Message id=401106' in text:
                save_data_popup_by_dir.setdefault(d.name, []).append(
                    f'runtime artifact {d.name} detected corrupted-save native semaphore in {name}'
                )
            if 'native-fullread: SUBMIT' in text or 'FULL-INIT' in text:
                proof['product_submit'] = True
            if 'native-fullread: b80 reached RESIDENT' in text or 'full read' in text or 'FULL-INIT' in text:
                proof['continue_load'] = True
            if 'native-fullread: DESER' in text or 'b80_deserialize_67b290' in text or 'CAP b80_deserialize' in text:
                proof['deserialize'] = True
            if 'native-fullread: *** COMMIT continue_confirm' in text or 'CAP continue_confirm' in text or 'STAGE2-SETSTATE5 fired' in text:
                proof['confirm'] = True
            if 'simulated_button_presses_total=0' in text or 'simulated_button_presses_total": 0' in text:
                proof['zero_input'] = True
            if 'world-stable' in text or 'SetState5' in text or 'max oracle' in text:
                proof['world'] = True
        count = sum(proof.values())
        hard_popup_dirs = set(messagebox_by_dir) | set(legal_popup_by_dir) | set(save_data_popup_by_dir) | set(server_status_by_dir)
        best_has_hard_popup = best_runtime[1] is not None and best_runtime[1].name in hard_popup_dirs
        has_hard_popup = d.name in hard_popup_dirs
        if count > best_runtime[0] or (count == best_runtime[0] and best_has_hard_popup and not has_hard_popup):
            best_runtime = (count, d, proof)
best_count, best_dir, proof = best_runtime
if best_dir is None:
    runtime_failures.append('runtime proof missing product-core artifact directory')
else:
    missing = [key for key in required_runtime if not proof[key]]
    if missing:
        severe_runtime_keys = {'ready', 'world', 'zero_input', 'expected_save', 'no_postload_popup'}
        severity = 'severe' if any(key in severe_runtime_keys for key in missing) else 'detail'
        runtime_failures.append(f'runtime proof {severity} artifact {best_dir.name} missing {",".join(missing)}')
    runtime_mode_failures.extend(runtime_mode_by_dir.get(best_dir.name, []))
    eula_popup_failures.extend(legal_popup_by_dir.get(best_dir.name, []))
    server_status_failures.extend(server_status_by_dir.get(best_dir.name, []))
scored_runtime_dirs = {p.name for p in [best_dir, latest_runtime_dir] if p is not None}
scored_runtime_artifacts_raw = [
    raw
    for dir_name in scored_runtime_dirs
    for raw in runtime_artifacts_raw_by_dir.get(dir_name, [])
]
for dir_name in scored_runtime_dirs:
    messagebox_dialog_failures.extend(messagebox_by_dir.get(dir_name, []))
    save_data_popup_failures.extend(save_data_popup_by_dir.get(dir_name, []))
    server_status_failures.extend(server_status_by_dir.get(dir_name, []))

native_trace_blockers: list[str] = []
native_trace_hits_total = 0
native_trace_unique_breakpoints = 0
native_trace_latest = ''
trace_summaries = []
if rt_root.exists():
    trace_summaries = sorted(
        rt_root.glob('user-drive-trace-*/trace-hit-summary.json'),
        key=lambda p: p.stat().st_mtime,
        reverse=True,
    )
if trace_summaries:
    latest_trace = trace_summaries[0]
    native_trace_latest = latest_trace.parent.name
    try:
        trace_data = json.loads(latest_trace.read_text(encoding='utf-8', errors='replace'))
    except Exception as exc:
        trace_data = {}
        native_trace_blockers.append(f'native trace summary {latest_trace} is not readable JSON: {exc}')
    hit_counts = trace_data.get('hit_counts') if isinstance(trace_data.get('hit_counts'), dict) else {}
    native_trace_hits_total = int(trace_data.get('total_hits') or 0)
    native_trace_unique_breakpoints = int(trace_data.get('unique_hits') or len(hit_counts))
    required_trace_hits = ['0x14082c240', '0x14082c2c8', '0x14082c374', '0x14067a810', '0x14082c521']
    missing_trace_hits = [addr for addr in required_trace_hits if int(hit_counts.get(addr, 0) or 0) <= 0]
    if missing_trace_hits:
        native_trace_blockers.append(
            f'native trace {native_trace_latest} missing key Load Game continuation hits: {",".join(missing_trace_hits)}'
        )
    if trace_data.get('attached_marker') is not True or trace_data.get('continuing_marker') is not True:
        native_trace_blockers.append(f'native trace {native_trace_latest} missing attached/continuing observer markers')
else:
    native_trace_blockers.append('native user-driven trace summary missing; tracebreakpoint/tooling blocker may still be unresolved')

title_cover_failures: list[str] = []
runtime_title_cover_failure_count = 0
for dir_name in scored_runtime_dirs:
    runtime_cover_failures = title_cover_runtime_by_dir.get(dir_name, [])
    runtime_title_cover_failure_count += len(runtime_cover_failures)
    title_cover_failures.extend(runtime_cover_failures)
title_cover_gate = function_body('title_native_menu_visual_suppression_enabled', exp_code) or ''
title_cover_hook = function_body('title_native_menu_visual_begin_title_hook', exp_code) or ''
title_cover_render_hook = function_body('title_native_menu_visual_window_fadein_hook', exp_code) or ''
# Missing the actual masquerade scene/character cover is a major product failure. A run that only
# hides the native title/PAB and reaches Banon is not close to complete Part B and must not rank as
# a high-water mark.
title_cover_penalty = runtime_title_cover_failure_count * 200
part_a_common = (
    '!save_override_telemetry_only()' in title_cover_gate
    and 'autoload_disabled()' in title_cover_gate
    and 'std::env::var' not in title_cover_gate
    and 'er-effects-' not in title_cover_gate
    and 'START_TITLE_NATIVE_MENU_VISUAL_SUPPRESS.call_once' in lib_code
    and 'install_title_native_menu_visual_suppression_hook' in lib_code
    and lib_code.find('START_TITLE_NATIVE_MENU_VISUAL_SUPPRESS.call_once') < lib_code.find('START_MENU_WINDOW_LATCH.call_once')
    and 'TITLE_NATIVE_MENU_VISUAL_BEGIN_TITLE_RVA: usize = 0x81f9f0' in code
    and 'TITLE_NATIVE_MENU_VISUAL_FACTORY_RVA: usize = 0x7acbf0' in code
    and 'TITLE_NATIVE_MENU_VISUAL_NAME: &str = "05_000_Title"' in code
    and 'TITLE_NATIVE_MENU_VISUAL_SUPPRESSED_BUILDS.fetch_add' in title_cover_hook
    and 'oracle_title_native_menu_visual_suppressed_builds' in telemetry_src
    and 'title_native_menu_visual_suppressed_builds' in watcher
)
part_a_null_slot = '(out_slot as *mut usize).write(null)' in title_cover_hook
part_a_render_only = (
    'TITLE_NATIVE_MENU_VISUAL_SUPPRESS_ORIG.load' in title_cover_hook
    and 'native_job' in title_cover_hook
    and 'START_TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS.call_once' in lib_code
    and 'install_title_native_menu_visual_render_suppression_hook' in lib_code
    and 'TITLE_NATIVE_MENU_VISUAL_WINDOW_FADEIN_RVA: usize = 0x744dd0' in code
    and 'TITLE_NATIVE_MENU_VISUAL_WINDOW_FADEIN_RUN_CALLER_RVA: usize = 0x7ad530' in code
    and 'CS_MENU_MAN_GLOBAL_RVA: usize = 0x3d6b7b0' in code
    and 'TITLE_NATIVE_MENU_VISUAL_VISIBLE_FLAGS_MASK: u8 = 0x3' in code
    and 'TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESSED_WINDOWS' in title_cover_render_hook
    and 'fetch_add' in title_cover_render_hook
    and 'flags_before & !TITLE_NATIVE_MENU_VISUAL_VISIBLE_FLAGS_MASK' in title_cover_render_hook
    and '.write_volatile(flags_after)' in title_cover_render_hook
    and 'oracle_title_native_menu_visual_render_suppressed_windows' in telemetry_src
    and 'title_native_menu_visual_render_suppressed_windows' in watcher
)
if not (part_a_common and (part_a_null_slot or part_a_render_only)):
    title_cover_failures.append('Part A native 05_000_Title BeginTitle visual suppression is missing or not independently observable')
    title_cover_penalty += 100

cover_docs = doc_text()
if not (
    '05_001_title_logo.gfx' in cover_docs
    and 'MENU_Title_EldenRing_01' in cover_docs
    and 'no `MENU_DummyProfileFace`' in cover_docs
    and 'no `SYSTEX_Menu_Profile`' in cover_docs
    and 'custom Scaleform target' in cover_docs
):
    title_cover_failures.append('Part B asset fork unresolved: 05_001_Title_Logo dummy-texture availability not decided')
    title_cover_penalty += 50

part_b_pressstart_suppression = (
    'TITLE_NATIVE_MENU_VISUAL_TITLE_INFORMATION_RVA: usize = 0x81f8d0' in code
    and 'TITLE_PAB_INFORMATION_VISUAL_NAME: &str = "05_020_TitleInformation"' in code
    and 'oracle_title_pab_information_visual_any_built' in telemetry_src
    and 'TITLE_PRESS_START_SET_VISIBLE_RVA: usize = 0x733340' in code
    and 'TITLE_GFX_VALUE_SET_VISIBLE_RVA: usize = 0xd844d0' in code
    and 'TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA: usize = 0x74a2f0' in code
    and 'install_title_gfx_value_set_visible_hook' in lib_code
    and 'install_title_scene_obj_proxy_named_child_bind_hook' in lib_code
    and 'oracle_title_press_start_gfx_any_hidden' in telemetry_src
    and 'oracle_title_press_start_bind_any_hidden' in telemetry_src
    and 'oracle_title_press_start_gfx_force_false_any' in telemetry_src
)
if not part_b_pressstart_suppression:
    title_cover_failures.append('Part B/PAB suppression path is not independently observable yet')
    title_cover_penalty += 50

# The experimental 05_010_ProfileSelect/MenuWindowJob canvas was user-falsified: it can advance to a
# blank Load Game/ProfileSelect frame but is not the clean custom title cover and can interfere with
# PAB/title advance. Do not count ProfileSelect canvas build/run telemetry as Part B success.
profile_select_canvas_installed = (
    'START_TITLE_CUSTOM_COVER_RUN.call_once' in lib_code
    or 'install_title_custom_cover_run_hook' in lib_code
    or 'build_profile_select_cover_job(base, rdx, r8' in code
)
profile_select_one_tick_cropped = (
    'current_calls >= 1' in code
    and 'MENU_DummyProfileFace_01' in code
    and 'TITLE_PROFILE_FACE_TRANSFORM_APPLIED' in code
    and 'TITLE_PROFILE_FACE_OTHER_HIDDEN' in code
    and 'TITLE_GFX_VALUE_SET_POSITION_RVA' in code
    and 'TITLE_GFX_VALUE_SET_SCALE_RVA' in code
    and 'oracle_title_loaded_character_portrait_rendered' in telemetry_src + '\n' + watcher
)
if profile_select_canvas_installed and not profile_select_one_tick_cropped:
    title_cover_failures.append('Part B false-positive guard: ProfileSelect/MenuWindowJob canvas is still installed without one-tick crop/scale portrait proof')
    title_cover_penalty += 100

# User-visible runtime falsified the old Part-A/B semaphores twice: native title/logo/PAB/Continue
# remained visible while CSMenuMan+0x90 said hidden, and again while TitleBackViewParts FadeIn was
# suppressed. The real visible logo surface is TitleBackViewParts (`05_001_Title_Logo`) at
# TitleTopDialog+0xaa8, but merely naming/suppressing its FadeIn is not product proof. The product
# goal is now narrower than "any clean cover": render the loaded character portrait/profile during
# boot-init and keep it up until the native map-loading screen takes over. A generic text/rectangle
# hudhook overlay is useful diagnostic scaffolding but must not score as final cover success.
portrait_overlay_cover_observable = (
    'draw_title_overlay_cover' in overlay_code
    and 'title_portrait_source_ready' in overlay_code
    and 'TITLE_CUSTOM_COVER_PROFILE_SOURCE_TEX_RESCAP' in overlay_code
    and 'TITLE_OVERLAY_COVER_RENDER_CALLS.fetch_add' in overlay_code
    and 'oracle_title_overlay_cover_rendered' in telemetry_src + '\n' + watcher
    and 'oracle_title_custom_cover_profile_source_ready' in telemetry_src + '\n' + watcher
    and 'oracle_title_custom_cover_profile_source_tex_rescap' in telemetry_src + '\n' + watcher
    and 'title_overlay_cover_display_sane' in telemetry_src
    and (
        'SYSTEX_Menu_Profile' in overlay_code
        or 'profile portrait' in overlay_code.lower()
        or 'character portrait' in overlay_code.lower()
    )
    and '&& !product_autoload_enabled()' in lib_code
)
actual_logo_profile_cover_observable = (
    (
        'TitleBackViewParts' in code + '\n' + telemetry_src + '\n' + watcher
        and '05_001_Title_Logo' in code + '\n' + telemetry_src + '\n' + watcher
        and 'profile_summary' in code + '\n' + telemetry_src + '\n' + watcher
        and 'SYSTEX_Menu_Profile' in code + '\n' + telemetry_src + '\n' + watcher
        and 'oracle_title_logo_gfx_visibility' in telemetry_src + '\n' + watcher
        and 'oracle_title_profile_cover_bound_to_logo_surface' in telemetry_src + '\n' + watcher
    )
    or portrait_overlay_cover_observable
    # `oracle_title_portrait_visible_surface_bound` alone is no longer acceptable: the
    # logo-replacement event screenshot showed it can assert while the screen is still a dark
    # LOAD GAME/ProfileSelect panel. Future success needs a real pixel/native visibility semaphore.
)
if 'draw_title_overlay_cover' in overlay_code and not portrait_overlay_cover_observable:
    title_cover_failures.append('Part B false-positive guard: hudhook cover is still generic text/rectangle scaffolding, not the loaded character portrait')
    title_cover_penalty += 100
if not actual_logo_profile_cover_observable:
    title_cover_failures.append('Part B false-positive guard: no RAM-backed oracle proves the loaded character portrait covers boot-init until the native map-loading screen takes over')
    title_cover_penalty += 50
if portrait_overlay_cover_observable and 'oracle_title_overlay_cover_texture_bound' not in telemetry_src + '\n' + watcher:
    title_cover_failures.append('Part B remaining gap: overlay is gated by the RAM-backed portrait source but does not yet prove the actual profile texture is bound/drawn')
    title_cover_penalty += 50

# Hard fail-closed product gate: source/texture-handle consumption is not equivalent to rendering
# the loaded character portrait at the right time. A valid max-score artifact must expose a runtime
# oracle that proves the portrait itself was rendered on the cover surface during boot-init and was
# held until native loading-screen takeover. Until that oracle exists and is asserted in the latest
# runtime proof, the portrait-cover objective is objectively unmet and the score must be 0.
portrait_render_oracle_present = (
    'oracle_title_loaded_character_portrait_rendered' in telemetry_src + '\n' + watcher
    and 'oracle_title_loaded_character_portrait_visible_during_boot' in telemetry_src + '\n' + watcher
    and 'oracle_title_loaded_character_portrait_held_until_loading_takeover' in telemetry_src + '\n' + watcher
)
portrait_render_oracle_true = any(
    re.search(r'"oracle_title_loaded_character_portrait_rendered"\s*:\s*true', raw)
    and re.search(r'"oracle_title_loaded_character_portrait_visible_during_boot"\s*:\s*true', raw)
    and re.search(r'"oracle_title_loaded_character_portrait_held_until_loading_takeover"\s*:\s*true', raw)
    and re.search(r'"oracle_title_portrait_pixels_visible"\s*:\s*true', raw)
    for raw in scored_runtime_artifacts_raw
)
offline_face_texture_runtime_drawn = 0
stylized_facedata_pixel_oracle = 0
stylized_facedata_pixel_reason = ''
if best_dir is not None:
    runtime_raw = (best_dir / 'readiness-result.json').read_text(encoding='utf-8', errors='replace') if (best_dir / 'readiness-result.json').exists() else ''
    runtime_raw += '\n'
    runtime_raw += (best_dir / 'er-effects-telemetry.json').read_text(encoding='utf-8', errors='replace') if (best_dir / 'er-effects-telemetry.json').exists() else ''
    if re.search(r'"oracle_title_offline_face_texture_uploaded"\s*:\s*true', runtime_raw) and re.search(r'"oracle_title_offline_face_texture_drawn"\s*:\s*true', runtime_raw):
        offline_face_texture_runtime_drawn = 1
    analysis_path = best_dir / 'logo-replacement-screenshot-analysis.json'
    if offline_face_texture_runtime_drawn and analysis_path.exists():
        try:
            analysis = json.loads(analysis_path.read_text(encoding='utf-8', errors='replace'))
            if (
                analysis.get('black_ratio_sane_for_portrait_stage') is True
                and analysis.get('known_false_positive_signature') is False
                and float(analysis.get('black32_ratio', 0.0)) >= float(analysis.get('black32_min_ratio', 0.55))
                and float(analysis.get('dark64_ratio', 1.0)) <= float(analysis.get('dark64_max_ratio', 0.95))
            ):
                stylized_facedata_pixel_oracle = 1
                stylized_facedata_pixel_reason = f"black32={analysis.get('black32_ratio')} dark64={analysis.get('dark64_ratio')}"
        except Exception as exc:
            stylized_facedata_pixel_reason = f'analysis parse failed: {exc}'
if stylized_facedata_pixel_oracle:
    print(f'DETAIL stylized_facedata_pixel_oracle visible diagnostic passed ({stylized_facedata_pixel_reason}); secondary diagnostic only, not final portrait_pixels_visible proof')

visible_surface_without_pixels_false_positive = (
    not stylized_facedata_pixel_oracle
    and any(
        re.search(r'"oracle_title_portrait_visible_surface_bound"\s*:\s*true', raw)
        and not re.search(r'"oracle_title_portrait_pixels_visible"\s*:\s*true', raw)
        for raw in scored_runtime_artifacts_raw
    )
)
profile_select_transform_false_positive = any(
    re.search(r'"oracle_title_loaded_character_portrait_rendered"\s*:\s*true', raw)
    and re.search(r'"oracle_title_custom_cover_profile_select_builds"\s*:\s*[1-9]', raw)
    and re.search(r'"oracle_title_custom_cover_run_calls"\s*:\s*1', raw)
    and re.search(r'"oracle_title_profile_face_transform_applied"\s*:\s*true', raw)
    and re.search(r'"oracle_title_profile_cover_bound_to_logo_surface"\s*:\s*false', raw)
    and not re.search(r'"oracle_title_portrait_pixels_visible"\s*:\s*true', raw)
    and not re.search(r'"oracle_title_portrait_visible_surface_bound"\s*:\s*true', raw)
    for raw in scored_runtime_artifacts_raw
)
if visible_surface_without_pixels_false_positive:
    title_cover_failures.append('Part B hard gate: visible-surface/ProfileSelect bind semaphore was visually falsified by logo-replacement screenshot; require oracle_title_portrait_pixels_visible or an equivalent native/pixel semaphore')
    title_cover_penalty += MAX_SCORE
elif profile_select_transform_false_positive:
    title_cover_failures.append('Part B hard gate: ProfileSelect one-tick transform/SYSTEX semaphores are a proven visual false positive; require a real visible-pixel/surface oracle, not transform flags')
    title_cover_penalty += MAX_SCORE
elif not (portrait_render_oracle_present and portrait_render_oracle_true):
    title_cover_failures.append('Part B hard gate: no runtime oracle proves the loaded character portrait pixels are visible at the right time; source/texture-handle/visible-surface telemetry and generic overlay drawing are not product success')
    title_cover_penalty += MAX_SCORE

overlay_body = function_body('draw_title_overlay_cover', overlay_code) or ''
generic_black_overlay_false_positive = (
    'add_rect([0.0, 0.0], [width, height]' in overlay_body
    and '.filled(true)' in overlay_body
    and 'add_image' not in overlay_body
)
if generic_black_overlay_false_positive:
    title_cover_failures.append('Part B visual-beauty gate: fullscreen black/dark rectangle overlay is a product false positive; the cover must visibly render the loaded character portrait')
    title_cover_penalty += 100

false_positives = 1 if (generic_black_overlay_false_positive or visible_surface_without_pixels_false_positive) else 0
all_detail_failures = []
for group in [legacy_failures, asset_failures, dll_failures, native_failures, field58_failures, direct_failures, input_failures, runtime_failures, runtime_mode_failures, eula_popup_failures, save_data_popup_failures, messagebox_dialog_failures, server_status_failures, title_cover_failures]:
    all_detail_failures.extend(group)

runtime_weight = 320 if any('runtime proof severe' in failure for failure in runtime_failures) else 80
weights = {
    'readiness': 15,
    'asset': 35,
    'dll': 45,
    'native': 45,
    'field58': 100,
    'direct': 85,
    'input': 85,
    'runtime': runtime_weight,
    'runtime_mode': 120,
    'eula_popup': 80,
    'save_data_popup': 160,
    'messagebox_dialog': 160,
    'server_status': 160,
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
    + len(runtime_mode_failures) * weights['runtime_mode']
    + len(eula_popup_failures) * weights['eula_popup']
    + len(save_data_popup_failures) * weights['save_data_popup']
    + len(messagebox_dialog_failures) * weights['messagebox_dialog']
    + len(server_status_failures) * weights['server_status']
    + title_cover_penalty
    + false_positives * weights['false_positive']
)
score = max(0, MAX_SCORE - penalty)

offline_face_source_ready = 0
offline_face_source_errors: list[str] = []
offline_face_source_hash = ''
offline_face_source_name = ''
hudhook_cpu_texture_bridge_ready = 0
hudhook_cpu_texture_bridge_errors: list[str] = []
try:
    with tempfile.NamedTemporaryFile(prefix='er-face-source-', suffix='.json', delete=False) as tmp:
        tmp_path = Path(tmp.name)
    try:
        subprocess.run(
            [
                'python3',
                str(root / 'scripts/save-slot-oracle.py'),
                '--save',
                os.environ.get('ER_EFFECTS_GOLD_SAVE', str(root / 'save-files/150-Banon/ER0000.sl2')),
                '--slot',
                os.environ.get('ER_EFFECTS_GOLD_SLOT', '0'),
                '--output',
                str(tmp_path),
            ],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=10,
        )
        face_oracle = json.loads(tmp_path.read_text(encoding='utf-8', errors='replace'))
    finally:
        tmp_path.unlink(missing_ok=True)
    decoded = face_oracle.get('decoded_fields', {}) if isinstance(face_oracle, dict) else {}
    face_hash = decoded.get('face_data_buffer_sha256')
    face_fields = decoded.get('face_body_fields')
    name = decoded.get('name') or decoded.get('character_name') or ''
    offline_face_source_name = str(name)
    if isinstance(face_hash, str):
        offline_face_source_hash = face_hash
    if not isinstance(face_hash, str) or not re.fullmatch(r'[0-9a-f]{64}', face_hash):
        offline_face_source_errors.append('save-slot-oracle did not expose a 64-hex FaceData buffer hash')
    if not isinstance(face_fields, dict) or len(face_fields) < 8:
        offline_face_source_errors.append('save-slot-oracle did not expose a populated face/body field map')
    if empty_name_like(str(name)):
        offline_face_source_errors.append('save-slot-oracle decoded an empty-like character name for the gold slot')
    if not offline_face_source_errors:
        offline_face_source_ready = 1
except Exception as exc:
    offline_face_source_errors.append(f'offline face source check crashed: {exc}')

real_portrait_asset_candidates = 0
profile_placeholder_assets_only = 0
portrait_asset_inventory_details: list[str] = []
portrait_asset_roots = [
    root / 'target/autoresearch/gfx-analysis/ffdec-export/images/05_010_profileselect.gfx',
    root / 'target/autoresearch/gfx-analysis/script-smoke/ffdec-export/image/05_010_profileselect.gfx',
]
placeholder_assets = []
chrome_assets = []
real_candidate_assets = []
chrome_tokens = (
    'cursor',
    'base',
    'profileselect',
    'menu_fl_',
    'line',
    'kick',
    'option',
    'deco',
    'waku',
    'bar',
    'arrow',
    'bloodmessage',
)
for asset_root in portrait_asset_roots:
    if not asset_root.exists():
        continue
    for path in asset_root.glob('*.png'):
        lower = path.name.lower()
        if 'dummyprofileface' in lower:
            placeholder_assets.append(path)
        elif any(token in lower for token in chrome_tokens):
            chrome_assets.append(path)
        elif any(token in lower for token in ('profile', 'face', 'portrait')):
            real_candidate_assets.append(path)
if real_candidate_assets:
    real_portrait_asset_candidates = len(real_candidate_assets)
    portrait_asset_inventory_details.append('real-looking non-chrome profile/face candidates: ' + ', '.join(str(p) for p in real_candidate_assets[:5]))
elif placeholder_assets:
    profile_placeholder_assets_only = 1
    portrait_asset_inventory_details.append(f'only dummy ProfileSelect placeholder face assets found ({len(placeholder_assets)} pngs); excluded {len(chrome_assets)} ProfileSelect chrome/surface PNGs; no loaded-character/precomputed portrait asset is present in FFDEC exports')
elif chrome_assets:
    portrait_asset_inventory_details.append(f'only ProfileSelect chrome/surface PNGs found ({len(chrome_assets)} pngs); no loaded-character/precomputed portrait asset is present in FFDEC exports')
else:
    portrait_asset_inventory_details.append('no profile/face/portrait PNG assets found in known FFDEC export roots')
for detail in portrait_asset_inventory_details:
    print(f'DETAIL portrait_asset_inventory {detail}')

for failure in offline_face_source_errors:
    print(f'DETAIL offline_face_source {failure}')
if offline_face_source_ready:
    print(f'DETAIL offline_face_source ready name={offline_face_source_name} face_sha256={offline_face_source_hash}')

hudhook_sources = []
for hudhook_root in (Path.home() / '.cargo/git/checkouts').glob('hudhook-*') if (Path.home() / '.cargo/git/checkouts').exists() else []:
    for lib_rs in hudhook_root.glob('*/src/lib.rs'):
        try:
            hudhook_sources.append(lib_rs.read_text(encoding='utf-8', errors='replace'))
        except OSError:
            pass
if not any('pub trait RenderContext' in src and 'fn load_texture(&mut self, data: &[u8], width: u32, height: u32)' in src for src in hudhook_sources):
    hudhook_cpu_texture_bridge_errors.append('local hudhook checkout does not expose RenderContext::load_texture(data,width,height)')
if not re.search(r'fn initialize\(&mut self, ctx: &mut Context, [A-Za-z_][A-Za-z0-9_]*: &mut dyn (?:hudhook::)?RenderContext\)', overlay_code):
    hudhook_cpu_texture_bridge_errors.append('overlay initialize does not receive a hudhook RenderContext for CPU texture upload')
if not hudhook_cpu_texture_bridge_errors:
    hudhook_cpu_texture_bridge_ready = 1
for failure in hudhook_cpu_texture_bridge_errors:
    print(f'DETAIL hudhook_cpu_texture_bridge {failure}')
if hudhook_cpu_texture_bridge_ready:
    print('DETAIL hudhook_cpu_texture_bridge ready: RenderContext::load_texture can upload CPU RGBA into a TextureId during overlay initialize')

for failure in all_detail_failures:
    print(f'DETAIL {failure}')
for failure in native_trace_blockers:
    print(f'DETAIL native_trace {failure}')
if native_trace_latest:
    print(f'DETAIL native_trace_latest={native_trace_latest}')
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
print(f'METRIC runtime_mode_failures={len(runtime_mode_failures)}')
print(f'METRIC eula_popup_failures={len(eula_popup_failures)}')
print(f'METRIC save_data_popup_failures={len(save_data_popup_failures)}')
print(f'METRIC messagebox_dialog_failures={len(messagebox_dialog_failures)}')
print(f'METRIC server_status_failures={len(server_status_failures)}')
print(f'METRIC title_cover_failures={len(title_cover_failures)}')
print(f'METRIC title_cover_penalty={title_cover_penalty}')
print(f'METRIC native_trace_blockers={len(native_trace_blockers)}')
print(f'METRIC native_trace_hits_total={native_trace_hits_total}')
print(f'METRIC native_trace_unique_breakpoints={native_trace_unique_breakpoints}')
print(f'METRIC false_positives={false_positives}')
print(f'METRIC offline_face_source_ready={offline_face_source_ready}')
print(f'METRIC hudhook_cpu_texture_bridge_ready={hudhook_cpu_texture_bridge_ready}')
print(f'METRIC offline_face_texture_runtime_drawn={offline_face_texture_runtime_drawn}')
print(f'METRIC stylized_facedata_pixel_oracle={stylized_facedata_pixel_oracle}')
print(f'METRIC real_portrait_asset_candidates={real_portrait_asset_candidates}')
print(f'METRIC profile_placeholder_assets_only={profile_placeholder_assets_only}')
PY
