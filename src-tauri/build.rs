fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu")
    {
        tauri_build::try_build(
            tauri_build::Attributes::new()
                .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest()),
        )
        .expect("failed to build Tauri resources");

        // Library test executables also need Common Controls v6 before Windows loads imports.
        embed_resource::compile_for_everything("windows/gnu.rc", embed_resource::NONE)
            .manifest_required()
            .expect("failed to compile the Windows manifest");
        let script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows/gnu-manifest.ld");
        println!("cargo:rustc-link-arg=-Wl,-T,{}", script.display());
        println!("cargo:rerun-if-changed=windows");
    } else {
        tauri_build::build();
    }
}
