//! 多网卡枚举与「当前上网网卡」判定（跨模块基础能力，见需求文档十一）。
//!
//! 通过 `Get-NetAdapter` / `Get-NetRoute` 枚举网卡、判定当前真正用于上网的
//! 网卡（默认路由接口，多默认路由时取跃点数最低者），并过滤虚拟 / VPN 网卡。
#![allow(dead_code)] // v0.1 骨架：部分字段 / 接口供诊断与快捷设置模块使用

use serde::Deserialize;

use super::powershell;

/// 虚拟网卡识别关键词（描述 / 接口名包含其一即视为虚拟网卡）。
const VIRTUAL_KEYWORDS: &[&str] = &[
    "virtual",
    "vpn",
    "hyper-v",
    "vethernet",
    "vmware",
    "virtualbox",
    "wsl",
    "tailscale",
    "zerotier",
    "loopback",
    "tap",
    "tun",
    "docker",
    "容器",
    "虚拟",
    "隧道",
    "环回",
];

/// 网卡信息。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Adapter {
    #[serde(rename = "InterfaceIndex", default)]
    pub interface_index: u32,
    #[serde(rename = "Name", default)]
    pub name: String,
    #[serde(rename = "InterfaceDescription", default)]
    pub description: String,
    #[serde(rename = "InterfaceType", default)]
    pub interface_type: String,
    #[serde(rename = "Status", default)]
    pub status: String,
    #[serde(rename = "HardwareInterface", default)]
    pub hardware_interface: bool,
}

impl Adapter {
    /// 是否判定为虚拟网卡（非上网网卡）。
    pub fn is_virtual(&self) -> bool {
        if !self.hardware_interface {
            return true;
        }
        let hay = format!(
            "{} {}",
            self.name.to_lowercase(),
            self.description.to_lowercase()
        );
        VIRTUAL_KEYWORDS.iter().any(|k| hay.contains(k))
    }

    /// 是否 Up（已连接 / 有链路）。
    pub fn is_up(&self) -> bool {
        self.status.eq_ignore_ascii_case("up")
    }

    /// 网卡类型徽标（物理 / 虚拟 / VPN）。
    pub fn kind_label(&self) -> &'static str {
        let hay = format!(
            "{} {}",
            self.name.to_lowercase(),
            self.description.to_lowercase()
        );
        if hay.contains("vpn") || hay.contains("隧道") {
            "VPN"
        } else if self.is_virtual() {
            "虚拟"
        } else {
            "物理"
        }
    }
}

/// 默认路由条目（用于判定当前上网网卡）。
#[derive(Debug, Clone, Deserialize, Default)]
struct RouteInfo {
    #[serde(rename = "InterfaceIndex", default)]
    interface_index: u32,
    #[serde(rename = "InterfaceAlias", default)]
    interface_alias: String,
    #[serde(rename = "RouteMetric", default)]
    route_metric: u32,
    #[serde(rename = "InterfaceMetric", default)]
    interface_metric: u32,
    #[serde(rename = "NextHop", default)]
    next_hop: String,
}

/// 枚举所有网卡（含虚拟网卡，调用方自行过滤）。
pub fn list_adapters() -> Vec<Adapter> {
    let script = "Get-NetAdapter -ErrorAction SilentlyContinue | \
                  Select-Object InterfaceIndex,Name,InterfaceDescription,@{N='InterfaceType';E={[string]$_.InterfaceType}},Status,HardwareInterface | \
                  ConvertTo-Json -Compress";
    let Some(json) = powershell::run_ps_json(script) else {
        return Vec::new();
    };
    parse_adapters(&json)
}

/// 判定「当前上网网卡」：取默认路由接口（多默认路由取跃点最低者）。
pub fn get_active_adapter() -> Option<Adapter> {
    let route = get_default_route()?;
    list_adapters()
        .into_iter()
        .find(|a| a.interface_index == route.interface_index)
}

/// 公开接口：返回默认路由下一跳 IP（供诊断使用）。
pub fn get_default_route_public() -> Option<String> {
    get_default_route().map(|r| r.next_hop)
}

/// 返回默认路由信息。
fn get_default_route() -> Option<RouteInfo> {
    for prefix in ["0.0.0.0/0", "::/0"] {
        let script = format!(
            "Get-NetRoute -DestinationPrefix '{prefix}' -ErrorAction SilentlyContinue | \
             Sort-Object RouteMetric,InterfaceMetric | Select-Object -First 1 \
             InterfaceIndex,InterfaceAlias,RouteMetric,InterfaceMetric,NextHop | ConvertTo-Json -Compress"
        );
        if let Some(json) = powershell::run_ps_json(&script) {
            if let Ok(r) = serde_json::from_str::<RouteInfo>(&json) {
                return Some(r);
            }
            // 可能返回数组（多网卡同 metric）
            if let Ok(list) = serde_json::from_str::<Vec<RouteInfo>>(&json) {
                return list.into_iter().next();
            }
        }
    }
    None
}

/// 解析 `Get-NetAdapter | ConvertTo-Json` 输出（单对象或数组）。
fn parse_adapters(json: &str) -> Vec<Adapter> {
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    match value {
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| serde_json::from_value::<Adapter>(v.clone()).ok())
            .collect(),
        serde_json::Value::Object(_) => serde_json::from_value::<Adapter>(value)
            .ok()
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_keyword_detection() {
        let a = Adapter {
            name: "vEthernet (WSL)".into(),
            description: "Hyper-V Virtual Ethernet Adapter".into(),
            hardware_interface: false,
            ..Default::default()
        };
        assert!(a.is_virtual());

        let b = Adapter {
            name: "以太网".into(),
            description: "Realtek PCIe GbE".into(),
            hardware_interface: true,
            status: "Up".into(),
            ..Default::default()
        };
        assert!(!b.is_virtual());
        assert!(b.is_up());
        assert_eq!(b.kind_label(), "物理");
    }

    #[test]
    fn parse_single_or_array() {
        let single = r#"{"InterfaceIndex":12,"Name":"以太网"}"#;
        assert_eq!(parse_adapters(single).len(), 1);
        let arr =
            r#"[{"InterfaceIndex":12,"Name":"以太网"},{"InterfaceIndex":1,"Name":"Loopback"}]"#;
        assert_eq!(parse_adapters(arr).len(), 2);
    }
}
