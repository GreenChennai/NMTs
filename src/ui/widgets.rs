//! 统一 UI 组件库（V2.0 规范，见设计指导第三节）。
//!
//! 沉淀可复用组件，解决「列表不可滚动、信息呈现不一致、缺表单/开关」三类问题。
//! 各模块（诊断/快捷设置/网工工具/拓扑/备份）统一调用，不再各自用 `Paragraph`
//! 把行塞进文本框。

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, List, ListItem, ListState, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Frame;

/// 状态徽标（连接 / 管理员 / 上网网卡等），用于 header 与详情。
pub fn status_badge(text: &str, color: Color) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {text} "),
        Style::default().fg(Color::Black).bg(color),
    ))
}

/// 开关组件：`on` 为当前状态，渲染 `[开]` / `[关]`（空格切换由调用方处理）。
pub fn toggle_line(label: &str, on: bool) -> Line<'static> {
    let (txt, color) = if on {
        ("[开]", Color::Green)
    } else {
        ("[关]", Color::Red)
    };
    Line::from(vec![
        Span::raw(format!(" {label} ")),
        Span::styled(txt, Style::default().fg(Color::Black).bg(color)),
    ])
}

/// 可滚动列表：`List` + 右侧 `Scrollbar`，自动保证选中项始终可见。
///
/// `selected` 为当前选中索引，`offset` 为可视区起始行（由本函数维护并回写）。
pub fn scroll_list(
    f: &mut Frame,
    area: Rect,
    block: Block,
    items: &[String],
    selected: usize,
    offset: &mut usize,
) {
    let inner_h = area.height.saturating_sub(2) as usize;
    if inner_h == 0 {
        return;
    }
    let max_off = items.len().saturating_sub(inner_h);
    if selected < *offset {
        *offset = selected;
    } else if selected >= offset.saturating_add(inner_h) {
        *offset = selected + 1 - inner_h;
    }
    *offset = (*offset).min(max_off);

    let list_items: Vec<ListItem> = items.iter().map(|s| ListItem::new(s.clone())).collect();
    let list = List::new(list_items)
        .block(block)
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan));
    let mut st = ListState::default();
    st.select(Some(selected));
    *st.offset_mut() = *offset;
    f.render_stateful_widget(list, area, &mut st);
    *offset = st.offset();

    let sb = Scrollbar::new(ScrollbarOrientation::VerticalRight);
    let mut sbs = ScrollbarState::new(items.len()).position(*offset);
    f.render_stateful_widget(sb, area, &mut sbs);
}
