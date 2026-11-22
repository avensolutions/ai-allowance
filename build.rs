use std::time::{SystemTime, UNIX_EPOCH};

/// Shown in the About dialog and written to the executable's CompanyName.
const PUBLISHER: &str = "Aven Solutions AI";

fn main() {
    // Build time for the About dialog, as Unix seconds. SOURCE_DATE_EPOCH
    // gives a fixed value for reproducible builds.
    let built = match std::env::var("SOURCE_DATE_EPOCH") {
        Ok(value) => value
            .parse::<u64>()
            .expect("SOURCE_DATE_EPOCH must be whole seconds since 1970"),
        Err(_) => SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before 1970")
            .as_secs(),
    };
    println!("cargo:rustc-env=AIALLOWANCE_BUILD_TIME={built}");
    println!("cargo:rustc-env=AIALLOWANCE_PUBLISHER={PUBLISHER}");
    // Rerun whenever the executable would be relinked, so the build time
    // stays the executable's own.
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        // Ordinal 1 so the tray can load it with Icon::from_resource(1, ..).
        res.set_icon_with_id("assets/aiallowance.ico", "1");
        res.set_icon_with_id("assets/providers/claude.ico", "2");
        res.set_icon_with_id("assets/providers/chatgpt.ico", "3");
        res.set_icon_with_id("assets/providers/copilot.ico", "4");
        res.set_manifest(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency><dependentAssembly>
    <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
  </dependentAssembly></dependency>
</assembly>"#);
        res.set("ProductName", "AI Allowance");
        res.set("FileDescription", "AI Allowance system tray");
        res.set("OriginalFilename", "ai-allowance.exe");
        res.set("CompanyName", PUBLISHER);
        res.compile().expect("failed to embed Windows resources");
        println!("cargo:rerun-if-changed=assets/aiallowance.ico");
        println!("cargo:rerun-if-changed=assets/providers");
    }
}
