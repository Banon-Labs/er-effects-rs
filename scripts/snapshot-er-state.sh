#!/usr/bin/env bash
# Fingerprint every candidate store where Elden Ring might persist "privacy policy accepted" /
# profile state, so a before/after diff (across a vanilla accept-the-policy boot) reveals WHERE the
# acceptance lands. Read-only. Usage: snapshot-er-state.sh <out-manifest-path>
set -uo pipefail
OUT="${1:?usage: snapshot-er-state.sh <out-manifest-path>}"
PFX="$HOME/.local/share/Steam/steamapps/compatdata/1245620/pfx"
APPDATA="$PFX/drive_c/users/steamuser/AppData/Roaming/EldenRing"
STEAM_USERDATA="$HOME/.local/share/Steam/userdata"
{
  echo "# ER state snapshot"
  echo "## appdata EldenRing tree (path | size | mtime | sha256)"
  if [[ -d "$APPDATA" ]]; then
    find "$APPDATA" -type f 2>/dev/null | sort | while IFS= read -r f; do
      printf '%s | %s | %s | %s\n' "${f#"$APPDATA/"}" "$(stat -c '%s' "$f")" "$(stat -c '%Y' "$f")" "$(sha256sum "$f" | cut -d' ' -f1)"
    done
  else
    echo "(appdata dir absent)"
  fi
  echo "## wine registry sha (system.reg user.reg userdef.reg)"
  for r in system.reg user.reg userdef.reg; do
    [[ -f "$PFX/$r" ]] && printf '%s | %s | sha=%s\n' "$r" "$(stat -c '%s' "$PFX/$r")" "$(sha256sum "$PFX/$r" | cut -d' ' -f1)"
  done
  echo "## wine registry lines matching EldenRing/FromSoftware/BANDAI/Eula/Policy/Tos/Agree/Privacy"
  for r in system.reg user.reg userdef.reg; do
    [[ -f "$PFX/$r" ]] && grep -in -E 'eldenring|fromsoftware|bandai|eula|privacy|policy|\btos\b|agree|accept' "$PFX/$r" 2>/dev/null | sed "s|^|$r:|"
  done
  echo "## steam userdata appid-1245620 files (path | size | mtime)"
  if [[ -d "$STEAM_USERDATA" ]]; then
    find "$STEAM_USERDATA" -path '*1245620*' -type f 2>/dev/null | sort | while IFS= read -r f; do
      printf '%s | %s | %s\n' "${f#"$STEAM_USERDATA/"}" "$(stat -c '%s' "$f")" "$(stat -c '%Y' "$f")"
    done
  fi
} > "$OUT" 2>&1
echo "wrote $OUT ($(wc -l < "$OUT") lines)"
