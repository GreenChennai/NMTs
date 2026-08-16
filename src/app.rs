//! 全局状态机：TUI 事件循环、模块路由、诊断事件分发。

use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::config::Config;
use crate::core::net_diag::{CheckResult, DiagEvent, Diagnoser};
use crate::ui;
use crate::windows::adapter::Adapter;

/// 五大模块。
pub const TABS: [&str; 5] = ["网络诊断", "快捷设置", "网工工具", "拓扑图", "配置备份"];

/// 模块一诊断界面状态。
#[derive(Debug, Default)]
pub struct DiagState {
    pub running: bool,
    pub started: bool,
    pub names: Vec<String>,
    pub results: Vec<Option<CheckResult>>,
    pub logs: Vec<String>,
    pub summary: Option<String>,
    pub log_scroll: usize,
}

impl DiagState {
    fn reset(&mut self) {
        self.running = true;
        self.started = true;
        self.names = Diagnoser::basic_check_names();
        self.results = vec![None; self.names.len()];
        self.logs.clear();
        self.summary = None;
        self.log_scroll = 0;
    }
}

pub struct App {
    #[allow(dead_code)] // v0.1 骨架：供 v0.2+ 模块读取配置使用
    pub config: Config,
    pub is_admin: bool,
    pub adapters: Vec<Adapter>,
    pub active_adapter: Option<Adapter>,
    pub tab: usize,
    pub diag: DiagState,
    pub show_help: bool,
    pub status_msg: Option<String>,
    running: bool,
    tx: UnboundedSender<DiagEvent>,
    rx: UnboundedReceiver<DiagEvent>,
    rt: Handle,
}

impl App {
    pub fn new(
        config: Config,
        rt: Handle,
        tx: UnboundedSender<DiagEvent>,
        rx: UnboundedReceiver<DiagEvent>,
    ) -> Self {
        let mut app = Self {
            config,
            is_admin: false,
            adapters: Vec::new(),
            active_adapter: None,
            tab: 0,
            diag: DiagState::default(),
            show_help: false,
            status_msg: None,
            running: true,
            tx,
            rx,
            rt,
        };
        app.probe();
        app
    }

    /// 启动时探测环境（网卡 / 管理员）。
    fn probe(&mut self) {
        let d = Diagnoser::new();
        self.is_admin = d.ctx.is_admin;
        self.adapters = d.ctx.adapters;
        self.active_adapter = d.ctx.active_adapter;
    }

    pub fn run(&mut self) -> Result<()> {
        let mut terminal = ratatui::init();
        terminal.clear()?;

        while self.running {
            self.drain_events();
            terminal.draw(|f| ui::draw(f, self))?;

            if event::poll(Duration::from_millis(100))? {
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
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('j') if !self.show_help => {
                self.tab = (self.tab + TABS.len() - 1) % TABS.len();
            }
            KeyCode::Enter | KeyCode::Char('r') | KeyCode::Char('R') => {
                if self.tab == 0 && !self.diag.running {
                    self.start_diag();
                }
            }
            _ => {}
        }
    }

    /// 启动诊断（在 tokio 运行时后台执行，事件经 mpsc 回传）。
    fn start_diag(&mut self) {
        let d = Diagnoser::new();
        self.is_admin = d.ctx.is_admin;
        self.adapters = d.ctx.adapters.clone();
        self.active_adapter = d.ctx.active_adapter.clone();
        self.diag.reset();
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let _ = d.run_basic(tx).await;
        });
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
                    self.diag.log_scroll = self.diag.logs.len().saturating_sub(1);
                }
                DiagEvent::Finished { summary } => {
                    self.diag.summary = Some(summary);
                    self.diag.running = false;
                }
            }
        }
    }
}
