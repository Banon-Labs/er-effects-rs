#!/usr/bin/env python3
"""Diagnose why the HUD Ash-of-War lookup returns nothing, by watching the GAME do it.

The HUD badge binds correctly (7 ctors / 6 bound / correct scene offsets) but every frame
takes the "no ash" branch. Somewhere in

    WorldChrMan(+0x1e508) -> PlayerIns
    GetWeaponGaitemHandleBySlot(player, &handle, slot)
    GetGaitemInsByHandle(&record, &record)
    GetSwordArtsParamForWeapon(&record, &result)

a step yields nothing, and the DLL cannot tell which one without a rebuild per guess.

So this does NOT call anything. `UpdatePlayerComponents` invokes
`GetWeaponGaitemHandleBySlot` every frame for the player's own weapons, so hooking it and
recording `(rcx, slot) -> handle` observes the game performing the exact lookup the DLL is
failing at -- zero calls issued from a frida thread, zero risk of racing the game thread.

The decisive comparison is `rcx`: the DLL passes `WorldChrMan+0x1e508`, and if the game
passes something else then the receiver object is wrong and the handle is always zero.

    uv run --with frida python3 scripts/frida/hud-arts-chain.py

Exits on its own once it has collected a sample budget (no sleeps, no polling: the JS side
sends `done` and the Python side blocks on that event).
"""

from __future__ import annotations

import argparse
import threading

import frida

#: `GetWeaponGaitemHandleBySlot(obj /*rcx*/, u32* out /*rdx*/, ChrAsmSlot /*r8d*/)`.
GET_WEAPON_GAITEM_HANDLE_BY_SLOT_RVA = 0x656920
#: `WorldChrMan` singleton pointer, and the local-player offset inside it.
WORLD_CHR_MAN_GLOBAL_RVA = 0x3D65F88
WORLD_CHR_MAN_PLAYER_INS_OFFSET = 0x1E508
#: `GetGaitemInsByHandle(record /*rcx*/, record /*rdx*/)` -- record is in+out, handle at +0x0.
GET_GAITEM_INS_BY_HANDLE_RVA = 0x672E40
#: `GetSwordArtsParamForWeapon(record /*rcx*/, SwordArtsParamLookupResult* /*rdx*/)`.
GET_SWORD_ARTS_PARAM_FOR_WEAPON_RVA = 0x673F30
#: `SwordArtsParam.iconId`, u16.
SWORD_ARTS_PARAM_ICON_ID_OFFSET = 0x1A
#: `GetEquipParamGem(EquipParamGemLookupResult* out, uint id)` -- {paramId@0, row@8}.
GET_EQUIP_PARAM_GEM_RVA = 0xD2A360
#: `EquipParamGem.iconId` u16 @+0x4, `.swordArtsParamId` s32 @+0x18.
EQUIP_PARAM_GEM_ICON_ID_OFFSET = 0x4
EQUIP_PARAM_GEM_SWORD_ARTS_ID_OFFSET = 0x18

AGENT = r"""
const HANDLE_RVA = %d;
const WCM_RVA = %d;
const PLAYER_OFF = %d;
const BUDGET = %d;
const GAITEM_RVA = %d;
const ARTS_RVA = %d;
const ICON_OFF = %d;
const GEM_RVA = %d;
const GEM_ICON_OFF = %d;
const GEM_ARTS_OFF = %d;

const m = Process.enumerateModules().find(x => /eldenring\.exe/i.test(x.name));
if (!m) {
  send({error: 'eldenring.exe module not found'});
} else {
  // What the DLL computes, read once up front.
  let wcm = null, player = null;
  try {
    wcm = m.base.add(WCM_RVA).readPointer();
    if (!wcm.isNull()) player = wcm.add(PLAYER_OFF).readPointer();
  } catch (e) { send({walk_error: String(e)}); }
  send({
    module_base: m.base.toString(),
    world_chr_man: wcm ? wcm.toString() : null,
    player_ins_dll_computes: player ? player.toString() : null,
  });

  // What the GAME actually passes, observed on its own per-frame calls.
  const seen = {};
  let n = 0;
  Interceptor.attach(m.base.add(HANDLE_RVA), {
    onEnter(args) {
      this.obj = args[0];
      this.out = args[1];
      this.slot = args[2].toInt32();
    },
    onLeave() {
      let handle = null;
      try { handle = this.out.readU32(); } catch (e) { handle = -1; }
      const key = this.obj.toString() + '/' + this.slot;
      if (seen[key] === undefined) {
        seen[key] = true;
        send({
          call: {
            obj: this.obj.toString(),
            slot: this.slot,
            handle: handle,
            obj_is_dll_player: player ? this.obj.equals(player) : null,
          }
        });
        if (++n >= BUDGET) { stage2(); }
      }
    }
  });

  // STAGE 2 -- run the rest of the chain ourselves with the handles the game just produced.
  // The two callees are pure lookups (a singleton fetch plus a param-table read), so driving
  // them from here answers "which step yields nothing" without a rebuild-and-relaunch per
  // guess. Everything is reported as raw fields so a wrong STRUCT LAYOUT is as visible as a
  // wrong value.
  const getHandle = new NativeFunction(m.base.add(HANDLE_RVA), 'pointer',
    ['pointer', 'pointer', 'int']);
  const getGaitem = new NativeFunction(m.base.add(GAITEM_RVA), 'pointer',
    ['pointer', 'pointer']);
  const getArts = new NativeFunction(m.base.add(ARTS_RVA), 'pointer',
    ['pointer', 'pointer']);
  const getGem = new NativeFunction(m.base.add(GEM_RVA), 'void', ['pointer', 'uint']);

  // The MENU badge (which demonstrably shows correct icons) reads the icon off EquipParamGem,
  // reaching it by the `arts_id * 100` convention and then VERIFYING the row's own
  // swordArtsParamId matches. Check that same route for the two equipped weapons.
  function gemIcon(artsId) {
    if (artsId <= 0) return {skipped: 'arts_id <= 0'};
    const res = Memory.alloc(0x10);
    res.writeByteArray(new Array(0x10).fill(0));
    getGem(res, artsId * 100);
    const row = res.add(8).readPointer();
    if (row.isNull()) return {gem_id: artsId * 100, row: null};
    return {
      gem_id: artsId * 100,
      row: row.toString(),
      gem_sword_arts_id: row.add(GEM_ARTS_OFF).readS32(),
      gem_icon_id: row.add(GEM_ICON_OFF).readU16(),
    };
  }

  let ran2 = false;
  function stage2() {
    if (ran2) return;
    ran2 = true;
    if (!player || player.isNull()) { send({done: true, reason: 'no player'}); return; }
    for (const slot of [-2, -1]) {
      try {
        const hOut = Memory.alloc(8);
        hOut.writeU32(0);
        getHandle(player, hOut, slot);
        const handle = hOut.readU32();

        // record: {u32 handle @0, ptr ins @8, u32 kind @0x10}
        const rec = Memory.alloc(0x20);
        rec.writeByteArray(new Array(0x20).fill(0));
        rec.writeU32(handle);
        getGaitem(rec, rec);

        const out = Memory.alloc(0x10);
        out.writeByteArray(new Array(0x10).fill(0));
        getArts(rec, out);
        const paramId = out.readU32();
        const row = out.add(8).readPointer();
        let icon = null;
        if (!row.isNull()) icon = row.add(ICON_OFF).readU16();

        send({chain: {
          slot: slot,
          handle: handle,
          rec_ins: rec.add(8).readPointer().toString(),
          rec_kind: rec.add(0x10).readU32(),
          arts_param_id: paramId,
          arts_row: row.toString(),
          icon_id: icon,
          gem: gemIcon(paramId),
        }});
      } catch (e) {
        send({chain_error: {slot: slot, err: String(e)}});
      }
    }
    send({done: true});
  }

  send({installed: true});
  // A frame or two is enough for every distinct (obj, slot) pair; end the run on a timer
  // that exists only as a backstop for a paused/menu'd game with no HUD updates.
  setTimeout(stage2, 6000);
}
"""


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--gadget", default="127.0.0.1:27042")
    ap.add_argument("--budget", type=int, default=24, help="distinct (obj, slot) pairs")
    args = ap.parse_args()

    device = frida.get_device_manager().add_remote_device(args.gadget)
    session = device.attach("Gadget")
    script = session.create_script(
        AGENT
        % (
            GET_WEAPON_GAITEM_HANDLE_BY_SLOT_RVA,
            WORLD_CHR_MAN_GLOBAL_RVA,
            WORLD_CHR_MAN_PLAYER_INS_OFFSET,
            args.budget,
            GET_GAITEM_INS_BY_HANDLE_RVA,
            GET_SWORD_ARTS_PARAM_FOR_WEAPON_RVA,
            SWORD_ARTS_PARAM_ICON_ID_OFFSET,
            GET_EQUIP_PARAM_GEM_RVA,
            EQUIP_PARAM_GEM_ICON_ID_OFFSET,
            EQUIP_PARAM_GEM_SWORD_ARTS_ID_OFFSET,
        )
    )

    finished = threading.Event()

    def on_message(msg, _data):
        payload = msg.get("payload", msg)
        print(payload, flush=True)
        if isinstance(payload, dict) and payload.get("done"):
            finished.set()

    script.on("message", on_message)
    script.on("destroyed", finished.set)
    script.load()
    finished.wait()
    try:
        script.unload()
    except frida.InvalidOperationError:
        pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
