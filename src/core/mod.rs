//! 业务核心层：与 UI 解耦，不依赖 ratatui / crossterm，便于单元测试。
//!
//! 设计原则：`core/` 只做业务逻辑，`ui/` 只做渲染。

pub mod backup;
pub mod design_check;
pub mod net_diag;
pub mod net_set;
pub mod serial;
pub mod topo_cli;
pub mod topology;
pub mod vendor_cli;
