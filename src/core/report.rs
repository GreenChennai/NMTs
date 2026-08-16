//! 报告导出（Markdown / 操作审计）。
//!
//! 诊断结果、操作记录可导出为 Markdown 报告，便于交接与归档（桌面运维高频）。

use std::fs;
use std::path::{Path, PathBuf};

use super::net_diag::CheckResult;

/// 生成诊断报告（Markdown）。
pub fn diag_report_md(results: &[CheckResult], summary: &str, extra: &[String]) -> String {
    let mut s = String::new();
    s.push_str("# NMTs 网络诊断报告\n\n");
    s.push_str(&format!(
        "- 生成时间：{}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));
    s.push_str(&format!("- 结论：{summary}\n\n"));

    s.push_str("## 检查结果\n\n");
    s.push_str("| 状态 | 检查项 | 详情 | 修复建议 |\n");
    s.push_str("| --- | --- | --- | --- |\n");
    for r in results {
        let fix = r.fix.as_ref().map(|f| f.label.clone()).unwrap_or_default();
        s.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            r.status.label(),
            r.name,
            r.detail.replace('|', "\\|"),
            fix.replace('|', "\\|")
        ));
    }

    if !extra.is_empty() {
        s.push_str("\n## 附加信息\n\n");
        for e in extra {
            s.push_str(&format!("- {e}\n"));
        }
    }
    s
}

/// 写入报告文件，返回路径。
pub fn write_report(root: &Path, filename: &str, content: &str) -> Result<PathBuf, String> {
    let dir = root.join("reports");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(filename);
    fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(path)
}

/// 追加操作审计记录（谁 / 何时 / 做了什么）。
pub fn audit(root: &Path, action: &str, result: &str) {
    let dir = root.join("logs");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let line = format!(
        "{} | {action} | {result}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("audit.log"))
    {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::net_diag::{CheckResult, Fix, FixKind, Layer, Status};

    fn mk(name: &str, status: Status) -> CheckResult {
        CheckResult {
            id: "x",
            name: name.into(),
            layer: Layer::Basic,
            status,
            detail: "d".into(),
            fix: Some(Fix { kind: FixKind::Manual("m".into()), label: "建议".into() }),
            scope: None,
        }
    }

    #[test]
    fn report_contains_table() {
        let r = vec![mk("网关连通", Status::Ok), mk("DNS", Status::Error)];
        let md = diag_report_md(&r, "检测完成", &[]);
        assert!(md.contains("# NMTs 网络诊断报告"));
        assert!(md.contains("| 检查项 |"));
        assert!(md.contains("网关连通"));
        assert!(md.contains("DNS"));
    }
}
