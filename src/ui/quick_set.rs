//! 模块二：快捷设置（V3.0.5：IPv4/IPv6 左右布局 + 高级横向常驻 + 手动保存）。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, QsRow, QS_BTN_COUNT};
use crate::ui::widgets::toggle_line;

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    // DNS 优选交互模式：整屏展示可交互排名表
    if app.dns.interactive {
        draw_dns_table(f, app, area);
        return;
    }

    // 整块外框，内部按区域竖向切分（IPv4/IPv6 列在中间横向并排）。
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 快捷设置面板（修改后按「保存设置」手动应用） ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Length(2), // 顶部：当前网卡 + 提示
        Constraint::Length(1), // 切换当前网卡（可选 + 可切换）
        Constraint::Min(8),    // IPv4 | IPv6 左右双列
        Constraint::Length(1), // MAC（只读）
        Constraint::Length(3), // 高级设置（横向常驻，默认展开）
        Constraint::Length(3), // 底部操作按钮
        Constraint::Length(1), // 结果提示
    ])
    .split(inner);

    draw_header(f, app, chunks[0]);
    draw_adapter_switch(f, app, chunks[1]);
    draw_ip_columns(f, app, chunks[2]);
    draw_mac(f, app, chunks[3]);
    draw_advanced(f, app, chunks[4]);
    draw_buttons(f, app, chunks[5]);
    draw_result(f, app, chunks[6]);
}

/// 焦点前缀：选中显示 ▶，否则留空。
fn focus_prefix(focused: bool) -> &'static str {
    if focused {
        " ▶ "
    } else {
        "   "
    }
}

/// 顶部：当前网卡状态条 + 操作提示。
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
    f.render_widget(
        Paragraph::new(Line::from(spans)).wrap(Wrap { trim: true }),
        rows[0],
    );

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
    } else if app.quick_set.dirty {
        Span::styled(
            " 有未保存修改 · ←/→ 切换列 · Enter 切换/编辑/执行",
            Style::default().fg(Color::Yellow),
        )
    } else {
        Span::styled(
            " ↑/↓ 移动焦点 · ←/→ 切换 IPv4/IPv6 列 · Enter 切换/编辑/执行",
            Style::default().fg(Color::DarkGray),
        )
    };
    f.render_widget(
        Paragraph::new(Line::from(hint)).wrap(Wrap { trim: true }),
        rows[1],
    );
}

/// 切换当前网卡：可选（有焦点标记）+ 可切换（Enter 循环）。
fn draw_adapter_switch(f: &mut Frame, app: &App, area: Rect) {
    let cur = app.quick_set.current_row(app.ipv6_on);
    let focused = cur == QsRow::AdapterSwitch;

    let name = app
        .active_adapter
        .as_ref()
        .map(|a| a.name.clone())
        .unwrap_or_else(|| "（未找到）".into());
    let kind = app
        .active_adapter
        .as_ref()
        .map(|a| a.kind_label())
        .unwrap_or("");

    let line = Line::from(vec![
        Span::styled(
            focus_prefix(focused),
            if focused {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
        Span::styled("切换当前网卡 ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("[{name}]"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("（{kind}）"),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("  ⏎ 切换", Style::default().fg(Color::Yellow)),
    ]);
    f.render_widget(Paragraph::new(line).wrap(Wrap { trim: true }), area);
}

/// IPv4 / IPv6 左右双列。
fn draw_ip_columns(f: &mut Frame, app: &App, area: Rect) {
    let columns = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    draw_ip_block(f, app, columns[0], false);
    draw_ip_block(f, app, columns[1], true);
}

/// 单列渲染（v6=true 渲染 IPv6，否则 IPv4）。
fn draw_ip_block(f: &mut Frame, app: &App, area: Rect, v6: bool) {
    let qs = &app.quick_set;
    let on = app.ipv6_on;
    let cur = qs.current_row(on);

    let mut lines: Vec<Line> = vec![section(if v6 { " IPv6 " } else { " IPv4 " })];

    if !v6 {
        lines.push(focus_line(
            toggle_line("静态 IP / DHCP", qs.ipv4_static),
            cur == QsRow::Ipv4Toggle,
        ));
        for i in 0..5 {
            let editing = qs.editing && !qs.editing_v6 && qs.field_idx == i;
            let readonly = !qs.ipv4_static;
            lines.push(field_line(
                crate::app::IP_FIELD_LABELS[i],
                &qs.ipv4_fields[i],
                cur == QsRow::Ipv4Field(i),
                editing,
                readonly,
            ));
        }
    } else {
        lines.push(focus_line(
            toggle_line("开启 / 关闭", on),
            cur == QsRow::Ipv6Toggle,
        ));
        if on {
            lines.push(focus_line(
                toggle_line("静态 / 自动获取", qs.ipv6_static),
                cur == QsRow::Ipv6Mode,
            ));
            for i in 0..5 {
                let editing = qs.editing && qs.editing_v6 && qs.field_idx == i;
                let readonly = !qs.ipv6_static;
                lines.push(field_line(
                    crate::app::IP_FIELD_LABELS[i],
                    &qs.ipv6_fields[i],
                    cur == QsRow::Ipv6Field(i),
                    editing,
                    readonly,
                ));
            }
        } else {
            lines.push(gray("   （本机未启用 IPv6）"));
        }
    }

    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }),
        area,
    );
}

/// MAC 地址（只读）。
fn draw_mac(f: &mut Frame, app: &App, area: Rect) {
    let line = if app.current_mac.is_empty() {
        gray("   MAC：（未获取，等待环境探测）")
    } else {
        normal(format!("   MAC：{}", app.current_mac))
    };
    f.render_widget(Paragraph::new(line).wrap(Wrap { trim: true }), area);
}

/// 高级设置：独立分割窗，选项从左到右排布，默认全部展开（无合并选项）。
fn draw_advanced(f: &mut Frame, app: &App, area: Rect) {
    let cur = app.quick_set.current_row(app.ipv6_on);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 高级设置（默认展开） ");

    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    for (i, (name, _)) in crate::app::ADVANCED_ACTIONS.iter().enumerate() {
        let focused = cur == QsRow::AdvancedItem(i);
        if focused {
            spans.push(Span::styled(
                format!(" ▶ {name} "),
                Style::default().fg(Color::Black).bg(Color::Cyan),
            ));
        } else {
            spans.push(Span::styled(
                format!(" {name} "),
                Style::default().fg(Color::White),
            ));
        }
        spans.push(Span::raw("  "));
    }

    let p = Paragraph::new(Line::from(spans))
        .block(block)
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

/// 底部四个操作按钮。
fn draw_buttons(f: &mut Frame, app: &App, area: Rect) {
    let cur = app.quick_set.current_row(app.ipv6_on);
    let dirty = app.quick_set.dirty;
    let labels = ["保存设置", "恢复默认", "保存备份", "从备份恢复"];
    // 保存设置 / 恢复默认 在未修改时灰色禁用（仍显示）。
    let disabled = [!dirty, !dirty, false, false];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 操作（Enter 执行） ");

    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    for i in 0..QS_BTN_COUNT {
        let focused = cur == QsRow::Button(i);
        let txt = format!(" [ {} ] ", labels[i]);
        if disabled[i] {
            spans.push(Span::styled(txt, Style::default().fg(Color::DarkGray)));
        } else if focused {
            spans.push(Span::styled(
                txt,
                Style::default().fg(Color::Black).bg(Color::Cyan),
            ));
        } else {
            spans.push(Span::styled(txt, Style::default().fg(Color::White)));
        }
        spans.push(Span::raw(" "));
    }

    let p = Paragraph::new(Line::from(spans))
        .block(block)
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

/// 结果提示（按状态着色 + 自动换行）。
fn draw_result(f: &mut Frame, app: &App, area: Rect) {
    if let Some(r) = &app.quick_set.result {
        let color = if r.starts_with('✓') {
            Color::Green
        } else if r.starts_with('✗') {
            Color::Red
        } else {
            Color::Yellow
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {r}"),
                Style::default().fg(color),
            )))
            .wrap(Wrap { trim: true }),
            area,
        );
    }
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

/// 字段行。readonly=true 时灰显并标注「自动获取」，表示不可编辑。
fn field_line(
    label: &str,
    val: &str,
    focused: bool,
    editing: bool,
    readonly: bool,
) -> Line<'static> {
    let marker = if editing {
        Span::styled(" ▸ ", Style::default().fg(Color::Black).bg(Color::Cyan))
    } else if focused {
        Span::styled(" ▶ ", Style::default().fg(Color::Black).bg(Color::Cyan))
    } else {
        Span::raw("   ")
    };
    let (val_txt, val_style) = if readonly {
        let t = if val.is_empty() {
            "（自动获取）".to_string()
        } else {
            val.to_string()
        };
        (t, Style::default().fg(Color::DarkGray))
    } else if editing {
        (
            format!("{val}_"),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
    } else if val.is_empty() {
        (
            "（未填）".to_string(),
            Style::default().fg(Color::White),
        )
    } else {
        (
            val.to_string(),
            Style::default().fg(Color::White).add_modifier(if focused {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
        )
    };
    Line::from(vec![
        marker,
        Span::styled(
            format!(" {label}: "),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(val_txt, val_style),
    ])
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
    crate::ui::widgets::scroll_list(
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
        )))
        .wrap(Wrap { trim: true });
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
