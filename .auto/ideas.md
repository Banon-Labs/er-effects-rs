# Ideas backlog

- Replace fixed live-dialog activation settle with a real readiness predicate before trying <60 frames; 30-frame fixed settle produced false-positive visual/world evidence without selected save/player correctness.
- 45-frame fixed live-dialog settle also failed on rerun despite appearing loaded; keep 60 frames unless a structured readiness predicate replaces the fixed wait.
