//! 模块五：配置备份 / 恢复界面（.nmtsbak 归档）。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;

const ACTIONS: [(&str, &str); 3] = [
    ("备份本机网络配置", "netsh dump + wlan profile + 注册表 → .nmtsbak"),
    ("恢复最近备份", "netsh -f + wlan add + reg import"),
    ("刷新备份列表", "重新扫描 backups/"),
];

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(4),
    ])
    .split(area);

    let hint = if !app.is_admin {
        Span::styled(" ⚠ 非管理员：备份/恢复可能失败，请以管理员身份运行", Style::default().fg(Color::Yellow))
    } else {
        Span::styled(" ↑/↓ 选择 · Enter 执行", Style::default().fg(Color::DarkGray))
    };
    f.render_widget(Paragraph::new(Line::from(hint)), chunks[0]);

    let body = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[1]);
    draw_actions(f, app, body[0]);
    draw_bundles(f, app, body[1]);
}

fn draw_actions(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = ACTIONS
        .iter()
        .enumerate()
        .map(|(i, (name, desc))| {
            if i == app.backup.selected {
                Line::from(vec![
                    Span::styled(format!(" ▶ {name} "), Style::default().fg(Color::Black).bg(Color::Cyan)),
                    Span::styled(format!(" {desc}\n", ), Style::default().fg(Color::DarkGray)),
                ])
            } else {
                Line::from(vec![
                    Span::styled(format!("   {name} "), Style::default().fg(Color::White)),
                    Span::styled(format!(" {desc}\n", ), Style::default().fg(Color::DarkGray)),
                ])
            }
        })
        .collect();

    if let Some(r) = &app.backup.result {
        lines.push(Line::from(""));
        let color = if r.starts_with('✓') { Color::Green } else { Color::Red };
        lines.push(Line::from(Span::styled(format!(" {r}"), Style::default().fg(color))));
    }

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" 备份 / 恢复 "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

fn draw_bundles(f: &mut Frame, app: &App, area: Rect) {
    let lines: Vec<Line> = if app.backup.bundles.is_empty() {
        vec![Line::from(Span::styled(
            " （暂无备份）",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        app.backup
            .bundles
            .iter()
            .map(|b| {
                Line::from(Span::styled(
                    format!(" {} ", b.file_name().unwrap_or_default().to_string_lossy()),
                    Style::default().fg(Color::White),
                ))
            })
            .collect()
    };

    let p = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" 已备份 (.nmtsbak) "),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}
