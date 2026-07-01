#!/usr/bin/env python3
"""bake-corpus.py -- corpus-scale scorer for the ER function-bake pipeline.

Drives scripts/bake-function.py `bake()` over a deterministic stratified sample
of the Arxan thunk corpus (scripts/arxan-thunks.tsv) and reports the metric the
/goal autoresearch loop maximizes:

  PRIMARY   = # functions BAKED (ref==recompiled) AND non-vacuous
              (input_dependent OR proven-constant-with-real-callees)
  SECONDARY = # functions BAKED at all (incl. stub-only constant passes)

Also emits a per-kind breakdown and a failure-stage/reason histogram -- that
histogram is the DIRECTION signal (it says which collapse/bounding/callee work
would unlock the most functions next). Writes incremental JSONL + a summary JSON,
and (with --baseline) flags any regression (a VA that passed before and not now).

This scorer ONLY orchestrates bake(); it never touches the locked differential
verifier (wine_exit / the ref==recompiled comparison) inside bake-function.py.

Each function runs in an isolated work dir (bf.WORK) so a prior function's stale
intermediates cannot create a false-positive pass; the real wineprefix is reused.

Run under: uv run --with unicorn --with capstone python3 scripts/bake-corpus.py ...
"""
import argparse, importlib.util, json, os, random, re, shutil, sys, time
from collections import Counter, defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
TSV = os.path.join(HERE, "arxan-thunks.tsv")


def load_bake():
    spec = importlib.util.spec_from_file_location("bake_function", os.path.join(HERE, "bake-function.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def read_corpus():
    rows = []
    with open(TSV) as f:
        hdr = f.readline().rstrip("\n").split("\t")
        vi, ki = hdr.index("thunk_va"), hdr.index("kind")
        for line in f:
            c = line.rstrip("\n").split("\t")
            if len(c) > max(vi, ki):
                rows.append({"va": c[vi], "kind": c[ki]})
    return rows


def stratified(rows, n_per_kind, seed):
    by_kind = defaultdict(list)
    for r in rows:
        by_kind[r["kind"]].append(r)
    sample = []
    for kind in sorted(by_kind):
        pool = sorted(by_kind[kind], key=lambda r: r["va"])
        rng = random.Random(f"{seed}:{kind}")
        sample.extend(rng.sample(pool, min(n_per_kind, len(pool))))
    return sample


def reason_bucket(res):
    """Normalize a failure into a coarse bucket for the direction histogram."""
    if res.get("ok"):
        return "input_dependent" if res.get("input_dependent") else "constant_pass"
    stage = res.get("stage", "?")
    err = (res.get("err") or "")
    if stage == "reassemble":
        if "conditional branch into Arxan" in err:
            return "reassemble:cond-branch-into-arxan"
        if "call into Arxan" in err:
            return "reassemble:call-into-arxan"
        if "branch into Arxan" in err:
            return "reassemble:jmp-into-arxan-unresolved"
        if "stack gadgets" in err:
            return "reassemble:stack-gadget"
        if "no instructions recovered" in err:
            return "reassemble:recovery-derailed"
        if "undecodable" in err:
            return "reassemble:undecodable"
        return "reassemble:other"
    return f"{stage}"


def score(va, kind, bf, workbase, paths, budget):
    wd = os.path.join(workbase, va)
    shutil.rmtree(wd, ignore_errors=True)
    bf.WORK = wd
    t0 = time.time()
    try:
        res = bf.bake(int(va, 16), paths, budget)
    except Exception as e:
        res = {"ok": False, "stage": "exception", "err": repr(e)[:300]}
    dt = round(time.time() - t0, 1)
    shutil.rmtree(wd, ignore_errors=True)
    return {"va": va, "kind": kind, "ok": bool(res.get("ok")),
            "input_dependent": bool(res.get("input_dependent")),
            "stage": res.get("stage"), "ref": res.get("ref"), "recompiled": res.get("recompiled"),
            "collapsed": len(res.get("collapsed_gadgets") or {}),
            "ncallees": len(res.get("callees") or []),
            "bucket": reason_bucket(res),
            "err": (res.get("err") or "")[:200] if not res.get("ok") else None,
            "seconds": dt}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--sample", type=int, default=12, help="functions per kind (default 12)")
    ap.add_argument("--seed", default="1337")
    ap.add_argument("--kinds", default="stub-return,reloc-body,chain-deep")
    ap.add_argument("--paths", type=int, default=200)
    ap.add_argument("--budget", type=int, default=1500)
    ap.add_argument("--full", action="store_true", help="score the ENTIRE corpus (slow)")
    ap.add_argument("--out", default=os.path.expanduser("~/er-llvm-spike/corpus-score.json"))
    ap.add_argument("--workbase", default=os.path.expanduser("~/er-llvm-spike/corpus-work"))
    ap.add_argument("--baseline", help="prior summary JSON to diff against for regressions")
    args = ap.parse_args()

    bf = load_bake()
    rows = read_corpus()
    keep = set(args.kinds.split(","))
    rows = [r for r in rows if r["kind"] in keep]
    sample = rows if args.full else stratified(rows, args.sample, args.seed)

    jsonl = args.out + ".l"
    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    results = []
    with open(jsonl, "w") as jl:
        for i, r in enumerate(sample, 1):
            res = score(r["va"], r["kind"], bf, args.workbase, args.paths, args.budget)
            results.append(res)
            jl.write(json.dumps(res) + "\n"); jl.flush()
            tag = ("PASS*" if res["ok"] and res["input_dependent"] else
                   "pass " if res["ok"] else "FAIL ")
            print(f"[{i:>4}/{len(sample)}] {tag} {res['va']} {res['kind']:<11} "
                  f"{res['bucket']:<34} {res['seconds']}s", flush=True)

    primary = sum(1 for r in results if r["ok"] and r["input_dependent"])
    secondary = sum(1 for r in results if r["ok"])
    per_kind = {}
    for k in sorted(keep):
        ks = [r for r in results if r["kind"] == k]
        per_kind[k] = {"n": len(ks),
                       "baked": sum(1 for r in ks if r["ok"]),
                       "input_dependent": sum(1 for r in ks if r["ok"] and r["input_dependent"])}
    buckets = Counter(r["bucket"] for r in results)

    summary = {"sample": len(results), "seed": args.seed, "sample_per_kind": args.sample,
               "primary_input_dependent": primary, "secondary_baked": secondary,
               "per_kind": per_kind, "buckets": dict(buckets.most_common()),
               "ok_vas": sorted(r["va"] for r in results if r["ok"]),
               "results": results}

    # A "regression" must be DETERMINISTIC: was baked, now fails at a deterministic
    # stage (reassemble/compile/assemble/lift/...). run-ref failures are NOT counted --
    # they are flaky (the scalar harness feeds pointer-taking functions garbage, so the
    # reference exe crashes/hangs non-deterministically and flaps across the 90s wine cap;
    # observed: 140420240 took 110s->pass in baseline, 90s->timeout in the next run, same
    # code path). Those are reported separately as `flaky` so the regression guard the
    # /goal loop relies on stays trustworthy.
    regressions, flaky = [], []
    if args.baseline and os.path.exists(args.baseline):
        base = json.load(open(args.baseline))
        base_ok = set(base.get("ok_vas", []))
        now = {r["va"]: r for r in results}
        for va in sorted(base_ok & set(now) - set(summary["ok_vas"])):
            (flaky if now[va]["bucket"] == "run-ref" else regressions).append(va)
        summary["regressions"] = regressions
        summary["flaky"] = flaky

    json.dump(summary, open(args.out, "w"), indent=2)

    print("\n==== SUMMARY ====")
    print(f"sample={len(results)}  PRIMARY(input_dependent)={primary}  SECONDARY(baked)={secondary}")
    for k, v in per_kind.items():
        print(f"  {k:<12} n={v['n']:<4} baked={v['baked']:<4} input_dependent={v['input_dependent']}")
    print("buckets (direction signal):")
    for b, c in buckets.most_common():
        print(f"  {c:>4}  {b}")
    if regressions:
        print(f"\n!!! DETERMINISTIC REGRESSIONS ({len(regressions)}): {regressions}")
    if flaky:
        print(f"flaky run-ref flaps (not regressions): {len(flaky)}: {flaky}")
    print(f"\nwrote {args.out}")


if __name__ == "__main__":
    main()
