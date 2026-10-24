fn main() {
    // Cargo does not watch this file. Without this, a new icon.ico stays out of the exe.
    println!("cargo:rerun-if-changed=icons/icon.ico");
    tauri_build::build()
}
