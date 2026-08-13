#!/usr/bin/env bash
# Measure whether the Windows-target DLL build is byte-reproducible, and whether a
# PURE CODE MOVE (a function relocated verbatim into a submodule, zero behaviour
# change) leaves the DLL bytes untouched.
#
# This exists to answer, with evidence rather than intuition, whether a CI gate of
# the form "refactor branches must produce byte-identical DLLs" is implementable.
#
# Three builds of crates/er-crash-logging-dll (the smallest cdylib shell):
#   A  clean + build, source unchanged            -- baseline
#   B  clean + build, source unchanged            -- reproducible run-to-run?
#   C  clean + build, after a pure code move      -- does a no-op refactor move bytes?
#
# Artifacts (DLL copies + a byte-diff report) go to $OUT_DIR, default /tmp.
# The source tree is restored and the package rebuilt before exit.
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
out="${OUT_DIR:-/tmp/er-dll-determinism}"
mkdir -p "$out"
pkg=er-crash-logging-dll
target=x86_64-pc-windows-msvc
dll="$root/target/$target/release/er_crash_logging_dll.dll"
src="$root/crates/$pkg/src/lib.rs"
moved_src="$root/crates/$pkg/src/dllmain.rs"
cd "$root"

build() {
	cargo clean -p "$pkg" --release --target "$target"
	cargo xwin build --release -p "$pkg" --target "$target" >/dev/null 2>&1
	cp -f "$dll" "$out/$1.dll"
	echo "[$1] $(sha256sum "$out/$1.dll" | cut -c1-16)  $(stat -c%s "$out/$1.dll") bytes"
}

restore() {
	[ -f "$out/lib.rs.orig" ] && cp -f "$out/lib.rs.orig" "$src"
	rm -f "$moved_src"
	cargo clean -p "$pkg" --release --target "$target" >/dev/null 2>&1 || true
	cargo xwin build --release -p "$pkg" --target "$target" >/dev/null 2>&1 || true
	echo "[restore] source restored, package rebuilt"
}
trap restore EXIT

cp -f "$src" "$out/lib.rs.orig"
build A
build B

# --- the pure move: DllMain and its statics relocated verbatim into a submodule ---
python3 - "$src" "$moved_src" <<'PY'
import sys
lib, moved = sys.argv[1], sys.argv[2]
s = open(lib).read()
start = s.index('#[unsafe(no_mangle)]')
end = s.index('#[cfg(not(windows))]')
body = s[start:end]
head, tail = s[:start], s[end:]
open(moved, 'w').write(
    'use std::sync::Once;\n\n'
    'const DLL_PROCESS_ATTACH: u32 = 1;\n'
    'const DLL_MAIN_SUCCESS: i32 = 1;\n\n'
    'static START: Once = Once::new();\n\n' + body)
for gone in ('use std::sync::Once;\n\n',
             'const DLL_PROCESS_ATTACH: u32 = 1;\n',
             'const DLL_MAIN_SUCCESS: i32 = 1;\n',
             'static START: Once = Once::new();\n'):
    head = head.replace(gone, '')
head += 'mod dllmain;\n\nconst DLL_MAIN_SUCCESS: i32 = 1;\n\n'
open(lib, 'w').write(head + tail)
PY
build C

python3 - "$out" <<'PY'
import sys, struct, os
out = sys.argv[1]
def read(n): return open(os.path.join(out, n + '.dll'), 'rb').read()
A, B, C = read('A'), read('B'), read('C')

def pe_fields(d):
    e = struct.unpack_from('<I', d, 0x3c)[0]
    return {'coff_timestamp': (e + 8, 4), 'checksum': (e + 0x58, 4)}

def diff(x, y, label):
    if len(x) != len(y):
        print(f'{label}: SIZE DIFFERS {len(x)} vs {len(y)}')
    n = min(len(x), len(y))
    off = [i for i in range(n) if x[i] != y[i]]
    fields = pe_fields(x)
    named = {}
    for name, (o, sz) in fields.items():
        hit = [i for i in off if o <= i < o + sz]
        if hit:
            named[name] = len(hit)
    other = [i for i in off if not any(o <= i < o + sz for o, sz in fields.values())]
    print(f'{label}: {len(off)} differing bytes  '
          f'(known PE header fields: {named or "none"};  everything else: {len(other)})')
    if other:
        runs, s = [], other[0]
        for a, b in zip(other, other[1:] + [None]):
            if b is None or b != a + 1:
                runs.append((s, a)); s = b
        print(f'   {len(runs)} run(s), first 6: '
              + ', '.join(f'0x{a:x}-0x{b:x}({b - a + 1})' for a, b in runs[:6]))
    return len(other)

print()
same_ab = diff(A, B, 'A vs B (identical source, rebuilt)')
same_ac = diff(A, C, 'A vs C (PURE CODE MOVE)')
print()
print('reproducible run-to-run outside PE timestamp/checksum:', same_ab == 0)
print('pure code move leaves bytes untouched:', same_ac == 0)
PY
