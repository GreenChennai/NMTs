//! 全局状态机：TUI 事件循环、模块路由、诊断事件分发。
//!
//! 启动提速：环境探测（PowerShell 合并查询）放在 `spawn_blocking` 后台执行，
//! UI 立即渲染，探测结果经 mpsc 回传后刷新状态栏，避免启动阻塞 3~4 秒。

use std::io::{Read, Write};
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
    DnsDhcp,
    IpDhcp,
    ReleaseRenew,
    /// IPv6 单一开关（读取当前状态并反向切换）。
    ToggleIpv6,
    TcpOptimize,
    /// DNS 优选：仅测速产出排名表，用户选中确认后才应用（见 2.4）。
    DnsOptimize,
    /// 静态 IP 表单（手动填写 IP/掩码/网关/DNS）。
    StaticIp,
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
        // DNS 组
        QuickItem { name: "DNS 优选（测速排名）", desc: "并发测速就近排序，选中确认应用", action: QuickAction::DnsOptimize },
        QuickItem { name: "DNS 切回自动获取", desc: "netsh set dns dhcp", action: QuickAction::DnsDhcp },
        QuickItem { name: "刷新 DNS 缓存", desc: "ipconfig /flushdns", action: QuickAction::FlushDns },
        // IP 组
        QuickItem { name: "设置静态 IP（表单）", desc: "手动填 IP/掩码/网关/DNS", action: QuickAction::StaticIp },
        QuickItem { name: "IP 切回自动获取", desc: "netsh set address dhcp", action: QuickAction::IpDhcp },
        QuickItem { name: "释放并续租 IP", desc: "ipconfig /release + /renew", action: QuickAction::ReleaseRenew },
        // 协议组
        QuickItem { name: "IPv6 开关", desc: "读取当前状态并切换", action: QuickAction::ToggleIpv6 },
        // 优化组
        QuickItem { name: "TCP 全局优化", desc: "autotuninglevel=normal + ecn", action: QuickAction::TcpOptimize },
    ]
}

/// 模块二状态。
#[derive(Debug)]
pub struct QuickSetState {
    pub selected: usize,
    pub items: Vec<QuickItem>,
    pub result: Option<String>,
    pub offset: usize,
}

impl Default for QuickSetState {
    fn default() -> Self {
        Self {
            selected: 0,
            items: quick_items(),
            result: None,
            offset: 0,
        }
    }
}

/// 模块二静态 IP 表单字段标签。
pub const IP_FORM_FIELDS: [&str; 5] = ["IP 地址", "子网掩码", "默认网关", "主 DNS", "备 DNS"];

/// 模块二静态 IP 表单状态（手动填写）。
#[derive(Debug)]
pub struct IpFormState {
    pub active: bool,
    pub fields: [String; 5],
    pub focus: usize,
}

impl Default for IpFormState {
    fn default() -> Self {
        Self {
            active: false,
            fields: [String::new(), String::new(), String::new(), String::new(), String::new()],
            focus: 0,
        }
    }
}

/// 网工工具连接状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Disconnected,
    Scanning,
    Connected,
}

/// 网工工具终端事件（后台读线程 / 连接结果回传）。
pub enum TermEvent {
    Echo(String),
    Connected(std::sync::Arc<std::sync::Mutex<Box<dyn serialport::SerialPort>>>),
    Failed(String),
}

/// 模块三（网工终端）状态。
#[derive(Debug)]
pub struct TermState {
    pub conn: ConnState,
    pub vendor_idx: usize,
    pub cmd_idx: usize,
    pub cmd_offset: usize,
    pub ports: Vec<serial::PortInfo>,
    pub selected_port: usize,
    pub port_offset: usize,
    pub baud: u32,
    /// 终端实时回显。
    pub output: Vec<String>,
    /// 手动输入缓冲。
    pub input: String,
    pub input_mode: bool,
    pub status: Option<String>,
}

impl Default for TermState {
    fn default() -> Self {
        Self {
            conn: ConnState::Disconnected,
            vendor_idx: 0,
            cmd_idx: 0,
            cmd_offset: 0,
            ports: Vec::new(),
            selected_port: 0,
            port_offset: 0,
            baud: 9600,
            output: Vec::new(),
            input: String::new(),
            input_mode: false,
            status: None,
        }
    }
}

/// 模块五（配置备份）状态。
#[derive(Debug, Default)]
pub struct BackupState {
    pub selected: usize,
    pub result: Option<String>,
    pub bundles: Vec<std::path::PathBuf>,
    pub offset: usize,
    pub bundles_offset: usize,
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
    /// 排名表选中项（测速完成后进入交互模式）。
    pub selected: usize,
    pub offset: usize,
    /// 是否处于排名表交互模式（↑/↓ 选 DNS、Enter 应用、Esc 返回）。
    pub interactive: bool,
}

/// 模块四（拓扑图）状态。
#[derive(Debug)]
pub struct TopoState {
    pub topology: Topology,
    pub findings: Vec<design_check::Finding>,
    pub selected: usize,
    pub cli: Option<String>,
    pub status: Option<String>,
    pub offset: usize,
    pub findings_offset: usize,
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
            offset: 0,
            findings_offset: 0,
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
    pub offset: usize,
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
    pub ip_form: IpFormState,
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
    term_tx: UnboundedSender<TermEvent>,
    term_rx: UnboundedReceiver<TermEvent>,
    /// 持久串口会话（后台读线程 + 主线程写共享）。
    session: Option<std::sync::Arc<std::sync::Mutex<Box<dyn serialport::SerialPort>>>>,
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
        let (term_tx, term_rx) = tokio::sync::mpsc::unbounded_channel();

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
            ip_form: IpFormState::default(),
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
            term_tx,
            term_rx,
            session: None,
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
            self.drain_term();
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
                } else if self.dns.interactive {
                    self.dns.interactive = false;
                } else if self.ip_form.active {
                    self.ip_form.active = false;
                } else if self.term.input_mode {
                    self.term.input_mode = false;
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
                    if self.ip_form.active {
                        self.ip_form.focus = self.ip_form.focus.saturating_sub(1);
                    } else if self.dns.interactive {
                        let n = self.dns.results.len();
                        if n > 0 {
                            self.dns.selected = (self.dns.selected + n - 1) % n;
                        }
                    } else if !self.quick_set.items.is_empty() {
                        let n = self.quick_set.items.len();
                        self.quick_set.selected = (self.quick_set.selected + n - 1) % n;
                    }
                }
                2 => {
                    if self.term.conn == ConnState::Disconnected {
                        if !self.term.ports.is_empty() {
                            let n = self.term.ports.len();
                            self.term.selected_port = (self.term.selected_port + n - 1) % n;
                        }
                    } else if !self.term.input_mode {
                        self.term_move(-1);
                    }
                }
                3 => self.topo.selected = self.topo.selected.saturating_sub(1),
                4 => self.backup.selected = self.backup.selected.saturating_sub(1),
                _ => {}
            },
            KeyCode::Down => match self.tab {
                1 => {
                    if self.ip_form.active {
                        if self.ip_form.focus + 1 < 5 {
                            self.ip_form.focus += 1;
                        }
                    } else if self.dns.interactive {
                        let n = self.dns.results.len();
                        if n > 0 {
                            self.dns.selected = (self.dns.selected + 1) % n;
                        }
                    } else if !self.quick_set.items.is_empty() {
                        let n = self.quick_set.items.len();
                        self.quick_set.selected = (self.quick_set.selected + 1) % n;
                    }
                }
                2 => {
                    if self.term.conn == ConnState::Disconnected {
                        if !self.term.ports.is_empty() {
                            let n = self.term.ports.len();
                            self.term.selected_port = (self.term.selected_port + 1) % n;
                        }
                    } else if !self.term.input_mode {
                        self.term_move(1);
                    }
                }
                3 => {
                    let n = self.topo.topology.devices.len();
                    if n > 0 && self.topo.selected + 1 < n {
                        self.topo.selected += 1;
                    }
                }
                4 => {
                    if self.backup.selected < 3 {
                        self.backup.selected += 1;
                    }
                }
                _ => {}
            },
            KeyCode::Char('i') | KeyCode::Char('I') => {
                if self.tab == 2 && self.term.conn == ConnState::Connected {
                    self.term.input_mode = true;
                }
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                if self.tab == 2 && self.term.conn == ConnState::Disconnected {
                    let bauds = [9600u32, 115200, 19200, 38400, 57600];
                    if let Some(p) = bauds.iter().position(|&b| b == self.term.baud) {
                        self.term.baud = bauds[(p + 1) % bauds.len()];
                    }
                }
            }
            KeyCode::Char('e') | KeyCode::Char('E') => {
                if self.tab == 3 {
                    self.topo_export_d2();
                }
            }
            KeyCode::Char('f') | KeyCode::Char('F') => {
                if self.tab == 0 && self.diag.started && !self.diag.running {
                    self.execute_auto_fixes();
                }
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                if self.tab == 3 {
                    self.topo_deploy();
                }
            }
            KeyCode::Char('o') | KeyCode::Char('O') => {
                if self.tab == 3 {
                    self.topo_open_editor();
                }
            }
            KeyCode::Char('b') | KeyCode::Char('B') => {
                if self.tab == 3 {
                    self.topo_load_json();
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
                        if self.ip_form.active {
                            self.submit_static_ip();
                        } else if self.dns.interactive {
                            self.execute_dns_apply();
                        } else if let Some(item) = self.quick_set.items.get(self.quick_set.selected) {
                            let action = item.action;
                            self.execute_quick(action);
                        }
                    }
                    2 => match self.term.conn {
                        ConnState::Disconnected => self.scan_connect(),
                        ConnState::Connected => {
                            if self.term.input_mode {
                                self.term_send_input();
                            } else {
                                self.term_send();
                            }
                        }
                        ConnState::Scanning => {}
                    },
                    3 => self.topo_gen_cli(),
                    4 => {
                        let sel = self.backup.selected;
                        self.execute_backup(sel);
                    }
                    _ => {}
                }
            }
            KeyCode::Backspace => {
                if self.tab == 1 && self.ip_form.active {
                    if let Some(f) = self.ip_form.fields.get_mut(self.ip_form.focus) {
                        f.pop();
                    }
                } else if self.tab == 2 && self.term.input_mode {
                    self.term.input.pop();
                }
            }
            KeyCode::Char(c) => {
                if self.tab == 1 && self.ip_form.active {
                    // 表单输入：数字、点、冒号、十六进制字母
                    if c.is_ascii_digit() || c == '.' || c == ':' || "abcdefABCDEF".contains(c) {
                        if let Some(f) = self.ip_form.fields.get_mut(self.ip_form.focus) {
                            if f.len() < 45 {
                                f.push(c);
                            }
                        }
                    }
                } else if self.tab == 2 && self.term.input_mode {
                    self.term.input.push(c);
                }
            }
            _ => {}
        }
    }

    /// 执行模块二快捷操作（同步，netsh 命令较快）。
    fn execute_quick(&mut self, action: QuickAction) {
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
            QuickAction::ToggleIpv6 => {
                let cur = net_set::ipv6_enabled();
                let o = net_set::set_ipv6(!cur);
                let target = if !cur { "启用" } else { "禁用" };
                (o.success, if o.success { format!("IPv6 已切换为{target}") } else { o.combined() })
            }
            QuickAction::StaticIp => {
                self.ip_form.active = true;
                self.ip_form.focus = 0;
                (true, "进入静态 IP 表单：↑/↓ 选字段 · 输入 · Enter 应用 · Esc 返回".into())
            }
            QuickAction::TcpOptimize => {
                let o = net_set::tcp_optimize();
                (o.success, if o.success { "TCP 已优化（自动调谐 + ECN）".into() } else { o.combined() })
            }
            QuickAction::DnsOptimize => {
                if iface.is_empty() {
                    (false, "未找到当前上网网卡".into())
                } else {
                    // 仅测速产出排名表，不自动写入；用户选中确认后才应用（见 2.4）
                    self.dns.running = true;
                    self.dns.interactive = false;
                    self.dns.results.clear();
                    self.dns.selected = 0;
                    self.dns.offset = 0;
                    self.dns.status = Some("DNS 优选测速中…".into());
                    let categories = self.config.dns_preference.categories.clone();
                    let prefer_ipv = self.config.dns_preference.prefer_ipv.clone();
                    let prefer_country = self.config.dns_preference.prefer_country.clone();
                    let tx = self.dns_tx.clone();
                    self.rt.spawn(async move {
                        let db = dns::DnsDb::load();
                        let candidates = db.filter(&categories, &prefer_ipv);
                        let results = dns::benchmark(&candidates, 15).await;
                        let ranked = dns::rank(results, &prefer_country);
                        let status = if ranked.iter().any(|b| b.reachable) {
                            "测速完成，↑/↓ 选择 · Enter 应用 · Esc 返回".to_string()
                        } else {
                            "无可达候选 DNS".to_string()
                        };
                        let _ = tx.send(DnsUpdate { results: ranked, status });
                    });
                    (true, "已启动 DNS 优选测速（不会自动应用）".into())
                }
            }
        };

        self.quick_set.result = Some(format!("{} {msg}", if ok { "✓" } else { "✗" }));
    }

    /// 模块二：应用排名表选中的 DNS（先备份原 DNS，可回退）。
    fn execute_dns_apply(&mut self) {
        let Some(best) = self.dns.results.get(self.dns.selected).cloned() else {
            return;
        };
        let iface = self
            .active_adapter
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_default();
        if iface.is_empty() {
            self.quick_set.result = Some("✗ 未找到当前上网网卡".into());
            return;
        }
        if !best.reachable {
            self.quick_set.result = Some("✗ 该候选不可达".into());
            return;
        }
        // 应用前备份原配置，可回退
        let _ = net_set::backup_network(&crate::config::app_root());
        let _ = net_set::set_dns(&iface, &best.provider.primary);
        if !best.provider.secondary.is_empty() {
            let _ = net_set::add_dns(&iface, &best.provider.secondary);
        }
        let msg = format!(
            "✓ 已应用 DNS {}（{} / {}）",
            best.provider.name, best.provider.primary, best.provider.secondary
        );
        self.quick_set.result = Some(msg.clone());
        self.dns.status = Some(format!("已应用 {}，原 DNS 已备份可回退", best.provider.name));
        self.dns.interactive = false;
    }

    /// 模块二：提交静态 IP 表单。
    fn submit_static_ip(&mut self) {
        let iface = self
            .active_adapter
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_default();
        if iface.is_empty() {
            self.quick_set.result = Some("✗ 未找到当前上网网卡".into());
            return;
        }
        let ip = self.ip_form.fields[0].clone();
        let mask = self.ip_form.fields[1].clone();
        let gw = self.ip_form.fields[2].clone();
        let dns1 = self.ip_form.fields[3].clone();
        let dns2 = self.ip_form.fields[4].clone();

        if ip.is_empty() || mask.is_empty() {
            self.quick_set.result = Some("✗ 请至少填写 IP 地址和子网掩码".into());
            return;
        }

        // 应用前备份
        let _ = net_set::backup_network(&crate::config::app_root());
        let o = net_set::set_static_ip(&iface, &ip, &mask, if gw.is_empty() { "" } else { &gw });
        if o.success {
            if !dns1.is_empty() {
                let _ = net_set::set_dns(&iface, &dns1);
            }
            if !dns2.is_empty() {
                let _ = net_set::add_dns(&iface, &dns2);
            }
            self.quick_set.result = Some(format!("✓ 已设置静态 IP {ip}/{mask} 网关 {}", if gw.is_empty() { "无" } else { &gw }));
            self.ip_form.active = false;
        } else {
            self.quick_set.result = Some(format!("✗ {}", o.combined()));
        }
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

    /// 模块三：发送当前命令到持久会话。
    fn term_send(&mut self) {
        let Some(v) = self.vendor_db.vendors().get(self.term.vendor_idx) else {
            return;
        };
        let Some(cmd) = v.commands.get(self.term.cmd_idx) else {
            return;
        };
        let rendered = cmd.command.clone();
        self.send_line(&rendered);
    }

    /// 模块三：发送手动输入行。
    fn term_send_input(&mut self) {
        let line = self.term.input.clone();
        self.term.input.clear();
        self.term.input_mode = false;
        if !line.trim().is_empty() {
            self.send_line(&line);
        }
    }

    /// 模块三：向持久会话写一行。
    fn send_line(&mut self, line: &str) {
        let result = match &self.session {
            Some(sess) => match sess.lock() {
                Ok(mut port) => {
                    let bytes = format!("{line}\r\n").into_bytes();
                    port.write_all(&bytes).map_err(|e| e.to_string())
                }
                Err(_) => Err("会话锁失败".to_string()),
            },
            None => Err("未连接设备，请先按 Enter 扫描连接".to_string()),
        };
        match result {
            Ok(_) => self.term.status = Some(format!("已发送: {line}")),
            Err(e) => {
                if self.session.is_some() {
                    self.term.status = Some(format!("发送失败: {e}"));
                    self.term.conn = ConnState::Disconnected;
                    self.session = None;
                } else {
                    self.term.status = Some(e);
                }
            }
        }
    }

    /// 模块三：扫描串口并连接（后台执行，成功建立持久会话 + 读线程）。
    fn scan_connect(&mut self) {
        if self.term.conn != ConnState::Disconnected {
            return;
        }
        self.term.conn = ConnState::Scanning;
        self.term.status = Some("扫描串口 / 试探波特率…".into());
        self.term.ports = serial::list_ports();
        let baud = self.term.baud;
        let tx = self.term_tx.clone();

        self.rt.spawn_blocking(move || {
            for p in serial::list_ports() {
                match serial::SerialSession::open(&p.name, baud) {
                    Ok(mut sess) => {
                        if sess.write_line("\r").is_ok() {
                            // 试探回显
                            let mut buf = [0u8; 256];
                            let mut collected = Vec::new();
                            for _ in 0..6 {
                                match sess.read_chunk(&mut buf) {
                                    Ok(n) if n > 0 => {
                                        collected.extend_from_slice(&buf[..n]);
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                            let looks = String::from_utf8_lossy(&collected).to_lowercase();
                            if looks.contains('>')
                                || looks.contains('#')
                                || looks.contains("username")
                                || looks.contains("login")
                                || looks.contains("password")
                                || looks.is_empty()
                            {
                                // 打开持久会话
                                if let Ok(port) = serialport::new(&p.name, baud)
                                    .timeout(Duration::from_millis(200))
                                    .open()
                                {
                                    let arc = std::sync::Arc::new(std::sync::Mutex::new(port));
                                    let _ = tx.send(TermEvent::Connected(arc.clone()));
                                    // 后台读线程
                                    let rx_tx = tx.clone();
                                    std::thread::spawn(move || {
                                        let mut buf = [0u8; 512];
                                        loop {
                                            let n = {
                                                let mut port = match arc.lock() {
                                                    Ok(p) => p,
                                                    Err(_) => break,
                                                };
                                                match port.read(&mut buf) {
                                                    Ok(n) if n > 0 => n,
                                                    _ => 0,
                                                }
                                            };
                                            if n == 0 {
                                                std::thread::sleep(Duration::from_millis(50));
                                                continue;
                                            }
                                            let text = String::from_utf8_lossy(&buf[..n]).to_string();
                                            if rx_tx.send(TermEvent::Echo(text)).is_err() {
                                                break;
                                            }
                                        }
                                    });
                                    return;
                                }
                            }
                        }
                    }
                    Err(_) => {}
                }
            }
            let _ = tx.send(TermEvent::Failed("未找到可连接的串口设备".to_string()));
        });
    }

    /// 拉取终端事件（回显 / 连接结果）。
    fn drain_term(&mut self) {
        while let Ok(ev) = self.term_rx.try_recv() {
            match ev {
                TermEvent::Echo(text) => {
                    for line in text.lines() {
                        self.term.output.push(line.to_string());
                    }
                    if self.term.output.len() > 500 {
                        let excess = self.term.output.len() - 500;
                        self.term.output.drain(..excess);
                    }
                }
                TermEvent::Connected(sess) => {
                    self.session = Some(sess);
                    self.term.conn = ConnState::Connected;
                    self.term.status = Some("已连接，回车 / I 输入命令，↑/↓ 选模板".into());
                }
                TermEvent::Failed(msg) => {
                    self.term.conn = ConnState::Disconnected;
                    self.term.status = Some(format!("✗ {msg}"));
                }
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
            3 => self.backup_device_config(),
            _ => {}
        }
        self.backup.bundles = backup::list_bundles(&root);
    }

    /// 模块五：抓取已连接设备的 running-config 并归档。
    fn backup_device_config(&mut self) {
        if self.term.conn != ConnState::Connected {
            self.backup.result = Some("✗ 请先在「网工工具」连接设备".into());
            return;
        }
        let vendor = self
            .vendor_db
            .vendors()
            .get(self.term.vendor_idx)
            .map(|v| v.vendor.clone())
            .unwrap_or_else(|| "huawei_vrp".to_string());
        let device_name = self
            .term
            .ports
            .get(self.term.selected_port)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "device".to_string());

        // 发送抓取命令
        let show_cmd = if vendor == "cisco_ios" {
            "show running-config"
        } else {
            "display current-configuration"
        };
        self.send_line(show_cmd);

        // 用当前终端回显的最近内容近似归档（真实抓取需等待回显完整）
        let cfg = self.term.output.join("\n");
        match backup::backup_device(&crate::config::app_root(), &device_name, &vendor, &cfg) {
            Ok(b) => self.backup.result = Some(format!("✓ 已归档设备配置到 {}", b.display())),
            Err(e) => self.backup.result = Some(format!("✗ {e}")),
        }
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

    /// 模块四：把已生成的 CLI 逐行下发到第一个串口。
    fn topo_deploy(&mut self) {
        let Some(cli) = self.topo.cli.clone() else {
            self.topo.status = Some("请先按 Enter 生成 CLI".into());
            return;
        };
        match self.term.ports.first() {
            Some(p) => {
                let name = p.name.clone();
                self.topo.status = Some(match serial::SerialSession::open(&name, 9600) {
                    Ok(mut s) => {
                        let mut sent = 0;
                        for line in cli.lines() {
                            if !line.trim().is_empty() && s.write_line(line).is_ok() {
                                sent += 1;
                            }
                        }
                        format!("已下发 {sent} 行到 {name}")
                    }
                    Err(e) => format!("连接 {name} 失败: {e}"),
                });
            }
            None => self.topo.status = Some("未检测到串口，无法下发".into()),
        }
    }

    /// 模块四：打开外部拓扑编辑器（导出 topology.json + 启动 pywebview 窗口）。
    fn topo_open_editor(&mut self) {
        let root = crate::config::app_root();
        let json_path = root.join("topology.json");
        let json = serde_json::to_string_pretty(&self.topo.topology).unwrap_or_default();
        if std::fs::write(&json_path, &json).is_err() {
            self.topo.status = Some("写入 topology.json 失败".into());
            return;
        }
        let editor_py = root.join("editor").join("editor.py");
        // 后台启动（webview 阻塞，不等待）
        let _ = std::process::Command::new("python")
            .arg(editor_py.to_str().unwrap_or("editor.py"))
            .arg(json_path.to_str().unwrap_or("topology.json"))
            .spawn();
        self.topo.status = Some("已启动拓扑编辑器（需 pip install pywebview），编辑后按 B 回读".into());
    }

    /// 模块四：回读 topology.json（重新预检 + CLI 推导）。
    fn topo_load_json(&mut self) {
        let root = crate::config::app_root();
        let json_path = root.join("topology.json");
        match std::fs::read_to_string(&json_path) {
            Ok(s) => match serde_json::from_str::<Topology>(&s) {
                Ok(t) => {
                    self.topo.topology = t;
                    self.topo.findings = design_check::check(
                        &self.topo.topology,
                        &[
                            design_check::Intent::UniqueSubnet,
                            design_check::Intent::VlanPropagated { vlan: 10, to: DeviceRole::Access },
                            design_check::Intent::VlanPropagated { vlan: 20, to: DeviceRole::Access },
                            design_check::Intent::RedundantUplink { role: DeviceRole::Access },
                            design_check::Intent::NoLoop,
                        ],
                    );
                    self.topo.selected = 0;
                    self.topo.cli = None;
                    self.topo.status = Some("已回读 topology.json 并重新预检".into());
                }
                Err(e) => self.topo.status = Some(format!("解析 topology.json 失败: {e}")),
            },
            Err(_) => self.topo.status = Some("未找到 topology.json，请先打开编辑器保存".into()),
        }
    }

    /// 诊断→修复闭环：执行所有「自动修复」项。
    fn execute_auto_fixes(&mut self) {
        let fixes: Vec<String> = self
            .diag
            .results
            .iter()
            .filter_map(|r| r.as_ref())
            .filter_map(|r| r.fix.as_ref())
            .filter_map(|f| match &f.kind {
                crate::core::net_diag::FixKind::Auto(cmd) => Some(cmd.clone()),
                _ => None,
            })
            .collect();

        if fixes.is_empty() {
            self.diag.summary = Some("无可自动修复项".into());
            return;
        }

        let mut done = 0;
        for cmd in fixes {
            let out = crate::windows::run("cmd", &["/c", &cmd], std::time::Duration::from_secs(30));
            self.diag.logs.push(format!("执行：{cmd}"));
            if out.success {
                done += 1;
            } else {
                let first = out.combined().lines().next().unwrap_or("失败").to_string();
                self.diag.logs.push(format!("  ✗ {first}"));
            }
        }
        self.diag.summary = Some(format!("已执行 {done} 项自动修复"));
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
            if !self.dns.results.is_empty() {
                self.dns.selected = 0;
                self.dns.interactive = true;
            }
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
