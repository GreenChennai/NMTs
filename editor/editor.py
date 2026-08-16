#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""NMTs 拓扑编辑器独立窗口（pywebview 拉起 React Flow 画布）。

用法：python editor.py [topology.json 路径]
依赖：pip install pywebview
"""
import json
import os
import sys

import webview

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
    api = Api()
    webview.create_window("NMTs 拓扑编辑器", HTML, js_api=api, width=1280, height=800)
    webview.start()


if __name__ == "__main__":
    main()
