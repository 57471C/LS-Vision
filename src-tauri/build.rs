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
        let dest = dir.join(file_name);
        if dest.is_file()
            && fs::metadata(dll).ok().map(|meta| meta.len())
                == fs::metadata(&dest).ok().map(|meta| meta.len())
        {
            continue;
        }
        let _ = fs::create_dir_all(&dir);
        let _ = fs::copy(dll, dest);
    }
}
