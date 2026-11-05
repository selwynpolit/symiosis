use tauri_build::{Attributes, WindowsAttributes};

fn main() {
    // Test executables get no application manifest, so on Windows they load the
    // old comctl32 and die with STATUS_ENTRYPOINT_NOT_FOUND before any test runs.
    // Embedding the manifest through link args covers every target, so
    // tauri-build's own copy is turned off to avoid a duplicate.
    let mut attributes = Attributes::new();
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
        attributes =
            attributes.windows_attributes(WindowsAttributes::new_without_app_manifest());
    }
    if let Err(error) = tauri_build::try_build(attributes) {
        println!("{error:#}");
        std::process::exit(1);
    }
}
