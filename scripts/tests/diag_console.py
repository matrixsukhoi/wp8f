#!/usr/bin/env python3
"""对着**正在运行**的控制台实例做体检：控制通道 → 窗口 API → 页面内容。

用法：python scripts/tests/diag_console.py
"""
import json
import sys
import urllib.parse
import urllib.request
from pathlib import Path

# 输出重定向到文件时 Windows 默认 GBK，打印 ✓/✕ 会 UnicodeEncodeError 直接崩；
# 统一强制 UTF-8，让 CI/重定向场景与真实控制台行为一致。
sys.stdout.reconfigure(encoding="utf-8", errors="replace")


def _find_root() -> Path:
    """仓库根：从本文件向上找 Cargo.toml（脚本可能被放在 scripts/tests/ 下）"""
    for probe in [Path(__file__).resolve().parent, *Path(__file__).resolve().parents]:
        if (probe / "Cargo.toml").is_file() and (probe / "gui").is_dir():
            return probe
    return Path(__file__).resolve().parents[2]


ROOT = _find_root()
RUNTIME = ROOT / "logs" / "console-host.json"


def get(url: str, timeout: float = 8.0) -> str:
    with urllib.request.urlopen(url, timeout=timeout) as r:
        return r.read().decode("utf-8", "replace")


def main() -> int:
    if not RUNTIME.is_file():
        print(f"✗ 没有 {RUNTIME} —— 控制台没在跑（或 root 推断错了，运行态文件写到别处）")
        return 1
    j = json.loads(RUNTIME.read_text(encoding="utf-8"))
    print(f"常驻 pid     : {j['pid']}")
    print(f"控制通道     : {j['control_url']}")

    # 控制通道状态（运行态文件可能是被强杀后残留的 → 先探活）
    try:
        ping = urllib.request.Request(j["control_url"] + "/control/ping",
                                      headers={"X-WP8F-Token": j["token"]})
        urllib.request.urlopen(ping, timeout=5)
    except Exception as e:  # noqa: BLE001
        print(f"✗ 控制通道无响应（{e}）—— 该运行态文件已过期（进程被强杀后残留）")
        print("  请先启动控制台：双击 wp8f-gui.exe，或 .\\wp8f-gui.exe --root .")
        return 2
    req = urllib.request.Request(j["control_url"] + "/control/status",
                                 headers={"X-WP8F-Token": j["token"]})
    st = json.loads(urllib.request.urlopen(req, timeout=8).read())
    print(f"窗口 pid     : {st.get('window_pid')}  (exists={st.get('window_exists')})")
    print(f"wp8f running : {st.get('wp8f_running')}")
    url = (st.get("window_url") or "").rstrip("/")
    print(f"窗口地址     : {url or '（窗口进程还没上报 —— 可能没起来）'}")
    if not url:
        return 1

    # 窗口 API
    print("\n--- 窗口 API ---")
    for path in ("/api/health", "/api/config/list", "/api/system", "/api/resident/status"):
        try:
            body = get(url + path)
            print(f"  {path:26} {body[:160]}")
        except Exception as e:  # noqa: BLE001
            print(f"  {path:26} ✗ {e}")

    # 页面与静态资源
    print("\n--- 页面 ---")
    try:
        html = get(url + "/")
        checks = {
            "config-select": "config-select" in html,
            "id=notify": 'id="notify"' in html,
            "app.js": "static/app.js" in html,
            "长度>3000": len(html) > 3000,
        }
        print(f"  首页 len={len(html)}  {checks}")
    except Exception as e:  # noqa: BLE001
        print(f"  首页 ✗ {e}")
    for asset in ("/static/app.js", "/static/style.css", "/static/vendor/echarts.min.js"):
        try:
            print(f"  {asset:34} {len(get(url + asset))} 字节")
        except Exception as e:  # noqa: BLE001
            print(f"  {asset:34} ✗ {e}")

    # 磁盘上的配置
    print("\n--- 磁盘 config/ ---")
    for p in sorted((ROOT / "config").glob("*.json")):
        print(f"  {p.name}  {p.stat().st_size} 字节")

    print(f"\n下一步（看页面里的 DOM/JS 异常）：\n    python scripts/tests/ooda.py --url {url}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
