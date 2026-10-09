fn main() -> std::io::Result<()> {
    println!("cargo:rerun-if-changed=assets/benchmon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/benchmon.ico")
            .set("ProductName", "benchmon")
            .set("FileDescription", "benchmon system monitor")
            .set("OriginalFilename", "benchmon.exe")
            .compile()?;
    }
    Ok(())
}
