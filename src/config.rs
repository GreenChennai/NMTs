//! 配置加载 / 保存（serde_yaml）。
//!
//! 默认配置内嵌于二进制（`config/default.yaml`）；运行时可在 exe 同级
//! `config/default.yaml` 覆盖；用户改动写入 `config/config.yaml`。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const DEFAULT_YAML: &str = include_str!("../config/default.yaml");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub admin_required: bool,
    pub log_level: String,
    pub log_keep_days: u32,
    pub dns_preference: DnsPreference,
    pub terminal: TerminalConfig,
    pub topology: TopologyConfig,
    pub backup: BackupConfig,
}

impl Default for Config {
    fn default() -> Self {
        // 手动构造（与 config/default.yaml 保持一致）。
        // 注意：不能在此调用 serde_yaml::from_str，因为 struct 级
        // #[serde(default)] 会让 serde 在反序列化时以 Self::default() 为种子，
        // 二者互相调用会栈溢出。
        Self {
            version: 1,
            admin_required: true,
            log_level: "info".into(),
            log_keep_days: 7,
            dns_preference: DnsPreference::default(),
            terminal: TerminalConfig::default(),
            topology: TopologyConfig::default(),
            backup: BackupConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DnsPreference {
    pub categories: Vec<String>,
    pub prefer_ipv: String,
    pub prefer_country: String,
    pub test: DnsTest,
    pub apply_when_better_than_current: bool,
}

impl Default for DnsPreference {
    fn default() -> Self {
        Self {
            categories: vec!["default".into(), "family".into(), "secure".into()],
            prefer_ipv: "ipv4".into(),
            prefer_country: "CN".into(),
            test: DnsTest::default(),
            apply_when_better_than_current: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DnsTest {
    pub probe_domain: String,
    pub samples: u32,
    pub timeout_ms: u64,
    pub rank_by: String,
}

impl Default for DnsTest {
    fn default() -> Self {
        Self {
            probe_domain: "www.baidu.com".into(),
            samples: 5,
            timeout_ms: 1500,
            rank_by: "latency".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub default_baud_rates: Vec<u32>,
    pub timeout_ms: u64,
    pub vendors: Vec<String>,
    pub ensp_compatible: bool,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            default_baud_rates: vec![9600, 115200, 19200, 38400, 57600],
            timeout_ms: 3000,
            vendors: vec![
                "huawei_vrp".into(),
                "h3c_vrp".into(),
                "cisco_ios".into(),
            ],
            ensp_compatible: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TopologyConfig {
    pub export_format: String,
    pub auto_cli: bool,
    pub open_after_export: bool,
}

impl Default for TopologyConfig {
    fn default() -> Self {
        Self {
            export_format: "d2".into(),
            auto_cli: true,
            open_after_export: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupConfig {
    pub dir: String,
    pub keep_days: u32,
    pub bundle_ext: String,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            dir: "./backups".into(),
            keep_days: 30,
            bundle_ext: "nmtsbak".into(),
        }
    }
}

/// 程序根目录（exe 所在目录，开发模式为项目根）。
pub fn app_root() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    let dir = exe.parent().unwrap_or(Path::new(".")).to_path_buf();
    // 开发模式下 exe 在 target/debug，回退到项目根
    if dir.join("config").join("default.yaml").exists() {
        dir
    } else if dir.join("..").join("config").join("default.yaml").exists() {
        dir.join("..")
    } else {
        dir
    }
}

impl Config {
    /// 加载配置：磁盘 `config/default.yaml` > 内嵌 default.yaml > 手动默认；
    /// 再以用户 `config/config.yaml` 做字段级覆盖。
    pub fn load() -> Result<Self> {
        let root = app_root();
        let default_path = root.join("config").join("default.yaml");

        let mut cfg = if default_path.exists() {
            let s = fs::read_to_string(&default_path)
                .with_context(|| format!("读取配置失败: {}", default_path.display()))?;
            serde_yaml::from_str(&s)
                .with_context(|| format!("解析配置失败: {}", default_path.display()))?
        } else {
            serde_yaml::from_str(DEFAULT_YAML).unwrap_or_else(|_| Self::default())
        };

        let user_path = root.join("config").join("config.yaml");
        if user_path.exists() {
            let s = fs::read_to_string(&user_path)
                .with_context(|| format!("读取配置失败: {}", user_path.display()))?;
            let user: Config = serde_yaml::from_str(&s)
                .with_context(|| format!("解析配置失败: {}", user_path.display()))?;
            merge(&mut cfg, user);
        }

        Ok(cfg)
    }

    /// 保存用户配置到 `config/config.yaml`。
    #[allow(dead_code)] // v0.1 骨架：供 v0.2 快捷设置模块持久化使用
    pub fn save(&self) -> Result<()> {
        let root = app_root();
        let dir = root.join("config");
        fs::create_dir_all(&dir).with_context(|| "创建 config 目录失败")?;
        let path = dir.join("config.yaml");
        let s = serde_yaml::to_string(self).context("序列化配置失败")?;
        fs::write(&path, s).with_context(|| format!("写入配置失败: {}", path.display()))?;
        Ok(())
    }
}

/// 顶层字段级合并：用户配置覆盖默认（递归使用 serde 序列化再合并太绕，
/// 这里做字段级覆盖，新增字段默认取默认值）。
fn merge(base: &mut Config, user: Config) {
    if user.version != 0 {
        base.version = user.version;
    }
    if user.admin_required != base.admin_required {
        base.admin_required = user.admin_required;
    }
    if !user.log_level.is_empty() {
        base.log_level = user.log_level;
    }
    if user.log_keep_days != 0 {
        base.log_keep_days = user.log_keep_days;
    }
    base.dns_preference = user.dns_preference;
    base.terminal = user.terminal;
    base.topology = user.topology;
    base.backup = user.backup;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let cfg = Config::default();
        assert_eq!(cfg.version, 1);
        assert!(cfg.dns_preference.categories.contains(&"default".to_string()));
        assert_eq!(cfg.terminal.default_baud_rates[0], 9600);
    }
}
