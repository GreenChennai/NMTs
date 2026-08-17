//! 模块三：网工终端界面（连接状态机 + 实时回显 + 厂商命令模板）。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, ConnState, ConnType};
use crate::ui::widgets::scroll_list;

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(3), // 状态灯 + 连接类型/厂商
        Constraint::Min(4),    // 主体
    ])
    .split(area);

    draw_header(f, app, chunks[0]);

    match app.term.conn {
        ConnState::Connected => draw_connected(f, app, chunks[1]),
        _ => draw_scan(f, app, chunks[1]),
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    // 连接状态灯
    let (light, label) = match app.term.conn {
        ConnState::Disconnected => (Color::Red, "未连接"),
        ConnState::Scanning => (Color::Yellow, "扫描中"),
        ConnState::Connected => (Color::Green, "已连接"),
    };
    let mut spans = vec![
        Span::styled(" ● ", Style::default().fg(light)),
        Span::styled(
            format!("{label} "),
            Style::default().fg(light).add_modifier(Modifier::BOLD),
        ),
    ];

    match app.term.conn {
        ConnState::Disconnected | ConnState::Scanning => {
            // 连接类型（通用 / eNSP），不预设厂商
            for t in [ConnType::Generic, ConnType::Ensp] {
                if t == app.term.conn_type {
                    spans.push(Span::styled(
                        format!(" {}(连接类型) ", t.label()),
                        Style::default().fg(Color::Black).bg(Color::Cyan),
                    ));
                } else {
                    spans.push(Span::styled(
                        format!(" {} ", t.label()),
                        Style::default().fg(Color::White),
                    ));
                }
                spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
            }
            spans.push(Span::styled(
                "  [←/→ 切类型]",
                Style::default().fg(Color::DarkGray),
            ));
        }
        ConnState::Connected => {
            // 连后识别结果 + 当前 CLI 厂商
            if let Some(v) = &app.term.detected_vendor {
                spans.push(Span::styled(
                    format!(" 设备：{v}"),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ));
                if let Some(m) = &app.term.detected_model {
                    spans.push(Span::styled(
                        format!("({m})"),
                        Style::default().fg(Color::White),
                    ));
                }
            } else {
                spans.push(Span::styled(
                    " 设备：未识别",
                    Style::default().fg(Color::Yellow),
                ));
            }
            if let Some(v) = app.vendor_db.vendors().get(app.term.vendor_idx) {
                spans.push(Span::styled(
                    format!("  CLI：{} ", v.label),
                    Style::default().fg(Color::White),
                ));
            }
            spans.push(Span::styled(
                "  [←/→ 手动切厂商]",
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// 未连接态：串口列表 + 扫描入口，管理命令不显示。
fn draw_scan(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).split(area);

    // 串口列表
    let items: Vec<String> = if app.term.ports.is_empty() {
        vec![" （未检测到串口）".to_string()]
    } else {
        app.term
            .ports
            .iter()
            .map(|p| format!(" {} — {}", p.name, p.description))
            .collect()
    };
    let selected = app.term.selected_port.min(items.len().saturating_sub(1));
    scroll_list(
        f,
        chunks[0],
        Block::default().borders(Borders::ALL).title(" 可用串口 "),
        &items,
        selected,
        &mut app.term.port_offset,
    );

    // 提示
    let mut lines = vec![
        Line::from(Span::styled(
            " 连接设备",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!(" 波特率：{}（S 切换）", app.term.baud),
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(Span::styled(
            " Enter  扫描并连接",
            Style::default().fg(Color::Cyan),
        )),
        Line::from(Span::styled(
            " S      切换波特率",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            " ↑/↓    选择串口",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            " 连接成功后才会显示命令模板与设备信息",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    if let Some(s) = &app.term.status {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(" {s}"),
            Style::default().fg(Color::Yellow),
        )));
    }

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" 连接 "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, chunks[1]);
}

/// 已连接态：命令模板 + 实时回显终端。
fn draw_connected(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(area);

    // 命令模板列表
    if let Some(v) = app.vendor_db.vendors().get(app.term.vendor_idx) {
        let items: Vec<String> = v
            .commands
            .iter()
            .map(|c| {
                let category = v
                    .categories
                    .iter()
                    .find(|cat| cat.id == c.category)
                    .map(|cat| cat.label.clone())
                    .unwrap_or_default();
                format!("{}  [{category}]  {}", c.label, c.command)
            })
            .collect();
        let selected = app.term.cmd_idx;
        scroll_list(
            f,
            chunks[0],
            Block::default()
                .borders(Borders::ALL)
                .title(" 命令模板（↑/↓ · Enter 发送） "),
            &items,
            selected,
            &mut app.term.cmd_offset,
        );
    }

    // 终端回显 + 输入行
    let visible = area.height.saturating_sub(3) as usize;
    let start = app.term.output.len().saturating_sub(visible);
    let mut lines: Vec<Line> = app.term.output[start..]
        .iter()
        .map(|l| Line::from(Span::styled(l.clone(), Style::default().fg(Color::White))))
        .collect();

    // 输入行
    if app.term.input_mode {
        lines.push(Line::from(vec![
            Span::styled(
                " > ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}_", app.term.input),
                Style::default().fg(Color::Yellow),
            ),
        ]));
    } else {
        lines.push(Line::from(Span::styled(
            " 按 I 进入输入模式",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" 终端回显 "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, chunks[1]);
}
