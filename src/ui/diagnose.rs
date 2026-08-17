//! 模块一：网络诊断界面（V3.0 分层：全部体检 + 单项列表 + 下钻面板）。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::core::net_diag::{FixKind, Status};
use crate::ui::widgets::scroll_list;
use crate::ui::{status_color, status_icon};

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(2), // 全部体检按钮 + 提示 + 分隔
        Constraint::Min(4),    // 主体
    ])
    .split(area);

    draw_top(f, app, chunks[0]);

    let body = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[1]);

    draw_check_list(f, app, body[0]);

    if app.diag.drill_open {
        draw_drill_panel(f, app, body[1]);
    } else {
        let right = Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(body[1]);
        draw_log(f, app, right[0]);
        draw_adapters(f, app, right[1]);
    }
}

/// 顶部：全部体检按钮 + 状态提示 + 分隔线。
fn draw_top(f: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);

    let btn = Span::styled(
        " [ 全部体检 ] ",
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    let hint = if app.diag.running {
        Span::styled("  诊断进行中…", Style::default().fg(Color::Yellow))
    } else if app.diag.started {
        Span::styled(
            "  R 重新体检 · ↑/↓ 选单项 · Enter 下钻 · F 修复选中项 · T 追踪 · G 导出",
            Style::default().fg(Color::DarkGray),
        )
    } else {
        Span::styled(
            "  按 Enter / R 开始全部体检",
            Style::default().fg(Color::Cyan),
        )
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![btn, hint])).wrap(Wrap { trim: true }),
        rows[0],
    );

    let sep = "─".repeat(area.width as usize);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            sep,
            Style::default().fg(Color::DarkGray),
        ))),
        rows[1],
    );
}

fn draw_check_list(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<String> = if !app.diag.started {
        vec![
            " 尚未运行诊断。".to_string(),
            "".to_string(),
            " 将检测：网卡判定 / DHCP·IP / 默认路由 / 网关 / DNS / 代理 / 虚拟网卡".to_string(),
            "        驱动 / 链路 / MTU / 外网 / 病毒 / 环路 / MAC 锁".to_string(),
        ]
    } else {
        let mut list = Vec::new();
        for (i, name) in app.diag.names.iter().enumerate() {
            let result = app.diag.results.get(i).and_then(|r| r.as_ref());
            let (icon, status_txt, detail) = match result {
                Some(r) => (
                    status_icon(r.status),
                    r.status.label().to_string(),
                    r.detail.clone(),
                ),
                None => (
                    status_icon(Status::Running),
                    "检测中…".to_string(),
                    String::new(),
                ),
            };
            let mut line = format!("{icon} {name} [{status_txt}] {detail}");
            if let Some(r) = result {
                if matches!(r.fix.as_ref().map(|f| &f.kind), Some(FixKind::Auto(_))) {
                    line.push_str("  → [一键修复]");
                }
                if !r.drill.is_empty() {
                    line.push_str("  ·可下钻");
                }
            }
            list.push(line);
        }
        if let Some(summary) = &app.diag.summary {
            list.push(String::new());
            list.push(format!("▶ {summary}"));
        }
        list
    };

    let selected = if app.diag.started {
        app.diag.selected
    } else {
        0
    };
    scroll_list(
        f,
        area,
        Block::default().borders(Borders::ALL).title(" 单项检测 "),
        &items,
        selected,
        &mut app.diag.offset,
    );
}

/// 下钻面板：当前值 + 子动作按钮（↑/↓ 选择 · Enter 执行）。
fn draw_drill_panel(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    if let Some(r) = app
        .diag
        .results
        .get(app.diag.selected)
        .and_then(|r| r.as_ref())
    {
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} ", status_icon(r.status)),
                Style::default()
                    .fg(status_color(r.status))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                r.name.clone(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        if let Some(cur) = &r.current {
            lines.push(Line::from(Span::styled(
                format!(" 当前值：{cur}"),
                Style::default().fg(Color::White),
            )));
        }
        lines.push(Line::from(Span::styled(
            format!(" 详情：{}", r.detail),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " 子动作（↑/↓ · Enter 执行）",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));

        if r.drill.is_empty() {
            lines.push(Line::from(Span::styled(
                "  （无可用子动作）",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            for (j, d) in r.drill.iter().enumerate() {
                if j == app.diag.drill_selected {
                    lines.push(Line::from(Span::styled(
                        format!(" ▶ {} ", d.label()),
                        Style::default().fg(Color::Black).bg(Color::Cyan),
                    )));
                } else {
                    lines.push(Line::from(Span::styled(
                        format!("   {} ", d.label()),
                        Style::default().fg(Color::White),
                    )));
                }
            }
        }

        if let Some(fix) = &r.fix {
            lines.push(Line::from(""));
            let color = match &fix.kind {
                FixKind::Auto(_) => Color::Green,
                FixKind::Manual(_) => Color::Yellow,
            };
            lines.push(Line::from(Span::styled(
                format!(" 修复：{}", fix.label),
                Style::default().fg(color),
            )));
        }
    }

    let p = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" 详情 / 子动作 "),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
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
            .map(|l| {
                Line::from(Span::styled(
                    format!(" › {l}"),
                    Style::default().fg(Color::Gray),
                ))
            })
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
        lines.push(Line::from(Span::styled(
            " （未检测到网卡）",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for a in app.adapters.iter().take(8) {
            let color = if a.is_virtual() {
                Color::Yellow
            } else {
                Color::White
            };
            let mark = if a.is_virtual() {
                format!(" [{}]", a.kind_label())
            } else {
                String::new()
            };
            let up = if a.is_up() { "↑" } else { "↓" };
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {up} "),
                    Style::default().fg(if a.is_up() {
                        Color::Green
                    } else {
                        Color::DarkGray
                    }),
                ),
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
