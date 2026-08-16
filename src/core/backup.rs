//! 模块五：本机 / 设备网络配置备份与恢复（`.nmtsbak` zip 归档）。
//!
//! 归档结构（对齐需求文档）：
//! ```text
//! <timestamp>_<host>.nmtsbak
//! ├── manifest.json
//! ├── windows/
//! │   ├── netsh_ip_dump.txt
//! │   ├── wlan_profiles/
//! │   └── registry_net.reg
//! └── devices/
//! ```

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::windows::{netsh, run};

/// 备份元信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub source: String,
    pub time: String,
    pub kind: String,
}

impl Manifest {
    fn new(kind: &str) -> Self {
        Self {
            version: 1,
            source: hostname(),
            time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            kind: kind.to_string(),
        }
    }
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".into())
}

fn bundle_dir(root: &Path) -> PathBuf {
    root.join("backups")
}

/// 备份本机网络配置，返回 `.nmtsbak` 路径。
pub fn backup_windows(root: &Path) -> Result<PathBuf, String> {
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let name = format!("{}_{}.nmtsbak", ts, hostname());
    let bundle = bundle_dir(root).join(&name);

    // 采集到临时目录
    let tmp = std::env::temp_dir().join(format!("nmts_bak_{ts}"));
    let win_dir = tmp.join("windows");
    fs::create_dir_all(&win_dir).map_err(|e| e.to_string())?;

    // netsh ip dump
    let ip = netsh::run_netsh(&["interface", "ip", "dump"]);
    if ip.success {
        let _ = fs::write(win_dir.join("netsh_ip_dump.txt"), &ip.stdout);
    }
    let ipv6 = netsh::run_netsh(&["interface", "ipv6", "dump"]);
    if ipv6.success {
        let _ = fs::write(win_dir.join("netsh_ipv6_dump.txt"), &ipv6.stdout);
    }

    // wlan profiles
    let wlan_dir = win_dir.join("wlan_profiles");
    fs::create_dir_all(&wlan_dir).ok();
    let _ = run(
        "netsh",
        &["wlan", "export", "profile", "folder=", wlan_dir.to_str().unwrap_or("."), "key=clear"],
        Duration::from_secs(20),
    );

    // 网络相关注册表导出
    let reg_file = win_dir.join("registry_net.reg");
    let _ = run(
        "reg",
        &[
            "export",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            reg_file.to_str().unwrap_or("reg.reg"),
            "/y",
        ],
        Duration::from_secs(20),
    );

    // manifest
    let manifest = Manifest::new("windows");
    let _ = fs::write(
        tmp.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
    );

    // 打包
    fs::create_dir_all(bundle.parent().unwrap()).map_err(|e| e.to_string())?;
    zip_dir(&tmp, &bundle)?;

    // 清理临时目录
    let _ = fs::remove_dir_all(&tmp);
    Ok(bundle)
}

/// 恢复本机网络配置。
pub fn restore_windows(bundle: &Path) -> Result<(), String> {
    let tmp = std::env::temp_dir().join(format!("nmts_restore_{}", chrono::Local::now().timestamp_millis()));
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    unzip_dir(bundle, &tmp)?;

    let win = tmp.join("windows");
    let ip = win.join("netsh_ip_dump.txt");
    if ip.exists() {
        let p = ip.to_string_lossy();
        let out = run("netsh", &["-f", p.as_ref()], Duration::from_secs(30));
        if !out.success {
            return Err(format!("恢复 IP 配置失败: {}", out.combined()));
        }
    }

    // wlan profiles 重新导入
    let wlan_dir = win.join("wlan_profiles");
    if wlan_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&wlan_dir) {
            for e in entries.flatten() {
                let p = e.path().to_string_lossy().to_string();
                let _ = run("netsh", &["wlan", "add", "profile", "filename=", &p], Duration::from_secs(15));
            }
        }
    }

    // 注册表导入
    let reg_file = win.join("registry_net.reg");
    if reg_file.exists() {
        let p = reg_file.to_string_lossy().to_string();
        let _ = run("reg", &["import", &p], Duration::from_secs(20));
    }

    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}

/// 列出所有 `.nmtsbak` 备份（按时间倒序）。
pub fn list_bundles(root: &Path) -> Vec<PathBuf> {
    let dir = bundle_dir(root);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut list: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "nmtsbak").unwrap_or(false))
        .map(|e| e.path())
        .collect();
    list.sort();
    list.reverse();
    list
}

/// 递归打包目录为 zip。
fn zip_dir(src: &Path, out: &Path) -> Result<(), String> {
    let file = fs::File::create(out).map_err(|e| e.to_string())?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();

    fn walk(zw: &mut zip::ZipWriter<fs::File>, dir: &Path, base: &Path, opts: zip::write::SimpleFileOptions) -> Result<(), String> {
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let rel = path.strip_prefix(base).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                zw.add_directory(rel.clone(), opts).map_err(|e| e.to_string())?;
                walk(zw, &path, base, opts)?;
            } else {
                zw.start_file(rel, opts).map_err(|e| e.to_string())?;
                let mut f = fs::File::open(&path).map_err(|e| e.to_string())?;
                io::copy(&mut f, zw).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    walk(&mut zw, src, src, opts)?;
    zw.finish().map_err(|e| e.to_string())?;
    Ok(())
}

/// 解压 zip 到目录。
fn unzip_dir(src: &Path, out: &Path) -> Result<(), String> {
    let file = fs::File::open(src).map_err(|e| e.to_string())?;
    let mut za = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    for i in 0..za.len() {
        let mut entry = za.by_index(i).map_err(|e| e.to_string())?;
        let out_path = out.join(entry.mangled_name());
        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut f = fs::File::create(&out_path).map_err(|e| e.to_string())?;
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            f.write_all(&buf).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("nmts_zip_test_{}", chrono::Local::now().timestamp_millis()));
        fs::create_dir_all(tmp.join("windows")).unwrap();
        fs::write(tmp.join("manifest.json"), "{}").unwrap();
        fs::write(tmp.join("windows/netsh_ip_dump.txt"), "dump").unwrap();

        let out = tmp.join("bundle.nmtsbak");
        zip_dir(&tmp, &out).unwrap();
        assert!(out.exists());

        let extract = tmp.join("extract");
        unzip_dir(&out, &extract).unwrap();
        assert!(extract.join("windows/netsh_ip_dump.txt").exists());
        assert_eq!(
            fs::read_to_string(extract.join("windows/netsh_ip_dump.txt")).unwrap(),
            "dump"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn manifest_fields() {
        let m = Manifest::new("windows");
        assert_eq!(m.version, 1);
        assert_eq!(m.kind, "windows");
        assert!(!m.source.is_empty());
    }
}
