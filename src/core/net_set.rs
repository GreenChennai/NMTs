//! 模块二：快捷设置（封装 netsh / PowerShell）。
//!
//! 原则：所有「修改类」操作前先备份（`netsh interface ip/ipv6 dump` + tcp
//! global），可一键回退；回退按钮在优化成功后常驻。
#![allow(dead_code)] // add_dns 供 v0.6 DNS 优选引擎设置主/备 DNS 使用

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::windows::{netsh, powershell, run, CmdOutput};

/// 备份目录相对程序根。
pub fn backup_dir(root: &Path) -> PathBuf {
    root.join("backups")
}

/// 备份当前网络配置，返回备份目录路径。
pub fn backup_network(root: &Path) -> std::result::Result<PathBuf, String> {
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dir = backup_dir(root).join(ts.to_string());
    fs::create_dir_all(&dir).map_err(|e| format!("创建备份目录失败: {e}"))?;

    let ip_dump = netsh::run_netsh(&["interface", "ip", "dump"]);
    if ip_dump.success {
        let _ = fs::write(dir.join("netsh_ip_dump.txt"), &ip_dump.stdout);
    }

    let ipv6_dump = netsh::run_netsh(&["interface", "ipv6", "dump"]);
    if ipv6_dump.success {
        let _ = fs::write(dir.join("netsh_ipv6_dump.txt"), &ipv6_dump.stdout);
    }

    let tcp = netsh::run_netsh(&["int", "tcp", "show", "global"]);
    if tcp.success {
        let _ = fs::write(dir.join("tcp_global.txt"), &tcp.stdout);
    }

    Ok(dir)
}

/// 从备份目录恢复网络配置。
pub fn restore_network(dir: &Path) -> std::result::Result<(), String> {
    let ip = dir.join("netsh_ip_dump.txt");
    if ip.exists() {
        let p = ip.to_string_lossy();
        let out = run("netsh", &["-f", p.as_ref()], Duration::from_secs(30));
        if !out.success {
            return Err(format!("恢复 IP 配置失败: {}", out.combined()));
        }
    }
    let ipv6 = dir.join("netsh_ipv6_dump.txt");
    if ipv6.exists() {
        let p = ipv6.to_string_lossy();
        let _ = run("netsh", &["-f", p.as_ref()], Duration::from_secs(30));
    }
    Ok(())
}

/// 列出所有备份目录（按时间倒序）。
pub fn list_backups(root: &Path) -> Vec<PathBuf> {
    let dir = backup_dir(root);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut list: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();
    list.sort();
    list.reverse();
    list
}

/// 刷新 DNS 缓存。
pub fn flush_dns() -> CmdOutput {
    netsh::flush_dns()
}

/// 设置静态 IP / 掩码 / 网关。
pub fn set_static_ip(iface: &str, ip: &str, mask: &str, gw: &str) -> CmdOutput {
    netsh::set_static_ip(iface, ip, mask, gw)
}

/// 设置静态 DNS（主）。
pub fn set_dns(iface: &str, dns: &str) -> CmdOutput {
    netsh::set_dns(iface, dns)
}

/// 追加备用 DNS。
pub fn add_dns(iface: &str, dns: &str) -> CmdOutput {
    netsh::run_netsh(&["interface", "ip", "add", "dns", iface, dns, "index=2"])
}

/// 设置静态 IPv6 DNS（主）。
pub fn set_dns_v6(iface: &str, dns: &str) -> CmdOutput {
    netsh::set_dns_v6(iface, dns)
}

/// 追加备用 IPv6 DNS。
pub fn add_dns_v6(iface: &str, dns: &str) -> CmdOutput {
    netsh::add_dns_v6(iface, dns)
}

/// DNS 切回 DHCP。
pub fn set_dns_dhcp(iface: &str) -> CmdOutput {
    netsh::set_dns_dhcp(iface)
}

/// IP 切回 DHCP。
pub fn set_ip_dhcp(iface: &str) -> CmdOutput {
    netsh::set_dhcp(iface)
}

/// 释放并续租 IP。
pub fn release_renew(iface: &str) -> CmdOutput {
    let rel = netsh::ipconfig_release(iface);
    let ren = netsh::ipconfig_renew(iface);
    CmdOutput {
        success: ren.success,
        exit_code: ren.exit_code,
        stdout: format!("{}\n{}", rel.stdout, ren.stdout),
        stderr: format!("{}\n{}", rel.stderr, ren.stderr),
    }
}

/// 开 / 关 IPv6（全局）。
pub fn set_ipv6(enabled: bool) -> CmdOutput {
    netsh::run_netsh(&[
        "interface",
        "ipv6",
        "set",
        "state",
        if enabled { "enabled" } else { "disabled" },
    ])
}

/// 查询 IPv6 是否启用（注册表 DisabledComponents）。
pub fn ipv6_enabled() -> bool {
    let script =
        "$d=(Get-ItemProperty 'HKLM:\\SYSTEM\\CurrentControlSet\\Services\\Tcpip6\\Parameters' \
                  -Name DisabledComponents -ErrorAction SilentlyContinue).DisabledComponents; \
                  [PSCustomObject]@{ v=[int]$d } | ConvertTo-Json -Compress";
    let Some(json) = powershell::run_ps_json(script) else {
        return true;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) else {
        return true;
    };
    v.get("v").and_then(|x| x.as_i64()).unwrap_or(0) == 0
}

/// TCP 全局优化。
pub fn tcp_optimize() -> CmdOutput {
    netsh::run_netsh(&[
        "int",
        "tcp",
        "set",
        "global",
        "autotuninglevel=normal",
        "ecncapability=enabled",
    ])
}
