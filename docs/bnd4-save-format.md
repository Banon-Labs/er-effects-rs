# Elden Ring save file format (`ER0000.sl2` / `.co2`) -- BND4, plaintext + MD5 (NO encryption)

Fully reverse-engineered from the real bytes of `save-files/45-Slots/ER0000.sl2`
(28,967,888 bytes). Every offset/value below was read from that file's hexdump and **proven**
(the slot MD5 verifies). This grounds the **OWN-LOAD** plan: we read a slot ourselves and feed
the **plaintext** `0x280000` body to the engine's parser (see bd `OWN-LOAD-buffer-fed-replication-recipe`).

>  CORRECTION (2026-06-22, proven): **ER PC saves are NOT encrypted.** Earlier notes (and an
> earlier version of this doc) assumed per-entry AES-128-CBC. That is FALSE for PC `.sl2`. Each
> entry is `[16-byte MD5 checksum of the body][plaintext body]`; the 16 leading bytes are an
> **MD5 integrity hash, not an AES IV**. Verified: `md5(body[0x280000]) == the 16 stored bytes`.
> No key, no cipher -- just read it. (Confirmed against `chozandrias76/ER-Save-Editor`, whose PC
> save reader does zero decryption.) `.co2` (Seamless) uses the same BND4+MD5 layout.

---

## 1. Big picture

```
ER0000.sl2  =  one BND4 container
                +-- BND4 header               (0x40 bytes @ 0x00)
                +-- 12 file-entry headers      (0x20 bytes each @ 0x40 .. 0x1c0)
                +-- entry name table           (UTF-16LE, NUL-term @ 0x1c0 .. 0x300)
                +-- 12 entry data blobs         (@ 0x300 onward)
                      each blob = [16-byte MD5(body)] [ plaintext "USER_DATA" body ]
                      NO encryption -- the 16 bytes are an integrity checksum
```

The 12 entries are named `USER_DATA000` ... `USER_DATA011` (single underscore,
inside "USER_DATA" -- the trailing index has NO separating underscore; verified
against the on-disk name table, see SS4):
- `USER_DATA000` ... `USER_DATA009` -> the **10 character slots** (what we load).
- `USER_DATA010` -> small **common/settings** block.
- `USER_DATA011` -> **general/profile** block (menu-visible slot list, steam id, etc.).

---

## 2. BND4 header -- 0x40 bytes @ 0x00 (real values)

```
00: 42 4E 44 34                "BND4"     magic
04: 00                         unk04 (bool)            = 0
05: 00                         unk05 (bool)            = 0
06: 00 00                      pad
08: 00                         bigEndian (bool)        = 0  -> little-endian
09: 01                         bitBigEndian (bool)     = 1
0A: 00 00                      pad
0C: 0C 00 00 00                fileCount (int32)       = 0x0C = 12 entries
10: 40 00 00 00 00 00 00 00    headerSize (int64)      = 0x40
18: "30 30 30 30 30 30 30 31"  version (8 ASCII)       = "00000001"
20: 20 00 00 00 00 00 00 00    fileHeaderSize (int64)  = 0x20  (size of each entry header)
28: 00 03 00 00 00 00 00 00    fileHeadersEnd (int64)  = 0x300 (== where entry data begins)
30: 01                         unicode (bool)          = 1   (names are UTF-16LE)
31: 20                         rawFormat (byte)        = 0x20 (entry-field layout flags)
32: 00                         extended (byte)         = 0
33: 00                         pad
34: 00 00 00 00                pad
38: 00 00 00 00 00 00 00 00    (buckets/unk38)         = 0
```

Notes:
- `fileCount` x `fileHeaderSize` = 12 x 0x20 = 0x180 of entry headers (0x40 -> 0x1C0).
- `fileHeadersEnd` (0x300) is the end of headers **+ name table** = the first data offset.

---

## 3. File-entry header -- 0x20 bytes each, @ 0x40 + i*0x20 (real values)

Layout (confirmed by parsing entries 0,1,9,10,11):

```
+0x00: rawFlags (int32)        = 0x50   (uncompressed; "has size + name + data offset")
+0x04: 0xFFFFFFFF (int32)      padding / -1
+0x08: compressedSize (int64)  = the ENCRYPTED entry size (incl the 16-byte IV)
+0x10: dataOffset (int32)      = absolute file offset of this entry's data blob
+0x14: nameOffset (int32)      = absolute file offset of this entry's UTF-16 name
+0x18: 00 00 00 00 00 00 00 00 (uncompressedSize/unused for these entries)
```

Real entries:

| # | name          | dataOffset | entry size (`+0x08`) | body size (after MD5) | role |
|---|---------------|-----------|----------------------|-----------------------|------|
| 0 | USER_DATA000  | 0x000300  | 0x280010             | **0x280000**          | char slot 0 |
| 1 | USER_DATA001  | 0x280310  | 0x280010             | 0x280000              | char slot 1 |
| ... | ...         | ...        | 0x280010             | 0x280000              | char slots 2-8 |
| 9 | USER_DATA009  | 0x1680390 | 0x280010             | 0x280000              | char slot 9 |
| 10| USER_DATA010  | 0x19003a0 | 0x060010             | 0x060000              | common/settings |
| 11| USER_DATA011  | 0x19603b0 | 0x240020             | 0x240010              | general/profile |

- Slot `N`'s entry = `[dataOffset(N) .. dataOffset(N)+0x280010)` = `[0x10 MD5][0x280000 body]`.
- `dataOffset(N+1) = dataOffset(N) + 0x280010` for the slots (they're contiguous).
- body size = entry size - 0x10 (the leading **MD5 checksum** is stripped, NOT an IV). For the
  slots that's `0x280010 - 0x10 = 0x280000` -- **exactly the buffer size `0x67b290` allocs and the
  engine parser expects.** (Also matches `ER-Save-File-Readers` `SaveSlot::length()==2621456==0x280010`.)

---

## 4. Entry name table -- UTF-16LE, NUL-terminated, @ 0x1C0..0x300

Each `nameOffset` points here. Names are UTF-16LE, e.g. bytes at 0x1C0:
`55 00 53 00 45 00 52 00 5F 00 44 00 41 00 54 00 41 00 30 00 30 00 30 00 00 00`
= `U S E R _ D A T A 0 0 0` = `"USER_DATA000"` + UTF-16 NUL (note: exactly ONE
`5F` '_' per name -- the index "000" has no leading underscore). They run
consecutively `USER_DATA000`...`USER_DATA011`.

---

## 5. Per-entry integrity -- MD5 checksum (NO encryption)

Each entry's data blob is:

```
[ 0x00 .. 0x10 )  = 16-byte MD5 checksum of the body
[ 0x10 .. end  )  = plaintext body  (length = entry_size - 0x10)
```

**PROVEN** on slot 0 of `45-Slots`: `md5(body[0x280000]) == the 16 stored bytes`
(`49414531758e2530e4bf1aa539ea662a`). So those 16 bytes are an integrity hash, **not an AES IV**,
and the body is **plaintext** (entropy ~3.6, ~50% zeros, visibly structured -- not ~8.0 ciphertext).
There is **no key, no cipher, no DCX/zlib**. To read a slot: read its body bytes. To write one:
recompute `md5(body)` and store it in the 16-byte prefix (exactly what `ER-Save-Editor`'s
`PCSaveSlot::write` does: `digest = md5::compute(save_slot_bytes)`).

History note: earlier project notes assumed AES-128-CBC with a "save key" -- that is FALSE for PC
`.sl2` and was never actually tested. A decrypt round-trip with the supposed key produced uniform
garbage (entropy 7.54), which is the tell that there is nothing to decrypt.

---

## 6. The plaintext slot body (the `0x280000` buffer the engine parses)

The body (`dataOffset + 0x10` .. `+0x280010`, i.e. `0x280000` bytes after the MD5) is exactly what
`0x67b290` allocs and the engine parser consumes -- **we hand it this body directly, no transform**:
- `0x67bd70(GameMan, body, 0x280000)` reads a **0x10-byte header** off the front and writes
  `GameMan+0xc30` (saved map id) from **header+4** (slot 0 of `45-Slots`: `body+4 = 0x1c000000`).
- The big deserialize `0x140258840` (stream over `body`) populates PlayerGameData / world state.
- The body contains the character: name, level, stats, inventory, event flags, world progress, map
  coords, etc. `third_party/ER-Save-File-Readers` `src/models/save_slot/` and
  `chozandrias76/ER-Save-Editor` `src/save/common/save_slot.rs` are the field-by-field reference for
  the body interior. For OUR purpose we don't parse the interior -- we hand the whole `0x280000`
  body to the engine parser.

---

## 7. How this feeds the OWN-LOAD plan (simplified -- no decrypt)

1. Read `ER0000.sl2` (path `.../EldenRing/<steamid>/ER0000.sl2`; folder from builder `0x140e0e680`).
2. Parse the BND4 header + entry headers (SS2-3) -> locate `USER_DATA00<slot>` (dataOffset).
3. Take the **plaintext body** = `file[dataOffset+0x10 .. dataOffset+0x10+0x280000]`. (Optionally
   verify `md5(body)` == the 16-byte prefix.) **No decryption.**
4. Feed that body to the engine parser (replicate `0x67b290`'s post-read body on it -- bd
   `OWN-LOAD-buffer-fed-replication-recipe`), then `SetState(5)`.

This removes the hardest imagined step (decrypt + key) entirely: a slot load is "read file, slice
the body, hand it to the parser." The only real work left is the in-DLL replication of `0x67b290`'s
parse calls on our body buffer + the `SetState(5)` world handoff.
