# NMTs 发布说明

## v0.1.0（草稿）

首个里程碑：项目骨架 + TUI 框架 + 配置加载 + 模块一「网络诊断」基础层。

### 新增

- **项目骨架**：Rust (stable, MSVC) + `ratatui` / `crossterm` TUI，`build.rs` 嵌入 UAC 清单（`requireAdministrator`）。
- **配置体系**：`config/default.yaml` 默认配置内嵌二进制，运行时 `config/config.yaml` 覆盖，`serde_yaml` 加载 / 保存。
- **数据文件**：
  - `vendor_db/dns_providers.yaml` — DNS 优选数据源（74 条，含 CN 国内节点补齐）。
  - `vendor_db/{huawei_vrp,h3c_vrp,cisco_ios}.yaml` — 三厂商命令模板（模块三/四备用）。
- **windows 封装层**：子进程静默执行（GBK/UTF-8 解码、超时）、管理员检测、netsh / PowerShell 封装、多网卡枚举与「当前上网网卡」判定（默认路由 + 跃点排序 + 虚拟网卡过滤）。
- **模块一（基础层诊断）**：当前上网网卡判定、DHCP / IP 配置、默认路由 / 网关、网关连通、DNS 解析、系统代理检测、虚拟网卡干扰，共 7 项检查，逐项区分 `正常 / 警告 / 异常` 与 `自动修复 / 手动步骤`。
- **执行反馈**：诊断过程经 `mpsc` 流式推送进度 / 结果 / 命令回显，TUI 步骤清单 + 实时日志面板实时刷新，杜绝「看着像卡死」。
- **CLI 模式**：`nmts diag --quick` 无界面快速诊断，可脚本化。

### 已知限制

- 模块二～五为占位界面，规划于 v0.2～v0.6 逐步落地。
- 诊断本机环境层（驱动 / MTU）与外部因素层（病毒 / 环路 / MAC 锁）规划于 v0.2。
- `Get-NetAdapter` / `Get-NetIPConfiguration` 依赖 Windows 8+（目标平台 Win10/11）。
