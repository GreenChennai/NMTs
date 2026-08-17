//! 单次 PowerShell 合并探测：一次调用拿全网络环境数据。
//!
//! 启动/诊断的提速关键——把原本 7 次 PowerShell 冷启动（每次 ~1.5s）合并为
//! 一次调用，返回网卡、默认路由、当前网卡 IP/DNS/网关、系统代理、MTU、
//! 驱动异常、威胁数量、管理员权限等静态数据；动态检测（ping/nslookup）另行走。

use super::adapter::Adapter;
use super::powershell;

/// 一次探测返回的全部静态环境数据。
#[derive(Debug, Clone, Default)]
pub struct NetProbe {
    pub is_admin: bool,
    pub adapters: Vec<Adapter>,
    pub active_index: u32,
    pub next_hop: String,
    pub ip: String,
    pub prefix_len: u32,
    pub gateway: String,
    pub dns: Vec<String>,
    pub dhcp_enabled: bool,
    pub mtu: u32,
    /// 默认路由（0.0.0.0/0）条数，>1 提示多出口 / 环路风险。
    pub route_count: u32,
    pub ie_proxy_enabled: bool,
    pub ie_proxy_server: String,
    pub winhttp_proxy: String,
    /// 探测域名解析是否成功（合并进 PS，省 nslookup 冷启动）。
    pub dns_ok: bool,
    /// IPv6 是否启用（注册表 DisabledComponents == 0）。
    pub ipv6_enabled: bool,
    /// 当前上网网卡的 MAC 地址（模块二只读展示）。
    pub mac: String,
    /// 有异常状态的网络类 PnP 设备（驱动缺失/错误）。
    pub problem_devices: Vec<String>,
    /// Windows Defender 检测到的威胁数。
    pub threat_count: u32,
}

impl NetProbe {
    /// 当前上网网卡。
    pub fn active_adapter(&self) -> Option<&Adapter> {
        self.adapters
            .iter()
            .find(|a| a.interface_index == self.active_index)
    }
}

/// 执行一次合并探测。
pub fn probe_network() -> Option<NetProbe> {
    let script = r#"
$ErrorActionPreference='SilentlyContinue';
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator);
$adapters=Get-NetAdapter | Select-Object InterfaceIndex,Name,InterfaceDescription,@{N='InterfaceType';E={[string]$_.InterfaceType}},Status,@{N='HardwareInterface';E={[bool]$_.HardwareInterface}};
$routes=@(Get-NetRoute -DestinationPrefix '0.0.0.0/0' | Sort-Object RouteMetric,InterfaceMetric);
$route=$routes | Select-Object -First 1;
$route_count=$routes.Count;
$idx=$route.InterfaceIndex;
$ipaddr=Get-NetIPAddress -InterfaceIndex $idx -AddressFamily IPv4 | Where-Object {$_.IPAddress -notlike '169.254*'} | Select-Object -First 1;
$ipiface=Get-NetIPInterface -InterfaceIndex $idx -AddressFamily IPv4;
$dnsv=Get-DnsClientServerAddress -InterfaceIndex $idx -AddressFamily IPv4 | Select-Object -ExpandProperty ServerAddresses;
$p=Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings';
$wh=((netsh winhttp show proxy) | Out-String);
$bad=@(Get-PnpDevice -Class Net -PresentOnly | Where-Object {$_.Status -eq 'Error' -or $_.Status -eq 'Unknown'} | Select-Object -ExpandProperty FriendlyName);
$threat=@(Get-MpThreat | Select-Object -ExpandProperty ThreatName);
$dnsok=[bool](Resolve-DnsName 'www.baidu.com' -DnsOnly -ErrorAction SilentlyContinue | Select-Object -First 1);
$ipv6d=(Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Services\Tcpip6\Parameters' -Name DisabledComponents -ErrorAction SilentlyContinue).DisabledComponents;
$mac=(Get-NetAdapter -InterfaceIndex $idx).MacAddress;
[PSCustomObject]@{
  admin=[bool]$admin;
  adapters=@($adapters);
  active_index=$idx;
  next_hop=[string]$route.NextHop;
  route_count=$route_count;
  ip=[string]$ipaddr.IPAddress;
  prefix=[int]$ipaddr.PrefixLength;
  gateway=[string]$route.NextHop;
  dns=@($dnsv | Where-Object {$_});
  dhcp=($ipiface.Dhcp -eq 'Enabled');
  mtu=[int]$ipiface.NlMtu;
  ie_proxy_enabled=($p.ProxyEnable -eq 1);
  ie_proxy_server=[string]$p.ProxyServer;
  winhttp_proxy=$wh;
  dns_ok=[bool]$dnsok;
  ipv6_enabled=([int]$ipv6d -eq 0);
  mac=[string]$mac;
  problem_devices=@($bad);
  threat_count=@($threat).Count;
} | ConvertTo-Json -Compress -Depth 4
"#;

    let json = powershell::run_ps_json(script)?;
    parse_probe(&json)
}

fn parse_probe(json: &str) -> Option<NetProbe> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;

    let adapters = v
        .get("adapters")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| serde_json::from_value::<Adapter>(a.clone()).ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let dns = v
        .get("dns")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let problem_devices = v
        .get("problem_devices")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Some(NetProbe {
        is_admin: v.get("admin").and_then(|x| x.as_bool()).unwrap_or(false),
        adapters,
        active_index: v.get("active_index").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        next_hop: v
            .get("next_hop")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        ip: v
            .get("ip")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        prefix_len: v.get("prefix").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        gateway: v
            .get("gateway")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        dns,
        dhcp_enabled: v.get("dhcp").and_then(|x| x.as_bool()).unwrap_or(false),
        mtu: v.get("mtu").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        route_count: v.get("route_count").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        ie_proxy_enabled: v
            .get("ie_proxy_enabled")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        ie_proxy_server: v
            .get("ie_proxy_server")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        winhttp_proxy: v
            .get("winhttp_proxy")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        dns_ok: v.get("dns_ok").and_then(|x| x.as_bool()).unwrap_or(false),
        ipv6_enabled: v
            .get("ipv6_enabled")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        mac: v
            .get("mac")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        problem_devices,
        threat_count: v.get("threat_count").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal() {
        let json = r#"{"admin":true,"adapters":[],"active_index":9,"next_hop":"1.2.3.4","ip":"192.168.1.1","prefix":24,"gateway":"1.2.3.4","dns":["223.5.5.5"],"dhcp":true,"mtu":1500,"ie_proxy_enabled":false,"ie_proxy_server":"","winhttp_proxy":"direct","problem_devices":[],"threat_count":0}"#;
        let p = parse_probe(json).unwrap();
        assert!(p.is_admin);
        assert_eq!(p.active_index, 9);
        assert_eq!(p.dns.len(), 1);
        assert_eq!(p.mtu, 1500);
    }
}
