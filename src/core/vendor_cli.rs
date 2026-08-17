//! 厂商命令模板加载与实例化（模块三 / 模块四共用）。
//!
//! 命令模板集中维护于 `vendor_db/*.yaml`（内嵌二进制），描述「功能 → 厂商
//! 命令」，UI 按钮免记命令；模块四据此推导每台设备的 CLI 配置。
#![allow(dead_code)] // render_command / 字段供 v0.5 模块四 CLI 推导使用

use serde::Deserialize;

const HUAWEI_YAML: &str = include_str!("../../vendor_db/huawei_vrp.yaml");
const H3C_YAML: &str = include_str!("../../vendor_db/h3c_vrp.yaml");
const CISCO_YAML: &str = include_str!("../../vendor_db/cisco_ios.yaml");
const COMMON_YAML: &str = include_str!("../../vendor_db/common.yaml");

/// 命令参数定义。
#[derive(Debug, Clone, Deserialize)]
pub struct ArgDef {
    pub name: String,
    pub label: String,
    #[serde(rename = "type", default)]
    pub arg_type: String,
    #[serde(default)]
    pub placeholder: Option<String>,
}

/// 单条命令模板。
#[derive(Debug, Clone, Deserialize)]
pub struct CommandTemplate {
    pub id: String,
    pub category: String,
    pub label: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<ArgDef>,
    #[serde(default)]
    pub interactive: bool,
    #[serde(default)]
    pub confirm: Option<String>,
}

/// 命令分类。
#[derive(Debug, Clone, Deserialize)]
pub struct Category {
    pub id: String,
    pub label: String,
}

/// 厂商模板。
#[derive(Debug, Clone, Deserialize)]
pub struct VendorTemplate {
    pub vendor: String,
    pub label: String,
    #[serde(default)]
    pub prompt_features: Vec<String>,
    #[serde(default)]
    pub enter_config: String,
    #[serde(default)]
    pub exit_config: String,
    #[serde(default)]
    pub show_run: String,
    #[serde(default)]
    pub save_config: String,
    #[serde(default)]
    pub categories: Vec<Category>,
    #[serde(default)]
    pub commands: Vec<CommandTemplate>,
}

impl VendorTemplate {
    /// 按分类取命令。
    pub fn commands_by_category(&self, category: &str) -> Vec<&CommandTemplate> {
        self.commands
            .iter()
            .filter(|c| c.category == category)
            .collect()
    }

    /// 按 id 取命令。
    pub fn command(&self, id: &str) -> Option<&CommandTemplate> {
        self.commands.iter().find(|c| c.id == id)
    }
}

/// 厂商模板库。
#[derive(Debug, Clone, Default)]
pub struct VendorDb {
    vendors: Vec<VendorTemplate>,
}

impl VendorDb {
    /// 加载内嵌的厂商模板。
    pub fn load() -> Self {
        let mut vendors = Vec::new();
        for s in [HUAWEI_YAML, H3C_YAML, CISCO_YAML, COMMON_YAML] {
            if let Ok(v) = serde_yaml::from_str::<VendorTemplate>(s) {
                vendors.push(v);
            }
        }
        Self { vendors }
    }

    pub fn vendors(&self) -> &[VendorTemplate] {
        &self.vendors
    }

    pub fn get(&self, vendor: &str) -> Option<&VendorTemplate> {
        self.vendors.iter().find(|v| v.vendor == vendor)
    }
}

/// 渲染命令模板：把 `{name}` 占位符替换为参数值。
pub fn render_command(command: &str, args: &[(&str, String)]) -> String {
    let mut s = command.to_string();
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_vendors() {
        let db = VendorDb::load();
        assert_eq!(db.vendors().len(), 4);
        let h = db.get("huawei_vrp").unwrap();
        assert_eq!(h.label, "华为 (VRP)");
        assert!(h.command("save_config").is_some());
        // 通用排障命令（脚本中心）
        let c = db.get("common").unwrap();
        assert!(c.command("tracert").is_some());
    }

    #[test]
    fn render_placeholder() {
        let s = render_command(
            "ip address {ip} {mask}",
            &[
                ("ip", "192.168.1.1".into()),
                ("mask", "255.255.255.0".into()),
            ],
        );
        assert_eq!(s, "ip address 192.168.1.1 255.255.255.0");
    }

    #[test]
    fn cisco_differs() {
        let db = VendorDb::load();
        let c = db.get("cisco_ios").unwrap();
        assert_ne!(
            c.command("save_config").unwrap().command,
            db.get("huawei_vrp")
                .unwrap()
                .command("save_config")
                .unwrap()
                .command
        );
    }
}
