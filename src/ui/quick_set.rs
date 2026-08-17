//! 模块二：快捷设置（V3.0 结构化面板：IPv4/IPv6 开关 + 表单 + MAC + 高级设置）。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, QsRow, ADVANCED_ACTIONS, IP_FIELD_LABELS};
use crate::ui::widgets::{scroll_list, toggle_line};

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    // DNS 优选交互模式：整屏展示可交互排名表
    if app.dns.interactive {
        draw_dns_table(f, app, area);
        return;
    }

    let chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(4)]).split(area);
    draw_header(f, app, chunks[0]);
    draw_panel(f, app, chunks[1]);
}

/// 顶部：当前网卡状态条 + 提示。
fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);

    let mut spans: Vec<Span> = Vec::new();
    if let Some(a) = &app.active_adapter {
        spans.push(Span::styled(
            format!(" 当前网卡：{}", a.name),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        if !app.current_dns.is_empty() {
            spans.push(Span::styled(
                format!("  DNS {}", app.current_dns.join("/")),
                Style::default().fg(Color::DarkGray),
            ));
        }
    } else {
        spans.push(Span::styled(
            " 当前网卡：未找到",
            Style::default().fg(Color::Red),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), rows[0]);

    let hint = if !app.is_admin {
        Span::styled(
            " ⚠ 非管理员：修改类操作可能失败，请以管理员身份运行",
            Style::default().fg(Color::Yellow),
        )
    } else if app.quick_set.editing {
        Span::styled(
            " 编辑中：输入数字/点/冒号 · Enter 应用 · Esc 取消",
            Style::default().fg(Color::Cyan),
        )
    } else {
        Span::styled(
            " ↑/↓ 移动焦点 · Enter 切换/编辑/执行",
            Style::default().fg(Color::DarkGray),
        )
    };
    f.render_widget(Paragraph::new(Line::from(hint)), rows[1]);
}

fn draw_panel(f: &mut Frame, app: &App, area: Rect) {
    let qs = &app.quick_set;
    let on = app.ipv6_on;
    let cur = qs.current_row(on);
    let mut lines: Vec<Line> = Vec::new();

    // ---- IPv4 ----
    lines.push(section(" IPv4 "));
    lines.push(focus_line(
        toggle_line("静态 IP / DHCP", qs.ipv4_static),
        cur == QsRow::Ipv4Toggle,
    ));
    if qs.ipv4_static {
        for i in 0..5 {
            let editing = qs.editing && !qs.editing_v6 && qs.field_idx == i;
            lines.push(field_line(
                IP_FIELD_LABELS[i],
                &qs.ipv4_fields[i],
                cur == QsRow::Ipv4Field(i),
                editing,
            ));
        }
    }

    // ---- IPv6 ----
    lines.push(section(" IPv6 "));
    if on {
        lines.push(focus_line(
            toggle_line("开启 / 关闭", on),
            cur == QsRow::Ipv6Toggle,
        ));
        for i in 0..5 {
            let editing = qs.editing && qs.editing_v6 && qs.field_idx == i;
            lines.push(field_line(
                IP_FIELD_LABELS[i],
                &qs.ipv6_fields[i],
                cur == QsRow::Ipv6Field(i),
                editing,
            ));
        }
    } else {
        lines.push(focus_line(
            toggle_line("开启 / 关闭", false),
            cur == QsRow::Ipv6Toggle,
        ));
        lines.push(gray("   （本机未启用 IPv6，开关灰显）"));
    }

    // ---- MAC ----
    lines.push(section(" MAC 地址（只读） "));
    if app.current_mac.is_empty() {
        lines.push(gray("   （未获取，等待环境探测）"));
    } else {
        lines.push(normal(format!("   {}", app.current_mac)));
    }

    // ---- 高级设置 ----
    let adv_title = if qs.advanced_open {
        " 高级设置 ▾"
    } else {
        " 高级设置 ▸"
    };
    lines.push(focus_line(
        Line::from(Span::styled(
            adv_title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        cur == QsRow::AdvancedToggle,
    ));
    if qs.advanced_open {
        for (i, (name, _)) in ADVANCED_ACTIONS.iter().enumerate() {
            lines.push(adv_line(name, cur == QsRow::AdvancedItem(i)));
        }
    }

    // ---- 结果 ----
    if let Some(r) = &qs.result {
        lines.push(Line::from(""));
        let color = if r.starts_with('✓') {
            Color::Green
        } else if r.starts_with('✗') {
            Color::Red
        } else {
            Color::Yellow
        };
        lines.push(Line::from(Span::styled(
            format!(" {r}"),
            Style::default().fg(color),
        )));
    }

    let p = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" 快捷设置面板 "),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

fn section(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("{title}"),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))
}

fn focus_line(line: Line<'static>, focused: bool) -> Line<'static> {
    if focused {
        let mut spans = vec![Span::styled(
            " ▶ ",
            Style::default().fg(Color::Black).bg(Color::Cyan),
        )];
        spans.extend(line.spans);
        Line::from(spans)
    } else {
        let mut spans = vec![Span::raw("   ")];
        spans.extend(line.spans);
        Line::from(spans)
    }
}

fn field_line(label: &str, val: &str, focused: bool, editing: bool) -> Line<'static> {
    let marker = if editing {
        Span::styled(" ▸ ", Style::default().fg(Color::Black).bg(Color::Cyan))
    } else if focused {
        Span::styled(" ▶ ", Style::default().fg(Color::Black).bg(Color::Cyan))
    } else {
        Span::raw("   ")
    };
    let val_txt = if editing {
        format!("{val}_")
    } else if val.is_empty() {
        "（未填）".to_string()
    } else {
        val.to_string()
    };
    let val_style = if focused || editing {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    Line::from(vec![
        marker,
        Span::styled(format!(" {label}: "), Style::default().fg(Color::DarkGray)),
        Span::styled(val_txt, val_style),
    ])
}

fn adv_line(name: &str, focused: bool) -> Line<'static> {
    if focused {
        Line::from(vec![
            Span::styled(" ▶ ", Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(name.to_string(), Style::default().fg(Color::White)),
        ])
    } else {
        Line::from(vec![
            Span::raw("   "),
            Span::styled(name.to_string(), Style::default().fg(Color::White)),
        ])
    }
}

fn gray(s: &str) -> Line<'static> {
    Line::from(Span::styled(
        s.to_string(),
        Style::default().fg(Color::DarkGray),
    ))
}

fn normal(s: String) -> Line<'static> {
    Line::from(Span::styled(s, Style::default().fg(Color::White)))
}

/// DNS 优选排名表（测速完成后，选中确认才应用）。
fn draw_dns_table(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<String> = app
        .dns
        .results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let lat = r
                .latency_ms
                .map(|l| format!("{l}ms"))
                .unwrap_or_else(|| "超时".into());
            let mark = if r.reachable { "" } else { " [不可达]" };
            format!(
                "{:>2}. [{}] {:<16} [{}] {}{}",
                i + 1,
                r.protocol.label(),
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
        Block::default()
            .borders(Borders::ALL)
            .title(" DNS 优选排名（Enter 应用） "),
        &items,
        selected,
        &mut app.dns.offset,
    );

    if let Some(r) = app.dns.results.get(selected) {
        let tip = if r.reachable {
            format!(
                " 将应用：[{}] {}（{} / {}），应用前自动备份",
                r.protocol.label(),
                r.provider.name,
                r.provider.primary,
                r.provider.secondary
            )
        } else {
            " 该候选不可达".to_string()
        };
        let p = Paragraph::new(Line::from(Span::styled(
            tip,
            Style::default().fg(Color::Yellow),
        )));
        f.render_widget(
            p,
            Rect {
                y: area.y + area.height.saturating_sub(2),
                x: area.x,
                width: area.width,
                height: 1,
            },
        );
    }
}
