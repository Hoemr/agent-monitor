fn main() {
    // Include the island and shared assets when rebuilding the embedded UI.
    println!("cargo:rerun-if-changed=../src");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-changed=icons/icon.png");
    tauri_build::build()
}
