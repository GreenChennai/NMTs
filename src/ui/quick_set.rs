//! 模块二：快捷设置界面（v0.2 落地）。

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;

pub fn draw(f: &mut Frame, _app: &App, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from("  快捷设置（DHCP / IP / DNS 优选 / IPv4·IPv6 / 网络优化）"),
        Line::from(""),
        Line::from("  规划于 v0.2 落地，含："),
        Line::from("    · 静态 IP / 掩码 / 网关，切回 DHCP"),
        Line::from("    · DNS 优选引擎（内置 DNS 库，并发测速、就近优选）"),
        Line::from("    · IPv6 开关、无线配置管理、TCP 全局优化"),
        Line::from("    · 注册表优化（先备份、可回退）"),
    ];
    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" 快捷设置 ")
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(p, area);
}
