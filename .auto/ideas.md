# Deferred ideas

- If registry `[dialog+0xa48]` remains opaque, add a read-only registry observer around `0x1407ab370`/`0x1407a6c00` that latches inserted `MenuMemberFuncJob` nodes with provenance, then use that semantic latch as `title_menu_action_ready` instead of a broad dialog scan.
- Add a tiny static disassembly parser test that validates the `MenuMemberFuncJob::run` ABI (`[node+0x10]`, `[node+0x18]`, `[node+0x20]`) and `ProfileLoadDialog::load_activate` precondition offsets directly from the decrypted binary when present, skipping gracefully when the binary is absent.
