//! 模块三：网工工具（终端）界面（v0.3 落地）。

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;

pub fn draw(f: &mut Frame, _app: &App, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from("  网工终端（类 SecureCRT）"),
        Line::from(""),
        Line::from("  规划于 v0.3 落地，含："),
        Line::from("    · 自动识别串口 / 试探波特率连接路由·交换"),
        Line::from("    · 兼容华为·H3C (VRP)、Cisco (IOS)，支持 eNSP"),
        Line::from("    · 厂商命令模板（vendor_db/），免记命令"),
    ];
    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" 网工工具 ")
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(p, area);
}
