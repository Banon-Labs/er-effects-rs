export const meta = {
  name: 're-worldres-loadstate-request-skip',
  description: 'RE why block 0x1c000000 load-state is never created on the 2nd load (load never kicked off) - the resident/reuse discriminator',
  phases: [{ title: 'RE', detail: 'two agents: load-state creation + resident-skip' }],
}

const TOOLS = [
  'TOOLING (er-effects-rs, /home/choza/projects/er-effects-rs):',
  '- PRIMARY try Ghidra (real symbols/types): scripts/ghidra-query.sh <PostScript>.java [args] on /home/banon/ghidra_maporch/proj program ermaporch (~5s). Reusable Java postScripts in scripts/ghidra/ (DecompAddr.java, DecompAt.java, XrefsToAddr.java, StructAt.java, FindFieldWrite.java, FindFieldAccess.java). Batch all decompiles into ONE script. If it errors with a LOCK or permission problem, try Ghidra MCP tools (ghidra_decompile_function_by_address, xrefs).',
  '- FALLBACK (if Ghidra unavailable): offline disassembly of the deobf binary. Use the project Pi tool er_disasm (er_disasm kind=deobf va=0x140XXXXXX nbytes=0x200) or scripts/disas-deobf.sh --color=never 0x140XXXXXX 0x200. Addresses below are DEOBF/runtime VAs unless marked dump.',
  '- Do NOT use bash grep/cat/ls (rtk redaction); use the Read tool and python3 -c regex.',
  '- Your final message is raw structured data to the orchestrator; cite VAs + decompile/disasm snippets.',
].join('\n')

const FACTS = [
  'RUNTIME-CONFIRMED FACTS (angrE, area 0x1c legacy; autoload-then-reload-same-save; 2026-07-17):',
  '- Load1 (fresh boot autoload) and Load2 (in-game System-Quit reload of the SAME save) run the world-res PopulateLists block-res builder with IDENTICAL input (count=111, SAME list ptr, area 0x1c present both). So the per-block WorldBlockRes (+0xce0) is built the same on both loads. POPULATE IS NOT THE DIVERGENCE.',
  '- Yet at the Load2 stall (STEP_WorldResWait / MoveMapStep child step 3): block 0x1c000000 EXISTS in the WorldInfo area-block list (worldres+0xb3030; oracle blk_found=1) but the blocks LOAD-STATE is NULL: reading the block entry vtable slot +0x10 getter (block->vtable[0x10](block)) returns 0 (oracle blk_ls=0). Per the check FUN_14066d4d0: if(loadstate==0) return 0 -> it bails forever. On Load1 the same getter returns non-null and the world loads.',
  '- CONCLUSION: on the 2nd load, block 0x1c000000 load is NEVER KICKED OFF -- its load-state object is never created -- even though the block object and its block-res exist. This matches the user report that a subsequent load REUSES the world for the same character and skips re-loading the block. Loading a DIFFERENT character first then this one also stalls, so it is a general resident/skip, not same-char specific.',
  '',
  'DEOBF/runtime addresses (dump->deobf shift already applied):',
  '- FUN_14066d4d0 -> deobf 0x66d3e0 : WorldResWait CHECK; iterates worldres+0xb3030 (count +0xb3140), matches areaId (BlockId byte3), calls block->vtable[0x10] load-state getter, requires loadstate!=0 && +0x2d!=0 && +0x35==0x0a(10).',
  '- FUN_14066d8d0 -> deobf 0x66d7e0 : the ISSUER; sets load-state +0x2c=1 to REQUEST a block load. Routes overworld (areaId 0x32..0x58) to the +0xb3148 grid path, else the +0xb3030 non-overworld path.',
  '- FUN_14066da30 -> deobf 0x66d940 : pushes the dest BlockId into worldres+0xb798.',
  '- FUN_140624cb0 -> deobf 0x624bd0 : the STEP_WorldResWait step body that calls FUN_14066d4d0.',
  '- Block load-state fields: +0x2c request flag, +0x2d ready, +0x35 phase(==10 loaded). Block entry vtable slot +0x10 = the load-state getter.',
].join('\n')

const SCHEMA = {
  type: 'object',
  properties: {
    summary: { type: 'string' },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: { claim: { type: 'string' }, evidence: { type: 'string' }, confidence: { type: 'string', enum: ['high', 'medium', 'low'] } },
        required: ['claim', 'evidence', 'confidence'],
      },
    },
    instrument_points: { type: 'string', description: 'exact fn VA + arg layout + flag/offset to log so a runtime run confirms the skip on load 2' },
    fix_direction: { type: 'string' },
  },
  required: ['summary', 'findings', 'instrument_points', 'fix_direction'],
}

const Q1 = [
  'TASK: Find what CREATES a block load-state (the object the block entry vtable+0x10 getter returns), and why it is skipped for block 0x1c000000 on the 2nd load.',
  'Decompile the block entry vtable+0x10 load-state GETTER for a worldres+0xb3030 block entry (find the vtable, slot 0x10, decompile the getter): where does the returned load-state pointer come from -- a field on the block object? Which offset?',
  'Then find who WRITES that field (allocates/creates the load-state and stores it). Decompile that creator. Under what condition does the creator run for a block?',
  'Decompile FUN_14066d8d0 (deobf 0x66d7e0, the issuer): does it CREATE the load-state (alloc + store) or only set +0x2c on an EXISTING one? If it only sets +0x2c, the load-state must be created earlier -- by whom? Is there an early-out that skips a block whose load-state already exists OR whose block is flagged resident?',
  'KEY QUESTION: on a reload where block 0x1c000000 object already exists (resident from load 1) but its load-state was torn down to null, which step is supposed to RE-CREATE the load-state, and what condition makes it skip when the block is considered already-resident?',
  'Deliver: the load-state field offset on the block, the creator function VA, the condition under which creation is skipped (the resident/already-loaded discriminator), and an instrument point.',
].join('\n')

const Q2 = [
  'TASK: Find the RESIDENT discriminator that makes the 2nd load treat block 0x1c000000 as already-loaded and skip requesting its load.',
  'Decompile FUN_14066da30 (deobf 0x66d940, pushes dest BlockId into worldres+0xb798): what is its input source for the dest BlockId, and is there a membership/dedup check that SKIPS pushing a block already present in some resident list (b798, +0xb3030, +0xb3148 grid, or a resident set)? On a reload the block may already be in that list from load 1.',
  'Decompile FUN_140624cb0 (deobf 0x624bd0) and how it drives FUN_14066d4d0 (0x66d3e0) and FUN_14066d8d0 (0x66d7e0): trace which blocks get their load REQUESTED each pass. Is the request gated by a per-block resident/loaded flag or by set membership that persists across the in-game reload?',
  'Find any world/session TEARDOWN on the in-game System-Quit reload that clears the block LOAD-STATE (so the getter returns null) but does NOT clear the resident flag/list -- that inconsistency (state torn down, resident flag kept) would make the reload think the block is loaded and never re-request it. Where is the resident flag SET (load 1), CHECKED (load 2 skip), and is it CLEARED by the reload teardown?',
  'Deliver: the resident flag/list (address or struct offset), where set/checked/cleared, whether the reload teardown clears the load-state but not the resident marker, and an instrument point (fn VA + the flag offset) to confirm at runtime.',
].join('\n')

phase('RE')
const r = await parallel([
  () => agent(TOOLS + '\n\n' + FACTS + '\n\n' + Q1, { label: 'ghidra:loadstate-create', phase: 'RE', schema: SCHEMA }),
  () => agent(TOOLS + '\n\n' + FACTS + '\n\n' + Q2, { label: 'ghidra:resident-skip', phase: 'RE', schema: SCHEMA }),
])
return { loadstate: r[0], resident: r[1] }
