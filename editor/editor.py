#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""NMTs 拓扑编辑器独立窗口。

用法：python editor.py [topology.json 路径]
依赖（可选）：pip install pywebview
  - 已安装 pywebview → 用独立窗口打开，保存/读取经 bridge 直写 topology.json
  - 未安装 pywebview → 回退到系统默认浏览器打开编辑器（保存将下载 topology.json）
"""
import json
import os
import sys

try:
    import webview
    HAS_WEBVIEW = True
except ImportError:
    webview = None
    HAS_WEBVIEW = False

HTML = os.path.join(os.path.dirname(os.path.abspath(__file__)), "topology_editor.html")
JSON_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "topology.json")


class Api:
    def save(self, content):
        with open(JSON_PATH, "w", encoding="utf-8") as f:
            f.write(content)
        return True

    def load(self):
        if os.path.exists(JSON_PATH):
            with open(JSON_PATH, "r", encoding="utf-8") as f:
                return f.read()
        return json.dumps({"devices": [], "links": []})


def main():
    global JSON_PATH
    if len(sys.argv) > 1:
        JSON_PATH = os.path.abspath(sys.argv[1])

    if HAS_WEBVIEW:
        api = Api()
        webview.create_window("NMTs 拓扑编辑器", HTML, js_api=api, width=1280, height=800)
        webview.start()
    else:
        # 无 pywebview：用系统默认浏览器打开编辑器（HTML 内 load 走文件选择 / save 走下载）
        import webbrowser
        url = "file://" + HTML.replace("\\", "/")
        webbrowser.open(url)
        print("[NMTs] 未安装 pywebview，已用默认浏览器打开编辑器（保存将下载 topology.json）")


if __name__ == "__main__":
    main()
