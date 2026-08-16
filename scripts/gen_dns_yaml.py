#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""把 DNS优选.md 归一化为 vendor_db/dns_providers.yaml。
一次性生成脚本：解析原始文本，补齐 CN 国内节点，输出结构化 YAML。
"""
import re
import sys
from pathlib import Path

SRC = Path(r"E:\GCissue\Ai Agent 软件编写\DNS优选.md")
DST = Path(r"E:\平日资料\GitHub\NMTs\vendor_db\dns_providers.yaml")

CAT_MAP = {
    "Ipv4_Default": ("ipv4_default", "ipv4", "default", "默认 IPv4"),
    "Ipv4_Family":  ("ipv4_family",  "ipv4", "family",  "家庭过滤 IPv4"),
    "Ipv4_Secure":  ("ipv4_secure",  "ipv4", "secure",  "安全拦截 IPv4"),
    "Ipv6_Default": ("ipv6_default", "ipv6", "default", "默认 IPv6"),
    "Ipv6_Family":  ("ipv6_family",  "ipv6", "family",  "家庭过滤 IPv6"),
    "Ipv6_Secure":  ("ipv6_secure",  "ipv6", "secure",  "安全拦截 IPv6"),
}

def parse_line(line):
    line = line.strip()
    if not line or line.startswith("#") or line.startswith("["):
        return None
    # 格式：国家 - 名称=主,备,推荐
    m = re.match(r"^([A-Z]{2})\s*-\s*(.+?)\s*=\s*(.+)$", line)
    if not m:
        return None
    country, name, rest = m.group(1), m.group(2).strip(), m.group(3)
    parts = [p.strip() for p in rest.split(",")]
    primary = parts[0] if len(parts) > 0 else ""
    secondary = parts[1] if len(parts) > 1 else ""
    recommended = True
    if len(parts) > 2:
        recommended = parts[2].strip().lower() != "false"
    return {"country": country, "name": name, "primary": primary,
            "secondary": secondary, "recommended": recommended}

def main():
    text = SRC.read_text(encoding="utf-8")
    providers = []
    cat = None
    for line in text.splitlines():
        s = line.strip()
        if s.startswith("["):
            cat = s.strip("[]")
            continue
        if cat and cat in CAT_MAP:
            rec = parse_line(s)
            if rec:
                cid, proto, kind, label = CAT_MAP[cat]
                rec.update({"category": cid})
                providers.append(rec)

    # 补齐 CN 国内节点（原文档未收录，模块二按 prefer_country: CN 就近优先）
    cn_extra = [
        ("阿里 AliDNS", "223.5.5.5", "223.6.6.6"),
        ("腾讯 DNSPod", "119.29.29.29", "119.28.28.28"),
        ("百度 Baidu DNS", "180.76.76.76", ""),
        ("114DNS", "114.114.114.114", "114.114.115.115"),
    ]
    for name, primary, secondary in cn_extra:
        providers.insert(0, {"country": "CN", "name": name, "primary": primary,
                             "secondary": secondary, "recommended": True,
                             "category": "ipv4_default"})

    # 去重（按 category + primary）
    seen = set()
    uniq = []
    for p in providers:
        key = (p["category"], p["primary"])
        if key in seen:
            continue
        seen.add(key)
        uniq.append(p)

    lines = []
    lines.append("# NMTs DNS 优选数据源（由 DNS优选.md 归一，含 CN 国内节点补齐）")
    lines.append("# 字段：country / name / category / primary / secondary / recommended")
    lines.append("version: 1")
    lines.append("")
    lines.append("categories:")
    for cid, proto, kind, label in CAT_MAP.values():
        lines.append(f"  - id: {cid}")
        lines.append(f"    protocol: {proto}")
        lines.append(f"    kind: {kind}")
        lines.append(f"    label: \"{label}\"")
    lines.append("")
    lines.append("providers:")
    for p in uniq:
        lines.append("  - country: \"%s\"" % p["country"])
        lines.append("    name: \"%s\"" % p["name"].replace('"', "'"))
        lines.append("    category: %s" % p["category"])
        lines.append("    primary: \"%s\"" % p["primary"])
        lines.append("    secondary: \"%s\"" % p["secondary"])
        lines.append("    recommended: %s" % ("true" if p["recommended"] else "false"))

    DST.parent.mkdir(parents=True, exist_ok=True)
    DST.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"OK: {len(uniq)} 条 DNS 记录 -> {DST}")

if __name__ == "__main__":
    main()
