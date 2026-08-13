use std::{
    env,
    path::{Path, PathBuf},
};

// Single cc-compile of the vendored MinHook C source, replacing the three near-identical build
// scripts that previously lived in each game cdylib (er-effects-rs, er-reload-trace-dll,
// er-input-harness-dll). Windows-target gated -- the C uses Win32 APIs, so a host `cargo check`
// (which still runs build scripts) must not try to compile it.
fn main() {
    let root_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let target = env::var("TARGET").unwrap();
    if !target.contains("windows") {
        println!("cargo:rerun-if-changed=build.rs");
        return;
    }

    let arch = target.split('-').next().unwrap_or_default();

    let hde = match arch {
        "i686" => "hde/hde32.c",
        "x86_64" => "hde/hde64.c",
        _ => panic!("Architecture '{arch}' not supported by bundled MinHook"),
    };

    let mh_src_dir = resolve_minhook_src_dir(Path::new(&root_dir));

    cc::Build::new()
        .file(mh_src_dir.join("buffer.c"))
        .file(mh_src_dir.join("hook.c"))
        .file(mh_src_dir.join("trampoline.c"))
        .file(mh_src_dir.join(hde))
        .compile("minhook");

    println!("cargo:rerun-if-env-changed=ER_MINHOOK_SRC_DIR");
    println!("cargo:rerun-if-env-changed=ER_EFFECTS_MINHOOK_SRC_DIR");
    println!("cargo:rerun-if-changed={}", mh_src_dir.display());
}

/// The main working tree, asked of git rather than guessed from the directory layout.
///
/// Returns `None` for anything that is not a git checkout, or if git is unavailable -- the caller
/// then falls through to its existing panic, which names the env overrides.
fn git_common_dir_parent(manifest_dir: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(manifest_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let common = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
    // `<main tree>/.git` -> `<main tree>`. A bare repo has no working tree and yields nothing
    // useful, which the `buffer.c` check at the call site rejects anyway.
    common.parent().map(Path::to_path_buf)
}

fn resolve_minhook_src_dir(manifest_dir: &Path) -> PathBuf {
    for env_name in ["ER_MINHOOK_SRC_DIR", "ER_EFFECTS_MINHOOK_SRC_DIR"] {
        if let Ok(dir) = env::var(env_name) {
            let dir = PathBuf::from(dir);
            if dir.join("buffer.c").is_file() {
                return dir;
            }
            panic!("{env_name}={} does not point at MinHook src", dir.display());
        }
    }

    let repo_local = manifest_dir.join("../../vendor/minhook/src");
    if repo_local.join("buffer.c").is_file() {
        return repo_local;
    }

    for ancestor in manifest_dir.ancestors() {
        let candidate = ancestor.join("vendor/minhook/src");
        if candidate.join("buffer.c").is_file() {
            return candidate;
        }
        // `vendor/` is git-excluded and exists only in the MAIN checkout, so a build from a linked
        // git worktree finds nothing above itself. Follow the worktree's `.git` file back to the
        // main working tree and look there before giving up.
        if let Some(main_worktree) = main_worktree_root(ancestor) {
            let candidate = main_worktree.join("vendor/minhook/src");
            if candidate.join("buffer.c").is_file() {
                return candidate;
            }
        }
    }

    // A git WORKTREE has no `vendor/` of its own: the directory is gitignored, so it exists only in
    // the checkout that populated it, and a worktree's ancestors are wherever it happens to sit --
    // typically a sibling of the repo, where the loop above finds nothing. Every worktree therefore
    // failed this build with "set ER_MINHOOK_SRC_DIR", which is a per-agent, per-session workaround
    // for a path the repo can resolve by itself.
    //
    // `--git-common-dir` is the main checkout's `.git` even when invoked from a worktree, so its
    // parent is the main working tree and the vendor drop lives beneath it.
    if let Some(main_tree) = git_common_dir_parent(manifest_dir) {
        let candidate = main_tree.join("vendor/minhook/src");
        if candidate.join("buffer.c").is_file() {
            return candidate;
        }
    }

    panic!(
        "unable to find vendor/minhook/src (checked {} and ancestor vendor dirs; set ER_MINHOOK_SRC_DIR or ER_EFFECTS_MINHOOK_SRC_DIR to the MinHook src directory)",
        repo_local.display()
    );
}

/// Main working tree of a linked git worktree, from its `.git` FILE.
///
/// A linked worktree's `.git` is a file reading `gitdir: <main>/.git/worktrees/<name>`, so the main
/// working tree is the grandparent of that `worktrees/<name>` directory, minus the `.git` component.
/// Returns None for a normal checkout (`.git` is a directory) or any shape that does not match.
fn main_worktree_root(dir: &Path) -> Option<PathBuf> {
    let dot_git = dir.join(".git");
    if !dot_git.is_file() {
        return None;
    }
    let contents = std::fs::read_to_string(&dot_git).ok()?;
    let gitdir = Path::new(contents.strip_prefix("gitdir:")?.trim());
    // <main>/.git/worktrees/<name> -> <main>/.git -> <main>
    let worktrees = gitdir.parent()?;
    if worktrees.file_name()? != "worktrees" {
        return None;
    }
    worktrees.parent()?.parent().map(Path::to_path_buf)
}
