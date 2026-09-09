// Name a list of x64 MSVC vtables out of the live process, via their RTTI.
//
// # Why this exists
//
// On 2026-09-09 the title-row walk found eight vtables in the TitleTopDialog row vector, none
// of them the `MEMBERFUNCJOB_VTABLE_RVA` the product DLL matches on. An address is not an
// identification: naming those eight is what decides whether the DLL is looking for the wrong
// class or the right class in the wrong place. The 1.17 Ghidra dump carries no symbols, so the
// image cannot answer it -- but the running process can, because MSVC emits the class name
// beside every polymorphic vtable.
//
// Layout, x64: `vtable[-1]` is a RTTICompleteObjectLocator*. For signature 1 the locator holds
// image-base-relative RVAs: `+0x0c` pTypeDescriptor, `+0x14` pClassDescriptor, `+0x14`.. and
// `+0x18` is the locator's own RVA, from which the image base is recovered. The TypeDescriptor's
// decorated name begins at `+0x10`.
//
// Read-only: no hook, no write.
'use strict';

const COL_SIGNATURE = 0x00;
const COL_TYPE_DESCRIPTOR = 0x0c;
const COL_SELF_RVA = 0x14;
const TYPE_DESCRIPTOR_NAME = 0x10;

function rttiName(vtable) {
    const out = { vtable: '0x' + vtable.toString(16) };
    let col;
    try { col = vtable.sub(8).readPointer(); } catch (e) { out.err = 'no locator slot'; return out; }
    if (col.isNull()) { out.err = 'null locator'; return out; }
    let signature, tdRva, selfRva;
    try {
        signature = col.add(COL_SIGNATURE).readU32();
        tdRva = col.add(COL_TYPE_DESCRIPTOR).readU32();
        selfRva = col.add(COL_SELF_RVA).readU32();
    } catch (e) { out.err = 'locator unreadable'; return out; }
    out.signature = signature;
    if (signature !== 1) { out.err = 'not a 64-bit locator (signature ' + signature + ')'; return out; }
    // The locator knows its own RVA, so the image base is the difference -- no assumption about
    // where the module was loaded, which matters because this is read through Wine.
    const imageBase = col.sub(selfRva);
    out.image_base = '0x' + imageBase.toString(16);
    try { out.name = imageBase.add(tdRva).add(TYPE_DESCRIPTOR_NAME).readCString(); }
    catch (e) { out.err = 'name unreadable'; }
    const module = Process.findModuleByAddress(vtable);
    if (module !== null) out.rva = '0x' + vtable.sub(module.base).toString(16);
    return out;
}

recv('vtables', function (message) {
    send({ kind: 'vtable-rtti-names', results: message.vtables.map(v => rttiName(ptr(v))) });
});
