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
        rva: rStart.sub(start).toNumber(),
        size: rEnd.sub(rStart).toNumber(),
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
      var bytes = Memory.readByteArray(address, size);
      return bytes === null ? null : bytes;
    } catch (e) {
      // A page that vanished or refuses to read is reported as a hole, not an abort: losing one
      // page should cost that page, not the whole dump.
      return null;
    }
  },
};
