//! 模块二：快捷设置界面（可滚动操作列表 + 结果 / DNS 优选排名表）。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, IP_FORM_FIELDS};
use crate::ui::widgets::scroll_list;

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
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
    } else if app.dns.interactive {
        Span::styled(" ↑/↓ 选 DNS · Enter 应用（先备份） · Esc 返回", Style::default().fg(Color::Cyan))
    } else {
        Span::styled(" ↑/↓ 选择 · Enter 执行", Style::default().fg(Color::DarkGray))
    };
    f.render_widget(Paragraph::new(Line::from(hint)), chunks[0]);

    let body = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[1]);

    draw_list(f, app, body[0]);
    draw_detail(f, app, body[1]);
}

fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<String> = app
        .quick_set
        .items
        .iter()
        .map(|item| format!("{} — {}", item.name, item.desc))
        .collect();
    let selected = app.quick_set.selected;
    scroll_list(
        f,
        area,
        Block::default().borders(Borders::ALL).title(" 快捷设置 "),
        &items,
        selected,
        &mut app.quick_set.offset,
    );
}

fn draw_detail(f: &mut Frame, app: &mut App, area: Rect) {
    // 静态 IP 表单模式
    if app.ip_form.active {
        draw_ip_form(f, app, area);
        return;
    }
    // DNS 优选交互模式：右侧展示可交互的排名表
    if app.dns.interactive {
        draw_dns_table(f, app, area);
        return;
    }

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

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" 操作结果 "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

/// 静态 IP 表单（手动填写）。
fn draw_ip_form(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        " 静态 IP 配置",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    for (i, label) in IP_FORM_FIELDS.iter().enumerate() {
        let value = &app.ip_form.fields[i];
        if i == app.ip_form.focus {
            lines.push(Line::from(vec![
                Span::styled(format!(" ▸ {label}: "), Style::default().fg(Color::Black).bg(Color::Cyan)),
                Span::styled(format!("{value}_"), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(format!("   {label}: "), Style::default().fg(Color::DarkGray)),
                Span::styled(if value.is_empty() { "（未填）" } else { value }, Style::default().fg(Color::White)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ↑/↓ 选字段 · 输入数字/点/冒号 · Backspace 删除",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(Span::styled(
        " Enter 应用（先备份） · Esc 返回",
        Style::default().fg(Color::Yellow),
    )));

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" 静态 IP 表单 "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

/// DNS 优选排名表（测速完成后，选中确认才应用）。
fn draw_dns_table(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<String> = app
        .dns
        .results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let lat = r.latency_ms.map(|l| format!("{l}ms")).unwrap_or_else(|| "超时".into());
            let mark = if r.reachable { "" } else { " [不可达]" };
            format!(
                "{:>2}. {:<18} [{}] {}{}",
                i + 1,
                r.provider.name,
                r.provider.country,
                lat,
                mark
            )
        })
        .collect();

    let selected = app.dns.selected;
    scroll_list(
        f,
        area,
        Block::default().borders(Borders::ALL).title(" DNS 优选排名（Enter 应用） "),
        &items,
        selected,
        &mut app.dns.offset,
    );

    // 底部提示当前选中项
    if let Some(r) = app.dns.results.get(selected) {
        let tip = if r.reachable {
            format!(
                " 将应用：{}（{} / {}），应用前自动备份",
                r.provider.name, r.provider.primary, r.provider.secondary
            )
        } else {
            " 该候选不可达".to_string()
        };
        let p = Paragraph::new(Line::from(Span::styled(tip, Style::default().fg(Color::Yellow))));
        f.render_widget(p, Rect { y: area.y + area.height.saturating_sub(2), x: area.x, width: area.width, height: 1 });
    }
}
