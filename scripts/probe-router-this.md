# router_this (title CSMenu controller) — runtime probe map

Static-derived (READ-ONLY, decrypted eldenring.exe 1.16.1, base 0x140000000).
Resolves the "where do the selectable rows live + how to populate them" blocker.

## Object identities (DISTINCT objects, both CSMenu-derived, shared base dtor 0x140734230)

- TitleTopDialog  @ owner+0xe0.  vtable on-disk 0x142b25668; update [vt+0x10]=0x1409aac10,
  [vt+0x18]=0x1409aa420. +0x1290 = Scaleform/GFx MARKUP TEXT (NOT a vector).
  Registers entry SPECS into [dialog+0xa48] via registrar 0x1409b24e0.
- router_this  = a SEPARATE title CSMenu controller.  ctor 0x1409060d8; primary vtable
  on-disk 0x142af9270 (runtime [0]=0x142afa070; +0xe00 dump/PE skew), update
  [vt+0x10]=0x140745570 (widget pump), [vt+0x18]=0x1407451b0.
  Layout: cursor sub-object @ +0xa38 (cursor int = [+0xa38+0xd4] = [+0xb0c]);
  bound/count @ [+0xb08]; ROW VECTOR begin/end @ [+0x1290]/[+0x1298] (stride 0x210);
  list-model / delegate sub-objects @ +0x12c8..+0x1378 (back-ptr [sub+8]=router_this);
  also reads +0x1278 (scroll), +0x1bc0 (flag), +0x2300 (anim) at runtime.

## Select / confirm routing (rsi = router_this)
- router 0x14078e1c0:  cursor=getter 0x140739e20([rsi+0xa38]) -> resolver 0x14078fbd0
  (rcx=rsi -> [rsi+0x1290]+idx*0x210); if [entry+0xf8]!=0 -> rax=[entry]; call [rax+0x10]
  (entry action). Confirm 0x14078ef20(rcx=rsi, rdx=entry); confirm-desc [entry+0x180].
  Load-Game entry action -> dialog_factory 0x14081ead0 -> ProfileLoadDialog.
- load_activate 0x1409a4670(rcx=this): reads cursor [this+0xb0c], bound [this+0xb08],
  dispatch [this->vt+0x90]. SAME +0xa38/+0xb08/+0xb0c CSMenu layout as router_this.

## Spec -> row PUSH (materializes [router_this+0x1290])  — ZERO-INPUT
- spec_to_row 0x14078cf70(rcx=dest_entry, rdx=spec): parses spec keys (0x14074a2f0)
  into entry sub-objects at entry+0x10 (action, via 0x14078dec0) and entry+0x140.
- rebuild_rows 0x14078d2c0(rcx=list-model container, rdx=src iterator pair): emplaces
  every src element at [[container]+8]+idx*0x210 via spec_to_row; grow via 0x14078c01e
  -> move-realloc 0x14078b610 (stride-0x210 copy loop, per-entry 0x1407411e0).
- append_one 0x14078eea0(rcx=this, r8=&idx): single emplace.
- Dispatch: set_items thunk 0x14078ec90 = vtable 0x142aa1618 slot +0x48; append_one = +0x80.
- VERIFIED zero-input: spec_to_row/rebuild_rows/append_one reference NONE of
  inputmgr 0x143d6b7b0, accept byte 0x144589bdc, accept reader 0x140e85f50, or keystate.
  Rows are pure-data materialized when the list-model's set_items/populate step runs.

## OPEN (resolve at runtime — template vtables not RIP-installed, opaque to static):
1. router_this address & how to reach it from owner / TitleTopDialog (scan owner+0xe0
   neighborhood + the title owner for an object whose [0]==0x142afa070 and [+0xa38+0xd4]
   is a small int; confirm [obj+0x1290]!=ASCII).
2. The outermost step that calls set_items (vtable 0x142aa1618 +0x48). Set a write-watch
   on [router_this+0x1298] (row-vector END ptr) to catch the exact populate caller, or
   hook 0x14078d2c0 / 0x14078eea0 and log rcx + caller.

## Zero-input recipe (once router_this located):
A) let native title flow tick (registrar self-fires on Loop/TextFadeOut, latch [dialog+0xa40]).
B) trigger the populate: either (i) wait for the native per-frame populate step that calls
   set_items (preferred, fully zero-input), or (ii) directly call rebuild_rows 0x14078d2c0
   (rcx=router_this list-model container, rdx = the +0xa48 spec iterator range) — no input.
C) set cursor [router_this+0xb0c]=Load-Game row idx (< [+0xb08]); call load_activate
   0x1409a4670(rcx=router_this); PGD [0x144588268]!=0 precond.
D) pass frames so the selector ticks the mount; guard (ac0==slot && c30 real) ->
   continue_confirm 0x140b0e180 -> SetState(owner,5).
