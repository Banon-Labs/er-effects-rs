#!/usr/bin/env python3
"""Gate: the SessionStart / PreCompact prime hook must stay small enough to be seen.

Claude Code inlines a hook's stdout only up to a few KB. Past that it writes the whole
thing to `tool-results/hook-<uuid>-stdout.txt` and shows a 2 KB preview -- so an
oversized prime hook is worse than no prime hook: the content does not reach the agent
and it costs a large chunk of every session, PreCompact included. That is exactly what
happened at 2452 memories: 157.4 KB, persisted, unread.

The size therefore has to be a gate, not a habit. This drives the real
scripts/gen-beads-prime.py against a synthetic store far larger than the live one and
asserts (a) the output fits the budget, (b) it still teaches memory discovery, (c) the
full title list is preserved on disk, and (d) a broken/absent bd degrades instead of
breaking the hook.

`bd` is injected via BD_REAL_BIN, so this runs anywhere -- no beads install, no db.
"""
import json
import os
import subprocess
import sys
import tempfile

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GEN = os.path.join(REPO, "scripts", "gen-beads-prime.py")

# Deliberately larger than the live store (2454 on 2026-08-24) so the gate stays
# meaningful as the store grows.
N_MEMORIES = 6000
N_READY = 250
BUDGET = 8192

STUB = r"""#!/usr/bin/env python3
import json, sys
args = sys.argv[1:]
if args[:1] == ["prime"]:
    sys.stdout.write(
        "[bd prime] If this output is truncated by your host, read the full persisted "
        "hook output before continuing.\n\n# Beads Workflow Context\n\n"
        "> **Context Recovery**: Run `bd prime` after compaction\n\n"
        "## Persistent Memories (%(n)d)\n\n" + "### k\nbody\n" * 50)
elif args[:1] == ["memories"]:
    body = "x" * 900
    out = {}
    for i in range(%(n)d):
        day = 1 + (i %% 28)
        out["topic%%d-autoload-native-reload-portrait-detail-%%d-2026-%%02d-%%02d"
            %% (i %% 40, i, 1 + (i %% 12), day)] = body
    json.dump(out, sys.stdout)
elif args[:1] == ["ready"]:
    json.dump([{"id": "er-quickload-%%04x" %% i, "priority": i %% 4,
                "title": "A deliberately long ready-queue title that would blow the "
                         "budget if every row were emitted, number %%d" %% i,
                "description": "d" * 2000} for i in range(%(r)d)], sys.stdout)
else:
    sys.exit(2)
""" % {"n": N_MEMORIES, "r": N_READY}

BROKEN_STUB = "#!/bin/sh\nexit 3\n"


def make_stub(directory, source):
    path = os.path.join(directory, "bd")
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(source)
    os.chmod(path, 0o755)
    return path


def run_gen(stub, index_path, env_extra=None):
    env = dict(os.environ, BD_REAL_BIN=stub)
    env.pop("BEADS_PRIME_MAX_BYTES", None)
    env.pop("BEADS_PRIME_RECENT", None)
    env.pop("BEADS_PRIME_TERMS", None)
    env.pop("BEADS_PRIME_READY", None)
    env.update(env_extra or {})
    return subprocess.run(
        [sys.executable, GEN, "--index", index_path],
        capture_output=True, text=True, timeout=25, env=env, check=False,
    )


def fail(msg):
    print(f"FAIL: {msg}", file=sys.stderr)
    sys.exit(1)


def main():
    with tempfile.TemporaryDirectory() as tmp:
        stub = make_stub(tmp, STUB)
        index = os.path.join(tmp, "PRIME-memory-index.txt")
        proc = run_gen(stub, index)
        if proc.returncode != 0:
            fail(f"generator exited {proc.returncode}: {proc.stderr[:400]}")

        size = len(proc.stdout.encode("utf-8"))
        if size > BUDGET:
            fail(f"prime output is {size}B with {N_MEMORIES} memories; budget is {BUDGET}B. "
                 "It will be persisted to a file and never read.")
        if size < 400:
            fail(f"prime output is only {size}B -- the index collapsed to nothing")

        # Discovery must survive the diet: an agent that cannot find a memory is no
        # better off than one whose index was truncated away.
        for needle in ("bd memories", "bd recall", "PRIME-memory-index.txt"):
            if needle not in proc.stdout:
                fail(f"prime output never mentions {needle!r}; memories became undiscoverable")

        # bd's truncation disclaimer must not survive: there is no persisted copy to read.
        if "If this output is truncated" in proc.stdout:
            fail("prime output still carries bd's truncation disclaimer")

        # Whole keys only. A mid-line cut yields a `bd recall` that cannot succeed.
        for line in proc.stdout.splitlines():
            if line.startswith("- topic") and not line.rstrip().endswith(tuple("0123456789")):
                fail(f"memory key looks truncated: {line!r}")

        if not os.path.exists(index):
            fail("full memory title index was not written")
        titles = [ln for ln in open(index, encoding="utf-8").read().splitlines() if ln]
        if len(titles) != N_MEMORIES:
            fail(f"index holds {len(titles)} titles, expected {N_MEMORIES}")

        # A ready queue of 250 must not drag the whole list in.
        if proc.stdout.count("er-quickload-") > 20:
            fail("the ready queue was inlined wholesale instead of topped")

        # A bd that fails outright must still produce a usable, small file: the hook is
        # best-effort, and a hard failure here would break every SessionStart.
        broken_dir = os.path.join(tmp, "broken")
        os.makedirs(broken_dir, exist_ok=True)
        broken = make_stub(broken_dir, BROKEN_STUB)
        proc2 = run_gen(broken, os.path.join(tmp, "index2.txt"))
        if proc2.returncode != 0:
            fail(f"generator must survive a failing bd, exited {proc2.returncode}")
        if len(proc2.stdout.encode("utf-8")) > BUDGET:
            fail("degraded output is over budget")

        # The cap is honoured when tightened, too -- proving fit() shrinks rather than
        # the default merely happening to be small.
        proc3 = run_gen(stub, os.path.join(tmp, "index3.txt"),
                        {"BEADS_PRIME_MAX_BYTES": "2000"})
        if len(proc3.stdout.encode("utf-8")) > 2000:
            fail(f"BEADS_PRIME_MAX_BYTES=2000 ignored: got "
                 f"{len(proc3.stdout.encode('utf-8'))}B")

    print(f"OK: prime output {size}B <= {BUDGET}B budget at {N_MEMORIES} memories; "
          f"{len(titles)} titles preserved in the index file")


if __name__ == "__main__":
    main()
