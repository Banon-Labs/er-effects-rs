---
name: elden-ring-product-dll-bisect
description: Use when bisecting Elden Ring product-DLL runtime regressions, classifying product DLL candidates, or running manual/playability bisection checks.
---

# Elden Ring Product-DLL Bisect

Use this to isolate a runtime regression without poisoning the oracle.

## Setup invariants

1. Start from a clean known-good baseline. Run it first, prove the harness can classify a known good state, and record the baseline artifact path before testing candidates.
2. Vary only the product DLL when bisecting product regressions. Keep the companion DLL set, ME3/profile contents, launch path, save state, and artifacts layout fixed unless the hypothesis explicitly requires changing one of them.
3. For manual or movement-relevant runs, omit `er_input_harness_dll.dll`. Its mere presence is default-on and contaminates/blocks user movement, even if you think you are not using it.
4. Ensure an active/default save exists before runtime candidates, or explicitly provision one. Missing-save prompts make the run invalid. The user permits deleting/replacing `.co2`/`.sl2` for this work; record the exact paths touched and whether each was deleted, replaced, copied, or left intact.
5. Clear stale control files before launch: autoload, switch, prove-movement, stay-active, input-trace, and harness-drive markers. A candidate that reloads before movement/readiness because of leftover switch files is INVALID, not GOOD.

## Oracle rules

- Calibrate the oracle on the clean known-good baseline before classifying any candidate.
- Do not accept stale or brittle telemetry as world-readiness proof. For manual runs, the controlling oracle is the human observation: if the human can or cannot move, record that verdict as controlling.
- For movement/readiness regressions, telemetry may support the verdict but must not override a manual movement observation from an uncontaminated run.
- Stop patch-stacking. Once the first bad commit/delta is isolated, inspect, revert, or split that delta; do not add runtime compensations on top of an uninspected regression.

## Required record for each candidate

Write artifacts and a Beads comment containing:

- product DLL commit and content hash;
- exact companion DLL set, explicitly noting whether `er_input_harness_dll.dll` is absent or present;
- profile contents used for the launch;
- active save state and every `.co2`/`.sl2` path touched;
- stale control files cleared before launch;
- artifact directory and key log/crash files;
- manual/user observations, especially movement and quit/reload behavior;
- final classification: GOOD, BAD, or INVALID, with the reason.

## GOOD / BAD / INVALID checklist

Classify only after the setup invariants and oracle rules above are satisfied.

- GOOD: uncontaminated candidate reaches the target behavior. Example: `main` `e3e169e` no-harness run where the user manually moved, then quit => GOOD for first-load playable.
- BAD: uncontaminated candidate fails the target behavior. Example: `4ac4366` no-harness run crashed at RVA `0x67141a` before manual dwell => BAD.
- INVALID: the run cannot answer the question. Examples: `er_input_harness_dll.dll` was loaded while asking the user to move; missing-save prompt appeared; leftover switch/control files forced a reload before movement/readiness.
