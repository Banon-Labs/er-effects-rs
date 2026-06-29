use bevy::{
    prelude::*,
    reflect::TypePath,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
    sprite_render::{Material2d, Material2dPlugin, MeshMaterial2d},
};

const SHADER_ASSET_PATH: &str = "shaders/elden_rune_lab.wgsl";

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "ER Bevy Shader Lab".to_owned(),
                        resolution: (1280, 720).into(),
                        ..default()
                    }),
                    ..default()
                })
                .set(AssetPlugin {
                    file_path: format!("{}/assets", env!("CARGO_MANIFEST_DIR")),
                    ..default()
                }),
            Material2dPlugin::<RuneMaterial>::default(),
        ))
        .add_systems(Startup, setup)
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<RuneMaterial>>,
) {
    commands.spawn(Camera2d);

    commands.spawn((
        Mesh2d(meshes.add(Rectangle::new(16.0, 9.0))),
        MeshMaterial2d(materials.add(RuneMaterial {
            glow: LinearRgba::new(1.0, 0.74, 0.28, 1.0),
            shadow: LinearRgba::new(0.055, 0.035, 0.018, 1.0),
        })),
        Transform::from_scale(Vec3::splat(70.0)),
    ));
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct RuneMaterial {
    #[uniform(0)]
    glow: LinearRgba,
    #[uniform(1)]
    shadow: LinearRgba,
}

impl Material2d for RuneMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER_ASSET_PATH.into()
    }
}
