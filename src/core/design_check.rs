//! 模块四：前置条件校验（设计期预检）。
//!
//! 将 `Intent`（声明式约束）逐个对 `(devices, links, configs)` 求值，输出问题
//! 清单；`Error` 级阻断 CLI 生成 / 下发，`Warn` 级给出风险提醒。
#![allow(dead_code)] // 部分 Intent 变体 / Finding 字段供 v1.0 完整预检使用

use std::net::Ipv4Addr;

use petgraph::algo::is_cyclic_undirected;

use super::topology::{Device, DeviceRole, RoutingProtocol, Topology};

/// 问题严重度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warn,
    Info,
}

impl Severity {
    pub fn label(&self) -> &'static str {
        match self {
            Severity::Error => "错误",
            Severity::Warn => "警告",
            Severity::Info => "信息",
        }
    }
}

/// 前置条件（声明式约束）。
#[derive(Debug, Clone)]
pub enum Intent {
    /// 所有 L3 子网不重叠。
    UniqueSubnet,
    /// 管理 VLAN 与业务 VLAN 分离。
    MgmtVlanSeparate,
    /// 指定角色设备双上联（防单点）。
    RedundantUplink { role: DeviceRole },
    /// VLAN 须传播到某类设备。
    VlanPropagated { vlan: u16, to: DeviceRole },
    /// 同域路由协议一致。
    RoutingConsistent,
    /// 冗余链路须有 STP，避免二层环路。
    NoLoop,
    /// 链路两端 MTU 一致。
    MtuMatch,
    /// 关键链路冗余。
    NoSinglePointOfFailure,
}

impl Intent {
    pub fn label(&self) -> String {
        match self {
            Intent::UniqueSubnet => "子网不重叠".into(),
            Intent::MgmtVlanSeparate => "管理/业务 VLAN 分离".into(),
            Intent::RedundantUplink { .. } => "冗余上联".into(),
            Intent::VlanPropagated { vlan, to } => format!("VLAN {vlan} 传播到{}", to.label()),
            Intent::RoutingConsistent => "路由协议一致".into(),
            Intent::NoLoop => "无二层环路".into(),
            Intent::MtuMatch => "MTU 一致".into(),
            Intent::NoSinglePointOfFailure => "无单点故障".into(),
        }
    }
}

/// 校验发现的问题。
#[derive(Debug, Clone)]
pub struct Finding {
    pub intent: String,
    pub severity: Severity,
    pub devices: Vec<String>,
    pub message: String,
    pub suggestion: String,
}

/// 执行预检，返回问题清单。
pub fn check(t: &Topology, intents: &[Intent]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for intent in intents {
        match intent {
            Intent::UniqueSubnet => check_unique_subnet(t, &mut findings),
            Intent::MgmtVlanSeparate => check_mgmt_vlan(t, &mut findings),
            Intent::RedundantUplink { role } => check_redundant_uplink(t, *role, &mut findings),
            Intent::VlanPropagated { vlan, to } => {
                check_vlan_propagated(t, *vlan, *to, &mut findings)
            }
            Intent::RoutingConsistent => check_routing(t, &mut findings),
            Intent::NoLoop => check_loop(t, &mut findings),
            Intent::MtuMatch => check_mtu(t, &mut findings),
            Intent::NoSinglePointOfFailure => check_spof(t, &mut findings),
        }
    }
    findings
}

fn check_unique_subnet(t: &Topology, out: &mut Vec<Finding>) {
    let mut subs: Vec<(&Device, u32, u32)> = Vec::new(); // (device, network, prefix)
    for d in &t.devices {
        for l3 in &d.config.l3_intfs {
            if let Some((net, prefix)) = parse_cidr(&l3.subnet) {
                subs.push((d, net, prefix));
            }
        }
    }
    for i in 0..subs.len() {
        for j in (i + 1)..subs.len() {
            let (da, na, pa) = subs[i];
            let (db, nb, pb) = subs[j];
            if cidr_overlap(na, pa, nb, pb) {
                out.push(Finding {
                    intent: "子网不重叠".into(),
                    severity: Severity::Error,
                    devices: vec![da.id.clone(), db.id.clone()],
                    message: format!(
                        "{} 与 {} 的 L3 子网重叠（{}/{} vs {}/{}）",
                        da.name, db.name, na, pa, nb, pb
                    ),
                    suggestion: "调整网段，保证各三层子网互不重叠".into(),
                });
            }
        }
    }
}

fn check_mgmt_vlan(t: &Topology, out: &mut Vec<Finding>) {
    // 简化：若某设备同时存在 access_vlan 与三层管理接口，提示分离
    for d in &t.devices {
        let has_l3 = !d.config.l3_intfs.is_empty();
        let has_access = d.config.access_vlan.is_some();
        if has_l3 && has_access {
            out.push(Finding {
                intent: "管理/业务 VLAN 分离".into(),
                severity: Severity::Info,
                devices: vec![d.id.clone()],
                message: format!(
                    "{} 同时配置了业务 access VLAN 与三层接口，注意管理流量与业务流量分离",
                    d.name
                ),
                suggestion: "建议管理 VLAN 独立规划".into(),
            });
        }
    }
}

fn check_redundant_uplink(t: &Topology, role: DeviceRole, out: &mut Vec<Finding>) {
    for d in t.devices.iter().filter(|d| d.role == role) {
        let uplink_count = t
            .neighbors(&d.id)
            .iter()
            .filter_map(|n| t.device(n))
            .filter(|n| is_uplink_role(d.role, n.role))
            .count();
        if uplink_count < 2 {
            out.push(Finding {
                intent: "冗余上联".into(),
                severity: Severity::Warn,
                devices: vec![d.id.clone()],
                message: format!(
                    "{}（{}）仅 {uplink_count} 条上联，存在单点故障风险",
                    d.name,
                    role.label()
                ),
                suggestion: "增加第二条上联链路".into(),
            });
        }
    }
}

fn check_vlan_propagated(t: &Topology, vlan: u16, to: DeviceRole, out: &mut Vec<Finding>) {
    for d in t.devices.iter().filter(|d| d.role == to) {
        let needs = d.config.access_vlan == Some(vlan)
            || d.config.vlans.iter().any(|v| v.id == vlan)
            || d.config.trunk_vlans.contains(&vlan);
        if !needs {
            continue;
        }
        // 检查其上联设备是否 trunk 放行该 VLAN
        let upstream_ok = t
            .neighbors(&d.id)
            .iter()
            .filter_map(|n| t.device(n))
            .any(|u| u.config.trunk_vlans.contains(&vlan));
        if !upstream_ok {
            out.push(Finding {
                intent: format!("VLAN {vlan} 传播"),
                severity: Severity::Error,
                devices: vec![d.id.clone()],
                message: format!(
                    "{} 需要 VLAN {vlan}，但上联设备 trunk 未放行该 VLAN",
                    d.name
                ),
                suggestion: format!("在上联 trunk 接口放行 VLAN {vlan}"),
            });
        }
    }
}

fn check_routing(t: &Topology, out: &mut Vec<Finding>) {
    let protos: Vec<RoutingProtocol> = t
        .devices
        .iter()
        .filter(|d| !d.config.l3_intfs.is_empty())
        .map(|d| d.config.routing)
        .collect();
    if protos.is_empty() {
        return;
    }
    let first = protos[0];
    if protos.iter().any(|p| *p != first) {
        out.push(Finding {
            intent: "路由协议一致".into(),
            severity: Severity::Warn,
            devices: t
                .devices
                .iter()
                .filter(|d| !d.config.l3_intfs.is_empty())
                .map(|d| d.id.clone())
                .collect(),
            message: "三层设备路由协议不一致，可能导致路由不通".into(),
            suggestion: "统一同域路由协议（静态 / OSPF）".into(),
        });
    }
}

fn check_loop(t: &Topology, out: &mut Vec<Finding>) {
    let g = t.build_graph();
    if is_cyclic_undirected(&g) {
        let no_stp: Vec<String> = t
            .devices
            .iter()
            .filter(|d| {
                !d.config.stp_enabled
                    && matches!(
                        d.role,
                        DeviceRole::Core | DeviceRole::Dist | DeviceRole::Access
                    )
            })
            .map(|d| d.id.clone())
            .collect();
        if !no_stp.is_empty() {
            out.push(Finding {
                intent: "无二层环路".into(),
                severity: Severity::Error,
                devices: no_stp,
                message: "拓扑存在冗余环路，但以下设备未启用 STP，存在广播风暴风险".into(),
                suggestion: "启用生成树协议（STP）".into(),
            });
        }
    }
}

fn check_mtu(t: &Topology, out: &mut Vec<Finding>) {
    for l in &t.links {
        let a = t.device(&l.from).and_then(|d| d.config.mtu);
        let b = t.device(&l.to).and_then(|d| d.config.mtu);
        if let (Some(a), Some(b)) = (a, b) {
            if a != b {
                out.push(Finding {
                    intent: "MTU 一致".into(),
                    severity: Severity::Warn,
                    devices: vec![l.from.clone(), l.to.clone()],
                    message: format!(
                        "链路 {}↔{} MTU 不一致（{a} vs {b}），大包可能分片/丢包",
                        l.from, l.to
                    ),
                    suggestion: "统一链路两端 MTU".into(),
                });
            }
        }
    }
}

fn check_spof(t: &Topology, out: &mut Vec<Finding>) {
    // 单点：某设备只有一条链路，且不是叶子接入设备
    for d in &t.devices {
        let deg = t.neighbors(&d.id).len();
        if deg <= 1
            && matches!(
                d.role,
                DeviceRole::Core | DeviceRole::Dist | DeviceRole::Router
            )
        {
            out.push(Finding {
                intent: "无单点故障".into(),
                severity: Severity::Warn,
                devices: vec![d.id.clone()],
                message: format!("{} 仅 {deg} 条链路，是关键单点", d.name),
                suggestion: "增加冗余链路 / 设备".into(),
            });
        }
    }
}

fn is_uplink_role(down: DeviceRole, up: DeviceRole) -> bool {
    // 接入→汇聚/核心、汇聚→核心 视为上联
    matches!(
        (down, up),
        (DeviceRole::Access, DeviceRole::Dist)
            | (DeviceRole::Access, DeviceRole::Core)
            | (DeviceRole::Dist, DeviceRole::Core)
    )
}

/// 解析 CIDR 为 (网络号, 前缀长度)。
fn parse_cidr(s: &str) -> Option<(u32, u32)> {
    let (ip, len) = s.split_once('/')?;
    let ip: Ipv4Addr = ip.trim().parse().ok()?;
    let len: u32 = len.trim().parse().ok()?;
    if len > 32 {
        return None;
    }
    Some((u32::from(ip), len))
}

/// 判断两个 CIDR 是否重叠。
fn cidr_overlap(na: u32, pa: u32, nb: u32, pb: u32) -> bool {
    let mask = |l: u32| if l == 0 { 0 } else { u32::MAX << (32 - l) };
    let min_len = pa.min(pb);
    let m = mask(min_len);
    (na & m) == (nb & m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::topology::{DeviceConfig, L3Intf, Link};

    fn topo() -> Topology {
        let mut t = Topology::default();
        t.devices.push(Device {
            id: "core".into(),
            name: "核心".into(),
            vendor: crate::core::topology::Vendor::HuaweiVrp,
            role: DeviceRole::Core,
            mgmt_ip: String::new(),
            creds: None,
            config: DeviceConfig {
                hostname: "CORE".into(),
                trunk_vlans: vec![10],
                stp_enabled: true,
                l3_intfs: vec![L3Intf {
                    name: "Vlanif10".into(),
                    subnet: "192.168.10.0/24".into(),
                    vrrp: false,
                }],
                ..Default::default()
            },
        });
        t.devices.push(Device {
            id: "acc1".into(),
            name: "接入1".into(),
            vendor: crate::core::topology::Vendor::HuaweiVrp,
            role: DeviceRole::Access,
            mgmt_ip: String::new(),
            creds: None,
            config: DeviceConfig {
                hostname: "ACC1".into(),
                access_vlan: Some(10),
                stp_enabled: true,
                l3_intfs: vec![L3Intf {
                    name: "Vlanif10".into(),
                    subnet: "192.168.10.0/24".into(),
                    vrrp: false,
                }],
                ..Default::default()
            },
        });
        t.links.push(Link {
            from: "core".into(),
            to: "acc1".into(),
            from_port: String::new(),
            to_port: String::new(),
            from_ip: String::new(),
            to_ip: String::new(),
        });
        t
    }

    #[test]
    fn subnet_overlap_detected() {
        let t = topo();
        let f = check(&t, &[Intent::UniqueSubnet]);
        assert!(f
            .iter()
            .any(|x| x.severity == Severity::Error && x.message.contains("重叠")));
    }

    #[test]
    fn vlan_propagated_ok_when_trunk_allows() {
        let t = topo();
        let f = check(
            &t,
            &[Intent::VlanPropagated {
                vlan: 10,
                to: DeviceRole::Access,
            }],
        );
        assert!(!f.iter().any(|x| x.severity == Severity::Error));
    }

    #[test]
    fn redundant_uplink_warns_single() {
        let t = topo();
        let f = check(
            &t,
            &[Intent::RedundantUplink {
                role: DeviceRole::Access,
            }],
        );
        assert!(f
            .iter()
            .any(|x| x.severity == Severity::Warn && x.message.contains("1 条上联")));
    }

    #[test]
    fn cidr_overlap_cases() {
        assert!(cidr_overlap(0xC0A80A00, 24, 0xC0A80A01, 24)); // 192.168.10.0/24 vs 192.168.10.1/24
        assert!(!cidr_overlap(0xC0A80A00, 24, 0xC0A80B00, 24)); // 10.0/24 vs 11.0/24
    }
}
