//! 模块五：配置备份界面（v0.4 落地）。

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;

pub fn draw(f: &mut Frame, _app: &App, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from("  网络设置备份 / 恢复"),
        Line::from(""),
        Line::from("  规划于 v0.4 落地，含："),
        Line::from("    · 本机网络配置备份（IP / DNS / 无线 profile / 注册表）"),
        Line::from("    · 单台路由 / 交换机配置备份（经模块三抓取）"),
        Line::from("    · 归档为 .nmtsbak（zip），一键还原"),
    ];
    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" 配置备份 ")
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(p, area);
}
