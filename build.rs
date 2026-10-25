//! Gives the Windows build its icon and version details.

fn main() {
    println!("cargo:rerun-if-changed=packaging/icons/fmx.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("packaging/icons/fmx.ico")
            .set("ProductName", "FMX")
            .set("FileDescription", "Free Media Xpress (FMX) media converter")
            .compile()
            .expect("embed the Windows icon");
    }
}
