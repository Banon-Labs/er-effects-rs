//! Post-translation SPIR-V fixups for the passthrough path.
//!
//! dxil-spirv emits `localInstance = gl_InstanceIndex - gl_BaseInstance` (and similar
//! for `gl_BaseVertex`) using the `DrawParameters` capability. Those builtins are only
//! defined when the device enables `VK_KHR_shader_draw_parameters`; wgpu's passthrough
//! path does **not** enable it, so the builtin reads back garbage and the subtraction
//! yields a ~4-billion "instance index" that indexes the engine's instance SSBO far out
//! of bounds — a deterministic GPU fault (and a CPU segfault under lavapipe).
//!
//! For every draw this harness issues, `firstInstance`/`firstVertex` are `0`, so the
//! correct value of both builtins is `0`. [`neutralize_draw_parameters`] rewrites each
//! load of those builtins to a copy of the constant `0` — both safe and correct here —
//! removing the garbage-index fault without otherwise altering the native shader.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

const OP_DECORATE: u16 = 71;
const OP_LOAD: u16 = 61;
const OP_COPY_OBJECT: u16 = 83;
const OP_CONSTANT: u16 = 43;
const OP_TYPE_INT: u16 = 21;
const OP_VARIABLE: u16 = 59;
const OP_ACCESS_CHAIN: u16 = 65;
const OP_IN_BOUNDS_ACCESS_CHAIN: u16 = 66;
const DECO_BUILTIN: u32 = 11;
const DECO_BINDING: u32 = 33;
const DECO_DESCRIPTOR_SET: u32 = 34;
const DECO_NON_WRITABLE: u32 = 24;
const BUILTIN_BASE_VERTEX: u32 = 4424;
const BUILTIN_BASE_INSTANCE: u32 = 4425;
const STORAGE_CLASS_STORAGE_BUFFER: u32 = 12;
const SPIRV_MAGIC: u32 = 0x0723_0203;

fn rd(spv: &[u8], word: usize) -> u32 {
    u32::from_le_bytes([
        spv[word * 4],
        spv[word * 4 + 1],
        spv[word * 4 + 2],
        spv[word * 4 + 3],
    ])
}
fn wr(spv: &mut [u8], word: usize, v: u32) {
    spv[word * 4..word * 4 + 4].copy_from_slice(&v.to_le_bytes());
}

/// Force `gl_BaseInstance`/`gl_BaseVertex` loads to `0`. Returns the number of loads
/// rewritten (0 if the shader doesn't use these builtins, or no matching `uint 0`
/// constant exists). No-op on non-SPIR-V input.
pub fn neutralize_draw_parameters(spv: &mut [u8]) -> usize {
    if spv.len() < 20 || rd(spv, 0) != SPIRV_MAGIC {
        return 0;
    }
    let total = spv.len() / 4;

    // Pass 1: collect uint type ids, `uint 0` constants (by type), and the variable ids
    // decorated as BaseInstance/BaseVertex.
    let mut uint_types: HashSet<u32> = HashSet::new();
    let mut zero_by_type: HashMap<u32, u32> = HashMap::new();
    let mut builtin_vars: HashSet<u32> = HashSet::new();
    let mut i = 5;
    while i < total {
        let word0 = rd(spv, i);
        let wc = (word0 >> 16) as usize;
        let op = (word0 & 0xffff) as u16;
        if wc == 0 || i + wc > total {
            break;
        }
        match op {
            OP_TYPE_INT if wc >= 4 => {
                // result, width, signedness — unsigned 32-bit.
                if rd(spv, i + 2) == 32 && rd(spv, i + 3) == 0 {
                    uint_types.insert(rd(spv, i + 1));
                }
            }
            OP_CONSTANT if wc >= 4 => {
                // resulttype, result, value.
                let ty = rd(spv, i + 1);
                if uint_types.contains(&ty) && rd(spv, i + 3) == 0 {
                    zero_by_type.entry(ty).or_insert_with(|| rd(spv, i + 2));
                }
            }
            OP_DECORATE if wc >= 4 => {
                if rd(spv, i + 2) == DECO_BUILTIN {
                    let b = rd(spv, i + 3);
                    if b == BUILTIN_BASE_INSTANCE || b == BUILTIN_BASE_VERTEX {
                        builtin_vars.insert(rd(spv, i + 1));
                    }
                }
            }
            _ => {}
        }
        i += wc;
    }
    if builtin_vars.is_empty() || zero_by_type.is_empty() {
        return 0;
    }

    // Pass 2: rewrite `OpLoad <uint> <res> <builtinVar>` -> `OpCopyObject <uint> <res> <const0>`.
    // Both are 4-word instructions, so this is an in-place edit.
    let mut patched = 0;
    let mut i = 5;
    while i < total {
        let word0 = rd(spv, i);
        let wc = (word0 >> 16) as usize;
        let op = (word0 & 0xffff) as u16;
        if wc == 0 || i + wc > total {
            break;
        }
        if op == OP_LOAD && wc == 4 && builtin_vars.contains(&rd(spv, i + 3)) {
            let result_type = rd(spv, i + 1);
            if let Some(&zero) = zero_by_type.get(&result_type) {
                wr(spv, i, (4u32 << 16) | OP_COPY_OBJECT as u32);
                wr(spv, i + 3, zero);
                patched += 1;
            }
        }
        i += wc;
    }
    patched
}

/// Force every scalar `uint` load out of a **read-only** storage buffer to `0`.
///
/// Elden Ring object vertex shaders read `g_InstanceIndexBuffer` (a read-only SSBO) to
/// map `gl_InstanceIndex` to an engine instance slot. For a single-instance studio draw
/// the slot is always `0` (exactly what a correct instance buffer would hold), and the
/// SSBO descriptor doesn't bind reliably under wgpu passthrough on lavapipe (null base →
/// segfault). Rewriting those loads to `0` removes the dependency while keeping the slot
/// correct. Only `NonWritable` SSBOs are touched, and only `uint`-typed loads through a
/// (possibly nested) access chain rooted at such a buffer. Returns the count rewritten.
pub fn force_readonly_ssbo_loads_zero(spv: &mut [u8]) -> usize {
    if spv.len() < 20 || rd(spv, 0) != SPIRV_MAGIC {
        return 0;
    }
    let total = spv.len() / 4;

    // Pass 1: uint types + `uint 0` consts; NonWritable ids; StorageBuffer variable ids.
    let mut uint_types: HashSet<u32> = HashSet::new();
    let mut zero_by_type: HashMap<u32, u32> = HashMap::new();
    let mut non_writable: HashSet<u32> = HashSet::new();
    let mut ssbo_vars: HashSet<u32> = HashSet::new();
    let mut i = 5;
    while i < total {
        let word0 = rd(spv, i);
        let wc = (word0 >> 16) as usize;
        let op = (word0 & 0xffff) as u16;
        if wc == 0 || i + wc > total {
            break;
        }
        match op {
            OP_TYPE_INT if wc >= 4 => {
                if rd(spv, i + 2) == 32 && rd(spv, i + 3) == 0 {
                    uint_types.insert(rd(spv, i + 1));
                }
            }
            OP_CONSTANT if wc >= 4 => {
                let ty = rd(spv, i + 1);
                if uint_types.contains(&ty) && rd(spv, i + 3) == 0 {
                    zero_by_type.entry(ty).or_insert_with(|| rd(spv, i + 2));
                }
            }
            OP_DECORATE if wc >= 3 && rd(spv, i + 2) == DECO_NON_WRITABLE => {
                non_writable.insert(rd(spv, i + 1));
            }
            OP_VARIABLE if wc >= 4 && rd(spv, i + 3) == STORAGE_CLASS_STORAGE_BUFFER => {
                ssbo_vars.insert(rd(spv, i + 2));
            }
            _ => {}
        }
        i += wc;
    }
    // Keep only read-only SSBO variables.
    ssbo_vars.retain(|v| non_writable.contains(v));
    if ssbo_vars.is_empty() || zero_by_type.is_empty() {
        return 0;
    }

    // Pass 2: access-chain result ids rooted at a read-only SSBO var (transitively).
    let mut from_ssbo: HashSet<u32> = HashSet::new();
    let mut i = 5;
    while i < total {
        let word0 = rd(spv, i);
        let wc = (word0 >> 16) as usize;
        let op = (word0 & 0xffff) as u16;
        if wc == 0 || i + wc > total {
            break;
        }
        if (op == OP_ACCESS_CHAIN || op == OP_IN_BOUNDS_ACCESS_CHAIN) && wc >= 4 {
            let base = rd(spv, i + 3);
            if ssbo_vars.contains(&base) || from_ssbo.contains(&base) {
                from_ssbo.insert(rd(spv, i + 2));
            }
        }
        i += wc;
    }

    // Pass 3: rewrite `OpLoad <uint> <res> <chain-into-ssbo>` -> `OpCopyObject <uint> <res> 0`.
    let mut patched = 0;
    let mut i = 5;
    while i < total {
        let word0 = rd(spv, i);
        let wc = (word0 >> 16) as usize;
        let op = (word0 & 0xffff) as u16;
        if wc == 0 || i + wc > total {
            break;
        }
        if op == OP_LOAD && wc == 4 && from_ssbo.contains(&rd(spv, i + 3)) {
            if let Some(&zero) = zero_by_type.get(&rd(spv, i + 1)) {
                wr(spv, i, (4u32 << 16) | OP_COPY_OBJECT as u32);
                wr(spv, i + 3, zero);
                patched += 1;
            }
        }
        i += wc;
    }
    patched
}

/// Remap descriptor `Binding` numbers within each set to a contiguous `0..N` range,
/// preserving relative order. lavapipe + wgpu + passthrough NULLS descriptors when the
/// binding indices are SPARSE (e.g. `{4,5,8,10,11,12,25}` → segfault), but binds a
/// contiguous `{0..6}` set correctly (verified). Returns `(old (set,binding), new
/// (set,binding))` pairs so the caller can remap its bind-group entries and buffer writes
/// to match the rewritten shader.
///
/// Single-module: compacts each set's bindings independently. For a shared vertex+pixel
/// resource set, the two modules must be compacted against a common map (a vertex-isolation
/// draw, which binds only the vertex resources, is unaffected).
pub fn compact_descriptor_bindings(spv: &mut [u8]) -> Vec<((u32, u32), (u32, u32))> {
    if spv.len() < 20 || rd(spv, 0) != SPIRV_MAGIC {
        return Vec::new();
    }
    let total = spv.len() / 4;
    let mut id_set: HashMap<u32, u32> = HashMap::new();
    let mut id_binding: HashMap<u32, (u32, usize)> = HashMap::new(); // id -> (binding, value word)
    let mut i = 5;
    while i < total {
        let word0 = rd(spv, i);
        let wc = (word0 >> 16) as usize;
        let op = (word0 & 0xffff) as u16;
        if wc == 0 || i + wc > total {
            break;
        }
        if op == OP_DECORATE && wc >= 4 {
            let target = rd(spv, i + 1);
            match rd(spv, i + 2) {
                DECO_DESCRIPTOR_SET => {
                    id_set.insert(target, rd(spv, i + 3));
                }
                DECO_BINDING => {
                    id_binding.insert(target, (rd(spv, i + 3), i + 3));
                }
                _ => {}
            }
        }
        i += wc;
    }
    // Group bindings by set, then assign a contiguous index in ascending-binding order.
    let mut by_set: BTreeMap<u32, Vec<(u32, usize)>> = BTreeMap::new();
    for (&id, &(b, word)) in &id_binding {
        let s = id_set.get(&id).copied().unwrap_or(0);
        by_set.entry(s).or_default().push((b, word));
    }
    let mut mapping = Vec::new();
    for (&s, list) in by_set.iter_mut() {
        list.sort_by_key(|x| x.0);
        for (new_b, &(old_b, word)) in list.iter().enumerate() {
            let new_b = new_b as u32;
            if new_b != old_b {
                wr(spv, word, new_b);
            }
            mapping.push(((s, old_b), (s, new_b)));
        }
    }
    mapping
}

/// Compact descriptor `Binding` numbers across MULTIPLE SPIR-V modules using a single
/// shared map, so a resource used by more than one stage (e.g. `cbSceneParam`, shared by
/// the vertex and pixel shader) keeps the SAME compacted binding in every module. Same
/// motivation as [`compact_descriptor_bindings`] (lavapipe nulls sparse bindings), but
/// consistent across a vertex+pixel pipeline. Returns the shared old→new mapping.
pub fn compact_descriptor_bindings_unified(
    modules: &mut [&mut [u8]],
) -> Vec<((u32, u32), (u32, u32))> {
    // Pass 1: per module, find each resource's set + binding-value word; accumulate the
    // global union of (set, old_binding) and where to rewrite each.
    let mut word_locs: Vec<(usize, usize, u32, u32)> = Vec::new(); // (module, word, set, old)
    let mut union: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for (mi, spv) in modules.iter().enumerate() {
        if spv.len() < 20 || rd(spv, 0) != SPIRV_MAGIC {
            continue;
        }
        let total = spv.len() / 4;
        let mut id_set: HashMap<u32, u32> = HashMap::new();
        let mut id_binding: HashMap<u32, (u32, usize)> = HashMap::new();
        let mut i = 5;
        while i < total {
            let word0 = rd(spv, i);
            let wc = (word0 >> 16) as usize;
            let op = (word0 & 0xffff) as u16;
            if wc == 0 || i + wc > total {
                break;
            }
            if op == OP_DECORATE && wc >= 4 {
                let target = rd(spv, i + 1);
                match rd(spv, i + 2) {
                    DECO_DESCRIPTOR_SET => {
                        id_set.insert(target, rd(spv, i + 3));
                    }
                    DECO_BINDING => {
                        id_binding.insert(target, (rd(spv, i + 3), i + 3));
                    }
                    _ => {}
                }
            }
            i += wc;
        }
        for (&id, &(b, word)) in &id_binding {
            let s = id_set.get(&id).copied().unwrap_or(0);
            union.entry(s).or_default().insert(b);
            word_locs.push((mi, word, s, b));
        }
    }
    // Build one shared map: per set, ascending old binding -> 0..N.
    let mut newmap: HashMap<(u32, u32), u32> = HashMap::new();
    let mut mapping = Vec::new();
    for (&s, olds) in &union {
        for (new_b, &old_b) in olds.iter().enumerate() {
            newmap.insert((s, old_b), new_b as u32);
            mapping.push(((s, old_b), (s, new_b as u32)));
        }
    }
    // Pass 2: rewrite every module's binding words through the shared map.
    for (mi, word, s, old_b) in word_locs {
        if let Some(&nb) = newmap.get(&(s, old_b)) {
            if nb != old_b {
                wr(modules[mi], word, nb);
            }
        }
    }
    mapping
}

/// Assign EVERY resource across the given modules a globally-unique, contiguous binding in
/// set 0. dxil-spirv emits the D3D register model where different descriptor *types* reuse
/// the same binding (`t1`/`s1`/`b1` all → binding 1) and different cbuffers across stages
/// reuse registers — both legal for vkd3d-proton's descriptor management but a hard
/// collision in a single merged Vulkan/wgpu pipeline (and [`compact_descriptor_bindings`]
/// preserves the collision). This gives each resource variable a distinct binding (no
/// sharing), so the merged pipeline is valid and the bindings are contiguous (lavapipe
/// needs that). Returns, per module, a map `(old_set, old_binding) -> new_binding` — exact
/// for collision-free modules (the vertex shader), so callers can locate the cbuffers they
/// write the transform/lighting into.
///
/// Returns, per module, a list of `(old_binding, new_binding, storage_class)` for every
/// resource (`storage_class`: Uniform=2 cbuffer, StorageBuffer=12, UniformConstant=0
/// texture/sampler) — so callers can locate a specific cbuffer (e.g. the `Uniform` at old
/// register 2) even when D3D registers collided across types.
pub fn assign_unique_bindings(modules: &mut [&mut [u8]]) -> Vec<Vec<(u32, u32, u32)>> {
    const OP_TYPE_POINTER: u16 = 32;
    let mut counter = 0u32;
    let mut maps = Vec::new();
    for spv in modules.iter_mut() {
        let mut out: Vec<(u32, u32, u32)> = Vec::new();
        if spv.len() < 20 || rd(spv, 0) != SPIRV_MAGIC {
            maps.push(out);
            continue;
        }
        let total = spv.len() / 4;
        let mut binding_words: Vec<(u32, u32, usize)> = Vec::new(); // (id, old_binding, value word)
        let mut var_sc: HashMap<u32, u32> = HashMap::new(); // var id -> storage class
        let mut i = 5;
        while i < total {
            let w0 = rd(spv, i);
            let wc = (w0 >> 16) as usize;
            let op = (w0 & 0xffff) as u16;
            if wc == 0 || i + wc > total {
                break;
            }
            match op {
                OP_DECORATE if wc >= 4 && rd(spv, i + 2) == DECO_BINDING => {
                    binding_words.push((rd(spv, i + 1), rd(spv, i + 3), i + 3));
                }
                OP_VARIABLE if wc >= 4 => {
                    var_sc.insert(rd(spv, i + 2), rd(spv, i + 3));
                }
                _ => {
                    let _ = OP_TYPE_POINTER;
                }
            }
            i += wc;
        }
        // Deterministic order: by original binding then id.
        binding_words.sort_by_key(|x| (x.1, x.0));
        for (id, old, word) in binding_words {
            let sc = var_sc.get(&id).copied().unwrap_or(u32::MAX);
            let nb = counter;
            counter += 1;
            wr(spv, word, nb);
            out.push((old, nb, sc));
        }
        maps.push(out);
    }
    maps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_op_on_garbage() {
        let mut junk = vec![0u8; 8];
        assert_eq!(neutralize_draw_parameters(&mut junk), 0);
        assert_eq!(force_readonly_ssbo_loads_zero(&mut junk), 0);
        assert!(compact_descriptor_bindings(&mut junk).is_empty());
    }

    /// Sparse bindings {4, 25} in set 0 compact to {0, 1}.
    #[test]
    fn compacts_sparse_bindings() {
        let mut words: Vec<u32> = vec![SPIRV_MAGIC, 0x0001_0600, 0, 30, 0];
        // OpDecorate %10 DescriptorSet 0 ; OpDecorate %10 Binding 4
        words.extend([(4 << 16) | OP_DECORATE as u32, 10, DECO_DESCRIPTOR_SET, 0]);
        words.extend([(4 << 16) | OP_DECORATE as u32, 10, DECO_BINDING, 4]);
        // OpDecorate %20 DescriptorSet 0 ; OpDecorate %20 Binding 25
        words.extend([(4 << 16) | OP_DECORATE as u32, 20, DECO_DESCRIPTOR_SET, 0]);
        words.extend([(4 << 16) | OP_DECORATE as u32, 20, DECO_BINDING, 25]);

        let mut bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let map = compact_descriptor_bindings(&mut bytes);
        assert!(map.contains(&((0, 4), (0, 0))));
        assert!(map.contains(&((0, 25), (0, 1))));
    }

    /// Hand-built minimal module: uint type, const 0, a BaseInstance var, and one load of
    /// it — the load must become an OpCopyObject of the constant.
    #[test]
    fn rewrites_baseinstance_load() {
        let mut words: Vec<u32> = vec![
            SPIRV_MAGIC,
            0x0001_0600,
            0,
            20,
            0, // header (bound=20)
        ];
        // OpDecorate %10 BuiltIn BaseInstance  (wc=4)
        words.extend([
            (4 << 16) | OP_DECORATE as u32,
            10,
            DECO_BUILTIN,
            BUILTIN_BASE_INSTANCE,
        ]);
        // OpTypeInt %2 32 0  (wc=4)
        words.extend([(4 << 16) | OP_TYPE_INT as u32, 2, 32, 0]);
        // OpConstant %2 %3 0  (wc=4)
        words.extend([(4 << 16) | OP_CONSTANT as u32, 2, 3, 0]);
        // OpLoad %2 %5 %10  (wc=4) — the BaseInstance load
        let load_at = words.len();
        words.extend([(4 << 16) | OP_LOAD as u32, 2, 5, 10]);

        let mut bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        assert_eq!(neutralize_draw_parameters(&mut bytes), 1);

        // The load is now OpCopyObject %2 %5 %3 (const 0).
        let op =
            u32::from_le_bytes(bytes[load_at * 4..load_at * 4 + 4].try_into().unwrap()) & 0xffff;
        let operand = u32::from_le_bytes(
            bytes[(load_at + 3) * 4..(load_at + 3) * 4 + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(op, OP_COPY_OBJECT as u32);
        assert_eq!(operand, 3, "operand should be the const-0 id");
    }
}
