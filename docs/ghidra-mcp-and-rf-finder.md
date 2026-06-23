# Ghidra MCP + Random-Forest function finder

Two complementary additions for LLM-driven RE on the ER binaries. They cover
different needs and are independent of each other.

## 1. RF function-start finder (headless, no GUI, no MCP)

Drives Ghidra's official **MachineLearning** extension (Random Forest Function
Finder) from a headless GhidraScript and emits discovered candidate function-start
VAs as JSON. This is the path to "let the LLM use the ML extension" -- it needs no
GUI and no MCP, and fits the existing `ghidra-query.sh` headless pattern.

- Script: `scripts/ghidra/FindFunctionStartsRF.java` -- read-only; trains an RF on a
  program's already-known functions, classifies the undefined byte ranges, and
  prints `{program, image_base, best_model, count, candidates:[{va,score}]}`
  between `RF_RESULT_JSON_BEGIN`/`_END` markers. The mutating steps from Ghidra's
  example (`DisassembleCommand`/`CreateFunctionCmd`) are intentionally removed so it
  is safe under `-process -readOnly`.
- Wrapper: `scripts/ghidra/find-functions-rf.sh`
  ```bash
  # smoke target: the symbolized DUMP (low yield, addresses carry the ~0x10 shift)
  scripts/ghidra/find-functions-rf.sh --threshold 0.90 --max-starts 500
  # real target: the deobf-binary project (DEOBF-NATIVE VAs, no shift) -- see below
  scripts/ghidra/find-functions-rf.sh --proj-dir /home/banon/ghidra_maporch/proj-deobf \
      --proj-name erdeobf --threshold 0.85
  ```
- Install (one-time): the ML extension ships with Ghidra as a zip and is now
  extracted into `<ghidra>/Ghidra/Extensions/MachineLearning/` so `analyzeHeadless`
  loads it. (Re-extract from `<ghidra>/Extensions/Ghidra/*MachineLearning.zip` if a
  Ghidra upgrade clears it.)

### deobf-binary project (the address-bearing target)

The RF finder is only useful for *addresses you will call/patch* when run on a
program whose VAs match the deobf binary. The dump (`ermaporch`) is for SEMANTICS
and carries the ~0x10 dump-vs-deobf shift. Build a deobf-native project once:

```bash
scripts/ghidra/import-deobf.sh   # raw BinaryLoader, base 0x140000000, x86-64; SLOW (~94MB analysis)
```

Output project: `/home/banon/ghidra_maporch/proj-deobf` program `erdeobf`. Then run
the finder against it (command above). VAs are deobf-native -- ready for
`er_disasm` / `scripts/disas-deobf.sh` without shifting.

## 2. 13bm GhidraMCP, served PRE-WARMED and HEADLESS (no GUI, no Xvfb)

Exposes ~70 RE tools (decompile, xrefs, struct get/edit, search, ...) to this MCP
client. Built **from source** so the auto-launched native bridge is ours:

- Source: `/home/banon/tools/GhidraMCP-13bm` (cloned). Rebuild both halves with
  `scripts/ghidra/build-ghidramcp.sh` (uses local gradle 8.14 + JDK 21, since the
  system JDK 26 is too new for gradle 8.14).
- Bridge binary: `/home/banon/tools/GhidraMCP-13bm/mcp-bridge/mcp_bridge`
- Extension: built ZIP installed into `<ghidra>/Ghidra/Extensions/GhidraMCP-13bm/`.
- MCP client config: `.mcp.json` (project scope) registers the `ghidra` server
  pointing at the bridge.

### Local bridge patch: output formatting + JP->EN translate

The upstream Go bridge returned every tool result as `string(rawJSON)`, so multi-line
fields (decompiled C especially) came back as an escaped JSON string with literal
`\n` and no formatting. Our patch (`scripts/ghidra/ghidramcp-localfmt.patch`, applied
by `build-ghidramcp.sh`, source-of-truth `mcp-bridge/render.go`) replaces that single
choke point with `renderResult`, which:
- returns bare-string results (e.g. `decompile_function_by_address`) with **real
  newlines**, surfaces a text field (`decompiled`/`listing`/...) under a compact
  `// name=... address=...` header, renders disassembly as aligned listing lines, and
  pretty-prints everything else. This fixes the bug for all text tools at once.
- adds a **`translate`** boolean param to **every** tool. It defaults **ON whenever a
  dictionary is present** (toggle a single call off with `translate:false`; force the
  global default either way with `GHIDRA_TRANSLATE=off|on`). When on, output runs through
  a maintained JP->EN dictionary (`scripts/ghidra/jp-en-dict.json`, path overridable via
  `GHIDRA_JP_DICT`, wired in `.mcp.json`). Any **leftover Japanese is flagged** at the end of the output
  (`// [untranslated JP -- add to ...: Xie Wen Zi , Bai Zhao Huan , ...]`) so the dictionary is
  trivial to grow as new terms appear. Editing the JSON + restarting the bridge is
  enough; no rebuild needed.

Verified: `decompile_function_by_address` returns clean multi-line C (0 escaped `\n`);
translation runs by default (no param) and is skipped with `translate:false`; manual
entries override the auto ones (e.g. a bad `anime`->"Mechanical rape" auto match is
overridden to "animation").

#### Bootstrapping the dictionary (extract -> autotranslate -> refine)

Rather than hand-adding terms, the dictionary is bootstrapped from the program itself:

1. **Extract** -- `scripts/ghidra/extract-jp-terms.py` pages `list_strings` over the warm
   MCP daemon (no second Ghidra open), collects every bounded Japanese run, and writes
   `scripts/ghidra/jp-terms.json` (frequency-sorted; ~963 unique runs from 63,992 strings).
2. **Autotranslate** -- `scripts/ghidra/autotranslate-jp.py` translates each run into
   `scripts/ghidra/jp-en-dict.auto.json`. Default engine is **MyMemory** (free online, no
   key); `--engine argos` uses a fully **offline** neural model (set up once via
   `scripts/ghidra/setup-argos.sh`). It is additive/idempotent (skips terms already in
   either dict) and checkpoints, so it is safe to re-run/resume.
3. **Refine** -- the bridge loads BOTH files: `jp-en-dict.auto.json` (regenerable bulk)
   overlaid by `jp-en-dict.json` (hand-verified, **wins on conflict**). To correct a bad
   machine translation, add the right entry to `jp-en-dict.json`; re-running autotranslate
   never clobbers it. The "untranslated JP" flag in tool output surfaces anything still missing.

So the loop is: extract -> autotranslate (bulk first pass) -> use -> move corrections into the
verified dict as you notice them. Updating either JSON + restarting the bridge is enough.

### The key insight: the server does not need the GUI

13bm's `MCPServer`/`MCPContextProvider` are plain objects, not GUI plugins. The
context provider only dereferences the `PluginTool` in two GUI-cursor tools
(`get_current_address` / `get_current_function`); every other tool runs off the
loaded `Program`. So we run the server from a long-lived **headless** GhidraScript
(`scripts/ghidra/MCPServeHeadless.java`, `new MCPServer(null)`), which keeps one
program loaded and WARM in a single `analyzeHeadless` process. No GUI, no Xvfb, and
Ghidra is started ONCE -- not per operation.

### Pre-warm and reuse

```bash
scripts/ghidra/mcp-ghidra-daemon.sh start          # warms ermaporch (semantics) on :8765, read-only
scripts/ghidra/mcp-ghidra-daemon.sh status
scripts/ghidra/mcp-ghidra-daemon.sh stop
# second, address-accurate instance on another port (multi-instance: pass target_port to route):
scripts/ghidra/mcp-ghidra-daemon.sh start --proj-dir /home/banon/ghidra_maporch/proj-deobf \
    --proj-name erdeobf --port 8766
```

The daemon detaches (`setsid`) so the warm program survives across client/session
restarts; clean shutdown is a stop-file. Default is **writable** so the agent's
rename/struct/comment edits accumulate and **persist** into the project (saved on a
clean `stop`; a crash loses unsaved in-memory edits) -- pass `--readonly` to serve
query-only instead. The bridge (launched by `.mcp.json`) connects to the daemon's
port and reconnects automatically if it restarts. After starting the daemon, restart
this MCP client so it picks up `.mcp.json`.

Write path verified: `set_bookmark` -> `get_bookmarks` -> `remove_bookmark` round-trips
against the warm program.

Verified end-to-end (headless daemon -> bridge -> MCP): `initialize` ok, 70 tools
listed, `get_program_info` returns the warm program (`pc_eldenring_runtime.1.16.1`,
366744 functions).

> Caveat that does not change: the MCP queries the loaded program. Addresses from
> the dump (`ermaporch`) carry the ~0x10 shift -- ground-truth anything you will
> call/patch against the deobf binary (`er_disasm` / `disas-deobf.sh`, or the
> `erdeobf` instance), exactly as before. The two GUI-cursor tools
> (`get_current_address` / `get_current_function`) are unavailable headless; pass
> explicit addresses instead.
