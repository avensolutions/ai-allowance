fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        // Ordinal 1 so the tray can load it with Icon::from_resource(1, ..).
        res.set_icon_with_id("assets/aiallowance.ico", "1");
        res.set("ProductName", "AIAllowance");
        res.set("FileDescription", "AIAllowance system tray");
        res.compile().expect("failed to embed Windows resources");
        println!("cargo:rerun-if-changed=assets/aiallowance.ico");
    }
}
