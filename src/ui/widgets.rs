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

/// 按显示宽度把一行文本折成多行（贪心断行，按字符切分）。
fn wrap_text(s: &str, max: usize) -> Vec<String> {
    if max == 0 {
        return vec![s.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut w = 0usize;
    for c in s.chars() {
        let cw = if (c as u32) == 0x00
            || (0x2000..=0x9fff).contains(&(c as u32))
            || (0xff00..=0xffef).contains(&(c as u32))
        {
            2
        } else {
            1
        };
        if w + cw > max && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            w = 0;
        }
        cur.push(c);
        w += cw;
    }
    out.push(cur);
    out
}

/// 可滚动列表：自动换行（边缘检测，超长内容不再顶出 UI）+ 右侧 `Scrollbar`，
/// 选中项（含其所有折行）始终可见并以反色高亮。
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
    let inner_w = area.width.saturating_sub(2) as usize; // 减去左右边框
    let inner_h = area.height.saturating_sub(2) as usize;
    if inner_h == 0 {
        return;
    }

    // 把每条原始项按宽度折成若干「可视行」，并记录其所属原始项索引。
    let mut flat: Vec<(usize, String)> = Vec::new();
    for (oi, s) in items.iter().enumerate() {
        let lines = wrap_text(s, inner_w.max(1));
        if lines.len() == 1 && lines[0].is_empty() {
            flat.push((oi, String::new()));
        } else {
            for ln in lines {
                flat.push((oi, ln));
            }
        }
    }

    // 选中项的第一条可视行必须可见。
    let sel_first = flat
        .iter()
        .position(|(oi, _)| *oi == selected)
        .unwrap_or(0);
    let max_off = flat.len().saturating_sub(inner_h);
    if sel_first < *offset {
        *offset = sel_first;
    } else if sel_first >= *offset + inner_h {
        *offset = sel_first + 1 - inner_h;
    }
    *offset = (*offset).min(max_off);

    // 仅渲染可视区内的可视行；属于选中项的整行反色高亮。
    let list_items: Vec<ListItem> = flat
        .iter()
        .skip(*offset)
        .take(inner_h)
        .map(|(oi, s)| {
            if *oi == selected {
                ListItem::new(s.clone()).style(Style::default().fg(Color::Black).bg(Color::Cyan))
            } else {
                ListItem::new(s.clone())
            }
        })
        .collect();
    let list = List::new(list_items).block(block);
    let mut st = ListState::default();
    st.select(None);
    f.render_stateful_widget(list, area, &mut st);

    let sb = Scrollbar::new(ScrollbarOrientation::VerticalRight);
    let mut sbs = ScrollbarState::new(flat.len()).position(*offset);
    f.render_stateful_widget(sb, area, &mut sbs);
}
