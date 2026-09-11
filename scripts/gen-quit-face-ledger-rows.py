#!/usr/bin/env python3
"""Emit the two ledger rows the Quit-panel portrait refresh needs, as a TSV on stdout.

The rows are a paragraph of derivation each, and a ledger row is one physical line, so composing
them by hand in an editor invites a stray newline that splits a row in two -- which
`verify-rva-map-1170.py` then reads as a malformed pair and skips at exit 0. Generating them keeps
the paragraph readable in source and the file correct.

Pipe into `scripts/append-verified-rva-rows.py` to add them:

    python3 scripts/gen-quit-face-ledger-rows.py > /tmp/rows.tsv
    python3 scripts/append-verified-rva-rows.py /tmp/rows.tsv
"""

import sys

ROWS = [
    (
        "0x1407c5c40",
        "0x1407c6ac0",
        "IDENTICAL-WHOLE",
        "1.000",
        "38",
        """hand-derived 2026-09-10 for the System>Quit panel's in-place portrait refresh:
        FUN_1407c5c40(), which picks which of two static descriptors a CS::MenuPlayerChrStatus is
        built from -- &DAT_143d6c8e0 when GLOBAL_WorldChrMan is live, &DAT_143d6c990 otherwise --
        and is the first of the two calls CS::OptionSettingTopDialog's constructor makes before
        FUN_14099b950. Derived from that constructor, whose pair 0x140966120 -> 0x1409672c0 this
        ledger already carries: the 1.16.2 body reads CALL 0x1407c5c40 at +0x1d6 and the 1.17 body
        reads CALL 0x1407c6ac0 at the same +0x1d6, with byte-identical surroundings (MOV RCX,RAX /
        LEA RDX,[RBP+0x1c0] / CALL the second helper / MOV byte ptr [RSP+0x20],1 / MOV R9,RAX / MOV
        R8D,0x13 / MOV RDX,RDI / LEA RCX,[RDI+0x1890] / CALL the builder). Confirmations. (1)
        map-rvas-1162-to-1170.py resolves it independently, UNIQUE at a 40B signature with 22B
        fixed, needing no anchor of its own. (2) This verifier: IDENTICAL-WHOLE over all 38
        instructions, both .pdata extents 0xc1, entry declared in both. (3)
        check-dump-deobf-identity.py --port 8767 reports MATCH at shift 0 for 0x1407c6ac0. (4) The
        entry bytes 48 83 ec 38 48 c7 44 24 20 fe ff ff ff are identical in eldenring-deobf.bin,
        eldenring-deobf-1.17.bin and eldenring-deobf-1.17.1.bin, and the two 1.17 images are
        byte-identical over the first 20 bytes -- which they must be, since 0x7c6ac0 is below the
        0xafefe9 boundary, so the 1.17.1 address is the same 0x1407c6ac0. (5) The +0xe80 delta is
        the one the four faceSource getters the builder calls already move by, read out of the two
        builder decompilations: 0x1407c8350 -> 0x1407c91d0, 0x1407c80b0 -> 0x1407c8f30, 0x1407c95e0
        -> 0x1407ca460, 0x1407c95d0 -> 0x1407ca450""",
        "BOTH-ENTRIES",
        "PDATA:0xc1/0xc1",
    ),
    (
        "0x1407c6f40",
        "0x1407c7dc0",
        "IDENTICAL-WHOLE",
        "1.000",
        "54",
        """hand-derived 2026-09-10 alongside 0x1407c5c40 above, and by the same route:
        FUN_1407c6f40(descriptor, dest), the second of the two calls
        CS::OptionSettingTopDialog's constructor makes before the SYSTEX_Menu_StatusFace builder.
        It refreshes CSMenuMan->playerStatusCalculator from GameDataMan->mainPlayerGameData through
        FUN_1407cb640 and then fills dest through FUN_1407c95f0, whose writes are the whole reason
        re-invoking the builder picks up a build import: dest+0x10 =
        PlayerGameData::GetFaceDataBuffer, dest+0x18 = mainPlayerIns->GetChrAsm(), dest+0x38 =
        gender, dest+0x39 = chrType == Hollow, all re-derived from the live character on every
        call. It also takes the non-returning DLPanic FD4Singleton.h 0xb4 path when
        GLOBAL_CSMenuMan is null, which is why er-profile-summary-core guards that singleton before
        calling. Derived from the constructor pair already in this ledger: 1.16.2 CALL 0x1407c6f40
        at +0x1e5, 1.17 CALL 0x1407c7dc0 at the same +0x1e5. Confirmations. (1)
        map-rvas-1162-to-1170.py agrees at the region's +0xe80, nearest anchor 0x1407c5c40 above,
        which is itself a unique match needing no anchor. (2) This verifier: IDENTICAL-WHOLE over
        all 54 instructions, both .pdata extents 0x101, entry declared in both. (3)
        check-dump-deobf-identity.py --port 8767 reports MATCH at shift 0 for 0x1407c7dc0. (4)
        Entry bytes 40 57 48 81 ec 90 00 00 00 48 c7 44 24 28 fe ff ff ff identical across all
        three images, and the two 1.17 images byte-identical over the first 20 bytes; below the
        0xafefe9 boundary, so the 1.17.1 address is the same 0x1407c7dc0""",
        "BOTH-ENTRIES",
        "PDATA:0x101/0x101",
    ),
]


def main() -> int:
    for row in ROWS:
        fields = [" ".join(field.split()) for field in row]
        sys.stdout.write("\t".join(fields) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
