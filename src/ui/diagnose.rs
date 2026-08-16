//! 模块一：网络诊断界面（步骤清单 + 实时日志）。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::core::net_diag::Status;
use crate::ui::widgets::scroll_list;
use crate::ui::status_icon;

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(2),  // 提示行
        Constraint::Min(5),     // 列表 + 日志
    ])
    .split(area);

    // 提示行
    let hint = if app.diag.running {
        Span::styled(" 诊断进行中…", Style::default().fg(Color::Yellow))
    } else if app.diag.started {
        Span::styled(
            " 诊断完成，按 R 重新运行 · F 自动修复 · T 路由追踪",
            Style::default().fg(Color::DarkGray),
        )
    } else {
        Span::styled(" 按 Enter / R 开始诊断 · T 路由追踪", Style::default().fg(Color::Cyan))
    };
    f.render_widget(Paragraph::new(Line::from(hint)), chunks[0]);

    let body = Layout::horizontal([
        Constraint::Percentage(55),
        Constraint::Percentage(45),
    ])
    .split(chunks[1]);

    draw_check_list(f, app, body[0]);
    let right = Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(body[1]);
    draw_log(f, app, right[0]);
    draw_adapters(f, app, right[1]);
}

fn draw_check_list(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<String> = if !app.diag.started {
        vec![
            " 尚未运行诊断。".to_string(),
            "".to_string(),
            " 将检测：网卡判定 / DHCP·IP / 默认路由 / 网关连通 / DNS / 代理 / 虚拟网卡干扰".to_string(),
            "        驱动 / 物理链路 / MTU / 外网连通 / 病毒 / 环路 / MAC 锁".to_string(),
        ]
    } else {
        let mut list = Vec::new();
        for (i, name) in app.diag.names.iter().enumerate() {
            let result = app.diag.results.get(i).and_then(|r| r.as_ref());
            let (icon, status_txt, detail) = match result {
                Some(r) => (status_icon(r.status), r.status.label().to_string(), r.detail.clone()),
                None => (status_icon(Status::Running), "检测中…".to_string(), String::new()),
            };
            let fix_txt = result
                .and_then(|r| r.fix.as_ref())
                .map(|f| format!("  → {}", f.label))
                .unwrap_or_default();
            list.push(format!("{icon} {name} [{status_txt}] {detail}{fix_txt}"));
        }
        if let Some(summary) = &app.diag.summary {
            list.push(String::new());
            list.push(format!("▶ {summary}"));
        }
        list
    };

    scroll_list(
        f,
        area,
        Block::default().borders(Borders::ALL).title(" 诊断步骤清单 "),
        &items,
        app.diag.offset.min(items.len().saturating_sub(1)),
        &mut app.diag.offset,
    );
}

fn draw_log(f: &mut Frame, app: &App, area: Rect) {
    let logs = &app.diag.logs;
    let visible = 15usize;
    let start = logs.len().saturating_sub(visible);
    let lines: Vec<Line> = if logs.is_empty() {
        vec![Line::from(Span::styled(
            " （实时日志将显示命令回显）",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        logs[start..]
            .iter()
            .map(|l| Line::from(Span::styled(format!(" › {l}"), Style::default().fg(Color::Gray))))
            .collect()
    };

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" 实时日志 "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

/// 多网卡列表（多网卡场景可见，虚拟 / VPN 标记）。
fn draw_adapters(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    if app.adapters.is_empty() {
        lines.push(Line::from(Span::styled(" （未检测到网卡）", Style::default().fg(Color::DarkGray))));
    } else {
        for a in app.adapters.iter().take(8) {
            let color = if a.is_virtual() { Color::Yellow } else { Color::White };
            let mark = if a.is_virtual() { format!(" [{}]", a.kind_label()) } else { String::new() };
            let up = if a.is_up() { "↑" } else { "↓" };
            lines.push(Line::from(vec![
                Span::styled(format!(" {up} "), Style::default().fg(if a.is_up() { Color::Green } else { Color::DarkGray })),
                Span::styled(a.name.clone(), Style::default().fg(color)),
                Span::styled(mark, Style::default().fg(Color::Yellow)),
            ]));
        }
    }

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" 网卡列表 "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}
