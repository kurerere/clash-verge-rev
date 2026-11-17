fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Layered child windows require an OS compatibility declaration in the executable manifest.
    let windows_manifest = include_str!("windows.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        // Tauri's resource is linked to the app binary, but not to Rust unit-test harnesses or examples.
        // Give non-app targets their own manifest so tests and examples can resolve TaskDialogIndirect.
        let out_dir = std::env::var_os("OUT_DIR").ok_or_else(|| std::io::Error::other("OUT_DIR is not set"))?;
        let manifest_path = std::path::PathBuf::from(out_dir).join("windows-test-manifest.xml");
        std::fs::write(&manifest_path, windows_manifest)?;
        let manifest_display = manifest_path.display();
        if std::env::var_os("CARGO_FEATURE_CLIPPY").is_some() {
            println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
            println!("cargo:rustc-link-arg=/MANIFESTINPUT:{manifest_display}");
        } else {
            println!("cargo:rustc-link-arg-examples=/MANIFEST:EMBED");
            println!("cargo:rustc-link-arg-examples=/MANIFESTINPUT:{manifest_display}");
        }
    }

    #[cfg(feature = "clippy")]
    {
        println!("cargo:warning=Skipping tauri_build during Clippy");
    }

    #[cfg(not(feature = "clippy"))]
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new().app_manifest(windows_manifest)),
    )?;

    Ok(())
}
