#!/usr/bin/env python3
"""Generate a small, *readable* `.beads/PRIME.md` for `bd prime`.

Why this exists, and why the obvious fix is not enough
------------------------------------------------------
`bd prime` inlines every persistent memory body by default: 4.6 MB / ~650k tokens
at 2454 memories. The first fix here replaced that with a titles-only index, which
got it to 157 KB -- still far past what Claude Code will inline. The harness wrote
the whole thing to `tool-results/hook-<uuid>-stdout.txt` and showed the agent a
2 KB preview, so the priming content was effectively lost while still costing a
large chunk of every session. It also fires on PreCompact, i.e. exactly when
context is scarcest.

Titles-only was the wrong axis. The memory index is not a document to be read; it
is a search corpus, and `bd` already ships the search (`bd memories <keyword>`,
`bd recall <key>`). So this generator emits only what an agent cannot get on
demand:

  * the newest memories (recency -- no bd query orders by date),
  * a term histogram over the keys (which words are worth searching, with counts),
  * the top of the ready queue,
  * and the path to the full titles list, written beside PRIME.md.

Nothing rule-like is dropped: `bd prime`'s entire non-memory output is a 367-byte
header (measured -- `bd prime --export --full` carries no command reference in
this build), and every project rule lives in AGENTS.md, which CLAUDE.md imports.

The byte budget is enforced, not hoped for: `fit()` shrinks the optional sections
until the output fits BEADS_PRIME_MAX_BYTES, and `scripts/test-beads-prime-size.py`
drives this generator against a synthetic 6000-memory store in `scripts/check.sh`
so the regression cannot come back quietly.

Env knobs (all optional):
  BEADS_PRIME_MAX_BYTES   hard output cap (default 8192)
  BEADS_PRIME_RECENT      newest-memory lines (default 30)
  BEADS_PRIME_TERMS       search terms in the topic index (default 60)
  BEADS_PRIME_READY       ready-queue rows (default 8)
  BD_REAL_BIN             bd binary to use (also how the test injects a stub)
"""
import glob
import json
import os
import re
import subprocess
import sys
from collections import Counter

# bd's own header opens with a truncation disclaimer telling the reader to go find a
# persisted copy of this output. Once the output fits inline there is no persisted copy,
# so the line sends agents hunting for a file that does not exist. Dropped below.
TRUNCATION_DISCLAIMER = "[bd prime] If this output is truncated"

MEMORY_MARKERS = ("## Persistent Memories", "## Memories")
DATED_KEY = re.compile(r"\d{4}-\d{2}-\d{2}$")
TOKEN_SPLIT = re.compile(r"[^a-z0-9]+")

# Words that appear in keys but are useless as `bd memories <term>` queries: they match
# a hundred unrelated memories and narrow nothing.
TERM_STOPWORDS = frozenset("""
a an and are as at be been but by for from has have in into is it its must need needs
never no not of on only or our out over plus real should so than that the then there
these this to too was were what when which while why with works your yes
also actual again both does done down each else even ever full high just keep left less
like made make many more most much must next once same still such take them they very
want well were will your
""".split())


def resolve_bd():
    """Mirror beads-prime.sh's resolve_bd: env override, current user, any user, system."""
    candidates = [
        os.environ.get("BD_REAL_BIN", ""),
        os.path.join(os.path.expanduser("~"), ".local/bin/bd"),
        *sorted(glob.glob("/home/*/.local/bin/bd")),
        "/usr/local/bin/bd",
    ]
    for c in candidates:
        if c and os.access(c, os.X_OK):
            return c
    sys.exit("gen-beads-prime: no bd binary found (set BD_REAL_BIN to override)")


BD = resolve_bd()
MAX_BYTES = int(os.environ.get("BEADS_PRIME_MAX_BYTES") or 8192)
N_RECENT = int(os.environ.get("BEADS_PRIME_RECENT") or 30)
N_TERMS = int(os.environ.get("BEADS_PRIME_TERMS") or 60)
N_READY = int(os.environ.get("BEADS_PRIME_READY") or 8)


def run(args):
    """Best-effort bd call. A bd failure must degrade the index, never break the hook."""
    try:
        return subprocess.run(
            [BD, *args], capture_output=True, text=True, timeout=25
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return ""


def load_json(args):
    raw = run(args)
    try:
        return json.loads(raw)
    except (json.JSONDecodeError, TypeError):
        return None


def base_context():
    """bd's default prime text, minus the inlined-memory section and the stale disclaimer."""
    text = run(["prime", "--export"])
    for marker in MEMORY_MARKERS:
        i = text.find(marker)
        if i != -1:
            text = text[:i]
            break
    lines = [ln for ln in text.splitlines() if not ln.startswith(TRUNCATION_DISCLAIMER)]
    return "\n".join(lines).strip()


def memory_keys():
    data = load_json(["memories", "--json"])
    if not isinstance(data, dict):
        return []
    return list(data)


def newest(keys, n):
    """Most-recent memories, ordered by the trailing YYYY-mm-DD their keys carry.

    bd exposes no created/updated timestamp on a memory (`bd recall --json` returns
    key/value/found only), so the date in the key is the only recency signal there is.
    Undated keys simply do not appear in this slice; they are still in the index file
    and still findable with `bd memories`.
    """
    dated = [k for k in keys if DATED_KEY.search(k)]
    dated.sort(key=lambda k: (k[-10:], k), reverse=True)
    return dated[:n]


def terms(keys, n):
    """Search terms ranked by how many memory keys contain them, with counts.

    This is the part that replaces the flat 2452-line dump: it tells an agent which
    words actually pay off as `bd memories <term>` queries, in ~700 bytes.
    """
    counter = Counter()
    for key in keys:
        counter.update({
            t for t in TOKEN_SPLIT.split(key.lower())
            if len(t) >= 4 and not t.isdigit() and t not in TERM_STOPWORDS
        })
    return counter.most_common(n)


def ready_rows(n):
    data = load_json(["ready", "--json"])
    if not isinstance(data, list):
        return 0, []
    rows = []
    for issue in data[:n]:
        if not isinstance(issue, dict):
            continue
        ident = issue.get("id", "?")
        prio = issue.get("priority")
        prio = f"P{prio} " if isinstance(prio, int) else ""
        title = str(issue.get("title", "")).strip()
        if len(title) > 92:
            title = title[:89] + "..."
        rows.append(f"- {ident} {prio}{title}")
    return len(data), rows


def render(base, keys, recent, term_list, ready_total, ready, index_path):
    out = [base, ""]
    out.append(f"## Memories ({len(keys)}) -- search them, do not list them")
    out.append("")
    out.append("Bodies are NOT inlined here: at this store size that is 4.6 MB, which the")
    out.append("harness truncates to a 2 KB preview, so nothing would reach you at all.")
    out.append("")
    out.append("- `bd memories <keyword>`  search keys + bodies (start here)")
    out.append("- `bd recall <key>`        print one memory in full")
    out.append("- `bd remember --key <k> \"...\"`  write one (never MEMORY.md files)")
    if index_path:
        out.append(f"- every title, one per line: `{index_path}`")
    out.append("")
    out.append("Invoke the real binary: `$HOME/.local/bin/bd` (bare `bd` is an interactive-")
    out.append("shell guard function and fails in agent shells). Project rules live in")
    out.append("AGENTS.md, which CLAUDE.md imports -- they are not repeated here.")

    if term_list:
        out += ["", "### Worth searching (term -> memories whose key contains it)", ""]
        out.append(", ".join(f"{t} {c}" for t, c in term_list))

    if recent:
        out += ["", f"### Newest {len(recent)} memories", ""]
        out += [f"- {k}" for k in recent]

    if ready:
        out += ["", f"### Ready queue (top {len(ready)} of {ready_total}) -- live: `bd ready`", ""]
        out += ready

    out.append("")
    return "\n".join(out)


def fit(base, keys, index_path):
    """Render inside MAX_BYTES by shrinking the optional sections, never by truncating.

    A mid-line cut would hand the agent half a memory key, which is worse than an
    honestly shorter list: a truncated key is a `bd recall` that fails.
    """
    n_recent, n_terms, n_ready = N_RECENT, N_TERMS, N_READY
    ready_total, ready = ready_rows(n_ready)
    while True:
        text = render(base, keys, newest(keys, n_recent), terms(keys, n_terms),
                      ready_total, ready[:n_ready], index_path)
        if len(text.encode("utf-8")) <= MAX_BYTES:
            return text
        if n_recent > 5:
            n_recent = max(5, n_recent - 5)
        elif n_terms > 10:
            n_terms = max(10, n_terms - 10)
        elif n_ready > 0:
            n_ready -= 1
        else:
            # Nothing optional left; the base header alone is already over budget.
            return text


def write_index(path, keys):
    """Persist the full titles list beside PRIME.md, so nothing is actually lost."""
    if not path:
        return None
    try:
        tmp = path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as fh:
            fh.write("\n".join(keys) + ("\n" if keys else ""))
        os.replace(tmp, path)
    except OSError:
        return None
    return path


def main():
    index_arg = ""
    argv = sys.argv[1:]
    if argv and argv[0] == "--index" and len(argv) > 1:
        index_arg = argv[1]

    keys = memory_keys()
    written = write_index(index_arg, keys)
    sys.stdout.write(fit(base_context(), keys, written))


if __name__ == "__main__":
    main()
