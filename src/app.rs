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
use crate::core::{serial, vendor_cli::VendorDb};
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
    pub show_help: bool,
    pub status_msg: Option<String>,
    running: bool,
    tx: UnboundedSender<DiagEvent>,
    rx: UnboundedReceiver<DiagEvent>,
    probe_tx: UnboundedSender<NetProbe>,
    probe_rx: UnboundedReceiver<NetProbe>,
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
            show_help: false,
            status_msg: None,
            running: true,
            tx,
            rx,
            probe_tx,
            probe_rx,
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
                _ => {}
            },
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
