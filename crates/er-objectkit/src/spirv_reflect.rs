//! Minimal SPIR-V reflection for the dxil-spirv passthrough modules.
//!
//! naga rejects ER's shaders (capability `DrawParameters` etc.), so we can't lean on
//! naga reflection — but we still need the bind-group + vertex-input layout to build a
//! matching wgpu pipeline. This walks the SPIR-V word stream directly and collects:
//! entry-point stage, vertex input locations, and resource bindings (set/binding +
//! kind) from `OpDecorate` + `OpVariable` storage classes. Just enough to lay out a
//! pipeline; not a full disassembler.

const MAGIC: u32 = 0x0723_0203;

// Op codes we care about.
const OP_ENTRY_POINT: u16 = 15;
const OP_NAME: u16 = 5;
const OP_DECORATE: u16 = 71;
const OP_VARIABLE: u16 = 59;
const OP_TYPE_POINTER: u16 = 32;
const OP_TYPE_IMAGE: u16 = 25;
const OP_TYPE_SAMPLER: u16 = 26;
const OP_TYPE_SAMPLED_IMAGE: u16 = 27;
const OP_TYPE_ARRAY: u16 = 28;
const OP_TYPE_RUNTIME_ARRAY: u16 = 29;

// Decoration enums.
const DEC_BINDING: u32 = 33;
const DEC_DESCRIPTOR_SET: u32 = 34;
const DEC_LOCATION: u32 = 30;

// Storage classes.
const SC_UNIFORM_CONSTANT: u32 = 0; // sampled images / samplers
const SC_INPUT: u32 = 1;
const SC_UNIFORM: u32 = 2; // cbuffers (and SSBO under some lowerings)
const SC_OUTPUT: u32 = 3;
const SC_STORAGE_BUFFER: u32 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
    Compute,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// Uniform / constant buffer (`cbMtdParam`, `cbInstanceData`, ...).
    Buffer,
    /// Read/write or read-only storage buffer (ER's `--ssbo-*` lowering).
    StorageBuffer,
    /// Sampled texture (`OpTypeImage` / `OpTypeSampledImage`).
    Texture,
    /// Separate sampler (`OpTypeSampler`).
    Sampler,
}

#[derive(Debug, Clone)]
pub struct Binding {
    pub set: u32,
    pub binding: u32,
    pub kind: BindingKind,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Reflection {
    pub stage: Stage,
    /// `OpEntryPoint` name — required to build a pipeline from a passthrough module
    /// (wgpu can't reflect it). dxil-spirv typically emits `main`.
    pub entry_name: String,
    /// Vertex input attribute locations (only meaningful for the vertex stage).
    pub input_locations: Vec<u32>,
    /// Fragment output locations = render-target count (fragment stage).
    pub output_locations: Vec<u32>,
    pub bindings: Vec<Binding>,
}

impl Default for Stage {
    fn default() -> Self {
        Stage::Other
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReflectError {
    #[error("not SPIR-V (bad magic)")]
    BadMagic,
    #[error("truncated SPIR-V")]
    Truncated,
}

/// Reflect a SPIR-V binary (little-endian, as dxil-spirv emits).
pub fn reflect(spirv: &[u8]) -> Result<Reflection, ReflectError> {
    if spirv.len() < 20 {
        return Err(ReflectError::Truncated);
    }
    let words: Vec<u32> = spirv
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    if words[0] != MAGIC {
        return Err(ReflectError::BadMagic);
    }

    // id -> (set, binding, location), id -> name, id -> storage class.
    use std::collections::{HashMap, HashSet};
    let mut sets: HashMap<u32, u32> = HashMap::new();
    let mut binds: HashMap<u32, u32> = HashMap::new();
    let mut locs: HashMap<u32, u32> = HashMap::new();
    let mut names: HashMap<u32, String> = HashMap::new();
    // Type-id classification, to tell textures from samplers from buffers.
    let mut image_types: HashSet<u32> = HashSet::new(); // OpTypeImage / OpTypeSampledImage
    let mut sampler_types: HashSet<u32> = HashSet::new(); // OpTypeSampler
    let mut array_elem: HashMap<u32, u32> = HashMap::new(); // array type -> element type
    let mut ptr_pointee: HashMap<u32, u32> = HashMap::new(); // pointer type -> pointee type
    let mut variables: Vec<(u32, u32, u32)> = Vec::new(); // (result id, storage class, pointer type)
    let mut input_ids: Vec<u32> = Vec::new();
    let mut output_ids: Vec<u32> = Vec::new();
    let mut stage = Stage::Other;
    let mut entry_name = String::new();

    let mut i = 5usize; // skip 5-word header
    while i < words.len() {
        let word0 = words[i];
        let op = (word0 & 0xFFFF) as u16;
        let len = (word0 >> 16) as usize;
        if len == 0 || i + len > words.len() {
            break;
        }
        let operands = &words[i + 1..i + len];
        match op {
            OP_ENTRY_POINT => {
                // operands: [execution model, entry id, name..., interface ids...]
                if let Some(&model) = operands.first() {
                    stage = match model {
                        0 => Stage::Vertex,
                        4 => Stage::Fragment,
                        5 => Stage::Compute,
                        _ => Stage::Other,
                    };
                }
                if operands.len() > 2 {
                    entry_name = decode_string(&operands[2..]);
                }
            }
            OP_NAME => {
                if let Some(&target) = operands.first() {
                    let s = decode_string(&operands[1..]);
                    if !s.is_empty() {
                        names.insert(target, s);
                    }
                }
            }
            OP_DECORATE => {
                if operands.len() >= 2 {
                    let target = operands[0];
                    let dec = operands[1];
                    let val = operands.get(2).copied();
                    match (dec, val) {
                        (DEC_DESCRIPTOR_SET, Some(v)) => {
                            sets.insert(target, v);
                        }
                        (DEC_BINDING, Some(v)) => {
                            binds.insert(target, v);
                        }
                        (DEC_LOCATION, Some(v)) => {
                            locs.insert(target, v);
                        }
                        _ => {}
                    }
                }
            }
            OP_TYPE_IMAGE | OP_TYPE_SAMPLED_IMAGE => {
                if let Some(&id) = operands.first() {
                    image_types.insert(id);
                }
            }
            OP_TYPE_SAMPLER => {
                if let Some(&id) = operands.first() {
                    sampler_types.insert(id);
                }
            }
            OP_TYPE_ARRAY | OP_TYPE_RUNTIME_ARRAY => {
                // result, element type, [length]
                if operands.len() >= 2 {
                    array_elem.insert(operands[0], operands[1]);
                }
            }
            OP_TYPE_POINTER => {
                // result, storage class, pointee type
                if operands.len() >= 3 {
                    ptr_pointee.insert(operands[0], operands[2]);
                }
            }
            OP_VARIABLE => {
                // result type (a pointer), result id, storage class
                if operands.len() >= 3 {
                    let ptr_type = operands[0];
                    let result = operands[1];
                    let sc = operands[2];
                    variables.push((result, sc, ptr_type));
                    if sc == SC_INPUT {
                        input_ids.push(result);
                    } else if sc == SC_OUTPUT {
                        output_ids.push(result);
                    }
                }
            }
            _ => {}
        }
        i += len;
    }

    let mut input_locations: Vec<u32> = input_ids
        .iter()
        .filter_map(|id| locs.get(id).copied())
        .collect();
    input_locations.sort_unstable();
    input_locations.dedup();

    let mut output_locations: Vec<u32> = output_ids
        .iter()
        .filter_map(|id| locs.get(id).copied())
        .collect();
    output_locations.sort_unstable();
    output_locations.dedup();

    // Resolve a pointer type to its underlying resource type, seeing through arrays.
    let resolve_kind = |ptr_type: u32, sc: u32| -> Option<BindingKind> {
        match sc {
            SC_UNIFORM => return Some(BindingKind::Buffer),
            SC_STORAGE_BUFFER => return Some(BindingKind::StorageBuffer),
            SC_UNIFORM_CONSTANT => {}
            _ => return None,
        }
        // UniformConstant: distinguish image vs sampler via the pointee type.
        let mut t = *ptr_pointee.get(&ptr_type)?;
        // unwrap arrays (bindless texture/sampler arrays)
        for _ in 0..8 {
            if let Some(&elem) = array_elem.get(&t) {
                t = elem;
            } else {
                break;
            }
        }
        if image_types.contains(&t) {
            Some(BindingKind::Texture)
        } else if sampler_types.contains(&t) {
            Some(BindingKind::Sampler)
        } else {
            // Unknown UniformConstant shape (e.g. combined sampler we didn't map) —
            // treat as a texture so it still gets a binding slot.
            Some(BindingKind::Texture)
        }
    };

    let mut bindings = Vec::new();
    for (id, sc, ptr_type) in variables {
        let (Some(&set), Some(&binding)) = (sets.get(&id), binds.get(&id)) else {
            continue;
        };
        let Some(kind) = resolve_kind(ptr_type, sc) else {
            continue;
        };
        bindings.push(Binding {
            set,
            binding,
            kind,
            name: names.get(&id).cloned(),
        });
    }
    bindings.sort_by_key(|b| (b.set, b.binding));

    Ok(Reflection {
        stage,
        entry_name,
        input_locations,
        output_locations,
        bindings,
    })
}

/// Decode a SPIR-V literal string (NUL-terminated, packed 4 chars/word, LE).
fn decode_string(words: &[u32]) -> String {
    let mut bytes = Vec::new();
    'outer: for &w in words {
        for shift in [0, 8, 16, 24] {
            let b = ((w >> shift) & 0xFF) as u8;
            if b == 0 {
                break 'outer;
            }
            bytes.push(b);
        }
    }
    // UTF-8 Lossy: SPIR-V literal strings are spec'd UTF-8; a malformed module shouldn't panic reflection (names are diagnostic-only).
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passthrough::{first_pair, translate};
    use crate::shaderbundle::parse_bundle;

    /// Reflect a real translated ER shader pair: the vertex stage must expose input
    /// locations and the pixel stage must expose texture + buffer bindings (it samples
    /// material textures and reads cbuffers).
    #[test]
    fn real_pair_reflects_bindings_and_inputs() {
        if er_shaderkit::discover_dxil_spirv().is_none() {
            eprintln!("skip: dxil-spirv not built");
            return;
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/er-objectkit/shaderbdle");
        // Deterministic: sort, take the first bundle (read_dir order is otherwise
        // arbitrary and would pick different shaders across runs).
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("shaderbdle"))
            .collect();
        files.sort();
        let Some(file) = files.first() else {
            eprintln!("skip: no .shaderbdle extracted");
            return;
        };
        let shaders = parse_bundle(&std::fs::read(file).unwrap()).unwrap();

        // Vertex: any vpo exposes input locations.
        let v = shaders
            .iter()
            .find(|s| s.stage == crate::shaderbundle::ShaderStage::Vertex)
            .unwrap();
        let vr = reflect(&translate(&v.container).unwrap()).expect("reflect vertex");
        assert_eq!(vr.stage, Stage::Vertex);
        assert!(
            !vr.input_locations.is_empty(),
            "vertex shader exposed no input locations"
        );

        // Pixel: a texture-sampling pass (Gbuf/Fwd) must expose texture + buffer
        // bindings. Depth/velocity passes legitimately have no textures, so target a
        // colour pass deterministically.
        let p = shaders
            .iter()
            .filter(|s| s.stage == crate::shaderbundle::ShaderStage::Pixel)
            .find(|s| {
                let l = s.name.to_lowercase();
                l.contains("gbuf") || l.contains("_fwd")
            })
            .or_else(|| first_pair(&shaders).map(|(_, p)| p))
            .expect("a pixel shader");
        let pr = reflect(&translate(&p.container).unwrap()).expect("reflect pixel");
        assert_eq!(pr.stage, Stage::Fragment);
        let tex = pr
            .bindings
            .iter()
            .filter(|b| b.kind == BindingKind::Texture)
            .count();
        let samplers = pr
            .bindings
            .iter()
            .filter(|b| b.kind == BindingKind::Sampler)
            .count();
        let bufs = pr
            .bindings
            .iter()
            .filter(|b| matches!(b.kind, BindingKind::Buffer | BindingKind::StorageBuffer))
            .count();
        eprintln!(
            "{}: vertex inputs {:?}; pixel {} ({} textures, {} samplers, {} buffers)",
            file.file_name().unwrap().to_string_lossy(),
            vr.input_locations,
            p.name,
            tex,
            samplers,
            bufs
        );
        // dxil-spirv does NOT preserve HLSL resource names (measured: 0 named), so
        // mapping the ~23 textures to roles needs the DX container's RDEF chunk or a
        // RenderDoc capture — not the translated SPIR-V.
        let named = pr.bindings.iter().filter(|b| b.name.is_some()).count();
        eprintln!("named bindings: {named}/{}", pr.bindings.len());
        assert!(
            tex > 0,
            "colour-pass pixel shader exposed no texture bindings"
        );
        assert!(bufs > 0, "pixel shader exposed no buffer bindings");
    }
}
