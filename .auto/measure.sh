#!/usr/bin/env bash
set -u -o pipefail

ROOT="$(pwd)"
ACTIVE_SAVE="${ER_EFFECTS_GOLD_SAVE:-$HOME/.local/share/Steam/steamapps/compatdata/1245620/pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing/76561197986456766/ER0000.sl2}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$ROOT/target/runtime-probe/samechar-3x-$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$ARTIFACT_DIR"

echo "artifact_dir=$ARTIFACT_DIR"
echo "objective=same-character load1+reload+reload, zero simulated menu/controller input for loading"
echo "boot_save=$ACTIVE_SAVE slot=${ER_EFFECTS_GOLD_SLOT:-0}"

build_rc=0
cargo xwin build --release --target x86_64-pc-windows-msvc \
  -p er-effects-rs -p er-reload-trace-dll -p er-input-harness-dll -p er-telemetry-dll \
  >"$ARTIFACT_DIR/build.log" 2>&1 || build_rc=$?
if (( build_rc != 0 )); then
  tail -80 "$ARTIFACT_DIR/build.log" >&2 || true
  python3 - "$ARTIFACT_DIR" <<'PY'
import sys
print('METRIC failure_score=1600')
print('METRIC false_positives=0')
print('ASI artifact_dir=' + sys.argv[1])
print('ASI failure=build_failed')
PY
  exit 1
fi

run_rc=0
CAP_SECONDS="${CAP_SECONDS:-240}" \
BOOT_FILE="$ACTIVE_SAVE" \
BOOT_SLOT="${ER_EFFECTS_GOLD_SLOT:-0}" \
DRIVE_RELOAD_SLOTS="${DRIVE_RELOAD_SLOTS:-0,0}" \
PROVE_MOVEMENT="${PROVE_MOVEMENT:-1}" \
STEADY_WINDOW_SECONDS="${STEADY_WINDOW_SECONDS:-3}" \
ARTIFACT_DIR="$ARTIFACT_DIR" \
scripts/run-samechar-3x-threedll.sh >"$ARTIFACT_DIR/runner.out" 2>"$ARTIFACT_DIR/runner.err" || run_rc=$?

python3 - "$ARTIFACT_DIR" "$run_rc" <<'PY'
from __future__ import annotations
import json, math, re, statistics, sys
from pathlib import Path
artifact = Path(sys.argv[1])
run_rc = int(sys.argv[2])
rows = []
for p in [artifact/'telemetry-timeseries.jsonl', artifact/'er-telemetry-timeseries.jsonl']:
    if p.exists():
        for line in p.read_text(encoding='utf-8', errors='replace').splitlines():
            try: rows.append(json.loads(line))
            except Exception: pass
        break
final = {}
for p in [artifact/'er-effects-telemetry.json', artifact/'telemetry.json']:
    if p.exists():
        try: final = json.loads(p.read_text(encoding='utf-8', errors='replace'))
        except Exception: final = {}
        break
report = (artifact/'samechar-3x-report.md').read_text(encoding='utf-8', errors='replace') if (artifact/'samechar-3x-report.md').exists() else ''

def as_int(v, d=0):
    try:
        if isinstance(v, bool): return int(v)
        return int(v)
    except Exception:
        return d

def as_float(v, d=0.0):
    try: return float(v)
    except Exception: return d

def as_bool(v):
    return bool(v) and v not in (0, '0', 'false', 'False', None)

epochs: dict[int, list[dict]] = {}
for r in rows:
    ep = as_int(r.get('system_quit_continue_confirm_fresh_deser_count'), 0)
    epochs.setdefault(ep, []).append(r)

score = 0
reasons: list[str] = []
metrics = {}
if run_rc != 0:
    score += 120; reasons.append(f'runner_rc={run_rc}')
if not rows:
    score += 900; reasons.append('missing_timeseries')
if '## Verdict: PASS' not in report:
    score += 100; reasons.append('report_not_pass')

sim = as_int(final.get('simulated_button_presses_total', 0), 0)
msg = max(as_int(final.get('oracle_msgbox_total_builds', 0), 0), as_int(final.get('oracle_msgbox_any_seen', 0), 0))
metrics['simulated_button_presses_total'] = sim
metrics['messagebox_builds_or_seen'] = msg
if sim != 0:
    score += 400; reasons.append(f'simulated_button_presses_total={sim}')
if msg != 0:
    score += 350; reasons.append(f'messagebox={msg}')

required = [0,1,2]
names = []
move_means = []
for ep in required:
    samples = epochs.get(ep, [])
    metrics[f'epoch{ep}_samples'] = len(samples)
    if not samples:
        score += 300; reasons.append(f'missing_epoch_{ep}'); continue
    ep_names = sorted({str(s.get('oracle_char_name')).strip() for s in samples if s.get('oracle_char_name')})
    names.append(ep_names[-1] if ep_names else '')
    canmove = any(as_bool(s.get('oracle_can_move')) for s in samples)
    didmove = max([as_int(s.get('oracle_did_move_frames'), 0) for s in samples] or [0])
    metrics[f'epoch{ep}_did_move_frames'] = didmove
    metrics[f'epoch{ep}_can_move'] = int(canmove)
    if not canmove or didmove < 60:
        score += 150; reasons.append(f'epoch{ep}_movement_unproven(can_move={canmove},did={didmove})')
    fps = [as_float(s.get('oracle_fps')) for s in samples if as_bool(s.get('oracle_can_move')) and as_float(s.get('oracle_fps')) > 0]
    if len(fps) < 4:
        score += 90; reasons.append(f'epoch{ep}_playable_fps_missing(n={len(fps)})')
        metrics[f'epoch{ep}_fps_mean'] = 0
        continue
    mean = statistics.mean(fps)
    move_means.append(mean)
    metrics[f'epoch{ep}_fps_mean'] = round(mean, 2)
    metrics[f'epoch{ep}_fps_min'] = round(min(fps), 2)
    metrics[f'epoch{ep}_fps_n'] = len(fps)
if len(set(n for n in names if n)) > 1 or any(not n for n in names[:len(required)]):
    score += 300; reasons.append(f'char_mismatch_or_missing={names}')
if len(move_means) == 3:
    mn, mx = min(move_means), max(move_means)
    ratio = mn / mx if mx > 0 else 0
    metrics['fps_parity_ratio'] = round(ratio, 4)
    if ratio < 0.80:
        score += int((0.80 - ratio) * 600) + 100
        reasons.append(f'fps_unstable_ratio={ratio:.3f}')
else:
    metrics['fps_parity_ratio'] = 0

score = min(1600, score)
metrics['false_positives'] = 0
print(f'METRIC failure_score={score}')
for k,v in metrics.items():
    print(f'METRIC {k}={v}')
print('ASI artifact_dir=' + str(artifact.resolve()))
print('ASI run_rc=' + str(run_rc))
print('ASI reasons=' + ('; '.join(reasons) if reasons else 'none'))
print('ASI report_verdict=' + (re.search(r'## Verdict: (.*)', report).group(1) if re.search(r'## Verdict: (.*)', report) else 'missing'))
print('ASI epoch_names=' + ','.join(names))
# Exit nonzero unless perfect. run_experiment/log_experiment status still decides keep/discard/crash.
sys.exit(0 if score == 0 else 1)
PY
score_rc=$?

tail -40 "$ARTIFACT_DIR/runner.out" 2>/dev/null || true
exit "$score_rc"
