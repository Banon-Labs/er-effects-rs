#!/usr/bin/env python3
"""Store a `bd remember` memory whose body lives in a file rather than on the command line.

`bd remember` takes the insight as a positional argument, which means a multi-paragraph memory has
to arrive through a shell substitution -- and a long `$(cat ...)` is exactly the shape of command
the workspace guard refuses to reason about, so agents end up truncating the memory instead. This
passes the file's bytes straight to the binary as one argv entry.

    python3 scripts/bd-remember-file.py <key> <path-to-body>

The binary is `$HOME/.local/bin/bd` (never the bare `bd`, which is an interactive shell guard
function); override with BD_BIN.
"""

import os
import subprocess
import sys


def main(argv):
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    key, path = argv[1], argv[2]
    with open(path, encoding="utf-8") as handle:
        body = handle.read().strip()
    if not body:
        print(f"{path} is empty; refusing to store an empty memory", file=sys.stderr)
        return 1
    binary = os.environ.get("BD_BIN", os.path.expanduser("~/.local/bin/bd"))
    # 30s is the workspace cap for any agent-run subprocess; a `bd remember` that has not
    # answered by then is a hung database, not a slow write.
    return subprocess.run(
        [binary, "remember", body, "--key", key], check=False, timeout=30
    ).returncode


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
