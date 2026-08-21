'use strict';
// Frida agent: trace what Seamless Co-op ACTUALLY executes during an invasion.
//
// WHY A TRACE AND NOT STATIC ANALYSIS. `ersc.dll` is Themida-packed, so its on-disk code is
// obfuscated and largely unreadable. But the packer only protects the FILE -- by the time the
// player uses the invasion item, the code is unpacked and running. Watching it run reads
// straight through the protection instead of fighting it.
//
// TWO MODES, cheapest first:
//
//   hooks   Intercept known/suspected ersc RVAs, and on each hit record the arguments and a
//           BACKTRACE. The backtrace is the actual deliverable: it names the ersc functions on
//           the invasion path, which is what static analysis could not give us.
//
//   stalker Follow a thread and record every basic block executed INSIDE ersc.dll. Use when the
//           hook backtraces come back empty or obviously wrong -- Themida's obfuscated frames
//           can defeat stack unwinding. Far heavier, and a packer may object to being stalked,
//           so it is opt-in rather than the default.
//
// READ-ONLY: this agent never writes target memory and never calls into the target.

var ersc = null;
var seq = 0;
var stalking = [];

function resolveErsc(name) {
  var module = Process.findModuleByName(name);
  if (module === null) {
    return null;
  }
  ersc = { base: module.base, size: module.size, end: module.base.add(module.size), name: module.name };
  return ersc;
}

function inErsc(address) {
  return ersc !== null && address.compare(ersc.base) >= 0 && address.compare(ersc.end) < 0;
}

// A frame is only useful if we can say WHERE it is relative to a module. A raw VA in an
// obfuscated blob tells us nothing; `ersc.dll+0x8f4b0` is an address we can go read.
function describe(address) {
  if (inErsc(address)) {
    return ersc.name + '+0x' + address.sub(ersc.base).toString(16);
  }
  var module = Process.findModuleByAddress(address);
  if (module !== null) {
    return module.name + '+0x' + address.sub(module.base).toString(16);
  }
  return address.toString();
}

function backtrace(context) {
  var frames = [];
  // ACCURATE first: it respects unwind info and gives real callers when it works. FUZZY is the
  // fallback because Themida frames routinely have no usable unwind data, and a noisy trace
  // beats none -- but it is LABELLED, because fuzzy frames include false positives and must
  // never be reported as if they were confirmed callers.
  try {
    Thread.backtrace(context, Backtracer.ACCURATE).forEach(function (f) {
      frames.push({ kind: 'accurate', at: describe(f), raw: f.toString() });
    });
  } catch (e) { /* no unwind info; fall through to fuzzy */ }
  if (frames.length === 0) {
    try {
      Thread.backtrace(context, Backtracer.FUZZY).forEach(function (f) {
        frames.push({ kind: 'fuzzy', at: describe(f), raw: f.toString() });
      });
    } catch (e) { /* nothing available */ }
  }
  return frames;
}

rpc.exports = {
  init: function (moduleName) {
    var info = resolveErsc(moduleName);
    if (info === null) {
      return null;
    }
    return { name: info.name, base: info.base.toString(), size: info.size };
  },

  // Hook a list of {rva, label, argCount} inside ersc.dll.
  installHooks: function (specs) {
    if (ersc === null) {
      return { error: 'module not resolved; call init first' };
    }
    var installed = [];
    specs.forEach(function (spec) {
      var address = ersc.base.add(spec.rva);
      try {
        Interceptor.attach(address, {
          onEnter: function (args) {
            var captured = [];
            var count = spec.argCount === undefined ? 4 : spec.argCount;
            for (var i = 0; i < count; i++) {
              try {
                captured.push(args[i].toString());
              } catch (e) {
                captured.push('<unreadable>');
              }
            }
            send({
              type: 'hit',
              seq: seq++,
              label: spec.label,
              at: describe(address),
              rva: spec.rva,
              thread: this.threadId,
              args: captured,
              backtrace: backtrace(this.context),
            });
          },
        });
        installed.push({ label: spec.label, rva: spec.rva, at: describe(address) });
      } catch (e) {
        installed.push({ label: spec.label, rva: spec.rva, error: e.message });
      }
    });
    return installed;
  },

  // Read a native pointer-sized value, for sampling engine state at a hit (e.g. GameMan's
  // lastLoadPosition) without calling anything.
  readBytes: function (addressStr, size) {
    try {
      return Memory.readByteArray(ptr(addressStr), size);
    } catch (e) {
      return null;
    }
  },

  listThreads: function () {
    return Process.enumerateThreads().map(function (t) {
      return { id: t.id, state: t.state, pc: describe(t.context.pc) };
    });
  },

  // Follow threads, recording only blocks that execute inside ersc.dll. `compile` events fire
  // once per basic block, which is the cheap way to get coverage -- `call` events on every call
  // in the process would be far heavier for no extra answer.
  startStalker: function (threadIds) {
    if (ersc === null) {
      return { error: 'module not resolved; call init first' };
    }
    var targets = (threadIds && threadIds.length)
      ? threadIds
      : Process.enumerateThreads().map(function (t) { return t.id; });
    targets.forEach(function (id) {
      try {
        Stalker.follow(id, {
          events: { compile: true },
          onReceive: function (events) {
            var parsed = Stalker.parse(events, { annotate: true, stringify: false });
            var blocks = [];
            parsed.forEach(function (event) {
              // ['compile', start, end]
              if (event[0] !== 'compile') {
                return;
              }
              var start = event[1];
              if (inErsc(start)) {
                blocks.push(describe(start));
              }
            });
            if (blocks.length) {
              send({ type: 'blocks', seq: seq++, thread: id, blocks: blocks });
            }
          },
        });
        stalking.push(id);
      } catch (e) {
        send({ type: 'stalker-error', thread: id, error: e.message });
      }
    });
    return { following: stalking };
  },

  stopStalker: function () {
    stalking.forEach(function (id) {
      try {
        Stalker.unfollow(id);
      } catch (e) { /* thread may already be gone */ }
    });
    Stalker.garbageCollect();
    var stopped = stalking;
    stalking = [];
    return { stopped: stopped };
  },
};
