use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    // Cargo does not watch this file. Without this, a new icon.ico stays out of the exe.
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-env-changed=OPENCV_LINK_LIBS");
    println!("cargo:rerun-if-env-changed=OPENCV_LINK_PATHS");
    // Delay-load so a bench without the OpenCV DLL still starts. Metrology
    // checks for the DLL before it measures, and skips the pass when it is missing.
    if let Some((name, path)) = world_dll() {
        println!("cargo:rustc-link-arg=/DELAYLOAD:{name}");
        println!("cargo:rustc-link-lib=delayimp");
        println!("cargo:rustc-env=LS_VISION_OPENCV_DLL={name}");
        copy_beside_target(&path);
        // The installer reads opencv/*.dll. Dev builds also copy beside the exe.
        copy_for_installer(&path);
    }
    tauri_build::build();
}

fn world_dll() -> Option<(String, PathBuf)> {
    let libs = env::var("OPENCV_LINK_LIBS").ok()?;
    let raw = libs
        .split(',')
        .map(str::trim)
        .find(|item| item.to_ascii_lowercase().contains("opencv_world"))?;
    let stem = raw
        .trim_start_matches("dylib=")
        .trim_start_matches("static=");
    let stem = Path::new(stem)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(stem);
    let file_name = format!("{stem}.dll");
    let mut bins = Vec::new();
    if let Ok(paths) = env::var("OPENCV_LINK_PATHS") {
        for path in paths.split(',') {
            let path = PathBuf::from(path.trim());
            if let Some(parent) = path.parent() {
                bins.push(parent.join("bin"));
            }
            bins.push(path);
        }
    }
    for bin in &bins {
        let candidate = bin.join(&file_name);
        if candidate.is_file() {
            return Some((file_name, candidate));
        }
    }
    Some((file_name, PathBuf::new()))
}

fn copy_beside_target(dll: &Path) {
    if !dll.is_file() {
        return;
    }
    let Ok(out) = env::var("OUT_DIR") else {
        return;
    };
    // OUT_DIR is target/<profile>/build/<crate>/out.
    let Some(profile) = Path::new(&out).ancestors().nth(3) else {
        return;
    };
    let Some(file_name) = dll.file_name() else {
        return;
    };
    for dir in [profile.to_path_buf(), profile.join("deps")] {
        copy_if_changed(dll, &dir.join(file_name));
    }
}

/// `tauri.conf.json` maps `opencv/*.dll` beside the installed exe.
fn copy_for_installer(dll: &Path) {
    if !dll.is_file() {
        return;
    }
    let Ok(manifest) = env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let Some(file_name) = dll.file_name() else {
        return;
    };
    copy_if_changed(dll, &PathBuf::from(manifest).join("opencv").join(file_name));
}

fn copy_if_changed(dll: &Path, dest: &Path) {
    if dest.is_file()
        && fs::metadata(dll).ok().map(|meta| meta.len()) == fs::metadata(dest).ok().map(|meta| meta.len())
    {
        return;
    }
    if let Some(dir) = dest.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Err(err) = fs::copy(dll, dest) {
        println!(
            "cargo:warning=could not copy OpenCV DLL to {}: {err}",
            dest.display()
        );
    }
}
