//! 模块三：串口识别 / 波特率试探 / 会话（serialport crate）。
//!
//! 真实串口（路由器 / 交换机 Console 口）需硬件环境验证；沙箱仅保证编译通过，
//! 枚举与试探逻辑通过单元测试覆盖。
#![allow(dead_code)] // probe_baud / read_chunk 供 v1.0 联动与真机串口会话使用

use std::io::Read;
use std::time::Duration;

use anyhow::{Context, Result};
use serialport::{SerialPort, SerialPortType};

/// 串口信息。
#[derive(Debug, Clone)]
pub struct PortInfo {
    pub name: String,
    pub description: String,
}

/// 枚举可用串口（COM 口）。
pub fn list_ports() -> Vec<PortInfo> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|p| PortInfo {
            name: p.port_name,
            description: port_desc(&p.port_type),
        })
        .collect()
}

fn port_desc(t: &SerialPortType) -> String {
    match t {
        SerialPortType::UsbPort(info) => {
            let mut parts = Vec::new();
            if let Some(m) = &info.manufacturer {
                parts.push(m.clone());
            }
            if let Some(p) = &info.product {
                parts.push(p.clone());
            }
            if parts.is_empty() {
                "USB 串口".to_string()
            } else {
                parts.join(" ")
            }
        }
        SerialPortType::BluetoothPort => "蓝牙串口".to_string(),
        SerialPortType::PciPort => "PCI 串口".to_string(),
        SerialPortType::Unknown => "未知串口".to_string(),
    }
}

/// 串口会话（阻塞读写，供后台线程使用）。
pub struct SerialSession {
    port: Box<dyn SerialPort>,
}

impl SerialSession {
    /// 打开串口。
    pub fn open(name: &str, baud: u32) -> Result<Self> {
        let port = serialport::new(name, baud)
            .timeout(Duration::from_millis(200))
            .open()
            .with_context(|| format!("打开串口 {name} 失败"))?;
        Ok(Self { port })
    }

    /// 写一行（追加 \r\n）。
    pub fn write_line(&mut self, line: &str) -> Result<()> {
        self.port
            .write_all(format!("{line}\r\n").as_bytes())
            .with_context(|| "写串口失败")?;
        self.port.flush().ok();
        Ok(())
    }

    /// 读一段（阻塞直到超时）。
    pub fn read_chunk(&mut self, buf: &mut [u8]) -> Result<usize> {
        Ok(self.port.read(buf)?)
    }

    /// 持续读取一段时间（约 `dur`），返回收到的文本（型号识别用）。
    pub fn read_all(&mut self, dur: Duration) -> String {
        let mut out = Vec::new();
        let start = std::time::Instant::now();
        let mut buf = [0u8; 256];
        while start.elapsed() < dur {
            match self.port.read(&mut buf) {
                Ok(n) if n > 0 => out.extend_from_slice(&buf[..n]),
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        String::from_utf8_lossy(&out).to_string()
    }
}

/// 连后探测厂商与型号（V3.0 原则 P5）。
///
/// 先发 VRP 的 `display version`，命中 Huawei / H3C 即返回；否则发 IOS 的
/// `show version`，命中 Cisco 即返回。返回 `(vendor id, 型号)`；无法识别返回 `None`。
pub fn detect_vendor(sess: &mut SerialSession) -> Option<(String, String)> {
    let _ = sess.write_line("display version");
    let vrp = sess.read_all(Duration::from_millis(1200));
    let lower = vrp.to_lowercase();
    if lower.contains("huawei") {
        return Some(("huawei_vrp".to_string(), extract_model(&vrp)));
    }
    if lower.contains("h3c") {
        return Some(("h3c_vrp".to_string(), extract_model(&vrp)));
    }

    let _ = sess.write_line("show version");
    let ios = sess.read_all(Duration::from_millis(1200));
    let lower = ios.to_lowercase();
    if lower.contains("cisco") {
        return Some(("cisco_ios".to_string(), extract_model(&ios)));
    }
    None
}

/// 从设备回显中提取常见型号（S5731 / AR6300 / USG6000 / WS-C2960 / C9300 / SR6600 …）。
fn extract_model(out: &str) -> String {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i)(S\d{4}|AR\d+|USG\d+|CE\d+|WS-C\d+|ISR\d+|C\d{4}|MSR\d+|SR\d+|CX\d+)",
        )
        .unwrap()
    });
    re.captures(out)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "未知型号".to_string())
}

/// 波特率试探：按给定顺序连接并发送 `\r`，命中回显即返回。
pub fn probe_baud(name: &str, bauds: &[u32]) -> Option<u32> {
    for &baud in bauds {
        if let Ok(mut s) = SerialSession::open(name, baud) {
            if s.write_line("\r").is_ok() {
                let mut buf = [0u8; 256];
                let mut collected = Vec::new();
                for _ in 0..6 {
                    match s.read_chunk(&mut buf) {
                        Ok(n) if n > 0 => {
                            collected.extend_from_slice(&buf[..n]);
                            break;
                        }
                        _ => {}
                    }
                }
                if looks_like_prompt(&collected) {
                    return Some(baud);
                }
            }
        }
    }
    None
}

/// 判断回显是否像设备提示符。
fn looks_like_prompt(data: &[u8]) -> bool {
    let s = String::from_utf8_lossy(data).to_lowercase();
    s.contains('>')
        || s.contains('#')
        || s.contains("username")
        || s.contains("login")
        || s.contains("password")
        || s.contains(">")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_detection() {
        assert!(looks_like_prompt(b"<Huawei>"));
        assert!(looks_like_prompt(b"\r\nUsername:"));
        assert!(looks_like_prompt(b"Router#"));
        assert!(!looks_like_prompt(b""));
        assert!(!looks_like_prompt(b"garbage without prompt"));
    }

    #[test]
    fn port_desc_usb() {
        let t = SerialPortType::UsbPort(serialport::UsbPortInfo {
            vid: 0x1234,
            pid: 0x5678,
            serial_number: None,
            manufacturer: Some("FTDI".into()),
            product: Some("USB Serial".into()),
        });
        assert_eq!(port_desc(&t), "FTDI USB Serial");
    }
}
