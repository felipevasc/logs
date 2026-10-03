fn main() {
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if windows_msvc {
        // Tauri's resource embedding targets binaries only. Supply its unchanged
        // 2.7 manifest through the linker so the library test harness gets it too.
        // Keep icons/version resources, without embedding a second app manifest.
        let attributes = tauri_build::Attributes::new().windows_attributes(
            tauri_build::WindowsAttributes::new_without_app_manifest(),
        );
        tauri_build::try_build(attributes).expect("failed to run tauri-build");
        let manifest = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo package directory"),
        )
        .join("windows/tauri-default.manifest.xml");
        println!("cargo:rerun-if-changed=windows/tauri-default.manifest.xml");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        // The original Tauri manifest has no UAC or DPI overrides. Prevent the
        // linker from adding its default UAC section while embedding this XML.
        println!("cargo:rustc-link-arg=/MANIFESTUAC:NO");
    } else {
        tauri_build::build()
    }
}
