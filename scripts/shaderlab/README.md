# shaderlab -- Elden Ring shader extraction tooling

Extracts ER shader containers from the encrypted `Data*.bhd/bdt` archives and classifies
the bytecode (DXIL/SM6 vs DXBC/SM5). See `docs/shaderlab/er-shader-pipeline.md` for the
reverse-engineering write-up.

## Dependencies (host)

- .NET SDK (10.x) at `~/.dotnet`
- Smithbox binary install at `~/.local/share/smithbox/app` (provides `Andre.*` DLLs,
  `oo2core_6_win64.dll`, and `Assets/UXM Dictionaries/EldenRingDictionary.txt`)
- `wine` (Oodle/DCX-KRAK decompression needs the Windows `oo2core` DLL)

## `extract/` -- the extractor

Mounts the archives, RSA-decrypts the BHD5 (managed), reads the BDT, DCX-decompresses, and
parses the inner BND4. Two modes:

```bash
# Survey every shader container present in the archives (container-level summary):
bash scripts/shaderlab/extract/run-win.sh --shaders

# Dump one container's members with a DXIL-vs-DXBC verdict per member:
bash scripts/shaderlab/extract/run-win.sh "/shader/gxflvershader.shaderbnd.dcx"
```

`run-win.sh` runs a **win-x64** build under wine so `oo2core_6_win64.dll` loads (Kraken).
First build it:

```bash
bash scripts/shaderlab/extract/publish-win.sh
```

`run.sh` runs the same code as a **Linux-native** build (faster, no wine) but fails at the
Oodle step -- usable only for the BHD5/hash/header layers, not for reading compressed members.

Override the game dir with `ER_GAME_DIR=...`.

## `probe/` -- disposable reflection probes

Scratch scripts used to discover the `Andre.IO` / `Andre.Formats` / `SoulsFormats` API
surface (VFS, `ArchiveKeys`, `BHD5`, `BhdDictionary`, `Oodle` P/Invoke). Kept for reference;
edit `probe/Program.cs` and `bash probe/run.sh` to introspect more types.
