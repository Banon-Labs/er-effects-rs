#!/bin/bash
# Ground-truth deobf VAs for the now-loading portrait-cover bind path.
cd /home/banon/projects/er-effects-rs
echo "=== CreateTpfResCap (reverse deobf 0x140b83680) ==="
python3 scripts/dump-deobf-shift.py --reverse 0x140b83680 2>&1 | tail -5
echo "=== FUN_140b83ec0 (reverse deobf 0x140b83dd0) ==="
python3 scripts/dump-deobf-shift.py --reverse 0x140b83dd0 2>&1 | tail -5
echo "=== Update (dump 0x1402a2c40) ==="
python3 scripts/dump-deobf-shift.py 0x1402a2c40 2>&1 | tail -5
echo "=== ctor (dump 0x1402a20e0) ==="
python3 scripts/dump-deobf-shift.py 0x1402a20e0 2>&1 | tail -5
echo "=== setter caller FUN_1407661e0 (dump) ==="
python3 scripts/dump-deobf-shift.py 0x1407661e0 2>&1 | tail -5
echo "=== producer parent FUN_140d6b020 (dump) ==="
python3 scripts/dump-deobf-shift.py 0x140d6b020 2>&1 | tail -5
echo "=== screen factory FUN_140764170 (dump) ==="
python3 scripts/dump-deobf-shift.py 0x140764170 2>&1 | tail -5
