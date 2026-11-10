//! Builds the Tauri context. App commands get an ACL manifest, so each window may call only
//! the commands its capability in `capabilities/` allows.

include!("src/ipc_commands.rs");

fn main() {
    println!("cargo:rerun-if-changed=src/ipc_commands.rs");
    let manifest = tauri_build::AppManifest::new().commands(COMMANDS);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("tauri-build failed");
}
