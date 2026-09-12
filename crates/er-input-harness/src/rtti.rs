//! In-process MSVC RTTI reader: name the class of a live game object rather than guess it.
//!
//! Why this exists. Every pointer this harness walks used to be validated by one test -- "is it
//! bigger than `0x10000`" -- and that test is satisfied by text. `currentTopMenuJob+0x130` read
//! `0x2f003a00320063` on 1.17.1, the UTF-16 for `c2:/aet/aet050/A...`, and the harness handed that
//! asset path to `top_menu_id`, to the `GridControl` scan and to the `OptionSetting` tab chain as
//! though it were a `CS::MenuWindow` (bd er-effects-rs-h09b). The offset was not wrong; the object
//! was. `+0x130` is `owningMenuWindow`, the last field of a `CS::MenuWindowJob`, and
//! `CSPopupMenu::StartTopMenuJob` never puts a `MenuWindowJob` at `currentTopMenuJob` -- it puts a
//! `0x58`-byte wrapper job there, so the read was `0xd8` bytes past the end of the object.
//!
//! The fix has to ask what the object is, and the game already carries the answer: it is compiled
//! with `/GR`, its own `DLPanic` paths call `GetRuntimeClassName`, and `scripts/er-rtti-map.py`
//! parses the same structures offline. Doing it live costs a handful of fault-safe reads and needs
//! no address constant at all, which is the other reason to prefer it -- a class name does not move
//! between builds the way a vtable address does.
//!
//! Layout (x64 MSVC), the three hops this module makes:
//!
//! ```text
//!   object      [ vtable ][ ... ]
//!   vtable      [ COL* ][ vfunc0 ] ...            -- the COL pointer sits at vtable-8
//!   COL         { u32 signature; u32 offset; u32 cdOffset;
//!                 u32 pTypeDescriptor; u32 pClassDescriptor; u32 pSelf }   -- all RVAs
//!   TypeDesc    { void* pVFTable; void* spare; char name[] }               -- name at +0x10
//!   CHD         { u32 signature; u32 attributes; u32 numBaseClasses; u32 pBaseClassArray }
//!   BaseArray   u32[numBaseClasses] -> BaseClassDescriptor, whose +0x00 is its own TypeDescriptor
//! ```
//!
//! `pSelf` is what makes the walk safe rather than hopeful: a genuine `CompleteObjectLocator`
//! stores its own RVA, so requiring `base + pSelf == col` rejects a coincidence outright. Signature
//! 1 (x64) is checked too, and so is the `.?AV` prefix every mangled class name starts with.

/// Fault-safe reads, indirected so the walk can be exercised on a host build against a fake address
/// space. The live implementation is `ReadProcessMemory` on this process's pseudo-handle; the tests
/// hand it a byte map. Nothing here ever dereferences a raw pointer.
pub(crate) trait GameMemory {
    fn read_usize(&self, addr: usize) -> Option<usize>;
    fn read_u32(&self, addr: usize) -> Option<u32>;
    /// Fill as much of `out` as is mapped, returning how many bytes were read. One call, because
    /// the only caller is the name reader and a per-byte loop there is 160 syscalls per candidate.
    fn read_bytes(&self, addr: usize, out: &mut [u8]) -> usize;
}

/// The live process: every read goes through the same `ReadProcessMemory` idiom the rest of this
/// DLL uses, so a stale or garbage pointer answers `None` instead of faulting the game thread.
pub(crate) struct LiveMemory;

impl GameMemory for LiveMemory {
    fn read_usize(&self, addr: usize) -> Option<usize> {
        unsafe { crate::win32::read_usize(addr) }
    }
    fn read_u32(&self, addr: usize) -> Option<u32> {
        unsafe { crate::win32::read_u32(addr) }
    }
    fn read_bytes(&self, addr: usize, out: &mut [u8]) -> usize {
        unsafe { crate::win32::read_bytes(addr, out) }
    }
}

/// `CompleteObjectLocator*` sits one qword before the first virtual function.
const COL_POINTER_OFFSET_FROM_VTABLE: usize = 8;
/// x64 `CompleteObjectLocator` signature. Signature 0 is the x86 form, whose fields are pointers
/// rather than RVAs, so accepting it would misread every field after it.
const COL_SIGNATURE_X64: u32 = 1;
const COL_TYPE_DESCRIPTOR_RVA_OFFSET: usize = 0x0c;
const COL_CLASS_DESCRIPTOR_RVA_OFFSET: usize = 0x10;
const COL_SELF_RVA_OFFSET: usize = 0x14;
/// The mangled name inside a `TypeDescriptor`.
const TYPE_DESCRIPTOR_NAME_OFFSET: usize = 0x10;
const CHD_BASE_CLASS_COUNT_OFFSET: usize = 0x08;
const CHD_BASE_CLASS_ARRAY_RVA_OFFSET: usize = 0x0c;
/// A `BaseClassDescriptor`'s own `TypeDescriptor` is its first field.
const BASE_CLASS_DESCRIPTOR_TYPE_RVA_OFFSET: usize = 0x00;
/// Upper bound on the base-class list walked by [`derives_from`]. `CS::MenuWindow`'s own chain is
/// three deep and the widest class in this image is far inside this; a count past it is a sign the
/// descriptor is not one, and the walk stops rather than reading a few thousand qwords.
const MAX_BASE_CLASSES: u32 = 64;
/// Longest mangled name this reader will materialise. The names it matches on are 21 and 24 bytes
/// (`.?AVMenuWindow@CS@@`, `.?AVMenuWindowJob@CS@@`); the template-heavy classes in this image run
/// to about 200, and one that does not fit simply fails to match, which is the safe direction.
pub(crate) const CLASS_NAME_MAX: usize = 160;

/// A mangled RTTI class name read out of the live image, e.g. `.?AVMenuWindowJob@CS@@`.
///
/// A fixed buffer rather than a `String` because this is read on the game thread, potentially once
/// per candidate object in a graph walk, and a per-object heap allocation there is a cost with no
/// purpose -- the name is compared and discarded.
#[derive(Clone, Copy)]
pub(crate) struct ClassName {
    bytes: [u8; CLASS_NAME_MAX],
    len: usize,
}

impl ClassName {
    pub(crate) fn as_str(&self) -> &str {
        // The bytes were screened to printable ASCII as they were read, so this cannot fail; an
        // unexpected byte would have ended the read before it was stored.
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl core::fmt::Display for ClassName {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The `CompleteObjectLocator` of a live object, validated by its own self-RVA, or `None` when the
/// pointer is not a polymorphic object of this image at all.
///
/// Four reads and no name, which is why the graph walk screens candidates with this and only asks
/// for the name of the nodes it actually dequeues.
pub(crate) fn locator<M: GameMemory>(mem: &M, base: usize, obj: usize) -> Option<usize> {
    if obj == 0 || !obj.is_multiple_of(core::mem::align_of::<usize>()) {
        return None;
    }
    let vtable = mem.read_usize(obj)?;
    if vtable <= base || !vtable.is_multiple_of(core::mem::align_of::<usize>()) {
        return None;
    }
    let col = mem.read_usize(vtable - COL_POINTER_OFFSET_FROM_VTABLE)?;
    if col <= base {
        return None;
    }
    if mem.read_u32(col)? != COL_SIGNATURE_X64 {
        return None;
    }
    // The self-check: a genuine locator records its own RVA, so this equality is what separates an
    // object from a qword that merely looked like one.
    let self_rva = mem.read_u32(col + COL_SELF_RVA_OFFSET)?;
    (base + self_rva as usize == col).then_some(col)
}

/// Read the mangled name out of a `TypeDescriptor` at `base + rva`.
fn name_at<M: GameMemory>(mem: &M, base: usize, type_descriptor_rva: u32) -> Option<ClassName> {
    let name_addr = base + type_descriptor_rva as usize + TYPE_DESCRIPTOR_NAME_OFFSET;
    let mut out = ClassName {
        bytes: [0; CLASS_NAME_MAX],
        len: 0,
    };
    let mut raw = [0u8; CLASS_NAME_MAX];
    let read = mem.read_bytes(name_addr, &mut raw);
    for (index, byte) in raw[..read].iter().copied().enumerate() {
        if byte == 0 {
            break;
        }
        // Printable ASCII only. A mangled name is `.?AV<name>@<scopes>@@`, so a byte outside this
        // range means the descriptor is not one and the candidate is rejected rather than
        // half-read.
        if !(0x20..0x7f).contains(&byte) {
            return None;
        }
        out.bytes[index] = byte;
        out.len = index + 1;
    }
    (out.len > 4 && out.bytes.starts_with(b".?AV")).then_some(out)
}

/// The class name of a live object, or `None` when it is not a polymorphic object of this image.
pub(crate) fn class_name<M: GameMemory>(mem: &M, base: usize, obj: usize) -> Option<ClassName> {
    let col = locator(mem, base, obj)?;
    let type_rva = mem.read_u32(col + COL_TYPE_DESCRIPTOR_RVA_OFFSET)?;
    name_at(mem, base, type_rva)
}

/// True when `obj` is exactly `wanted`, by mangled class name.
pub(crate) fn is_class<M: GameMemory>(mem: &M, base: usize, obj: usize, wanted: &str) -> bool {
    class_name(mem, base, obj).is_some_and(|n| n.as_str() == wanted)
}

/// True when `obj` is `wanted` or derives from it.
///
/// Needed because `CS::MenuWindow` is not the class any live window actually is: 107 classes in
/// this image derive from it (`OptionSettingTopDialog`, `ProfileSelectDialog`, `TitleTopDialog`,
/// `MessageBoxDialog`, ...), measured from the 1.17.1 image's class-hierarchy descriptors. A vtable
/// equality check against `CS::MenuWindow` would therefore reject every real window, which is the
/// trap this function exists to avoid.
pub(crate) fn derives_from<M: GameMemory>(mem: &M, base: usize, obj: usize, wanted: &str) -> bool {
    let Some(col) = locator(mem, base, obj) else {
        return false;
    };
    let Some(chd_rva) = mem.read_u32(col + COL_CLASS_DESCRIPTOR_RVA_OFFSET) else {
        return false;
    };
    let chd = base + chd_rva as usize;
    let Some(count) = mem.read_u32(chd + CHD_BASE_CLASS_COUNT_OFFSET) else {
        return false;
    };
    if count == 0 || count > MAX_BASE_CLASSES {
        return false;
    }
    let Some(array_rva) = mem.read_u32(chd + CHD_BASE_CLASS_ARRAY_RVA_OFFSET) else {
        return false;
    };
    let array = base + array_rva as usize;
    for index in 0..count as usize {
        let Some(descriptor_rva) = mem.read_u32(array + index * 4) else {
            return false;
        };
        let Some(type_rva) =
            mem.read_u32(base + descriptor_rva as usize + BASE_CLASS_DESCRIPTOR_TYPE_RVA_OFFSET)
        else {
            continue;
        };
        if name_at(mem, base, type_rva).is_some_and(|n| n.as_str() == wanted) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests;
