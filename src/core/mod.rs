//! 业务核心层：与 UI 解耦，不依赖 ratatui / crossterm，便于单元测试。
//!
//! 设计原则：`core/` 只做业务逻辑，`ui/` 只做渲染。

pub mod net_diag;
pub mod net_set;
