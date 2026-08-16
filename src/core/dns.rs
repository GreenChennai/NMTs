//! 模块二：DNS 优选引擎。
//!
//! 按「类别 × 协议」从 `vendor_db/dns_providers.yaml` 筛选候选，并发测速（ping
//! RTT 就近代理），`prefer_country` 命中者加权优先，产出排名表，可一键应用。
#![allow(dead_code)] // label / recommended 字段供后续 UI 展示

use serde::Deserialize;

use crate::windows::run;

const DNS_YAML: &str = include_str!("../../vendor_db/dns_providers.yaml");

/// DNS 分类。
#[derive(Debug, Clone, Deserialize)]
pub struct DnsCategory {
    pub id: String,
    pub protocol: String,
    pub kind: String,
    pub label: String,
}

/// DNS 候选。
#[derive(Debug, Clone, Deserialize)]
pub struct DnsProvider {
    pub country: String,
    pub name: String,
    pub category: String,
    pub primary: String,
    #[serde(default)]
    pub secondary: String,
    #[serde(default)]
    pub recommended: bool,
}

/// DNS 库。
#[derive(Debug, Clone, Default)]
pub struct DnsDb {
    pub categories: Vec<DnsCategory>,
    pub providers: Vec<DnsProvider>,
}

impl DnsDb {
    pub fn load() -> Self {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            categories: Vec<DnsCategory>,
            #[serde(default)]
            providers: Vec<DnsProvider>,
        }
        serde_yaml::from_str::<Raw>(DNS_YAML).map(|r| Self {
            categories: r.categories,
            providers: r.providers,
        }).unwrap_or_default()
    }

    /// 按类别（default/family/secure）与协议（ipv4/ipv6/both）筛选候选。
    pub fn filter(&self, kinds: &[String], prefer_ipv: &str) -> Vec<DnsProvider> {
        let mut out: Vec<DnsProvider> = Vec::new();
        for p in &self.providers {
            let Some(cat) = self.categories.iter().find(|c| c.id == p.category) else {
                continue;
            };
            if !kinds.contains(&cat.kind) {
                continue;
            }
            if prefer_ipv != "both" && cat.protocol != prefer_ipv {
                continue;
            }
            out.push(p.clone());
        }
        out
    }
}

/// 单候选测速结果。
#[derive(Debug, Clone)]
pub struct DnsBench {
    pub provider: DnsProvider,
    pub latency_ms: Option<u32>,
    pub reachable: bool,
}

/// 并发测速：ping 每个候选 primary，取 RTT。
pub async fn benchmark(providers: &[DnsProvider], max: usize) -> Vec<DnsBench> {
    let subset: Vec<DnsProvider> = providers.iter().take(max).cloned().collect();
    let mut handles = Vec::new();
    for p in subset {
        handles.push(tokio::spawn(async move {
            let rtt = ping_rtt(&p.primary).await;
            DnsBench {
                provider: p,
                latency_ms: rtt,
                reachable: rtt.is_some(),
            }
        }));
    }
    let mut results = Vec::new();
    for h in handles {
        if let Ok(r) = h.await {
            results.push(r);
        }
    }
    results
}

/// 排序：prefer_country 命中优先，其次按延迟升序（不可达排最后）。
pub fn rank(mut results: Vec<DnsBench>, prefer_country: &str) -> Vec<DnsBench> {
    results.sort_by(|a, b| {
        let a_cn = a.provider.country == prefer_country;
        let b_cn = b.provider.country == prefer_country;
        // 国家命中优先
        b_cn.cmp(&a_cn)
            // 可达性
            .then_with(|| b.reachable.cmp(&a.reachable))
            // 延迟
            .then_with(|| a.latency_ms.unwrap_or(u32::MAX).cmp(&b.latency_ms.unwrap_or(u32::MAX)))
    });
    results
}

async fn ping_rtt(ip: &str) -> Option<u32> {
    let ip = ip.to_string();
    tokio::task::spawn_blocking(move || {
        let out = run("ping", &["-n", "1", "-w", "1000", &ip], std::time::Duration::from_secs(4));
        parse_rtt(&out.stdout)
    })
    .await
    .unwrap_or(None)
}

/// 从 ping 输出解析 RTT（中文「时间=1ms」/ 英文「time=1ms」/「时间<1ms」）。
fn parse_rtt(stdout: &str) -> Option<u32> {
    use std::sync::OnceLock;
    static LT1: OnceLock<regex::Regex> = OnceLock::new();
    static EQ: OnceLock<regex::Regex> = OnceLock::new();
    let lt1 = LT1.get_or_init(|| regex::Regex::new(r"(?:时间|[Tt]ime)\s*<\s*1").unwrap());
    let eq = EQ.get_or_init(|| regex::Regex::new(r"(?:时间|[Tt]ime)\s*=\s*(\d+)").unwrap());

    if lt1.is_match(stdout) {
        return Some(0);
    }
    if let Some(c) = eq.captures(stdout) {
        return c.get(1).and_then(|m| m.as_str().parse().ok());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_and_filter() {
        let db = DnsDb::load();
        assert!(db.providers.len() >= 70);
        let v4 = db.filter(&["default".into()], "ipv4");
        assert!(!v4.is_empty());
        assert!(v4.iter().any(|p| p.country == "CN"));
    }

    #[test]
    fn rank_prefers_country_then_latency() {
        let mk = |country: &str, lat: Option<u32>| DnsBench {
            provider: DnsProvider {
                country: country.into(),
                name: "x".into(),
                category: "ipv4_default".into(),
                primary: "1.1.1.1".into(),
                secondary: String::new(),
                recommended: true,
            },
            latency_ms: lat,
            reachable: lat.is_some(),
        };
        let v = vec![mk("US", Some(10)), mk("CN", Some(50)), mk("CN", Some(5))];
        let r = rank(v, "CN");
        assert_eq!(r[0].provider.country, "CN");
        assert_eq!(r[0].latency_ms, Some(5));
        assert_eq!(r[1].provider.country, "CN");
    }

    #[test]
    fn parse_rtt_zh_en() {
        assert_eq!(parse_rtt("来自 223.5.5.5 的回复: 字节=32 时间=8ms TTL=117"), Some(8));
        assert_eq!(parse_rtt("Reply from 8.8.8.8: bytes=32 time=12ms TTL=117"), Some(12));
        assert_eq!(parse_rtt("时间<1ms TTL=64"), Some(0));
        assert_eq!(parse_rtt("请求超时"), None);
    }
}
