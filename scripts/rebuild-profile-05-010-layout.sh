#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

hot_reload_only=false
if [[ ${1:-} == "--hot-reload" ]]; then
  hot_reload_only=true
  shift
fi

vanilla_gfx=${ER_PROFILE_05_010_VANILLA_GFX:-$HOME/er-extract/nuxe-menu-20260619-170932/menu/05_010_profileselect.gfx}
layout_file=${ER_PROFILE_05_010_LAYOUT:-$repo_root/crates/er-gfx/profile_05_010_layout.toml}
out_gfx=${ER_PROFILE_05_010_OUT_GFX:-$repo_root/target/pi-local/profile-05-010-manual-layout.gfx}
edit_table=$repo_root/crates/er-gfx/src/title_05_010_edits.rs
fingerprint_src=$repo_root/crates/er-gfx/src/title_05_010.rs

mkdir -p "$(dirname "$out_gfx")"

cargo run -q -p er-gfx --example make_05_010_stats -- "$vanilla_gfx" "$out_gfx" "$layout_file"
if [[ "$hot_reload_only" == true ]]; then
  python3 - "$out_gfx" "$layout_file" <<'PY'
from pathlib import Path
import sys
out = Path(sys.argv[1])
layout = Path(sys.argv[2])
b = out.read_bytes()
h = 0xcbf29ce484222325
for x in b:
    h ^= x
    h = (h * 0x100000001b3) & ((1 << 64) - 1)
print(f'edited_gfx={out}')
print(f'edited_len={len(b)}')
print(f'edited_fnv=0x{h:016x}')
print(f'layout_file={layout}')
print('hot_reload_only=true')
PY
  exit 0
fi
python3 scripts/gfx_tag_diff.py "$vanilla_gfx" "$out_gfx" --emit-rust TITLE_05_010_STATS_EDITS > "$edit_table"
python3 - "$out_gfx" "$fingerprint_src" "$layout_file" <<'PY'
from pathlib import Path
import re
import sys
out = Path(sys.argv[1])
src = Path(sys.argv[2])
layout = Path(sys.argv[3])

def read_layout_numbers(path):
    section = None
    values = {}
    for raw in path.read_text().splitlines():
        line = raw.split('#', 1)[0].strip()
        if not line:
            continue
        if line.startswith('[') and line.endswith(']'):
            section = line[1:-1]
            continue
        if '=' not in line or section != 'list':
            continue
        k, v = [p.strip() for p in line.split('=', 1)]
        if k in {'row_pitch', 'visible_rows', 'mask_height', 'scrollbar_x', 'scrollbar_top_y', 'scrollbar_track_height'}:
            values[k] = int(v)
    missing = {'row_pitch', 'visible_rows', 'mask_height', 'scrollbar_x', 'scrollbar_top_y', 'scrollbar_track_height'} - values.keys()
    if missing:
        raise SystemExit(f'{path}: missing [list] values: {sorted(missing)}')
    if values['visible_rows'] != 10:
        raise SystemExit(f'{path}: list.visible_rows must stay 10')
    return values

b = out.read_bytes()
h = 0xcbf29ce484222325
for x in b:
    h ^= x
    h = (h * 0x100000001b3) & ((1 << 64) - 1)
text = src.read_text()
nums = read_layout_numbers(layout)
text = re.sub(r'pub const COMPACT_ROW_PITCH_PX: i32 = -?\d+;', f"pub const COMPACT_ROW_PITCH_PX: i32 = {nums['row_pitch']};", text)
text = re.sub(r'pub const COMPACT_VISIBLE_ROW_COUNT: i32 = -?\d+;', f"pub const COMPACT_VISIBLE_ROW_COUNT: i32 = {nums['visible_rows']};", text)
text = re.sub(r'pub const COMPACT_LIST_HEIGHT_PX: i32 = [^;]+;', f"pub const COMPACT_LIST_HEIGHT_PX: i32 = {nums['mask_height']};", text)
text = re.sub(r'pub const COMPACT_SCROLLBAR_X_PX: i32 = -?\d+;', f"pub const COMPACT_SCROLLBAR_X_PX: i32 = {nums['scrollbar_x']};", text)
text = re.sub(r'pub const COMPACT_SCROLLBAR_TOP_Y_PX: i32 = [^;]+;', f"pub const COMPACT_SCROLLBAR_TOP_Y_PX: i32 = {nums['scrollbar_top_y']};", text)
text = re.sub(r'pub const COMPACT_SCROLLBAR_TRACK_HEIGHT_PX: i32 = [^;]+;', f"pub const COMPACT_SCROLLBAR_TRACK_HEIGHT_PX: i32 = {nums['scrollbar_track_height']};", text)
text = re.sub(r'pub const EDITED_LEN: usize = \d+;', f'pub const EDITED_LEN: usize = {len(b)};', text)
pretty = f'0x{h:016x}'
pretty = '0x' + '_'.join(pretty[2:][i:i+4] for i in range(0, 16, 4))
text = re.sub(r'pub const EDITED_FNV1A64: u64 = 0x[0-9a-fA-F_]+;', f'pub const EDITED_FNV1A64: u64 = {pretty};', text)
src.write_text(text)
print(f'edited_gfx={out}')
print(f'edited_len={len(b)}')
print(f'edited_fnv=0x{h:016x}')
PY

cargo fmt --all -- --check
cargo test -p er-gfx profile_05_010_layout -- --nocapture
cargo test -p er-gfx --test profile_stats stats_panel_output_scales_row_internal_chrome_to_compact_pitch -- --nocapture
cargo xwin build -p er-effects-rs --release --target x86_64-pc-windows-msvc
sha256sum target/x86_64-pc-windows-msvc/release/er_effects_rs.dll | awk '{print "dll_sha256=" $1}'
printf 'layout_file=%s\n' "$layout_file"
