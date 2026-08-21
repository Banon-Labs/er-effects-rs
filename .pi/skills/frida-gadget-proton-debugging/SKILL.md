---
name: frida-gadget-proton-debugging
description: Use in er-effects-rs when debugging prose asks to use Frida, poke a live failed Elden Ring state, inspect runtime memory, attach to Proton/Wine Elden Ring, or asks "WSL? Windows?" during runtime diagnosis. Loads Frida Gadget as a Windows x64 ME3 native and connects with native Linux Python Frida instead of native-attaching to wine64-preloader.
---

# Frida Gadget Proton Debugging

Use this skill for Elden Ring runtime debugging in this repo when the task calls for Frida-style live memory inspection or poking.

## Ground truth for this repo

- The host is native Linux/CachyOS running Steam Proton/Wine, not WSL and not Windows.
- The target process is a Windows x64 process inside Proton. Frida code running inside the target should be Windows/x64 Frida Gadget.
- Do **not** treat the Wine host PID as a normal native Linux process for Frida injection. Native `frida.attach(<wine64-preloader pid>)` can crash Frida's bootstrapper and may kill the game.
- Do **not** late-inject Gadget into the already-running game with the Wine debugger/`LoadLibraryA` path unless explicitly testing the injector; this has killed the process before reaching the failed state.
- Preferred path: load `frida-gadget.dll` through ME3 as a temporary debug-only `[[natives]]` entry, then connect to Gadget at `127.0.0.1:27042`.

## Trigger checklist

If the user says any of these while debugging this repo, load and follow this skill:

- "use Frida"
- "poke it in the failed state"
- "live failed state"
- "inspect memory live"
- "attach to Elden Ring"
- "Wine/Proton Frida"
- "WSL? Windows?" in the context of runtime/debugging

## Setup Gadget

Use `uv` to provision Python Frida ephemerally; do not require a global Python package install.

```bash
cd /home/banon/projects/er-effects-rs
uv run --with frida python3 - <<'PY'
import frida
print(frida.__version__)
PY
```

Fetch the matching Windows x64 Gadget release and write a listen config:

```bash
cd /home/banon/projects/er-effects-rs
uv run --with frida python3 - <<'PY'
import frida, hashlib, json, lzma, pathlib, urllib.request
ver = frida.__version__
outdir = pathlib.Path('target/frida-gadget')
outdir.mkdir(parents=True, exist_ok=True)
xz = outdir / f'frida-gadget-{ver}-windows-x86_64.dll.xz'
dll = outdir / 'frida-gadget.dll'
if not dll.exists():
    if not xz.exists():
        url = f'https://github.com/frida/frida/releases/download/{ver}/frida-gadget-{ver}-windows-x86_64.dll.xz'
        xz.write_bytes(urllib.request.urlopen(url, timeout=60).read())
    dll.write_bytes(lzma.decompress(xz.read_bytes()))
config = {
    'interaction': {
        'type': 'listen',
        'address': '127.0.0.1',
        'port': 27042,
        'on_port_conflict': 'pick-next',
        'on_load': 'resume',
    }
}
for name in ('frida-gadget.config', 'frida-gadget.dll.config'):
    (outdir / name).write_text(json.dumps(config, indent=2))
print(dll.resolve(), dll.stat().st_size, hashlib.sha256(dll.read_bytes()).hexdigest())
PY
```

## Stage a debug-only ME3 profile

1. Create an artifact directory under `target/runtime-probe/<meaningful-name>`.
2. Copy `/home/banon/Elden/quicksave.me3` into that artifact directory before editing.
3. Add Gadget as a temporary native entry. Keep `er_effects_rs.dll` first; put Gadget before Seamless unless the current hypothesis needs a different order.
4. Restore the original profile after the Frida probe. Gadget must not remain in the product profile.

Minimal debug profile shape:

```toml
 profileVersion = "v1"
 start_online = false

 [[supports]]
 game = "eldenring"

 [[natives]]
 path = '/home/banon/projects/er-effects-rs/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll'

 [[natives]]
 path = '/home/banon/projects/er-effects-rs/target/frida-gadget/frida-gadget.dll'

 [[natives]]
 path = '/home/banon/.steam/steam/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll'
```

Launch through the approved user launcher:

```bash
cd /home/banon/projects/er-effects-rs
source scripts/steam-running.sh
steam_running || { echo 'Steam helper reports Steam absent'; exit 2; }
/home/banon/Elden/launch.sh > target/runtime-probe/<run>/me3-live.log 2>&1 &
```

Use a loud user-visible launch banner immediately before launching.

## Wait for Gadget and the target state

Check ports `27042..27051`. Do not trust stale telemetry; check telemetry mtime before deciding the run reached a state.

```bash
cd /home/banon/projects/er-effects-rs
uv run --with frida python3 - <<'PY'
import json, pathlib, socket, time
tele = pathlib.Path('/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game/er-effects-telemetry.json')
for _ in range(120):
    ports = []
    for port in range(27042, 27052):
        s = socket.socket(); s.settimeout(0.08)
        try:
            s.connect(('127.0.0.1', port)); ports.append(port)
        except OSError:
            pass
        finally:
            s.close()
    age = None
    state = {}
    if tele.exists():
        age = time.time() - tele.stat().st_mtime
        try:
            j = json.load(open(tele, errors='replace'))
            state = {k: j.get(k) for k in ['oracle_player_present', 'oracle_loading_bar_current_frame', 'oracle_loading_screen_close_sent', 'oracle_saved_map_c30', 'oracle_loaded_peak_c30']}
        except Exception as e:
            state = {'error': repr(e)}
    print('ports', ports, 'tele_age', None if age is None else round(age, 2), state)
    if ports and age is not None and age < 5:
        break
    time.sleep(2)
PY
```

## Connect to Gadget

Use the remote device and attach to the single `Gadget` process. In this Gadget runtime, the target process reports as Windows/x64 and `eldenring.exe` base should be `0x140000000`.

Important Frida 17 API details observed in this repo:

- Use `Process.getModuleByName('eldenring.exe').base`.
- Do **not** use `Module.getBaseAddress('eldenring.exe')`; this Gadget runtime reported `TypeError: not a function`.
- Use NativePointer instance methods: `p.readU32()`, `p.readS32()`, `p.readPointer()`, `p.writeU32(v)`.
- Enumerate exports with `Process.getModuleByName(name).enumerateExports()`.

Snapshot template:

```bash
cd /home/banon/projects/er-effects-rs
uv run --with frida python3 - <<'PY'
import frida, json
mgr = frida.get_device_manager()
dev = mgr.add_remote_device('127.0.0.1:27042')
procs = dev.enumerate_processes()
print(json.dumps([{'pid': p.pid, 'name': p.name} for p in procs], indent=2))
sess = dev.attach(procs[0].pid)  # usually one process named Gadget
js = r'''
function hx(p) { return p.isNull() ? '0x0' : p.toString(); }
function rptr(p) { try { return p.readPointer(); } catch (_) { return ptr(0); } }
rpc.exports = {
  snap: function () {
    const base = Process.getModuleByName('eldenring.exe').base;
    const gm = rptr(base.add(0x3d69918));
    const gd = rptr(base.add(0x3d5df38));
    const menuMan = rptr(base.add(0x3d6b7b0));
    const out = { base: base.toString(), gm: hx(gm), gameDataMan: hx(gd), menuMan: hx(menuMan) };
    if (!gm.isNull()) {
      out.gm = {
        b72: gm.add(0xb72).readU8(),
        b73: gm.add(0xb73).readU8(),
        b78: gm.add(0xb78).readS32(),
        b80: gm.add(0xb80).readS32(),
        bc4: gm.add(0xbc4).readS32(),
        bf5: gm.add(0xbf5).readU8(),
        ac8: '0x' + (gm.add(0xac8).readU32() >>> 0).toString(16),
        c30: '0x' + (gm.add(0xc30).readU32() >>> 0).toString(16),
        df0: hx(rptr(gm.add(0xdf0))),
      };
    }
    return out;
  }
};
'''
script = sess.create_script(js)
script.load()
print(json.dumps(script.exports_sync.snap(), indent=2))
sess.detach()
PY
```

## Known field anchors

- `eldenring.exe` base: `0x140000000`
- `GameMan` singleton pointer RVA: `0x3d69918`
- `GameDataMan` global RVA: `0x3d5df38`
- menu/input manager global RVA: `0x3d6b7b0`
- title proceed gate RVA: `0x3d856a0`
- important `GameMan` offsets:
  - `+0xb72` save requested byte
  - `+0xb73` save-request companion byte
  - `+0xb78` requested save slot load / warp target
  - `+0xb80` load FSM
  - `+0xbc4` return-title predicate
  - `+0xbf5` loading mode
  - `+0xac8` load target map id
  - `+0xc30` current/saved map id
  - `+0xdf0` resident device pointer

## Poking rules

- Prefer read-only snapshots first.
- Poke exactly one field at a time, record before/after and the telemetry response.
- Do not conclude success from a write succeeding. A prior poke of `GameMan+0xc30` from `0x0a010000` back to `0x1c000000` succeeded but did **not** restore player/world readiness.
- If a poke changes state, convert it into DLL telemetry/hook logic only after identifying the native owner/timing. Frida is a scalpel, not a product patch.

## Cleanup

- Restore `/home/banon/Elden/quicksave.me3` from the saved artifact copy.
- If the debug run remains live and invalid, terminate only after a user-visible banner naming the process/profile.
- Record artifact paths for:
  - original profile
  - Gadget profile
  - `me3-live.log`
  - Frida connection snapshot
  - each poke snapshot
