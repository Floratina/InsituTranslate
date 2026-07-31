fn main() {
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-changed=windows-common-controls.manifest");
    println!("cargo:rerun-if-changed=windows-common-controls.rc");
    tauri_build::build();

    let is_windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if is_windows {
        // Cargo has no link-arg selector for library unit-test binaries.
        // MANIFESTINPUT lets MSVC merge the dependency without the quoting
        // ambiguity of MANIFESTDEPENDENCY's space-containing value.
        let manifest_dir = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is not set"),
        );
        let manifest = manifest_dir.join("windows-common-controls.manifest");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        // The Tauri binary already receives manifest resource ID 1 from
        // tauri-build's resource.lib. Keep that resource authoritative and
        // suppress only the linker's second, duplicate manifest for the bin.
        println!("cargo:rustc-link-arg-bin=insitu-translate=/MANIFEST:NO");

        let resource = manifest_dir.join("windows-common-controls.rc");
        embed_resource::compile_for_tests(resource, embed_resource::NONE)
            .manifest_required()
            .expect("Unable to compile the Windows test manifest resource");
        // Integration tests receive resource ID 1 from compile_for_tests;
        // only lib unit tests need the global linker-generated manifest.
        println!("cargo:rustc-link-arg-tests=/MANIFEST:NO");
    }
}
