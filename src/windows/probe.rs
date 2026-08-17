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
    /// IPv6 是否启用（网卡绑定 ms_tcpip6 是否 Enabled，比注册表 DisabledComponents 更准确）。
    pub ipv6_enabled: bool,
    /// 当前上网网卡的 MAC 地址（模块二只读展示）。
    pub mac: String,
    /// 有异常状态的网络类 PnP 设备（驱动缺失/错误）。
    pub problem_devices: Vec<String>,
    /// Windows Defender 检测到的威胁数。
    pub threat_count: u32,
    // ---- IPv6 明细（模块二 IPv6 字段展示 / 静态可编辑）----
    /// IPv6 全局单播地址（首选、非 WellKnown）。
    pub ipv6_addr: String,
    /// IPv6 前缀长度。
    pub ipv6_prefix: u32,
    /// IPv6 默认网关（::/0 下一跳）。
    pub ipv6_gateway: String,
    /// IPv6 DNS 服务器列表。
    pub ipv6_dns: Vec<String>,
    /// IPv6 地址是否手动配置（静态，否则 SLAAC/DHCPv6 自动获取）。
    pub ipv6_static: bool,
}

impl NetProbe {
    /// 当前上网网卡。
    pub fn active_adapter(&self) -> Option<&Adapter> {
        self.adapters
            .iter()
            .find(|a| a.interface_index == self.active_index)
    }
}

/// 执行一次合并探测（自动判定当前上网网卡）。
pub fn probe_network() -> Option<NetProbe> {
    let script = build_script(None);
    let json = powershell::run_ps_json(&script)?;
    parse_probe(&json)
}

/// 对指定网卡执行探测（用户手动切换网卡时调用）。
pub fn probe_network_for(interface_index: u32) -> Option<NetProbe> {
    let script = build_script(Some(interface_index));
    let json = powershell::run_ps_json(&script)?;
    parse_probe(&json)
}

/// 探测脚本模板：用 `__IDX_SETUP__` 占位符区分「自动判定上网网卡」与「指定网卡」。
const PROBE_SCRIPT: &str = r#"
$ErrorActionPreference='SilentlyContinue';
__IDX_SETUP__
$adapters=Get-NetAdapter | Select-Object InterfaceIndex,Name,InterfaceDescription,@{N='InterfaceType';E={[string]$_.InterfaceType}},Status,@{N='HardwareInterface';E={[bool]$_.HardwareInterface}};
$ipaddr=Get-NetIPAddress -InterfaceIndex $idx -AddressFamily IPv4 | Where-Object {$_.IPAddress -notlike '169.254*'} | Select-Object -First 1;
$ipiface=Get-NetIPInterface -InterfaceIndex $idx -AddressFamily IPv4;
$dnsv=Get-DnsClientServerAddress -InterfaceIndex $idx -AddressFamily IPv4 | Select-Object -ExpandProperty ServerAddresses;
$gateway=(Get-NetRoute -InterfaceIndex $idx -DestinationPrefix '0.0.0.0/0' | Select-Object -First 1).NextHop;
$p=Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings';
$wh=((netsh winhttp show proxy) | Out-String);
$bad=@(Get-PnpDevice -Class Net -PresentOnly | Where-Object {$_.Status -eq 'Error' -or $_.Status -eq 'Unknown'} | Select-Object -ExpandProperty FriendlyName);
$threat=@(Get-MpThreat | Select-Object -ExpandProperty ThreatName);
$dnsok=[bool](Resolve-DnsName 'www.baidu.com' -DnsOnly -ErrorAction SilentlyContinue | Select-Object -First 1);
$mac=(Get-NetAdapter -InterfaceIndex $idx).MacAddress;
$ipv6a=@(Get-NetIPAddress -InterfaceIndex $idx -AddressFamily IPv6 -ErrorAction SilentlyContinue | Where-Object {$_.PrefixOrigin -ne 'WellKnown'});
$ipv6addr=$ipv6a | Where-Object {$_.AddressState -eq 'Preferred'} | Select-Object -First 1;
$ipv6gw=(Get-NetRoute -InterfaceIndex $idx -AddressFamily IPv6 -DestinationPrefix '::/0' -ErrorAction SilentlyContinue | Select-Object -First 1).NextHop;
$ipv6dns=@(Get-DnsClientServerAddress -InterfaceIndex $idx -AddressFamily IPv6 -ErrorAction SilentlyContinue | Select-Object -ExpandProperty ServerAddresses);
$ipv6static=if($ipv6addr){ ($ipv6addr.PrefixOrigin -eq 'Manual') -or ($ipv6addr.SuffixOrigin -eq 'Manual') }else{$false};
$ipv6bind=(Get-NetAdapterBinding -InterfaceIndex $idx -ComponentID ms_tcpip6 -ErrorAction SilentlyContinue).Enabled;
$ipv6reg=(Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Services\Tcpip6\Parameters' -Name DisabledComponents -ErrorAction SilentlyContinue).DisabledComponents;
$regFullyDisabled=(([int]$ipv6reg) -band 0xFF) -eq 0xFF;
$ipv6_enabled=if($null -eq $ipv6bind){ -not $regFullyDisabled }else{ ([bool]$ipv6bind) -and (-not $regFullyDisabled) };
[PSCustomObject]@{
  admin=[bool]$admin;
  adapters=@($adapters);
  active_index=$idx;
  next_hop=[string]$gateway;
  route_count=$route_count;
  ip=[string]$ipaddr.IPAddress;
  prefix=[int]$ipaddr.PrefixLength;
  gateway=[string]$gateway;
  dns=@($dnsv | Where-Object {$_});
  dhcp=($ipiface.Dhcp -eq 'Enabled');
  mtu=[int]$ipiface.NlMtu;
  ie_proxy_enabled=($p.ProxyEnable -eq 1);
  ie_proxy_server=[string]$p.ProxyServer;
  winhttp_proxy=$wh;
  dns_ok=[bool]$dnsok;
  ipv6_addr=[string]$ipv6addr.IPAddress;
  ipv6_prefix=[int]($ipv6addr.PrefixLength);
  ipv6_gateway=[string]$ipv6gw;
  ipv6_dns=@($ipv6dns | Where-Object {$_});
  ipv6_static=[bool]$ipv6static;
  ipv6_enabled=[bool]$ipv6_enabled;
  mac=[string]$mac;
  problem_devices=@($bad);
  threat_count=@($threat).Count;
} | ConvertTo-Json -Compress -Depth 4
"#;

/// 构建探测脚本：forced 为 None 时自动按默认路由判定上网网卡，
/// 为 Some(idx) 时直接探测指定网卡。
fn build_script(forced: Option<u32>) -> String {
    let idx_setup = match forced {
        Some(i) => format!("$idx={i};$route_count=0;"),
        None => "$routes=@(Get-NetRoute -DestinationPrefix '0.0.0.0/0' | Sort-Object RouteMetric,InterfaceMetric);$route=$routes|Select-Object -First 1;$idx=$route.InterfaceIndex;$route_count=$routes.Count;".to_string(),
    };
    PROBE_SCRIPT.replace("__IDX_SETUP__", &idx_setup)
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

    let ipv6_dns = v
        .get("ipv6_dns")
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
        ipv6_addr: v
            .get("ipv6_addr")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        ipv6_prefix: v
            .get("ipv6_prefix")
            .and_then(|x| x.as_u64())
            .unwrap_or(0) as u32,
        ipv6_gateway: v
            .get("ipv6_gateway")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        ipv6_dns,
        ipv6_static: v
            .get("ipv6_static")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal() {
        let json = r#"{"admin":true,"adapters":[],"active_index":9,"next_hop":"1.2.3.4","ip":"192.168.1.1","prefix":24,"gateway":"1.2.3.4","dns":["223.5.5.5"],"dhcp":true,"mtu":1500,"ie_proxy_enabled":false,"ie_proxy_server":"","winhttp_proxy":"direct","ipv6_addr":"","ipv6_prefix":0,"ipv6_gateway":"","ipv6_dns":[],"ipv6_static":false,"ipv6_enabled":true,"problem_devices":[],"threat_count":0}"#;
        let p = parse_probe(json).unwrap();
        assert!(p.is_admin);
        assert_eq!(p.active_index, 9);
        assert_eq!(p.dns.len(), 1);
        assert_eq!(p.mtu, 1500);
        assert!(p.ipv6_enabled);
    }
}
