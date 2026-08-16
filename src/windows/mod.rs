//! Windows 系统调用封装层。
//!
//! 统一封装子进程执行（静默窗口、超时、GBK/UTF-8 解码）、管理员检测、
//! netsh / PowerShell 调用，以及多网卡枚举与「当前上网网卡」判定。
#![allow(dead_code)] // v0.1 骨架：部分 API 供 v0.2+ 模块（快捷设置 / 备份）使用

pub mod adapter;
pub mod netsh;
pub mod powershell;

use std::process::Command;
use std::time::Duration;

/// 子进程静默窗口标志（不弹出黑色控制台）。
#[cfg(target_os = "windows")]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 命令执行结果。
#[derive(Debug, Clone, Default)]
pub struct CmdOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutput {
    pub fn combined(&self) -> String {
        if self.stdout.is_empty() {
            self.stderr.clone()
        } else if self.stderr.is_empty() {
            self.stdout.clone()
        } else {
            format!("{}\n{}", self.stdout, self.stderr)
        }
    }
}

/// 将命令输出字节按编码解码（GBK 优先，兼容 ASCII；失败回退 lossy UTF-8）。
pub fn decode_output(bytes: Vec<u8>) -> String {
    // Windows 控制台命令（ipconfig/netsh/route 等）中文系统输出为 GBK(936)，
    // GBK 是 ASCII 超集，解码 ASCII 亦无副作用。
    let (text, _, had_errors) = encoding_rs::GBK.decode(&bytes);
    if had_errors {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        text.into_owned()
    }
}

/// 执行任意外部命令（静默、带超时）。
pub fn run(program: &str, args: &[&str], timeout: Duration) -> CmdOutput {
    run_inner(program, args, timeout, false)
}

/// 执行外部命令并强制按 UTF-8 解码输出（用于 PowerShell 等）。
pub fn run_utf8(program: &str, args: &[&str], timeout: Duration) -> CmdOutput {
    run_inner(program, args, timeout, true)
}

fn run_inner(program: &str, args: &[&str], _timeout: Duration, utf8: bool) -> CmdOutput {
    // 注：命令级超时（防假死）规划于 v0.2 用 wait-timeout 落地；
    // 当前依赖各命令自带超时（如 ping -w、nslookup），且经 spawn_blocking 不阻塞 UI。
    let mut cmd = Command::new(program);
    cmd.args(args);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let out = cmd.output();
    match out {
        Ok(o) => {
            let stdout = if utf8 {
                String::from_utf8_lossy(&o.stdout).into_owned()
            } else {
                decode_output(o.stdout)
            };
            let stderr = if utf8 {
                String::from_utf8_lossy(&o.stderr).into_owned()
            } else {
                decode_output(o.stderr)
            };
            CmdOutput {
                success: o.status.success(),
                exit_code: o.status.code(),
                stdout,
                stderr,
            }
        }
        Err(e) => CmdOutput {
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: e.to_string(),
        },
    }
}

/// 检测当前进程是否具有管理员权限（`net session` 探测法，Win7+ 通用）。
pub fn is_admin() -> bool {
    let mut cmd = Command::new("net");
    cmd.arg("session");
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }
    match cmd.status() {
        Ok(s) => s.success(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_gbk_ascii() {
        assert_eq!(decode_output(b"hello 8.8.8.8".to_vec()), "hello 8.8.8.8");
    }
}
