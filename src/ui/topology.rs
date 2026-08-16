//! 模块四：拓扑图工具界面（v0.5 落地）。

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;

pub fn draw(f: &mut Frame, _app: &App, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from("  网络拓扑图工具（设计 + 配置 + 校验一体化）"),
        Line::from(""),
        Line::from("  规划于 v0.5 落地，含："),
        Line::from("    · petgraph 拓扑建模 + 设备参数 / 配置窗口"),
        Line::from("    · 按拓扑推导每台设备 CLI（D2 / Graphviz 渲染）"),
        Line::from("    · 前置条件预检（子网重叠 / VLAN 放行 / STP 环路等）"),
        Line::from("    · 与模块三联动（自动连接 / 配置漂移检测）"),
    ];
    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" 拓扑图 ")
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(p, area);
}
