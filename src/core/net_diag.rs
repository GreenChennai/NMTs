//! 模块一：网络诊断引擎（三层诊断模型）。
//!
//! 基础层（DHCP/IP/DNS/网关/掩码/代理/虚拟网卡）→ 本机环境层（驱动/物理
//! 链路/MTU）→ 外部因素层（病毒/路由器光猫/环路/MAC 锁）。
//!
//! v0.2 落地三层全部检查器。静态数据由 `windows::probe::probe_network()`
//! 一次 PowerShell 调用拿全，动态检测（ping 网关 / ping 公网 / DNS 解析）并发
//! 执行，避免多次子进程冷启动导致「启动慢 / 诊断慢」。执行过程通过 `mpsc`
//! channel 流式推送进度 / 结果，TUI 每帧渲染。
#![allow(dead_code)] // Layer/Status::Pending/FixKind 命令串等供后续 UI 与模块二使用

use tokio::sync::mpsc::UnboundedSender;

use crate::windows::probe::{probe_network, NetProbe};
use crate::windows::{netsh, run};

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
    Started { total: usize },
    CheckStarted { index: usize, name: String },
    CheckDone { index: usize, result: CheckResult },
    Log(String),
    Finished { summary: String },
}

/// 一次诊断的动态检测结果（并发执行，避免串行等待）。
#[derive(Debug, Clone, Default)]
struct DynResults {
    gateway_ping: bool,
    wan_ping: bool,
}

/// 检查项定义（id, name, layer）。
const CHECKS: [(&str, &str, Layer); 14] = [
    // 基础层
    ("active_adapter", "当前上网网卡判定", Layer::Basic),
    ("dhcp_ip", "DHCP / IP 地址配置", Layer::Basic),
    ("default_route", "默认路由 / 网关", Layer::Basic),
    ("gateway_ping", "网关连通性", Layer::Basic),
    ("dns_resolve", "DNS 解析", Layer::Basic),
    ("system_proxy", "系统代理检测", Layer::Basic),
    ("virtual_nic", "虚拟网卡干扰", Layer::Basic),
    // 本机环境层
    ("driver_health", "网卡驱动状态", Layer::Local),
    ("link_status", "物理链路状态", Layer::Local),
    ("mtu", "MTU 设置", Layer::Local),
    // 外部因素层
    ("wan_connectivity", "外网连通（路由器/光猫）", Layer::External),
    ("threat", "病毒 / 威胁", Layer::External),
    ("loop_risk", "二层环路风险", Layer::External),
    ("mac_lock", "MAC 锁排查", Layer::External),
];

/// 诊断引擎。
pub struct Diagnoser {
    pub ctx: DiagContext,
}

/// 诊断上下文（启动时探测一次）。
#[derive(Debug, Clone, Default)]
pub struct DiagContext {
    pub probe: NetProbe,
}

impl Diagnoser {
    /// 探测上下文（单次 PowerShell 合并查询）。
    pub fn new() -> Self {
        Self {
            ctx: DiagContext {
                probe: probe_network().unwrap_or_default(),
            },
        }
    }

    /// 当前上网网卡。
    pub fn active_adapter(&self) -> Option<&crate::windows::adapter::Adapter> {
        self.ctx.probe.active_adapter()
    }

    /// 三层全部检查项名称（供 UI 预先渲染列表）。
    pub fn all_check_names() -> Vec<String> {
        CHECKS.iter().map(|(_, n, _)| n.to_string()).collect()
    }

    /// 异步执行三层诊断，逐项推送事件，返回结果列表。
    pub async fn run(&self, tx: UnboundedSender<DiagEvent>) -> Vec<CheckResult> {
        let total = CHECKS.len();
        let _ = tx.send(DiagEvent::Started { total });

        // 动态检测并发执行（DNS 解析已合并进探测，这里只并发 ping 网关与公网）
        let gateway = self.ctx.probe.gateway.clone();
        let wan_ip = "223.5.5.5".to_string();

        let _ = tx.send(DiagEvent::Log("并发检测：ping 网关 / ping 公网…".into()));
        let (gw_ping, wan_ping) = tokio::join!(
            ping_ok(&gateway),
            ping_ok(&wan_ip),
        );
        let dyn_results = DynResults {
            gateway_ping: gw_ping,
            wan_ping,
        };

        // 逐项构建结果（基于缓存 + 动态结果）
        let mut results = Vec::with_capacity(total);
        for (i, (id, name, layer)) in CHECKS.iter().enumerate() {
            let _ = tx.send(DiagEvent::CheckStarted {
                index: i,
                name: name.to_string(),
            });
            let result = self.build_check(id, *layer, &dyn_results);
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

    /// 依据检查项 id 构建结果。
    fn build_check(&self, id: &str, layer: Layer, dynr: &DynResults) -> CheckResult {
        let p = &self.ctx.probe;
        let active = p.active_adapter();
        let scope = active.map(|a| a.name.clone());

        match id {
            "active_adapter" => self.check_active_adapter(layer),
            "dhcp_ip" => self.check_dhcp_ip(layer),
            "default_route" => self.check_default_route(layer),
            "gateway_ping" => self.check_gateway_ping(layer, dynr),
            "dns_resolve" => self.check_dns_resolve(layer, dynr),
            "system_proxy" => self.check_system_proxy(layer),
            "virtual_nic" => self.check_virtual_nic(layer),
            "driver_health" => self.check_driver_health(layer),
            "link_status" => self.check_link_status(layer),
            "mtu" => self.check_mtu(layer),
            "wan_connectivity" => self.check_wan(layer, dynr),
            "threat" => self.check_threat(layer),
            "loop_risk" => self.check_loop(layer),
            "mac_lock" => self.check_mac_lock(layer),
            _ => CheckResult {
                id: "unknown",
                name: "未知检查".into(),
                layer,
                status: Status::Info,
                detail: String::new(),
                fix: None,
                scope,
            },
        }
    }

    // ---- 基础层 ----

    fn check_active_adapter(&self, layer: Layer) -> CheckResult {
        match self.active_adapter() {
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
                            "当前上网网卡「{}」（{}，{}）。",
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

    fn check_dhcp_ip(&self, layer: Layer) -> CheckResult {
        let p = &self.ctx.probe;
        let Some(a) = p.active_adapter() else {
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
        let scope = Some(a.name.clone());

        if p.ip.starts_with("169.254.") {
            return CheckResult {
                id: "dhcp_ip",
                name: "DHCP / IP 地址配置".into(),
                layer,
                status: Status::Error,
                detail: format!("网卡「{}」地址为 {}(自动专用地址)，说明 DHCP 获取失败。", a.name, p.ip),
                fix: Some(Fix {
                    kind: FixKind::Auto(format!("ipconfig /release \"{}\" && ipconfig /renew \"{}\"", a.name, a.name)),
                    label: "可自动执行：释放并重新获取 IP".into(),
                }),
                scope,
            };
        }

        let dhcp_txt = if p.dhcp_enabled { "DHCP 自动获取" } else { "静态配置" };
        let gw_txt = if p.gateway.is_empty() { "无".into() } else { p.gateway.clone() };
        let detail = format!(
            "网卡「{}」{}：IP {}，前缀长度 {}，网关 {}。",
            a.name, dhcp_txt, p.ip, p.prefix_len, gw_txt
        );
        CheckResult {
            id: "dhcp_ip",
            name: "DHCP / IP 地址配置".into(),
            layer,
            status: if p.gateway.is_empty() { Status::Warn } else { Status::Ok },
            detail,
            fix: if p.gateway.is_empty() {
                Some(Fix {
                    kind: FixKind::Manual("确认 DHCP 服务器 / 路由器是否正常，或手动配置网关。".into()),
                    label: "需手动：检查网关配置".into(),
                })
            } else {
                None
            },
            scope,
        }
    }

    fn check_default_route(&self, layer: Layer) -> CheckResult {
        let p = &self.ctx.probe;
        if !p.next_hop.is_empty() {
            CheckResult {
                id: "default_route",
                name: "默认路由 / 网关".into(),
                layer,
                status: Status::Ok,
                detail: format!("存在默认路由，下一跳 {}", p.next_hop),
                fix: None,
                scope: p.active_adapter().map(|a| a.name.clone()),
            }
        } else {
            CheckResult {
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
            }
        }
    }

    fn check_gateway_ping(&self, layer: Layer, dynr: &DynResults) -> CheckResult {
        let p = &self.ctx.probe;
        let scope = p.active_adapter().map(|a| a.name.clone());
        if p.gateway.is_empty() {
            return CheckResult {
                id: "gateway_ping",
                name: "网关连通性".into(),
                layer,
                status: Status::Warn,
                detail: "无默认网关，无法测试网关连通。".into(),
                fix: None,
                scope,
            };
        }
        CheckResult {
            id: "gateway_ping",
            name: "网关连通性".into(),
            layer,
            status: if dynr.gateway_ping { Status::Ok } else { Status::Error },
            detail: if dynr.gateway_ping {
                format!("网关 {} 可达。", p.gateway)
            } else {
                format!("网关 {} 不可达，局域网链路可能中断。", p.gateway)
            },
            fix: if dynr.gateway_ping {
                None
            } else {
                Some(Fix {
                    kind: FixKind::Manual("检查网线 / 无线连接、路由器 / 交换机是否通电正常。".into()),
                    label: "需手动：检查物理链路与网关设备".into(),
                })
            },
            scope,
        }
    }

    fn check_dns_resolve(&self, layer: Layer, _dynr: &DynResults) -> CheckResult {
        let p = &self.ctx.probe;
        CheckResult {
            id: "dns_resolve",
            name: "DNS 解析".into(),
            layer,
            status: if p.dns_ok { Status::Ok } else { Status::Error },
            detail: if p.dns_ok {
                "域名 www.baidu.com 解析成功，DNS 服务可用。".into()
            } else {
                "域名解析失败，DNS 服务可能不可用或配置被篡改。".into()
            },
            fix: if p.dns_ok {
                None
            } else {
                Some(Fix {
                    kind: FixKind::Auto("ipconfig /flushdns".into()),
                    label: "可自动执行：刷新 DNS 缓存".into(),
                })
            },
            scope: p.active_adapter().map(|a| a.name.clone()),
        }
    }

    fn check_system_proxy(&self, layer: Layer) -> CheckResult {
        let p = &self.ctx.probe;
        let winhttp_direct = p.winhttp_proxy.contains("直接访问")
            || p.winhttp_proxy.to_lowercase().contains("direct access");
        let has_proxy = !winhttp_direct || p.ie_proxy_enabled;

        if has_proxy {
            let mut detail = String::from("检测到系统代理已开启：");
            if !winhttp_direct {
                detail.push_str(" WinHTTP 代理已设置；");
            }
            if p.ie_proxy_enabled {
                detail.push_str(&format!(" 系统代理 {}", p.ie_proxy_server));
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

    fn check_virtual_nic(&self, layer: Layer) -> CheckResult {
        let p = &self.ctx.probe;
        let virtual_nics: Vec<String> = p
            .adapters
            .iter()
            .filter(|a| a.is_virtual() && a.is_up())
            .map(|a| a.name.clone())
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

        let active_is_virtual = p.active_adapter().map(|a| a.is_virtual()).unwrap_or(false);
        let (status, detail) = if active_is_virtual {
            (
                Status::Error,
                format!(
                    "当前上网网卡为虚拟网卡，另检测到虚拟网卡：{}。VPN / 虚拟网卡可能抢占网关导致异常。",
                    virtual_nics.join("、")
                ),
            )
        } else {
            (
                Status::Warn,
                format!("检测到启用的虚拟网卡：{}（仅供参考，不计入上网判定）。", virtual_nics.join("、")),
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

    // ---- 本机环境层 ----

    fn check_driver_health(&self, layer: Layer) -> CheckResult {
        let p = &self.ctx.probe;
        if p.problem_devices.is_empty() {
            CheckResult {
                id: "driver_health",
                name: "网卡驱动状态".into(),
                layer,
                status: Status::Ok,
                detail: "未发现状态异常的网络设备驱动。".into(),
                fix: None,
                scope: None,
            }
        } else {
            CheckResult {
                id: "driver_health",
                name: "网卡驱动状态".into(),
                layer,
                status: Status::Error,
                detail: format!("发现异常网络设备：{}（驱动缺失或错误）。", p.problem_devices.join("、")),
                fix: Some(Fix {
                    kind: FixKind::Manual("在设备管理器更新 / 重装对应网卡驱动。".into()),
                    label: "需手动：更新网卡驱动".into(),
                }),
                scope: None,
            }
        }
    }

    fn check_link_status(&self, layer: Layer) -> CheckResult {
        let p = &self.ctx.probe;
        match p.active_adapter() {
            Some(a) if a.is_up() => CheckResult {
                id: "link_status",
                name: "物理链路状态".into(),
                layer,
                status: Status::Ok,
                detail: format!("网卡「{}」物理链路已连接（Up）。", a.name),
                fix: None,
                scope: Some(a.name.clone()),
            },
            Some(a) => CheckResult {
                id: "link_status",
                name: "物理链路状态".into(),
                layer,
                status: Status::Error,
                detail: format!("网卡「{}」物理链路断开（{}），可能是网线脱落 / 无线断开 / 网卡损坏。", a.name, a.status),
                fix: Some(Fix {
                    kind: FixKind::Manual("检查网线是否插好、无线是否连接、网卡指示灯是否亮。".into()),
                    label: "需手动：检查物理连接".into(),
                }),
                scope: Some(a.name.clone()),
            },
            None => CheckResult {
                id: "link_status",
                name: "物理链路状态".into(),
                layer,
                status: Status::Warn,
                detail: "无当前上网网卡，跳过物理链路检测。".into(),
                fix: None,
                scope: None,
            },
        }
    }

    fn check_mtu(&self, layer: Layer) -> CheckResult {
        let p = &self.ctx.probe;
        if p.mtu == 0 {
            return CheckResult {
                id: "mtu",
                name: "MTU 设置".into(),
                layer,
                status: Status::Warn,
                detail: "未能读取 MTU。".into(),
                fix: None,
                scope: None,
            };
        }
        if p.mtu < 1280 {
            CheckResult {
                id: "mtu",
                name: "MTU 设置".into(),
                layer,
                status: Status::Warn,
                detail: format!("当前 MTU 为 {}，过小可能导致分片 / 丢包。", p.mtu),
                fix: Some(Fix {
                    kind: FixKind::Manual("建议将 MTU 恢复为 1500（以太网标准）。".into()),
                    label: "需手动：调整 MTU".into(),
                }),
                scope: p.active_adapter().map(|a| a.name.clone()),
            }
        } else {
            CheckResult {
                id: "mtu",
                name: "MTU 设置".into(),
                layer,
                status: Status::Ok,
                detail: format!("当前 MTU {}，正常。", p.mtu),
                fix: None,
                scope: p.active_adapter().map(|a| a.name.clone()),
            }
        }
    }

    // ---- 外部因素层 ----

    fn check_wan(&self, layer: Layer, dynr: &DynResults) -> CheckResult {
        // 网关通但公网不通 → 路由器/光猫/运营商问题；网关也不通则问题在局域网。
        if dynr.wan_ping {
            CheckResult {
                id: "wan_connectivity",
                name: "外网连通（路由器/光猫）".into(),
                layer,
                status: Status::Ok,
                detail: "公网地址 223.5.5.5 可达，出口链路正常。".into(),
                fix: None,
                scope: None,
            }
        } else if dynr.gateway_ping {
            CheckResult {
                id: "wan_connectivity",
                name: "外网连通（路由器/光猫）".into(),
                layer,
                status: Status::Error,
                detail: "局域网可达但公网不通，问题可能在路由器 / 光猫 / 运营商链路。".into(),
                fix: Some(Fix {
                    kind: FixKind::Manual("重启路由器 / 光猫；仍不通则联系运营商（宽带欠费 / 线路故障）。".into()),
                    label: "需手动：重启光猫路由器或联系运营商".into(),
                }),
                scope: None,
            }
        } else {
            CheckResult {
                id: "wan_connectivity",
                name: "外网连通（路由器/光猫）".into(),
                layer,
                status: Status::Warn,
                detail: "局域网与公网均不可达，先排查本机与局域网链路。".into(),
                fix: None,
                scope: None,
            }
        }
    }

    fn check_threat(&self, layer: Layer) -> CheckResult {
        let p = &self.ctx.probe;
        if p.threat_count > 0 {
            CheckResult {
                id: "threat",
                name: "病毒 / 威胁".into(),
                layer,
                status: Status::Error,
                detail: format!("Windows Defender 检测到 {} 个威胁，可能导致网络异常。", p.threat_count),
                fix: Some(Fix {
                    kind: FixKind::Manual("运行 Windows 安全中心全盘扫描并清除威胁。".into()),
                    label: "需手动：查杀病毒".into(),
                }),
                scope: None,
            }
        } else {
            CheckResult {
                id: "threat",
                name: "病毒 / 威胁".into(),
                layer,
                status: Status::Ok,
                detail: "未检测到活跃威胁。".into(),
                fix: None,
                scope: None,
            }
        }
    }

    fn check_loop(&self, layer: Layer) -> CheckResult {
        let p = &self.ctx.probe;
        if p.route_count > 1 {
            CheckResult {
                id: "loop_risk",
                name: "二层环路风险".into(),
                layer,
                status: Status::Warn,
                detail: format!("检测到 {} 条默认路由（多出口），若伴随网络卡顿需排查环路 / 冗余链路。", p.route_count),
                fix: Some(Fix {
                    kind: FixKind::Manual("检查交换机是否成环；冗余链路需启用生成树协议（STP）。".into()),
                    label: "需手动：排查环路 / 启用 STP".into(),
                }),
                scope: None,
            }
        } else {
            CheckResult {
                id: "loop_risk",
                name: "二层环路风险".into(),
                layer,
                status: Status::Ok,
                detail: "未发现明显环路 / 多出口特征。".into(),
                fix: None,
                scope: None,
            }
        }
    }

    fn check_mac_lock(&self, layer: Layer) -> CheckResult {
        let p = &self.ctx.probe;
        CheckResult {
            id: "mac_lock",
            name: "MAC 锁排查".into(),
            layer,
            status: Status::Info,
            detail: "若仅本机无法上网而其他设备正常，可能是路由器 MAC 过滤 / 绑定限制。".into(),
            fix: Some(Fix {
                kind: FixKind::Manual(format!(
                    "登录路由器后台检查 MAC 过滤名单；本机 MAC 可在 `ipconfig /all` 查看（网卡「{}」）。",
                    p.active_adapter().map(|a| a.name.clone()).unwrap_or_else(|| "未知".into())
                )),
                label: "需手动：检查路由器 MAC 过滤".into(),
            }),
            scope: None,
        }
    }
}

/// 汇总诊断结果，给出一句话结论。
pub fn summarize(results: &[CheckResult]) -> String {
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

/// ping 探测是否可达（单包，1s 超时）。
pub async fn ping_ok(host: &str) -> bool {
    if host.is_empty() {
        return false;
    }
    let host = host.to_string();
    tokio::task::spawn_blocking(move || {
        let out = run("ping", &["-n", "1", "-w", "1000", &host], std::time::Duration::from_secs(6));
        out.success
            || out.stdout.to_lowercase().contains("ttl=")
            || out.stdout.contains("TTL=")
    })
    .await
    .unwrap_or(false)
}

/// 刷新 DNS 缓存（供模块二复用）。
pub fn flush_dns() -> bool {
    netsh::flush_dns().success
}

/// traceroute 到目标（最多 8 跳），返回逐跳信息（实时路径探测）。
pub async fn traceroute(host: &str) -> Vec<String> {
    let host = host.to_string();
    tokio::task::spawn_blocking(move || {
        let out = run(
            "tracert",
            &["-d", "-h", "8", "-w", "800", &host],
            std::time::Duration::from_secs(25),
        );
        out.stdout
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| {
                !l.is_empty()
                    && (l.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
                        || l.contains('*')
                        || l.contains("超过")
                        || l.contains("over")
                        || l.contains("timed out"))
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_result(status: Status, fix: Option<FixKind>) -> CheckResult {
        CheckResult {
            id: "a",
            name: "a".into(),
            layer: Layer::Basic,
            status,
            detail: String::new(),
            fix: fix.map(|k| Fix { kind: k, label: "x".into() }),
            scope: None,
        }
    }

    #[test]
    fn summarize_counts() {
        let r = vec![
            mk_result(Status::Ok, None),
            mk_result(Status::Error, Some(FixKind::Auto("cmd".into()))),
            mk_result(Status::Warn, Some(FixKind::Manual("m".into()))),
        ];
        let s = summarize(&r);
        assert!(s.contains("正常 1"));
        assert!(s.contains("异常 1"));
        assert!(s.contains("警告 1"));
        assert!(s.contains("可自动修复 1"));
        assert!(s.contains("需手动处理 1"));
    }

    #[test]
    fn check_count() {
        assert_eq!(CHECKS.len(), 14);
        assert_eq!(Diagnoser::all_check_names().len(), 14);
    }
}
