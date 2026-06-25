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

const RENDER_SIZE: u32 = 256;

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
            other => {
                dir = std::path::PathBuf::from(other);
                i += 1;
            }
        }
    }

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
    .add_systems(Startup, setup)
    .add_systems(Update, (navigate, render_selected, screenshot_then_exit));
    if let Some(path) = shot {
        app.insert_resource(ShotMode { path, frame: 0 });
    }
    app.run();
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

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);

    // The rendered shader output, shown as a sprite on the right.
    commands.spawn((
        Sprite {
            custom_size: Some(Vec2::splat(560.0)),
            ..default()
        },
        Transform::from_xyz(180.0, 0.0, 0.0),
        Canvas,
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
