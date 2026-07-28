// Frida agent for scripts/frida-nudge.py -- the ER switch-reload low-fps live-nudge experiment.
//
// Loaded as raw text by the Python harness (which does NOT embed this source inline, so JS tooling
// -- eslint/tsc/node --check -- can validate this file directly; bd
// no-inline-foreign-language-source-load-from-own-file-2026-07-23).
//
// Exposes an RPC the harness calls: read/write memory, dynamically CALL a native VA, and -- for the
// RE'd singletons whose target is a pointer chain (singleton -> deref -> +offset) -- chainRead /
// chainWrite in a single attach. Addresses are DEOBF/live VAs (base 0x140000000).

'use strict';

// Deobf/dump VAs are expressed against the PE preferred base 0x140000000, but the running exe is
// ASLR-relocated (observed load base e.g. 0x7ff7........). Rebase any input VA that falls in the
// module's preferred range to the actual load base; runtime HEAP pointers (from readPointer / --arg,
// outside the module range) pass through unchanged.
const IMAGE_BASE = ptr('0x140000000');

function moduleVa(vaStr) {
  const p = ptr(vaStr);
  const m = Process.getModuleByName('eldenring.exe');
  if (p.compare(IMAGE_BASE) >= 0 && p.compare(IMAGE_BASE.add(m.size)) < 0) {
    return m.base.add(p.sub(IMAGE_BASE));
  }
  return p;
}

function readAt(p, ty) {
  switch (ty) {
    case 'u8': return p.readU8();
    case 'u16': return p.readU16();
    case 'u32': return p.readU32();
    case 'u64': return p.readU64().toString();
    case 'ptr': return p.readPointer().toString();
    case 'f32': return p.readFloat();
    case 'f64': return p.readDouble();
    case 'hex16': return hexdump(p, { length: 16, header: false });
    default: throw new Error('bad read type ' + ty);
  }
}

function writeAt(p, ty, val) {
  switch (ty) {
    case 'u8': p.writeU8(val | 0); break;
    case 'u32': p.writeU32(val | 0); break;
    case 'f32': p.writeFloat(val); break;
    case 'u64': p.writeU64(uint64(val)); break;
    default: throw new Error('bad write type ' + ty);
  }
  return true;
}

rpc.exports = {
  info: function () {
    const m = Process.getModuleByName('eldenring.exe');
    return { base: m.base.toString(), size: m.size, arch: Process.arch, pid: Process.id };
  },

  readMem: function (vaStr, ty) {
    return readAt(moduleVa(vaStr), ty);
  },

  writeMem: function (vaStr, ty, val) {
    return writeAt(moduleVa(vaStr), ty, val);
  },

  // Pointer-chain read: *(*(singleton) + offset) as `ty`. offset is a JS number (already parsed
  // from hex by the Python side). Used for the RE'd singletons (flipper/GameMan/CSFakeLoadingScreen).
  chainRead: function (singletonStr, offset, ty) {
    return readAt(moduleVa(singletonStr).readPointer().add(offset), ty);
  },

  chainWrite: function (singletonStr, offset, ty, val) {
    return writeAt(moduleVa(singletonStr).readPointer().add(offset), ty, val);
  },

  // Call a native VA. retType/argTypes are Frida NativeFunction type strings
  // (e.g. 'void','int','pointer','float'). args are strings; pointer args are ptr()'d,
  // float/double args parseFloat'd, everything else parseInt(_, 0).
  callNative: function (vaStr, retType, argTypes, args) {
    const fn = new NativeFunction(moduleVa(vaStr), retType, argTypes);
    const conv = args.map(function (a, i) {
      const t = argTypes[i];
      if (t === 'pointer') return ptr(a);
      if (t === 'float' || t === 'double') return parseFloat(a);
      return parseInt(a, 0) | 0;
    });
    const r = fn.apply(null, conv);
    return (r === undefined || r === null) ? 'void' : r.toString();
  },
};
