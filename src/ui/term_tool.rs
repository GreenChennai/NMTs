//! 模块三：网工终端界面（厂商命令模板速查 + 串口枚举 + 命令发送）。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(3), // 厂商选择
        Constraint::Min(4),    // 命令列表 + 详情
    ])
    .split(area);

    draw_vendor_tabs(f, app, chunks[0]);

    let body = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[1]);
    draw_cmd_list(f, app, body[0]);
    draw_cmd_detail(f, app, body[1]);
}

fn draw_vendor_tabs(f: &mut Frame, app: &App, area: Rect) {
    let vendors = app.vendor_db.vendors();
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled(" 厂商 [", Style::default().fg(Color::DarkGray)));
    for (i, v) in vendors.iter().enumerate() {
        let s = if i == app.term.vendor_idx {
            Span::styled(
                format!(" {}({}) ", v.label, v.vendor),
                Style::default().fg(Color::Black).bg(Color::Cyan),
            )
        } else {
            Span::styled(format!(" {} ", v.label), Style::default().fg(Color::White))
        };
        spans.push(s);
        if i + 1 < vendors.len() {
            spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
        }
    }
    spans.push(Span::styled("]  ←/→ 切换", Style::default().fg(Color::DarkGray)));

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_cmd_list(f: &mut Frame, app: &App, area: Rect) {
    let Some(v) = app.vendor_db.vendors().get(app.term.vendor_idx) else {
        return;
    };

    let lines: Vec<Line> = v
        .commands
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let category = v
                .categories
                .iter()
                .find(|cat| cat.id == c.category)
                .map(|cat| cat.label.clone())
                .unwrap_or_default();
            if i == app.term.cmd_idx {
                Line::from(vec![
                    Span::styled(
                        format!(" ▶ {} ", c.label),
                        Style::default().fg(Color::Black).bg(Color::Cyan),
                    ),
                    Span::styled(
                        format!("[{category}] {}\n", c.command),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled(format!("   {} ", c.label), Style::default().fg(Color::White)),
                    Span::styled(
                        format!("[{category}] {}\n", c.command),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            }
        })
        .collect();

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" 命令模板（↑/↓ 选择） "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

fn draw_cmd_detail(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    if let Some(v) = app.vendor_db.vendors().get(app.term.vendor_idx) {
        if let Some(c) = v.commands.get(app.term.cmd_idx) {
            lines.push(Line::from(Span::styled(
                format!(" 命令：{}", c.command),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                format!(" 功能：{}", c.label),
                Style::default().fg(Color::White),
            )));
            if !c.args.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(" 参数：", Style::default().fg(Color::Cyan))));
                for a in &c.args {
                    lines.push(Line::from(Span::styled(
                        format!("   {{{}}} — {} ({})", a.name, a.label, a.arg_type),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            if c.interactive {
                lines.push(Line::from(Span::styled(
                    " （交互命令，需确认）",
                    Style::default().fg(Color::Yellow),
                )));
            }
        }
    }

    // 串口列表
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" 串口：", Style::default().fg(Color::Cyan))));
    if app.term.ports.is_empty() {
        lines.push(Line::from(Span::styled(
            "   （未检测到串口）",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for p in &app.term.ports {
            lines.push(Line::from(Span::styled(
                format!("   {} — {}", p.name, p.description),
                Style::default().fg(Color::White),
            )));
        }
    }

    if let Some(s) = &app.term.status {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(format!(" {s}"), Style::default().fg(Color::Green))));
    } else {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " Enter 发送命令到串口",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" 命令详情 / 串口 "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}
