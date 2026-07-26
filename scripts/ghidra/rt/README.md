# scripts/ghidra/rt — in-repo Ghidra query toolkit (clean OSGi bundle)

Version-controlled Ghidra `analyzeHeadless` postScripts for querying the persistent ER 1.16.1
runtime-dump project, run via `scripts/ghidra/query.sh`:

```bash
bash scripts/ghidra/query.sh scripts/ghidra/rt/RtDecomp.java 0x140aec810   # decompile a dump VA
bash scripts/ghidra/query.sh scripts/ghidra/rt/RtSymAddr.java STEP_MoveMap_Update  # resolve symbols
bash scripts/ghidra/query.sh scripts/ghidra/rt/RtSyms.java movemapstep     # keyword symbol search
```

## Why this dir exists (the OSGi-clean contract)

Ghidra 12.1 compiles **every** `.java` in a `-scriptPath` directory as one OSGi bundle. A single
sibling that fails to compile poisons the whole bundle ("class could not be found"), so the passed
script silently fails. The repo's top-level `scripts/ghidra/` has ~80 historical postScripts and at
least one that does not compile under 12.1, so it cannot be used as a `-scriptPath`.

**This directory is kept deliberately clean: every `.java` here MUST compile under Ghidra 12.1.**
`query.sh` adds the passed script's own directory to `-scriptPath`, so pointing it at a script in
here compiles only this dir's files. If you add a helper, verify it compiles by running any script
from here (a compile failure in a sibling will surface as "class could not be found" for the one you
ran). This satisfies the standing rule that everything we run is tracked in the repo — no out-of-tree
`.java`.

## Notable scripts

- `RtDecomp.java <va>...` — decompile the function containing each dump VA.
- `RtByName.java <name>...` — decompile global functions by exact name.
- `RtSymAddr.java <name>...` — resolve symbol names to addresses (handles namespaced symbols).
- `RtSyms.java <kw>...` — list symbols whose name contains any keyword.
- `RtStep7.java [op addr [count]]` — step-table / xref / pointer-table digging (InGameStep steppers).
- `DumpExecImage.java <out> [base]` — export the dump's exec image to a flat RVA-aligned file for
  `scripts/dump-deobf-shift.py` (writes `dump-exec.bin`, which is gitignored — game-derived).
