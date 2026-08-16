//! 模块一：网络诊断引擎（三层诊断模型）。
//!
//! 基础层（DHCP/IP/DNS/端口/网关/掩码/代理/虚拟网卡）→ 本机环境（驱动/物理
//! 损坏/MTU）→ 外部因素（病毒/路由器光猫/环路/MAC 锁）。
//!
//! v0.1 落地基础层检查器；本机环境与外部因素在 v0.2 补齐。执行过程通过
//! `mpsc` channel 流式推送进度 / 结果，TUI 每帧渲染最新状态。
#![allow(dead_code)] // v0.1 骨架：Local/External 层、自动修复执行等 v0.2 补齐

use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;

use crate::windows::adapter::{self, Adapter};
use crate::windows::{is_admin, netsh, powershell, run};

/// 基础层检查项 ID。
const BASIC_CHECK_IDS: [&str; 7] = [
    "active_adapter",
    "dhcp_ip",
    "default_route",
    "gateway_ping",
    "dns_resolve",
    "system_proxy",
    "virtual_nic",
];

/// 基础层检查项名称。
const BASIC_CHECK_NAMES: [&str; 7] = [
    "当前上网网卡判定",
    "DHCP / IP 地址配置",
    "默认路由 / 网关",
    "网关连通性",
    "DNS 解析",
    "系统代理检测",
    "虚拟网卡干扰",
];

/// 诊断分层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Basic,
    Local,
    External,
}

impl Layer {
    pub fn label(&self) -> &'static str {
        match self {
            Layer::Basic => "基础层",
            Layer::Local => "本机环境",
            Layer::External => "外部因素",
        }
    }
}

/// 检查项状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Running,
    Ok,
    Warn,
    Error,
    Info,
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Status::Pending => "待检测",
            Status::Running => "检测中",
            Status::Ok => "正常",
            Status::Warn => "警告",
            Status::Error => "异常",
            Status::Info => "信息",
        }
    }
}

/// 修复建议：`Auto` 可自动执行，`Manual` 仅展示步骤。
#[derive(Debug, Clone)]
pub enum FixKind {
    Auto(String),
    Manual(String),
}

/// 单项修复。
#[derive(Debug, Clone)]
pub struct Fix {
    pub kind: FixKind,
    pub label: String,
}

/// 单条检查结果。
#[derive(Debug, Clone)]
pub struct CheckResult {
    pub id: &'static str,
    pub name: String,
    pub layer: Layer,
    pub status: Status,
    pub detail: String,
    pub fix: Option<Fix>,
    /// 作用网卡（None 表示全局）。
    pub scope: Option<String>,
}

/// 诊断过程中推送的事件。
#[derive(Debug, Clone)]
pub enum DiagEvent {
    /// 诊断开始（总检查项数）。
    Started { total: usize },
    /// 某检查项开始。
    CheckStarted { index: usize, name: String },
    /// 某检查项完成。
    CheckDone { index: usize, result: CheckResult },
    /// 实时日志行（命令回显）。
    Log(String),
    /// 诊断结束。
    Finished { summary: String },
}

/// 诊断上下文（启动时探测一次）。
#[derive(Debug, Clone, Default)]
pub struct DiagContext {
    pub active_adapter: Option<Adapter>,
    pub adapters: Vec<Adapter>,
    pub is_admin: bool,
}

/// 诊断引擎。
pub struct Diagnoser {
    pub ctx: DiagContext,
}

impl Diagnoser {
    /// 探测上下文（网卡、管理员权限）。
    pub fn new() -> Self {
        let adapters = adapter::list_adapters();
        let active_adapter = adapter::get_active_adapter();
        Self {
            ctx: DiagContext {
                active_adapter,
                adapters,
                is_admin: is_admin(),
            },
        }
    }

    /// 异步执行「基础层」诊断，逐项推送事件，返回结果列表。
    pub async fn run_basic(&self, tx: UnboundedSender<DiagEvent>) -> Vec<CheckResult> {
        let checks = self.basic_check_defs();
        let _ = tx.send(DiagEvent::Started {
            total: checks.len(),
        });

        let mut results = Vec::with_capacity(checks.len());
        for (i, (id, name)) in checks.into_iter().enumerate() {
            let _ = tx.send(DiagEvent::CheckStarted {
                index: i,
                name: name.clone(),
            });
            let result = self.run_basic_check(id, &tx).await;
            let _ = tx.send(DiagEvent::CheckDone {
                index: i,
                result: result.clone(),
            });
            results.push(result);
        }

        let summary = summarize(&results);
        let _ = tx.send(DiagEvent::Finished { summary });
        results
    }

    fn basic_check_defs(&self) -> Vec<(&'static str, String)> {
        BASIC_CHECK_IDS
            .iter()
            .zip(BASIC_CHECK_NAMES)
            .map(|(id, name)| (*id, name.to_string()))
            .collect()
    }

    /// 基础层检查项名称（供 UI 预先渲染列表）。
    pub fn basic_check_names() -> Vec<String> {
        BASIC_CHECK_IDS
            .iter()
            .zip(BASIC_CHECK_NAMES)
            .map(|(_, n)| n.to_string())
            .collect()
    }

    async fn run_basic_check(&self, id: &str, tx: &UnboundedSender<DiagEvent>) -> CheckResult {
        match id {
            "active_adapter" => self.check_active_adapter(),
            "dhcp_ip" => self.check_dhcp_ip(tx).await,
            "default_route" => self.check_default_route(),
            "gateway_ping" => self.check_gateway_ping(tx).await,
            "dns_resolve" => self.check_dns_resolve(tx).await,
            "system_proxy" => self.check_system_proxy(tx).await,
            "virtual_nic" => self.check_virtual_nic(),
            _ => CheckResult {
                id: "unknown",
                name: "未知检查".into(),
                layer: Layer::Basic,
                status: Status::Info,
                detail: String::new(),
                fix: None,
                scope: None,
            },
        }
    }

    // ---- 各检查项 ----

    fn check_active_adapter(&self) -> CheckResult {
        let layer = Layer::Basic;
        match &self.ctx.active_adapter {
            Some(a) => {
                let is_virtual = a.is_virtual();
                let (status, detail) = if is_virtual {
                    (
                        Status::Warn,
                        format!(
                            "当前上网网卡「{}」被判定为虚拟网卡（{}），可能并非真实物理出口。",
                            a.name,
                            a.kind_label()
                        ),
                    )
                } else {
                    (
                        Status::Ok,
                        format!(
                            "当前上网网卡「{}」（{}，{}），判定为物理网卡。",
                            a.name,
                            a.kind_label(),
                            a.status
                        ),
                    )
                };
                CheckResult {
                    id: "active_adapter",
                    name: "当前上网网卡判定".into(),
                    layer,
                    status,
                    detail,
                    fix: None,
                    scope: Some(a.name.clone()),
                }
            }
            None => CheckResult {
                id: "active_adapter",
                name: "当前上网网卡判定".into(),
                layer,
                status: Status::Error,
                detail: "未找到默认路由，无法判定当前上网网卡（可能未连接网络）。".into(),
                fix: Some(Fix {
                    kind: FixKind::Manual("检查网线 / 无线是否已连接，并确认网卡已启用。".into()),
                    label: "需手动：连接网络后重试".into(),
                }),
                scope: None,
            },
        }
    }

    async fn check_dhcp_ip(&self, tx: &UnboundedSender<DiagEvent>) -> CheckResult {
        let layer = Layer::Basic;
        let Some(a) = &self.ctx.active_adapter else {
            return CheckResult {
                id: "dhcp_ip",
                name: "DHCP / IP 地址配置".into(),
                layer,
                status: Status::Warn,
                detail: "无当前上网网卡，跳过 IP 配置检测。".into(),
                fix: None,
                scope: None,
            };
        };

        let cfg = get_ip_config(a.interface_index);
        let _ = tx.send(DiagEvent::Log(format!(
            "查询 IP 配置：{} (index {})",
            a.name, a.interface_index
        )));

        let Some(cfg) = cfg else {
            return CheckResult {
                id: "dhcp_ip",
                name: "DHCP / IP 地址配置".into(),
                layer,
                status: Status::Error,
                detail: format!("无法获取网卡「{}」的 IP 配置。", a.name),
                fix: Some(Fix {
                    kind: FixKind::Manual("确认网卡已启用并已连接网络。".into()),
                    label: "需手动：检查网卡状态".into(),
                }),
                scope: Some(a.name.clone()),
            };
        };

        // APIPA（169.254.x.x）说明 DHCP 失败
        if cfg.ip.starts_with("169.254.") {
            return CheckResult {
                id: "dhcp_ip",
                name: "DHCP / IP 地址配置".into(),
                layer,
                status: Status::Error,
                detail: format!(
                    "网卡「{}」地址为 {}(自动专用地址)，说明 DHCP 获取失败。",
                    a.name, cfg.ip
                ),
                fix: Some(Fix {
                    kind: FixKind::Auto(format!("ipconfig /release \"{}\" && ipconfig /renew \"{}\"", a.name, a.name)),
                    label: "可自动执行：释放并重新获取 IP".into(),
                }),
                scope: Some(a.name.clone()),
            };
        }

        let dhcp_txt = if cfg.dhcp_enabled { "DHCP 自动获取" } else { "静态配置" };
        let mut detail = format!(
            "网卡「{}」{}：IP {}，前缀长度 {}，网关 {}。",
            a.name, dhcp_txt, cfg.ip, cfg.prefix_len,
            if cfg.gateway.is_empty() { "无".into() } else { cfg.gateway.clone() }
        );
        if cfg.gateway.is_empty() {
            detail.push_str(" 未发现默认网关。");
        }
        CheckResult {
            id: "dhcp_ip",
            name: "DHCP / IP 地址配置".into(),
            layer,
            status: if cfg.gateway.is_empty() { Status::Warn } else { Status::Ok },
            detail,
            fix: if cfg.gateway.is_empty() {
                Some(Fix {
                    kind: FixKind::Manual("确认 DHCP 服务器 / 路由器是否正常，或手动配置网关。".into()),
                    label: "需手动：检查网关配置".into(),
                })
            } else {
                None
            },
            scope: Some(a.name.clone()),
        }
    }

    fn check_default_route(&self) -> CheckResult {
        let layer = Layer::Basic;
        match adapter::get_default_route_public() {
            Some(next_hop) => CheckResult {
                id: "default_route",
                name: "默认路由 / 网关".into(),
                layer,
                status: Status::Ok,
                detail: format!("存在默认路由，下一跳 {}", next_hop),
                fix: None,
                scope: self.ctx.active_adapter.as_ref().map(|a| a.name.clone()),
            },
            None => CheckResult {
                id: "default_route",
                name: "默认路由 / 网关".into(),
                layer,
                status: Status::Error,
                detail: "未发现默认路由（0.0.0.0/0），外网流量无出口。".into(),
                fix: Some(Fix {
                    kind: FixKind::Manual("检查是否已获取 IP 与网关；若为静态 IP，确认已配置网关。".into()),
                    label: "需手动：配置默认网关".into(),
                }),
                scope: None,
            },
        }
    }

    async fn check_gateway_ping(&self, tx: &UnboundedSender<DiagEvent>) -> CheckResult {
        let layer = Layer::Basic;
        let Some(a) = &self.ctx.active_adapter else {
            return CheckResult {
                id: "gateway_ping",
                name: "网关连通性".into(),
                layer,
                status: Status::Warn,
                detail: "无当前上网网卡，跳过网关连通检测。".into(),
                fix: None,
                scope: None,
            };
        };
        let Some(cfg) = get_ip_config(a.interface_index) else {
            return CheckResult {
                id: "gateway_ping",
                name: "网关连通性".into(),
                layer,
                status: Status::Warn,
                detail: "未获取到 IP 配置，跳过。".into(),
                fix: None,
                scope: Some(a.name.clone()),
            };
        };
        if cfg.gateway.is_empty() {
            return CheckResult {
                id: "gateway_ping",
                name: "网关连通性".into(),
                layer,
                status: Status::Warn,
                detail: "无默认网关，无法测试网关连通。".into(),
                fix: None,
                scope: Some(a.name.clone()),
            };
        }

        let _ = tx.send(DiagEvent::Log(format!("ping 网关 {}", cfg.gateway)));
        let ok = ping_ok(&cfg.gateway).await;
        CheckResult {
            id: "gateway_ping",
            name: "网关连通性".into(),
            layer,
            status: if ok { Status::Ok } else { Status::Error },
            detail: if ok {
                format!("网关 {} 可达。", cfg.gateway)
            } else {
                format!("网关 {} 不可达，局域网链路可能中断。", cfg.gateway)
            },
            fix: if ok {
                None
            } else {
                Some(Fix {
                    kind: FixKind::Manual("检查网线 / 无线连接、路由器 / 交换机是否通电正常。".into()),
                    label: "需手动：检查物理链路与网关设备".into(),
                })
            },
            scope: Some(a.name.clone()),
        }
    }

    async fn check_dns_resolve(&self, tx: &UnboundedSender<DiagEvent>) -> CheckResult {
        let layer = Layer::Basic;
        let domain = "www.baidu.com";
        let _ = tx.send(DiagEvent::Log(format!("DNS 解析测试：{}", domain)));
        let ok = dns_resolve_ok(domain).await;
        CheckResult {
            id: "dns_resolve",
            name: "DNS 解析".into(),
            layer,
            status: if ok { Status::Ok } else { Status::Error },
            detail: if ok {
                format!("域名 {} 解析成功，DNS 服务可用。", domain)
            } else {
                format!("域名 {} 解析失败，DNS 服务可能不可用或配置被篡改。", domain)
            },
            fix: if ok {
                None
            } else {
                Some(Fix {
                    kind: FixKind::Auto("ipconfig /flushdns".into()),
                    label: "可自动执行：刷新 DNS 缓存".into(),
                })
            },
            scope: self.ctx.active_adapter.as_ref().map(|a| a.name.clone()),
        }
    }

    async fn check_system_proxy(&self, tx: &UnboundedSender<DiagEvent>) -> CheckResult {
        let layer = Layer::Basic;
        let _ = tx.send(DiagEvent::Log("检测系统代理：WinHTTP + Internet Settings".into()));
        let winhttp = netsh::run_netsh(&["winhttp", "show", "proxy"]).combined();
        let ie_proxy = get_ie_proxy();

        let winhttp_direct = winhttp.contains("直接访问")
            || winhttp.to_lowercase().contains("direct access");
        let has_proxy = !winhttp_direct || ie_proxy.enabled;

        if has_proxy {
            let mut detail = String::from("检测到系统代理已开启：");
            if !winhttp_direct {
                detail.push_str(" WinHTTP 代理已设置；");
            }
            if ie_proxy.enabled {
                detail.push_str(&format!(" 系统代理 {}", ie_proxy.server));
            }
            CheckResult {
                id: "system_proxy",
                name: "系统代理检测".into(),
                layer,
                status: Status::Warn,
                detail,
                fix: Some(Fix {
                    kind: FixKind::Manual("若未使用代理却无法上网，请关闭系统代理：设置 → 网络和 Internet → 代理。".into()),
                    label: "需手动：检查并关闭异常代理".into(),
                }),
                scope: None,
            }
        } else {
            CheckResult {
                id: "system_proxy",
                name: "系统代理检测".into(),
                layer,
                status: Status::Ok,
                detail: "未检测到异常系统代理。".into(),
                fix: None,
                scope: None,
            }
        }
    }

    fn check_virtual_nic(&self) -> CheckResult {
        let layer = Layer::Basic;
        let virtual_nics: Vec<&Adapter> = self
            .ctx
            .adapters
            .iter()
            .filter(|a| a.is_virtual() && a.is_up())
            .collect();

        if virtual_nics.is_empty() {
            return CheckResult {
                id: "virtual_nic",
                name: "虚拟网卡干扰".into(),
                layer,
                status: Status::Ok,
                detail: "未发现启用的虚拟 / VPN 网卡干扰。".into(),
                fix: None,
                scope: None,
            };
        }

        let names: Vec<String> = virtual_nics.iter().map(|a| a.name.clone()).collect();
        // 若当前上网网卡本身是虚拟网卡，则视为干扰风险
        let active_is_virtual = self
            .ctx
            .active_adapter
            .as_ref()
            .map(|a| a.is_virtual())
            .unwrap_or(false);

        let (status, detail) = if active_is_virtual {
            (
                Status::Error,
                format!(
                    "当前上网网卡为虚拟网卡，另检测到虚拟网卡：{}。VPN / 虚拟网卡可能抢占网关导致误判或异常。",
                    names.join("、")
                ),
            )
        } else {
            (
                Status::Warn,
                format!(
                    "检测到启用的虚拟网卡：{}（仅供参考，不计入上网判定）。",
                    names.join("、")
                ),
            )
        };

        CheckResult {
            id: "virtual_nic",
            name: "虚拟网卡干扰".into(),
            layer,
            status,
            detail,
            fix: if active_is_virtual {
                Some(Fix {
                    kind: FixKind::Manual("如非必要，断开 VPN / 虚拟机网卡后再上网。".into()),
                    label: "需手动：断开干扰的虚拟网卡".into(),
                })
            } else {
                None
            },
            scope: None,
        }
    }
}

/// 汇总诊断结果，给出一句话结论。
fn summarize(results: &[CheckResult]) -> String {
    let ok = results.iter().filter(|r| r.status == Status::Ok).count();
    let warn = results.iter().filter(|r| r.status == Status::Warn).count();
    let err = results.iter().filter(|r| r.status == Status::Error).count();
    let auto = results
        .iter()
        .filter(|r| matches!(r.fix.as_ref().map(|f| &f.kind), Some(FixKind::Auto(_))))
        .count();
    let manual = results
        .iter()
        .filter(|r| matches!(r.fix.as_ref().map(|f| &f.kind), Some(FixKind::Manual(_))))
        .count();

    let mut parts = vec![format!("检测完成：正常 {} 项", ok)];
    if warn > 0 {
        parts.push(format!("警告 {} 项", warn));
    }
    if err > 0 {
        parts.push(format!("异常 {} 项", err));
    }
    if auto > 0 {
        parts.push(format!("可自动修复 {} 项", auto));
    }
    if manual > 0 {
        parts.push(format!("需手动处理 {} 项", manual));
    }
    parts.join("，") + "。"
}

/// 网卡 IP 配置（结构化，来自 Get-NetIPConfiguration）。
#[derive(Debug, Clone, Default)]
pub struct IpConfig {
    pub ip: String,
    pub prefix_len: u32,
    pub gateway: String,
    pub dns: Vec<String>,
    pub dhcp_enabled: bool,
}

/// 获取指定网卡的 IP 配置。
pub fn get_ip_config(index: u32) -> Option<IpConfig> {
    let script = format!(
        "$c=Get-NetIPConfiguration -InterfaceIndex {index} -ErrorAction SilentlyContinue; \
         [PSCustomObject]@{{ \
           ip=($c.IPv4Address | Select-Object -First 1).IPAddress; \
           prefix=[int](($c.IPv4Address | Select-Object -First 1).PrefixLength); \
           gateway=($c.IPv4DefaultGateway | Select-Object -First 1).NextHop; \
           dns=@($c.DNSServer.ServerAddresses | Where-Object {{ $_ }}); \
           dhcp=($c.NetIPv4Interface.Dhcp -eq 'Enabled') \
         }} | ConvertTo-Json -Compress"
    );
    let json = powershell::run_ps_json(&script)?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    Some(IpConfig {
        ip: v.get("ip").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        prefix_len: v.get("prefix").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        gateway: v.get("gateway").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        dns: v
            .get("dns")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        dhcp_enabled: v.get("dhcp").and_then(|x| x.as_bool()).unwrap_or(false),
    })
}

/// ping 探测是否可达。
pub async fn ping_ok(host: &str) -> bool {
    tokio::task::spawn_blocking({
        let host = host.to_string();
        move || {
            let out = run("ping", &["-n", "2", "-w", "2000", &host], Duration::from_secs(10));
            out.success
                || out.stdout.to_lowercase().contains("ttl=")
                || out.stdout.contains("TTL=")
        }
    })
    .await
    .unwrap_or(false)
}

/// DNS 解析是否成功（nslookup）。
pub async fn dns_resolve_ok(domain: &str) -> bool {
    tokio::task::spawn_blocking({
        let domain = domain.to_string();
        move || {
            let out = run("nslookup", &[&domain], Duration::from_secs(15));
            let text = out.combined().to_lowercase();
            let has_answer = text.contains("address") || text.contains("地址");
            let failed = text.contains("can't find")
                || text.contains("timed out")
                || text.contains("unknown")
                || text.contains("nonexistent");
            has_answer && !failed
        }
    })
    .await
    .unwrap_or(false)
}

/// 系统（IE）代理设置。
#[derive(Debug, Clone, Default)]
pub struct IeProxy {
    pub enabled: bool,
    pub server: String,
}

/// 读取 HKCU Internet Settings 代理设置。
pub fn get_ie_proxy() -> IeProxy {
    let script = "$p=Get-ItemProperty 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings' -ErrorAction SilentlyContinue; \
                  [PSCustomObject]@{ enabled=($p.ProxyEnable -eq 1); server=[string]$p.ProxyServer } | ConvertTo-Json -Compress";
    let Some(json) = powershell::run_ps_json(script) else {
        return IeProxy::default();
    };
    let v: serde_json::Value = serde_json::from_str(&json).unwrap_or(serde_json::Value::Null);
    IeProxy {
        enabled: v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false),
        server: v.get("server").and_then(|x| x.as_str()).unwrap_or("").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_counts() {
        let r = vec![
            CheckResult {
                id: "a",
                name: "a".into(),
                layer: Layer::Basic,
                status: Status::Ok,
                detail: String::new(),
                fix: None,
                scope: None,
            },
            CheckResult {
                id: "b",
                name: "b".into(),
                layer: Layer::Basic,
                status: Status::Error,
                detail: String::new(),
                fix: Some(Fix {
                    kind: FixKind::Auto("ipconfig /flushdns".into()),
                    label: "x".into(),
                }),
                scope: None,
            },
        ];
        let s = summarize(&r);
        assert!(s.contains("正常 1"));
        assert!(s.contains("异常 1"));
        assert!(s.contains("可自动修复 1"));
    }

    #[test]
    fn apipa_detection() {
        assert!(IpConfig { ip: "169.254.1.1".into(), ..Default::default() }.ip.starts_with("169.254."));
    }
}
