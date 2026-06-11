# er-effects-rs docs

Reference notes and external resources for the Elden Ring runtime effects experiment.

## Reference resources

- [Souls Modding reference tree](./reference-tree/soulsmodding.md) — shallow crawl from Souls Modding into Elden Ring / Nightreign resources and relevant git/project links.
- [Souls Modding reference tree — depth 2](./reference-tree/soulsmodding-depth-2.md) — focused expansion into SpEffect animation data, FXR/SFX pages, and selected git project metadata.
- [Souls Modding reference tree — depth 3](./reference-tree/soulsmodding-depth-3.md) — concrete next-layer resources: `anim 63010` SpEffect entries, source data links, and repo entry points for FXR reload, me3, HKS/Havok, and Smithbox.
- [Souls Modding reference tree — depth 4](./reference-tree/soulsmodding-depth-4.md) — focused runtime-trigger comparison for `anim 63010`, local seeded SpEffect IDs, and Elden Ring vs Nightreign `Event63010` HKS semantics.
- [Souls Modding reference tree — depth 5](./reference-tree/soulsmodding-depth-5.md) — deeper HKS event-semantics comparison for `Event630xx`, Elden Ring `EventCommonFunction`, and Nightreign near-death / `1021xx` SpEffect transition handling.
- [Souls Modding reference tree — depth 6](./reference-tree/soulsmodding-depth-6.md) — Nightreign near-death entry, direct-death gates, and selected `1021xx` / revival SpEffect control mapping behind `Event63010`.
- [Souls Modding reference tree — depth 7](./reference-tree/soulsmodding-depth-7.md) — Nightreign `W_NearDeath*` target mapping, `Event60910`, and near-death start/start-to-idle/idle state functions.
- [Souls Modding reference tree — depth 8](./reference-tree/soulsmodding-depth-8.md) — HkbEditor/HKB routing layer: HKS state callbacks, `ExecEvent("W_*")`, wildcard transitions, game-event templates, and event-listener validation path.
- [Souls Modding reference tree — depth 9](./reference-tree/soulsmodding-depth-9.md) — repo-local Elden Ring runtime layer: `time_act` trigger fields, SpEffect apply/remove API, local param schema fields, and the validation boundary for `SpEffect 5008`.
- [Souls Modding reference tree — depth 10](./reference-tree/soulsmodding-depth-10.md) — param-row acquisition and active-SpEffect observation options: Smithbox, Elden Ring Debug Tool, ParamStructGenerator, libER, and The Grand Archives Cheat Table.
- [`@cccode/fxr`](./references/cccode-fxr.md) — JavaScript/TypeScript library for creating and editing FromSoftware FXR particle-effect files, including Elden Ring.
