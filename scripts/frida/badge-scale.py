#!/usr/bin/env python3
"""Live scaler for the armament Ash-of-War badge (bd er-effects-rs-pe98).

`SetIcon` scales its drawn quad by the target clip's LOCAL rect, so inflating that rect
inflates the rendered badge. This hooks the local-rect getter and grows ONLY the badge's
rect, identified by its exact signature -- the tile-local origin of the bottom-left
`ArtsIcon` slot. Every other caller (ItemIcon at 160px, AttributeIcon at 37px) is
untouched, so the rest of the menu renders normally.

The rect is anchored at the slot's origin, so the badge grows down/right from where it
already sits rather than drifting.

Tuning loop: the hook lives only as long as this process, so re-run with a different
factor to retry. Tiles are re-measured on populate -- reopen/re-enter the menu to see a
new factor take effect.

    uv run --with frida python3 scripts/frida/badge-scale.py 2.5

Requires the game running with frida-gadget listening (see the me3 profile that includes
target/frida-gadget/frida-gadget.dll).
"""

from __future__ import annotations

import argparse
import sys
import threading

import frida

#: `FUN_140d82060(CSScaleformValue*, float* out4)` -- LOCAL bounds {xmin,ymin,xmax,ymax}
#: in px. deobf 0x140d81fb0. This is the rect the icon setter divides the texture by.
PROXY_LOCAL_RECT_RVA = 0xD81FB0
#: Tile-local origin of the `ArtsIcon` slot (decoded from its PlaceObject2 matrix in
#: 02_011_equip.gfx: translate (-32, +37)). Used as the badge's identifying signature.
BADGE_ORIGIN_X, BADGE_ORIGIN_Y = -32.0, 37.0

AGENT = r"""
const RVA = %d;
const F = %f;
const OX = %f, OY = %f;
const m = Process.enumerateModules().find(x => /eldenring\.exe/i.test(x.name));
if (!m) {
  send({error: 'eldenring.exe module not found'});
} else {
  const target = m.base.add(RVA);
  let hits = 0, scaled = 0;
  Interceptor.attach(target, {
    onEnter(args) { this.out = args[1]; },
    onLeave() {
      hits++;
      const o = this.out;
      if (o === undefined || o.isNull()) return;
      const x0 = o.readFloat(), y0 = o.add(4).readFloat();
      const x1 = o.add(8).readFloat(), y1 = o.add(12).readFloat();
      if (Math.abs(x0 - OX) < 0.5 && Math.abs(y0 - OY) < 0.5 && (x1 - x0) > 1.0) {
        const nx = x0 + (x1 - x0) * F, ny = y0 + (y1 - y0) * F;
        o.add(8).writeFloat(nx);
        o.add(12).writeFloat(ny);
        scaled++;
        if (scaled <= 5) send({scaled: scaled, was: [x0, y0, x1, y1], now: [x0, y0, nx, ny]});
      }
    }
  });
  send({installed: target.toString(), factor: F});
  setInterval(function () { send({heartbeat: {rect_calls: hits, badge_scaled: scaled}}); }, 15000);
}
"""


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("factor", nargs="?", type=float, default=2.5,
                    help="badge rect multiplier (default 2.5)")
    ap.add_argument("--gadget", default="127.0.0.1:27042",
                    help="frida-gadget address (default 127.0.0.1:27042)")
    args = ap.parse_args()

    device = frida.get_device_manager().add_remote_device(args.gadget)
    session = device.attach("Gadget")
    script = session.create_script(
        AGENT % (PROXY_LOCAL_RECT_RVA, args.factor, BADGE_ORIGIN_X, BADGE_ORIGIN_Y)
    )
    script.on("message", lambda msg, _data: print(msg.get("payload", msg), flush=True))
    script.load()
    print(
        f"badge-scale: resident, factor={args.factor} -- close stdin (Ctrl-D) or stop the "
        "game to detach",
        flush=True,
    )
    # Stay resident on OBSERVABLE events only: the gadget script being destroyed (game gone)
    # or stdin reaching EOF (the operator detaching). A polling sleep loop here was both a
    # banned pattern and strictly worse -- it woke up to do nothing and detached up to five
    # seconds after the game had already died.
    detached = threading.Event()
    script.on("destroyed", detached.set)
    try:
        sys.stdin.read()
    except KeyboardInterrupt:
        pass
    detached.set()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
