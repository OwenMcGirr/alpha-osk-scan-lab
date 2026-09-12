fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    if std::env::var("TARGET")
        .unwrap_or_default()
        .contains("windows-msvc")
    {
        let path = std::path::Path::new("app.manifest").canonicalize().unwrap();
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", path.display());
    }
}
