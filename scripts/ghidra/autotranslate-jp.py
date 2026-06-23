#!/usr/bin/env python3
"""Auto-translate extracted Japanese terms (jp-terms.json) into the regenerable auto dictionary
(jp-en-dict.auto.json). The hand-verified jp-en-dict.json overrides these in the bridge, so
re-running this never clobbers your refinements.

  scripts/ghidra/autotranslate-jp.py [--engine mymemory|argos] [--min-count N] [--max N] [--limit-only]

Engines:
  mymemory  free online API, no key (default). Polite delay; anonymous daily word cap applies.
  argos     fully offline neural MT (needs the argostranslate venv + ja_en model; see
            scripts/ghidra/setup-argos.sh). Use for offline refreshes.

Workflow: autotranslate fills the AUTO file; review it, and move corrected entries into
jp-en-dict.json (which wins). The bridge also flags any still-untranslated runs in tool output."""
import argparse, json, os, sys, time, urllib.parse, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
TERMS = os.path.join(HERE, "jp-terms.json")
AUTO  = os.path.join(HERE, "jp-en-dict.auto.json")
MANUAL = os.path.join(HERE, "jp-en-dict.json")

def load_json(path, default):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, json.JSONDecodeError):
        return default

def mymemory(term):
    q = urllib.parse.quote(term)
    url = f"https://api.mymemory.translated.net/get?q={q}&langpair=ja|en"
    with urllib.request.urlopen(url, timeout=20) as r:
        d = json.load(r)
    if d.get("responseStatus") == 200:
        t = d.get("responseData", {}).get("translatedText", "").strip()
        # MyMemory echoes the input back when it has no match; treat that as a miss.
        if t and t != term:
            return t
    return None

def argos_translator():
    import argostranslate.translate as t
    def tr(term):
        out = t.translate(term, "ja", "en")
        return out.strip() if out and out.strip() != term else None
    return tr

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--engine", choices=["mymemory", "argos"], default="mymemory")
    ap.add_argument("--min-count", type=int, default=1, help="only translate runs appearing >= N times")
    ap.add_argument("--max", type=int, default=0, help="cap number of NEW translations this run (0 = no cap)")
    ap.add_argument("--delay", type=float, default=0.3, help="seconds between online requests")
    args = ap.parse_args()

    terms = load_json(TERMS, [])
    auto = load_json(AUTO, {})
    manual = load_json(MANUAL, {})
    if not isinstance(auto, dict): auto = {}

    translate = argos_translator() if args.engine == "argos" else mymemory

    todo = [row["jp"] for row in terms
            if row.get("count", 0) >= args.min_count
            and row["jp"] not in auto and row["jp"] not in manual and row["jp"] != "_comment"]
    if args.max > 0:
        todo = todo[:args.max]
    print(f"{len(todo)} terms to translate via {args.engine} "
          f"(skipping {len(auto)} already auto, {len(manual)} manual)")

    done = 0
    for i, term in enumerate(todo, 1):
        try:
            en = translate(term)
        except Exception as e:
            print(f"  [{i}/{len(todo)}] {term}: ERROR {e}", file=sys.stderr)
            en = None
        if en:
            auto[term] = en
            done += 1
        if i % 25 == 0 or i == len(todo):
            print(f"  {i}/{len(todo)} done={done}", file=sys.stderr)
            with open(AUTO, "w", encoding="utf-8") as f:   # checkpoint
                json.dump(auto, f, ensure_ascii=False, indent=2, sort_keys=True)
        if args.engine == "mymemory" and i < len(todo):
            time.sleep(args.delay)

    with open(AUTO, "w", encoding="utf-8") as f:
        json.dump(auto, f, ensure_ascii=False, indent=2, sort_keys=True)
    print(f"wrote {len(auto)} auto entries -> {AUTO} (+{done} this run)")

if __name__ == "__main__":
    main()
