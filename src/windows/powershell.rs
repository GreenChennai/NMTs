//! PowerShell 调用封装（结构化数据 JSON 输出）。
#![allow(dead_code)] // v0.1 骨架：run_ps_raw 供诊断命令回显使用

use std::time::Duration;

use super::{run_utf8, CmdOutput};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// 执行 PowerShell 脚本，强制输出编码 UTF-8，便于 JSON 解析。
pub fn run_ps(script: &str) -> CmdOutput {
    let preamble = "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;";
    run_utf8(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &format!("{preamble} {script}")],
        DEFAULT_TIMEOUT,
    )
}

/// 以 JSON 字符串形式执行脚本（返回 stdout，失败时返回 None）。
pub fn run_ps_json(script: &str) -> Option<String> {
    let out = run_ps(script);
    if out.success && !out.stdout.trim().is_empty() {
        Some(out.stdout.trim().to_string())
    } else {
        None
    }
}

/// 执行一段任意 PowerShell 脚本（用于诊断命令回显）。
pub fn run_ps_raw(script: &str) -> CmdOutput {
    run_ps(script)
}
