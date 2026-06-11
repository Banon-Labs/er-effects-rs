# Ghidra recon data

This directory holds output from the `FastSpEffectRecon.java` Ghidra script
(lives outside the repo at `~/ghidra_scripts/FastSpEffectRecon.java` on the
analysis machine) and is the drop location for future recon runs.

## Workflow

1. Run `FastSpEffectRecon.java` in Ghidra against the Elden Ring binary. It
   writes `%LOCALAPPDATA%\Temp\ghidra-fast-speffect-recon.txt` (and mirrors
   every line to the Ghidra console).
2. Copy the report here as `ghidra-fast-speffect-recon.txt` (from WSL:
   `cp /mnt/c/Users/<user>/AppData/Local/Temp/ghidra-fast-speffect-recon.txt docs/recon/`).
3. Ingest it:

   ```bash
   # Summary + candidate SpEffect IDs extracted from matched symbols/strings
   cargo run -p er-param-inspect -- recon docs/recon/ghidra-fast-speffect-recon.txt

   # Additionally check each candidate against SpEffectParam in a regulation file
   cargo run -p er-param-inspect -- recon docs/recon/ghidra-fast-speffect-recon.txt <regulation.bin>
   ```

   Confirmed IDs worth keeping become entries in `data/effects.json` (which the
   DLL embeds and `er-param-inspect validate` checks).

The parser lives in `crates/soulsformats/src/recon.rs`
(`er_soulsformats::recon::parse_recon_report`).

## Report schema

Line-oriented text, three sections. Order is fixed; blank lines separate
sections.

```
program=<program name>
executablePath=<path to analyzed binary>
domainFile=<ghidra project path>
outputFile=<absolute path of this report>

symbol-matches
symbol name="<name>" type=<SymbolType> address=<addr> namespace="<namespace>"
  refFrom=<addr> type=<RefType>          # up to 8 per match
  refFrom=<truncated>                    # marker when more references exist
symbol-matches-truncated=true            # only when the 400-match cap was hit
symbol-scan-cancelled=<bool>
symbol-scan-count=<n>
symbol-match-count=<n>

defined-string-matches
string address=<addr> value="<escaped value>"
  refFrom=...                            # same reference format as above
string-matches-truncated=true            # only when the 200-match cap was hit
defined-string-scan-cancelled=<bool>
defined-data-scan-count=<n>
defined-string-match-count=<n>

done                                     # present only if the run completed
```

Notes:

- A symbol or string is matched when its name/value case-insensitively
  contains one of the script's terms: `speffect`, `sp_effect`,
  `specialeffect`, `special_effect`, `eye`, `phantom`, `200181`.
- String **values** are escaped (`\\`, `\"`, `\n`, `\r`); symbol names are
  emitted raw. The parser unescapes values and splits symbol lines from the
  right to tolerate exotic names.
- "Candidate SpEffect IDs" are distinct 4–9 digit decimal runs found in
  matched symbol names and string values.
- A report without the trailing `done` line means the Ghidra run was
  cancelled or is still in progress; the parser flags this via
  `ReconReport::complete`.
