// libgsm has no pkg-config file. Homebrew on macOS keeps it outside the default
// linker search path, so point the linker at it.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        for dir in ["/opt/homebrew/lib", "/usr/local/lib"] {
            if std::path::Path::new(dir).join("libgsm.dylib").exists() {
                println!("cargo:rustc-link-search=native={dir}");
            }
        }
    }
}
