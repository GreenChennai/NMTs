//! 命令行参数（clap）。可选无界面模式：`nmts diag --quick`。

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "nmts",
    version,
    about = "NMTs — Network Maintenance Tool set（Windows 网络维护工具集）",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// 网络诊断（无界面快速模式）
    Diag {
        /// 快速诊断：直接输出文本结果，不进入 TUI
        #[arg(long)]
        quick: bool,
    },
}
