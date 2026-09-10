// Find the live instances of a game class by name, using the RTTI FromSoft ships in the image.
//
// This exists because a struct offset carried forward from another build is a guess, and an object
// graph walk is a fishing trip. The RTTI chain is neither: a mangled class name sits inside a
// TypeDescriptor, a CompleteObjectLocator points at that TypeDescriptor by RVA, a vtable's [-1]
// slot points at the locator, and every live instance carries that vtable in its first word. Each
// link is a memory scan, so the answer is a fact about the running process.
//
// It earned its keep on 2026-09-10: asked for `CSBreakInManager` it found the class does not exist
// in this build at all, which is why `CSNetMan+0xa8 -> BreakInManager -> targets` read zero and
// could never have read anything else. The break-in state belongs to `FNBreakInImpl@FromNet`.
//
// Edit WANTED and save; the watcher reloads the file in place.
//
//   uv run --with frida python3 scripts/er-frida-watch.py \
//     --agent scripts/frida/er-rtti-find-instances.js
//
// Names are the MSVC mangling, so a class `Foo` in namespace `Bar` is `.?AVFoo@Bar@@`.
const WANTED = ['.?AVFNBreakInImpl@FromNet@@', '.?AVFNClientImpl@FromNet@@'];
// A SteamID64 for an individual account always begins with these 32 bits, which makes a plain
// field dump able to say "this one is a person".
const STEAM_HIGH = 0x01100001;

let BASE = null, SIZE = 0, END = null;

function log(s) { send({ line: s }); }
function inImage(p) { return p.compare(BASE) >= 0 && p.compare(END) < 0; }

function bytesOf(p, n) {
  const out = [];
  for (let i = 0; i < n; i++) out.push(('0' + p.add(i).readU8().toString(16)).slice(-2));
  return out.join(' ');
}
function u32Pattern(v) {
  const out = [];
  for (let s = 0; s < 32; s += 8) out.push(('0' + ((v >>> s) & 0xff).toString(16)).slice(-2));
  return out.join(' ');
}
function rttiName(objPtr) {
  let vt;
  try { vt = objPtr.readPointer(); } catch (e) { return null; }
  if (vt.isNull() || !inImage(vt)) return null;
  let col;
  try { col = vt.sub(8).readPointer(); } catch (e) { return null; }
  if (col.isNull() || !inImage(col)) return null;
  try {
    if (col.readU32() !== 1) return null;
    if (col.sub(BASE).toUInt32() !== col.add(0x14).readU32()) return null;
    return BASE.add(col.add(0x0c).readU32()).add(0x10).readCString();
  } catch (e) { return null; }
}

// name -> every type descriptor in the image carrying exactly that name.
function typeDescriptorsFor(name) {
  const ascii = [];
  for (let i = 0; i < name.length; i++) ascii.push(('0' + name.charCodeAt(i).toString(16)).slice(-2));
  const out = [];
  for (const h of Memory.scanSync(BASE, SIZE, ascii.join(' '))) {
    let text;
    try { text = h.address.readCString(); } catch (e) { continue; }
    if (text !== name) continue;
    out.push(h.address.sub(0x10));
  }
  return out;
}

function vtablesFor(td) {
  const rva = td.sub(BASE).toUInt32();
  const out = [];
  for (const ch of Memory.scanSync(BASE, SIZE, u32Pattern(rva))) {
    const col = ch.address.sub(0x0c);
    if (col.and(3).toUInt32() !== 0) continue;
    let sig;
    try { sig = col.readU32(); } catch (e) { continue; }
    if (sig !== 1) continue;
    try { if (col.sub(BASE).toUInt32() !== col.add(0x14).readU32()) continue; } catch (e) { continue; }
    for (const vh of Memory.scanSync(BASE, SIZE, bytesOf(col, 8))) {
      if (vh.address.and(7).toUInt32() !== 0) continue;
      const vt = vh.address.add(8);
      let first;
      try { first = vt.readPointer(); } catch (e) { continue; }
      if (!inImage(first)) continue;
      out.push(vt);
    }
  }
  return out;
}

function instancesOf(vt, cap) {
  const pat = bytesOf(vt, 8);
  const out = [];
  for (const r of Process.enumerateRanges('rw-')) {
    if (r.size > 0x4000000) continue;
    let hits;
    try { hits = Memory.scanSync(r.base, r.size, pat); } catch (e) { continue; }
    for (const h of hits) {
      if (h.address.and(7).toUInt32() !== 0) continue;
      out.push(h.address);
      if (out.length >= cap) return out;
    }
  }
  return out;
}

function dump(obj, span) {
  for (let off = 0; off < span; off += 8) {
    let v;
    try { v = obj.add(off).readPointer(); } catch (e) { continue; }
    if (v.isNull()) continue;
    if (v.shr(32).toNumber() === STEAM_HIGH) { log('      +0x' + off.toString(16) + ' STEAMID ' + v.toString(10)); continue; }
    const n = rttiName(v);
    if (n !== null) { log('      +0x' + off.toString(16) + ' -> ' + n); continue; }
    if (v.compare(ptr('0x10000')) > 0 && v.shr(48).toNumber() === 0) log('      +0x' + off.toString(16) + ' = ' + v);
  }
}

function main() {
  const mod = Process.findModuleByName('eldenring.exe');
  if (mod === null) { log('eldenring.exe not found'); return; }
  BASE = mod.base; SIZE = mod.size; END = BASE.add(SIZE);
  log('image ' + BASE + ' + 0x' + SIZE.toString(16));

  for (const name of WANTED) {
    const tds = typeDescriptorsFor(name);
    log(name + ': ' + tds.length + ' type descriptor(s)');
    if (tds.length === 0) {
      log('  the class does not exist in this build. That is an answer, not a miss.');
      continue;
    }
    for (const td of tds) {
      const vts = vtablesFor(td);
      log('  td=' + td + ' vtables: ' + vts.length + ' ' + vts.join(' '));
      for (const vt of vts) {
        const inst = instancesOf(vt, 8);
        log('  vtable ' + vt + ' -> ' + inst.length + ' instance(s)');
        for (const i of inst) {
          log('    instance @ ' + i);
          dump(i, 0x180);
        }
      }
    }
  }
}

main();
