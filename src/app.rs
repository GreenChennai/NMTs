//! 全局状态机：TUI 事件循环、模块路由、诊断事件分发。
//!
//! 启动提速：环境探测（PowerShell 合并查询）放在 `spawn_blocking` 后台执行，
//! UI 立即渲染，探测结果经 mpsc 回传后刷新状态栏，避免启动阻塞 3~4 秒。

use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::config::Config;
use crate::core::net_diag::{CheckResult, DiagEvent, Diagnoser};
use crate::core::net_set;
use crate::core::topology::{DeviceRole, Topology};
use crate::core::{backup, design_check, dns, serial, topo_cli, topology, vendor_cli::VendorDb};
use crate::ui;
use crate::windows::adapter::Adapter;
use crate::windows::probe::{probe_network, NetProbe};

/// 五大模块。
pub const TABS: [&str; 5] = ["网络诊断", "快捷设置", "网工工具", "拓扑图", "配置备份"];

/// 模块二快捷设置操作。
#[derive(Debug, Clone, Copy)]
pub enum QuickAction {
    FlushDns,
    SetDns(&'static str, &'static str),
    DnsDhcp,
    IpDhcp,
    ReleaseRenew,
    Ipv6(bool),
    TcpOptimize,
    Backup,
    Restore,
    DnsOptimize,
}

/// 模块二操作项。
#[derive(Debug, Clone, Copy)]
pub struct QuickItem {
    pub name: &'static str,
    pub desc: &'static str,
    pub action: QuickAction,
}

fn quick_items() -> Vec<QuickItem> {
    vec![
        QuickItem { name: "刷新 DNS 缓存", desc: "ipconfig /flushdns", action: QuickAction::FlushDns },
        QuickItem { name: "设置 DNS：阿里", desc: "223.5.5.5", action: QuickAction::SetDns("阿里", "223.5.5.5") },
        QuickItem { name: "设置 DNS：腾讯", desc: "119.29.29.29", action: QuickAction::SetDns("腾讯", "119.29.29.29") },
        QuickItem { name: "设置 DNS：Google", desc: "8.8.8.8", action: QuickAction::SetDns("Google", "8.8.8.8") },
        QuickItem { name: "设置 DNS：Cloudflare", desc: "1.1.1.1", action: QuickAction::SetDns("Cloudflare", "1.1.1.1") },
        QuickItem { name: "DNS 切回 DHCP", desc: "自动获取 DNS", action: QuickAction::DnsDhcp },
        QuickItem { name: "IP 切回 DHCP", desc: "自动获取 IP", action: QuickAction::IpDhcp },
        QuickItem { name: "释放并续租 IP", desc: "ipconfig /release + /renew", action: QuickAction::ReleaseRenew },
        QuickItem { name: "开启 IPv6", desc: "netsh ipv6 set state enabled", action: QuickAction::Ipv6(true) },
        QuickItem { name: "关闭 IPv6", desc: "netsh ipv6 set state disabled", action: QuickAction::Ipv6(false) },
        QuickItem { name: "TCP 全局优化", desc: "autotuninglevel=normal + ecn", action: QuickAction::TcpOptimize },
        QuickItem { name: "DNS 优选（测速+应用最优）", desc: "并发测速就近优选一键应用", action: QuickAction::DnsOptimize },
        QuickItem { name: "备份当前配置", desc: "netsh dump → backups/", action: QuickAction::Backup },
        QuickItem { name: "恢复最近备份", desc: "netsh -f 最近备份", action: QuickAction::Restore },
    ]
}

/// 模块二状态。
#[derive(Debug)]
pub struct QuickSetState {
    pub selected: usize,
    pub items: Vec<QuickItem>,
    pub result: Option<String>,
}

impl Default for QuickSetState {
    fn default() -> Self {
        Self {
            selected: 0,
            items: quick_items(),
            result: None,
        }
    }
}

/// 模块三（网工终端）状态。
#[derive(Debug, Default)]
pub struct TermState {
    pub vendor_idx: usize,
    pub cmd_idx: usize,
    pub ports: Vec<serial::PortInfo>,
    pub status: Option<String>,
}

/// 模块五（配置备份）状态。
#[derive(Debug, Default)]
pub struct BackupState {
    pub selected: usize,
    pub result: Option<String>,
    pub bundles: Vec<std::path::PathBuf>,
}

/// DNS 优选结果更新。
#[derive(Debug, Clone)]
pub struct DnsUpdate {
    pub results: Vec<dns::DnsBench>,
    pub status: String,
}

/// 模块二 DNS 优选状态。
#[derive(Debug, Default)]
pub struct DnsState {
    pub running: bool,
    pub results: Vec<dns::DnsBench>,
    pub status: Option<String>,
}

/// 模块四（拓扑图）状态。
#[derive(Debug)]
pub struct TopoState {
    pub topology: Topology,
    pub findings: Vec<design_check::Finding>,
    pub selected: usize,
    pub cli: Option<String>,
    pub status: Option<String>,
}

impl Default for TopoState {
    fn default() -> Self {
        let topology = topology::demo_topology();
        let findings = design_check::check(
            &topology,
            &[
                design_check::Intent::UniqueSubnet,
                design_check::Intent::VlanPropagated { vlan: 10, to: DeviceRole::Access },
                design_check::Intent::VlanPropagated { vlan: 20, to: DeviceRole::Access },
                design_check::Intent::RedundantUplink { role: DeviceRole::Access },
                design_check::Intent::NoLoop,
            ],
        );
        Self {
            topology,
            findings,
            selected: 0,
            cli: None,
            status: None,
        }
    }
}

/// 模块一诊断界面状态。
#[derive(Debug, Default)]
pub struct DiagState {
    pub running: bool,
    pub started: bool,
    pub names: Vec<String>,
    pub results: Vec<Option<CheckResult>>,
    pub logs: Vec<String>,
    pub summary: Option<String>,
}

impl DiagState {
    fn reset(&mut self) {
        self.running = true;
        self.started = true;
        self.names = Diagnoser::all_check_names();
        self.results = vec![None; self.names.len()];
        self.logs.clear();
        self.summary = None;
    }
}

pub struct App {
    #[allow(dead_code)] // v0.2 供快捷设置模块读取
    pub config: Config,
    pub is_admin: bool,
    pub adapters: Vec<Adapter>,
    pub active_adapter: Option<Adapter>,
    pub env_ready: bool,
    pub tab: usize,
    pub diag: DiagState,
    pub quick_set: QuickSetState,
    pub term: TermState,
    pub vendor_db: VendorDb,
    pub backup: BackupState,
    pub topo: TopoState,
    pub dns: DnsState,
    pub show_help: bool,
    pub status_msg: Option<String>,
    running: bool,
    tx: UnboundedSender<DiagEvent>,
    rx: UnboundedReceiver<DiagEvent>,
    probe_tx: UnboundedSender<NetProbe>,
    probe_rx: UnboundedReceiver<NetProbe>,
    dns_tx: UnboundedSender<DnsUpdate>,
    dns_rx: UnboundedReceiver<DnsUpdate>,
    rt: Handle,
}

impl App {
    pub fn new(
        config: Config,
        rt: Handle,
        tx: UnboundedSender<DiagEvent>,
        rx: UnboundedReceiver<DiagEvent>,
    ) -> Self {
        let (probe_tx, probe_rx) = tokio::sync::mpsc::unbounded_channel();
        let (dns_tx, dns_rx) = tokio::sync::mpsc::unbounded_channel();

        // 后台探测（不阻塞 UI）
        let probe_tx_clone = probe_tx.clone();
        rt.spawn_blocking(move || {
            let p = probe_network().unwrap_or_default();
            let _ = probe_tx_clone.send(p);
        });

        Self {
            config,
            is_admin: false,
            adapters: Vec::new(),
            active_adapter: None,
            env_ready: false,
            tab: 0,
            diag: DiagState::default(),
            quick_set: QuickSetState::default(),
            term: TermState {
                ports: serial::list_ports(),
                ..Default::default()
            },
            vendor_db: VendorDb::load(),
            backup: BackupState::default(),
            topo: TopoState::default(),
            dns: DnsState::default(),
            show_help: false,
            status_msg: None,
            running: true,
            tx,
            rx,
            probe_tx,
            probe_rx,
            dns_tx,
            dns_rx,
            rt,
        }
    }

    /// 应用探测结果（刷新状态栏）。
    fn apply_probe(&mut self, p: NetProbe) {
        self.is_admin = p.is_admin;
        self.adapters = p.adapters.clone();
        self.active_adapter = p.active_adapter().cloned();
        self.env_ready = true;
    }

    pub fn run(&mut self) -> Result<()> {
        let mut terminal = ratatui::init();
        terminal.clear()?;

        while self.running {
            self.drain_events();
            self.drain_probe();
            self.drain_dns();
            terminal.draw(|f| ui::draw(f, self))?;

            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.on_key(key.code);
                    }
                }
            }
        }

        ratatui::restore();
        Ok(())
    }

    fn on_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Char('Q') => self.running = false,
            KeyCode::Char('h') | KeyCode::Char('?') => self.show_help = !self.show_help,
            KeyCode::Esc => {
                if self.show_help {
                    self.show_help = false;
                }
            }
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                self.tab = (self.tab + 1) % TABS.len();
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('j') => {
                self.tab = (self.tab + TABS.len() - 1) % TABS.len();
            }
            KeyCode::Char('[') => {
                if self.tab == 2 && !self.vendor_db.vendors().is_empty() {
                    let n = self.vendor_db.vendors().len();
                    self.term.vendor_idx = (self.term.vendor_idx + n - 1) % n;
                    self.term.cmd_idx = 0;
                }
            }
            KeyCode::Char(']') => {
                if self.tab == 2 && !self.vendor_db.vendors().is_empty() {
                    let n = self.vendor_db.vendors().len();
                    self.term.vendor_idx = (self.term.vendor_idx + 1) % n;
                    self.term.cmd_idx = 0;
                }
            }
            KeyCode::Up => match self.tab {
                1 => {
                    if !self.quick_set.items.is_empty() {
                        let n = self.quick_set.items.len();
                        self.quick_set.selected = (self.quick_set.selected + n - 1) % n;
                    }
                }
                2 => self.term_move(-1),
                3 => self.topo.selected = self.topo.selected.saturating_sub(1),
                4 => self.backup.selected = self.backup.selected.saturating_sub(1),
                _ => {}
            },
            KeyCode::Down => match self.tab {
                1 => {
                    if !self.quick_set.items.is_empty() {
                        let n = self.quick_set.items.len();
                        self.quick_set.selected = (self.quick_set.selected + 1) % n;
                    }
                }
                2 => self.term_move(1),
                3 => {
                    let n = self.topo.topology.devices.len();
                    if n > 0 && self.topo.selected + 1 < n {
                        self.topo.selected += 1;
                    }
                }
                4 => {
                    if self.backup.selected < 2 {
                        self.backup.selected += 1;
                    }
                }
                _ => {}
            },
            KeyCode::Char('e') | KeyCode::Char('E') => {
                if self.tab == 3 {
                    self.topo_export_d2();
                }
            }
            KeyCode::Enter | KeyCode::Char('r') | KeyCode::Char('R') => {
                match self.tab {
                    0 => {
                        if !self.diag.running {
                            self.start_diag();
                        }
                    }
                    1 => {
                        if let Some(item) = self.quick_set.items.get(self.quick_set.selected) {
                            let action = item.action;
                            self.execute_quick(action);
                        }
                    }
                    2 => self.term_send(),
                    3 => self.topo_gen_cli(),
                    4 => {
                        let sel = self.backup.selected;
                        self.execute_backup(sel);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// 执行模块二快捷操作（同步，netsh 命令较快）。
    fn execute_quick(&mut self, action: QuickAction) {
        let root = crate::config::app_root();
        let iface = self
            .active_adapter
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_default();

        let (ok, msg) = match action {
            QuickAction::FlushDns => {
                let o = net_set::flush_dns();
                (o.success, if o.success { "已刷新 DNS 缓存".into() } else { o.combined() })
            }
            QuickAction::SetDns(label, ip) => {
                if iface.is_empty() {
                    (false, "未找到当前上网网卡".into())
                } else {
                    let o = net_set::set_dns(&iface, ip);
                    (o.success, if o.success { format!("已设置 DNS「{label}」{ip}") } else { o.combined() })
                }
            }
            QuickAction::DnsDhcp => {
                if iface.is_empty() {
                    (false, "未找到当前上网网卡".into())
                } else {
                    let o = net_set::set_dns_dhcp(&iface);
                    (o.success, if o.success { "DNS 已切回自动获取".into() } else { o.combined() })
                }
            }
            QuickAction::IpDhcp => {
                if iface.is_empty() {
                    (false, "未找到当前上网网卡".into())
                } else {
                    let o = net_set::set_ip_dhcp(&iface);
                    (o.success, if o.success { "IP 已切回自动获取".into() } else { o.combined() })
                }
            }
            QuickAction::ReleaseRenew => {
                if iface.is_empty() {
                    (false, "未找到当前上网网卡".into())
                } else {
                    let o = net_set::release_renew(&iface);
                    (o.success, if o.success { "已释放并重新获取 IP".into() } else { o.combined() })
                }
            }
            QuickAction::Ipv6(on) => {
                let o = net_set::set_ipv6(on);
                (o.success, if o.success { format!("IPv6 已{}", if on { "开启" } else { "关闭" }) } else { o.combined() })
            }
            QuickAction::TcpOptimize => {
                let o = net_set::tcp_optimize();
                (o.success, if o.success { "TCP 已优化（自动调谐 + ECN）".into() } else { o.combined() })
            }
            QuickAction::Backup => match net_set::backup_network(&root) {
                Ok(dir) => (true, format!("已备份到 {}", dir.display())),
                Err(e) => (false, e),
            },
            QuickAction::Restore => match net_set::list_backups(&root).into_iter().next() {
                Some(dir) => match net_set::restore_network(&dir) {
                    Ok(_) => (true, format!("已从 {} 恢复", dir.display())),
                    Err(e) => (false, e),
                },
                None => (false, "无可用备份".into()),
            },
            QuickAction::DnsOptimize => {
                if iface.is_empty() {
                    (false, "未找到当前上网网卡".into())
                } else {
                    self.dns.running = true;
                    self.dns.results.clear();
                    self.dns.status = Some("DNS 优选测速中…".into());
                    let categories = self.config.dns_preference.categories.clone();
                    let prefer_ipv = self.config.dns_preference.prefer_ipv.clone();
                    let prefer_country = self.config.dns_preference.prefer_country.clone();
                    let iface = iface.clone();
                    let tx = self.dns_tx.clone();
                    self.rt.spawn(async move {
                        let db = dns::DnsDb::load();
                        let candidates = db.filter(&categories, &prefer_ipv);
                        let results = dns::benchmark(&candidates, 15).await;
                        let ranked = dns::rank(results, &prefer_country);
                        let status = if let Some(best) = ranked.first().filter(|b| b.reachable) {
                            let _ = net_set::set_dns(&iface, &best.provider.primary);
                            if !best.provider.secondary.is_empty() {
                                let _ = net_set::add_dns(&iface, &best.provider.secondary);
                            }
                            format!(
                                "最优：{} ({}) {}ms，已应用",
                                best.provider.name,
                                best.provider.primary,
                                best.latency_ms.unwrap_or(0)
                            )
                        } else {
                            "无可达候选 DNS".to_string()
                        };
                        let _ = tx.send(DnsUpdate { results: ranked, status });
                    });
                    (true, "已启动 DNS 优选测速".into())
                }
            }
        };

        self.quick_set.result = Some(format!("{} {msg}", if ok { "✓" } else { "✗" }));
    }

    /// 模块三：导航命令模板。
    fn term_move(&mut self, delta: i64) {
        let Some(v) = self.vendor_db.vendors().get(self.term.vendor_idx) else {
            return;
        };
        let n = v.commands.len();
        if n == 0 {
            return;
        }
        let cur = self.term.cmd_idx as i64;
        self.term.cmd_idx = ((cur + delta + n as i64) % n as i64) as usize;
    }

    /// 模块三：发送当前命令到第一个可用串口。
    fn term_send(&mut self) {
        let Some(v) = self.vendor_db.vendors().get(self.term.vendor_idx) else {
            return;
        };
        let Some(cmd) = v.commands.get(self.term.cmd_idx) else {
            return;
        };
        let rendered = cmd.command.clone();
        match self.term.ports.first() {
            Some(p) => {
                let name = p.name.clone();
                let line = rendered.clone();
                self.term.status = Some(match serial::SerialSession::open(&name, 9600) {
                    Ok(mut s) => match s.write_line(&line) {
                        Ok(_) => format!("已发送到 {name}: {line}"),
                        Err(e) => format!("发送失败: {e}"),
                    },
                    Err(e) => format!("连接 {name} 失败: {e}"),
                });
            }
            None => {
                self.term.status = Some(format!("未检测到串口，命令：{rendered}"));
            }
        }
    }

    /// 模块五：执行备份 / 恢复 / 刷新。
    fn execute_backup(&mut self, action: usize) {
        let root = crate::config::app_root();
        match action {
            0 => match backup::backup_windows(&root) {
                Ok(b) => self.backup.result = Some(format!("✓ 已备份到 {}", b.display())),
                Err(e) => self.backup.result = Some(format!("✗ {e}")),
            },
            1 => match backup::list_bundles(&root).into_iter().next() {
                Some(b) => match backup::restore_windows(&b) {
                    Ok(_) => self.backup.result = Some(format!("✓ 已从 {} 恢复", b.display())),
                    Err(e) => self.backup.result = Some(format!("✗ {e}")),
                },
                None => self.backup.result = Some("✗ 无可用备份".into()),
            },
            2 => {
                self.backup.bundles = backup::list_bundles(&root);
                self.backup.result = Some(format!("已列出 {} 个备份", self.backup.bundles.len()));
            }
            _ => {}
        }
        self.backup.bundles = backup::list_bundles(&root);
    }

    /// 模块四：生成选中设备的 CLI。
    fn topo_gen_cli(&mut self) {
        let Some(d) = self.topo.topology.devices.get(self.topo.selected) else {
            return;
        };
        let cli = topo_cli::generate_device_cli(d, &self.topo.topology);
        self.topo.status = Some(format!("已生成 {} 的 CLI", d.name));
        self.topo.cli = Some(cli);
    }

    /// 模块四：导出 D2 并尝试渲染 SVG。
    fn topo_export_d2(&mut self) {
        let d2 = self.topo.topology.export_d2();
        let root = crate::config::app_root();
        let d2_path = root.join("topology.d2");
        match std::fs::write(&d2_path, &d2) {
            Ok(_) => {
                let svg = root.join("topology.svg");
                let out = crate::windows::run(
                    "d2",
                    &[
                        d2_path.to_str().unwrap_or("topology.d2"),
                        svg.to_str().unwrap_or("topology.svg"),
                    ],
                    std::time::Duration::from_secs(30),
                );
                if out.success {
                    self.topo.status = Some(format!("已导出并渲染：{}", svg.display()));
                } else {
                    self.topo.status = Some(format!(
                        "已导出 {}（未检测到 d2 CLI，跳过渲染，可手动 `d2 {}`）",
                        d2_path.display(),
                        d2_path.display()
                    ));
                }
            }
            Err(e) => self.topo.status = Some(format!("导出失败: {e}")),
        }
    }

    /// 启动诊断（后台重新探测 + 异步执行，事件经 mpsc 回传）。
    fn start_diag(&mut self) {
        let tx = self.tx.clone();
        let probe_tx = self.probe_tx.clone();
        self.diag.reset();
        self.rt.spawn(async move {
            let d = tokio::task::spawn_blocking(Diagnoser::new)
                .await
                .unwrap_or_else(|_| Diagnoser {
                    ctx: crate::core::net_diag::DiagContext::default(),
                });
            // 回传最新探测结果，刷新状态栏
            let _ = probe_tx.send(d.ctx.probe.clone());
            let _ = d.run(tx).await;
        });
    }

    /// 拉取环境探测结果。
    fn drain_probe(&mut self) {
        while let Ok(p) = self.probe_rx.try_recv() {
            self.apply_probe(p);
        }
    }

    /// 拉取 DNS 优选测速结果。
    fn drain_dns(&mut self) {
        while let Ok(u) = self.dns_rx.try_recv() {
            self.dns.results = u.results;
            self.dns.status = Some(u.status);
            self.dns.running = false;
        }
    }

    /// 拉取诊断事件并更新状态。
    fn drain_events(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                DiagEvent::Started { total } => {
                    self.diag.started = true;
                    if self.diag.results.len() != total {
                        self.diag.results = vec![None; total];
                    }
                }
                DiagEvent::CheckStarted { index, name } => {
                    if let Some(slot) = self.diag.names.get_mut(index) {
                        *slot = name;
                    }
                }
                DiagEvent::CheckDone { index, result } => {
                    if let Some(slot) = self.diag.results.get_mut(index) {
                        *slot = Some(result);
                    }
                }
                DiagEvent::Log(line) => {
                    self.diag.logs.push(line);
                    if self.diag.logs.len() > 500 {
                        let excess = self.diag.logs.len() - 500;
                        self.diag.logs.drain(..excess);
                    }
                }
                DiagEvent::Finished { summary } => {
                    self.diag.summary = Some(summary);
                    self.diag.running = false;
                }
            }
        }
    }
}
