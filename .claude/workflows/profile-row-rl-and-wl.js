export const meta = {
  name: 'profile-row-rl-and-wl',
  description: 'Change the ProfileSelect row Level caption to RL, and add a WL (max weapon upgrade level) readout',
  phases: [
    { title: 'Ground', detail: 'caption override mechanism, max-weapon-level source, render surface' },
    { title: 'Verify', detail: 'adversarially refute each lane\u2019s load-bearing claim' },
    { title: 'Plan', detail: 'synthesize a single implementation plan' },
  ],
}

const REPO = '/home/banon/projects/er-effects-rs'

const HOUSE_RULES = `
You are investigating the repo at ${REPO} (branch research/quit-menu-load-ui-parity). READ-ONLY.
Do NOT edit files, do NOT build, do NOT commit, do NOT launch Elden Ring, do NOT run any runtime probe.
Produce findings only.

TOOLING RULES (violating these wastes your whole run):
- A Cupcake/OPA guard INTERCEPTS bare \`grep\`/\`ls\`/\`find\`/\`cat\` bash commands and denies them.
  Use the Read tool, and \`python3 -c\` one-liners for content search. Example:
  python3 -c "import re,glob; [print(f'{f}:{i}:',l.rstrip()) for f in glob.glob('crates/**/*.rs',recursive=True) for i,l in enumerate(open(f,encoding='utf-8',errors='replace'),1) if re.search(r'PATTERN',l)]"
- Do NOT use \`rtk grep\` \u2014 it REDACTS identifier tokens in both output and matching, so it returns
  false negatives (confirmed for tokens like 'online', 'continue', 'input', 'block'). Never treat an
  rtk zero-result as proof of absence.
- Every bash call is hard-capped at 30s. Keep commands short.

GHIDRA (the authoritative first pass for any Elden Ring RE question):
- A Ghidra MCP daemon on localhost:8765 serves the ELDEN RING 1.16.2 dump, which MATCHES the running game.
- Query it with: python3 scripts/ghidra/mcp_query.py <method> [args]   (methods are camelCase:
  getDecompiledCode, decompileFunctionByName, disassembleFunction, getFunctionByAddress,
  getXrefsTo, getXrefsFrom, searchFunctionsByName, getStructure)
- There is also a terser CLI at scripts/ghidra/q.py \u2014 read it first if you use it.
- CRITICAL: for 1.16.2 the dump VA == the deobf VA == the LIVE runtime VA. The shift is ZERO.
  Do NOT run scripts/dump-deobf-shift.py \u2014 its dump side is still 1.16.1, so it invents a nonzero
  shift and returns mid-instruction addresses.
- eldenring-deobf.bin is a FLAT image: file offset == RVA, VA = 0x140000000 + file_offset.

Report concrete file:line citations and concrete VAs. Distinguish sharply between what you VERIFIED
by reading code/disasm and what you are INFERRING. Say "unverified" when it is unverified.
`

const CONTEXT = `
BACKGROUND \u2014 the feature.
This mod DLL renders a custom ProfileSelect / save-picker UI on the 05_010_profileselect.gfx row
template (row template = sprite 76). Each row clip has these text fields, and the SAME clips are
recycled across three list kinds (character slots, file browse, drive strip):

  PlayerName, StaticText_110502 (the "Level" CAPTION), Level (the level VALUE),
  Location, PlayTime, ErStats, ErCharStats, DriveCell_0..2

Field geometry lives in crates/er-gfx/profile_05_010_layout.toml, which IS the shipped default
(Profile05_010Layout::default() parses it via include_str!). Current relevant boxes (x, width, px):
  StaticText_110502  x=-346  w=149  align=left    (renders "Level")
  Level              x=-300  w=52   align=right   (renders e.g. "125")
  ErCharStats        x=-230  w=484  align=center  (renders "VIG 50 MND 10 END 50 STR 21 ...")
  ErStats            x=-324  w=587  align=left    (BLANK on character rows; used by browse rows)

Known RE, recorded in crates/er-effects-rs/src/constants/stats_panel_text.rs:118-151 \u2014 READ THAT
COMMENT BLOCK FIRST, it is dense and load-bearing:
  * StaticText_110502 is ENGINE-populated. The named-child binder hands every child name to the FMG
    static-text pass FUN_14074c540, which matches the "StaticText_" prefix from the table at
    PTR_s_StaticText__142a94aa0, atoi()s the trailing id (110502), and calls the SetText CORE
    FUN_140d842a0 DIRECTLY. Row-populate never writes it, so it is written once per bind and
    survives row-clip reuse.
  * Level is the VALUE: FUN_140749ed0(&proxy->comp, *(u32*)(rowModel + 0x88)) \u2014 DLString::FormatW("%d").
  * The comment claims a post-populate re-resolve of a NATIVE named child is impossible, because the
    named-child ctor 0x14074a7c0 resolves out of the PARENT proxy's embedded value at +0x28, which
    the populate destroys at its end. Browse rows therefore HIDE the level fields rather than blank
    them, via the game's own visibility wrapper (TITLE_PRESS_START_SET_VISIBLE_RVA).

THE USER'S REQUEST (verbatim):
  "You are already showing per slot rune level, displayed as Level followed by the value, right?
   I'm just asking for Level to go to RL to make it smaller. I would LOVE to have WL also, which
   shows the maximum weapon level value on the character as a result of the extra space"

So: (1) caption text "Level" -> "RL"; (2) NEW readout "WL <n>" = the maximum weapon upgrade level
on that character, fitting in the horizontal space freed by the shorter caption.
`

const FINDINGS_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['summary', 'verified', 'unverified', 'recommendation', 'risks'],
  properties: {
    summary: { type: 'string', description: '2-5 sentence bottom line' },
    verified: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['claim', 'evidence'],
        properties: {
          claim: { type: 'string' },
          evidence: { type: 'string', description: 'file:line and/or VA + what the code literally does' },
        },
      },
    },
    unverified: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['claim', 'whatWouldProveIt'],
        properties: { claim: { type: 'string' }, whatWouldProveIt: { type: 'string' } },
      },
    },
    recommendation: { type: 'string', description: 'concrete recommended approach with file:line touch points' },
    risks: { type: 'array', items: { type: 'string' } },
  },
}

const VERDICT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['refuted', 'reasoning', 'correction'],
  properties: {
    refuted: { type: 'boolean', description: 'true if the claim is wrong or unsupported' },
    reasoning: { type: 'string' },
    correction: { type: 'string', description: 'the corrected statement, or "" if the claim stands' },
  },
}

const LANES = [
  {
    key: 'caption',
    label: 'caption-override',
    prompt: `${HOUSE_RULES}
${CONTEXT}

YOUR LANE: how do we make the caption render "RL" instead of "Level"?

Investigate, in order:
1. Read crates/er-effects-rs/src/constants/stats_panel_text.rs around lines 100-200 in full.
2. Find every place the DLL already writes text into a ProfileSelect row field. Start from
   crates/er-effects-rs/src/experiments/startup_hooks/loading_cover/title_resources_stats_text.rs
   (it is large \u2014 find the SetText helpers, the row text pass, apply_row_slot_info_visibility around
   line 1240, and profile_editor_live_text_for_field around line 777). Establish EXACTLY which
   mechanism successfully writes ErStats / ErCharStats / DriveCell_* today, and whether that same
   mechanism can target the NATIVE child StaticText_110502.
3. Decide between these candidate approaches and rank them:
   (a) SetText the StaticText_110502 child with "RL" after the FMG static pass writes "Level".
       Determine WHEN that pass runs relative to our row text pass, and whether the child is still
       resolvable then (the constants comment says native children cannot be re-resolved
       post-populate \u2014 test that claim against how the drive-cell/ErStats writes actually work,
       and note that the FMG pass happens at BIND, which may be a different moment than POPULATE).
   (b) Hook/intercept FUN_14074c540 (the FMG static-text pass) or the SetText core FUN_140d842a0 and
       substitute the string for id 110502. Decompile FUN_14074c540 via the Ghidra MCP and report
       its signature, how it looks up the FMG text, and where a substitution would be cleanest.
   (c) Override the FMG entry 110502 itself. IMPORTANT: determine whether FMG id 110502 is used
       ANYWHERE ELSE in the game's menus \u2014 if it is a shared "Level" string, overriding it globally
       would change unrelated screens. That is a disqualifier; say so if true.
   (d) Hide StaticText_110502 entirely and render "RL" ourselves into a field we already control.
       Note which field could carry it and what it costs.
4. Also check: does the repo have an existing GFX-edit path that rewrites the 05_010 asset's
   DefineEditText contents (see crates/er-gfx/src/title_05_010.rs and
   crates/er-gfx/examples/make_05_010_stats.rs)? If the caption text is baked in the asset rather
   than FMG-sourced, that changes the answer. Verify which it actually is.

Deliver a ranked recommendation with exact touch points.`,
  },
  {
    key: 'weaponlevel',
    label: 'max-weapon-level',
    prompt: `${HOUSE_RULES}
${CONTEXT}

YOUR LANE: where does the MAXIMUM WEAPON UPGRADE LEVEL for a character come from?

The eight attributes (VIG/MND/END/STR/DEX/INT/FAI/ARC) already shown on a row are decoded OFFLINE
from the save slot body by crates/er-save-loader/src/stats.rs \u2014 read that whole file first. It
locates the PlayerGameData stat block by scanning for an offset satisfying the Rune Level identity
(the eight attributes sum to level + 79) and reads level + attributes from fixed offsets.

Establish:
1. How the per-slot stats reach the row today. Read crates/er-save-loader/src/stats.rs and
   crates/er-save-loader/src/bnd4.rs, then find the consumer that turns them into the
   "VIG 50 MND 10 ..." string and the cache that holds them (search for ensure_profile_slot_stats_cached
   and ErCharStats producers). Report the exact data path: .sl2 bytes -> struct -> string -> field.
2. Whether the inventory / equipment is reachable from the SAME slot body. In Elden Ring's save
   format, a character's inventory lives in the slot body alongside PlayerGameData. Determine
   whether this repo already parses any inventory/gaitem structure ANYWHERE (search the whole repo
   for gaitem, inventory, equip, GaItem, item_id, weapon). Report what exists.
3. What "weapon upgrade level" IS mechanically, and how to derive it. In ER the weapon param id
   encodes reinforcement: a weapon's id is baseId + reinforceLevel, base ids allocated in blocks of
   100, standard weapons +0..+25 and somber weapons +0..+10. VERIFY this against the repo's own
   param tooling if it exists (crates/soulsformats / er-soulsformats, tools/er-param-inspect,
   EquipParamWeapon, ReinforceParamWeapon) rather than taking my word for it. State clearly whether
   "maximum weapon level" should mean:
     - max over ALL weapons in inventory, or
     - max over EQUIPPED weapons only, or
     - the highest reinforce level reachable given the character's materials
   and which is both most useful and most cheaply derivable.
4. The DECISIVE question: can max weapon level be derived from the save slot body OFFLINE (the same
   way stats.rs derives attributes), or does it require live game memory? If offline, describe the
   concrete decode: where in the slot body, what the record layout is, and what invariant could
   validate the decode the way the Rune Level identity validates the stat block. If you cannot
   locate the inventory layout with confidence, SAY SO plainly and say what would be needed \u2014 do
   not invent offsets.
5. Note whether a local extraction/save corpus exists to test against (search for ER_GFX_CORPUS_ROOT,
   save-files, and any test that reads real .sl2 bytes and SKIPS when absent).

Be rigorous about offsets: a wrong offset here produces a plausible-looking wrong number on screen.`,
  },
  {
    key: 'layout',
    label: 'render-surface',
    prompt: `${HOUSE_RULES}
${CONTEXT}

YOUR LANE: WHERE does "WL <n>" render, and does the geometry actually work?

1. Read crates/er-gfx/profile_05_010_layout.toml in full, and crates/er-gfx/src/profile_05_010_layout.rs
   (font metrics, the clip_height floor, the font-height ceiling, validation, defaults). Understand:
   - line box = (MENU_FONT_ASCENT + MENU_FONT_DESCENT) / MENU_FONT_EM_SQUARE * font_height
   - min_clip_height_px = ceil(line_box + 2 * TEXT_DOC_INSET_PX_PER_EDGE)
   These are hard floors; a field whose clip_height is under its floor renders TRUNCATED text. This
   was a real shipped bug. Any geometry you propose MUST clear the floor \u2014 compute it and show the
   arithmetic.
2. Compute the ACTUAL rendered ink extents of the current cluster. The caption is left-aligned at
   x=-346 w=149; the value is right-aligned at x=-300 w=52. Work out how much horizontal room
   shrinking "Level" (5 glyphs) to "RL" (2 glyphs) really frees, at font_height 24, using the menu
   font metrics available in the repo (see crates/er-gfx/examples/dump_profile_layout_metrics.rs and
   any glyph-advance data in the DefineFont3 handling). If per-glyph advances are not available
   offline, say so and give a bounded estimate with your reasoning \u2014 do not fabricate precision.
3. Enumerate the candidate render surfaces for "WL <n>" and pick one:
   (a) Widen/reuse the caption field to carry "RL" and let a NEW field carry "WL <n>".
   (b) Put "RL <lvl>  WL <wl>" all in ONE field (which one? the caption is engine-owned, the value
       field is int-formatted natively \u2014 both are constrained; ErStats is BLANK on character rows and
       is 587px wide at x=-324, so it is a genuine candidate. Check what hides/shows it:
       RowSlotFieldVisibility in crates/er-loading-portrait/src/title_stats_text.rs, NATIVE vs
       browse_row.)
   (c) Add a brand-new DefineEditText field to the row template via the existing GFX edit pipeline
       (crates/er-gfx/src/title_05_010.rs, examples/make_05_010_stats.rs,
       scripts/rebuild-profile-05-010-layout.sh). Report how much machinery adding a field actually
       costs \u2014 asset regen, FIELD_NAMES, schema, visibility table, tests \u2014 and whether the tracked
       generated file (title_05_010_edits.rs) must be regenerated by the full rebuild script rather
       than the editor's hot-reload.
4. CHECK THE OVERLAP TRAP: ErStats [x=-324 w=587] and ErCharStats [x=-230 w=484] overlap heavily,
   and the row clips are RECYCLED across list kinds, so a field one kind writes keeps its text when
   another kind reuses the clip unless every kind states its visibility. Read
   RowSlotFieldVisibility (crates/er-loading-portrait/src/title_stats_text.rs) and
   apply_row_slot_info_visibility (title_resources_stats_text.rs ~line 1240). Whatever surface you
   pick, state EXACTLY what its visibility must be for each row kind (native character row, browse
   file row, drive row) or it will leak across views. This leak is a bug the user has already hit
   twice \u2014 do not reintroduce it.
5. Produce concrete proposed TOML geometry (x/y/width/clip_height/font_height/align) for whatever
   you propose, with the floor arithmetic shown, and confirm no two VISIBLE-together boxes on a
   character row overlap.`,
  },
]

phase('Ground')
const ground = await parallel(
  LANES.map((lane) => () =>
    agent(lane.prompt, { label: lane.label, phase: 'Ground', schema: FINDINGS_SCHEMA })
      .then((r) => (r ? { ...r, key: lane.key, label: lane.label } : null))
  )
)

const lanes = ground.filter(Boolean)
log(`ground: ${lanes.length}/${LANES.length} lanes returned`)

// Adversarially refute the single most load-bearing claim from each lane, plus its recommendation.
phase('Verify')
const targets = []
for (const lane of lanes) {
  const top = (lane.verified || []).slice(0, 2)
  for (const v of top) targets.push({ lane: lane.key, kind: 'claim', text: v.claim, evidence: v.evidence })
  targets.push({ lane: lane.key, kind: 'recommendation', text: lane.recommendation, evidence: '(the lane\u2019s proposed approach)' })
}
log(`verify: ${targets.length} claims under adversarial review`)

const verdicts = await parallel(
  targets.map((t, i) => () =>
    agent(`${HOUSE_RULES}
${CONTEXT}

You are an ADVERSARIAL REVIEWER. Another agent investigated the "${t.lane}" lane and asserts:

CLAIM (${t.kind}):
${t.text}

CITED EVIDENCE:
${t.evidence}

Your job is to REFUTE it. Go read the actual code / decompile the actual function and try to show the
claim is wrong, overstated, or unsupported by the cited evidence. Specifically hunt for:
  - an offset, VA, or field name that does not actually appear where claimed
  - a mechanism asserted to work that has no code path proving it
  - a claim about WHEN something runs (bind vs populate vs per-frame) with no ordering evidence
  - a geometry/arithmetic claim that does not survive recomputation
  - a proposal that would leak text across recycled row clips, or truncate text under the clip_height floor
  - reasoning that would produce a plausible-looking WRONG NUMBER on screen (worst outcome here)

Default to refuted=true when the evidence does not actually establish the claim. "It sounds
reasonable" is not evidence. If after genuinely checking you find the claim IS supported, set
refuted=false and say what specifically confirmed it.`,
      { label: `refute:${t.lane}:${i}`, phase: 'Verify', schema: VERDICT_SCHEMA, effort: 'high' })
      .then((v) => (v ? { ...t, ...v } : null))
  )
)

const checked = verdicts.filter(Boolean)
const refuted = checked.filter((v) => v.refuted)
log(`verify: ${refuted.length}/${checked.length} claims refuted or corrected`)

phase('Plan')
const plan = await agent(`${HOUSE_RULES}
${CONTEXT}

Three investigations ran in parallel, then every load-bearing claim was adversarially reviewed.

=== LANE FINDINGS ===
${JSON.stringify(lanes, null, 2)}

=== ADVERSARIAL VERDICTS (trust these OVER the lane findings where they conflict) ===
${JSON.stringify(checked, null, 2)}

Write the single implementation plan, in markdown, for the user's two asks:
  (1) the row's "Level" caption becomes "RL"
  (2) a new "WL <n>" readout showing the character's maximum weapon upgrade level

Requirements for the plan:
- LEAD with a straight verdict on ask (2): is max weapon level derivable OFFLINE from the save slot
  body with confidence, or not? If the offsets are not established, say so plainly and give the
  concrete next step to establish them rather than papering over it with a guess. A wrong offset
  ships a confident wrong number to the user's screen \u2014 that is the worst outcome, worse than
  shipping only ask (1).
- Treat ask (1) and ask (2) as SEPARATELY shippable. If (1) is cheap and certain and (2) needs more
  work, say exactly that and order the work accordingly.
- Give exact file:line touch points for every change, in dependency order.
- State the visibility statement for every row kind (native character row, browse file row, drive
  row) for any field whose text or visibility changes. The row clips are recycled; an unstated field
  leaks. Call this out explicitly \u2014 it is a bug the user has already hit twice.
- Show the clip_height floor arithmetic for any field whose font_height or clip_height changes.
- List which existing tests will FAIL and need updating (be specific: the repo has tests asserting
  the literal string "Level" \u2014 e.g. crates/er-gfx/tests/profile_stats.rs:731 and :787 \u2014 and tests
  asserting field geometry). Missing these turns a green gate red at push time.
- Note anything that must change TOGETHER in one commit to avoid an intermittent wrong-render.
- Flag every remaining unknown as an explicit open question, with what would resolve it.
- Do NOT include a summary of what the agents did or how the investigation was run. Just the plan.`,
  { label: 'synthesize', phase: 'Plan', effort: 'high' })

return { plan, refutedCount: refuted.length, checkedCount: checked.length }
