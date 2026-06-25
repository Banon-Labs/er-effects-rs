//! Object view: render a real Elden Ring FLVER object's geometry in 3D.
//!
//! M2 — geometry only, with a neutral placeholder material (the real per-material
//! shaders come in M3). Loads via er-objectkit (FLVER -> [`ObjectModel`]), converts
//! to Bevy meshes, frames the model, and gives a simple orbit camera.

use bevy::asset::RenderAssetUsages;
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

// Bevy 0.19 renamed Events -> Messages (`EventReader` -> `MessageReader`).

use er_objectkit::{DecodedTexture, TexturedObject};

/// What object to show, resolved from CLI args before the app starts.
#[derive(Resource, Clone)]
pub enum ObjectSource {
    /// A direct path to an extracted `.flver` (geometry only).
    FlverPath(std::path::PathBuf),
    /// A character id (e.g. `c4800`) — fully textured, extracted on demand.
    Character(String),
}

#[derive(Resource)]
pub struct LoadedObject {
    pub object: TexturedObject,
    pub label: String,
}

/// Marks everything spawned for the object view, for clean teardown.
#[derive(Component)]
pub struct ObjectEntity;

#[derive(Component)]
pub struct OrbitCamera {
    pub focus: Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
}

/// Load the model (called once, before app build, so failures are reported on the
/// terminal rather than as an empty window).
pub fn load(source: &ObjectSource) -> Result<LoadedObject, String> {
    match source {
        ObjectSource::FlverPath(p) => {
            let bytes = std::fs::read(p).map_err(|e| format!("read {}: {e}", p.display()))?;
            let model = er_objectkit::flver::parse(&bytes).map_err(|e| e.to_string())?;
            let label = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("flver")
                .to_owned();
            Ok(LoadedObject {
                object: TexturedObject::from_model(model, label.clone()),
                label,
            })
        }
        ObjectSource::Character(id) => {
            let object = er_objectkit::load_textured_character(id)?;
            Ok(LoadedObject {
                label: object.label.clone(),
                object,
            })
        }
    }
}

/// Convert one decoded submesh to a Bevy mesh. Returns `None` for empty/edge-compressed
/// meshes (no positions).
pub fn to_bevy_mesh(m: &er_objectkit::ObjectMesh) -> Option<Mesh> {
    if m.positions.is_empty() || m.indices.is_empty() {
        return None;
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, m.positions.clone());
    if m.normals.len() == m.positions.len() {
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals.clone());
    }
    if m.uvs.len() == m.positions.len() {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, m.uvs.clone());
    }
    mesh.insert_indices(Indices::U32(m.indices.clone()));
    if m.normals.len() != m.positions.len() {
        mesh.compute_flat_normals();
    }
    Some(mesh)
}

/// A decoded RGBA texture -> a Bevy GPU image. `srgb` for color (albedo), linear for
/// data maps (normal, metallic/roughness).
fn to_image(t: &DecodedTexture, srgb: bool) -> Image {
    Image::new(
        Extent3d {
            width: t.width,
            height: t.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        t.rgba.clone(),
        if srgb {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        },
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    )
}

/// Spawn the object scene: meshes, lights, and a framed orbit camera.
pub fn setup_object(
    mut commands: Commands,
    loaded: Res<LoadedObject>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let object = &loaded.object;
    let (min, max) = object.bounding_box;
    let center = Vec3::from_array([
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ]);
    let size = (Vec3::from_array(max) - Vec3::from_array(min))
        .length()
        .max(0.5);

    let fallback = materials.add(StandardMaterial {
        base_color: Color::srgb(0.78, 0.78, 0.80),
        perceptual_roughness: 0.6,
        metallic: 0.05,
        cull_mode: None,
        ..default()
    });

    let mut spawned = 0;
    let mut textured = 0;
    for tm in &object.meshes {
        let Some(mesh) = to_bevy_mesh(&tm.mesh) else {
            continue;
        };
        // Per-mesh material: real albedo/normal textures when resolved, else fallback.
        let material = if let Some(albedo) = &tm.textures.albedo {
            textured += 1;
            materials.add(StandardMaterial {
                base_color_texture: Some(images.add(to_image(albedo, true))),
                normal_map_texture: tm
                    .textures
                    .normal
                    .as_ref()
                    .map(|n| images.add(to_image(n, false))),
                metallic_roughness_texture: tm
                    .textures
                    .metallic
                    .as_ref()
                    .map(|m| images.add(to_image(m, false))),
                perceptual_roughness: 0.7,
                metallic: 0.1,
                cull_mode: None,
                ..default()
            })
        } else {
            fallback.clone()
        };
        commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            ObjectEntity,
        ));
        spawned += 1;
    }
    let tris: usize = object.meshes.iter().map(|m| m.mesh.triangle_count()).sum();
    info!(
        "object '{}': {spawned}/{} meshes ({textured} textured), {tris} tris",
        loaded.label,
        object.meshes.len(),
    );

    // Key + fill lights.
    commands.spawn((
        DirectionalLight {
            illuminance: 8000.0,
            ..default()
        },
        Transform::from_xyz(center.x + size, center.y + size, center.z + size)
            .looking_at(center, Vec3::Y),
        ObjectEntity,
    ));

    let distance = size * 1.6;
    let cam = OrbitCamera {
        focus: center,
        distance,
        yaw: 0.5,
        pitch: 0.25,
    };
    let transform = orbit_transform(&cam);
    // AmbientLight is a per-camera component in Bevy 0.19.
    commands.spawn((
        Camera3d::default(),
        transform,
        AmbientLight {
            color: Color::srgb(0.6, 0.62, 0.7),
            brightness: 350.0,
            ..default()
        },
        cam,
        ObjectEntity,
    ));
}

pub fn teardown_object(mut commands: Commands, q: Query<Entity, With<ObjectEntity>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

fn orbit_transform(c: &OrbitCamera) -> Transform {
    let rot = Quat::from_rotation_y(c.yaw) * Quat::from_rotation_x(-c.pitch);
    let eye = c.focus + rot * Vec3::new(0.0, 0.0, c.distance);
    Transform::from_translation(eye).looking_at(c.focus, Vec3::Y)
}

/// Drag to orbit, wheel to zoom.
pub fn orbit_controls(
    mouse: Res<ButtonInput<MouseButton>>,
    mut motion: MessageReader<MouseMotion>,
    mut wheel: MessageReader<MouseWheel>,
    mut q: Query<(&mut OrbitCamera, &mut Transform)>,
) {
    let Ok((mut cam, mut transform)) = q.single_mut() else {
        return;
    };
    let mut changed = false;
    if mouse.pressed(MouseButton::Left) {
        for ev in motion.read() {
            cam.yaw -= ev.delta.x * 0.005;
            cam.pitch = (cam.pitch + ev.delta.y * 0.005).clamp(-1.5, 1.5);
            changed = true;
        }
    } else {
        motion.clear();
    }
    for ev in wheel.read() {
        cam.distance = (cam.distance * (1.0 - ev.y * 0.1)).max(0.2);
        changed = true;
    }
    if changed {
        *transform = orbit_transform(&cam);
    }
}
