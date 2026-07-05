//! CPU-only inspector for the SPIR-V dxil-spirv emits — to diagnose the deterministic
//! GPUVM fault when DRAWING a real ER object `.vpo` through passthrough, WITHOUT touching
//! the GPU. Translates a `.vpo` with selectable flags and dumps every descriptor-bound
//! resource (storage class, set/binding, block type, array stride) plus the declared
//! capabilities — so we can see exactly how `g_InstanceIndexBuffer` (t25) is represented
//! and whether a physical-buffer-address path is involved.
//!
//! Run: `cargo run -p er-shaderkit --example inspect_spirv [path-to.vpo]`.

use std::collections::HashMap;
use std::process::Command;

use er_shaderkit::discover_dxil_spirv;

/// Translate a DX container with explicit dxil-spirv flags (so we can compare
/// with/without `--ssbo-srv`). CPU only.
fn translate(container: &[u8], flags: &[&str]) -> Option<Vec<u8>> {
    let bin = discover_dxil_spirv()?;
    let dir = std::env::temp_dir();
    let inp = dir.join(format!("inspect-{}.dxbc", container.len()));
    let outp = dir.join(format!("inspect-{}.spv", container.len()));
    std::fs::write(&inp, container).ok()?;
    let mut cmd = Command::new(&bin);
    cmd.arg(&inp).arg("--output").arg(&outp);
    for f in flags {
        cmd.arg(f);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        eprintln!(
            "dxil-spirv failed for flags {flags:?}: {}",
            // UTF-8 Lossy: external tool stderr is human diagnostics; preserve printable context even if it is not valid UTF-8.
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    let spv = std::fs::read(&outp).ok()?;
    let _ = std::fs::remove_file(&inp);
    let _ = std::fs::remove_file(&outp);
    Some(spv)
}

fn w(spv: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([spv[i * 4], spv[i * 4 + 1], spv[i * 4 + 2], spv[i * 4 + 3]])
}

fn cap_name(c: u32) -> String {
    match c {
        0 => "Matrix".into(),
        1 => "Shader".into(),
        32 => "DrawParameters".into(),
        37 => "SampledImageArrayDynamicIndexing".into(),
        5301 => " RuntimeDescriptorArray".into(),
        5302 => "InputAttachmentArrayDynamicIndexing".into(),
        5306 => "StorageBufferArrayNonUniformIndexing".into(),
        5347 => "PhysicalStorageBufferAddresses".into(),
        4427 => "ShaderNonUniform".into(),
        other => format!("cap#{other}"),
    }
}

fn sc_name(s: u32) -> &'static str {
    match s {
        0 => "UniformConstant",
        1 => "Input",
        2 => "Uniform",
        3 => "Output",
        9 => "PushConstant",
        12 => "StorageBuffer",
        5349 => "PhysicalStorageBuffer",
        other => Box::leak(format!("sc#{other}").into_boxed_str()),
    }
}

fn dump(label: &str, spv: &[u8]) {
    println!("\n========== {label} ({} bytes) ==========", spv.len());
    if spv.len() < 20 || w(spv, 0) != 0x0723_0203 {
        println!("not SPIR-V");
        return;
    }
    let bound = w(spv, 3);
    let total = spv.len() / 4;
    let mut i = 5;

    let mut names: HashMap<u32, String> = HashMap::new();
    let mut caps: Vec<u32> = Vec::new();
    // id -> (set, binding)
    let mut set: HashMap<u32, u32> = HashMap::new();
    let mut binding: HashMap<u32, u32> = HashMap::new();
    let mut array_stride: HashMap<u32, u32> = HashMap::new();
    let mut block_kind: HashMap<u32, &'static str> = HashMap::new(); // Block / BufferBlock
    // type graph
    let mut ptr_to: HashMap<u32, (u32, u32)> = HashMap::new(); // ptr id -> (storage class, pointee)
    let mut runtime_array_of: HashMap<u32, u32> = HashMap::new();
    let mut struct_members: HashMap<u32, Vec<u32>> = HashMap::new();
    // var id -> (result type ptr, storage class)
    let mut vars: Vec<(u32, u32, u32)> = Vec::new();

    while i < total {
        let word0 = w(spv, i);
        let wc = (word0 >> 16) as usize;
        let op = word0 & 0xffff;
        if wc == 0 {
            break;
        }
        match op {
            17 => caps.push(w(spv, i + 1)), // OpCapability
            5 => {
                // OpName target, "name"
                let id = w(spv, i + 1);
                let bytes: Vec<u8> = (i + 2..i + wc)
                    .flat_map(|k| w(spv, k).to_le_bytes())
                    .take_while(|&b| b != 0)
                    .collect();
                // UTF-8 Lossy: SPIR-V OpName strings are diagnostic labels; malformed bytes should not abort inspection.
                names.insert(id, String::from_utf8_lossy(&bytes).into_owned());
            }
            71 => {
                // OpDecorate target deco [operands]
                let id = w(spv, i + 1);
                let deco = w(spv, i + 2);
                match deco {
                    33 => {
                        binding.insert(id, w(spv, i + 3));
                    }
                    34 => {
                        set.insert(id, w(spv, i + 3));
                    }
                    6 => {
                        array_stride.insert(id, w(spv, i + 3));
                    }
                    2 => {
                        block_kind.insert(id, "Block");
                    }
                    3 => {
                        block_kind.insert(id, "BufferBlock");
                    }
                    _ => {}
                }
            }
            32 => {
                // OpTypePointer result, storageclass, type
                ptr_to.insert(w(spv, i + 1), (w(spv, i + 2), w(spv, i + 3)));
            }
            28 | 29 => {
                // OpTypeArray (28) / OpTypeRuntimeArray (29) result, elementtype
                runtime_array_of.insert(w(spv, i + 1), w(spv, i + 2));
            }
            30 => {
                // OpTypeStruct result, members...
                struct_members.insert(w(spv, i + 1), (i + 2..i + wc).map(|k| w(spv, k)).collect());
            }
            59 => {
                // OpVariable: i+1 = result type (pointer), i+2 = result id, i+3 = storage class.
                vars.push((w(spv, i + 1), w(spv, i + 2), w(spv, i + 3)));
            }
            _ => {}
        }
        i += wc;
    }

    println!("bound ids: {bound}");
    let mut cs: Vec<String> = caps.iter().map(|&c| cap_name(c)).collect();
    cs.sort();
    println!("capabilities: {}", cs.join(", "));

    // Resource-class variables WITHOUT a (set,binding) — push constants or orphans the
    // draw harness wouldn't provide (a prime suspect for an uninitialized-index fault).
    println!("resource vars lacking set/binding (push-constant / orphan):");
    let mut any_orphan = false;
    for &(_rtype, id, sc) in &vars {
        let is_resource = matches!(sc, 0 | 2 | 9 | 12); // UniformConstant/Uniform/PushConstant/StorageBuffer
        if is_resource && !(set.contains_key(&id) && binding.contains_key(&id)) {
            any_orphan = true;
            println!(
                "  !! var %{id} storage={} (no set/binding) name='{}'",
                sc_name(sc),
                names.get(&id).cloned().unwrap_or_default()
            );
        }
    }
    if !any_orphan {
        println!("  (none)");
    }

    println!("descriptor-bound variables (set, binding):");
    let mut rows: Vec<(u32, u32, u32, u32)> = Vec::new(); // set, binding, var, sc
    for &(rtype, id, sc) in &vars {
        if let (Some(&s), Some(&b)) = (set.get(&id), binding.get(&id)) {
            rows.push((s, b, id, sc));
            let _ = rtype;
        }
    }
    rows.sort();
    for (s, b, id, sc) in rows {
        // resolve the block type behind the pointer for stride/struct info.
        let rtype = vars.iter().find(|v| v.1 == id).map(|v| v.0).unwrap_or(0);
        let pointee = ptr_to.get(&rtype).map(|&(_, t)| t).unwrap_or(0);
        let bk = block_kind.get(&pointee).copied().unwrap_or("");
        // a struct whose member is a runtime array (SSBO) — get that array's stride.
        let mut stride = String::new();
        if let Some(members) = struct_members.get(&pointee) {
            for &m in members {
                if let Some(st) = array_stride.get(&m).or_else(|| array_stride.get(&pointee)) {
                    stride = format!(" array_stride={st}");
                }
                let _ = runtime_array_of.get(&m);
            }
        }
        if let Some(st) = array_stride.get(&pointee) {
            stride = format!(" array_stride={st}");
        }
        println!(
            "  set {s} binding {b:<3} {:<22} {} {bk}{stride}  '{}'",
            sc_name(sc),
            if pointee != 0 {
                format!("blockType%{pointee}")
            } else {
                String::new()
            },
            names.get(&id).cloned().unwrap_or_default()
        );
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/er-objectkit/sample.vpo".into());
    let container = std::fs::read(&path).expect("read .vpo");
    if discover_dxil_spirv().is_none() {
        eprintln!("dxil-spirv not built");
        return;
    }
    // Current production flags vs. dropping --ssbo-srv (only needed for naga, which
    // the passthrough path bypasses) vs. plain.
    if let Some(spv) = translate(&container, &["--validate", "--ssbo-uav", "--ssbo-srv"]) {
        dump(
            "WITH --ssbo-uav --ssbo-srv (current passthrough flags)",
            &spv,
        );
    }
    if let Some(spv) = translate(&container, &["--validate", "--ssbo-uav"]) {
        dump(
            "WITH --ssbo-uav only (SRV left as typed/texel buffer)",
            &spv,
        );
    }
    if let Some(spv) = translate(&container, &["--validate"]) {
        dump("NO ssbo flags (dxil-spirv default)", &spv);
    }
}
