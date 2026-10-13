fn main() -> std::io::Result<()> {
    println!("cargo:rerun-if-changed=assets/benchmon.ico");
    // Only the panel carries the icon and version resource; the library alone needs none.
    let gui = std::env::var_os("CARGO_FEATURE_GUI").is_some();
    if gui && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/benchmon.ico")
            .set("ProductName", "benchmon")
            .set("FileDescription", "benchmon system monitor")
            .set("OriginalFilename", "benchmon.exe")
            .compile()?;
    }
    Ok(())
}
