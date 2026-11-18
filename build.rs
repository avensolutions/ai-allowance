fn main() {
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
        res.compile().expect("failed to embed Windows resources");
        println!("cargo:rerun-if-changed=assets/aiallowance.ico");
        println!("cargo:rerun-if-changed=assets/providers");
    }
}
