//! 模块一：网络诊断界面（步骤清单 + 实时日志）。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::core::net_diag::{FixKind, Status};
use crate::ui::{status_color, status_icon};

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(2),  // 提示行
        Constraint::Min(5),     // 列表 + 日志
    ])
    .split(area);

    // 提示行
    let hint = if app.diag.running {
        Span::styled(" 诊断进行中…", Style::default().fg(Color::Yellow))
    } else if app.diag.started {
        Span::styled(" 诊断完成，按 R 重新运行", Style::default().fg(Color::DarkGray))
    } else {
        Span::styled(" 按 Enter / R 开始诊断", Style::default().fg(Color::Cyan))
    };
    f.render_widget(Paragraph::new(Line::from(hint)), chunks[0]);

    let body = Layout::horizontal([
        Constraint::Percentage(55),
        Constraint::Percentage(45),
    ])
    .split(chunks[1]);

    draw_check_list(f, app, body[0]);
    draw_log(f, app, body[1]);
}

fn draw_check_list(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    if !app.diag.started {
        lines.push(Line::from(Span::styled(
            " 尚未运行诊断。",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " 将检测：当前上网网卡 / DHCP·IP / 默认路由 / 网关连通 / DNS 解析 / 系统代理 / 虚拟网卡干扰",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for (i, name) in app.diag.names.iter().enumerate() {
            let result = app.diag.results.get(i).and_then(|r| r.as_ref());
            let (icon, color, status_txt, detail) = match result {
                Some(r) => (
                    status_icon(r.status),
                    status_color(r.status),
                    r.status.label(),
                    r.detail.clone(),
                ),
                None => (
                    status_icon(Status::Running),
                    status_color(Status::Running),
                    "检测中…",
                    String::new(),
                ),
            };
            let mut spans = vec![
                Span::styled(
                    format!(" {} ", icon),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {name} "), Style::default().fg(Color::White)),
                Span::styled(format!("[{status_txt}]"), Style::default().fg(color)),
            ];
            if !detail.is_empty() {
                spans.push(Span::styled(
                    format!("  {detail}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            lines.push(Line::from(spans));

            // 修复建议
            if let Some(fix) = result.and_then(|r| r.fix.as_ref()) {
                let (label, fcolor) = match &fix.kind {
                    FixKind::Auto(_) => (format!("     ⚙ {}", fix.label), Color::Green),
                    FixKind::Manual(_) => (format!("     ✎ {}", fix.label), Color::Yellow),
                };
                lines.push(Line::from(Span::styled(label, Style::default().fg(fcolor))));
            }
        }

        if let Some(summary) = &app.diag.summary {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!(" ▶ {summary}"),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )));
        }
    }

    let p = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" 诊断步骤清单 "),
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
            .map(|l| Line::from(Span::styled(format!(" › {l}"), Style::default().fg(Color::Gray))))
            .collect()
    };

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" 实时日志 "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}
