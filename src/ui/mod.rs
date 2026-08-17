//! UI 渲染层：仅负责渲染，不含业务逻辑。

pub mod backup;
pub mod diagnose;
pub mod ime;
pub mod nav;
pub mod quick_set;
pub mod term_tool;
pub mod topology;
pub mod widgets;

use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs, Wrap};
use ratatui::Frame;

use crate::app::{App, TABS};
use crate::core::net_diag::Status;

/// 状态对应颜色。
#[allow(dead_code)] // v1.6 组件收口时用于状态着色
pub fn status_color(s: Status) -> Color {
    match s {
        Status::Ok => Color::Green,
        Status::Warn => Color::Yellow,
        Status::Error => Color::Red,
        Status::Running => Color::Blue,
        Status::Info => Color::Cyan,
        Status::Pending => Color::DarkGray,
    }
}

/// 状态图标。
pub fn status_icon(s: Status) -> &'static str {
    match s {
        Status::Ok => "✓",
        Status::Warn => "!",
        Status::Error => "✗",
        Status::Running => "…",
        Status::Info => "i",
        Status::Pending => "·",
    }
}

/// 主布局与路由。
pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(2), // 标题栏
        Constraint::Length(3), // 标签栏
        Constraint::Min(5),    // 内容区
        Constraint::Length(2), // 状态栏
    ])
    .split(area);

    draw_header(f, app, chunks[0]);
    draw_tabs(f, app, chunks[1]);

    if app.show_help {
        draw_help(f, chunks[2]);
    } else {
        match app.tab {
            0 => diagnose::draw(f, app, chunks[2]),
            1 => quick_set::draw(f, app, chunks[2]),
            2 => term_tool::draw(f, app, chunks[2]),
            3 => topology::draw(f, app, chunks[2]),
            4 => backup::draw(f, app, chunks[2]),
            _ => {}
        }
    }

    draw_footer(f, app, chunks[3]);
}

fn draw_header(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let mut spans: Vec<Span> = vec![Span::styled(
        " NMTs · 网络维护工具集 ",
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )];

    // 当前上网网卡标识
    if !app.env_ready {
        spans.push(Span::raw("  正在检测网络环境…"));
        spans.push(Span::styled(" ⏳", Style::default().fg(Color::Yellow)));
    } else {
        match &app.active_adapter {
            Some(a) => {
                let color = if a.is_virtual() {
                    Color::Yellow
                } else {
                    Color::Green
                };
                spans.push(Span::raw("  当前上网网卡: "));
                spans.push(Span::styled(
                    a.name.clone(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(
                    format!(" [{}]", a.kind_label()),
                    Style::default().fg(color),
                ));
            }
            None => {
                spans.push(Span::raw("  当前上网网卡: "));
                spans.push(Span::styled("未找到", Style::default().fg(Color::Red)));
            }
        }

        // 管理员标识（统一状态徽标）
        let admin = if app.is_admin {
            crate::ui::widgets::status_badge("管理员", Color::Green)
        } else {
            crate::ui::widgets::status_badge("非管理员", Color::Yellow)
        };
        spans.push(Span::raw("  "));
        spans.extend(admin.spans);
    }

    let p = Paragraph::new(Line::from(spans));
    f.render_widget(p, area);
}

fn draw_tabs(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let titles: Vec<Line> = TABS.iter().map(|t| Line::from(format!(" {t} "))).collect();
    let tabs = Tabs::new(titles)
        .select(app.tab)
        .block(Block::default().borders(Borders::BOTTOM))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .divider(Span::raw("│"));
    f.render_widget(tabs, area);
}

fn draw_footer(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let left = if let Some(msg) = &app.status_msg {
        msg.clone()
    } else if let Some(summary) = &app.diag.summary {
        summary.clone()
    } else {
        format!(
            "Tab {}/{} — Tab / Ctrl+← / Ctrl+→ 切换模块 · H 帮助 · Q 退出",
            app.tab + 1,
            TABS.len()
        )
    };
    let p = Paragraph::new(Line::from(vec![Span::styled(
        left,
        Style::default().fg(Color::DarkGray),
    )]))
    .block(Block::default().borders(Borders::TOP));
    f.render_widget(p, area);
}

fn draw_help(f: &mut Frame, area: ratatui::layout::Rect) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  全局快捷键",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  Tab / Ctrl+← / Ctrl+→   切换模块"),
        Line::from("  ← / →                   局部导航（各模块内）"),
        Line::from("  H            显示 / 隐藏帮助　Q 退出"),
        Line::from(""),
        Line::from(Span::styled(
            "  模块一 诊断",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  Enter / R    运行诊断　F 修复选中项　T 路由追踪　G 导出报告"),
        Line::from("  ↑/↓ 选检查项　Enter 下钻详情（当前值 + 子动作）"),
        Line::from(""),
        Line::from(Span::styled(
            "  模块二 快捷设置",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  ↑/↓ 选字段/动作　Enter 切换/应用　Esc 退出表单"),
        Line::from(""),
        Line::from(Span::styled(
            "  模块三 网工工具",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  ←/→ 切连接类型（通用/eNSP）　Enter 扫描连接（连后自动识别型号）"),
        Line::from("  I 输入模式　S 切波特率　[ / ] 手动切厂商"),
        Line::from(""),
        Line::from(Span::styled(
            "  模块四 拓扑",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  ↑/↓ 选拓扑　Enter 打开　N 新建　R 重命名　C 复制　Delete 删除"),
        Line::from("  O 打开编辑器　B 回读　E 导出 D2　D 下发 CLI"),
        Line::from(""),
        Line::from(Span::styled(
            "  说明",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  修改网络配置需管理员权限；修改类操作先自动备份可回退。"),
        Line::from("  非输入态已禁用输入法，单字母热键不被组字窗拦截（F2 切换）。"),
    ];
    let p = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" 帮助（F1 / H） "),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}
