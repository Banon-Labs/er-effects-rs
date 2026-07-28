export const meta = {
  name: 're-worldres-populate-reuse-divergence',
  description: 'RE why the subsequent load does not create the destination block-res while the first load does',
  phases: [{ title: 'RE', detail: 'two Ghidra agents' }],
}

const GHIDRA = [
  'GHIDRA ACCESS (er-effects-rs, /home/choza/projects/er-effects-rs):',
  '- scripts/ghidra-query.sh <PostScript>.java [args] runs analyzeHeadless on /home/banon/ghidra_maporch/proj program ermaporch (1.16.1 runtime dump, real symbols/types), about 5s. Reusable Java postScripts go in scripts/ghidra/ (examples: DecompAddr.java, DecompAt.java, XrefsToAddr.java, StructAt.java, FindFieldWrite.java, FindFieldAccess.java). Batch all decompiles for one question into ONE script.',
  '- Dump is for SEMANTICS (names, decompiled C, struct layouts). Decompile by dump VA directly.',
  '- If the project is LOCKED (user GUI open): try Ghidra MCP tools (ghidra_decompile_function_by_address, xrefs) before offline disasm; use scripts/disas-deobf.sh / er_disasm only as last resort.',
  '- Bounded-query hygiene: single known program, no whole-repo enumeration.',
  '- Tooling: do NOT use bash grep/cat/ls (rtk redaction); use the Read tool and python3 -c regex.',
  '- Your final message is raw structured data to the orchestrator; cite dump VAs and decompile snippets.',
].join('\n')

const EVIDENCE = [
  'RUNTIME EVIDENCE (angrE, area 0x1c legacy; autoload-then-reload-same-save repro; DLL 9d03a59a 2026-07-17):',
  '- FIRST load (boot autoload): enters world fine; creates the per-block world-res load-state for block 0x1c000000.',
  '- SECOND load (in-game System Quit Load-Profile reload of the SAME save): STALLS at STEP_WorldResWait (MoveMapStep child step 3). Oracle: blk_found=1 (area 0x1c IS in worldres+0xb3030 area list), blk_ls=0 (per-block load-state getter FUN_14062f550 returns NULL: no +0xce0 entry with mapId2==0x1c000000), req248=2, ig_d8=1.',
  '- The SECOND load STEP_MoveMap_Init DID run _Common_Initialize (0x140aed910): at MMS-INIT the InGameStep+0x238 loadlistlistFileCap is NON-null and InGameStep+0x210 worldloadlistlistVirtualPath = map:/WorldMsbList.worldloadlistlist (the correct master list). FieldArea current block (FieldArea+0x2c) = 0xffffffff UNSET at that instant.',
  '- So _Common_Initialize to ProcessMsbLoadLists (0x14066b2c0) is NOT skipped on the 2nd load, yet the 0x1c000000 block-res is still not created. Re-invoking ProcessMsbLoadLists after the fact does NOT help.',
  '- USER DIRECTIVE: the fix must make the loading SEQUENCE identical for every load (no same-character special case, no reuse the world). The bug generalizes: loading a different character first then this one also stalls. Same char twice is just the easy repro.',
  '',
  'KEY: getter FUN_14062f550 (0x14062f550) searches WorldAreaRes+0xce0 (count +0xcd8, stride 0xb98) for worldBlockInfo mapId2 (entry+0x34)==requested BlockId. +0xce0 entries built by PopulateLists (0x14066aee0) then ResetAreaResLists (0x14066e9b0). Normal legacy order: (1) _Common_Initialize to ProcessMsbLoadLists to Initialize (0x14066b030) to PopulateLists CREATES the 0x1c block-res; (2) FUN_14066da30 (0x14066da30) pushes dest BlockId into b798; (3) STEP_WorldResWait polls to LOADED.',
].join('\n')

const SCHEMA = {
  type: 'object',
  properties: {
    summary: { type: 'string' },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          claim: { type: 'string' },
          evidence: { type: 'string' },
          confidence: { type: 'string', enum: ['high', 'medium', 'low'] },
        },
        required: ['claim', 'evidence', 'confidence'],
      },
    },
    hook_points: { type: 'string' },
    fix_direction: { type: 'string' },
  },
  required: ['summary', 'findings', 'hook_points', 'fix_direction'],
}

const Q1 = [
  'TASK: Decompile the world-res POPULATE and block-res CREATION path and find the world-reuse / skip-if-already-populated condition that makes the SECOND load NOT create the 0x1c000000 block-res.',
  'Decompile (dump VAs): ProcessMsbLoadLists 0x14066b2c0; Initialize 0x14066b030; PopulateLists 0x14066aee0; ResetAreaResLists 0x14066e9b0; getter FUN_14062f550 0x14062f550.',
  'Answer:',
  '1. Inside PopulateLists / Initialize / ProcessMsbLoadLists: any EARLY-RETURN or GUARD that skips (re)building the +0xce0 per-block res when the area/world is ALREADY populated or resident (a reuse fast path)? A count!=0 check, an already-initialized bool, an area-already-loaded membership test? Show the condition + dump VA. Prime suspect for second load reuses the world and skips populate.',
  '2. What EXACTLY creates a +0xce0 block-res entry (WorldBlockRes with mapId2 at +0x34)? The function + the loop appending to WorldAreaRes+0xce0 (count +0xcd8, stride 0xb98). Is creation driven by loadlist contents, by the current-block request, or by an area-membership iteration? If it depends on FieldArea current block (+0x2c) which was UNSET at STEP_MoveMap_Init, could an unset current-block omit the dest block?',
  '3. Does ResetAreaResLists CLEAR +0xce0 and rely on a later populate to rebuild? If the reload clears +0xce0 but the rebuild is skipped by the reuse guard from Q1, that is the bug.',
  '4. Exact function + dump VA to HOOK to log, per load, every mapId2 that gets a +0xce0 block-res created, and where to read the reuse-guard boolean.',
  'Deliver the reuse/skip condition (or its absence), the block-res creation site, and hook points.',
].join('\n')

const Q2 = [
  'TASK: Decompile the SUBSEQUENT-load TEARDOWN and the destination-block REQUEST push to find what the reload does differently from a fresh boot so the dest block-res is never created.',
  'Decompile (dump VAs): FUN_14066da30 0x14066da30 (pushes dest BlockId into b798); STEP_MoveMap_Init 0x140aec210 and _Common_Initialize 0x140aed910 (full sequence + any branch that differs when a world already exists); WorldResWait chain FUN_140624cb0 / FUN_14066d4d0 / FUN_14066d8d0 (how the dest block load-state is requested; FUN_14066d8d0 sets loadstate+0x2c=1). Also find the world/session TEARDOWN on an in-game reload (the partial quit-to-menu side effect): what clears the resident world-res and whether it runs on the subsequent load.',
  'Answer:',
  '1. Does FUN_14066da30 (push dest BlockId into b798) run on the SECOND load? Input source for the dest BlockId -- FieldArea current block (+0x2c, UNSET 0xffffffff at STEP_MoveMap_Init)? If current block is unset when the push happens, the dest block may never be requested so its res is never created.',
  '2. In STEP_MoveMap_Init / _Common_Initialize: a branch taken ONLY when a prior world is still resident (reload) vs fresh boot (no world)? skip teardown, skip populate, reuse existing FieldArea/WorldInfoOwner? Show the branch + dump VA. This is the reuse-the-world path the user wants removed.',
  '3. FUN_14066d8d0 (load-state issuer): for the dest block, +0xb3030 non-overworld get-or-create path or +0xb3148 grid path? For a NULL +0xce0 res, does it CREATE one or bail? (getter FUN_14066d4d0 bails if loadstate==0.)',
  '4. Sequence diff: enumerate what the FIRST (boot, no resident world) load does that the SECOND (resident world) load skips, at teardown to populate to push-dest-block. Which SKIPPED step is responsible for the missing 0x1c000000 block-res?',
  'Deliver the divergent branch + the skipped step + hook points to confirm at runtime.',
].join('\n')

phase('RE')
const results = await parallel([
  () => agent(GHIDRA + '\n\n' + EVIDENCE + '\n\n' + Q1, { label: 'ghidra:populate-creator', phase: 'RE', schema: SCHEMA }),
  () => agent(GHIDRA + '\n\n' + EVIDENCE + '\n\n' + Q2, { label: 'ghidra:teardown-push', phase: 'RE', schema: SCHEMA }),
])
return { populate: results[0], teardown: results[1] }
