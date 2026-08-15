# Autoresearch: execute PR #193 crate-extraction DAG through R58

## Objective
Translate every approved work package in `docs/plans/crate-extraction-execution-roadmap.md` into a small, single-concern pull request, preserving dependency-local stacks and using the maximum safe number of disjoint worktree lanes. Continue through R58. A documented, evidence-backed rejection at a Phase F decision gate is valid completion for the optional extraction it rejects; merely opening a Beads issue is not plan-to-PR completion.

## Primary metric
- `plans_translated_to_prs` (higher is better): distinct `pr193-roadmap` Beads plan IDs linked one-to-one to a real GitHub PR that is OPEN or MERGED.

## Secondary metrics
- `planned_nodes_total`: current roadmap plan-node count, including expanded child nodes.
- `completion_pct`: translated plans / total plans.
- `open_prs`, `merged_prs`, `draft_prs`.
- `ready_untranslated_plans`: dependency-ready roadmap issues without a qualifying PR.
- `false_positives`: malformed mappings, missing PRs, closed-unmerged PRs, duplicate plan/PR mappings, or plan labels not present in the roadmap/expanded DAG. Keep decisions require `false_positives == 0`.

## Operating model

### Three separate kinds of work

1. **Translation work** implements one dependency-ready roadmap node, completes its stated proof gate, opens its one-to-one PR, and records the resulting mapping.
2. **Proof/unblock work** performs bounded static RE, offline validation, or feature-specific runtime diagnosis needed to make a future node eligible. It is not a PR-translation benchmark iteration.
3. **Measurement** verifies the mapping ledger after a real PR transition. It is not work by itself.

Never collapse these categories. A successful build, commit, static finding, runtime probe, Beads comment, or branch push is evidence for translation work; none changes the primary metric without a qualifying PR.

### When `run_experiment` and `log_experiment` are allowed

Call `bash .auto/measure.sh` through `run_experiment` only after an actual, reviewable PR transition: a qualifying PR was opened, merged, or became invalid/closed and the ledger must be remeasured. Call `log_experiment` only for that measurement and cite the exact PR number, roadmap ID, and proof artifact.

Do **not** run the measurement or log a `discard` when no qualifying PR transition occurred. In particular, do not turn failed runtime probes, static investigation, unchanged repository state, or repeated measurement into benchmark discards. Record those outcomes in the owning Beads issue with the artifact path, exact failed/passed oracle, and the next falsifiable hypothesis.

A runtime-gated node may open its PR only after its feature-specific live oracle passes. Never create a placeholder PR merely to move the metric. If a Phase F optional extraction is rejected, record the evidence-backed decision in its owning roadmap issue; do not fabricate a PR mapping.

### Translation loop

1. Select a dependency-ready, unmapped roadmap node and name its owning Beads issue before editing.
2. Read the node's proof gate and choose the smallest implementation or proof change that can falsify the next hypothesis.
3. Keep proof/unblock work in its own non-main worktree. Preserve committed evidence; do not let benchmark bookkeeping reset unrelated worktree changes.
4. For runtime work, use static RE first. Run one bounded, feature-specific probe only after the needed oracle is present. A failed probe is evidence for the next static diagnosis, not a reason to repeat it.
5. Once the node satisfies its proof gate, create and push a non-main branch, open the single-node PR, and attach the evidence. Then run `measure.sh` and log the observed ledger transition.
6. If no node is eligible for a PR transition, continue proof/unblock work. Do not measure merely because an iteration was requested.

## Scope
- `docs/plans/crate-extraction-execution-roadmap.md`
- Beads issues labeled `pr193-roadmap` and `roadmap-<normalized-plan-id>`
- dependency-local feature branches/worktrees and their pull requests
- source/tests/docs directly required by those roadmap nodes

## Constraints
- Do not overfit or falsify the metric. A branch, commit, draft note, or Beads ticket without a real qualifying GitHub PR does not count.
- One roadmap plan ID per PR and one PR per plan ID unless the roadmap explicitly records an evidence-backed rejection.
- Preserve the single shipped `er_effects_rs.dll` product contract.
- Parallel writers require disjoint worktrees. Beads writes are parent-owned and serialized.
- Follow every node's static/runtime proof gates. Runtime-affecting nodes are not complete without their required live oracle.
- Create new Beads tickets only for roadmap child expansion or newly discovered in-scope blockers.
- Never push directly to `main`; push feature/dependency-stack branches.
- A `discard` is valid only for a real PR-transition measurement that reveals an invalid mapping or a lost qualifying PR. It must name that PR and the corrected ledger state.
- Continue autonomously until interrupted.

## Current state
The PR #193 roadmap has 104 plan nodes and 17 valid one-to-one PR mappings. Current runtime-gated R33 proof is blocked after title-menu open: the former `0x140764b80` probe was corrected to the idle `01_900_Black` job, not Continue. The next R33 proof/unblock step is a passive oracle for the actual `FUN_1409abc30` CommandList Continue-label (`0x61f95`) branch, followed by a bounded product run only if that oracle is installed. This is proof/unblock work, not a metric iteration.
