//! 输入法守卫（V3.0 原则 P2）：非输入态禁用 IME。
//!
//! TUI 用 crossterm 读键，开着中文输入法时按 `R`/`F` 等单字母热键，OS 会先把
//! 按键送进组字窗口，软件收不到 `Char('r')`。这里通过 `ImmAssociateContext`
//! 解除控制台窗口的 IME 关联，保证所有单字母热键始终生效；仅在需要中文输入的
//! 命名字段临时 `enable_ime()`，离开恢复。
//!
//! 跨平台留空实现（非 Windows 无 IME 概念）。

#[cfg(windows)]
mod imp {
    use std::sync::Mutex;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Console::GetConsoleWindow;
    use windows::Win32::UI::Input::Ime::{ImmAssociateContext, ImmGetContext, HIMC};

    /// 保存的默认 IME 上下文指针（禁用前备份，恢复时还原）。
    static SAVED: Mutex<isize> = Mutex::new(0);

    fn console_hwnd() -> HWND {
        unsafe { GetConsoleWindow() }
    }

    /// 禁用控制台窗口 IME（解除关联，单字母热键不再被组字拦截）。
    pub fn disable_ime() {
        let hw = console_hwnd();
        if hw.0.is_null() {
            return;
        }
        unsafe {
            let ctx = ImmGetContext(hw);
            if let Ok(mut s) = SAVED.lock() {
                *s = ctx.0 as isize;
            }
            // 传 NULL 即解除 IME 关联。
            let _ = ImmAssociateContext(hw, HIMC::default());
        }
    }

    /// 恢复 IME（命名类字段输入中文前调用）。
    pub fn enable_ime() {
        let hw = console_hwnd();
        if hw.0.is_null() {
            return;
        }
        unsafe {
            let saved = *SAVED.lock().unwrap_or_else(|e| e.into_inner());
            if saved != 0 {
                let _ = ImmAssociateContext(hw, HIMC(saved as *mut core::ffi::c_void));
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn disable_ime() {}
    pub fn enable_ime() {}
}

pub use imp::{disable_ime, enable_ime};
