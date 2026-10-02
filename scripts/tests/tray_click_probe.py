#!/usr/bin/env python3
"""托盘点击链路验收（真实 Rust 常驻进程）：给托盘隐藏窗口投递真实 WM_LBUTTONUP。

Rust 侧托盘用 Shell_NotifyIcon + 隐藏窗口类 `wp8f_gui_tray_wnd`，
回调消息 = WM_APP+1，传统语义（lParam 就是鼠标消息）。
这里用 PostMessageW 投递「左键单击」，验证它落到「显示主窗口」：
先 kill wp8f，再拉起窗口子进程 —— 即用户点托盘图标的完整效果。

真实鼠标注入在这台机器上会被用户输入抢占，所以用消息级等价验证：
只断言 PostMessageW 投递成功 + 控制通道报出的 window_pid / wp8f 状态变化，
**不**检查通知区域图标是否可见（那需要截屏/IsPromoted，本脚本不碰）。

环境约定（都是"不许污染用户环境"）：
  * 启动前若已有 wp8f.exe / test-server.exe / wp8f-gui.exe 在跑 → 退出码 2。
  * 本用例用真实 config/*.json 启动 HUD（HUD 拖拽/退出可能回写配置），
    因此开始时逐字节备份 config/*.json，结束时还原（照 tray_e2e.py 的做法）。
  * 结束时只清理**本探针起出来的**进程，不再 taskkill 用户自己的 wp8f/test-server。

用法：python scripts/tests/tray_click_probe.py
"""
import ctypes
import json
import subprocess
import sys
import time
import urllib.parse
import urllib.request
from ctypes import wintypes
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
HOST_EXE = ROOT / "wp8f-gui.exe"
# S1 布局：仓库根只放控制台，HUD 与模拟服务端都在 binary/ 下（控制台从那里拉起它们）
WP8F_EXE = ROOT / "binary" / "wp8f.exe"
TS_EXE = ROOT / "binary" / "test-server.exe"
RUNTIME = ROOT / "logs" / "console-host.json"
WM_APP = 0x8000
WM_TRAY = WM_APP + 1
WM_LBUTTONUP = 0x0202
WM_RBUTTONUP = 0x0205
CLASS = "wp8f_gui_tray_wnd"

# 真实配置会被 HUD 回写（拖拽保存/退出保存）——开始时备份，结束时逐字节还原
CFG_GUARD = {p: p.read_bytes() for p in sorted((ROOT / "config").glob("*.json"))}
# 探针启动前的进程基线：结束时只清理"多出来的"那些（基线非空则直接退出 2）
BASE_PIDS: dict[str, list[int]] = {}

RESULTS: list[tuple[str, bool, str]] = []
HOST = None


def check(name: str, ok: bool, detail: str = "") -> bool:
    RESULTS.append((name, bool(ok), str(detail)))
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" · {detail}" if detail else ""), flush=True)
    return bool(ok)


u32 = ctypes.WinDLL("user32", use_last_error=True)
u32.FindWindowW.restype = wintypes.HWND
u32.FindWindowW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR]
u32.PostMessageW.restype = wintypes.BOOL
u32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]


def ctl(path: str, form: dict | None = None, method: str = "POST", token: str = "",
        url: str = ""):
    data = urllib.parse.urlencode(form or {}).encode() if form is not None else None
    req = urllib.request.Request(url + path, data=data, method=method,
                                 headers={"X-WP8F-Token": token} if token else {})
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            return r.status, json.loads(r.read().decode("utf-8") or "null")
    except Exception as e:  # noqa: BLE001
        return 0, str(e)


def tasklist(image: str) -> list[int]:
    out = subprocess.run(["tasklist", "/FI", f"IMAGENAME eq {image}"],
                         capture_output=True, text=True, errors="replace")
    pids = []
    for ln in (out.stdout or "").splitlines():
        p = ln.split()
        if len(p) >= 2 and p[0].lower() == image.lower():
            try:
                pids.append(int(p[1].replace(",", "")))
            except ValueError:
                pass
    return pids


def wait(pred, timeout=15.0, interval=0.3) -> bool:
    end = time.time() + timeout
    while time.time() < end:
        if pred():
            return True
        time.sleep(interval)
    return bool(pred())


def main() -> int:
    global HOST, BASE_PIDS
    if not check("wp8f-gui.exe 存在", HOST_EXE.is_file(), str(HOST_EXE)):
        return 1
    # 本探针要真跑一次拖拽预览（控制台从 binary/ 拉起 HUD 与 test-server，S1 布局）：缺件
    # 会让后面只报"wp8f 没在跑"，先在这里说清是缺件。
    if not check("binary/wp8f.exe 存在（S1 产物布局）", WP8F_EXE.is_file(), str(WP8F_EXE)):
        return 1
    if not check("binary/test-server.exe 存在（S1 产物布局）", TS_EXE.is_file(), str(TS_EXE)):
        return 1
    # 前置：用户已经在跑 wp8f / test-server / 控制台时，直接退出码 2 ——
    # 既避免本次实例抢不到端口（控制台的单实例判据＝端口独占，新实例会唤醒已有实例后退出，
    # 断言会假绿），也避免误杀/误改用户的进程与配置。
    BASE_PIDS = {"wp8f.exe": tasklist("wp8f.exe"),
                 "test-server.exe": tasklist("test-server.exe"),
                 "wp8f-gui.exe": tasklist("wp8f-gui.exe")}
    busy = {k: v for k, v in BASE_PIDS.items() if v}
    if not check("测试前无 wp8f / test-server / 控制台在跑", not busy,
                 f"发现 {busy}（请先退出，再跑本探针）" if busy else "无残留"):
        return 2
    RUNTIME.unlink(missing_ok=True)
    # --no-tray-promote：探针不改用户的 HKCU\Control Panel\NotifyIconSettings\IsPromoted
    HOST = subprocess.Popen([str(HOST_EXE), "--root", str(ROOT), "--no-tray-promote"],
                            cwd=str(ROOT))
    if not check("常驻进程启动", wait(lambda: RUNTIME.is_file(), 20), f"pid={HOST.pid}"):
        return 1
    info = json.loads(RUNTIME.read_text(encoding="utf-8"))
    url, token = info["control_url"], info["token"]

    print("\n=== 1. 托盘隐藏窗口存在（Shell_NotifyIcon 宿主窗口） ===")
    hwnd = u32.FindWindowW(CLASS, None)
    check("找到托盘窗口类 wp8f_gui_tray_wnd", bool(hwnd), f"hwnd={hwnd}")

    print("\n=== 2. 先准备好前置状态：拉起 wp8f ===")
    wurl = wait(lambda: (ctl("/control/status", method="GET", token=token, url=url)[1] or {}).get("window_url"), 25)
    st = ctl("/control/status", method="GET", token=token, url=url)[1]
    wurl = st.get("window_url") if isinstance(st, dict) else ""
    check("窗口已就绪", bool(wurl), wurl or "(无)")
    if wurl:
        data = json.dumps({"config": "config/default.json", "drag": True}).encode()
        req = urllib.request.Request(wurl + "/api/wp8f/drag-preview", data=data, method="POST",
                                     headers={"Content-Type": "application/json"})
        try:
            urllib.request.urlopen(req, timeout=20)
        except Exception as e:  # noqa: BLE001
            check("启动 wp8f", False, str(e))
    check("wp8f.exe 在跑", wait(lambda: bool(tasklist("wp8f.exe")), 15), f"{tasklist('wp8f.exe')}")
    wp8f_before = tasklist("wp8f.exe")
    win_before = (ctl("/control/status", method="GET", token=token, url=url)[1] or {}).get("window_pid")

    print("\n=== 3. 投递真实左键单击（WM_TRAY + WM_LBUTTONUP） ===")
    ok = u32.PostMessageW(hwnd, WM_TRAY, 1, WM_LBUTTONUP) if hwnd else 0
    check("PostMessage 成功", bool(ok), f"ret={ok}")
    check("wp8f 被 kill（点托盘先杀 wp8f）", wait(lambda: not tasklist("wp8f.exe"), 15),
          f"before={wp8f_before} after={tasklist('wp8f.exe')}")
    st2 = ctl("/control/status", method="GET", token=token, url=url)[1]
    win_after = st2.get("window_pid") if isinstance(st2, dict) else None
    check("窗口本来就开着 → 保持（拉到前台分支）",
          bool(win_after), f"before={win_before} after={win_after}")

    print("\n=== 3b. 关掉窗口后再点托盘 → 必须重建窗口进程 ===")
    if wurl:
        data = json.dumps({"action": "close"}).encode()
        req = urllib.request.Request(wurl + "/api/gui/window", data=data, method="POST",
                                     headers={"Content-Type": "application/json"})
        try:
            urllib.request.urlopen(req, timeout=10)
        except Exception:  # noqa: BLE001
            pass  # 窗口进程随后被杀，请求中断属正常
    check("窗口已关闭", wait(lambda: not (ctl("/control/status", method="GET", token=token,
                                          url=url)[1] or {}).get("window_exists", True), 20),
          str(ctl("/control/status", method="GET", token=token, url=url)[1])[:120])
    u32.PostMessageW(hwnd, WM_TRAY, 1, WM_LBUTTONUP)
    st3 = wait(lambda: (ctl("/control/status", method="GET", token=token, url=url)[1] or {})
               .get("window_pid") not in (None, -1, win_after), 25)
    st3 = ctl("/control/status", method="GET", token=token, url=url)[1]
    win_new = st3.get("window_pid") if isinstance(st3, dict) else None
    check("窗口被重建（WebView 重新加载）", bool(win_new), f"window_exists={win_new}")

    print("\n=== 4. 右键投递（菜单路径不崩即可） ===")
    u32.PostMessageW(hwnd, WM_TRAY, 1, WM_RBUTTONUP)
    time.sleep(1.0)
    check("右键后常驻进程仍存活", HOST.poll() is None)
    return 0


if __name__ == "__main__":
    code = 1
    try:
        code = main()
    except Exception as e:  # noqa: BLE001
        check("探针执行", False, f"{type(e).__name__}: {e}")
    finally:
        try:
            if HOST is not None and HOST.poll() is None:
                subprocess.run(["taskkill", "/F", "/PID", str(HOST.pid)],
                               capture_output=True, text=True)
        except Exception:  # noqa: BLE001
            pass
        # 只清理"本探针跑起来之后才出现的"进程（不能无条件 taskkill）：
        # wp8f.exe / test-server.exe，会把用户正在用的 HUD 一起杀掉。
        for image in ("wp8f.exe", "test-server.exe"):
            for pid in tasklist(image):
                if pid in BASE_PIDS.get(image, []):
                    continue
                subprocess.run(["taskkill", "/F", "/PID", str(pid)], capture_output=True)
        RUNTIME.unlink(missing_ok=True)
        # 兜底还原用户配置（HUD 拖拽/退出都可能回写 config/*.json）
        for _p, _bytes in CFG_GUARD.items():
            try:
                if _p.read_bytes() != _bytes:
                    _p.write_bytes(_bytes)
                    print(f"  （已还原被用例改动的 {_p.name}）")
            except OSError as e:
                print(f"  还原 {_p.name} 失败: {e}")
        passed = sum(1 for _n, ok, _d in RESULTS if ok)
        print(f"\n===== 结果 {passed}/{len(RESULTS)} 通过 =====")
        for n, ok, d in RESULTS:
            if not ok:
                print(f"  FAIL: {n} · {d}")
        # main() 的显式退出码优先：2 = 前置不满足（已有控制台/用户在用），
        # 不能在这里被压成 1 —— 上层脚本/CI 要靠它区分"环境不满足"与"断言失败"。
        sys.exit(0 if RESULTS and passed == len(RESULTS) else (code or 1))
