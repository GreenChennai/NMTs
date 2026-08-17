//! 模块五：配置备份 / 恢复界面（.nmtsbak 归档）。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::ui::widgets::scroll_list;

const ACTIONS: [(&str, &str); 4] = [
    (
        "备份本机网络配置",
        "netsh dump + wlan profile + 注册表 → .nmtsbak",
    ),
    (
        "备份已连接设备配置",
        "抓取 running-config 归档（需先连接设备）",
    ),
    ("恢复最近备份", "netsh -f + wlan add + reg import"),
    ("刷新备份列表", "重新扫描 backups/"),
];

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(4)]).split(area);

    let hint = if !app.is_admin {
        Span::styled(
            " ⚠ 非管理员：备份/恢复可能失败，请以管理员身份运行",
            Style::default().fg(Color::Yellow),
        )
    } else {
        Span::styled(
            " ↑/↓ 选择 · Enter 执行",
            Style::default().fg(Color::DarkGray),
        )
    };
    f.render_widget(Paragraph::new(Line::from(hint)), chunks[0]);

    let body = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[1]);
    draw_actions(f, app, body[0]);
    draw_bundles(f, app, body[1]);
}

fn draw_actions(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<String> = ACTIONS
        .iter()
        .map(|(name, desc)| format!("{name} — {desc}"))
        .collect();
    let selected = app.backup.selected;
    scroll_list(
        f,
        area,
        Block::default()
            .borders(Borders::ALL)
            .title(" 备份 / 恢复 "),
        &items,
        selected,
        &mut app.backup.offset,
    );

    if let Some(r) = &app.backup.result {
        let color = if r.starts_with('✓') {
            Color::Green
        } else {
            Color::Red
        };
        let p = Paragraph::new(Line::from(Span::styled(
            format!(" {r}"),
            Style::default().fg(color),
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

fn draw_bundles(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<String> = if app.backup.bundles.is_empty() {
        vec![" （暂无备份）".to_string()]
    } else {
        app.backup
            .bundles
            .iter()
            .map(|b| format!(" {} ", b.file_name().unwrap_or_default().to_string_lossy()))
            .collect()
    };

    scroll_list(
        f,
        area,
        Block::default()
            .borders(Borders::ALL)
            .title(" 已备份 (.nmtsbak) "),
        &items,
        app.backup.bundles_offset.min(items.len().saturating_sub(1)),
        &mut app.backup.bundles_offset,
    );
}
