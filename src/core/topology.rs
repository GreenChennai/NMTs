//! 模块四：拓扑数据模型 + 图构建 + D2 导出。
//!
//! 拓扑是「网络设计模型」：每个设备带完整参数（`DeviceConfig`），链路描述端口
//! 角色；`petgraph` 承载图数据供预检 / 遍历；渲染交给 D2（图表即代码）。
#![allow(dead_code)] // PortConf/PortKind 等字段供 v1.0 拓扑编辑窗口使用

use petgraph::graph::{Graph, UnGraph};
use serde::{Deserialize, Serialize};

/// 厂商。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Vendor {
    #[serde(rename = "huawei_vrp")]
    HuaweiVrp,
    #[serde(rename = "h3c_vrp")]
    H3cVrp,
    #[serde(rename = "cisco_ios")]
    CiscoIos,
}

impl Vendor {
    pub fn id(&self) -> &'static str {
        match self {
            Vendor::HuaweiVrp => "huawei_vrp",
            Vendor::H3cVrp => "h3c_vrp",
            Vendor::CiscoIos => "cisco_ios",
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Vendor::HuaweiVrp => "华为",
            Vendor::H3cVrp => "H3C",
            Vendor::CiscoIos => "Cisco",
        }
    }
}

/// 设备角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceRole {
    Core,
    Dist,
    Access,
    Router,
    Firewall,
}

impl DeviceRole {
    pub fn label(&self) -> &'static str {
        match self {
            DeviceRole::Core => "核心",
            DeviceRole::Dist => "汇聚",
            DeviceRole::Access => "接入",
            DeviceRole::Router => "路由器",
            DeviceRole::Firewall => "防火墙",
        }
    }
}

/// 设备登录凭据（供模块三自动连接）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    #[serde(default = "default_protocol")]
    pub protocol: String,
}

fn default_protocol() -> String {
    "ssh".to_string()
}

/// VLAN。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vlan {
    pub id: u16,
    pub name: String,
    #[serde(default)]
    pub purpose: String,
}

/// 三层接口 / 网关。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L3Intf {
    pub name: String,
    /// 子网，形如 `192.168.1.0/24`。
    pub subnet: String,
    #[serde(default)]
    pub vrrp: bool,
}

/// 端口角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortKind {
    Uplink,
    Downlink,
    Trunk,
    Access,
}

/// 端口配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortConf {
    pub name: String,
    pub kind: PortKind,
    #[serde(default)]
    pub access_vlan: Option<u16>,
}

/// 路由协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RoutingProtocol {
    Static,
    Ospf,
    #[default]
    None,
}

/// 设备级默认参数（可在设备窗口编辑）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub hostname: String,
    #[serde(default)]
    pub vlans: Vec<Vlan>,
    #[serde(default)]
    pub trunk_vlans: Vec<u16>,
    #[serde(default)]
    pub access_vlan: Option<u16>,
    #[serde(default)]
    pub l3_intfs: Vec<L3Intf>,
    #[serde(default)]
    pub routing: RoutingProtocol,
    #[serde(default)]
    pub stp_enabled: bool,
    #[serde(default)]
    pub dhcp_server: bool,
    /// 链路 MTU（用于 MtuMatch 校验）。
    #[serde(default)]
    pub mtu: Option<u16>,
}

/// 设备节点。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub vendor: Vendor,
    pub role: DeviceRole,
    #[serde(default)]
    pub mgmt_ip: String,
    #[serde(default)]
    pub creds: Option<Credentials>,
    #[serde(default)]
    pub config: DeviceConfig,
}

/// 链路。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub from_port: String,
    #[serde(default)]
    pub to_port: String,
}

/// 拓扑。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Topology {
    pub devices: Vec<Device>,
    #[serde(default)]
    pub links: Vec<Link>,
}

impl Topology {
    pub fn device(&self, id: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }

    /// 与某设备相连的所有设备 id。
    pub fn neighbors(&self, id: &str) -> Vec<String> {
        self.links
            .iter()
            .filter_map(|l| {
                if l.from == id {
                    Some(l.to.clone())
                } else if l.to == id {
                    Some(l.from.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// 构建无向图（节点权重 = 设备 id 索引）。
    pub fn build_graph(&self) -> UnGraph<String, ()> {
        let mut g: UnGraph<String, ()> = Graph::new_undirected();
        let mut idx = std::collections::HashMap::new();
        for d in &self.devices {
            idx.insert(d.id.clone(), g.add_node(d.id.clone()));
        }
        for l in &self.links {
            if let (Some(&a), Some(&b)) = (idx.get(&l.from), idx.get(&l.to)) {
                g.add_edge(a, b, ());
            }
        }
        g
    }

    /// 导出 D2 文本（渲染交给 d2 CLI）。
    pub fn export_d2(&self) -> String {
        let mut out = String::new();
        out.push_str("# NMTs 拓扑（由 NMTs 生成）\n");
        out.push_str("direction: right\n\n");
        for d in &self.devices {
            let shape = match d.role {
                DeviceRole::Router | DeviceRole::Firewall => "hexagon",
                _ => "rectangle",
            };
            out.push_str(&format!(
                "{}: {} {{\n  shape: {}\n  style.fill: \"#E8F4FD\"\n}}\n",
                d.id, d.name, shape
            ));
        }
        for l in &self.links {
            out.push_str(&format!("{} -> {}\n", l.from, l.to));
        }
        out
    }
}

/// 内置演示拓扑（用于界面展示与预检 / CLI 推导验证）。
pub fn demo_topology() -> Topology {
    let core = Device {
        id: "core".into(),
        name: "核心交换机".into(),
        vendor: Vendor::HuaweiVrp,
        role: DeviceRole::Core,
        mgmt_ip: "10.0.0.1".into(),
        creds: None,
        config: DeviceConfig {
            hostname: "CORE".into(),
            vlans: vec![
                Vlan { id: 10, name: "业务".into(), purpose: "办公".into() },
                Vlan { id: 20, name: "访客".into(), purpose: "Guest".into() },
            ],
            trunk_vlans: vec![10, 20],
            l3_intfs: vec![
                L3Intf { name: "Vlanif10".into(), subnet: "192.168.10.0/24".into(), vrrp: true },
                L3Intf { name: "Vlanif20".into(), subnet: "192.168.20.0/24".into(), vrrp: true },
            ],
            routing: RoutingProtocol::Static,
            stp_enabled: true,
            mtu: Some(1500),
            ..Default::default()
        },
    };
    let dist = Device {
        id: "dist".into(),
        name: "汇聚交换机".into(),
        vendor: Vendor::HuaweiVrp,
        role: DeviceRole::Dist,
        mgmt_ip: "10.0.0.2".into(),
        creds: None,
        config: DeviceConfig {
            hostname: "DIST".into(),
            trunk_vlans: vec![10],
            l3_intfs: vec![L3Intf { name: "Vlanif10".into(), subnet: "192.168.10.0/24".into(), vrrp: true }],
            stp_enabled: true,
            mtu: Some(1500),
            ..Default::default()
        },
    };
    let acc1 = Device {
        id: "acc1".into(),
        name: "接入交换机1".into(),
        vendor: Vendor::HuaweiVrp,
        role: DeviceRole::Access,
        mgmt_ip: "10.0.0.11".into(),
        creds: None,
        config: DeviceConfig { hostname: "ACC1".into(), access_vlan: Some(10), stp_enabled: true, ..Default::default() },
    };
    let acc2 = Device {
        id: "acc2".into(),
        name: "接入交换机2".into(),
        vendor: Vendor::CiscoIos,
        role: DeviceRole::Access,
        mgmt_ip: "10.0.0.12".into(),
        creds: None,
        config: DeviceConfig { hostname: "ACC2".into(), access_vlan: Some(20), stp_enabled: false, ..Default::default() },
    };
    let router = Device {
        id: "r1".into(),
        name: "出口路由器".into(),
        vendor: Vendor::CiscoIos,
        role: DeviceRole::Router,
        mgmt_ip: "10.0.0.254".into(),
        creds: Some(Credentials { username: "admin".into(), password: "".into(), protocol: "ssh".into() }),
        config: DeviceConfig {
            hostname: "R1".into(),
            l3_intfs: vec![L3Intf { name: "GigabitEthernet0/0".into(), subnet: "203.0.113.0/30".into(), vrrp: false }],
            routing: RoutingProtocol::Static,
            ..Default::default()
        },
    };

    Topology {
        devices: vec![core, dist, acc1, acc2, router],
        links: vec![
            Link { from: "core".into(), to: "dist".into(), from_port: "GE0/0/1".into(), to_port: "GE0/0/1".into() },
            Link { from: "dist".into(), to: "acc1".into(), from_port: "GE0/0/2".into(), to_port: "GE0/0/1".into() },
            Link { from: "dist".into(), to: "acc2".into(), from_port: "GE0/0/3".into(), to_port: "Gi0/1".into() },
            Link { from: "core".into(), to: "r1".into(), from_port: "GE0/0/24".into(), to_port: "Gi0/0".into() },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Topology {
        Topology {
            devices: vec![
                Device {
                    id: "core".into(),
                    name: "核心交换机".into(),
                    vendor: Vendor::HuaweiVrp,
                    role: DeviceRole::Core,
                    mgmt_ip: "10.0.0.1".into(),
                    creds: None,
                    config: DeviceConfig {
                        hostname: "CORE".into(),
                        vlans: vec![Vlan { id: 10, name: "业务".into(), purpose: String::new() }],
                        trunk_vlans: vec![10],
                        ..Default::default()
                    },
                },
                Device {
                    id: "acc1".into(),
                    name: "接入交换机".into(),
                    vendor: Vendor::HuaweiVrp,
                    role: DeviceRole::Access,
                    mgmt_ip: "10.0.0.2".into(),
                    creds: None,
                    config: DeviceConfig {
                        hostname: "ACC1".into(),
                        access_vlan: Some(10),
                        ..Default::default()
                    },
                },
            ],
            links: vec![Link {
                from: "core".into(),
                to: "acc1".into(),
                from_port: "GE0/0/1".into(),
                to_port: "GE0/0/1".into(),
            }],
        }
    }

    #[test]
    fn graph_and_neighbors() {
        let t = sample();
        let g = t.build_graph();
        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 1);
        assert_eq!(t.neighbors("core"), vec!["acc1"]);
    }

    #[test]
    fn d2_export() {
        let t = sample();
        let d2 = t.export_d2();
        assert!(d2.contains("core: 核心交换机"));
        assert!(d2.contains("core -> acc1"));
    }

    #[test]
    fn serialize_roundtrip() {
        let t = sample();
        let s = serde_yaml::to_string(&t).unwrap();
        let t2: Topology = serde_yaml::from_str(&s).unwrap();
        assert_eq!(t2.devices.len(), 2);
        assert_eq!(t2.device("core").unwrap().config.hostname, "CORE");
    }

    #[test]
    fn json_roundtrip_editor_exchange() {
        // 编辑器通过 topology.json 交换，验证 serde_json 兼容
        let t = demo_topology();
        let s = serde_json::to_string(&t).unwrap();
        let t2: Topology = serde_json::from_str(&s).unwrap();
        assert_eq!(t2.devices.len(), 5);
        assert_eq!(t2.device("core").unwrap().vendor, Vendor::HuaweiVrp);
        assert_eq!(t2.links.len(), 4);
    }
}
