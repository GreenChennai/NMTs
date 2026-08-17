//! netsh 调用封装。
#![allow(dead_code)] // v0.1 骨架：静态 IP / DNS 设置等 API 供 v0.2 模块二使用

use std::time::Duration;

use super::{run, CmdOutput};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// 执行 netsh 命令。
pub fn run_netsh(args: &[&str]) -> CmdOutput {
    run("netsh", args, DEFAULT_TIMEOUT)
}

/// 查询指定网卡的 DNS 配置。
pub fn show_dns(iface: &str) -> CmdOutput {
    run_netsh(&["interface", "ip", "show", "dns", iface])
}

/// 设置静态 DNS。
pub fn set_dns(iface: &str, dns: &str) -> CmdOutput {
    run_netsh(&["interface", "ip", "set", "dns", iface, "static", dns])
}

/// 设置静态 IPv6 DNS（主）。
pub fn set_dns_v6(iface: &str, dns: &str) -> CmdOutput {
    run_netsh(&[
        "interface",
        "ipv6",
        "set",
        "dnsservers",
        iface,
        "static",
        dns,
    ])
}

/// 设置静态 IPv6 地址（地址含 /前缀，例如 `2001:db8::1/64`）。
pub fn set_address_v6(iface: &str, addr_with_prefix: &str) -> CmdOutput {
    run_netsh(&[
        "interface",
        "ipv6",
        "set",
        "address",
        iface,
        "static",
        addr_with_prefix,
    ])
}

/// 设置 IPv6 默认网关（::/0 下一跳）。先删后加，避免重复路由。
pub fn set_gateway_v6(iface: &str, gw: &str) -> CmdOutput {
    let _ = run_netsh(&["interface", "ipv6", "delete", "route", "::/0", iface]);
    run_netsh(&["interface", "ipv6", "add", "route", "::/0", iface, gw])
}

/// 追加备用 IPv6 DNS。
pub fn add_dns_v6(iface: &str, dns: &str) -> CmdOutput {
    run_netsh(&[
        "interface",
        "ipv6",
        "add",
        "dnsservers",
        iface,
        dns,
        "index=2",
    ])
}

/// IPv6 DNS 切回 DHCP。
pub fn set_dns_v6_dhcp(iface: &str) -> CmdOutput {
    run_netsh(&["interface", "ipv6", "set", "dnsservers", iface, "dhcp"])
}

/// 切回 DHCP 自动获取 DNS。
pub fn set_dns_dhcp(iface: &str) -> CmdOutput {
    run_netsh(&["interface", "ip", "set", "dns", iface, "dhcp"])
}

/// 设置静态 IP / 掩码 / 网关。
pub fn set_static_ip(iface: &str, ip: &str, mask: &str, gw: &str) -> CmdOutput {
    run_netsh(&[
        "interface",
        "ip",
        "set",
        "address",
        iface,
        "static",
        ip,
        mask,
        gw,
    ])
}

/// 切回 DHCP 自动获取 IP。
pub fn set_dhcp(iface: &str) -> CmdOutput {
    run_netsh(&["interface", "ip", "set", "address", iface, "dhcp"])
}

/// 刷新 DNS 缓存。
pub fn flush_dns() -> CmdOutput {
    run("ipconfig", &["/flushdns"], DEFAULT_TIMEOUT)
}

/// 释放 / 续租 IP。
pub fn ipconfig_release(iface: &str) -> CmdOutput {
    run("ipconfig", &["/release", iface], DEFAULT_TIMEOUT)
}

pub fn ipconfig_renew(iface: &str) -> CmdOutput {
    run("ipconfig", &["/renew", iface], DEFAULT_TIMEOUT)
}

/// 输出 `ipconfig /all` 文本。
pub fn ipconfig_all() -> CmdOutput {
    run("ipconfig", &["/all"], DEFAULT_TIMEOUT)
}

/// 输出路由表。
pub fn route_print() -> CmdOutput {
    run("route", &["print"], DEFAULT_TIMEOUT)
}

/// 输出 ARP 表。
pub fn arp_table() -> CmdOutput {
    run("arp", &["-a"], DEFAULT_TIMEOUT)
}
