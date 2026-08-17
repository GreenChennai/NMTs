//! 入口：初始化日志、加载配置、解析命令行，进入 TUI 或快速诊断模式。

mod app;
mod cli;
mod config;
mod core;
mod ui;
mod web;
mod windows;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Commands};
use crate::config::Config;
use crate::core::net_diag::{DiagEvent, Diagnoser};

fn main() -> Result<()> {
    let config = Config::load().unwrap_or_default();
    init_logging(&config.log_level, config.log_keep_days)?;
    install_panic_hook();
    tracing::info!("NMTs 启动（版本见程序信息）");

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

/// 初始化日志：按天滚动，写入 `logs/nmts-YYYY-MM-DD.log`（以 .log 结尾，系统可识别），
/// 保留最近 N 天。
///
/// 不使用 tracing_appender 的滚动（其文件名只能在末尾追加日期，得到
/// `nmts.log.YYYY-MM-DD`，系统无法识别为 .log 文件）；改为自建按天命名的
/// 同步文件 writer，文件名固定以 `.log` 结尾。同步写入确保崩溃/退出不丢日志。
fn init_logging(level: &str, keep_days: u32) -> Result<()> {
    let log_dir = logs_dir();
    std::fs::create_dir_all(&log_dir)?;
    cleanup_old_logs(&log_dir, keep_days);

    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let log_path = log_dir.join(format!("nmts-{date}.log"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("无法打开日志文件: {}", log_path.display()))?;
    // 同步写入：进程退出/崩溃前日志立即落盘（non_blocking 在 abrupt exit 会丢缓冲）。
    let writer = std::sync::Mutex::new(file);

    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .with_target(false)
        .try_init()
        .ok();
    Ok(())
}

/// 全局 panic hook：把 panic 写入日志文件（日志初始化失败时回退 stderr），
/// 避免「崩溃无任何提示/记录」。
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "未知 panic".to_string()
        };
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "未知位置".into());
        tracing::error!("程序崩溃（panic）：{msg} @ {loc}");
        default_hook(info);
    }));
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
        // 匹配 nmts-YYYY-MM-DD.log（按天滚动文件名，以 .log 结尾）。
        if !name.starts_with("nmts-") || !name.ends_with(".log") {
            continue;
        }
        let date_part = &name["nmts-".len()..name.len() - ".log".len()];
        if let Ok(d) = chrono::NaiveDate::parse_from_str(date_part, "%Y-%m-%d") {
            let age = (now - d).num_days();
            if age > keep_days as i64 {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}
