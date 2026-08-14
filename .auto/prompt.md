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

## How to run
`bash .auto/measure.sh`

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
- Continue autonomously until interrupted.

## Current state
PR #193 contains the reviewed roadmap and drift guard. Its root planning issue `er-effects-rs-q4oh` is closed. The per-PR R0 Beads DAG has not yet been expanded; build that exact DAG first, then start the maximum set of dependency-ready disjoint lanes.
