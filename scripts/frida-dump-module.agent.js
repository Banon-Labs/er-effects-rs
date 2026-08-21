'use strict';
// Frida agent: dump a MODULE'S LIVE IMAGE out of the running game.
//
// WHY THIS EXISTS. `ersc.dll` (Seamless Co-op) is Themida-packed, so its on-disk bytes are
// obfuscated and most of its code cannot be read statically at all. The unpacked code exists
// only in memory, after the packer's loader stub has run. Dumping the live image is therefore
// not a convenience -- it is the only way to read Seamless's own implementation of anything.
//
// The dump is written as a FLAT image: file offset == RVA, exactly the convention
// `eldenring-deobf.bin` already uses in this repo, so a VA is just `base + offset` and every
// existing habit transfers. Unreadable pages become zeros rather than shifting everything after
// them, because a dump whose offsets silently slide is worse than one with holes.
//
// READ-ONLY. This agent never writes target memory and never calls into the target.

// Frida 17 removed NativePointer.toNumber(). Kept as one helper so a future API change has a
// single place to be fixed rather than being spread across the range loop.
function ptrToNumber(p) {
  return Number(p.toString());
}

// Kept across calls so a systematic read failure is reportable rather than silent.
var firstReadError = null;
var readFailures = 0;

rpc.exports = {
  // Every loaded module, so the caller can see what is actually present before choosing one.
  listModules: function () {
    return Process.enumerateModules().map(function (m) {
      return { name: m.name, base: m.base.toString(), size: m.size, path: m.path };
    });
  },

  // Metadata plus the readable page ranges INSIDE the module. Ranges are what makes the dump
  // possible: a packed module has holes and guard pages, and reading straight through would
  // fault.
  moduleInfo: function (name) {
    var m = Process.findModuleByName(name);
    if (m === null) {
      return null;
    }
    var start = m.base;
    var end = m.base.add(m.size);
    var ranges = Process.enumerateRanges('r--').filter(function (r) {
      return r.base.compare(end) < 0 && r.base.add(r.size).compare(start) > 0;
    }).map(function (r) {
      // Clip to the module: enumerateRanges can hand back a region that starts before the
      // module or runs past its end, and copying those would corrupt the RVA mapping.
      var rStart = r.base.compare(start) < 0 ? start : r.base;
      var rEnd = r.base.add(r.size).compare(end) > 0 ? end : r.base.add(r.size);
      return {
        base: rStart.toString(),
        // `NativePointer.toNumber()` was REMOVED in Frida 17 (this failed as
        // "TypeError: not a function" against gadget 17.16.4). `toString()` yields "0x...", which
        // Number() parses exactly, and both values here are offsets/sizes within one module -- far
        // inside the 2^53 range where that conversion is lossless.
        rva: ptrToNumber(rStart.sub(start)),
        size: ptrToNumber(rEnd.sub(rStart)),
        protection: r.protection,
      };
    }).filter(function (r) {
      return r.size > 0;
    });
    return {
      name: m.name,
      base: m.base.toString(),
      size: m.size,
      path: m.path,
      ranges: ranges,
    };
  },

  // One chunk, returned as binary. Chunked because a multi-megabyte single message is a good
  // way to stall the target while it is frozen.
  readChunk: function (addressStr, size) {
    var address = ptr(addressStr);
    try {
      // Frida 17 removed the legacy `Memory.readByteArray(addr, size)` in favour of the pointer
      // method. Calling the removed form threw for EVERY chunk, and the catch below turned that
      // into a silent all-zero dump that still reported success -- a broken API dressed up as a
      // module that refused to be read. See `firstReadError`.
      var bytes = address.readByteArray(size);
      return bytes === null ? null : bytes;
    } catch (e) {
      // A page that vanished or refuses to read is a hole, not an abort: losing one page should
      // cost that page, not the whole dump. But the FIRST failure is kept so a systematic failure
      // (wrong API, detached session, bad base) can be told apart from ordinary unreadable pages.
      if (firstReadError === null) {
        firstReadError = String(e && e.message ? e.message : e);
      }
      readFailures += 1;
      return null;
    }
  },

  // What went wrong, if anything did. The dumper prints this when a dump comes back mostly empty,
  // so "the module would not read" is never reported without the reason.
  readDiagnostics: function () {
    return { failures: readFailures, firstError: firstReadError };
  },
};
