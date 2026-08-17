//! 模块四：拓扑图工具（V3.0 多拓扑 + 三级导航 + 预检常驻）。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, TopoMode, TOPO_MENU};
use crate::core::design_check::Severity;
use crate::ui::widgets::scroll_list;

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(4)]).split(area);
    draw_hint(f, app, chunks[0]);
    match app.topo.mode {
        TopoMode::List => draw_list_mode(f, app, chunks[1]),
        TopoMode::Menu => draw_menu_mode(f, app, chunks[1]),
        TopoMode::Detail => draw_detail_mode(f, app, chunks[1]),
    }
}

fn draw_hint(f: &mut Frame, app: &App, area: Rect) {
    let hint = match app.topo.mode {
        TopoMode::List => " N 新建 · I 导入 · ↑/↓ 选拓扑 · Enter 打开菜单",
        TopoMode::Menu => " ↑/↓ 选操作 · Enter 执行 · Esc 返回",
        TopoMode::Detail => {
            " ↑/↓ 选设备 · Enter 生成 CLI · E 导出 · O 编辑器 · B 回读 · D 下发 · Esc 返回"
        }
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
}

/// 一级：拓扑列表 + 预检常驻。
fn draw_list_mode(f: &mut Frame, app: &mut App, area: Rect) {
    let body =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    draw_topo_list(f, app, body[0]);
    draw_findings(f, app, body[1]);
}

/// 二级：拓扑列表 + 操作菜单。
fn draw_menu_mode(f: &mut Frame, app: &mut App, area: Rect) {
    let body =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    draw_topo_list(f, app, body[0]);
    draw_menu(f, app, body[1]);
}

/// 三级：设备列表 + 预检常驻 + CLI 预览。
fn draw_detail_mode(f: &mut Frame, app: &mut App, area: Rect) {
    let body = Layout::horizontal([
        Constraint::Percentage(34),
        Constraint::Percentage(34),
        Constraint::Percentage(32),
    ])
    .split(area);
    draw_devices(f, app, body[0]);
    draw_findings(f, app, body[1]);
    draw_cli(f, app, body[2]);
}

fn draw_topo_list(f: &mut Frame, app: &mut App, area: Rect) {
    let mut items: Vec<String> = vec![
        " [新建拓扑图]  (N)".to_string(),
        " [导入拓扑图]  (I)".to_string(),
    ];
    for (i, e) in app.topo.entries.iter().enumerate() {
        let mark = if i == app.topo.selected { "▶ " } else { "  " };
        items.push(format!("{mark}{}", e.name));
    }

    // 列表选中索引：前两项为伪按钮，entries 从 +2 开始
    let selected = app.topo.selected + 2;
    scroll_list(
        f,
        area,
        Block::default()
            .borders(Borders::ALL)
            .title(" 拓扑图（多拓扑） "),
        &items,
        selected,
        &mut app.topo.offset,
    );
}

fn draw_menu(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<String> = TOPO_MENU.iter().map(|s| s.to_string()).collect();
    scroll_list(
        f,
        area,
        Block::default().borders(Borders::ALL).title(" 操作菜单 "),
        &items,
        app.topo.menu_idx,
        &mut app.topo.offset,
    );
}

fn draw_devices(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(e) = app.topo.current() else {
        return;
    };
    let mut items: Vec<String> = e
        .topology
        .devices
        .iter()
        .map(|d| format!(" {} [{}·{}]", d.name, d.vendor.label(), d.role.label()))
        .collect();

    items.push(String::new());
    items.push(" 链路：".to_string());
    for l in &e.topology.links {
        let mut line = format!("   {} ↔ {}", l.from, l.to);
        if !l.from_ip.is_empty() {
            line.push_str(&format!("  {}", l.from_ip));
        }
        items.push(line);
    }

    scroll_list(
        f,
        area,
        Block::default().borders(Borders::ALL).title(" 拓扑设备 "),
        &items,
        app.topo.dev_idx,
        &mut app.topo.offset,
    );
}

fn draw_findings(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<String> = match app.topo.current() {
        Some(e) if e.findings.is_empty() => vec![" 预检通过，无问题。".to_string()],
        Some(e) => e
            .findings
            .iter()
            .map(|f| {
                let icon = match f.severity {
                    Severity::Error => "✗",
                    Severity::Warn => "!",
                    Severity::Info => "i",
                };
                format!("{icon} {}  ↳ {}", f.message, f.suggestion)
            })
            .collect(),
        None => vec![" （无拓扑）".to_string()],
    };

    scroll_list(
        f,
        area,
        Block::default()
            .borders(Borders::ALL)
            .title(" 预检问题清单（常驻） "),
        &items,
        app.topo.findings_offset.min(items.len().saturating_sub(1)),
        &mut app.topo.findings_offset,
    );
}

fn draw_cli(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    if let Some(e) = app.topo.current() {
        lines.push(Line::from(Span::styled(
            format!(" 拓扑：{}", e.name),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        if let Some(d) = e.topology.devices.get(app.topo.dev_idx) {
            lines.push(Line::from(Span::styled(
                format!(" 设备：{}（{}）", d.name, d.vendor.label()),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
        }
    }

    if let Some(cli) = &app.topo.cli {
        for l in cli.lines() {
            lines.push(Line::from(Span::styled(
                format!(" {l}"),
                Style::default().fg(Color::White),
            )));
        }
    } else {
        lines.push(Line::from(Span::styled(
            " 按 Enter 生成选中设备的 CLI",
            Style::default().fg(Color::DarkGray),
        )));
    }

    if let Some(s) = &app.topo.status {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(" {s}"),
            Style::default().fg(Color::Green),
        )));
    }

    let p = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" CLI 预览 / 导出 "),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}
