// Two questions this session cannot answer from a static image.
//
// 1. The GFx tag-parse prologue is unique in eldenring-deobf-1.17.1.bin and unique in the shipped
//    eldenring.exe, and yet the DLL's boot-time scan of the LIVE .text calls it "absent or not
//    unique" and turns the movie swap off -- which is why the announcement banner is still
//    left-aligned. Counting the live matches says which of the two it is.
// 2. Whether the host's Steam persona name can be resolved in-process at all. The session field
//    gives a SteamID64; a name needs ISteamFriends, and Steam only knows a non-friend's name once
//    it has been told about them. Calling it here is cheaper than shipping a guess.
const PARSE_SIG = '40 53 48 83 EC 40 48 8B 41 18 48 8B D9 C6 44 24 30 01 48 83 C1 50 4C 8B 50 20 4C 8B 58 48';
// Hosts this run's telemetry caught in `session+0x1d8`.
const HOSTS = ['76561198120963003', '76561198983316799'];

function log(s) { send({ line: s }); }

// The address the prologue occupies in both static images, so a live read can say what is
// actually there instead of only that the scan found nothing.
const PARSE_RVA = 0x11d1010;

function question1(mod) {
  // The whole module rather than a parsed .text: the section walk was returning nothing and the
  // question is about a byte pattern, so a wider window can only over-report, never under-report.
  log('scanning ' + mod.base + ' + 0x' + mod.size.toString(16));
  const hits = Memory.scanSync(mod.base, mod.size, PARSE_SIG);
  log('GFx parse prologue matches in LIVE .text: ' + hits.length);
  for (const h of hits) log('  ' + h.address + '  (rva 0x' + h.address.sub(mod.base).toString(16) + ')');
  const at = mod.base.add(PARSE_RVA);
  try {
    const live = at.readByteArray(48);
    const v = new Uint8Array(live);
    const out = [];
    for (let i = 0; i < v.length; i++) out.push(('0' + v[i].toString(16)).slice(-2));
    log('live bytes at rva 0x' + PARSE_RVA.toString(16) + ' (' + at + '): ' + out.join(' '));
    log('static expects              : 40 53 48 83 ec 40 48 8b 41 18 48 8b d9 c6 44 24 30 01 48 83 c1 50 4c 8b 50 20 4c 8b 58 48');
  } catch (e) {
    log('live bytes at rva 0x' + PARSE_RVA.toString(16) + ' UNREADABLE: ' + e);
  }
}

function question2() {
  const steam = Process.findModuleByName('steam_api64.dll');
  if (steam === null) { log('steam_api64.dll not loaded'); return; }
  log('steam_api64.dll ' + steam.base);
  const wanted = [];
  for (const e of steam.enumerateExports()) {
    if (/Friends/i.test(e.name) && /PersonaName|RequestUserInformation|SteamFriends/i.test(e.name)) {
      wanted.push(e);
    }
  }
  for (const e of wanted) log('  export ' + e.name + ' @ ' + e.address);

  const getIface = wanted.filter(function (e) { return /^SteamAPI_SteamFriends_v/.test(e.name); })[0];
  const getName = wanted.filter(function (e) { return /GetFriendPersonaName$/.test(e.name); })[0];
  const request = wanted.filter(function (e) { return /RequestUserInformation$/.test(e.name); })[0];
  if (getIface === undefined || getName === undefined) { log('the two exports needed are not both present'); return; }

  const iface = new NativeFunction(getIface.address, 'pointer', [])();
  log('ISteamFriends* = ' + iface);
  if (iface.isNull()) { log('Steam interface is null -- the API is not initialised in this process'); return; }

  const fnName = new NativeFunction(getName.address, 'pointer', ['pointer', 'uint64']);
  const fnReq = request === undefined ? null : new NativeFunction(request.address, 'bool', ['pointer', 'uint64', 'bool']);
  for (const id of HOSTS) {
    if (fnReq !== null) {
      const pending = fnReq(iface, uint64(id), 1);
      log('  RequestUserInformation(' + id + ') pending=' + pending);
    }
    const namePtr = fnName(iface, uint64(id));
    let name = '<null>';
    try { name = namePtr.isNull() ? '<null>' : namePtr.readUtf8String(); } catch (e) { name = '<unreadable>'; }
    log('  GetFriendPersonaName(' + id + ') = "' + name + '"');
  }
}

function main() {
  const mod = Process.findModuleByName('eldenring.exe');
  if (mod === null) { log('eldenring.exe not found'); return; }
  log('eldenring.exe ' + mod.base + ' size 0x' + mod.size.toString(16));
  question1(mod);
  question2();
}

main();
