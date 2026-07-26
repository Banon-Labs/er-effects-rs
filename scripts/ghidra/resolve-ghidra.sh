#!/usr/bin/env bash
# Resolve the newest locally installed Ghidra. Source this file from Ghidra helper scripts.
# Explicit overrides still win:
#   GHIDRA_HEADLESS=/path/to/analyzeHeadless
#   GHIDRA_INSTALL_DIR=/path/to/ghidra_X_PUBLIC
#   ER_GHIDRA_INSTALL=/path/to/ghidra_X_PUBLIC  (legacy alias)

resolve_ghidra_install_dir() {
  if [[ -n "${GHIDRA_INSTALL_DIR:-}" ]]; then
    printf '%s\n' "$GHIDRA_INSTALL_DIR"
    return 0
  fi
  if [[ -n "${ER_GHIDRA_INSTALL:-}" ]]; then
    printf '%s\n' "$ER_GHIDRA_INSTALL"
    return 0
  fi

  local candidates=()
  local cand
  shopt -s nullglob
  for cand in \
    "$HOME"/tools/ghidra_*_PUBLIC \
    /opt/ghidra_*_PUBLIC \
    /mnt/d/ghidra/ghidra_*_PUBLIC; do
    if [[ -x "$cand/support/analyzeHeadless" ]]; then
      candidates+=("$cand")
    fi
  done
  shopt -u nullglob

  if [[ ${#candidates[@]} -eq 0 ]]; then
    return 1
  fi
  local versioned=()
  for cand in "${candidates[@]}"; do
    versioned+=("$(resolve_ghidra_version "$cand")	$cand")
  done
  printf '%b\n' "${versioned[@]}" | sort -V -k1,1 | tail -n 1 | cut -f2-
}

resolve_ghidra_headless() {
  if [[ -n "${GHIDRA_HEADLESS:-}" ]]; then
    printf '%s\n' "$GHIDRA_HEADLESS"
    return 0
  fi
  local install_dir
  install_dir="$(resolve_ghidra_install_dir)" || return 1
  printf '%s/support/analyzeHeadless\n' "$install_dir"
}

resolve_ghidra_version() {
  local install_dir="${1:-}"
  local base
  base="$(basename "$install_dir")"
  base="${base#ghidra_}"
  printf '%s\n' "${base%_PUBLIC}"
}

resolve_ghidra_version_at_least() {
  local install_dir="$1"
  local min_version="$2"
  local version
  version="$(resolve_ghidra_version "$install_dir")"
  [[ "$(printf '%s\n%s\n' "$min_version" "$version" | sort -V | tail -n 1)" == "$version" ]]
}
