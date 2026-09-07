#!/usr/bin/env bash
# Store a bd memory whose body lives in a file.
#
# `bd remember` takes the insight as one shell argument, and a multi-paragraph
# reverse-engineering note is exactly the kind of text that does not survive
# being typed on a command line: backticks, `*(this+0x8b0)`, quotes and newlines
# all get mangled or eaten by the shell before bd sees them. Writing the note to
# a file and passing the path keeps the bytes intact.
#
#   scripts/bd-remember-from-file.sh <key> <path-to-note>
#
# The bd binary is invoked at its real path on purpose: the bare `bd` command is
# an interactive-shell guard function that agent shells do not get.
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 <memory-key> <file-with-the-note>" >&2
    exit 2
fi

key=$1
note_path=$2

if [[ ! -r "$note_path" ]]; then
    echo "$0: cannot read $note_path" >&2
    exit 1
fi

bd_bin=${BD_BIN:-"$HOME/.local/bin/bd"}
if [[ ! -x "$bd_bin" ]]; then
    echo "$0: no bd binary at $bd_bin (override with BD_BIN)" >&2
    exit 1
fi

note=$(cat -- "$note_path")
exec "$bd_bin" remember "$note" --key "$key"
