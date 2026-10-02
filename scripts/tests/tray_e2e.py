#!/usr/bin/env python3
"""wp8f 控制台（单进程 wp8f-gui.exe）端到端验收：真实进程 + 真实 HTTP。

架构：wp8f-gui.exe（托盘 + HTTP API + wry/WebView2 窗口）
        ├─ wp8f.exe / test-server.exe（作业对象托管，比窗口活得久）

覆盖需求：
  1. 「开 始」启动 wp8f 并记录 pid；窗口关闭、WebView2 资源归还系统；只留托盘
  2. wp8f 结束 → test-server 一起结束
  3. 点「开始」= 重启：先结束自己启动的上一批（wp8f + test-server），再起新的
  4. 进程结束（含 taskkill /F）→ 全部子进程退出
  5. 关窗 ≠ 退出进程
  6. 托盘「显示主窗口」= 先 kill wp8f 再重建窗口
  7. 页面 API 与旧 Python 版一致（另见 scripts/tests/api_parity.py）

用法：python scripts/tests/tray_e2e.py
"""
import ctypes
import json
import subprocess
import sys
import time
import urllib.error
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
GUI_EXE = ROOT / "wp8f-gui.exe"
# S1 布局：仓库根只放控制台 wp8f-gui.exe，HUD 与模拟服务端都在 binary/ 下
# （控制台按 binary/ 找它们，见 gui/src/{env,children}.rs）。
WP8F_EXE = ROOT / "binary" / "wp8f.exe"
TS_EXE = ROOT / "binary" / "test-server.exe"
RUNTIME = ROOT / "logs" / "console-host.json"
HOST_LOG = ROOT / "logs" / "console-host.log"

# 本用例会以真实配置启动 wp8f.exe（拖拽预览），而 HUD 退出/拖拽时可能回写配置 ——
# 用真实配置跑测试就不该把结果留在用户的 config/ 里：开始时备份、结束时兜底还原。
CFG_GUARD = {}
for _p in sorted((ROOT / "config").glob("*.json")):
    CFG_GUARD[_p] = _p.read_bytes()

RESULTS: list[tuple[str, bool, str]] = []
GUI = None
CTL = {"url": "", "token": ""}


def check(name: str, ok: bool, detail: str = "") -> bool:
    RESULTS.append((name, bool(ok), str(detail)))
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" · {detail}" if detail else ""), flush=True)
    return bool(ok)


def http(url: str, method: str = "GET", form: dict | None = None,
         json_body: dict | None = None, token: str = "", timeout: float = 30.0):
    if json_body is not None:
        data = json.dumps(json_body).encode()
        headers = {"Content-Type": "application/json"}
    else:
        data = urllib.parse.urlencode(form or {}).encode() if form is not None else None
        headers = {"Content-Type": "application/x-www-form-urlencoded"} if data else {}
    if token:
        headers["X-WP8F-Token"] = token
    req = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, json.loads(r.read().decode("utf-8") or "null")
    except urllib.error.HTTPError as e:
        raw = e.read().decode("utf-8", "replace")
        try:
            return e.code, json.loads(raw)
        except Exception:  # noqa: BLE001
            return e.code, raw
    except Exception as e:  # noqa: BLE001
        return 0, str(e)


def ctl(path: str, form: dict | None = None, method: str = "GET"):
    return http(CTL["url"] + path, method=method, form=form, token=CTL["token"])


def status() -> dict:
    st, body = ctl("/control/status")
    return body if st == 200 and isinstance(body, dict) else {}


def window_url() -> str:
    return status().get("window_url") or ""


def _tasklist(image: str) -> list[int]:
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


def wp8f_pids() -> list[int]:
    return _tasklist("wp8f.exe")


def ts_pids() -> list[int]:
    return _tasklist("test-server.exe")


def pid_alive(pid) -> bool:
    if not pid or int(pid) <= 0:
        return False
    out = subprocess.run(["tasklist", "/FI", f"PID eq {int(pid)}"],
                         capture_output=True, text=True, errors="replace")
    return str(int(pid)) in (out.stdout or "").split()


def wv2_table() -> list[tuple[int, int]]:
    ps = ("Get-CimInstance Win32_Process -Filter \"Name='msedgewebview2.exe'\" | "
          "Select-Object ProcessId,ParentProcessId | ConvertTo-Json -Compress")
    try:
        out = subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", ps],
                             capture_output=True, text=True, errors="replace", timeout=90)
        data = json.loads(out.stdout or "[]")
    except Exception:  # noqa: BLE001
        return []
    if isinstance(data, dict):
        data = [data]
    return [(int(p["ProcessId"]), int(p["ParentProcessId"])) for p in data]


def wv2_of(root_pid, table=None) -> int:
    if not root_pid or int(root_pid) <= 0:
        return 0
    kids: dict[int, list[int]] = {}
    for pid, ppid in (table if table is not None else wv2_table()):
        kids.setdefault(ppid, []).append(pid)
    seen, stack, n = set(), [int(root_pid)], 0
    while stack:
        for c in kids.get(stack.pop(), []):
            if c not in seen:
                seen.add(c)
                n += 1
                stack.append(c)
    return n


class _PMC(ctypes.Structure):
    _fields_ = [
        ("cb", ctypes.c_uint32), ("PageFaultCount", ctypes.c_uint32),
        ("PeakWorkingSetSize", ctypes.c_size_t), ("WorkingSetSize", ctypes.c_size_t),
        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t), ("QuotaPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t), ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
        ("PagefileUsage", ctypes.c_size_t), ("PeakPagefileUsage", ctypes.c_size_t),
    ]


def rss_mb(pid) -> float:
    if not pid or int(pid) <= 0:
        return -1.0
    try:
        k32 = ctypes.WinDLL("kernel32", use_last_error=True)
        k32.OpenProcess.restype = ctypes.c_void_p
        k32.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32]
        h = k32.OpenProcess(0x0400 | 0x0010, 0, int(pid))
        if not h:
            return -1.0
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        psapi.GetProcessMemoryInfo.restype = ctypes.c_int
        psapi.GetProcessMemoryInfo.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32]
        c = _PMC()
        c.cb = ctypes.sizeof(c)
        ok = psapi.GetProcessMemoryInfo(h, ctypes.byref(c), c.cb)
        k32.CloseHandle(h)
        return c.WorkingSetSize / 1048576 if ok else -1.0
    except Exception:  # noqa: BLE001
        return -1.0


def wait(pred, timeout: float = 10.0, interval: float = 0.3) -> bool:
    end = time.time() + timeout
    while time.time() < end:
        if pred():
            return True
        time.sleep(interval)
    return bool(pred())


def kill(pid, force: bool = True) -> None:
    if not pid:
        return
    subprocess.run(["taskkill", "/PID", str(int(pid))] + (["/F"] if force else []),
                   capture_output=True, text=True, errors="replace")


def parent_of(pid) -> int:
    if not pid:
        return 0
    ps = f"(Get-CimInstance Win32_Process -Filter \"ProcessId={int(pid)}\").ParentProcessId"
    try:
        out = subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", ps],
                             capture_output=True, text=True, errors="replace", timeout=30)
        return int((out.stdout or "0").strip() or 0)
    except Exception:  # noqa: BLE001
        return 0


def log_text() -> str:
    try:
        raw = HOST_LOG.read_bytes()
    except OSError:
        return ""
    for enc in ("utf-8", "gbk"):
        try:
            return raw.decode(enc)
        except UnicodeDecodeError:
            continue
    return raw.decode("utf-8", "replace")


def main() -> int:
    global GUI

    print("=== 0. 前置检查 ===")
    if not check("wp8f-gui.exe 存在", GUI_EXE.is_file(), str(GUI_EXE)):
        return 1
    # 本用例要真跑一次拖拽预览：控制台是从 binary/ 拉起 HUD 与 test-server 的（S1 布局），
    # 少任何一个都只会在后面报"wp8f 没在跑"，先在这里说清楚是缺件而不是功能坏了。
    if not check("binary/wp8f.exe 存在（S1 产物布局）", WP8F_EXE.is_file(), str(WP8F_EXE)):
        return 1
    if not check("binary/test-server.exe 存在（S1 产物布局）", TS_EXE.is_file(), str(TS_EXE)):
        return 1
    base_wp8f, base_ts = wp8f_pids(), ts_pids()
    if not check("测试前无残留 wp8f/test-server", not base_wp8f and not base_ts,
                 f"wp8f={base_wp8f} ts={base_ts}"):
        return 1
    # 控制台自己的单实例判据是**端口独占**：已有控制台占着端口时，本次实例会唤醒它的
    # 窗口后自己退出（退出码 0），后面的探测就全打到**用户那个实例**上（假绿 + 可能改用户
    # 状态）。这里先明确拒绝，退出码 2（环境不满足），与其它探针一致。
    base_gui = _tasklist("wp8f-gui.exe")
    if not check("测试前无别的控制台在跑", not base_gui,
                 f"已有 wp8f-gui={base_gui}（先退出它再跑：本用例要独占 HTTP 端口）"
                 if base_gui else "无残留"):
        return 2
    RUNTIME.unlink(missing_ok=True)
    base_wv2 = len(wv2_table())
    print(f"    系统 WebView2 进程基线 = {base_wv2}")

    print("\n=== 1. 启动控制台（托盘 + 窗口） ===")
    GUI = subprocess.Popen([str(GUI_EXE), "--root", str(ROOT)], cwd=str(ROOT))
    if not check("运行态信息生成（控制通道就绪）", wait(lambda: RUNTIME.is_file(), 25),
                 f"pid={GUI.pid}"):
        print(log_text()[-1200:])
        return 1
    info = json.loads(RUNTIME.read_text(encoding="utf-8"))
    CTL["url"], CTL["token"] = info["control_url"], info["token"]
    check("进程 pid 与句柄一致", int(info["pid"]) == GUI.pid, f"{info['pid']} vs {GUI.pid}")
    check("日志出现「托盘已启动」", wait(lambda: "托盘已启动" in log_text(), 10),
          next((l.strip() for l in log_text().splitlines() if "托盘" in l), "(无)"))
    ok_win = wait(lambda: status().get("window_exists") is True, 25)
    st = status()
    check("窗口已打开（window_exists=true）", ok_win, str(st)[:140])
    wurl = wait(lambda: window_url() != "", 20) and window_url()
    check("页面地址可用", bool(wurl), wurl or "(无)")
    check("窗口就是本进程（单进程）", int(st.get("window_pid", 0)) == GUI.pid,
          f"window_pid={st.get('window_pid')} pid={GUI.pid}")
    check("前端静态资源可访问", http(wurl + "/api/health")[0] == 200)
    check("页面认得托盘（tray_available=true）",
          http(wurl + "/api/system")[1].get("tray_available") is True)
    check("WebView2 子进程已起", wait(lambda: wv2_of(GUI.pid) > 0, 25), f"count={wv2_of(GUI.pid)}")
    rss_open = rss_mb(GUI.pid)
    print(f"    开窗时 RSS = {rss_open:.1f}MB（含 WebView2 宿主）")

    print("\n=== 2. 页面 API：拖拽预览（本进程直接托管子进程） ===")
    st_code, r = http(wurl + "/api/wp8f/drag-preview", "POST",
                      json_body={"config": "config/default.json", "drag": True})
    if not check("drag-preview 返回 200", st_code == 200, str(r)[:200]):
        return 1
    pids = r.get("pids", {})
    w_pid, t_pid = pids.get("wp8f"), pids.get("test_server")
    check("wp8f.exe 真的在跑", bool(w_pid) and wait(lambda: w_pid in wp8f_pids(), 10),
          f"pid={w_pid} tasklist={wp8f_pids()}")
    check("test-server.exe 真的在跑", bool(t_pid) and wait(lambda: t_pid in ts_pids(), 10),
          f"pid={t_pid} tasklist={ts_pids()}")
    check("wp8f 的父进程 = 控制台进程", wait(lambda: parent_of(w_pid) == GUI.pid, 5),
          f"parent={parent_of(w_pid)} gui={GUI.pid}")

    print("\n=== 3. 再次启动 = 先结束上一批子进程，再起新的（不再 409） ===")
    st_code, r = http(wurl + "/api/wp8f/launch", "POST", json_body={"config": "config/default.json"})
    if not check("已有 wp8f 时 launch → 200（点「开始」= 重启）", st_code == 200,
                 f"status={st_code} {str(r)[:140]}"):
        return 1
    new_pid = r.get("pid")
    check("上一次的 wp8f 已被结束", wait(lambda: w_pid not in wp8f_pids(), 10),
          f"old={w_pid} now={wp8f_pids()}")
    check("上一次的 test-server 也被一起结束（非拖拽启动不带它）",
          bool(t_pid) and wait(lambda: t_pid not in ts_pids(), 10),
          f"old_ts={t_pid} now={ts_pids()}")
    check("新 wp8f 已起来且 pid 与上一次不同",
          bool(new_pid) and new_pid != w_pid and wait(lambda: new_pid in wp8f_pids(), 10),
          f"new={new_pid} old={w_pid} {wp8f_pids()}")
    check("只剩一个 wp8f.exe", len(wp8f_pids()) == 1, f"{wp8f_pids()}")
    w_pid = new_pid  # 后续步骤（关窗 / 托盘重建）都用新的那个

    print("\n=== 4. 「开 始」= 关窗释放 WebView2，只留托盘（进程不死） ===")
    st_code, r = http(wurl + "/api/gui/window", "POST", json_body={"action": "close"})
    check("close 返回 200", st_code == 200, str(r)[:140])
    check("窗口已销毁（window_exists=false）",
          wait(lambda: status().get("window_exists") is False, 20), str(status())[:140])
    check("WebView2 子进程全部退出（资源释放）", wait(lambda: wv2_of(GUI.pid) == 0, 25),
          f"count={wv2_of(GUI.pid)}")
    check("系统 WebView2 回落到基线附近", len(wv2_table()) <= base_wv2 + 2,
          f"now={len(wv2_table())} base={base_wv2}")
    check("控制台进程仍存活（关窗 ≠ 退出）", pid_alive(GUI.pid))
    check("关窗后 wp8f 仍在跑", w_pid in wp8f_pids(), f"{wp8f_pids()}")
    rss_closed = rss_mb(GUI.pid)
    print(f"    关窗后 RSS = {rss_closed:.1f}MB（WebView2 子进程已释放，宿主常驻）")

    print("\n=== 5. 托盘「显示主窗口」= 先 kill wp8f 再重建窗口 ===")
    st_code, r = ctl("/control/window/show", form={}, method="POST")
    check("控制通道 show 返回 200", st_code == 200, str(r)[:160])
    check("wp8f 被 kill", wait(lambda: not wp8f_pids(), 10), f"{wp8f_pids()}")
    check("窗口被重建（window_exists=true）",
          wait(lambda: status().get("window_exists") is True, 25), str(status())[:140])
    check("WebView2 重新起来", wait(lambda: wv2_of(GUI.pid) > 0, 25), f"count={wv2_of(GUI.pid)}")
    check("test-server 随 wp8f 回收（需求 2）", wait(lambda: not ts_pids(), 15), f"{ts_pids()}")
    check("页面可访问", wait(lambda: http(window_url() + "/api/health")[0] == 200, 20),
          window_url())

    print("\n=== 6. 再关一次、再开一次（可反复） ===")
    http(window_url() + "/api/gui/window", "POST", json_body={"action": "close"})
    check("第二次关窗：WebView2 退出",
          wait(lambda: status().get("window_exists") is False and wv2_of(GUI.pid) == 0, 25),
          f"wv2={wv2_of(GUI.pid)}")
    st_code, r2 = ctl("/control/window/show", form={}, method="POST")
    check("第二次开窗：窗口与 WebView2 回来",
          st_code == 200 and wait(lambda: wv2_of(GUI.pid) > 0, 25), f"wv2={wv2_of(GUI.pid)}")

    print("\n=== 7. 强杀控制台（taskkill /F）→ 子进程必须全退 ===")
    st_code, r = http(window_url() + "/api/wp8f/drag-preview", "POST",
                      json_body={"config": "config/default.json", "drag": True})
    pids2 = r.get("pids", {}) if isinstance(r, dict) else {}
    w2, t2 = pids2.get("wp8f"), pids2.get("test_server")
    check("再次启动 wp8f/test-server",
          st_code == 200 and wait(lambda: w2 in wp8f_pids() and t2 in ts_pids(), 15),
          f"wp8f={wp8f_pids()} ts={ts_pids()}")

    kill(GUI.pid, force=True)
    check("控制台已被强杀", wait(lambda: GUI.poll() is not None, 10), f"pid={GUI.pid}")
    check("wp8f.exe 一并退出（作业对象）", wait(lambda: not wp8f_pids(), 15), f"{wp8f_pids()}")
    check("test-server.exe 一并退出", wait(lambda: not ts_pids(), 15), f"{ts_pids()}")
    check("WebView2 全部退出",
          wait(lambda: wv2_of(GUI.pid) == 0 and len(wv2_table()) <= base_wv2 + 2, 25),
          f"wv2={wv2_of(GUI.pid)} total={len(wv2_table())} base={base_wv2}")

    print("\n=== 8. 收尾 ===")
    print(f"残余: wp8f={wp8f_pids()} test-server={ts_pids()}")
    return 0


if __name__ == "__main__":
    code = 1
    try:
        code = main()
    except KeyboardInterrupt:
        code = 1
    finally:
        try:
            if GUI is not None and GUI.poll() is None:
                kill(GUI.pid, force=True)
        except Exception:  # noqa: BLE001
            pass
        for pid in wp8f_pids() + ts_pids():
            kill(pid, force=True)
        # 只清**本次**实例留下的运行态：没起过实例（例如"已有控制台在跑"直接退出码 2）
        # 时不能删 —— 那份 logs/console-host.json 是用户控制台的（删了它就唤不出窗口了）
        if GUI is not None:
            RUNTIME.unlink(missing_ok=True)
        # 兜底还原用户配置（HUD 回写 / 拖拽保存都可能改到它）
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
        # main() 显式返回 2 = 环境不满足（已有 wp8f/控制台在跑），别被"有 FAIL 就退 1"吞掉
        if code == 2:
            sys.exit(2)
        sys.exit(0 if RESULTS and passed == len(RESULTS) else 1)
