//! Build script: on Windows, embed the app icon and version metadata into the
//! `.exe` resource table. This only affects the Explorer/file icon and the
//! file's Properties → Details; the in-app window/taskbar icon is set at runtime
//! in `main.rs`. Best-effort: if no resource compiler is available we emit a
//! warning and build a working (icon-less) binary rather than failing.

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=skipping .exe icon embedding: {e}");
        }
    }
}
