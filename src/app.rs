//! 全局状态机：TUI 事件循环、模块路由、诊断事件分发。
//!
//! 启动提速：环境探测（PowerShell 合并查询）放在 `spawn_blocking` 后台执行，
//! UI 立即渲染，探测结果经 mpsc 回传后刷新状态栏，避免启动阻塞 3~4 秒。

use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::core::net_diag::{CheckResult, DiagEvent, Diagnoser};
use crate::core::net_set;
use crate::core::topology::{DeviceRole, Topology};
use crate::core::{backup, design_check, dns, serial, topo_cli, topology, vendor_cli::VendorDb};
use crate::ui;
use crate::ui::ime;
use crate::ui::nav::NavIntent;
use crate::windows::adapter::Adapter;
use crate::windows::probe::{probe_network, NetProbe};

/// 五大模块。
pub const TABS: [&str; 5] = ["网络诊断", "快捷设置", "网工工具", "拓扑图", "配置备份"];

/// 模块二快捷设置操作（高级设置区动作）。
#[derive(Debug, Clone, Copy)]
pub enum QuickAction {
    FlushDns,
    ReleaseRenew,
    TcpOptimize,
    /// DNS 优选：仅测速产出排名表，用户选中确认后才应用（见 2.4）。
    DnsOptimize,
}

/// 模块二高级设置动作。
pub const ADVANCED_ACTIONS: [(&str, QuickAction); 4] = [
    ("刷新 DNS 缓存", QuickAction::FlushDns),
    ("释放并续租 IP", QuickAction::ReleaseRenew),
    ("DNS 优选（测速排名）", QuickAction::DnsOptimize),
    ("TCP 全局优化", QuickAction::TcpOptimize),
];

/// IPv4 / IPv6 字段标签（顺序：网关 / 掩码 / IP / 主 DNS / 备 DNS）。
pub const IP_FIELD_LABELS: [&str; 5] = ["网关", "子网掩码", "IP 地址", "主 DNS", "备 DNS"];

/// 模块二焦点行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QsRow {
    Ipv4Toggle,
    Ipv4Field(usize),
    Ipv6Toggle,
    Ipv6Field(usize),
    AdvancedToggle,
    AdvancedItem(usize),
}

/// 模块二状态（V3.0 结构化设置面板）。
#[derive(Debug)]
pub struct QuickSetState {
    /// 当前焦点行索引（对应 `focus_rows` 动态列表）。
    pub focus: usize,
    pub result: Option<String>,
    /// IPv4 是否静态（false=DHCP）。
    pub ipv4_static: bool,
    /// IPv4 静态表单字段（网关/掩码/IP/DNS1/DNS2）。
    pub ipv4_fields: [String; 5],
    /// IPv6 表单字段。
    pub ipv6_fields: [String; 5],
    /// 高级设置是否展开。
    pub advanced_open: bool,
    /// 高级设置选中项。
    pub advanced_selected: usize,
    /// 是否处于字段编辑态。
    pub editing: bool,
    /// 正在编辑的字段索引（0..5）。
    pub field_idx: usize,
    /// 编辑的是否为 IPv6 字段（否则 IPv4）。
    pub editing_v6: bool,
}

impl QuickSetState {
    /// 动态可聚焦行列表。
    fn focus_rows(&self, ipv6_on: bool) -> Vec<QsRow> {
        let mut rows = vec![QsRow::Ipv4Toggle];
        if self.ipv4_static {
            for i in 0..5 {
                rows.push(QsRow::Ipv4Field(i));
            }
        }
        rows.push(QsRow::Ipv6Toggle);
        if ipv6_on {
            for i in 0..5 {
                rows.push(QsRow::Ipv6Field(i));
            }
        }
        rows.push(QsRow::AdvancedToggle);
        if self.advanced_open {
            for i in 0..ADVANCED_ACTIONS.len() {
                rows.push(QsRow::AdvancedItem(i));
            }
        }
        rows
    }

    /// 当前焦点行。
    pub fn current_row(&self, ipv6_on: bool) -> QsRow {
        let rows = self.focus_rows(ipv6_on);
        rows.get(self.focus).copied().unwrap_or(QsRow::Ipv4Toggle)
    }

    /// 移动焦点（带越界收敛）。
    pub fn move_focus(&mut self, ipv6_on: bool, delta: i64) {
        let rows = self.focus_rows(ipv6_on);
        let n = rows.len();
        if n == 0 {
            return;
        }
        let cur = self.focus as i64;
        self.focus = ((cur + delta + n as i64) % n as i64) as usize;
    }
}

impl Default for QuickSetState {
    fn default() -> Self {
        Self {
            focus: 0,
            result: None,
            ipv4_static: false,
            ipv4_fields: Default::default(),
            ipv6_fields: Default::default(),
            advanced_open: false,
            advanced_selected: 0,
            editing: false,
            field_idx: 0,
            editing_v6: false,
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

/// 网工工具连接类型（V3.0 原则 P5：连前只选类型，连后识别厂商）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnType {
    /// 通用（串口 / SSH / Telnet）。
    Generic,
    /// eNSP 模拟器（本地管道 / Telnet / COM 映射）。
    Ensp,
}

impl ConnType {
    pub fn label(&self) -> &'static str {
        match self {
            ConnType::Generic => "通用",
            ConnType::Ensp => "eNSP",
        }
    }
}

/// 网工工具终端事件（后台读线程 / 连接结果回传）。
pub enum TermEvent {
    Echo(String),
    Connected {
        sess: std::sync::Arc<std::sync::Mutex<Box<dyn serialport::SerialPort>>>,
        vendor: Option<String>,
        model: Option<String>,
    },
    Failed(String),
}

/// 内置拓扑编辑器后端消息（WebSocket 前端 → TUI）。
pub enum EditorMsg {
    /// 前端实时推送的拓扑快照（每次编辑）。
    Update(crate::core::topology::Topology),
    /// 前端请求落盘保存。
    Save,
    /// 前端请求关闭编辑器服务。
    Close,
}

/// 模块三（网工终端）状态。
#[derive(Debug)]
pub struct TermState {
    pub conn: ConnState,
    pub conn_type: ConnType,
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
    /// 连后识别到的厂商 id（None=未识别）。
    pub detected_vendor: Option<String>,
    /// 连后识别到的设备型号。
    pub detected_model: Option<String>,
}

impl Default for TermState {
    fn default() -> Self {
        Self {
            conn: ConnState::Disconnected,
            conn_type: ConnType::Generic,
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
            detected_vendor: None,
            detected_model: None,
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

/// 单个拓扑条目（含预检结果缓存）。
#[derive(Debug)]
pub struct TopoEntry {
    pub name: String,
    pub topology: Topology,
    pub findings: Vec<design_check::Finding>,
}

/// 拓扑模块导航层级（V3.0 三级导航）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopoMode {
    /// 一级：拓扑列表（新建 / 导入 / 多拓扑）。
    List,
    /// 二级：操作菜单（打开编辑 / 重命名 / 复制 / 导出 / 删除）。
    Menu,
    /// 三级：拓扑详情（设备列表 + 预检常驻 + CLI）。
    Detail,
}

/// 模块四（拓扑图）状态。
#[derive(Debug)]
pub struct TopoState {
    pub entries: Vec<TopoEntry>,
    pub selected: usize,
    pub mode: TopoMode,
    /// 二级菜单选中项。
    pub menu_idx: usize,
    /// 三级详情中选中的设备。
    pub dev_idx: usize,
    pub cli: Option<String>,
    pub status: Option<String>,
    pub offset: usize,
    pub findings_offset: usize,
}

/// 二级菜单操作项。
pub const TOPO_MENU: [&str; 6] = [
    "打开编辑器",
    "设备与 CLI（下发）",
    "重命名",
    "复制一份",
    "导出（D2/SVG/CLI）",
    "删除",
];

impl TopoState {
    /// 当前选中的拓扑条目。
    pub fn current(&self) -> Option<&TopoEntry> {
        self.entries.get(self.selected)
    }

    /// 当前选中的拓扑条目（可变）。
    pub fn current_mut(&mut self) -> Option<&mut TopoEntry> {
        self.entries.get_mut(self.selected)
    }
}

impl Default for TopoState {
    fn default() -> Self {
        let topology = topology::demo_topology();
        let findings = design_check::check(
            &topology,
            &[
                design_check::Intent::UniqueSubnet,
                design_check::Intent::VlanPropagated {
                    vlan: 10,
                    to: DeviceRole::Access,
                },
                design_check::Intent::VlanPropagated {
                    vlan: 20,
                    to: DeviceRole::Access,
                },
                design_check::Intent::RedundantUplink {
                    role: DeviceRole::Access,
                },
                design_check::Intent::NoLoop,
            ],
        );
        Self {
            entries: vec![TopoEntry {
                name: "演示拓扑".to_string(),
                topology,
                findings,
            }],
            selected: 0,
            mode: TopoMode::List,
            menu_idx: 0,
            dev_idx: 0,
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
    /// 当前选中检查项（单项检测 / 修复 / 下钻作用域）。
    pub selected: usize,
    /// 是否处于下钻面板（详情抽屉）。
    pub drill_open: bool,
    /// 下钻面板选中的子动作。
    pub drill_selected: usize,
}

impl DiagState {
    fn reset(&mut self) {
        self.running = true;
        self.started = true;
        self.names = Diagnoser::all_check_names();
        self.results = vec![None; self.names.len()];
        self.logs.clear();
        self.summary = None;
        self.drill_open = false;
        self.drill_selected = 0;
    }
}

pub struct App {
    #[allow(dead_code)] // v0.2 供快捷设置模块读取
    pub config: Config,
    pub is_admin: bool,
    pub adapters: Vec<Adapter>,
    pub active_adapter: Option<Adapter>,
    pub env_ready: bool,
    pub ipv6_on: bool,
    /// 当前上网网卡的 DNS 服务器（下钻「DNS 测速」用）。
    pub current_dns: Vec<String>,
    /// 当前上网网卡的 MAC 地址（模块二只读展示）。
    pub current_mac: String,
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
    /// IME 是否已开启（默认禁用，F2 全局切换）。
    ime_on: bool,
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
    /// 内置拓扑编辑器（V3.0.2）：后端 WebSocket 服务与 TUI 的桥接。
    editor_tx: UnboundedSender<EditorMsg>,
    editor_rx: UnboundedReceiver<EditorMsg>,
    /// 编辑器服务监听端口（None=未启动）。
    editor_port: Option<u16>,
    /// 关闭信号（原子标志，供服务任务轮询）。
    editor_shutdown: Option<Arc<AtomicBool>>,
    /// 服务任务句柄（用于退出时 abort）。
    editor_handle: Option<JoinHandle<()>>,
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
        let (editor_tx, editor_rx) = tokio::sync::mpsc::unbounded_channel();

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
            ipv6_on: false,
            current_dns: Vec::new(),
            current_mac: String::new(),
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
            ime_on: false,
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
            editor_tx,
            editor_rx,
            editor_port: None,
            editor_shutdown: None,
            editor_handle: None,
            rt,
        }
    }

    /// 应用探测结果（刷新状态栏）。
    fn apply_probe(&mut self, p: NetProbe) {
        self.is_admin = p.is_admin;
        self.adapters = p.adapters.clone();
        self.active_adapter = p.active_adapter().cloned();
        self.ipv6_on = p.ipv6_enabled;
        self.current_dns = p.dns.clone();
        self.current_mac = p.mac.clone();
        self.env_ready = true;
    }

    pub fn run(&mut self) -> Result<()> {
        let mut terminal = ratatui::init();
        terminal.clear()?;
        // 非输入态禁用输入法（V3.0 P2）：单字母热键不被组字窗拦截。
        ime::disable_ime();

        while self.running {
            self.drain_events();
            self.drain_probe();
            self.drain_dns();
            self.drain_term();
            self.drain_editor();
            terminal.draw(|f| ui::draw(f, self))?;

            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.on_key(key);
                    }
                }
            }
        }

        ime::enable_ime();
        self.stop_editor();
        ratatui::restore();
        Ok(())
    }

    fn on_key(&mut self, key: KeyEvent) {
        let code = key.code;

        // 顶层：模块切换仅由 NavIntent 触发（Tab / Ctrl+Tab / Ctrl+← / Ctrl+→）。
        // 普通 ←/→ 下派给当前模块局部导航（见 ui/nav.rs）。
        match crate::ui::nav::nav_intent(code, key.modifiers) {
            NavIntent::ModuleNext => {
                self.tab = (self.tab + 1) % TABS.len();
                return;
            }
            NavIntent::ModulePrev => {
                self.tab = (self.tab + TABS.len() - 1) % TABS.len();
                return;
            }
            NavIntent::None => {}
        }

        match code {
            KeyCode::Char('q') | KeyCode::Char('Q') => self.running = false,
            KeyCode::Char('h') | KeyCode::Char('?') | KeyCode::F(1) => {
                self.show_help = !self.show_help
            }
            KeyCode::F(2) => self.toggle_ime(),
            KeyCode::Esc => {
                if self.show_help {
                    self.show_help = false;
                } else if self.diag.drill_open {
                    self.diag.drill_open = false;
                } else if self.dns.interactive {
                    self.dns.interactive = false;
                } else if self.quick_set.editing {
                    self.quick_set.editing = false;
                } else if self.term.input_mode {
                    self.term.input_mode = false;
                } else if self.tab == 3 {
                    match self.topo.mode {
                        TopoMode::Menu => self.topo.mode = TopoMode::List,
                        TopoMode::Detail => self.topo.mode = TopoMode::Menu,
                        TopoMode::List => {}
                    }
                }
            }
            KeyCode::Left => {
                if self.tab == 2 {
                    if self.term.conn == ConnState::Disconnected {
                        // 未连接：←/→ 切换连接类型（通用 / eNSP）
                        self.term.conn_type = match self.term.conn_type {
                            ConnType::Generic => ConnType::Ensp,
                            ConnType::Ensp => ConnType::Generic,
                        };
                    } else if !self.vendor_db.vendors().is_empty() {
                        // 已连接：←/→ 手动覆盖厂商
                        let n = self.vendor_db.vendors().len();
                        self.term.vendor_idx = (self.term.vendor_idx + n - 1) % n;
                        self.term.cmd_idx = 0;
                    }
                }
            }
            KeyCode::Right => {
                if self.tab == 2 {
                    if self.term.conn == ConnState::Disconnected {
                        self.term.conn_type = match self.term.conn_type {
                            ConnType::Generic => ConnType::Ensp,
                            ConnType::Ensp => ConnType::Generic,
                        };
                    } else if !self.vendor_db.vendors().is_empty() {
                        let n = self.vendor_db.vendors().len();
                        self.term.vendor_idx = (self.term.vendor_idx + 1) % n;
                        self.term.cmd_idx = 0;
                    }
                }
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
                0 => {
                    if self.diag.drill_open {
                        self.diag.drill_selected = self.diag.drill_selected.saturating_sub(1);
                    } else if self.diag.started && !self.diag.results.is_empty() {
                        let n = self.diag.results.len();
                        self.diag.selected = (self.diag.selected + n - 1) % n;
                    }
                }
                1 => {
                    if self.quick_set.editing {
                        if self.quick_set.field_idx > 0 {
                            self.quick_set.field_idx -= 1;
                        }
                    } else if self.dns.interactive {
                        let n = self.dns.results.len();
                        if n > 0 {
                            self.dns.selected = (self.dns.selected + n - 1) % n;
                        }
                    } else {
                        let on = self.ipv6_on;
                        self.quick_set.move_focus(on, -1);
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
                3 => self.topo_nav(-1),
                4 => self.backup.selected = self.backup.selected.saturating_sub(1),
                _ => {}
            },
            KeyCode::Down => match self.tab {
                0 => {
                    if self.diag.drill_open {
                        if self.diag.drill_selected + 1 < self.drill_count() {
                            self.diag.drill_selected += 1;
                        }
                    } else if self.diag.started && !self.diag.results.is_empty() {
                        let n = self.diag.results.len();
                        self.diag.selected = (self.diag.selected + 1) % n;
                    }
                }
                1 => {
                    if self.quick_set.editing {
                        if self.quick_set.field_idx + 1 < 5 {
                            self.quick_set.field_idx += 1;
                        }
                    } else if self.dns.interactive {
                        let n = self.dns.results.len();
                        if n > 0 {
                            self.dns.selected = (self.dns.selected + 1) % n;
                        }
                    } else {
                        let on = self.ipv6_on;
                        self.quick_set.move_focus(on, 1);
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
                3 => self.topo_nav(1),
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
                } else if self.tab == 3 && self.topo.mode == TopoMode::List {
                    self.topo_import();
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
                    self.execute_auto_fix_one(self.diag.selected);
                }
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                if self.tab == 0 {
                    self.trace_route();
                }
            }
            KeyCode::Char('g') | KeyCode::Char('G') => {
                if self.tab == 0 {
                    self.export_report();
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                if self.tab == 3 && self.topo.mode == TopoMode::List {
                    self.topo_new();
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
            KeyCode::Enter | KeyCode::Char('r') | KeyCode::Char('R') => match self.tab {
                0 => {
                    if self.diag.drill_open {
                        self.execute_drill();
                    } else if self.diag.started && !self.diag.running {
                        self.diag.drill_open = true;
                        self.diag.drill_selected = 0;
                    } else if !self.diag.running {
                        self.start_diag();
                    }
                }
                1 => self.on_quick_enter(),
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
                3 => self.topo_enter(),
                4 => {
                    let sel = self.backup.selected;
                    self.execute_backup(sel);
                }
                _ => {}
            },
            KeyCode::Backspace => {
                if self.tab == 1 && self.quick_set.editing {
                    self.quick_input_backspace();
                } else if self.tab == 2 && self.term.input_mode {
                    self.term.input.pop();
                }
            }
            KeyCode::Char(c) => {
                if self.tab == 1 && self.quick_set.editing {
                    // 表单输入：数字、点、冒号、十六进制字母
                    if c.is_ascii_digit() || c == '.' || c == ':' || "abcdefABCDEF".contains(c) {
                        self.quick_input_push(c);
                    }
                } else if self.tab == 2 && self.term.input_mode {
                    self.term.input.push(c);
                }
            }
            _ => {}
        }
    }

    /// F2：临时切换 IME 开/关（应付意外卡死）。
    fn toggle_ime(&mut self) {
        self.ime_on = !self.ime_on;
        if self.ime_on {
            ime::enable_ime();
            self.status_msg = Some("输入法已开启".into());
        } else {
            ime::disable_ime();
            self.status_msg = Some("输入法已禁用".into());
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
                (
                    o.success,
                    if o.success {
                        "已刷新 DNS 缓存".into()
                    } else {
                        o.combined()
                    },
                )
            }
            QuickAction::ReleaseRenew => {
                if iface.is_empty() {
                    (false, "未找到当前上网网卡".into())
                } else {
                    let o = net_set::release_renew(&iface);
                    (
                        o.success,
                        if o.success {
                            "已释放并重新获取 IP".into()
                        } else {
                            o.combined()
                        },
                    )
                }
            }
            QuickAction::TcpOptimize => {
                let o = net_set::tcp_optimize();
                (
                    o.success,
                    if o.success {
                        "TCP 已优化（自动调谐 + ECN）".into()
                    } else {
                        o.combined()
                    },
                )
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
                    // 本机启用 IPv6 才测 v6 候选，否则强制回落 ipv4（避免无 v6 机器测 v6 超时）
                    let prefer_ipv = if self.ipv6_on {
                        self.config.dns_preference.prefer_ipv.clone()
                    } else {
                        "ipv4".to_string()
                    };
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
                        let _ = tx.send(DnsUpdate {
                            results: ranked,
                            status,
                        });
                    });
                    (true, "已启动 DNS 优选测速（不会自动应用）".into())
                }
            }
        };

        let result = format!("{} {msg}", if ok { "✓" } else { "✗" });
        // 操作审计（记录到 logs/audit.log）
        crate::core::report::audit(&crate::config::app_root(), "快捷设置", &result);
        self.quick_set.result = Some(result);
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
        // 按协议写 v4 或 v6 DNS
        match best.protocol {
            dns::IpVersion::V6 => {
                let _ = net_set::set_dns_v6(&iface, &best.provider.primary);
                if !best.provider.secondary.is_empty() {
                    let _ = net_set::add_dns_v6(&iface, &best.provider.secondary);
                }
            }
            dns::IpVersion::V4 => {
                let _ = net_set::set_dns(&iface, &best.provider.primary);
                if !best.provider.secondary.is_empty() {
                    let _ = net_set::add_dns(&iface, &best.provider.secondary);
                }
            }
        }
        let msg = format!(
            "✓ 已应用 DNS {}（{} / {}）",
            best.provider.name, best.provider.primary, best.provider.secondary
        );
        self.quick_set.result = Some(msg.clone());
        self.dns.status = Some(format!(
            "已应用 {}，原 DNS 已备份可回退",
            best.provider.name
        ));
        self.dns.interactive = false;
    }

    /// 模块二：Enter 键分发（结构化面板）。
    fn on_quick_enter(&mut self) {
        // 编辑态：提交表单
        if self.quick_set.editing {
            self.submit_ip_form();
            return;
        }
        // DNS 优选交互模式
        if self.dns.interactive {
            self.execute_dns_apply();
            return;
        }
        let on = self.ipv6_on;
        match self.quick_set.current_row(on) {
            QsRow::Ipv4Toggle => self.toggle_ipv4_dhcp(),
            QsRow::Ipv4Field(i) => self.begin_edit(false, i),
            QsRow::Ipv6Toggle => self.toggle_ipv6(),
            QsRow::Ipv6Field(i) => self.begin_edit(true, i),
            QsRow::AdvancedToggle => {
                self.quick_set.advanced_open = !self.quick_set.advanced_open;
                self.quick_set.advanced_selected = 0;
            }
            QsRow::AdvancedItem(i) => {
                if let Some((_, action)) = ADVANCED_ACTIONS.get(i) {
                    self.execute_quick(*action);
                }
            }
        }
    }

    /// 进入字段编辑态。
    fn begin_edit(&mut self, v6: bool, idx: usize) {
        self.quick_set.editing = true;
        self.quick_set.editing_v6 = v6;
        self.quick_set.field_idx = idx;
    }

    /// 编辑态：输入一个字符。
    fn quick_input_push(&mut self, c: char) {
        let field = if self.quick_set.editing_v6 {
            &mut self.quick_set.ipv6_fields[self.quick_set.field_idx]
        } else {
            &mut self.quick_set.ipv4_fields[self.quick_set.field_idx]
        };
        if field.len() < 45 {
            field.push(c);
        }
    }

    /// 编辑态：删除一个字符。
    fn quick_input_backspace(&mut self) {
        let field = if self.quick_set.editing_v6 {
            &mut self.quick_set.ipv6_fields[self.quick_set.field_idx]
        } else {
            &mut self.quick_set.ipv4_fields[self.quick_set.field_idx]
        };
        field.pop();
    }

    /// IPv4 静态 / DHCP 切换。
    fn toggle_ipv4_dhcp(&mut self) {
        let iface = self
            .active_adapter
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_default();
        if iface.is_empty() {
            self.quick_set.result = Some("✗ 未找到当前上网网卡".into());
            return;
        }
        let _ = net_set::backup_network(&crate::config::app_root());
        if self.quick_set.ipv4_static {
            let o = net_set::set_ip_dhcp(&iface);
            if o.success {
                self.quick_set.ipv4_static = false;
                self.quick_set.result = Some("✓ IPv4 已切回 DHCP 自动获取".into());
            } else {
                self.quick_set.result = Some(format!("✗ {}", o.combined()));
            }
        } else {
            self.quick_set.ipv4_static = true;
            self.quick_set.result = Some("已切换为静态 IP：填字段后 Enter 应用".into());
        }
    }

    /// IPv6 开启 / 关闭切换。
    fn toggle_ipv6(&mut self) {
        let o = net_set::set_ipv6(!self.ipv6_on);
        if o.success {
            self.ipv6_on = !self.ipv6_on;
        }
        let target = if self.ipv6_on { "启用" } else { "禁用" };
        self.quick_set.result = Some(if o.success {
            format!("✓ IPv6 已切换为{target}")
        } else {
            format!("✗ {}", o.combined())
        });
    }

    /// 提交 IPv4 或 IPv6 静态表单。
    fn submit_ip_form(&mut self) {
        let iface = self
            .active_adapter
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_default();
        if iface.is_empty() {
            self.quick_set.result = Some("✗ 未找到当前上网网卡".into());
            self.quick_set.editing = false;
            return;
        }
        if self.quick_set.editing_v6 {
            // IPv6 表单：目前仅支持写 v6 DNS（静态 v6 地址需 slaac/前缀，暂简化为 DNS 落地）
            let dns1 = self.quick_set.ipv6_fields[3].clone();
            let dns2 = self.quick_set.ipv6_fields[4].clone();
            if dns1.is_empty() {
                self.quick_set.result = Some("✗ 请至少填写 IPv6 主 DNS".into());
                return;
            }
            let _ = net_set::backup_network(&crate::config::app_root());
            let _ = net_set::set_dns_v6(&iface, &dns1);
            if !dns2.is_empty() {
                let _ = net_set::add_dns_v6(&iface, &dns2);
            }
            let suffix = if dns2.is_empty() {
                String::new()
            } else {
                format!(" / {dns2}")
            };
            self.quick_set.result = Some(format!("✓ 已设置 IPv6 DNS {dns1}{suffix}"));
            self.quick_set.editing = false;
        } else {
            let gw = self.quick_set.ipv4_fields[0].clone();
            let mask = self.quick_set.ipv4_fields[1].clone();
            let ip = self.quick_set.ipv4_fields[2].clone();
            let dns1 = self.quick_set.ipv4_fields[3].clone();
            let dns2 = self.quick_set.ipv4_fields[4].clone();
            if ip.is_empty() || mask.is_empty() {
                self.quick_set.result = Some("✗ 请至少填写 IP 地址和子网掩码".into());
                return;
            }
            let _ = net_set::backup_network(&crate::config::app_root());
            let o =
                net_set::set_static_ip(&iface, &ip, &mask, if gw.is_empty() { "" } else { &gw });
            if o.success {
                if !dns1.is_empty() {
                    let _ = net_set::set_dns(&iface, &dns1);
                }
                if !dns2.is_empty() {
                    let _ = net_set::add_dns(&iface, &dns2);
                }
                self.quick_set.result = Some(format!(
                    "✓ 已设置静态 IP {ip}/{mask} 网关 {}",
                    if gw.is_empty() { "无" } else { &gw }
                ));
                self.quick_set.editing = false;
            } else {
                self.quick_set.result = Some(format!("✗ {}", o.combined()));
            }
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
                                // 连后识别厂商型号（V3.0 P5）
                                let detected = serial::detect_vendor(&mut sess);
                                // 打开持久会话
                                if let Ok(port) = serialport::new(&p.name, baud)
                                    .timeout(Duration::from_millis(200))
                                    .open()
                                {
                                    let arc = std::sync::Arc::new(std::sync::Mutex::new(port));
                                    let (vendor, model) = match detected {
                                        Some((v, m)) => (Some(v), Some(m)),
                                        None => (None, None),
                                    };
                                    let _ = tx.send(TermEvent::Connected {
                                        sess: arc.clone(),
                                        vendor,
                                        model,
                                    });
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
                                            let text =
                                                String::from_utf8_lossy(&buf[..n]).to_string();
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
                TermEvent::Connected {
                    sess,
                    vendor,
                    model,
                } => {
                    self.session = Some(sess);
                    self.term.conn = ConnState::Connected;
                    self.term.detected_vendor = vendor.clone();
                    self.term.detected_model = model.clone();
                    if let Some(v) = &vendor {
                        // 自动切 CLI 到识别厂商
                        if let Some(idx) =
                            self.vendor_db.vendors().iter().position(|x| &x.vendor == v)
                        {
                            self.term.vendor_idx = idx;
                            self.term.cmd_idx = 0;
                        }
                        let model_txt = model.clone().unwrap_or_else(|| "未知型号".to_string());
                        self.term.status = Some(format!("已连接，自动识别为 {v}（{model_txt}）"));
                    } else {
                        self.term.status =
                            Some("已连接，未能识别型号，请用 [ / ] 手动选择厂商".into());
                    }
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

    /// 模块四：三级导航（↑/↓）。
    fn topo_nav(&mut self, delta: i64) {
        match self.topo.mode {
            TopoMode::List => {
                let n = self.topo.entries.len();
                if n > 0 {
                    let cur = self.topo.selected as i64;
                    self.topo.selected = ((cur + delta + n as i64) % n as i64) as usize;
                }
            }
            TopoMode::Menu => {
                let n = TOPO_MENU.len();
                let cur = self.topo.menu_idx as i64;
                self.topo.menu_idx = ((cur + delta + n as i64) % n as i64) as usize;
            }
            TopoMode::Detail => {
                if let Some(e) = self.topo.current() {
                    let n = e.topology.devices.len();
                    if n > 0 {
                        let cur = self.topo.dev_idx as i64;
                        self.topo.dev_idx = ((cur + delta + n as i64) % n as i64) as usize;
                    }
                }
            }
        }
    }

    /// 模块四：Enter 键（按层级分发）。
    fn topo_enter(&mut self) {
        match self.topo.mode {
            TopoMode::List => {
                if !self.topo.entries.is_empty() {
                    self.topo.mode = TopoMode::Menu;
                    self.topo.menu_idx = 0;
                }
            }
            TopoMode::Menu => {
                let idx = self.topo.menu_idx;
                self.topo_menu_action(idx);
            }
            TopoMode::Detail => self.topo_gen_cli(),
        }
    }

    /// 模块四：执行二级菜单动作。
    fn topo_menu_action(&mut self, idx: usize) {
        match idx {
            // 打开编辑器：直接启动网页编辑器（不再进入设备列表）
            0 => self.topo_open_editor(),
            // 设备与 CLI：进入三级详情（CLI 推导 + 下发）
            1 => {
                self.topo.mode = TopoMode::Detail;
                self.topo.dev_idx = 0;
            }
            2 => self.topo_rename(),
            3 => self.topo_copy(),
            4 => self.topo_export_d2(),
            5 => self.topo_delete(),
            _ => {}
        }
    }

    /// 新建拓扑（demo 骨架）。
    fn topo_new(&mut self) {
        let name = format!("拓扑{}", self.topo.entries.len() + 1);
        let t = topology::demo_topology();
        let findings = self.check_topo(&t);
        self.topo.entries.push(TopoEntry {
            name,
            topology: t,
            findings,
        });
        self.topo.selected = self.topo.entries.len() - 1;
        self.topo.status = Some("已新建拓扑（O 打开编辑器编辑）".into());
    }

    /// 导入 topology.json 为新的拓扑条目。
    fn topo_import(&mut self) {
        let json_path = crate::config::app_root().join("topology.json");
        match std::fs::read_to_string(&json_path) {
            Ok(s) => match serde_json::from_str::<Topology>(&s) {
                Ok(t) => {
                    let name = format!("导入拓扑{}", self.topo.entries.len() + 1);
                    let findings = self.check_topo(&t);
                    self.topo.entries.push(TopoEntry {
                        name,
                        topology: t,
                        findings,
                    });
                    self.topo.selected = self.topo.entries.len() - 1;
                    self.topo.status = Some("已导入 topology.json".into());
                }
                Err(e) => self.topo.status = Some(format!("解析失败: {e}")),
            },
            Err(_) => self.topo.status = Some("未找到 topology.json（先 O 打开编辑器导出）".into()),
        }
    }

    /// 重命名当前拓扑（简化：加后缀；完整重命名在编辑器）。
    fn topo_rename(&mut self) {
        if let Some(e) = self.topo.current_mut() {
            let old = e.name.clone();
            e.name = format!("{old}·改");
            self.topo.status = Some(format!("已重命名「{old}」→「{}」", e.name));
        }
    }

    /// 复制当前拓扑。
    fn topo_copy(&mut self) {
        if let Some(e) = self.topo.current() {
            let copy = TopoEntry {
                name: format!("{} 副本", e.name),
                topology: e.topology.clone(),
                findings: e.findings.clone(),
            };
            self.topo.entries.push(copy);
            self.topo.selected = self.topo.entries.len() - 1;
            self.topo.status = Some("已复制拓扑".into());
        }
    }

    /// 删除当前拓扑（至少保留一个）。
    fn topo_delete(&mut self) {
        if self.topo.entries.len() <= 1 {
            self.topo.status = Some("至少保留一个拓扑".into());
            return;
        }
        let name = self
            .topo
            .current()
            .map(|e| e.name.clone())
            .unwrap_or_default();
        self.topo.entries.remove(self.topo.selected);
        if self.topo.selected >= self.topo.entries.len() {
            self.topo.selected = self.topo.entries.len().saturating_sub(1);
        }
        self.topo.mode = TopoMode::List;
        self.topo.status = Some(format!("已删除「{name}」"));
    }

    /// 预检辅助：对拓扑跑 design_check。
    fn check_topo(&self, t: &Topology) -> Vec<design_check::Finding> {
        design_check::check(
            t,
            &[
                design_check::Intent::UniqueSubnet,
                design_check::Intent::VlanPropagated {
                    vlan: 10,
                    to: DeviceRole::Access,
                },
                design_check::Intent::VlanPropagated {
                    vlan: 20,
                    to: DeviceRole::Access,
                },
                design_check::Intent::RedundantUplink {
                    role: DeviceRole::Access,
                },
                design_check::Intent::NoLoop,
            ],
        )
    }

    /// 模块四：生成选中设备的 CLI。
    fn topo_gen_cli(&mut self) {
        let Some(e) = self.topo.current() else {
            return;
        };
        let Some(d) = e.topology.devices.get(self.topo.dev_idx) else {
            return;
        };
        let cli = topo_cli::generate_device_cli(d, &e.topology);
        self.topo.status = Some(format!("已生成 {} 的 CLI", d.name));
        self.topo.cli = Some(cli);
    }

    /// 模块四：导出 D2 并尝试渲染 SVG。
    fn topo_export_d2(&mut self) {
        let Some(e) = self.topo.current() else {
            return;
        };
        let d2 = e.topology.export_d2();
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

    /// 模块四：打开内置拓扑编辑器（NMTs 作后端，浏览器动态页面实时同步，零依赖）。
    fn topo_open_editor(&mut self) {
        let Some(e) = self.topo.current() else {
            return;
        };
        let initial = e.topology.clone();

        // 已在运行：直接再开一个浏览器标签指向既有服务。
        if let Some(port) = self.editor_port {
            Self::open_browser(port);
            self.topo.status = Some("编辑器已在运行，已为你打开页面".into());
            return;
        }

        match crate::web::start_editor(initial, self.editor_tx.clone(), self.rt.clone()) {
            Ok(srv) => {
                self.editor_port = Some(srv.port);
                self.editor_shutdown = Some(srv.shutdown);
                self.editor_handle = Some(srv.handle);
                Self::open_browser(srv.port);
                self.topo.status = Some(format!(
                    "已启动内置拓扑编辑器 → http://127.0.0.1:{}/ （编辑实时同步回 NMTs）",
                    srv.port
                ));
            }
            Err(err) => {
                self.topo.status = Some(format!("编辑器启动失败：{err}"));
            }
        }
    }

    /// 用系统默认浏览器打开编辑器页面（Windows）。
    fn open_browser(port: u16) {
        let url = format!("http://127.0.0.1:{port}/");
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", "", &url])
            .spawn();
    }

    /// 拉取编辑器后端消息（实时拓扑同步 / 保存 / 关闭）。
    fn drain_editor(&mut self) {
        while let Ok(msg) = self.editor_rx.try_recv() {
            match msg {
                EditorMsg::Update(t) => {
                    let findings = self.check_topo(&t);
                    if let Some(e) = self.topo.current_mut() {
                        e.topology = t.clone();
                        e.findings = findings;
                    }
                    self.topo.status = Some("编辑器已实时同步到 NMTs（预检已更新）".into());
                }
                EditorMsg::Save => {
                    if let Some(e) = self.topo.current() {
                        let json = serde_json::to_string_pretty(&e.topology).unwrap_or_default();
                        let path = crate::config::app_root().join("topology.json");
                        if std::fs::write(&path, &json).is_ok() {
                            self.topo.status = Some(format!("已保存拓扑到 {}", path.display()));
                        } else {
                            self.topo.status = Some("保存失败：无法写入 topology.json".into());
                        }
                    }
                }
                EditorMsg::Close => self.stop_editor(),
            }
        }
    }

    /// 停止内置编辑器服务（收到前端 close 或退出程序时调用）。
    fn stop_editor(&mut self) {
        if let Some(sd) = self.editor_shutdown.take() {
            sd.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        if let Some(h) = self.editor_handle.take() {
            h.abort();
        }
        if self.editor_port.is_some() {
            self.editor_port = None;
            self.topo.status = Some("已关闭拓扑编辑器".into());
        }
    }

    /// 模块四：回读 topology.json（更新当前拓扑 + 重新预检）。
    fn topo_load_json(&mut self) {
        let json_path = crate::config::app_root().join("topology.json");
        match std::fs::read_to_string(&json_path) {
            Ok(s) => match serde_json::from_str::<Topology>(&s) {
                Ok(t) => {
                    let findings = self.check_topo(&t);
                    if let Some(e) = self.topo.current_mut() {
                        e.topology = t;
                        e.findings = findings;
                    }
                    self.topo.dev_idx = 0;
                    self.topo.cli = None;
                    self.topo.status = Some("已回读 topology.json 并重新预检".into());
                }
                Err(e) => self.topo.status = Some(format!("解析 topology.json 失败: {e}")),
            },
            Err(_) => self.topo.status = Some("未找到 topology.json，请先打开编辑器保存".into()),
        }
    }

    /// 报告导出：诊断结果导出为 Markdown。
    fn export_report(&mut self) {
        if !self.diag.started {
            self.diag.summary = Some("请先运行诊断".into());
            return;
        }
        let results: Vec<CheckResult> =
            self.diag.results.iter().filter_map(|r| r.clone()).collect();
        let summary = self.diag.summary.clone().unwrap_or_default();
        let md = crate::core::report::diag_report_md(&results, &summary, &[]);
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let filename = format!("diag_{ts}.md");
        match crate::core::report::write_report(&crate::config::app_root(), &filename, &md) {
            Ok(p) => self.diag.summary = Some(format!("报告已导出：{}", p.display())),
            Err(e) => self.diag.summary = Some(format!("导出失败：{e}")),
        }
    }

    /// 诊断实时化：路由追踪到公网（异步，结果追加到日志）。
    fn trace_route(&mut self) {
        let tx = self.tx.clone();
        self.diag
            .logs
            .push("路由追踪：tracert 223.5.5.5（最多 8 跳）…".into());
        self.rt.spawn(async move {
            let hops = crate::core::net_diag::traceroute("223.5.5.5").await;
            for h in hops {
                let _ = tx.send(DiagEvent::Log(format!("  {h}")));
            }
        });
    }

    /// 诊断→修复闭环：仅修复选中项（V3.0 分层）。
    fn execute_auto_fix_one(&mut self, index: usize) {
        let Some(r) = self.diag.results.get(index).and_then(|r| r.as_ref()) else {
            return;
        };
        let Some(cmd) = r.fix.as_ref().and_then(|f| match &f.kind {
            crate::core::net_diag::FixKind::Auto(cmd) => Some(cmd.clone()),
            _ => None,
        }) else {
            self.diag.summary = Some(format!("「{}」无可自动修复项", r.name));
            return;
        };
        let out = crate::windows::run("cmd", &["/c", &cmd], std::time::Duration::from_secs(30));
        self.diag.logs.push(format!("执行：{cmd}"));
        if out.success {
            self.diag.summary = Some(format!("已修复「{}」", r.name));
        } else {
            let first = out.combined().lines().next().unwrap_or("失败").to_string();
            self.diag.logs.push(format!("  ✗ {first}"));
            self.diag.summary = Some(format!("修复「{}」失败", r.name));
        }
    }

    /// 当前选中项的子动作数量。
    fn drill_count(&self) -> usize {
        self.diag
            .results
            .get(self.diag.selected)
            .and_then(|r| r.as_ref())
            .map(|r| r.drill.len())
            .unwrap_or(0)
    }

    /// 执行下钻面板选中的子动作。
    fn execute_drill(&mut self) {
        let Some(action) = self
            .diag
            .results
            .get(self.diag.selected)
            .and_then(|r| r.as_ref())
            .and_then(|r| r.drill.get(self.diag.drill_selected))
            .copied()
        else {
            self.diag.drill_open = false;
            return;
        };
        use crate::core::net_diag::DrillAction;
        match action {
            DrillAction::Fix => {
                let idx = self.diag.selected;
                self.execute_auto_fix_one(idx);
                self.diag.drill_open = false;
            }
            DrillAction::TraceRoute => {
                self.diag.drill_open = false;
                self.trace_route();
            }
            DrillAction::DnsSpeedTest => {
                self.diag.drill_open = false;
                self.start_dns_speed_test();
            }
            DrillAction::DnsOptimize => {
                self.diag.drill_open = false;
                self.tab = 1;
                self.quick_set.result = None;
                self.execute_quick(QuickAction::DnsOptimize);
            }
        }
    }

    /// DNS 测速（下钻动作）：测当前 DNS 各服务器延迟，追加到日志。
    fn start_dns_speed_test(&mut self) {
        let dns_list = self.current_dns.clone();
        if dns_list.is_empty() {
            self.diag.summary = Some("未获取到当前 DNS".into());
            return;
        }
        self.diag
            .logs
            .push(format!("DNS 测速：{}", dns_list.join("、")));
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            for d in dns_list {
                match dns::ping_latency(&d).await {
                    Some(ms) => {
                        let _ = tx.send(DiagEvent::Log(format!("  {d} → {ms}ms")));
                    }
                    None => {
                        let _ = tx.send(DiagEvent::Log(format!("  {d} → 超时")));
                    }
                }
            }
        });
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
