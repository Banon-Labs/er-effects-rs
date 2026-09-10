// Drive an invasion from the agent side, so nobody has to reach for the item.
//
// `er_invasion_warp.dll` exports `er_invasion_warp_request_invade`, which arms the request for the
// next game-task tick rather than calling into Seamless from the calling frame -- the same reason
// `request_invade` exists at all inside the DLL. Calling the export from here is therefore the
// same path the lynchpin takes, minus the item.
//
// FIRE is false by default. Requesting a search while the player is already mid-negotiation
// restarts one on top of another, so the read-only pass below is what runs unless a search is
// wanted.
const FIRE = false;
const MODULE = 'er_invasion_warp.dll';
const EXPORT = 'er_invasion_warp_request_invade';

function log(s) { send({ line: s }); }

function main() {
  const mod = Process.findModuleByName(MODULE);
  if (mod === null) { log(MODULE + ' is not loaded'); return; }
  log(MODULE + ' @ ' + mod.base + ' size 0x' + mod.size.toString(16));

  let target = null;
  for (const e of mod.enumerateExports()) {
    if (e.name === EXPORT) { target = e; break; }
  }
  if (target === null) {
    log('the export ' + EXPORT + ' is absent -- this build does not expose the driver');
    return;
  }
  log('found ' + target.name + ' @ ' + target.address);
  if (!FIRE) {
    log('read-only pass: set FIRE = true and save to request a search');
    return;
  }
  const request = new NativeFunction(target.address, 'int', []);
  const armed = request();
  log('requested a search: returned ' + armed + ' (1 = armed for the next game tick)');
}

main();
