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

## 2. 13bm GhidraMCP (interactive RE tools, needs the Ghidra GUI)

Exposes ~70 RE tools (decompile, xrefs, struct get/edit, search, ...) to this MCP
client. Built **from source** so the auto-launched native bridge is ours:

- Source: `/home/banon/tools/GhidraMCP-13bm` (cloned). Rebuild both halves with
  `scripts/ghidra/build-ghidramcp.sh` (uses local gradle 8.14 + JDK 21, since the
  system JDK 26 is too new for gradle 8.14).
- Bridge binary: `/home/banon/tools/GhidraMCP-13bm/mcp-bridge/mcp_bridge`
- Extension: built ZIP installed into `<ghidra>/Ghidra/Extensions/GhidraMCP-13bm/`.
- MCP client config: `.mcp.json` (project scope) registers the `ghidra` server
  pointing at the bridge.

### Activation (the manual step)

The MCP only goes live when a **Ghidra GUI** has the plugin running:

1. Open a Ghidra GUI **CodeBrowser** on the target program. Use the gzf-derived
   `ermaporch` (semantics) and/or `erdeobf` (addresses) -- **never** headless-open
   the shared `From Software.rep` (locked; forbidden per AGENTS.md).
2. `File > Configure > GhidraMCP` -> enable **MCPServerPlugin**. It starts a TCP
   server on `localhost:8765` and auto-launches the bridge.
3. Restart this MCP client (or reload MCP) so it picks up `.mcp.json`.

Until step 2, the bridge stays up and retries; the `ghidra` tools simply report no
connection. Multi-instance: open a second program on another port (8766...) and pass
`target_port` to route calls -- e.g. one window on `ermaporch`, one on `erdeobf`.

> Caveat that does not change: the MCP queries whatever program is open. Addresses
> from the dump still carry the shift -- ground-truth anything you will call/patch
> against the deobf binary (`er_disasm` / `disas-deobf.sh`), exactly as before.
