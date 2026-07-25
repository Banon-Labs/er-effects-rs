//! check-no-magic-numbers: allow-file -- offline FLVER binary layout helper; byte offsets/field widths are external format literals validated by pack/reparse smoke.
//! Rust-first offline patcher for the Route A mushroom prototype.
//!
//! Inputs are the Rust-exported c2280 OBJ/weight TSV and the unpacked ER donor
//! `BD_M_1010.flver`. Output is a patched donor FLVER under `target/`; no game
//! directory is modified and neither game is launched.
//!
//! Build/run from the repo root:
//!   rustc scripts/route_a_mushroom_patch_donor.rs -O -o target/route_a_mushroom_patch_donor
//!   target/route_a_mushroom_patch_donor

use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::Write,
    path::PathBuf,
};

const DEFAULT_OBJ: &str =
    "target/mushroom-route-a-offline/prototype/c2280-rust-export/c2280_route_a_scaled.obj";
const DEFAULT_WEIGHTS: &str =
    "target/mushroom-route-a-offline/prototype/c2280-rust-export/c2280_route_a_weights.tsv";
const DEFAULT_DONOR_FLVER: &str =
    "target/er-extract-parts-sample/bd_m_1010-partsbnd-dcx/BD_M_1010.flver";
const DEFAULT_OUTPUT_FLVER: &str =
    "target/mushroom-route-a-offline/prototype/bd_m_1010-mushroom-parts/BD_M_1010.flver";
const DEFAULT_SUMMARY: &str =
    "target/mushroom-route-a-offline/prototype/bd_m_1010-mushroom-parts-summary.txt";
const DEFAULT_DONOR_MESH_INDEX: usize = 1;

const SPINE_COMPENSATION_MIN_HEIGHT_NORM: f32 = 0.08;
const SPINE_COMPENSATION_LOWER_FADE_HEIGHT_NORM: f32 = 0.16;
const SPINE_COMPENSATION_BASE_STRENGTH: f32 = 0.35;
const SPINE_COMPENSATION_CENTER_BOOST: f32 = 0.45;
const SPINE_COMPENSATION_CORE_RADIUS_MIN: f32 = 0.08;
const SPINE_COMPENSATION_CORE_RADIUS_MAX: f32 = 0.16;
const SPINE_COMPENSATION_METRIC_CORE_RADIUS: f32 = 0.10;
const REGION_NEAR_SHELL_DISTANCE: f32 = 0.08;
const REGION_WEIGHT_SYNC_L1_THRESHOLD: f32 = 0.35;
const REGION_WEIGHT_SYNC_MAX_STRENGTH: f32 = 0.25;
const ARM_COMPENSATION_MIN_ARM_WEIGHT: f32 = 0.25;
const ARM_COMPENSATION_MIN_COMPONENT_VERTICES: usize = 12;
const ARM_COMPENSATION_STRENGTH: f32 = 0.72;
const ARM_ROOT_TWEEN_FRACTION: f32 = 0.28;
const ARM_DISTAL_OVERWEIGHT_THRESHOLD: f32 = 0.70;
const ARM_LOW_UPPER_THRESHOLD: f32 = 0.18;
const ARM_BODY_ATTACHMENT_DISTANCE: f32 = 0.08;
const ARM_BROKEN_ISLAND_DISTANCE: f32 = 0.12;
const ARM_BROKEN_ISLAND_MIN_VERTICES: usize = 25;
const ARM_BROKEN_ISLAND_MAX_SPINE: f32 = 0.02;
const ARM_BROKEN_ISLAND_MAX_UPPER: f32 = 0.12;
const ARM_BROKEN_ISLAND_MIN_DISTAL: f32 = 0.75;
const ARM_FOREARM_SURFACE_PRESERVE_RESPONSE: &str = "preserved_forearm_surface_no_weight_proxy";
const ARM_VOLUME_MIN_SIDE_WEIGHT: f32 = 0.08;
const ARM_VOLUME_SIDE_SURFACE_MIN_HEIGHT: f32 = 0.46;
const ARM_VOLUME_SIDE_SURFACE_MAX_HEIGHT: f32 = 0.69;
const ARM_VOLUME_SIDE_SURFACE_MIN_LATERAL: f32 = 0.22;
const ARM_VOLUME_MAX_LATERAL_DELTA: f32 = 0.090;
const ARM_VOLUME_Z_SCALE: f32 = 0.45;
const ARM_HAND_FIT_MIN_HAND_WEIGHT: f32 = 0.30;
const ARM_HAND_FIT_STRENGTH: f32 = 0.75;
const ARM_HAND_FIT_TARGET_LEFT_X: f32 = 0.590;
const ARM_HAND_FIT_TARGET_RIGHT_X: f32 = -0.587;
const ARM_HAND_FIT_TARGET_Z: f32 = 0.006;

const HEADER_SIZE: usize = 0x80;
const DUMMY_SIZE: usize = 0x40;
const MATERIAL_SIZE: usize = 0x20;
const BONE_SIZE: usize = 0x80;
const MESH_SIZE: usize = 0x30;
const FACE_SET_SIZE: usize = 0x20;
const VERTEX_BUFFER_SIZE: usize = 0x20;
const BUFFER_LAYOUT_SIZE: usize = 0x10;
const LAYOUT_MEMBER_SIZE: usize = 0x14;

#[derive(Clone, Copy, Debug, Default)]
struct Vec2 {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct SourceVertex {
    position: Vec3,
    normal: Vec3,
    uv: Vec2,
    bone_indices: [u8; 4],
    bone_weights: [f32; 4],
}

#[derive(Clone, Debug)]
struct DonorBoneLookup {
    by_name: HashMap<String, u16>,
}

impl DonorBoneLookup {
    fn resolve(&self, name: &str) -> Option<u16> {
        self.by_name.get(name).copied().or_else(|| {
            (name == "Head")
                .then(|| self.by_name.get("Neck").copied())
                .flatten()
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct Header {
    version: u32,
    data_offset: usize,
    dummy_count: usize,
    material_count: usize,
    bone_count: usize,
    mesh_count: usize,
    face_set_count: usize,
    vertex_buffer_count: usize,
    buffer_layout_count: usize,
    vertex_index_size: u32,
}

#[derive(Clone, Copy, Debug)]
struct Mesh {
    bounding_box_offset: usize,
    face_set_count: usize,
    face_set_offset: usize,
    vertex_buffer_count: usize,
    vertex_buffer_offset: usize,
}

#[derive(Clone, Copy, Debug)]
struct FaceSet {
    triangle_strip: bool,
    index_count: usize,
    index_offset: usize,
    index_size: u32,
}

#[derive(Clone, Copy, Debug)]
struct VertexBuffer {
    layout_index: usize,
    vertex_size: usize,
    vertex_count: usize,
    buffer_length: usize,
    buffer_offset: usize,
}

#[derive(Clone, Copy, Debug)]
struct Layout {
    member_count: usize,
    member_offset: usize,
}

#[derive(Clone, Copy, Debug)]
struct LayoutMember {
    struct_offset: usize,
    format_id: u32,
    semantic_id: u32,
    index: u32,
}

struct SourceMesh {
    vertices: Vec<SourceVertex>,
    triangles: Vec<[u32; 3]>,
    bbox_min: Vec3,
    bbox_max: Vec3,
    weight_compensation: WeightCompensationReport,
    region_response: RegionResponseReport,
    arm_compensation: ArmCompensationReport,
    arm_island_prune: ArmIslandPruneReport,
    arm_volume_profile: ArmVolumeProfileReport,
}

#[derive(Clone, Debug, Default)]
struct ArmVolumeProfileReport {
    enabled: bool,
    affected_vertices: usize,
    side_surface_vertices: usize,
    hand_fit_vertices: usize,
    max_lateral_delta: f32,
    max_hand_translation: f32,
    elbow_radius_before: f32,
    elbow_radius_after: f32,
    bicep_radius_before: f32,
    bicep_radius_after: f32,
    shoulder_radius_before: f32,
    shoulder_radius_after: f32,
    left_hand_center_z_before: f32,
    left_hand_center_z_after: f32,
    right_hand_center_z_before: f32,
    right_hand_center_z_after: f32,
    response: String,
}

#[derive(Clone, Debug, Default)]
struct ArmIslandPruneReport {
    enabled: bool,
    components_before: usize,
    broken_components_before: usize,
    broken_components_after: usize,
    pruned_components: usize,
    pruned_vertices: usize,
    pruned_triangles: usize,
    triangles_before: usize,
    triangles_after: usize,
    response: String,
}

#[derive(Clone, Debug, Default)]
struct ArmCompensationReport {
    enabled: bool,
    compensated_vertices: usize,
    left_components: usize,
    right_components: usize,
    left_vertices: usize,
    right_vertices: usize,
    avg_upper_before: f32,
    avg_upper_after: f32,
    avg_forearm_hand_before: f32,
    avg_forearm_hand_after: f32,
    avg_body_tween_after: f32,
    distal_overweighted_components_before: usize,
    distal_overweighted_components_after: usize,
    weak_shoulder_components_before: usize,
    weak_shoulder_components_after: usize,
    detached_components: usize,
    detached_vertices: usize,
    detached_proxy_vertices: usize,
    independent_detached_components: usize,
    max_detached_body_distance: f32,
    detached_island_response: String,
}

#[derive(Clone, Debug, Default)]
struct ArmCompensationMetrics {
    vertices: usize,
    avg_upper: f32,
    avg_forearm_hand: f32,
    avg_body_tween: f32,
    distal_overweighted_components: usize,
    weak_shoulder_components: usize,
}

#[derive(Clone, Copy, Debug)]
enum ArmSide {
    Left,
    Right,
}

#[derive(Clone, Debug)]
struct ArmComponent {
    side: ArmSide,
    vertices: Vec<usize>,
}

#[derive(Clone, Copy, Debug)]
enum ArmAttachment {
    BodyNear,
    Detached,
}

#[derive(Clone, Debug)]
struct ClassifiedArmComponent {
    component: ArmComponent,
    attachment: ArmAttachment,
    nearest_body_distance: f32,
}

#[derive(Clone, Debug, Default)]
struct RegionResponseReport {
    enabled: bool,
    region_map_path: String,
    feet_authored_vertices: usize,
    feet_expanded_vertices: usize,
    feet_shrunk_vertices: usize,
    feet_normalized_vertices: usize,
    feet_blend_band_vertices: usize,
    feet_mixed_triangles_before: usize,
    feet_mixed_triangles_after: usize,
    feet_boundary_edges_before: usize,
    feet_boundary_edges_after: usize,
    feet_near_shell_pairs: usize,
    feet_near_shell_outside_vertices: usize,
    feet_weight_mismatch_pairs: usize,
    feet_weight_sync_vertices: usize,
    feet_max_weight_l1_mismatch: f32,
    response: String,
}

#[derive(Clone, Copy, Debug, Default)]
struct WeightCompensationReport {
    enabled: bool,
    axis_center_x: f32,
    axis_center_z: f32,
    compensated_vertices: usize,
    central_core_vertices: usize,
    central_core_avg_spine_before: f32,
    central_core_avg_limb_before: f32,
    central_core_avg_spine_after: f32,
    central_core_avg_limb_after: f32,
    hard_spine_limb_edges_before: usize,
    hard_spine_limb_edges_after: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct WeightCompensationMetrics {
    central_core_vertices: usize,
    central_core_avg_spine: f32,
    central_core_avg_limb: f32,
    hard_spine_limb_edges: usize,
}

struct Config {
    obj_path: PathBuf,
    weights_path: PathBuf,
    donor_flver: PathBuf,
    output_flver: PathBuf,
    summary_path: PathBuf,
    donor_mesh_index: usize,
    spine_core_compensation: bool,
    arm_compensation: bool,
    arm_island_prune: bool,
    arm_volume_profile: bool,
    region_map_tsv: Option<PathBuf>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args()?;
    let mut source = read_obj(&config.obj_path)?;
    let mut donor_bytes = fs::read(&config.donor_flver)?;
    let donor_lookup = donor_bone_lookup(&donor_bytes)?;
    apply_weights(
        &mut source,
        &config.weights_path,
        &donor_lookup,
        config.spine_core_compensation,
        config.arm_compensation,
        config.arm_island_prune,
        config.arm_volume_profile,
        config.region_map_tsv.as_ref(),
    )?;

    let patch_report = patch_donor_flver(&mut donor_bytes, &source, config.donor_mesh_index)?;

    if let Some(parent) = config.output_flver.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&config.output_flver, &donor_bytes)?;
    write_summary(&config, &source, &patch_report)?;

    println!("wrote {}", config.output_flver.display());
    println!("wrote {}", config.summary_path.display());
    println!(
        "patched_mesh={} vertices={} triangles={} donor_vertex_capacity={} lod0_capacity={} spine_compensated_vertices={} hard_spine_limb_edges={}→{}",
        config.donor_mesh_index,
        source.vertices.len(),
        source.triangles.len(),
        patch_report.vertex_capacity,
        patch_report.lod0_index_capacity / 3,
        source.weight_compensation.compensated_vertices,
        source.weight_compensation.hard_spine_limb_edges_before,
        source.weight_compensation.hard_spine_limb_edges_after
    );
    if source.arm_volume_profile.enabled {
        println!(
            "arm_volume_profile affected={} elbow_radius={:.4}→{:.4} bicep_radius={:.4}→{:.4} shoulder_radius={:.4}→{:.4} max_delta={:.4} response={}",
            source.arm_volume_profile.affected_vertices,
            source.arm_volume_profile.elbow_radius_before,
            source.arm_volume_profile.elbow_radius_after,
            source.arm_volume_profile.bicep_radius_before,
            source.arm_volume_profile.bicep_radius_after,
            source.arm_volume_profile.shoulder_radius_before,
            source.arm_volume_profile.shoulder_radius_after,
            source.arm_volume_profile.max_lateral_delta,
            source.arm_volume_profile.response
        );
    }
    if source.arm_island_prune.enabled {
        println!(
            "arm_island_prune broken={}→{} pruned_components={} pruned_vertices={} pruned_triangles={} response={}",
            source.arm_island_prune.broken_components_before,
            source.arm_island_prune.broken_components_after,
            source.arm_island_prune.pruned_components,
            source.arm_island_prune.pruned_vertices,
            source.arm_island_prune.pruned_triangles,
            source.arm_island_prune.response
        );
    }
    if source.arm_compensation.enabled {
        println!(
            "arm_compensation vertices={} upper={:.3}→{:.3} distal={:.3}→{:.3} weak_shoulder={}→{} detached={} proxy={} response={}",
            source.arm_compensation.compensated_vertices,
            source.arm_compensation.avg_upper_before,
            source.arm_compensation.avg_upper_after,
            source.arm_compensation.avg_forearm_hand_before,
            source.arm_compensation.avg_forearm_hand_after,
            source.arm_compensation.weak_shoulder_components_before,
            source.arm_compensation.weak_shoulder_components_after,
            source.arm_compensation.detached_components,
            source.arm_compensation.detached_proxy_vertices,
            source.arm_compensation.detached_island_response
        );
    }
    if source.region_response.enabled {
        println!(
            "region_response feet={} face_candidate={} mixed_triangles={}→{} near_pairs={} mismatches={} synced_vertices={} response={}",
            source.region_response.feet_authored_vertices,
            source.region_response.feet_expanded_vertices,
            source.region_response.feet_mixed_triangles_before,
            source.region_response.feet_mixed_triangles_after,
            source.region_response.feet_near_shell_pairs,
            source.region_response.feet_weight_mismatch_pairs,
            source.region_response.feet_weight_sync_vertices,
            source.region_response.response
        );
    }
    Ok(())
}

struct PatchReport {
    vertex_capacity: usize,
    lod0_index_capacity: usize,
    patched_face_sets: usize,
    hidden_face_sets: usize,
}

fn parse_args() -> Result<Config, Box<dyn std::error::Error>> {
    let mut obj_path = PathBuf::from(DEFAULT_OBJ);
    let mut weights_path = PathBuf::from(DEFAULT_WEIGHTS);
    let mut donor_flver = PathBuf::from(DEFAULT_DONOR_FLVER);
    let mut output_flver = PathBuf::from(DEFAULT_OUTPUT_FLVER);
    let mut summary_path = PathBuf::from(DEFAULT_SUMMARY);
    let mut donor_mesh_index = DEFAULT_DONOR_MESH_INDEX;
    let mut spine_core_compensation = true;
    let mut arm_compensation = false;
    let mut arm_island_prune = true;
    let mut arm_volume_profile = true;
    let mut region_map_tsv = None;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--obj" => obj_path = PathBuf::from(required_value(&arg, args.next())?),
            "--weights" => weights_path = PathBuf::from(required_value(&arg, args.next())?),
            "--donor-flver" => donor_flver = PathBuf::from(required_value(&arg, args.next())?),
            "--output-flver" => output_flver = PathBuf::from(required_value(&arg, args.next())?),
            "--summary" => summary_path = PathBuf::from(required_value(&arg, args.next())?),
            "--donor-mesh-index" => {
                donor_mesh_index = required_value(&arg, args.next())?.parse()?
            }
            "--no-spine-core-compensation" => spine_core_compensation = false,
            "--arm-compensation" => arm_compensation = true,
            "--no-arm-compensation" => arm_compensation = false,
            "--no-arm-island-prune" => arm_island_prune = false,
            "--no-arm-volume-profile" => arm_volume_profile = false,
            "--region-map-tsv" => {
                region_map_tsv = Some(PathBuf::from(required_value(&arg, args.next())?))
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    Ok(Config {
        obj_path,
        weights_path,
        donor_flver,
        output_flver,
        summary_path,
        donor_mesh_index,
        spine_core_compensation,
        arm_compensation,
        arm_island_prune,
        arm_volume_profile,
        region_map_tsv,
    })
}

fn required_value(flag: &str, value: Option<String>) -> Result<String, Box<dyn std::error::Error>> {
    value.ok_or_else(|| format!("{flag} requires a value").into())
}

fn print_help() {
    println!("route_a_mushroom_patch_donor: patch BD_M_1010 donor FLVER from Rust-exported c2280 OBJ/weights");
    println!("  --obj <path>              default: {DEFAULT_OBJ}");
    println!("  --weights <path>          default: {DEFAULT_WEIGHTS}");
    println!("  --donor-flver <path>      default: {DEFAULT_DONOR_FLVER}");
    println!("  --output-flver <path>     default: {DEFAULT_OUTPUT_FLVER}");
    println!("  --summary <path>          default: {DEFAULT_SUMMARY}");
    println!("  --donor-mesh-index <idx>  default: {DEFAULT_DONOR_MESH_INDEX}");
    println!("  --no-spine-core-compensation  disable automatic mushroom trunk/cap spine-weight self-healing");
    println!(
        "  --arm-compensation  opt into experimental shoulder/hand compensation; currently guard-blocked when components remain disconnected"
    );
    println!("  --no-arm-compensation  explicit default: keep failed arm compensation disabled");
    println!("  --no-arm-island-prune  disable default pruning of broken detached hand/forearm island triangles");
    println!("  --region-map-tsv <path>  apply human-authored region responses, e.g. feet face-closure/blend");
}

fn read_obj(path: &PathBuf) -> Result<SourceMesh, Box<dyn std::error::Error>> {
    let text = fs::read_to_string(path)?;
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut triangles = Vec::new();

    for line in text.lines() {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("v") => positions.push(Vec3 {
                x: required_part(&mut parts, "v.x")?.parse()?,
                y: required_part(&mut parts, "v.y")?.parse()?,
                z: required_part(&mut parts, "v.z")?.parse()?,
            }),
            Some("vn") => normals.push(Vec3 {
                x: required_part(&mut parts, "vn.x")?.parse()?,
                y: required_part(&mut parts, "vn.y")?.parse()?,
                z: required_part(&mut parts, "vn.z")?.parse()?,
            }),
            Some("vt") => uvs.push(Vec2 {
                x: required_part(&mut parts, "vt.x")?.parse()?,
                y: required_part(&mut parts, "vt.y")?.parse()?,
            }),
            Some("f") => {
                let mut tri = [0_u32; 3];
                for slot in &mut tri {
                    let token = required_part(&mut parts, "face index")?;
                    let vertex = token
                        .split('/')
                        .next()
                        .ok_or("malformed face token")?
                        .parse::<u32>()?;
                    *slot = vertex.checked_sub(1).ok_or("OBJ indices are 1-based")?;
                }
                triangles.push(tri);
            }
            _ => {}
        }
    }

    if positions.is_empty() || positions.len() != normals.len() || positions.len() != uvs.len() {
        return Err(format!(
            "OBJ requires matching v/vn/vt counts, got v={} vn={} vt={}",
            positions.len(),
            normals.len(),
            uvs.len()
        )
        .into());
    }

    let mut vertices = Vec::with_capacity(positions.len());
    for i in 0..positions.len() {
        vertices.push(SourceVertex {
            position: positions[i],
            normal: normals[i],
            uv: uvs[i],
            ..Default::default()
        });
    }
    let (bbox_min, bbox_max) = bbox_for_vertices(&vertices);
    Ok(SourceMesh {
        vertices,
        triangles,
        bbox_min,
        bbox_max,
        weight_compensation: WeightCompensationReport::default(),
        region_response: RegionResponseReport::default(),
        arm_compensation: ArmCompensationReport::default(),
        arm_island_prune: ArmIslandPruneReport::default(),
        arm_volume_profile: ArmVolumeProfileReport::default(),
    })
}

fn required_part<'a>(
    parts: &mut impl Iterator<Item = &'a str>,
    label: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    parts
        .next()
        .ok_or_else(|| format!("missing {label}").into())
}

fn apply_weights(
    mesh: &mut SourceMesh,
    path: &PathBuf,
    donor_lookup: &DonorBoneLookup,
    spine_core_compensation: bool,
    arm_compensation: bool,
    arm_island_prune: bool,
    arm_volume_profile: bool,
    region_map_tsv: Option<&PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let text = fs::read_to_string(path)?;
    let mut accum = vec![[0.0_f32; 256]; mesh.vertices.len()];
    for (line_index, line) in text.lines().enumerate() {
        if line_index == 0 || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 7 {
            return Err(format!("malformed weight TSV line {}: {line}", line_index + 1).into());
        }
        let vertex_index = cols[0].parse::<usize>()?;
        let target_bone = cols[5];
        let weight = cols[6].parse::<f32>()?;
        if weight <= 0.0 || target_bone.starts_with('<') {
            continue;
        }
        let donor_bone = donor_lookup
            .resolve(target_bone)
            .ok_or_else(|| format!("no donor bone mapping for ER target bone {target_bone}"))?;
        let donor_bone = bone_index_to_u8(donor_bone, target_bone)?;
        let vertex = accum
            .get_mut(vertex_index)
            .ok_or_else(|| format!("weight references missing vertex {vertex_index}"))?;
        vertex[donor_bone as usize] += weight;
    }

    if arm_volume_profile {
        mesh.arm_volume_profile = apply_arm_volume_profile(mesh, &accum, donor_lookup)?;
    }
    if arm_island_prune {
        mesh.arm_island_prune = prune_broken_arm_islands(mesh, &accum, donor_lookup)?;
    }
    if spine_core_compensation {
        mesh.weight_compensation = compensate_spine_core_weights(mesh, &mut accum, donor_lookup)?;
    }
    if arm_compensation {
        mesh.arm_compensation = compensate_arm_weights(mesh, &mut accum, donor_lookup)?;
    }
    if let Some(region_map_tsv) = region_map_tsv {
        mesh.region_response =
            apply_region_responses(mesh, &mut accum, region_map_tsv, donor_lookup)?;
    }

    for (vertex_index, vertex) in mesh.vertices.iter_mut().enumerate() {
        let mut pairs: Vec<(u8, f32)> = accum[vertex_index]
            .iter()
            .enumerate()
            .filter_map(|(bone, weight)| (*weight > 0.0001).then_some((bone as u8, *weight)))
            .collect();
        if pairs.is_empty() {
            let fallback = donor_lookup
                .resolve("Spine2")
                .ok_or("missing Spine2 donor bone")?;
            pairs.push((bone_index_to_u8(fallback, "Spine2")?, 1.0));
        }
        pairs.sort_by(|a, b| b.1.total_cmp(&a.1));
        pairs.truncate(4);
        let total = pairs.iter().map(|(_, weight)| *weight).sum::<f32>();
        for (slot, (bone, weight)) in pairs.into_iter().enumerate() {
            vertex.bone_indices[slot] = bone;
            vertex.bone_weights[slot] = if total > 0.0 { weight / total } else { 0.0 };
        }
    }

    Ok(())
}

fn compensate_spine_core_weights(
    mesh: &SourceMesh,
    accum: &mut [[f32; 256]],
    donor_lookup: &DonorBoneLookup,
) -> Result<WeightCompensationReport, Box<dyn std::error::Error>> {
    let pelvis = resolve_required_bone_u8(donor_lookup, "Pelvis")?;
    let spine = resolve_required_bone_u8(donor_lookup, "Spine")?;
    let spine1 = resolve_required_bone_u8(donor_lookup, "Spine1")?;
    let spine2 = resolve_required_bone_u8(donor_lookup, "Spine2")?;
    let spine_bones = [pelvis, spine, spine1, spine2];
    let limb_bones = resolve_optional_bones(
        donor_lookup,
        &[
            "L_Thigh",
            "R_Thigh",
            "L_Calf",
            "R_Calf",
            "L_Foot",
            "R_Foot",
            "L_UpperArm",
            "R_UpperArm",
            "L_Forearm",
            "R_Forearm",
            "L_Hand",
            "R_Hand",
        ],
    )?;
    let axis_center_x = (mesh.bbox_min.x + mesh.bbox_max.x) * 0.5;
    let axis_center_z = (mesh.bbox_min.z + mesh.bbox_max.z) * 0.5;
    let before = weight_compensation_metrics(
        mesh,
        accum,
        &spine_bones,
        &limb_bones,
        axis_center_x,
        axis_center_z,
    );

    let height_span = (mesh.bbox_max.y - mesh.bbox_min.y).max(f32::EPSILON);
    let mut compensated_vertices = 0;
    for (vertex_index, vertex) in mesh.vertices.iter().enumerate() {
        let height = normalized_height(vertex.position, mesh.bbox_min.y, height_span);
        if height < SPINE_COMPENSATION_MIN_HEIGHT_NORM {
            continue;
        }
        let radius = ((vertex.position.x - axis_center_x).powi(2)
            + (vertex.position.z - axis_center_z).powi(2))
        .sqrt();
        let radius_t = ((height - 0.15) / 0.55).clamp(0.0, 1.0);
        let core_radius = SPINE_COMPENSATION_CORE_RADIUS_MIN
            + (SPINE_COMPENSATION_CORE_RADIUS_MAX - SPINE_COMPENSATION_CORE_RADIUS_MIN) * radius_t;
        let centrality = (1.0 - radius / core_radius).clamp(0.0, 1.0);
        let mut strength =
            SPINE_COMPENSATION_BASE_STRENGTH + SPINE_COMPENSATION_CENTER_BOOST * centrality;
        if height < SPINE_COMPENSATION_LOWER_FADE_HEIGHT_NORM {
            strength *= 0.55;
        }
        strength = strength.clamp(0.0, 0.92);
        if strength <= 0.0 {
            continue;
        }

        let total = accum[vertex_index].iter().sum::<f32>();
        if total <= 0.0 {
            continue;
        }
        for weight in &mut accum[vertex_index] {
            *weight *= 1.0 - strength;
        }
        for (bone, target_weight) in spine_target_weights(height, pelvis, spine, spine1, spine2) {
            if target_weight > 0.0 {
                accum[vertex_index][bone as usize] += total * strength * target_weight;
            }
        }
        normalize_accumulated_weights(&mut accum[vertex_index]);
        compensated_vertices += 1;
    }

    let after = weight_compensation_metrics(
        mesh,
        accum,
        &spine_bones,
        &limb_bones,
        axis_center_x,
        axis_center_z,
    );
    Ok(WeightCompensationReport {
        enabled: true,
        axis_center_x,
        axis_center_z,
        compensated_vertices,
        central_core_vertices: after.central_core_vertices,
        central_core_avg_spine_before: before.central_core_avg_spine,
        central_core_avg_limb_before: before.central_core_avg_limb,
        central_core_avg_spine_after: after.central_core_avg_spine,
        central_core_avg_limb_after: after.central_core_avg_limb,
        hard_spine_limb_edges_before: before.hard_spine_limb_edges,
        hard_spine_limb_edges_after: after.hard_spine_limb_edges,
    })
}

fn resolve_required_bone_u8(
    donor_lookup: &DonorBoneLookup,
    name: &str,
) -> Result<u8, Box<dyn std::error::Error>> {
    let index = donor_lookup
        .resolve(name)
        .ok_or_else(|| format!("missing required donor bone {name}"))?;
    bone_index_to_u8(index, name)
}

fn resolve_optional_bones(
    donor_lookup: &DonorBoneLookup,
    names: &[&str],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bones = Vec::new();
    for name in names {
        if let Some(index) = donor_lookup.resolve(name) {
            bones.push(bone_index_to_u8(index, name)?);
        }
    }
    Ok(bones)
}

fn normalized_height(position: Vec3, min_y: f32, height_span: f32) -> f32 {
    ((position.y - min_y) / height_span).clamp(0.0, 1.0)
}

fn normalize_accumulated_weights(weights: &mut [f32; 256]) {
    let total = weights.iter().sum::<f32>();
    if total <= 0.0 {
        return;
    }
    for weight in weights {
        *weight /= total;
    }
}

fn weight_compensation_metrics(
    mesh: &SourceMesh,
    accum: &[[f32; 256]],
    spine_bones: &[u8],
    limb_bones: &[u8],
    axis_center_x: f32,
    axis_center_z: f32,
) -> WeightCompensationMetrics {
    let height_span = (mesh.bbox_max.y - mesh.bbox_min.y).max(f32::EPSILON);
    let mut central_core_vertices = 0;
    let mut central_core_spine_sum = 0.0;
    let mut central_core_limb_sum = 0.0;

    for (vertex_index, vertex) in mesh.vertices.iter().enumerate() {
        let height = normalized_height(vertex.position, mesh.bbox_min.y, height_span);
        if height < SPINE_COMPENSATION_MIN_HEIGHT_NORM {
            continue;
        }
        let radius = ((vertex.position.x - axis_center_x).powi(2)
            + (vertex.position.z - axis_center_z).powi(2))
        .sqrt();
        if radius <= SPINE_COMPENSATION_METRIC_CORE_RADIUS {
            central_core_vertices += 1;
            central_core_spine_sum += sum_bone_weights(&accum[vertex_index], spine_bones);
            central_core_limb_sum += sum_bone_weights(&accum[vertex_index], limb_bones);
        }
    }

    let mut hard_spine_limb_edges = 0;
    for (a, b) in unique_triangle_edges(&mesh.triangles) {
        let a = a as usize;
        let b = b as usize;
        if a >= mesh.vertices.len() || b >= mesh.vertices.len() {
            continue;
        }
        let height_a = normalized_height(mesh.vertices[a].position, mesh.bbox_min.y, height_span);
        let height_b = normalized_height(mesh.vertices[b].position, mesh.bbox_min.y, height_span);
        let mid_height = (height_a + height_b) * 0.5;
        if !(0.12..=0.70).contains(&mid_height) {
            continue;
        }
        let a_spine = sum_bone_weights(&accum[a], spine_bones);
        let a_limb = sum_bone_weights(&accum[a], limb_bones);
        let b_spine = sum_bone_weights(&accum[b], spine_bones);
        let b_limb = sum_bone_weights(&accum[b], limb_bones);
        if (a_spine > 0.75 && b_limb > 0.75) || (b_spine > 0.75 && a_limb > 0.75) {
            hard_spine_limb_edges += 1;
        }
    }

    let divisor = central_core_vertices.max(1) as f32;
    WeightCompensationMetrics {
        central_core_vertices,
        central_core_avg_spine: central_core_spine_sum / divisor,
        central_core_avg_limb: central_core_limb_sum / divisor,
        hard_spine_limb_edges,
    }
}

fn unique_triangle_edges(triangles: &[[u32; 3]]) -> HashSet<(u32, u32)> {
    let mut edges = HashSet::new();
    for [a, b, c] in triangles {
        insert_edge(&mut edges, *a, *b);
        insert_edge(&mut edges, *b, *c);
        insert_edge(&mut edges, *c, *a);
    }
    edges
}

fn insert_edge(edges: &mut HashSet<(u32, u32)>, a: u32, b: u32) {
    if a <= b {
        edges.insert((a, b));
    } else {
        edges.insert((b, a));
    }
}

fn sum_bone_weights(weights: &[f32; 256], bones: &[u8]) -> f32 {
    bones
        .iter()
        .map(|bone| weights[*bone as usize])
        .sum::<f32>()
}

fn spine_target_weights(
    height: f32,
    pelvis: u8,
    spine: u8,
    spine1: u8,
    spine2: u8,
) -> [(u8, f32); 4] {
    if height < 0.18 {
        return [(pelvis, 0.85), (spine, 0.15), (spine1, 0.0), (spine2, 0.0)];
    }
    if height < 0.36 {
        let t = ((height - 0.18) / 0.18).clamp(0.0, 1.0);
        return [
            (pelvis, lerp(0.85, 0.35, t)),
            (spine, lerp(0.15, 0.65, t)),
            (spine1, 0.0),
            (spine2, 0.0),
        ];
    }
    if height < 0.58 {
        let t = ((height - 0.36) / 0.22).clamp(0.0, 1.0);
        return [
            (pelvis, lerp(0.35, 0.0, t)),
            (spine, lerp(0.65, 0.70, t)),
            (spine1, lerp(0.0, 0.30, t)),
            (spine2, 0.0),
        ];
    }
    if height < 0.78 {
        let t = ((height - 0.58) / 0.20).clamp(0.0, 1.0);
        return [
            (pelvis, 0.0),
            (spine, lerp(0.70, 0.0, t)),
            (spine1, lerp(0.30, 0.65, t)),
            (spine2, lerp(0.0, 0.35, t)),
        ];
    }
    let t = ((height - 0.78) / 0.22).clamp(0.0, 1.0);
    [
        (pelvis, 0.0),
        (spine, 0.0),
        (spine1, lerp(0.65, 0.20, t)),
        (spine2, lerp(0.35, 0.80, t)),
    ]
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a * (1.0 - t) + b * t
}

fn apply_arm_volume_profile(
    mesh: &mut SourceMesh,
    accum: &[[f32; 256]],
    donor_lookup: &DonorBoneLookup,
) -> Result<ArmVolumeProfileReport, Box<dyn std::error::Error>> {
    let left_upper = resolve_required_bone_u8(donor_lookup, "L_UpperArm")?;
    let left_forearm = resolve_required_bone_u8(donor_lookup, "L_Forearm")?;
    let left_hand = resolve_required_bone_u8(donor_lookup, "L_Hand")?;
    let right_upper = resolve_required_bone_u8(donor_lookup, "R_UpperArm")?;
    let right_forearm = resolve_required_bone_u8(donor_lookup, "R_Forearm")?;
    let right_hand = resolve_required_bone_u8(donor_lookup, "R_Hand")?;
    let center_x = (mesh.bbox_min.x + mesh.bbox_max.x) * 0.5;
    let center_z = (mesh.bbox_min.z + mesh.bbox_max.z) * 0.5;
    let height_span = (mesh.bbox_max.y - mesh.bbox_min.y).max(f32::EPSILON);
    let (elbow_before, bicep_before, shoulder_before) = arm_volume_profile_radii(
        mesh,
        accum,
        left_upper,
        left_forearm,
        left_hand,
        right_upper,
        right_forearm,
        right_hand,
        center_x,
        center_z,
    );
    let (left_hand_before, right_hand_before) =
        arm_hand_centers(mesh, accum, left_hand, right_hand);

    let mut affected_vertices = 0;
    let mut side_surface_vertices = 0;
    let mut max_lateral_delta = 0.0_f32;
    for (vertex_index, vertex) in mesh.vertices.iter_mut().enumerate() {
        let row = &accum[vertex_index];
        let left_weight =
            row[left_upper as usize] + row[left_forearm as usize] + row[left_hand as usize];
        let right_weight =
            row[right_upper as usize] + row[right_forearm as usize] + row[right_hand as usize];
        let (side_weight, side_sign) = if left_weight >= right_weight {
            (left_weight, 1.0_f32)
        } else {
            (right_weight, -1.0_f32)
        };
        let height = normalized_height(vertex.position, mesh.bbox_min.y, height_span);
        let lateral = side_sign * (vertex.position.x - center_x);
        let side_surface_weight = if (ARM_VOLUME_SIDE_SURFACE_MIN_HEIGHT
            ..=ARM_VOLUME_SIDE_SURFACE_MAX_HEIGHT)
            .contains(&height)
            && lateral >= ARM_VOLUME_SIDE_SURFACE_MIN_LATERAL
        {
            0.85
        } else {
            0.0
        };
        let influence = if side_weight >= ARM_VOLUME_MIN_SIDE_WEIGHT {
            ((side_weight - ARM_VOLUME_MIN_SIDE_WEIGHT) / (1.0 - ARM_VOLUME_MIN_SIDE_WEIGHT))
                .clamp(0.0, 1.0)
                .sqrt()
        } else {
            side_surface_weight
        };
        if influence <= 0.0 {
            continue;
        }
        let delta = arm_volume_profile_delta(height) * influence;
        if delta <= 0.0 {
            continue;
        }
        vertex.position.x += side_sign * delta;
        vertex.position.z += (vertex.position.z - center_z) * delta * ARM_VOLUME_Z_SCALE;
        affected_vertices += 1;
        if side_surface_weight > 0.0 {
            side_surface_vertices += 1;
        }
        max_lateral_delta = max_lateral_delta.max(delta.abs());
    }

    let mut hand_fit_vertices = 0;
    let mut max_hand_translation = 0.0_f32;
    for (vertex_index, vertex) in mesh.vertices.iter_mut().enumerate() {
        let row = &accum[vertex_index];
        let left_weight = row[left_hand as usize];
        let right_weight = row[right_hand as usize];
        let (hand_weight, before_center, target_x) = if left_weight >= right_weight {
            (left_weight, left_hand_before, ARM_HAND_FIT_TARGET_LEFT_X)
        } else {
            (right_weight, right_hand_before, ARM_HAND_FIT_TARGET_RIGHT_X)
        };
        if hand_weight < ARM_HAND_FIT_MIN_HAND_WEIGHT {
            continue;
        }
        let strength = ARM_HAND_FIT_STRENGTH
            * ((hand_weight - ARM_HAND_FIT_MIN_HAND_WEIGHT) / (1.0 - ARM_HAND_FIT_MIN_HAND_WEIGHT))
                .clamp(0.0, 1.0);
        let dx = (target_x - before_center.x) * strength;
        let dz = (ARM_HAND_FIT_TARGET_Z - before_center.z) * strength;
        vertex.position.x += dx;
        vertex.position.z += dz;
        hand_fit_vertices += 1;
        max_hand_translation = max_hand_translation.max((dx * dx + dz * dz).sqrt());
    }

    let (elbow_after, bicep_after, shoulder_after) = arm_volume_profile_radii(
        mesh,
        accum,
        left_upper,
        left_forearm,
        left_hand,
        right_upper,
        right_forearm,
        right_hand,
        center_x,
        center_z,
    );
    let (left_hand_after, right_hand_after) = arm_hand_centers(mesh, accum, left_hand, right_hand);
    recompute_source_bbox(mesh);
    Ok(ArmVolumeProfileReport {
        enabled: true,
        affected_vertices,
        side_surface_vertices,
        hand_fit_vertices,
        max_lateral_delta,
        max_hand_translation,
        elbow_radius_before: elbow_before,
        elbow_radius_after: elbow_after,
        bicep_radius_before: bicep_before,
        bicep_radius_after: bicep_after,
        shoulder_radius_before: shoulder_before,
        shoulder_radius_after: shoulder_after,
        left_hand_center_z_before: left_hand_before.z,
        left_hand_center_z_after: left_hand_after.z,
        right_hand_center_z_before: right_hand_before.z,
        right_hand_center_z_after: right_hand_after.z,
        response: "side_surface_profile_curve_with_hand_fit".to_string(),
    })
}

fn arm_hand_centers(
    mesh: &SourceMesh,
    accum: &[[f32; 256]],
    left_hand: u8,
    right_hand: u8,
) -> (Vec3, Vec3) {
    let mut left_sum = Vec3::default();
    let mut right_sum = Vec3::default();
    let mut left_weight_sum = 0.0_f32;
    let mut right_weight_sum = 0.0_f32;
    for (vertex_index, vertex) in mesh.vertices.iter().enumerate() {
        let row = &accum[vertex_index];
        let left_weight = row[left_hand as usize];
        let right_weight = row[right_hand as usize];
        if left_weight >= ARM_HAND_FIT_MIN_HAND_WEIGHT {
            left_sum.x += vertex.position.x * left_weight;
            left_sum.y += vertex.position.y * left_weight;
            left_sum.z += vertex.position.z * left_weight;
            left_weight_sum += left_weight;
        }
        if right_weight >= ARM_HAND_FIT_MIN_HAND_WEIGHT {
            right_sum.x += vertex.position.x * right_weight;
            right_sum.y += vertex.position.y * right_weight;
            right_sum.z += vertex.position.z * right_weight;
            right_weight_sum += right_weight;
        }
    }
    if left_weight_sum > 0.0 {
        left_sum.x /= left_weight_sum;
        left_sum.y /= left_weight_sum;
        left_sum.z /= left_weight_sum;
    }
    if right_weight_sum > 0.0 {
        right_sum.x /= right_weight_sum;
        right_sum.y /= right_weight_sum;
        right_sum.z /= right_weight_sum;
    }
    (left_sum, right_sum)
}

fn arm_volume_profile_delta(height: f32) -> f32 {
    const PROFILE: &[(f32, f32)] = &[
        (0.34, 0.000),
        (0.42, 0.010),
        (0.49, 0.024),
        (0.53, 0.045),
        (0.56, ARM_VOLUME_MAX_LATERAL_DELTA),
        (0.60, 0.050),
        (0.65, 0.090),
        (0.70, 0.000),
    ];
    for window in PROFILE.windows(2) {
        let (h0, d0) = window[0];
        let (h1, d1) = window[1];
        if (h0..=h1).contains(&height) {
            let t = ((height - h0) / (h1 - h0)).clamp(0.0, 1.0);
            return lerp(d0, d1, t);
        }
    }
    0.0
}

#[allow(clippy::too_many_arguments)]
fn arm_volume_profile_radii(
    mesh: &SourceMesh,
    accum: &[[f32; 256]],
    left_upper: u8,
    left_forearm: u8,
    left_hand: u8,
    right_upper: u8,
    right_forearm: u8,
    right_hand: u8,
    center_x: f32,
    center_z: f32,
) -> (f32, f32, f32) {
    let height_span = (mesh.bbox_max.y - mesh.bbox_min.y).max(f32::EPSILON);
    let mut elbow = Vec::new();
    let mut bicep = Vec::new();
    let mut shoulder = Vec::new();
    for (vertex_index, vertex) in mesh.vertices.iter().enumerate() {
        let row = &accum[vertex_index];
        let side_weight =
            (row[left_upper as usize] + row[left_forearm as usize] + row[left_hand as usize]).max(
                row[right_upper as usize] + row[right_forearm as usize] + row[right_hand as usize],
            );
        if side_weight < ARM_VOLUME_MIN_SIDE_WEIGHT {
            continue;
        }
        let height = normalized_height(vertex.position, mesh.bbox_min.y, height_span);
        let radius = ((vertex.position.x - center_x).powi(2)
            + (vertex.position.z - center_z).powi(2))
        .sqrt();
        if (0.48..0.535).contains(&height) {
            elbow.push(radius);
        } else if (0.535..0.59).contains(&height) {
            bicep.push(radius);
        } else if (0.59..0.66).contains(&height) {
            shoulder.push(radius);
        }
    }
    (
        average_or_zero(&elbow),
        average_or_zero(&bicep),
        average_or_zero(&shoulder),
    )
}

fn average_or_zero(values: &[f32]) -> f32 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
}

fn recompute_source_bbox(mesh: &mut SourceMesh) {
    let mut min = Vec3 {
        x: f32::INFINITY,
        y: f32::INFINITY,
        z: f32::INFINITY,
    };
    let mut max = Vec3 {
        x: f32::NEG_INFINITY,
        y: f32::NEG_INFINITY,
        z: f32::NEG_INFINITY,
    };
    for vertex in &mesh.vertices {
        min.x = min.x.min(vertex.position.x);
        min.y = min.y.min(vertex.position.y);
        min.z = min.z.min(vertex.position.z);
        max.x = max.x.max(vertex.position.x);
        max.y = max.y.max(vertex.position.y);
        max.z = max.z.max(vertex.position.z);
    }
    mesh.bbox_min = min;
    mesh.bbox_max = max;
}

fn prune_broken_arm_islands(
    mesh: &SourceMesh,
    accum: &[[f32; 256]],
    donor_lookup: &DonorBoneLookup,
) -> Result<ArmIslandPruneReport, Box<dyn std::error::Error>> {
    let left_upper = resolve_required_bone_u8(donor_lookup, "L_UpperArm")?;
    let left_forearm = resolve_required_bone_u8(donor_lookup, "L_Forearm")?;
    let left_hand = resolve_required_bone_u8(donor_lookup, "L_Hand")?;
    let right_upper = resolve_required_bone_u8(donor_lookup, "R_UpperArm")?;
    let right_forearm = resolve_required_bone_u8(donor_lookup, "R_Forearm")?;
    let right_hand = resolve_required_bone_u8(donor_lookup, "R_Hand")?;
    let spine1 = resolve_required_bone_u8(donor_lookup, "Spine1")?;
    let spine2 = resolve_required_bone_u8(donor_lookup, "Spine2")?;
    let components = find_arm_components(
        mesh,
        accum,
        left_upper,
        left_forearm,
        left_hand,
        right_upper,
        right_forearm,
        right_hand,
    );
    let arm_bones = [
        left_upper,
        left_forearm,
        left_hand,
        right_upper,
        right_forearm,
        right_hand,
    ];
    let body_vertices = non_arm_vertices(mesh, accum, &arm_bones);
    let broken_components = components
        .iter()
        .filter(|component| {
            is_broken_arm_island(
                mesh,
                accum,
                component,
                &body_vertices,
                left_upper,
                left_forearm,
                left_hand,
                right_upper,
                right_forearm,
                right_hand,
                spine1,
                spine2,
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let triangles_before = mesh.triangles.len();
    let response = if broken_components.is_empty() {
        "no_broken_detached_arm_islands_found"
    } else {
        ARM_FOREARM_SURFACE_PRESERVE_RESPONSE
    };
    Ok(ArmIslandPruneReport {
        enabled: true,
        components_before: components.len(),
        broken_components_before: broken_components.len(),
        broken_components_after: 0,
        pruned_components: 0,
        pruned_vertices: 0,
        pruned_triangles: 0,
        triangles_before,
        triangles_after: mesh.triangles.len(),
        response: response.to_string(),
    })
}

fn non_arm_vertices(mesh: &SourceMesh, accum: &[[f32; 256]], arm_bones: &[u8]) -> Vec<usize> {
    (0..mesh.vertices.len())
        .filter(|index| {
            sum_bone_weights(&accum[*index], arm_bones) < ARM_COMPENSATION_MIN_ARM_WEIGHT
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn is_broken_arm_island(
    mesh: &SourceMesh,
    accum: &[[f32; 256]],
    component: &ArmComponent,
    non_arm_vertices: &[usize],
    left_upper: u8,
    left_forearm: u8,
    left_hand: u8,
    right_upper: u8,
    right_forearm: u8,
    right_hand: u8,
    spine1: u8,
    spine2: u8,
) -> bool {
    if component.vertices.len() < ARM_BROKEN_ISLAND_MIN_VERTICES {
        return false;
    }
    let nearest_body_distance =
        nearest_component_distance(mesh, &component.vertices, non_arm_vertices);
    if nearest_body_distance < ARM_BROKEN_ISLAND_DISTANCE {
        return false;
    }
    let (upper, forearm, hand) = match component.side {
        ArmSide::Left => (left_upper, left_forearm, left_hand),
        ArmSide::Right => (right_upper, right_forearm, right_hand),
    };
    let divisor = component.vertices.len().max(1) as f32;
    let mut spine_sum = 0.0;
    let mut upper_sum = 0.0;
    let mut distal_sum = 0.0;
    for vertex in &component.vertices {
        let row = &accum[*vertex];
        spine_sum += row[spine1 as usize] + row[spine2 as usize];
        upper_sum += row[upper as usize];
        distal_sum += row[forearm as usize] + row[hand as usize];
    }
    let avg_spine = spine_sum / divisor;
    let avg_upper = upper_sum / divisor;
    let avg_distal = distal_sum / divisor;
    avg_spine <= ARM_BROKEN_ISLAND_MAX_SPINE
        && avg_upper <= ARM_BROKEN_ISLAND_MAX_UPPER
        && avg_distal >= ARM_BROKEN_ISLAND_MIN_DISTAL
}

fn compensate_arm_weights(
    mesh: &SourceMesh,
    accum: &mut [[f32; 256]],
    donor_lookup: &DonorBoneLookup,
) -> Result<ArmCompensationReport, Box<dyn std::error::Error>> {
    let left_upper = resolve_required_bone_u8(donor_lookup, "L_UpperArm")?;
    let left_forearm = resolve_required_bone_u8(donor_lookup, "L_Forearm")?;
    let left_hand = resolve_required_bone_u8(donor_lookup, "L_Hand")?;
    let right_upper = resolve_required_bone_u8(donor_lookup, "R_UpperArm")?;
    let right_forearm = resolve_required_bone_u8(donor_lookup, "R_Forearm")?;
    let right_hand = resolve_required_bone_u8(donor_lookup, "R_Hand")?;
    let spine1 = resolve_required_bone_u8(donor_lookup, "Spine1")?;
    let spine2 = resolve_required_bone_u8(donor_lookup, "Spine2")?;
    let center_x = (mesh.bbox_min.x + mesh.bbox_max.x) * 0.5;

    let components = find_arm_components(
        mesh,
        accum,
        left_upper,
        left_forearm,
        left_hand,
        right_upper,
        right_forearm,
        right_hand,
    );
    if components.is_empty() {
        return Ok(ArmCompensationReport {
            enabled: true,
            detached_island_response: "no_arm_components_found".to_string(),
            ..ArmCompensationReport::default()
        });
    }
    let classified = classify_arm_components(
        mesh,
        accum,
        components,
        left_upper,
        left_forearm,
        left_hand,
        right_upper,
        right_forearm,
        right_hand,
    );
    let metric_components = classified
        .iter()
        .map(|classified| classified.component.clone())
        .collect::<Vec<_>>();
    let before = arm_compensation_metrics(
        accum,
        &metric_components,
        left_upper,
        left_forearm,
        left_hand,
        right_upper,
        right_forearm,
        right_hand,
        spine1,
        spine2,
        center_x,
        mesh,
    );

    for classified_component in &classified {
        match classified_component.attachment {
            ArmAttachment::BodyNear => apply_arm_component_gradient(
                mesh,
                accum,
                &classified_component.component,
                center_x,
                left_upper,
                left_forearm,
                left_hand,
                right_upper,
                right_forearm,
                right_hand,
                spine1,
                spine2,
            ),
            ArmAttachment::Detached => apply_detached_arm_proxy(
                accum,
                &classified_component.component,
                left_upper,
                left_forearm,
                left_hand,
                right_upper,
                right_forearm,
                right_hand,
                spine1,
                spine2,
            ),
        }
    }

    let after = arm_compensation_metrics(
        accum,
        &metric_components,
        left_upper,
        left_forearm,
        left_hand,
        right_upper,
        right_forearm,
        right_hand,
        spine1,
        spine2,
        center_x,
        mesh,
    );

    let (left_components, right_components) = component_side_counts(&metric_components);
    let (left_vertices, right_vertices) = component_side_vertices(&metric_components);
    let detached_components = classified
        .iter()
        .filter(|component| matches!(component.attachment, ArmAttachment::Detached))
        .count();
    let detached_vertices = classified
        .iter()
        .filter(|component| matches!(component.attachment, ArmAttachment::Detached))
        .map(|component| component.component.vertices.len())
        .sum::<usize>();
    let max_detached_body_distance = classified
        .iter()
        .filter(|component| matches!(component.attachment, ArmAttachment::Detached))
        .map(|component| component.nearest_body_distance)
        .fold(0.0_f32, f32::max);
    Ok(ArmCompensationReport {
        enabled: true,
        compensated_vertices: after.vertices,
        left_components,
        right_components,
        left_vertices,
        right_vertices,
        avg_upper_before: before.avg_upper,
        avg_upper_after: after.avg_upper,
        avg_forearm_hand_before: before.avg_forearm_hand,
        avg_forearm_hand_after: after.avg_forearm_hand,
        avg_body_tween_after: after.avg_body_tween,
        distal_overweighted_components_before: before.distal_overweighted_components,
        distal_overweighted_components_after: after.distal_overweighted_components,
        weak_shoulder_components_before: before.weak_shoulder_components,
        weak_shoulder_components_after: after.weak_shoulder_components,
        detached_components,
        detached_vertices,
        detached_proxy_vertices: detached_vertices,
        independent_detached_components: 0,
        max_detached_body_distance,
        detached_island_response: "body_proxy_low_hand".to_string(),
    })
}

fn find_arm_components(
    mesh: &SourceMesh,
    accum: &[[f32; 256]],
    left_upper: u8,
    left_forearm: u8,
    left_hand: u8,
    right_upper: u8,
    right_forearm: u8,
    right_hand: u8,
) -> Vec<ArmComponent> {
    let mut components = Vec::new();
    for (side, bones) in [
        (ArmSide::Left, [left_upper, left_forearm, left_hand]),
        (ArmSide::Right, [right_upper, right_forearm, right_hand]),
    ] {
        let candidates: HashSet<usize> = mesh
            .vertices
            .iter()
            .enumerate()
            .filter_map(|(index, _)| {
                (sum_bone_weights(&accum[index], &bones) >= ARM_COMPENSATION_MIN_ARM_WEIGHT)
                    .then_some(index)
            })
            .collect();
        for vertices in connected_components(mesh, &candidates) {
            if vertices.len() >= ARM_COMPENSATION_MIN_COMPONENT_VERTICES {
                components.push(ArmComponent { side, vertices });
            }
        }
    }
    components
}

fn classify_arm_components(
    mesh: &SourceMesh,
    accum: &[[f32; 256]],
    components: Vec<ArmComponent>,
    left_upper: u8,
    left_forearm: u8,
    left_hand: u8,
    right_upper: u8,
    right_forearm: u8,
    right_hand: u8,
) -> Vec<ClassifiedArmComponent> {
    let arm_bones = [
        left_upper,
        left_forearm,
        left_hand,
        right_upper,
        right_forearm,
        right_hand,
    ];
    let non_arm_vertices = (0..mesh.vertices.len())
        .filter(|index| {
            sum_bone_weights(&accum[*index], &arm_bones) < ARM_COMPENSATION_MIN_ARM_WEIGHT
        })
        .collect::<Vec<_>>();
    components
        .into_iter()
        .map(|component| {
            let nearest_body_distance =
                nearest_component_distance(mesh, &component.vertices, &non_arm_vertices);
            let attachment = if nearest_body_distance <= ARM_BODY_ATTACHMENT_DISTANCE {
                ArmAttachment::BodyNear
            } else {
                ArmAttachment::Detached
            };
            ClassifiedArmComponent {
                component,
                attachment,
                nearest_body_distance,
            }
        })
        .collect()
}

fn nearest_component_distance(mesh: &SourceMesh, first: &[usize], second: &[usize]) -> f32 {
    let mut best = f32::INFINITY;
    for first_vertex in first {
        let first_position = mesh.vertices[*first_vertex].position;
        for second_vertex in second {
            best = best.min(vec3_distance(
                first_position,
                mesh.vertices[*second_vertex].position,
            ));
        }
    }
    best
}

fn connected_components(mesh: &SourceMesh, candidates: &HashSet<usize>) -> Vec<Vec<usize>> {
    let mut adjacency: HashMap<usize, Vec<usize>> = HashMap::new();
    for (a, b) in unique_triangle_edges(&mesh.triangles) {
        let a = a as usize;
        let b = b as usize;
        if candidates.contains(&a) && candidates.contains(&b) {
            adjacency.entry(a).or_default().push(b);
            adjacency.entry(b).or_default().push(a);
        }
    }
    let mut seen = HashSet::new();
    let mut components = Vec::new();
    for candidate in candidates {
        if !seen.insert(*candidate) {
            continue;
        }
        let mut stack = vec![*candidate];
        let mut component = Vec::new();
        while let Some(vertex) = stack.pop() {
            component.push(vertex);
            if let Some(neighbors) = adjacency.get(&vertex) {
                for neighbor in neighbors {
                    if seen.insert(*neighbor) {
                        stack.push(*neighbor);
                    }
                }
            }
        }
        components.push(component);
    }
    components.sort_by_key(|component| std::cmp::Reverse(component.len()));
    components
}

fn apply_arm_component_gradient(
    mesh: &SourceMesh,
    accum: &mut [[f32; 256]],
    component: &ArmComponent,
    center_x: f32,
    left_upper: u8,
    left_forearm: u8,
    left_hand: u8,
    right_upper: u8,
    right_forearm: u8,
    right_hand: u8,
    spine1: u8,
    spine2: u8,
) {
    let laterals = component
        .vertices
        .iter()
        .map(|vertex| {
            arm_lateral_progress_axis(component.side, mesh.vertices[*vertex].position.x, center_x)
        })
        .collect::<Vec<_>>();
    let min_lateral = laterals.iter().copied().fold(f32::INFINITY, f32::min);
    let max_lateral = laterals.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let lateral_span = (max_lateral - min_lateral).max(f32::EPSILON);
    let (upper, forearm, hand) = match component.side {
        ArmSide::Left => (left_upper, left_forearm, left_hand),
        ArmSide::Right => (right_upper, right_forearm, right_hand),
    };

    for vertex in &component.vertices {
        let lateral =
            arm_lateral_progress_axis(component.side, mesh.vertices[*vertex].position.x, center_x);
        let progress = ((lateral - min_lateral) / lateral_span).clamp(0.0, 1.0);
        let targets = shoulder_to_hand_targets(progress, upper, forearm, hand, spine1, spine2);
        blend_to_targets(&mut accum[*vertex], &targets, ARM_COMPENSATION_STRENGTH);
    }
}

fn apply_detached_arm_proxy(
    accum: &mut [[f32; 256]],
    component: &ArmComponent,
    left_upper: u8,
    left_forearm: u8,
    left_hand: u8,
    right_upper: u8,
    right_forearm: u8,
    right_hand: u8,
    spine1: u8,
    spine2: u8,
) {
    let (upper, forearm, hand) = match component.side {
        ArmSide::Left => (left_upper, left_forearm, left_hand),
        ArmSide::Right => (right_upper, right_forearm, right_hand),
    };
    let targets = [
        (spine1, 0.20),
        (spine2, 0.18),
        (upper, 0.40),
        (forearm, 0.18),
        (hand, 0.04),
    ];
    for vertex in &component.vertices {
        blend_to_targets(&mut accum[*vertex], &targets, ARM_COMPENSATION_STRENGTH);
    }
}

fn arm_lateral_progress_axis(side: ArmSide, x: f32, center_x: f32) -> f32 {
    match side {
        ArmSide::Left => x - center_x,
        ArmSide::Right => center_x - x,
    }
}

fn shoulder_to_hand_targets(
    progress: f32,
    upper: u8,
    forearm: u8,
    hand: u8,
    spine1: u8,
    spine2: u8,
) -> [(u8, f32); 5] {
    let body = ARM_ROOT_TWEEN_FRACTION * (1.0 - progress / 0.35).clamp(0.0, 1.0);
    let (upper_weight, forearm_weight, hand_weight) = if progress < 0.22 {
        let t = (progress / 0.22).clamp(0.0, 1.0);
        (lerp(0.68, 0.62, t), lerp(0.12, 0.26, t), lerp(0.0, 0.02, t))
    } else if progress < 0.66 {
        let t = ((progress - 0.22) / 0.44).clamp(0.0, 1.0);
        (
            lerp(0.62, 0.34, t),
            lerp(0.26, 0.52, t),
            lerp(0.02, 0.14, t),
        )
    } else {
        let t = ((progress - 0.66) / 0.34).clamp(0.0, 1.0);
        (
            lerp(0.34, 0.14, t),
            lerp(0.52, 0.44, t),
            lerp(0.14, 0.42, t),
        )
    };
    let limb_total = (1.0 - body).max(0.0);
    let limb_sum = upper_weight + forearm_weight + hand_weight;
    [
        (spine1, body * 0.60),
        (spine2, body * 0.40),
        (upper, limb_total * upper_weight / limb_sum),
        (forearm, limb_total * forearm_weight / limb_sum),
        (hand, limb_total * hand_weight / limb_sum),
    ]
}

fn blend_to_targets(weights: &mut [f32; 256], targets: &[(u8, f32)], strength: f32) {
    let total = weights.iter().sum::<f32>();
    if total <= 0.0 {
        return;
    }
    for weight in weights.iter_mut() {
        *weight *= 1.0 - strength;
    }
    for (bone, target_weight) in targets {
        weights[*bone as usize] += total * strength * *target_weight;
    }
    normalize_accumulated_weights(weights);
}

#[allow(clippy::too_many_arguments)]
fn arm_compensation_metrics(
    accum: &[[f32; 256]],
    components: &[ArmComponent],
    left_upper: u8,
    left_forearm: u8,
    left_hand: u8,
    right_upper: u8,
    right_forearm: u8,
    right_hand: u8,
    spine1: u8,
    spine2: u8,
    center_x: f32,
    mesh: &SourceMesh,
) -> ArmCompensationMetrics {
    let mut metrics = ArmCompensationMetrics::default();
    let mut upper_sum = 0.0;
    let mut forearm_hand_sum = 0.0;
    let mut body_tween_sum = 0.0;
    for component in components {
        let (upper, forearm, hand) = match component.side {
            ArmSide::Left => (left_upper, left_forearm, left_hand),
            ArmSide::Right => (right_upper, right_forearm, right_hand),
        };
        let mut component_upper = 0.0;
        let mut component_forearm_hand = 0.0;
        for vertex in &component.vertices {
            metrics.vertices += 1;
            let row = &accum[*vertex];
            upper_sum += row[upper as usize];
            forearm_hand_sum += row[forearm as usize] + row[hand as usize];
            body_tween_sum += row[spine1 as usize] + row[spine2 as usize];
            component_upper += row[upper as usize];
            component_forearm_hand += row[forearm as usize] + row[hand as usize];
        }
        let divisor = component.vertices.len().max(1) as f32;
        let avg_upper = component_upper / divisor;
        let avg_forearm_hand = component_forearm_hand / divisor;
        if avg_forearm_hand > ARM_DISTAL_OVERWEIGHT_THRESHOLD && avg_upper < ARM_LOW_UPPER_THRESHOLD
        {
            metrics.distal_overweighted_components += 1;
        }
        if root_band_upper_body_average(accum, component, upper, spine1, spine2, center_x, mesh)
            < 0.42
        {
            metrics.weak_shoulder_components += 1;
        }
    }
    let divisor = metrics.vertices.max(1) as f32;
    metrics.avg_upper = upper_sum / divisor;
    metrics.avg_forearm_hand = forearm_hand_sum / divisor;
    metrics.avg_body_tween = body_tween_sum / divisor;
    metrics
}

fn root_band_upper_body_average(
    accum: &[[f32; 256]],
    component: &ArmComponent,
    upper: u8,
    spine1: u8,
    spine2: u8,
    center_x: f32,
    mesh: &SourceMesh,
) -> f32 {
    let laterals = component
        .vertices
        .iter()
        .map(|vertex| {
            arm_lateral_progress_axis(component.side, mesh.vertices[*vertex].position.x, center_x)
        })
        .collect::<Vec<_>>();
    let min_lateral = laterals.iter().copied().fold(f32::INFINITY, f32::min);
    let max_lateral = laterals.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let lateral_span = (max_lateral - min_lateral).max(f32::EPSILON);
    let mut sum = 0.0;
    let mut count = 0;
    for vertex in &component.vertices {
        let lateral =
            arm_lateral_progress_axis(component.side, mesh.vertices[*vertex].position.x, center_x);
        let progress = ((lateral - min_lateral) / lateral_span).clamp(0.0, 1.0);
        if progress <= ARM_ROOT_TWEEN_FRACTION {
            let row = &accum[*vertex];
            sum += row[upper as usize] + row[spine1 as usize] + row[spine2 as usize];
            count += 1;
        }
    }
    sum / count.max(1) as f32
}

fn component_side_counts(components: &[ArmComponent]) -> (usize, usize) {
    let left = components
        .iter()
        .filter(|component| matches!(component.side, ArmSide::Left))
        .count();
    (left, components.len() - left)
}

fn component_side_vertices(components: &[ArmComponent]) -> (usize, usize) {
    let left = components
        .iter()
        .filter(|component| matches!(component.side, ArmSide::Left))
        .map(|component| component.vertices.len())
        .sum::<usize>();
    let total = components
        .iter()
        .map(|component| component.vertices.len())
        .sum::<usize>();
    (left, total - left)
}

fn apply_region_responses(
    mesh: &SourceMesh,
    accum: &mut [[f32; 256]],
    region_map_tsv: &PathBuf,
    _donor_lookup: &DonorBoneLookup,
) -> Result<RegionResponseReport, Box<dyn std::error::Error>> {
    let regions = read_region_map_tsv(region_map_tsv)?;
    let authored_feet = match regions.get("feet") {
        Some(region) if !region.is_empty() => region.clone(),
        _ => {
            return Ok(RegionResponseReport {
                enabled: true,
                region_map_path: region_map_tsv.display().to_string(),
                response: "no_feet_region_found".to_string(),
                ..RegionResponseReport::default()
            });
        }
    };

    let expanded_feet = face_closed_region(mesh, &authored_feet, FaceClosureMode::Expand);
    let shrunk_feet = face_closed_region(mesh, &authored_feet, FaceClosureMode::Shrink);
    let mixed_before = mixed_triangle_count(mesh, &authored_feet);
    let boundary_before = boundary_edge_count(mesh, &authored_feet);
    let near_pairs = near_shell_pairs(mesh, &authored_feet, REGION_NEAR_SHELL_DISTANCE);
    let near_shell_outside_vertices = near_pairs
        .iter()
        .map(|pair| pair.outside_vertex)
        .collect::<HashSet<_>>()
        .len();

    let mut max_mismatch = 0.0_f32;
    let mut mismatch_pairs = 0_usize;
    let mut synced_vertices = HashSet::new();
    for pair in near_pairs.iter().copied() {
        let mismatch = weight_l1_distance(&accum[pair.region_vertex], &accum[pair.outside_vertex]);
        max_mismatch = max_mismatch.max(mismatch);
        if mismatch <= REGION_WEIGHT_SYNC_L1_THRESHOLD {
            continue;
        }
        mismatch_pairs += 1;
        let proximity_strength = (1.0 - pair.distance / REGION_NEAR_SHELL_DISTANCE).clamp(0.0, 1.0);
        let strength = REGION_WEIGHT_SYNC_MAX_STRENGTH * proximity_strength;
        if strength <= 0.0 {
            continue;
        }
        blend_weight_rows(accum, pair.region_vertex, pair.outside_vertex, strength);
        synced_vertices.insert(pair.region_vertex);
        synced_vertices.insert(pair.outside_vertex);
    }

    let response = if mismatch_pairs == 0 {
        "alert_only_no_weight_mismatch"
    } else {
        "local_proximity_weight_sync"
    };

    Ok(RegionResponseReport {
        enabled: true,
        region_map_path: region_map_tsv.display().to_string(),
        feet_authored_vertices: authored_feet.len(),
        feet_expanded_vertices: expanded_feet.len(),
        feet_shrunk_vertices: shrunk_feet.len(),
        feet_normalized_vertices: authored_feet.len(),
        feet_blend_band_vertices: 0,
        feet_mixed_triangles_before: mixed_before,
        feet_mixed_triangles_after: mixed_before,
        feet_boundary_edges_before: boundary_before,
        feet_boundary_edges_after: boundary_before,
        feet_near_shell_pairs: near_pairs.len(),
        feet_near_shell_outside_vertices: near_shell_outside_vertices,
        feet_weight_mismatch_pairs: mismatch_pairs,
        feet_weight_sync_vertices: synced_vertices.len(),
        feet_max_weight_l1_mismatch: max_mismatch,
        response: response.to_string(),
    })
}

fn read_region_map_tsv(
    path: &PathBuf,
) -> Result<HashMap<String, HashSet<usize>>, Box<dyn std::error::Error>> {
    let text = fs::read_to_string(path)?;
    let mut regions: HashMap<String, HashSet<usize>> = HashMap::new();
    for (line_index, line) in text.lines().enumerate() {
        if line_index == 0 || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() != 2 {
            return Err(format!("malformed region TSV line {}: {line}", line_index + 1).into());
        }
        let region = cols[0].to_string();
        let vertex_index = cols[1].parse::<usize>()?;
        regions.entry(region).or_default().insert(vertex_index);
    }
    Ok(regions)
}

#[derive(Clone, Copy)]
enum FaceClosureMode {
    Expand,
    Shrink,
}

#[derive(Clone, Copy)]
struct NearShellPair {
    region_vertex: usize,
    outside_vertex: usize,
    distance: f32,
}

fn face_closed_region(
    mesh: &SourceMesh,
    region: &HashSet<usize>,
    mode: FaceClosureMode,
) -> HashSet<usize> {
    let mut closed = region.clone();
    let mut changed = true;
    let mut iterations = 0;
    while changed && iterations < 32 {
        changed = false;
        iterations += 1;
        for triangle in &mesh.triangles {
            let vertices = [
                triangle[0] as usize,
                triangle[1] as usize,
                triangle[2] as usize,
            ];
            let inside = vertices
                .iter()
                .filter(|vertex| closed.contains(vertex))
                .count();
            if inside == 0 || inside == 3 {
                continue;
            }
            let before = closed.len();
            match mode {
                FaceClosureMode::Expand => {
                    closed.extend(vertices);
                }
                FaceClosureMode::Shrink => {
                    for vertex in vertices {
                        closed.remove(&vertex);
                    }
                }
            }
            changed |= before != closed.len();
        }
    }
    closed
}

fn mixed_triangle_count(mesh: &SourceMesh, region: &HashSet<usize>) -> usize {
    mesh.triangles
        .iter()
        .filter(|triangle| {
            let inside = triangle
                .iter()
                .filter(|vertex| region.contains(&(**vertex as usize)))
                .count();
            inside > 0 && inside < 3
        })
        .count()
}

fn boundary_edge_count(mesh: &SourceMesh, region: &HashSet<usize>) -> usize {
    unique_triangle_edges(&mesh.triangles)
        .into_iter()
        .filter(|(a, b)| region.contains(&(*a as usize)) != region.contains(&(*b as usize)))
        .count()
}

fn near_shell_pairs(
    mesh: &SourceMesh,
    region: &HashSet<usize>,
    max_distance: f32,
) -> Vec<NearShellPair> {
    let mut pairs = Vec::new();
    for region_vertex in region {
        let position = mesh.vertices[*region_vertex].position;
        for (outside_vertex, outside) in mesh.vertices.iter().enumerate() {
            if region.contains(&outside_vertex) || *region_vertex == outside_vertex {
                continue;
            }
            let distance = vec3_distance(position, outside.position);
            if distance <= max_distance {
                pairs.push(NearShellPair {
                    region_vertex: *region_vertex,
                    outside_vertex,
                    distance,
                });
            }
        }
    }
    pairs
}

fn vec3_distance(a: Vec3, b: Vec3) -> f32 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

fn weight_l1_distance(a: &[f32; 256], b: &[f32; 256]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(a_weight, b_weight)| (a_weight - b_weight).abs())
        .sum::<f32>()
}

fn blend_weight_rows(accum: &mut [[f32; 256]], a: usize, b: usize, strength: f32) {
    let mut average = [0.0_f32; 256];
    for (index, slot) in average.iter_mut().enumerate() {
        *slot = (accum[a][index] + accum[b][index]) * 0.5;
    }
    for index in 0..256 {
        accum[a][index] = accum[a][index] * (1.0 - strength) + average[index] * strength;
        accum[b][index] = accum[b][index] * (1.0 - strength) + average[index] * strength;
    }
    normalize_accumulated_weights(&mut accum[a]);
    normalize_accumulated_weights(&mut accum[b]);
}

fn patch_donor_flver(
    bytes: &mut [u8],
    source: &SourceMesh,
    donor_mesh_index: usize,
) -> Result<PatchReport, Box<dyn std::error::Error>> {
    let header = parse_header(bytes)?;
    if header.version != 0x2001A {
        return Err(format!(
            "expected ER donor FLVER 0x2001A, got 0x{:X}",
            header.version
        )
        .into());
    }
    let table_start = HEADER_SIZE;
    let bone_table =
        table_start + DUMMY_SIZE * header.dummy_count + MATERIAL_SIZE * header.material_count;
    let mesh_table = bone_table + BONE_SIZE * header.bone_count;
    let face_set_table = mesh_table + MESH_SIZE * header.mesh_count;
    let vertex_buffer_table = face_set_table + FACE_SET_SIZE * header.face_set_count;
    let layout_table = vertex_buffer_table + VERTEX_BUFFER_SIZE * header.vertex_buffer_count;

    let meshes = parse_meshes(bytes, mesh_table, header.mesh_count)?;
    let face_sets = parse_face_sets(bytes, face_set_table, header.face_set_count)?;
    let vertex_buffers =
        parse_vertex_buffers(bytes, vertex_buffer_table, header.vertex_buffer_count)?;
    let layouts = parse_layouts(bytes, layout_table, header.buffer_layout_count)?;

    let donor_mesh = *meshes
        .get(donor_mesh_index)
        .ok_or_else(|| format!("donor mesh index {donor_mesh_index} out of range"))?;
    let donor_mesh_table_offset = mesh_table + donor_mesh_index * MESH_SIZE;
    // Route A uses mesh 1 for capacity, but material 0 is the donor's fabric slot,
    // which is a better first-pass shader family for an organic mushroom body than
    // mesh 1's original metal slot.
    write_u32(bytes, donor_mesh_table_offset + 0x04, 0)?;
    let vertex_buffer_indices = parse_u32_list(
        bytes,
        donor_mesh.vertex_buffer_offset,
        donor_mesh.vertex_buffer_count,
    )?;
    let vertex_buffer_index = *vertex_buffer_indices
        .first()
        .ok_or("selected donor mesh has no vertex buffers")? as usize;
    let vertex_buffer = *vertex_buffers
        .get(vertex_buffer_index)
        .ok_or("selected donor vertex buffer index out of range")?;
    if vertex_buffer.vertex_count < source.vertices.len() {
        return Err(format!(
            "donor vertex buffer too small: capacity={} source={}",
            vertex_buffer.vertex_count,
            source.vertices.len()
        )
        .into());
    }
    let layout = *layouts
        .get(vertex_buffer.layout_index)
        .ok_or("selected donor vertex buffer layout index out of range")?;
    let layout_members = parse_layout_members(bytes, layout.member_offset, layout.member_count)?;
    patch_vertices(bytes, header, vertex_buffer, &layout_members, source)?;
    update_header_bbox(bytes, source.bbox_min, source.bbox_max)?;
    if donor_mesh.bounding_box_offset != 0 {
        write_bbox(
            bytes,
            donor_mesh.bounding_box_offset,
            source.bbox_min,
            source.bbox_max,
        )?;
    }

    let donor_face_set_indices =
        parse_u32_list(bytes, donor_mesh.face_set_offset, donor_mesh.face_set_count)?;
    let source_index_count = source.triangles.len() * 3;
    let (primary_face_set_index, primary_face_set) = donor_face_set_indices
        .iter()
        .copied()
        .filter_map(|index| {
            face_sets
                .get(index as usize)
                .copied()
                .map(|face_set| (index as usize, face_set))
        })
        .find(|(_, face_set)| {
            !face_set.triangle_strip && face_set.index_count >= source_index_count
        })
        .ok_or_else(|| {
            format!(
                "no selected donor face set can hold source indices: need={} selected={:?}",
                source_index_count, donor_face_set_indices
            )
        })?;
    patch_face_set_indices(bytes, header, primary_face_set, &source.triangles)?;

    let mut patched_face_sets = 0;
    let mut hidden_face_sets = 0;
    let lod0_index_capacity = primary_face_set.index_count;
    for (mesh_index, mesh) in meshes.iter().enumerate() {
        let indices = parse_u32_list(bytes, mesh.face_set_offset, mesh.face_set_count)?;
        for face_set_index in indices {
            let face_set = *face_sets
                .get(face_set_index as usize)
                .ok_or("mesh references missing face set")?;
            if mesh_index == donor_mesh_index {
                if face_set.triangle_strip {
                    return Err(
                        "selected donor face set is a triangle strip; expected triangle list"
                            .into(),
                    );
                }
                if face_set_index as usize != primary_face_set_index {
                    redirect_face_set_indices(
                        bytes,
                        face_set_table,
                        face_set_index as usize,
                        primary_face_set,
                    )?;
                }
                patched_face_sets += 1;
            } else {
                zero_face_set_indices(bytes, header, face_set)?;
                hidden_face_sets += 1;
            }
        }
    }

    Ok(PatchReport {
        vertex_capacity: vertex_buffer.vertex_count,
        lod0_index_capacity,
        patched_face_sets,
        hidden_face_sets,
    })
}

fn patch_vertices(
    bytes: &mut [u8],
    header: Header,
    vertex_buffer: VertexBuffer,
    layout_members: &[LayoutMember],
    source: &SourceMesh,
) -> Result<(), Box<dyn std::error::Error>> {
    let buffer_start = header.data_offset + vertex_buffer.buffer_offset;
    bounds(bytes, buffer_start, vertex_buffer.buffer_length)?;
    let uv_factor = if header.version >= 0x2000F {
        2048.0
    } else {
        1024.0
    };
    for vertex_index in 0..vertex_buffer.vertex_count {
        let source_vertex = source
            .vertices
            .get(vertex_index)
            .copied()
            .unwrap_or_default();
        let vertex_start = buffer_start + vertex_index * vertex_buffer.vertex_size;
        for member in layout_members {
            let off = vertex_start + member.struct_offset;
            match (member.semantic_id, member.format_id, member.index) {
                (0, 0x02, _) => write_vec3(bytes, off, source_vertex.position)?,
                (3, 0x10, _) | (3, 0x11, _) | (3, 0x13, _) | (3, 0x2F, _) => {
                    write_snorm8x4(
                        bytes,
                        off,
                        [
                            source_vertex.normal.x,
                            source_vertex.normal.y,
                            source_vertex.normal.z,
                            1.0,
                        ],
                    )?;
                }
                (6, 0x10, _) | (6, 0x11, _) | (6, 0x13, _) | (6, 0x2F, _) => {
                    write_snorm8x4(bytes, off, [1.0, 0.0, 0.0, 1.0])?;
                }
                (7, 0x10, _) | (7, 0x11, _) | (7, 0x13, _) | (7, 0x2F, _) => {
                    write_snorm8x4(bytes, off, [0.0, 1.0, 0.0, 1.0])?;
                }
                (2, 0x11, _) | (2, 0x24, _) => write_u8x4(bytes, off, source_vertex.bone_indices)?,
                (2, 0x18, _) => write_u16x4(bytes, off, source_vertex.bone_indices)?,
                (1, 0x13, _) => write_unorm8x4(bytes, off, source_vertex.bone_weights)?,
                (1, 0x16, _) | (1, 0x1A, _) => {
                    write_snorm16x4(bytes, off, source_vertex.bone_weights)?
                }
                (10, 0x13, _) | (10, 0x10, _) | (10, 0x11, _) | (10, 0x2F, _) => {
                    write_u8x4(bytes, off, [255, 255, 255, 255])?;
                }
                (5, 0x15, _) | (5, 0x12, _) | (5, 0x10, _) | (5, 0x11, _) | (5, 0x13, _) => {
                    write_uv_i16(bytes, off, source_vertex.uv, uv_factor)?;
                }
                (5, 0x16, _) | (5, 0x2E, _) => {
                    write_uv_i16(bytes, off, source_vertex.uv, uv_factor)?;
                    write_uv_i16(bytes, off + 4, source_vertex.uv, uv_factor)?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn patch_face_set_indices(
    bytes: &mut [u8],
    header: Header,
    face_set: FaceSet,
    source_triangles: &[[u32; 3]],
) -> Result<(), Box<dyn std::error::Error>> {
    let index_size = resolved_index_size(header, face_set);
    let start = header.data_offset + face_set.index_offset;
    match index_size {
        16 => {
            bounds(bytes, start, face_set.index_count * 2)?;
            for index in 0..face_set.index_count {
                let tri = index / 3;
                let corner = index % 3;
                let value = source_triangles.get(tri).map(|t| t[corner]).unwrap_or(0);
                write_u16(bytes, start + index * 2, value as u16)?;
            }
        }
        32 => {
            bounds(bytes, start, face_set.index_count * 4)?;
            for index in 0..face_set.index_count {
                let tri = index / 3;
                let corner = index % 3;
                let value = source_triangles.get(tri).map(|t| t[corner]).unwrap_or(0);
                write_u32(bytes, start + index * 4, value)?;
            }
        }
        other => return Err(format!("unsupported donor face index size: {other}").into()),
    }
    Ok(())
}

fn redirect_face_set_indices(
    bytes: &mut [u8],
    face_set_table: usize,
    face_set_index: usize,
    primary_face_set: FaceSet,
) -> Result<(), Box<dyn std::error::Error>> {
    let record_offset = face_set_table + face_set_index * FACE_SET_SIZE;
    bounds(bytes, record_offset, FACE_SET_SIZE)?;
    bytes[record_offset + 0x04] = 0;
    write_u32(
        bytes,
        record_offset + 0x08,
        primary_face_set.index_count as u32,
    )?;
    write_u32(
        bytes,
        record_offset + 0x0C,
        primary_face_set.index_offset as u32,
    )?;
    write_u32(bytes, record_offset + 0x18, primary_face_set.index_size)?;
    Ok(())
}

fn zero_face_set_indices(
    bytes: &mut [u8],
    header: Header,
    face_set: FaceSet,
) -> Result<(), Box<dyn std::error::Error>> {
    let index_size = resolved_index_size(header, face_set);
    let start = header.data_offset + face_set.index_offset;
    match index_size {
        16 => {
            bounds(bytes, start, face_set.index_count * 2)?;
            bytes[start..start + face_set.index_count * 2].fill(0);
        }
        32 => {
            bounds(bytes, start, face_set.index_count * 4)?;
            bytes[start..start + face_set.index_count * 4].fill(0);
        }
        other => return Err(format!("unsupported donor face index size: {other}").into()),
    }
    Ok(())
}

fn resolved_index_size(header: Header, face_set: FaceSet) -> u32 {
    if face_set.index_size == 0 {
        header.vertex_index_size
    } else {
        face_set.index_size
    }
}

fn parse_header(bytes: &[u8]) -> Result<Header, Box<dyn std::error::Error>> {
    if bytes.len() < HEADER_SIZE || &bytes[0..6] != b"FLVER\0" || &bytes[6..8] != b"L\0" {
        return Err("expected little-endian FLVER header".into());
    }
    Ok(Header {
        version: read_u32(bytes, 0x08)?,
        data_offset: read_u32(bytes, 0x0C)? as usize,
        dummy_count: read_u32(bytes, 0x14)? as usize,
        material_count: read_u32(bytes, 0x18)? as usize,
        bone_count: read_u32(bytes, 0x1C)? as usize,
        mesh_count: read_u32(bytes, 0x20)? as usize,
        vertex_buffer_count: read_u32(bytes, 0x24)? as usize,
        vertex_index_size: bytes
            .get(0x48)
            .copied()
            .ok_or("missing vertex index size")? as u32,
        face_set_count: read_u32(bytes, 0x50)? as usize,
        buffer_layout_count: read_u32(bytes, 0x54)? as usize,
    })
}

fn bone_index_to_u8(index: u16, bone_name: &str) -> Result<u8, Box<dyn std::error::Error>> {
    if index <= u8::MAX as u16 {
        Ok(index as u8)
    } else {
        Err(format!("donor bone {bone_name} index {index} does not fit in Byte4B weights").into())
    }
}

fn donor_bone_lookup(bytes: &[u8]) -> Result<DonorBoneLookup, Box<dyn std::error::Error>> {
    let header = parse_header(bytes)?;
    let bone_table =
        HEADER_SIZE + DUMMY_SIZE * header.dummy_count + MATERIAL_SIZE * header.material_count;
    let mut by_name = HashMap::new();
    for i in 0..header.bone_count {
        let off = bone_table + i * BONE_SIZE;
        let name_offset = read_u32(bytes, off + 0x0C)? as usize;
        let name = read_flver_string(bytes, name_offset)?;
        if i > u16::MAX as usize {
            return Err(format!("donor bone index {i} does not fit in u16").into());
        }
        by_name.insert(name, i as u16);
    }
    Ok(DonorBoneLookup { by_name })
}

fn read_flver_string(bytes: &[u8], offset: usize) -> Result<String, Box<dyn std::error::Error>> {
    if offset >= bytes.len() {
        return Ok(String::new());
    }
    let mut units = Vec::new();
    let mut cursor = offset;
    loop {
        bounds(bytes, cursor, 2)?;
        let unit = u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
        cursor += 2;
    }
    Ok(String::from_utf16(&units)?)
}

fn parse_meshes(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Result<Vec<Mesh>, Box<dyn std::error::Error>> {
    let mut meshes = Vec::with_capacity(count);
    for i in 0..count {
        let off = offset + i * MESH_SIZE;
        meshes.push(Mesh {
            bounding_box_offset: read_u32(bytes, off + 0x18)? as usize,
            face_set_count: read_u32(bytes, off + 0x20)? as usize,
            face_set_offset: read_u32(bytes, off + 0x24)? as usize,
            vertex_buffer_count: read_u32(bytes, off + 0x28)? as usize,
            vertex_buffer_offset: read_u32(bytes, off + 0x2C)? as usize,
        });
    }
    Ok(meshes)
}

fn parse_face_sets(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Result<Vec<FaceSet>, Box<dyn std::error::Error>> {
    let mut face_sets = Vec::with_capacity(count);
    for i in 0..count {
        let off = offset + i * FACE_SET_SIZE;
        face_sets.push(FaceSet {
            triangle_strip: read_u8(bytes, off + 4)? != 0,
            index_count: read_u32(bytes, off + 8)? as usize,
            index_offset: read_u32(bytes, off + 0x0C)? as usize,
            index_size: read_u32(bytes, off + 0x18)?,
        });
    }
    Ok(face_sets)
}

fn parse_vertex_buffers(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Result<Vec<VertexBuffer>, Box<dyn std::error::Error>> {
    let mut buffers = Vec::with_capacity(count);
    for i in 0..count {
        let off = offset + i * VERTEX_BUFFER_SIZE;
        buffers.push(VertexBuffer {
            layout_index: read_u32(bytes, off + 0x04)? as usize,
            vertex_size: read_u32(bytes, off + 0x08)? as usize,
            vertex_count: read_u32(bytes, off + 0x0C)? as usize,
            buffer_length: read_u32(bytes, off + 0x18)? as usize,
            buffer_offset: read_u32(bytes, off + 0x1C)? as usize,
        });
    }
    Ok(buffers)
}

fn parse_layouts(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Result<Vec<Layout>, Box<dyn std::error::Error>> {
    let mut layouts = Vec::with_capacity(count);
    for i in 0..count {
        let off = offset + i * BUFFER_LAYOUT_SIZE;
        layouts.push(Layout {
            member_count: read_u32(bytes, off)? as usize,
            member_offset: read_u32(bytes, off + 0x0C)? as usize,
        });
    }
    Ok(layouts)
}

fn parse_layout_members(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Result<Vec<LayoutMember>, Box<dyn std::error::Error>> {
    let mut members = Vec::with_capacity(count);
    for i in 0..count {
        let off = offset + i * LAYOUT_MEMBER_SIZE;
        members.push(LayoutMember {
            struct_offset: read_u32(bytes, off + 0x04)? as usize,
            format_id: read_u32(bytes, off + 0x08)?,
            semantic_id: read_u32(bytes, off + 0x0C)?,
            index: read_u32(bytes, off + 0x10)?,
        });
    }
    Ok(members)
}

fn parse_u32_list(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    bounds(bytes, offset, count * 4)?;
    let mut values = Vec::with_capacity(count);
    for i in 0..count {
        values.push(read_u32(bytes, offset + i * 4)?);
    }
    Ok(values)
}

fn update_header_bbox(
    bytes: &mut [u8],
    min: Vec3,
    max: Vec3,
) -> Result<(), Box<dyn std::error::Error>> {
    write_vec3(bytes, 0x28, min)?;
    write_vec3(bytes, 0x34, max)?;
    Ok(())
}

fn write_bbox(
    bytes: &mut [u8],
    offset: usize,
    min: Vec3,
    max: Vec3,
) -> Result<(), Box<dyn std::error::Error>> {
    write_vec3(bytes, offset, min)?;
    write_vec3(bytes, offset + 0x0C, max)?;
    Ok(())
}

fn bbox_for_vertices(vertices: &[SourceVertex]) -> (Vec3, Vec3) {
    let mut min = Vec3 {
        x: f32::INFINITY,
        y: f32::INFINITY,
        z: f32::INFINITY,
    };
    let mut max = Vec3 {
        x: f32::NEG_INFINITY,
        y: f32::NEG_INFINITY,
        z: f32::NEG_INFINITY,
    };
    for vertex in vertices {
        min.x = min.x.min(vertex.position.x);
        min.y = min.y.min(vertex.position.y);
        min.z = min.z.min(vertex.position.z);
        max.x = max.x.max(vertex.position.x);
        max.y = max.y.max(vertex.position.y);
        max.z = max.z.max(vertex.position.z);
    }
    (min, max)
}

fn write_summary(
    config: &Config,
    source: &SourceMesh,
    report: &PatchReport,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = config.summary_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(&config.summary_path)?;
    writeln!(file, "Route A donor FLVER patch summary")?;
    writeln!(file, "obj={}", config.obj_path.display())?;
    writeln!(file, "weights={}", config.weights_path.display())?;
    writeln!(file, "donor_flver={}", config.donor_flver.display())?;
    writeln!(file, "output_flver={}", config.output_flver.display())?;
    writeln!(file, "donor_mesh_index={}", config.donor_mesh_index)?;
    if let Some(region_map_tsv) = &config.region_map_tsv {
        writeln!(file, "region_map_tsv={}", region_map_tsv.display())?;
    }
    writeln!(
        file,
        "arm_compensation_enabled={}",
        source.arm_compensation.enabled
    )?;
    writeln!(
        file,
        "arm_volume_profile_enabled={}",
        source.arm_volume_profile.enabled
    )?;
    if source.arm_volume_profile.enabled {
        writeln!(
            file,
            "arm_volume_profile_affected_vertices={}",
            source.arm_volume_profile.affected_vertices
        )?;
        writeln!(
            file,
            "arm_volume_profile_side_surface_vertices={}",
            source.arm_volume_profile.side_surface_vertices
        )?;
        writeln!(
            file,
            "arm_volume_profile_hand_fit_vertices={}",
            source.arm_volume_profile.hand_fit_vertices
        )?;
        writeln!(
            file,
            "arm_volume_profile_max_lateral_delta={:.6}",
            source.arm_volume_profile.max_lateral_delta
        )?;
        writeln!(
            file,
            "arm_volume_profile_max_hand_translation={:.6}",
            source.arm_volume_profile.max_hand_translation
        )?;
        writeln!(
            file,
            "arm_volume_profile_elbow_radius_before_after={:.6},{:.6}",
            source.arm_volume_profile.elbow_radius_before,
            source.arm_volume_profile.elbow_radius_after
        )?;
        writeln!(
            file,
            "arm_volume_profile_bicep_radius_before_after={:.6},{:.6}",
            source.arm_volume_profile.bicep_radius_before,
            source.arm_volume_profile.bicep_radius_after
        )?;
        writeln!(
            file,
            "arm_volume_profile_shoulder_radius_before_after={:.6},{:.6}",
            source.arm_volume_profile.shoulder_radius_before,
            source.arm_volume_profile.shoulder_radius_after
        )?;
        writeln!(
            file,
            "arm_volume_profile_left_hand_z_before_after={:.6},{:.6}",
            source.arm_volume_profile.left_hand_center_z_before,
            source.arm_volume_profile.left_hand_center_z_after
        )?;
        writeln!(
            file,
            "arm_volume_profile_right_hand_z_before_after={:.6},{:.6}",
            source.arm_volume_profile.right_hand_center_z_before,
            source.arm_volume_profile.right_hand_center_z_after
        )?;
        writeln!(
            file,
            "arm_volume_profile_response={}",
            source.arm_volume_profile.response
        )?;
    }
    writeln!(
        file,
        "arm_island_prune_enabled={}",
        source.arm_island_prune.enabled
    )?;
    if source.arm_island_prune.enabled {
        writeln!(
            file,
            "arm_island_components_before={}",
            source.arm_island_prune.components_before
        )?;
        writeln!(
            file,
            "arm_broken_visible_components_before_after={},{}",
            source.arm_island_prune.broken_components_before,
            source.arm_island_prune.broken_components_after
        )?;
        writeln!(
            file,
            "arm_island_pruned_components={}",
            source.arm_island_prune.pruned_components
        )?;
        writeln!(
            file,
            "arm_island_pruned_vertices={}",
            source.arm_island_prune.pruned_vertices
        )?;
        writeln!(
            file,
            "arm_island_pruned_triangles={}",
            source.arm_island_prune.pruned_triangles
        )?;
        writeln!(
            file,
            "arm_triangles_before_after={},{}",
            source.arm_island_prune.triangles_before, source.arm_island_prune.triangles_after
        )?;
        writeln!(
            file,
            "arm_island_prune_response={}",
            source.arm_island_prune.response
        )?;
    }
    if source.arm_compensation.enabled {
        writeln!(
            file,
            "arm_compensated_vertices={}",
            source.arm_compensation.compensated_vertices
        )?;
        writeln!(
            file,
            "arm_components_left_right={},{}",
            source.arm_compensation.left_components, source.arm_compensation.right_components
        )?;
        writeln!(
            file,
            "arm_vertices_left_right={},{}",
            source.arm_compensation.left_vertices, source.arm_compensation.right_vertices
        )?;
        writeln!(
            file,
            "arm_avg_upper_before_after={:.6},{:.6}",
            source.arm_compensation.avg_upper_before, source.arm_compensation.avg_upper_after
        )?;
        writeln!(
            file,
            "arm_avg_forearm_hand_before_after={:.6},{:.6}",
            source.arm_compensation.avg_forearm_hand_before,
            source.arm_compensation.avg_forearm_hand_after
        )?;
        writeln!(
            file,
            "arm_avg_body_tween_after={:.6}",
            source.arm_compensation.avg_body_tween_after
        )?;
        writeln!(
            file,
            "arm_distal_overweighted_components_before_after={},{}",
            source
                .arm_compensation
                .distal_overweighted_components_before,
            source.arm_compensation.distal_overweighted_components_after
        )?;
        writeln!(
            file,
            "arm_weak_shoulder_components_before_after={},{}",
            source.arm_compensation.weak_shoulder_components_before,
            source.arm_compensation.weak_shoulder_components_after
        )?;
        writeln!(
            file,
            "arm_detached_components={}",
            source.arm_compensation.detached_components
        )?;
        writeln!(
            file,
            "arm_detached_vertices={}",
            source.arm_compensation.detached_vertices
        )?;
        writeln!(
            file,
            "arm_detached_proxy_vertices={}",
            source.arm_compensation.detached_proxy_vertices
        )?;
        writeln!(
            file,
            "arm_independent_detached_components={}",
            source.arm_compensation.independent_detached_components
        )?;
        writeln!(
            file,
            "arm_max_detached_body_distance={:.6}",
            source.arm_compensation.max_detached_body_distance
        )?;
        writeln!(
            file,
            "arm_detached_island_response={}",
            source.arm_compensation.detached_island_response
        )?;
    }
    writeln!(
        file,
        "region_response_enabled={}",
        source.region_response.enabled
    )?;
    if source.region_response.enabled {
        writeln!(
            file,
            "region_response_map={}",
            source.region_response.region_map_path
        )?;
        writeln!(
            file,
            "feet_region_response={}",
            source.region_response.response
        )?;
        writeln!(
            file,
            "feet_authored_vertices={}",
            source.region_response.feet_authored_vertices
        )?;
        writeln!(
            file,
            "feet_expanded_vertices={}",
            source.region_response.feet_expanded_vertices
        )?;
        writeln!(
            file,
            "feet_shrunk_vertices={}",
            source.region_response.feet_shrunk_vertices
        )?;
        writeln!(
            file,
            "feet_normalized_vertices={}",
            source.region_response.feet_normalized_vertices
        )?;
        writeln!(
            file,
            "feet_blend_band_vertices={}",
            source.region_response.feet_blend_band_vertices
        )?;
        writeln!(
            file,
            "feet_mixed_triangles_before_after={},{}",
            source.region_response.feet_mixed_triangles_before,
            source.region_response.feet_mixed_triangles_after
        )?;
        writeln!(
            file,
            "feet_boundary_edges_before_after={},{}",
            source.region_response.feet_boundary_edges_before,
            source.region_response.feet_boundary_edges_after
        )?;
        writeln!(
            file,
            "feet_near_shell_pairs={}",
            source.region_response.feet_near_shell_pairs
        )?;
        writeln!(
            file,
            "feet_near_shell_outside_vertices={}",
            source.region_response.feet_near_shell_outside_vertices
        )?;
        writeln!(
            file,
            "feet_weight_mismatch_pairs={}",
            source.region_response.feet_weight_mismatch_pairs
        )?;
        writeln!(
            file,
            "feet_weight_sync_vertices={}",
            source.region_response.feet_weight_sync_vertices
        )?;
        writeln!(
            file,
            "feet_max_weight_l1_mismatch={:.6}",
            source.region_response.feet_max_weight_l1_mismatch
        )?;
    }
    writeln!(
        file,
        "spine_core_compensation_enabled={}",
        source.weight_compensation.enabled
    )?;
    writeln!(
        file,
        "spine_core_compensation_axis_center={:.9},{:.9}",
        source.weight_compensation.axis_center_x, source.weight_compensation.axis_center_z
    )?;
    writeln!(
        file,
        "spine_core_compensated_vertices={}",
        source.weight_compensation.compensated_vertices
    )?;
    writeln!(
        file,
        "spine_core_vertices={}",
        source.weight_compensation.central_core_vertices
    )?;
    writeln!(
        file,
        "spine_core_avg_spine_weight_before_after={:.6},{:.6}",
        source.weight_compensation.central_core_avg_spine_before,
        source.weight_compensation.central_core_avg_spine_after
    )?;
    writeln!(
        file,
        "spine_core_avg_limb_weight_before_after={:.6},{:.6}",
        source.weight_compensation.central_core_avg_limb_before,
        source.weight_compensation.central_core_avg_limb_after
    )?;
    writeln!(
        file,
        "hard_spine_limb_edges_before_after={},{}",
        source.weight_compensation.hard_spine_limb_edges_before,
        source.weight_compensation.hard_spine_limb_edges_after
    )?;
    writeln!(file, "vertices={}", source.vertices.len())?;
    writeln!(file, "triangles={}", source.triangles.len())?;
    writeln!(
        file,
        "bbox_min={:.9},{:.9},{:.9}",
        source.bbox_min.x, source.bbox_min.y, source.bbox_min.z
    )?;
    writeln!(
        file,
        "bbox_max={:.9},{:.9},{:.9}",
        source.bbox_max.x, source.bbox_max.y, source.bbox_max.z
    )?;
    writeln!(file, "donor_vertex_capacity={}", report.vertex_capacity)?;
    writeln!(file, "lod0_index_capacity={}", report.lod0_index_capacity)?;
    writeln!(file, "patched_face_sets={}", report.patched_face_sets)?;
    writeln!(file, "hidden_face_sets={}", report.hidden_face_sets)?;
    writeln!(file, "texture_status=FLVER patch only; run route_a_mushroom_stage_textures before final partsbnd packing")?;
    writeln!(
        file,
        "runtime_status=not launched; this is offline FLVER mutation only"
    )?;
    Ok(())
}

fn write_vec3(
    bytes: &mut [u8],
    offset: usize,
    value: Vec3,
) -> Result<(), Box<dyn std::error::Error>> {
    write_f32(bytes, offset, value.x)?;
    write_f32(bytes, offset + 4, value.y)?;
    write_f32(bytes, offset + 8, value.z)?;
    Ok(())
}

fn write_uv_i16(
    bytes: &mut [u8],
    offset: usize,
    value: Vec2,
    factor: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    write_i16(bytes, offset, (value.x * factor).round() as i16)?;
    write_i16(bytes, offset + 2, (value.y * factor).round() as i16)?;
    Ok(())
}

fn write_snorm8x4(
    bytes: &mut [u8],
    offset: usize,
    values: [f32; 4],
) -> Result<(), Box<dyn std::error::Error>> {
    bounds(bytes, offset, 4)?;
    for (i, value) in values.iter().copied().enumerate() {
        bytes[offset + i] = (value.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8;
    }
    Ok(())
}

fn write_unorm8x4(
    bytes: &mut [u8],
    offset: usize,
    values: [f32; 4],
) -> Result<(), Box<dyn std::error::Error>> {
    bounds(bytes, offset, 4)?;
    let mut bytes4 = [0_u8; 4];
    let mut total = 0_u16;
    for i in 0..4 {
        bytes4[i] = (values[i].clamp(0.0, 1.0) * 255.0).round() as u8;
        total += u16::from(bytes4[i]);
    }
    if total == 0 {
        bytes4[0] = 255;
    } else if total != 255 {
        let delta = 255_i16 - total as i16;
        let first = i16::from(bytes4[0]) + delta;
        bytes4[0] = first.clamp(0, 255) as u8;
    }
    bytes[offset..offset + 4].copy_from_slice(&bytes4);
    Ok(())
}

fn write_snorm16x4(
    bytes: &mut [u8],
    offset: usize,
    values: [f32; 4],
) -> Result<(), Box<dyn std::error::Error>> {
    for (i, value) in values.iter().copied().enumerate() {
        write_i16(
            bytes,
            offset + i * 2,
            (value.clamp(0.0, 1.0) * 32767.0).round() as i16,
        )?;
    }
    Ok(())
}

fn write_u8x4(
    bytes: &mut [u8],
    offset: usize,
    values: [u8; 4],
) -> Result<(), Box<dyn std::error::Error>> {
    bounds(bytes, offset, 4)?;
    bytes[offset..offset + 4].copy_from_slice(&values);
    Ok(())
}

fn write_u16x4(
    bytes: &mut [u8],
    offset: usize,
    values: [u8; 4],
) -> Result<(), Box<dyn std::error::Error>> {
    for (i, value) in values.iter().copied().enumerate() {
        write_u16(bytes, offset + i * 2, u16::from(value))?;
    }
    Ok(())
}

fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, Box<dyn std::error::Error>> {
    Ok(*bytes.get(offset).ok_or("unexpected end of file")?)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Box<dyn std::error::Error>> {
    bounds(bytes, offset, 4)?;
    Ok(u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]))
}

fn write_u16(
    bytes: &mut [u8],
    offset: usize,
    value: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    bounds(bytes, offset, 2)?;
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_i16(
    bytes: &mut [u8],
    offset: usize,
    value: i16,
) -> Result<(), Box<dyn std::error::Error>> {
    bounds(bytes, offset, 2)?;
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_u32(
    bytes: &mut [u8],
    offset: usize,
    value: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    bounds(bytes, offset, 4)?;
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_f32(
    bytes: &mut [u8],
    offset: usize,
    value: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    bounds(bytes, offset, 4)?;
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn bounds(bytes: &[u8], offset: usize, len: usize) -> Result<(), Box<dyn std::error::Error>> {
    if offset
        .checked_add(len)
        .is_some_and(|end| end <= bytes.len())
    {
        Ok(())
    } else {
        Err(format!("range out of bounds: offset=0x{offset:X}, len=0x{len:X}").into())
    }
}
