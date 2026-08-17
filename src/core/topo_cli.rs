//! 模块四：CLI 推导——据拓扑 + 设备参数 + 厂商模板生成每台设备的配置。
//!
//! 生成结果可导出或经模块三下发。v0.5 覆盖：主机名 / VLAN / trunk / access
//! 端口 / 三层接口 / 静态路由。
#![allow(dead_code)]

use super::topology::{Device, RoutingProtocol, Topology, Vendor};

/// 生成单台设备的 CLI 配置。
pub fn generate_device_cli(device: &Device, topo: &Topology) -> String {
    match device.vendor {
        Vendor::HuaweiVrp | Vendor::H3cVrp => gen_vrp(device, topo),
        Vendor::CiscoIos => gen_ios(device, topo),
    }
}

fn gen_vrp(device: &Device, topo: &Topology) -> String {
    let c = &device.config;
    let mut s = String::new();
    s.push_str("#\n");
    s.push_str(&format!("sysname {}\n", c.hostname));
    s.push_str("#\n");

    // VLAN
    if !c.vlans.is_empty() {
        let ids: Vec<String> = c.vlans.iter().map(|v| v.id.to_string()).collect();
        s.push_str(&format!("vlan batch {}\n", ids.join(" ")));
    } else if let Some(av) = c.access_vlan {
        s.push_str(&format!("vlan batch {av}\n"));
    }

    // 端口
    let ports = device_ports(device, topo);
    for p in &ports {
        s.push_str(&format!("interface {}\n", p.name));
        if !c.trunk_vlans.is_empty() {
            let ids: Vec<String> = c.trunk_vlans.iter().map(|v| v.to_string()).collect();
            s.push_str(" port link-type trunk\n");
            s.push_str(&format!(" port trunk allow-pass vlan {}\n", ids.join(" ")));
        } else if let Some(av) = c.access_vlan {
            s.push_str(&format!(" port default vlan {av}\n"));
        }
        s.push_str("#\n");
    }

    // 三层接口
    for l3 in &c.l3_intfs {
        s.push_str(&format!("interface {}\n", l3.name));
        if let Some((ip, mask)) = split_cidr(&l3.subnet) {
            s.push_str(&format!(" ip address {ip} {mask}\n"));
        }
        s.push_str("#\n");
    }

    // 静态路由
    if c.routing == RoutingProtocol::Static {
        if let Some(ip) = c.l3_intfs.first().and_then(|l| l.subnet.split('/').next()) {
            let _ = ip;
            s.push_str("ip route-static 0.0.0.0 0.0.0.0 <下一跳>\n");
        }
    }
    s.push_str("return\n");
    s
}

fn gen_ios(device: &Device, topo: &Topology) -> String {
    let c = &device.config;
    let mut s = String::new();
    s.push_str("!\n");
    s.push_str(&format!("hostname {}\n", c.hostname));
    s.push_str("!\n");

    for v in &c.vlans {
        s.push_str(&format!("vlan {}\n name {}\n", v.id, v.name));
    }
    if c.vlans.is_empty() {
        if let Some(av) = c.access_vlan {
            s.push_str(&format!("vlan {av}\n"));
        }
    }

    let ports = device_ports(device, topo);
    for p in &ports {
        s.push_str(&format!("interface {}\n", p.name));
        if !c.trunk_vlans.is_empty() {
            let ids: Vec<String> = c.trunk_vlans.iter().map(|v| v.to_string()).collect();
            s.push_str(" switchport mode trunk\n");
            s.push_str(&format!(
                " switchport trunk allowed vlan {}\n",
                ids.join(",")
            ));
        } else if let Some(av) = c.access_vlan {
            s.push_str(" switchport mode access\n");
            s.push_str(&format!(" switchport access vlan {av}\n"));
        }
        s.push_str("!\n");
    }

    for l3 in &c.l3_intfs {
        s.push_str(&format!("interface {}\n", l3.name));
        if let Some((ip, mask)) = split_cidr(&l3.subnet) {
            s.push_str(&format!(" ip address {ip} {mask}\n"));
        }
        s.push_str("!\n");
    }
    if c.routing == RoutingProtocol::Static {
        s.push_str("ip route 0.0.0.0 0.0.0.0 <下一跳>\n");
    }
    s.push_str("end\n");
    s
}

/// 设备在拓扑中涉及的端口。
fn device_ports(device: &Device, topo: &Topology) -> Vec<PortName> {
    topo.links
        .iter()
        .filter_map(|l| {
            if l.from == device.id {
                Some(PortName {
                    name: l.from_port.clone(),
                })
            } else if l.to == device.id {
                Some(PortName {
                    name: l.to_port.clone(),
                })
            } else {
                None
            }
        })
        .filter(|p| !p.name.is_empty())
        .collect()
}

struct PortName {
    name: String,
}

/// 拆分 CIDR 为 (ip, 掩码)。24 → 255.255.255.0。
fn split_cidr(cidr: &str) -> Option<(String, String)> {
    let (ip, len) = cidr.split_once('/')?;
    let len: u32 = len.trim().parse().ok()?;
    if len > 32 {
        return None;
    }
    let mask = if len == 0 { 0 } else { u32::MAX << (32 - len) };
    let mask_str = format!(
        "{}.{}.{}.{}",
        (mask >> 24) & 0xff,
        (mask >> 16) & 0xff,
        (mask >> 8) & 0xff,
        mask & 0xff
    );
    Some((ip.trim().to_string(), mask_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::topology::{DeviceConfig, Link};

    fn huawei_core() -> Topology {
        let mut t = Topology::default();
        t.devices.push(Device {
            id: "core".into(),
            name: "核心".into(),
            vendor: Vendor::HuaweiVrp,
            role: crate::core::topology::DeviceRole::Core,
            mgmt_ip: String::new(),
            creds: None,
            config: DeviceConfig {
                hostname: "CORE".into(),
                vlans: vec![crate::core::topology::Vlan {
                    id: 10,
                    name: "业务".into(),
                    purpose: String::new(),
                }],
                trunk_vlans: vec![10],
                ..Default::default()
            },
            ..Default::default()
        });
        t.devices.push(Device {
            id: "acc1".into(),
            name: "接入1".into(),
            vendor: Vendor::HuaweiVrp,
            role: crate::core::topology::DeviceRole::Access,
            mgmt_ip: String::new(),
            creds: None,
            config: DeviceConfig {
                hostname: "ACC1".into(),
                access_vlan: Some(10),
                ..Default::default()
            },
            ..Default::default()
        });
        t.links.push(Link {
            from: "core".into(),
            to: "acc1".into(),
            from_port: "GE0/0/1".into(),
            to_port: "GE0/0/1".into(),
            from_ip: String::new(),
            to_ip: String::new(),
        });
        t
    }

    #[test]
    fn vrp_generates_sysname_vlan_trunk() {
        let t = huawei_core();
        let cli = generate_device_cli(t.device("core").unwrap(), &t);
        assert!(cli.contains("sysname CORE"));
        assert!(cli.contains("vlan batch 10"));
        assert!(cli.contains("interface GE0/0/1"));
        assert!(cli.contains("port trunk allow-pass vlan 10"));
    }

    #[test]
    fn cidr_to_mask() {
        assert_eq!(split_cidr("192.168.1.0/24").unwrap().1, "255.255.255.0");
        assert_eq!(split_cidr("10.0.0.0/8").unwrap().1, "255.0.0.0");
    }
}
