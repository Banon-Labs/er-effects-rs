#!/usr/bin/env python3
"""Dump the complete function list (entry, size, name) from a headless Ghidra MCP daemon.

The daemon caps getAllFunctions at 10,000 items per call, so this pages through
totalCount in chunks and checkpoints to disk so an interrupted run resumes.

  python3 scripts/dump-ghidra-function-list.py --port 8767 --out /tmp/.../funcs-1170.tsv
"""
import argparse, os, sys, time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "ghidra"))
from mcp_query import query  # noqa: E402

PAGE = 10000


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--host", default="localhost")
    a = ap.parse_args()

    part = a.out + ".part"
    done = 0
    if os.path.exists(part):
        with open(part, encoding="utf-8") as fh:
            done = sum(1 for _ in fh)
        # resume on a page boundary only; partial pages are discarded
        done -= done % PAGE
        rows = []
        with open(part, encoding="utf-8") as fh:
            for i, line in enumerate(fh):
                if i >= done:
                    break
                rows.append(line)
        with open(part, "w", encoding="utf-8") as fh:
            fh.writelines(rows)
        print(f"resuming at offset {done}", flush=True)

    total = None
    t0 = time.time()
    with open(part, "a", encoding="utf-8") as fh:
        off = done
        while total is None or off < total:
            for attempt in range(4):
                try:
                    r = query("getAllFunctions", {"limit": PAGE, "offset": off},
                              host=a.host, port=a.port, timeout=120)
                    break
                except Exception as exc:  # transient socket/daemon contention
                    if attempt == 3:
                        raise
                    # Retry immediately. The old backoff slept 2/5/8s, but a sleep here
                    # synchronised nothing: the daemon either answers this page or it does not,
                    # and its own 120s request timeout is what actually bounds a stalled call.
                    # Waiting only delayed the next identical attempt.
                    print(f"retry offset={off} attempt={attempt}: {exc}", flush=True)
            res = r.get("result")
            if res is None:
                print("ERROR: " + str(r), flush=True)
                return 2
            total = res["totalCount"]
            items = res["items"]
            if not items:
                break
            for it in items:
                fh.write("%s\t%d\t%s\n" % (it["entry_point"], it.get("size") or 0, it["name"]))
            fh.flush()
            off += len(items)
            print(f"{off}/{total}  {time.time()-t0:.0f}s", flush=True)
    os.replace(part, a.out)
    print(f"wrote {a.out} ({off} rows) in {time.time()-t0:.0f}s", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
