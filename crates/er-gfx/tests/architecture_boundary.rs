use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("er-gfx must remain under <workspace>/crates")
        .to_path_buf()
}

fn manifest(package: &str) -> String {
    let path = workspace_root()
        .join("crates")
        .join(package)
        .join("Cargo.toml");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// Return dependency keys from ordinary and target-specific Cargo dependency tables.
///
/// The workspace uses only the two standard TOML forms this handles:
///
/// ```toml
/// [dependencies]
/// er-gfx = { path = "../er-gfx" }
///
/// [target.'cfg(windows)'.dependencies.er-hook]
/// path = "../er-hook"
/// ```
fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for raw in manifest.lines() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == "name" {
            return Some(value.trim().trim_matches(['\'', '"']).to_owned());
        }
    }
    None
}

fn dependency_names(manifest: &str) -> BTreeSet<String> {
    let mut section = String::new();
    let mut names = BTreeSet::new();
    for raw in manifest.lines() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_owned();
            let table_dependency = section.strip_prefix("dependencies.").or_else(|| {
                section
                    .split_once(".dependencies.")
                    .map(|(_, dependency)| dependency)
            });
            if let Some(dependency) = table_dependency {
                names.insert(dependency.trim_matches(['\'', '"']).to_owned());
            }
            continue;
        }
        if !(section == "dependencies" || section.ends_with(".dependencies")) {
            continue;
        }
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches(['\'', '"']);
        if !key.is_empty() {
            names.insert(key.to_owned());
        }
    }
    names
}

#[test]
fn manifest_parser_covers_workspace_dependency_forms() {
    let dependencies = dependency_names(
        r#"
        [dependencies]
        er-gfx = { path = "../er-gfx" }

        [dependencies.er-game-base]
        path = "../er-game-base"

        [target.'cfg(windows)'.dependencies.er-hook]
        path = "../er-hook"
        "#,
    );
    assert_eq!(
        dependencies,
        BTreeSet::from([
            "er-game-base".to_owned(),
            "er-gfx".to_owned(),
            "er-hook".to_owned(),
        ])
    );
}

#[test]
fn the_direct_codec_consumers_match_the_decision_evidence() {
    let mut consumers = BTreeSet::new();
    for family in ["crates", "tools"] {
        let parent = workspace_root().join(family);
        for entry in fs::read_dir(&parent)
            .unwrap_or_else(|error| panic!("read {}: {error}", parent.display()))
        {
            let entry = entry.expect("read workspace package entry");
            let path = entry.path().join("Cargo.toml");
            if !path.is_file() {
                continue;
            }
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            if dependency_names(&source).contains("er-gfx") {
                consumers.insert(
                    package_name(&source)
                        .unwrap_or_else(|| panic!("{} has no package name", path.display())),
                );
            }
        }
    }
    let mut expected = BTreeSet::from([
        "er-armament-icons".to_owned(),
        "er-effects-rs".to_owned(),
        "er-invasion-warp-dll".to_owned(),
        "er-loading-portrait".to_owned(),
    ]);
    if workspace_root()
        .join("crates/er-scaleform-hooks/Cargo.toml")
        .is_file()
    {
        expected.insert("er-scaleform-hooks".to_owned());
    }
    assert_eq!(
        consumers, expected,
        "update the D2 consumer/coupling analysis when the direct er-gfx boundary changes"
    );
}

#[test]
fn codec_stays_below_the_native_hook_layer() {
    let dependencies = dependency_names(&manifest("er-gfx"));
    // er-game-base's default tier is the zero-dependency owner for shared content primitives.
    let forbidden = [
        "eldenring",
        "er-effects-rs",
        "er-hook",
        "er-loading-portrait",
        "er-scaleform-hooks",
        "er-telemetry",
        "er-title-flow",
        "fromsoftware-shared",
        "windows",
    ];
    let found: Vec<_> = forbidden
        .into_iter()
        .filter(|dependency| dependencies.contains(*dependency))
        .collect();
    assert!(
        found.is_empty(),
        "er-gfx is the host-testable codec layer; native hooks belong in er-scaleform-hooks, not dependencies {found:?}"
    );
}

#[test]
fn the_counterfactual_deepening_edge_would_close_a_cycle() {
    let title_flow = dependency_names(&manifest("er-title-flow"));
    let portrait = dependency_names(&manifest("er-loading-portrait"));
    assert!(
        title_flow.contains("er-loading-portrait"),
        "D2 cycle evidence changed: expected er-title-flow -> er-loading-portrait"
    );
    assert!(
        portrait.contains("er-gfx"),
        "D2 cycle evidence changed: expected er-loading-portrait -> er-gfx"
    );
    assert!(
        !dependency_names(&manifest("er-gfx")).contains("er-title-flow"),
        "er-gfx -> er-title-flow closes er-gfx -> er-title-flow -> er-loading-portrait -> er-gfx"
    );
}

#[test]
fn the_selected_sibling_owner_points_toward_the_codec_when_present() {
    let scaleform_hooks = workspace_root()
        .join("crates")
        .join("er-scaleform-hooks")
        .join("Cargo.toml");
    if !scaleform_hooks.exists() {
        // D2 is decision-only. R23 creates the selected owner and activates this half of the gate.
        return;
    }
    let source = fs::read_to_string(&scaleform_hooks)
        .unwrap_or_else(|error| panic!("read {}: {error}", scaleform_hooks.display()));
    let dependencies = dependency_names(&source);
    let required = BTreeSet::from([
        "er-game-base".to_owned(),
        "er-gfx".to_owned(),
        "er-hook".to_owned(),
        "er-telemetry".to_owned(),
    ]);
    assert!(
        required.is_subset(&dependencies),
        "er-scaleform-hooks must own native plumbing over the selected shared layers; missing {:?}",
        required.difference(&dependencies).collect::<Vec<_>>()
    );

    let forbidden = [
        "er-effects-rs",
        "er-loading-portrait",
        "er-scaleform-hooks",
        "er-title-flow",
    ];
    let found: Vec<_> = forbidden
        .into_iter()
        .filter(|dependency| dependencies.contains(*dependency))
        .collect();
    assert!(
        found.is_empty(),
        "er-scaleform-hooks must use its narrow host callback instead of depending on product/feature owners {found:?}"
    );
    assert!(
        !dependency_names(&manifest("er-gfx")).contains("er-scaleform-hooks"),
        "the selected ownership edge is er-scaleform-hooks -> er-gfx, never the reverse"
    );
}
