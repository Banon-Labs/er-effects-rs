# Ideas backlog

- Replace fixed live-dialog activation settle with a real readiness predicate before trying <60 frames; 30-frame fixed settle produced false-positive visual/world evidence without selected save/player correctness.
- 45-frame fixed live-dialog settle also failed on rerun despite appearing loaded; keep 60 frames unless a structured readiness predicate replaces the fixed wait.
- 15-frame initial settle screenshot has world scenery/dialog but no visible character; likely not a true controllable state despite structured save/player fields. Treat as unsafe until visual/player-presence mismatch is explained.
- 20-frame initial settle reproduces the unsafe return-to-title modal/no-visible-character state; fixed initial settle must stay at 25 unless a positive readiness predicate replaces it.
- 150-frame PHASE_MENU_BUILD modal grace also reproduces the return-to-title modal/no-character state; keep fixed modal grace at 180 unless a positive Steam/title-modal readiness predicate replaces it.
- Old er-skip-splash-screens source only patches one byte: image offset/RVA 0xb0c3ed expected 0x74 -> 0x7f. On current exe this is stale by +0x90 versus the working built-in 0xb0c35d patch, so it does not imply another splash-skip phase.
