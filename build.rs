fn main() {
    // 仅在 Windows 目标嵌入 UAC 清单（requestedExecutionLevel=requireAdministrator）
    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_manifest_file("nmts.manifest");
        // 应用图标（如存在则嵌入）
        if std::path::Path::new("src/nmts/resources/icons/app.ico").exists() {
            res.set_icon("src/nmts/resources/icons/app.ico");
        }
        res.set("FileDescription", "NMTs - Network Maintenance Tool set");
        res.set("ProductName", "NMTs");
        res.set("LegalCopyright", "GPL-3.0-or-later");
        if let Err(e) = res.compile() {
            println!("cargo:warning=winres 编译资源失败: {e}");
        }
    }
}
