//! er-shader-viewer: a native Bevy GUI that browses the **real extracted Elden
//! Ring shader library** and renders each shader through the er-shaderkit backend
//! (DXIL -> SPIR-V -> naga -> reflection-driven render). View only; no editing.
//!
//! Point it at a directory of extracted `.ppo` fragment members:
//!   cargo run -p er-shader-viewer -- <dir>
//! (defaults to target/er-shaderkit-fixtures/gxpost). Up/Down/PageUp/PageDown/
//! Home/End navigate; the selected shader is rendered live with synthetic inputs
//! and its status (rendered / failed + reason) is shown. Not the in-game look —
//! these shaders need the game's geometry/textures/cbuffers — but real shader
//! output over the real library. `--screenshot <path>` saves a frame and exits.

use bevy::app::AppExit;
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use er_shaderkit::render::Headless;

mod object;
use object::{LoadedObject, ObjectSource};

const RENDER_SIZE: u32 = 256;

/// The two views the app can show; Tab toggles between them when an object is loaded.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
enum View {
    #[default]
    Shader,
    Object,
}

/// Marks shader-view entities (2D camera, canvas, UI) for teardown on view switch.
#[derive(Component)]
struct ShaderEntity;

#[derive(Resource)]
struct Lib {
    names: Vec<String>,
    paths: Vec<std::path::PathBuf>,
}

#[derive(Resource)]
struct Sel {
    idx: usize,
    dirty: bool,
}

#[derive(Resource)]
struct Backend(Option<Headless>);

#[derive(Resource)]
struct ShotMode {
    path: String,
    frame: u32,
}

#[derive(Component)]
struct Canvas;
#[derive(Component)]
struct StatusText;
#[derive(Component)]
struct ListText;

fn main() {
    let mut dir = std::path::PathBuf::from(format!(
        "{}/../../target/er-shaderkit-fixtures/gxpost",
        env!("CARGO_MANIFEST_DIR")
    ));
    let mut shot = None;
    let mut start_idx = 0usize;
    let mut object_source: Option<ObjectSource> = None;
    let mut rdc: Option<String> = None;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--screenshot" => {
                shot = args.get(i + 1).cloned();
                i += 2;
            }
            "--shader" => {
                start_idx = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(0);
                i += 2;
            }
            "--flver" => {
                object_source = args.get(i + 1).map(|p| ObjectSource::FlverPath(p.into()));
                i += 2;
            }
            "--object" => {
                object_source = args.get(i + 1).map(|c| ObjectSource::Character(c.clone()));
                i += 2;
            }
            "--rdc-capture" => {
                rdc = args
                    .get(i + 1)
                    .cloned()
                    .or_else(|| Some("/tmp/er-cap".to_owned()));
                i += 2;
            }
            other => {
                dir = std::path::PathBuf::from(other);
                i += 1;
            }
        }
    }

    // Resolve the object up front so load failures print to the terminal (not an
    // empty window), and decide which view to open in.
    let loaded = object_source
        .as_ref()
        .and_then(|src| match object::load(src) {
            Ok(l) => {
                let tris: usize = l
                    .object
                    .meshes
                    .iter()
                    .map(|m| m.mesh.triangle_count())
                    .sum();
                eprintln!(
                    "loaded object '{}': {} meshes, {} textured, {} tris",
                    l.label,
                    l.object.meshes.len(),
                    l.object.textured_mesh_count(),
                    tris
                );
                Some(l)
            }
            Err(e) => {
                eprintln!("object load failed: {e}");
                None
            }
        });
    let initial = if loaded.is_some() {
        View::Object
    } else {
        View::Shader
    };

    let (names, paths) = scan(&dir);
    eprintln!("loaded {} shaders from {}", names.len(), dir.display());

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "ER Shader Viewer".to_owned(),
                    name: Some("er-shader-viewer".to_owned()),
                    resolution: (1280, 720).into(),
                    ..default()
                }),
                ..default()
            })
            .set(AssetPlugin {
                file_path: format!("{}/assets", env!("CARGO_MANIFEST_DIR")),
                ..default()
            }),
    )
    .insert_resource(Lib { names, paths })
    .insert_resource(Sel {
        idx: start_idx,
        dirty: true,
    })
    .insert_resource(Backend(Headless::new().ok()))
    .insert_state(initial)
    .add_systems(OnEnter(View::Shader), setup_shader)
    .add_systems(OnExit(View::Shader), teardown_shader)
    .add_systems(OnEnter(View::Object), object::setup_object)
    .add_systems(OnExit(View::Object), object::teardown_object)
    .add_systems(
        Update,
        (
            (navigate, render_selected).run_if(in_state(View::Shader)),
            object::orbit_controls.run_if(in_state(View::Object)),
            toggle_view,
            screenshot_then_exit,
            rdc_capture_then_exit,
        ),
    );
    if let Some(l) = loaded {
        app.insert_resource(l);
    }
    if let Some(path) = shot {
        app.insert_resource(ShotMode { path, frame: 0 });
    }
    if let Some(out) = rdc {
        app.insert_resource(RdcMode { out, frame: 0 });
    }
    app.run();
}

/// When `--rdc-capture <path>` is set, trigger an in-app RenderDoc capture of one
/// settled frame, then exit. No-op unless launched under `renderdoccmd`/RenderDoc
/// (where `librenderdoc` is loaded). The capture's `.rdc` lands at `<path>_frameN.rdc`.
#[derive(Resource)]
struct RdcMode {
    out: String,
    frame: u32,
}

fn rdc_capture_then_exit(mode: Option<ResMut<RdcMode>>, mut exit: MessageWriter<AppExit>) {
    let Some(mut mode) = mode else {
        return;
    };
    mode.frame += 1;
    // Let the scene settle + assets upload before capturing.
    if mode.frame == 90 {
        match renderdoc::RenderDoc::<renderdoc::V141>::new() {
            Ok(mut rd) => {
                rd.set_capture_file_path_template(&mode.out);
                rd.trigger_capture();
                info!("RenderDoc: triggered capture -> {}_frameN.rdc", mode.out);
            }
            Err(e) => warn!("RenderDoc not available (run under renderdoccmd): {e}"),
        }
    }
    // The capture is written on the next present; give it a few frames, then quit.
    if mode.frame > 130 {
        exit.write(AppExit::Success);
    }
}

/// Tab switches between the shader list and the loaded object (no-op if no object).
fn toggle_view(
    keys: Res<ButtonInput<KeyCode>>,
    loaded: Option<Res<LoadedObject>>,
    state: Res<State<View>>,
    mut next: ResMut<NextState<View>>,
) {
    if loaded.is_none() || !keys.just_pressed(KeyCode::Tab) {
        return;
    }
    next.set(match state.get() {
        View::Shader => View::Object,
        View::Object => View::Shader,
    });
}

fn teardown_shader(mut commands: Commands, q: Query<Entity, With<ShaderEntity>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

fn scan(dir: &std::path::Path) -> (Vec<String>, Vec<std::path::PathBuf>) {
    let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("ppo"))
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    let names = entries
        .iter()
        .map(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_owned()
        })
        .collect();
    (names, entries)
}

fn setup_shader(mut commands: Commands, mut sel: ResMut<Sel>) {
    // Re-render the selection when (re)entering the shader view.
    sel.dirty = true;

    commands.spawn((Camera2d, ShaderEntity));

    // The rendered shader output, shown as a sprite on the right.
    commands.spawn((
        Sprite {
            custom_size: Some(Vec2::splat(560.0)),
            ..default()
        },
        Transform::from_xyz(180.0, 0.0, 0.0),
        Canvas,
        ShaderEntity,
    ));

    // Left UI panel: title, scrolling list window, status line.
    commands
        .spawn((
            Node {
                width: Val::Px(420.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(8.0),
                padding: UiRect::all(Val::Px(10.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.04, 0.05, 0.9)),
            ShaderEntity,
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("ER shader library"),
                TextColor(Color::srgb(1.0, 0.84, 0.4)),
            ));
            p.spawn((
                Text::new(""),
                TextColor(Color::srgb(0.8, 0.85, 0.9)),
                ListText,
            ));
            p.spawn((
                Text::new(""),
                TextColor(Color::srgb(0.7, 1.0, 0.7)),
                StatusText,
            ));
            p.spawn((
                Text::new("\u{2191}/\u{2193} step  PgUp/PgDn \u{00b1}10  Home/End"),
                TextColor(Color::srgb(0.5, 0.5, 0.55)),
            ));
        });
}

fn navigate(keys: Res<ButtonInput<KeyCode>>, lib: Res<Lib>, mut sel: ResMut<Sel>) {
    if lib.names.is_empty() {
        return;
    }
    let n = lib.names.len();
    let mut idx = sel.idx as i64;
    let mut moved = false;
    if keys.just_pressed(KeyCode::ArrowDown) {
        idx += 1;
        moved = true;
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        idx -= 1;
        moved = true;
    }
    if keys.just_pressed(KeyCode::PageDown) {
        idx += 10;
        moved = true;
    }
    if keys.just_pressed(KeyCode::PageUp) {
        idx -= 10;
        moved = true;
    }
    if keys.just_pressed(KeyCode::Home) {
        idx = 0;
        moved = true;
    }
    if keys.just_pressed(KeyCode::End) {
        idx = n as i64 - 1;
        moved = true;
    }
    if moved {
        sel.idx = idx.rem_euclid(n as i64) as usize;
        sel.dirty = true;
    }
}

fn render_selected(
    mut sel: ResMut<Sel>,
    lib: Res<Lib>,
    backend: Res<Backend>,
    mut images: ResMut<Assets<Image>>,
    mut canvas: Query<&mut Sprite, With<Canvas>>,
    mut status: Query<&mut Text, (With<StatusText>, Without<ListText>)>,
    mut list: Query<&mut Text, (With<ListText>, Without<StatusText>)>,
) {
    if !sel.dirty || lib.names.is_empty() {
        return;
    }
    sel.dirty = false;
    let idx = sel.idx;

    // List window around the selection.
    if let Ok(mut t) = list.single_mut() {
        let lo = idx.saturating_sub(8);
        let hi = (lo + 18).min(lib.names.len());
        let mut s = String::new();
        for i in lo..hi {
            let marker = if i == idx { "> " } else { "  " };
            s.push_str(&format!("{marker}{}\n", lib.names[i]));
        }
        **t = s;
    }

    let (status_text, pixels) = match &backend.0 {
        None => ("no GPU adapter available".to_owned(), None),
        Some(h) => match std::fs::read(&lib.paths[idx])
            .map_err(|e| e.to_string())
            .and_then(|bytes| er_shaderkit::dxil_to_spirv(&bytes, None).map_err(|e| e.to_string()))
            .and_then(|spv| {
                h.render_fragment_spirv(&spv, RENDER_SIZE)
                    .map_err(|e| e.to_string())
            }) {
            Ok(px) => (
                format!(
                    "[{}/{}] {}  \u{2713} rendered",
                    idx + 1,
                    lib.names.len(),
                    lib.names[idx]
                ),
                Some(px),
            ),
            Err(e) => (
                format!(
                    "[{}/{}] {}  \u{2717} {}",
                    idx + 1,
                    lib.names.len(),
                    lib.names[idx],
                    short(&e)
                ),
                None,
            ),
        },
    };

    if let Ok(mut t) = status.single_mut() {
        **t = status_text;
    }
    if let Ok(mut sprite) = canvas.single_mut() {
        let data = match pixels {
            Some(px) => px.into_iter().flatten().collect::<Vec<u8>>(),
            // failed/no-gpu: a dark placeholder so the canvas reads as "no image".
            None => vec![20u8; (RENDER_SIZE * RENDER_SIZE * 4) as usize],
        };
        let image = Image::new(
            Extent3d {
                width: RENDER_SIZE,
                height: RENDER_SIZE,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        );
        sprite.image = images.add(image);
    }
}

fn short(e: &str) -> String {
    let line = e.lines().next().unwrap_or(e);
    line.chars().take(60).collect()
}

fn screenshot_then_exit(
    mode: Option<ResMut<ShotMode>>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(mut mode) = mode else {
        return;
    };
    mode.frame += 1;
    if mode.frame == 60 {
        let path = mode.path.clone();
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    if mode.frame > 240 {
        exit.write(AppExit::Success);
    }
}
