//! 模块二：快捷设置界面（操作列表 + 结果）。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(2), // 提示行
        Constraint::Min(4),    // 列表 + 结果
    ])
    .split(area);

    let hint = if !app.is_admin {
        Span::styled(
            " ⚠ 非管理员：修改类操作可能失败，请以管理员身份运行",
            Style::default().fg(Color::Yellow),
        )
    } else if !app.env_ready {
        Span::styled(" 正在检测环境…", Style::default().fg(Color::Yellow))
    } else {
        Span::styled(
            " ↑/↓ 选择 · Enter 执行 · 修改前建议先「备份当前配置」",
            Style::default().fg(Color::DarkGray),
        )
    };
    f.render_widget(Paragraph::new(Line::from(hint)), chunks[0]);

    let body = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[1]);

    draw_list(f, app, body[0]);
    draw_detail(f, app, body[1]);
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let qs = &app.quick_set;
    let lines: Vec<Line> = qs
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            if i == qs.selected {
                Line::from(vec![
                    Span::styled(
                        format!(" ▶ {} ", item.name),
                        Style::default().fg(Color::Black).bg(Color::Cyan),
                    ),
                    Span::styled(
                        format!(" {}\n", item.desc),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled(format!("   {} ", item.name), Style::default().fg(Color::White)),
                    Span::styled(format!(" {}\n", item.desc), Style::default().fg(Color::DarkGray)),
                ])
            }
        })
        .collect();

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" 快捷设置 "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let qs = &app.quick_set;
    let mut lines: Vec<Line> = Vec::new();

    if let Some(item) = qs.items.get(qs.selected) {
        lines.push(Line::from(Span::styled(
            format!(" 当前选中：{}", item.name),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            format!(" 命令：{}", item.desc),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(Span::styled(
        " 最近操作：",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));
    match &qs.result {
        Some(r) => {
            let color = if r.starts_with('✓') { Color::Green } else { Color::Red };
            lines.push(Line::from(Span::styled(format!(" {r}"), Style::default().fg(color))));
        }
        None => {
            lines.push(Line::from(Span::styled(
                " （尚未执行操作）",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    // DNS 优选排名
    if !app.dns.results.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " DNS 优选排名：",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
        for (i, r) in app.dns.results.iter().take(8).enumerate() {
            let lat = r.latency_ms.map(|l| format!("{l}ms")).unwrap_or_else(|| "超时".into());
            let color = if r.reachable { Color::White } else { Color::DarkGray };
            lines.push(Line::from(vec![
                Span::styled(format!("  {}. ", i + 1), Style::default().fg(color)),
                Span::styled(format!("{} [{}] ", r.provider.name, r.provider.country), Style::default().fg(color)),
                Span::styled(lat, Style::default().fg(if r.reachable { Color::Green } else { Color::DarkGray })),
            ]));
        }
    }

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" 操作结果 "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}
