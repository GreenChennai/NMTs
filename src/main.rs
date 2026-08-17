//! 入口：初始化日志、加载配置、解析命令行，进入 TUI 或快速诊断模式。

mod app;
mod cli;
mod config;
mod core;
mod ui;
mod windows;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Commands};
use crate::config::Config;
use crate::core::net_diag::{DiagEvent, Diagnoser};

fn main() -> Result<()> {
    let config = Config::load().unwrap_or_default();
    init_logging(&config.log_level, config.log_keep_days)?;

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Diag { quick: true }) => {
            run_quick_diag()?;
        }
        _ => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let mut app = app::App::new(config, rt.handle().clone(), tx, rx);
            app.run()?;
        }
    }

    Ok(())
}

/// 无界面快速诊断：打印文本结果。
fn run_quick_diag() -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let diagnoser = Diagnoser::new();
    println!("NMTs 快速诊断");
    println!(
        "管理员权限：{}；当前上网网卡：{}",
        if diagnoser.ctx.probe.is_admin {
            "是"
        } else {
            "否"
        },
        diagnoser
            .active_adapter()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "未找到".into())
    );
    println!("{}", "-".repeat(60));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let handle = rt.spawn(async move { diagnoser.run(tx).await });

    // 阻塞等待事件，逐条打印；run 结束时 tx 被 drop，recv 返回 None。
    while let Some(event) = rt.block_on(rx.recv()) {
        match event {
            DiagEvent::CheckStarted { index, name } => {
                println!("[{}/?] 检测中：{}", index + 1, name);
            }
            DiagEvent::CheckDone { result, .. } => {
                let icon = match result.status {
                    crate::core::net_diag::Status::Ok => "[正常]",
                    crate::core::net_diag::Status::Warn => "[警告]",
                    crate::core::net_diag::Status::Error => "[异常]",
                    _ => "[信息]",
                };
                println!("  {icon} {}：{}", result.name, result.detail);
                if let Some(f) = &result.fix {
                    println!("        → {}", f.label);
                }
            }
            DiagEvent::Log(line) => {
                println!("        · {}", line);
            }
            DiagEvent::Finished { summary } => {
                println!("{}", "-".repeat(60));
                println!("{summary}");
            }
            DiagEvent::Started { .. } => {}
        }
    }

    let _ = rt.block_on(handle);
    Ok(())
}

/// 初始化日志：按天滚动，写入 `logs/nmts-YYYY-MM-DD.log`，保留最近 N 天。
fn init_logging(level: &str, keep_days: u32) -> Result<()> {
    let log_dir = logs_dir();
    std::fs::create_dir_all(&log_dir)?;
    cleanup_old_logs(&log_dir, keep_days);

    let file_appender = tracing_appender::rolling::daily(&log_dir, "nmts");
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(file_appender)
        .with_ansi(false)
        .try_init()
        .ok();
    Ok(())
}

fn logs_dir() -> PathBuf {
    config::app_root().join("logs")
}

fn cleanup_old_logs(dir: &std::path::Path, keep_days: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = chrono::Local::now().date_naive();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with("nmts-") || !name.ends_with(".log") {
            continue;
        }
        // 解析文件名中的日期 nmts-YYYY-MM-DD.log
        let date_part = &name["nmts-".len()..name.len() - ".log".len()];
        if let Ok(d) = chrono::NaiveDate::parse_from_str(date_part, "%Y-%m-%d") {
            let age = (now - d).num_days();
            if age > keep_days as i64 {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}
