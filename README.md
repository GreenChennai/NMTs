<p align="center">
  <h1 align="center">NMTs · 网络维护工具集</h1>
</p>
<p align="center">
  <img src="https://img.shields.io/badge/version-1.8.0-orange.svg" alt="Version">
  <img src="https://img.shields.io/badge/platform-Windows%2010%2F11-lightgrey.svg" alt="Platform">
  <img src="https://img.shields.io/badge/lang-Rust-DEA584.svg" alt="Language">
  <img src="https://img.shields.io/badge/UI-ratatui%20%2B%20crossterm-4D4D4D.svg" alt="UI">
  <img src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg" alt="License">
</p>
<p align="center">
  <strong>Windows 下的网络故障一键诊断 / 修复、网络快捷设置、网工终端与拓扑配置工具</strong>
</p>

---

## 简介

**NMTs**（Network Maintenance Tool set）是一款面向 Windows 10/11 的网络维护工具集，使用 Rust 编写、`ratatui` + `crossterm` 终端界面（TUI）。目标是让网络故障的排查、修复、快捷设置、网工运维（终端 / 拓扑 / 备份）都可以在一套工具内完成。

**五大模块：**

1. **网络诊断** — 三层诊断模型（基础层 / 本机环境 / 外部因素），一键修复 + 指导兜底。
2. **快捷设置** — DHCP / IP / DNS 优选 / IPv4·IPv6 / 网络与无线优化，优化前自动备份可回退。
3. **网工工具** — 类 SecureCRT 终端，自动识别串口 / 试探波特率，兼容华为·H3C（VRP）、Cisco（IOS），支持 eNSP。
4. **拓扑图工具** — 绘制网络拓扑，据拓扑推导每台设备 CLI 配置，可导出或一键下发。
5. **配置备份** — 本机网络配置与单台路由器 / 交换机配置的备份 / 恢复（`.nmtsbak`）。

## 当前进度

### 已封版（v0.1 ~ v1.0）
五大模块骨架 + 多网卡判定 + 执行反馈框架 + 拓扑设计/校验，`v1.0.0` 封版。

### V2.0 迭代（体验重构 + 缺口补全，语义化增量 → 最终 v2.0.0 收口）

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| v1.1.0 | 统一 ScrollList + DNS 优选改确认 + 删冗余/分组 | ✅ 完成 |
| v1.2.0 | 手动静态 IP 表单 + DHCP/IPv6 单一开关 | ✅ 完成 |
| v1.3.0 | 网工工具连接状态机 + eNSP + 持久会话 | ✅ 完成 |
| v1.4.0 | 配置备份支持设备 running-config | ✅ 完成 |
| v1.5.0 | 拓扑编辑器外置（React Flow + Python 独立窗口） | ✅ 完成 |
| v1.6.0 | 统一组件规范收口 + 回归测试 + 安装包 | ✅ 完成 |
| v1.7.0 | 多网卡深化 + EventBus + 诊断实时化 | ✅ 完成 |
| v1.8.0 | 配置漂移闭环 + 脚本中心 + 帮助引导 | ✅ 完成 |
| v2.0.0 | 大版本收口：报告导出 + 定时巡检 + 撤销审计 + 跨模块联动 | ⏳ |

## 环境与构建

- **Rust 工具链**：`rustup` stable，目标 `x86_64-pc-windows-msvc`
- **构建工具**：Visual Studio 生成工具（C++ 桌面开发，MSVC）

```bash
# 开发运行
cargo run

# 发布构建（单文件 exe 位于 target/release/nmts.exe）
cargo build --release

# 仅检查
cargo check

# 无界面快速诊断（可脚本化）
nmts diag --quick
```

## 以管理员身份运行

修改网络配置需要管理员权限。程序已通过 manifest 请求提权（双击 exe 会弹 UAC）。非管理员运行时，修改类功能会灰显并提示。

## 目录结构

```
nmts/
├── Cargo.toml
├── build.rs                  # 嵌入 UAC 清单
├── config/default.yaml       # 默认配置
├── vendor_db/                # 厂商 CLI 模板 + DNS 优选库
├── src/
│   ├── main.rs
│   ├── app.rs
│   ├── config.rs
│   ├── cli.rs
│   ├── ui/                   # 仅渲染，不含业务逻辑
│   ├── core/                 # 业务逻辑，与 UI 解耦
│   └── windows/              # netsh / powershell / 网卡枚举封装
└── logs/                     # 运行日志（按天滚动，保留 7 天）
```

## 许可证

[GPL-3.0-or-later](LICENSE)

## 参考

- [ratatui](https://github.com/ratatui-org/ratatui) · [crossterm](https://github.com/crossterm-rs/crossterm)
- [windows-rs](https://github.com/microsoft/windows-rs) · [serialport-rs](https://github.com/serialport/serialport-rs) · [petgraph](https://github.com/petgraph/petgraph)
- [D2](https://d2lang.com) · [Graphviz](https://graphviz.org)
