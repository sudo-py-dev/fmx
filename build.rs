//! Gives the Windows build its icon and version details.

fn main() {
    println!("cargo:rerun-if-changed=packaging/icons/muxr.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("packaging/icons/muxr.ico")
            .set("ProductName", "Muxr")
            .set("FileDescription", "Muxr media converter")
            .compile()
            .expect("embed the Windows icon");
    }
}
