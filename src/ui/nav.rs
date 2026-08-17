//! 键位分层（V3.0 原则 P1）：顶层模块切换与局部导航解耦。
//!
//! 顶层只拦截 `Tab` / `Ctrl+Tab`(BackTab) / `Ctrl+←` / `Ctrl+→` 切换模块；
//! 普通 `←/→` 返回 `None`，下派给当前模块的局部处理器（子菜单 / 厂商切换 /
//! 拓扑三级菜单等），从根上解决「最大层级占用了 ←/→，二三级菜单用不了」。

use ratatui::crossterm::event::{KeyCode, KeyModifiers};

/// 顶层导航意图。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavIntent {
    /// 下一个模块。
    ModuleNext,
    /// 上一个模块。
    ModulePrev,
    /// 非模块级按键，下派局部处理。
    None,
}

/// 把按键翻译为顶层导航意图。
pub fn nav_intent(code: KeyCode, mods: KeyModifiers) -> NavIntent {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    match code {
        KeyCode::Tab => NavIntent::ModuleNext,
        // BackTab 在多数终端为 Shift+Tab；部分终端把 Ctrl+Tab 也报成 BackTab。
        KeyCode::BackTab => NavIntent::ModulePrev,
        KeyCode::Left if ctrl => NavIntent::ModulePrev,
        KeyCode::Right if ctrl => NavIntent::ModuleNext,
        _ => NavIntent::None,
    }
}
