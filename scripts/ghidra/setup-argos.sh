#!/usr/bin/env bash
# Set up the OFFLINE neural MT engine (argostranslate + ja->en model) for autotranslate-jp.py.
# Uses uv to create a local venv (no sudo, no system pip). One-time network download of the
# model (~hundreds of MB); fully offline afterward. Run autotranslate with --engine argos using
# this venv's python, e.g.:
#   /home/banon/ghidra_maporch/argos-venv/bin/python scripts/ghidra/autotranslate-jp.py --engine argos
set -euo pipefail

VENV=/home/banon/ghidra_maporch/argos-venv
UV=/home/banon/.local/bin/uv

"$UV" venv "$VENV"
"$UV" pip install --python "$VENV/bin/python" argostranslate

# Download + install the Japanese->English model into the venv's argos package store.
"$VENV/bin/python" - <<'PY'
import argostranslate.package as pkg, argostranslate.translate as tr
pkg.update_package_index()
avail = pkg.get_available_packages()
ja_en = next(p for p in avail if p.from_code == "ja" and p.to_code == "en")
path = ja_en.download()
pkg.install_from_path(path)
print("installed ja->en; test:", tr.translate("遺灰", "ja", "en"))
PY
echo "argos ready at $VENV"
