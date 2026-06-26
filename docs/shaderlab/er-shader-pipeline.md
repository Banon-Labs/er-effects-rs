# Elden Ring Shader Pipeline -- Reverse-Engineering Notes

Verified findings from session 2026-06-25 (branch `research/er-shaders`). Everything
here was confirmed empirically by extracting and parsing real retail files, not assumed.

## 1. Where shaders live

ER ships shaders in the encrypted archives (`Data0-3.bhd/bdt`, `DLC.bhd/bdt`). The
dictionary (UXM `EldenRingDictionary.txt`) lists two trees:

- `/shader/...`        -- the path that ACTUALLY ships on PC retail (24 containers, all in `Data0`)
- `/shader_d3d12/...`  -- listed in the dictionary but **NOT present** in PC retail archives

Despite the `/shader/` name (legacy from the DX11/SM5 era of DS3/Sekiro), the game is
DX12 (`vkd3d-proton.cache` present) and the bytecode is Shader Model 6 (see SS4).

### Containers (all `*.dcx`, DCX-KRAK / Oodle-compressed, inner = BND4)

| Container | members | domain |
|---|---|---|
| `gxflvershader.shaderbnd.dcx` | 248 | FLVER material/mesh shaders |
| `gxffxshader.shaderbnd.dcx` | 488 | FFX (effects) |
| `gxposteffect.shaderbnd.dcx` | 193 | post-processing |
| `gxrenderershader.shaderbnd.dcx` | 264 | renderer core |
| `gxraytracing.shaderbnd.dcx` | 14 | DXR ray tracing (raygen/hit/miss + denoise) |
| `gxdecal` / `gxgui` / `gxshader` / `grass` | 52 / 17 / 26 / 10 | decals, UI, misc, grass |
| `speedtree*.shaderbdlebnd.dcx` | 111 | SpeedTree foliage |
| `shaderbdle*.shaderbdlebnd.dcx` | up to 750 | "shader bundles" (+ `_dlc01/02`, `_[rt]` RT variants) |
| `pipelinestatecache.dat.dcx` | -- | DX12 PSO cache (magic `PSC.`, not a BND) |

Member name suffixes: `.vpo` = vertex, `.fpo` = fragment/pixel, `.cpo` = compute / DXR lib.
Permutation tags: `_Dpt` (depth), `_Fwd` (forward), `_GBuf` (gbuffer), `_Velo` (velocity),
`_Skin` (skinned), `@[cl]` (a variant -- likely "clip"/cull or a feature toggle).

## 2. Unwrapping the archives (managed, no Windows native libs)

The BHD5 header is RSA-encrypted; file data in the BDT is plain (some entries AES, handled
by the reader). Smithbox's `Andre.SoulsFormats` does this with a Windows-native
`bhd5_decrypt_rust.dll`, but the same result is achievable fully managed:

1. PEM key per archive: `Andre.Formats.Util.ArchiveKeys.GetKey(bhdPath, Game.ER)` (internal;
   keys for `Data0-3`, `DLC`, `sd\sd`, `sd\sd_dlc02`).
2. `SoulsFormats.Util.CryptographyUtility.DecryptRsa(bhdPath, pem)` -> decrypted BHD5 stream
   (managed BouncyCastle).
3. `SoulsFormats.BHD5.Read(Memory<byte>, BHD5.Game.EldenRing)`.
4. Locate the file by path hash (see SS3), then `FileHeader.ReadFile(FileStream bdt)` -> bytes.
5. `DCX.Decompress` -> inner BND4 -> `BND4.Read` -> members.

### Oodle (DCX-KRAK)
The `.shaderbnd.dcx` payloads are Oodle Kraken. The decompressor (`oo2core_6_win64.dll`) is
Windows-only. We run the extractor as a **win-x64 build under wine**, where ER's own
`oo2core_6_win64.dll` loads natively (zero correctness risk vs. a 3rd-party Kraken decoder).
A pure-Linux path would need an open-source Kraken `.so` exporting `OodleLZ_Decompress`.

## 3. BHD5 path hash (Elden Ring)

64-bit, confirmed against `Andre.Core.Util.BhdDictionary.ComputeHash`:

```
norm = path, '\'->'/', leading '/', lowercased
hash = 0;  for each char c:  hash = hash * 0x85 (133) + c     // u64, wrapping
```

NOT the 32-bit `SFUtil.FromPathHash` (prime 37). Example:
`/shader/gxflvershader.shaderbnd.dcx` -> `0x317C271635120F6C`.

## 4. Bytecode format: DXIL / Shader Model 6

The 4-byte member magic is `DXBC` -- but that FourCC is the **DXContainer**, used for BOTH
SM5 DXBC and SM6 DXIL. The format is decided by the inner chunk FourCCs:

- SM6 / **DXIL**: `DXIL`, `ILDN`/`ILDB`, `RDAT`, `PSV0`, `ISG1`/`OSG1` (+`SFI0`,`STAT`,`HASH`)
- SM5 / DXBC:    `SHEX`/`SHDR`, `ISGN`/`OSGN`, `RDEF`

Measured chunks:
- `gxflvershader` `.vpo`: `SFI0,ISG1,OSG1,PSV0,RTS0,STAT,ILDN,HASH,DXIL` -> **DXIL/SM6**
- `gxraytracing` `.cpo`:  `SFI0,RDAT,STAT,ILDN,HASH,DXIL`                -> **DXIL/SM6** (DXR `lib_6_x`)

**Conclusion:** ER PC shaders are DXIL (Shader Model 6.x). The LLVM DXIL architecture doc
(<https://llvm.org/docs/DirectX/DXILArchitecture.html>) IS the right reference. The toolchain
is the SM6 stack -- `dxc` (DirectXShaderCompiler), `dxc -dumpbin`/disassembly, RGA -- not the
legacy `fxc`/DXBC tooling.

## 5. Roadmap

- [x] Locate + decrypt + decompress + enumerate shader containers
- [x] Identify bytecode format (DXIL/SM6)
- [x] **READ PATH VALIDATED** -- disassembled a real member end-to-end (see section 6)
- [ ] Understand the material->shader binding (MTD/matbin, gxflver param layout) -- the disasm
      already exposes named cbuffers (`cbMtdParam`, `cbMatDynParam`, `cbInstanceData`), so the
      param semantics are legible from reflection
- [ ] Edit: recompile HLSL->DXIL with dxc, repack into BND4+DCX-KRAK, reinsert into archive (or
      serve via mod loader / `mod/` override) and validate in-game. NOTE: edited DXIL must be
      re-validated/signed (HASH chunk via dxil.dll) or the driver rejects it; `dxc` signs
      automatically when compiling from HLSL
- [ ] (later) Bevy WGSL lab in `.worktrees/bevy-shader-tinkering` for shader experimentation

## 6. Read path -- validated 2026-06-25

Toolchain (Linux): prebuilt **dxc** from microsoft/DirectXShaderCompiler releases
(`linux_dxc_*.tar.gz`; installed by `er-shaderlab setup` to `~/tools/dxc`). Modern
`llvm-dis` (LLVM 22) CANNOT read DXIL bitcode (`error: i8 must be 8-bit aligned`) -- DXIL is
a custom LLVM-3.7 fork, so dxc's bundled disassembler is required.

Chain proven on `GXFlver_ColDifSpcBumpEmiIblGlow_DptA.ppo` (pixel shader):

```
Data0.bhd/bdt -> RSA-decrypt BHD5 -> read BDT -> DCX-KRAK (Oodle, under wine)
  -> BND4 -> member (DXContainer) -> dxc -dumpbin -> readable DXIL
```

`er-shaderlab disasm /shader/gxflvershader.shaderbnd.dcx GXFlver...DptA.ppo` yields: I/O
signatures; the original FromSoft debug path (`..\Dist\GR\WIN_D3D12_Pdb\flver\..._DptA.ppo.pdb`);
shader hash + PSV runtime info (`Pixel Shader`, SM6); **named cbuffers/fields** (`FC_GlowScale`,
`FC_EmissiveColor`, `FC_PhantomEdgeColor`, `FC_BloodAmount`, world matrices...) -- reflection
intact; and the DXIL IR: `target triple = "dxil-ms-dx"`, `define void @ps_main()` calling
`@dx.op.sample.f32` / `@dx.op.cbufferLoadLegacy` / `@dx.op.createHandle` / `@dx.op.discard`,
plus `!dx.shaderModel` / `!dx.entryPoints`.

`er_soulsformats::shaders::carve_dxil` also carves the raw LLVM bitcode out of the `DXIL`
chunk (DXContainer header -> part table -> DxilProgramHeader/DxilBitcodeHeader), in pure Rust.

## Tooling

- **`er-shaderlab`** (`tools/er-shaderlab`) -- the CLI: `doctor` / `setup` / `survey` /
  `extract <logical-path> <out-dir>` / `disasm <logical-path> <member-substr>`. Discovers
  Smithbox, the game, wine, dotnet and dxc (env overrides: `SMITHBOX_BINARY_DIR`,
  `ER_GAME_DIR`, `DXC_ROOT`).
- **`er_soulsformats::shaders`** (`crates/soulsformats/src/shaders.rs`) -- the library:
  config discovery, bridge build/run, DXContainer classify/carve, dxc disasm.
- **`er-shaderbridge`** (`crates/soulsformats/shaderbridge/`) -- the win-x64 .NET worker run
  under wine for the decrypt/decompress/unbind half (needs ER's `oo2core` Oodle DLL). Kept
  separate from the host-native `param-rows` bridge in `crates/soulsformats/bridge/`.
- **Both .NET versions:** both bridges target whichever framework the installed Andre stack
  is built for -- the TFM is auto-detected from `Andre.SoulsFormats.dll`'s
  `TargetFrameworkAttribute` (`detect_dotnet_tfm`), so net9 *and* net10 Smithbox installs
  work. The shader bridge is self-contained (bundles its runtime); the framework-dependent
  `param-rows` bridge adds `RollForward=Major` so it also runs on a newer-major runtime than
  it targets. (This Smithbox is net9.0.)
- E2E read-path test: `cargo test -p er-soulsformats --test shaders_e2e -- --ignored`
  (skips cleanly when the game/tools are absent).
