//! 模块四：拓扑图工具界面（设备列表 + 预检清单 + CLI 预览）。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::core::design_check::Severity;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(4),
    ])
    .split(area);

    let hint = Span::styled(
        " ↑/↓ 选设备 · Enter 生成 CLI · E 导出 D2/SVG（内置演示拓扑）",
        Style::default().fg(Color::DarkGray),
    );
    f.render_widget(Paragraph::new(Line::from(hint)), chunks[0]);

    let body = Layout::horizontal([
        Constraint::Percentage(34),
        Constraint::Percentage(34),
        Constraint::Percentage(32),
    ])
    .split(chunks[1]);

    draw_devices(f, app, body[0]);
    draw_findings(f, app, body[1]);
    draw_cli(f, app, body[2]);
}

fn draw_devices(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = app
        .topo
        .topology
        .devices
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let head = format!(" {} [{}·{}]", d.name, d.vendor.label(), d.role.label());
            if i == app.topo.selected {
                Line::from(Span::styled(
                    format!("▶{head}"),
                    Style::default().fg(Color::Black).bg(Color::Cyan),
                ))
            } else {
                Line::from(Span::styled(head, Style::default().fg(Color::White)))
            }
        })
        .collect();

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" 链路：", Style::default().fg(Color::Cyan))));
    for l in &app.topo.topology.links {
        lines.push(Line::from(Span::styled(
            format!("   {} ↔ {}", l.from, l.to),
            Style::default().fg(Color::DarkGray),
        )));
    }

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" 拓扑设备 "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

fn draw_findings(f: &mut Frame, app: &App, area: Rect) {
    let lines: Vec<Line> = if app.topo.findings.is_empty() {
        vec![Line::from(Span::styled(
            " 预检通过，无问题。",
            Style::default().fg(Color::Green),
        ))]
    } else {
        app.topo
            .findings
            .iter()
            .flat_map(|f| {
                let (color, icon) = match f.severity {
                    Severity::Error => (Color::Red, "✗"),
                    Severity::Warn => (Color::Yellow, "!"),
                    Severity::Info => (Color::Cyan, "i"),
                };
                vec![
                    Line::from(vec![
                        Span::styled(format!(" {icon} "), Style::default().fg(color)),
                        Span::styled(f.message.clone(), Style::default().fg(color)),
                    ]),
                    Line::from(Span::styled(
                        format!("   ↳ {}", f.suggestion),
                        Style::default().fg(Color::DarkGray),
                    )),
                    Line::from(""),
                ]
            })
            .collect()
    };

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" 预检问题清单 "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

fn draw_cli(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    if let Some(d) = app.topo.topology.devices.get(app.topo.selected) {
        lines.push(Line::from(Span::styled(
            format!(" 设备：{}（{}）", d.name, d.vendor.label()),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
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
        lines.push(Line::from(Span::styled(format!(" {s}"), Style::default().fg(Color::Green))));
    }

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" CLI 预览 / 导出 "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}
