#!/usr/bin/env python3
"""排查"拖动燃油条弹出黑色命令行窗口"：触发 FM 调用前后，对比**所有可见顶层窗口**。

比只查 ConsoleWindowClass 更广：Win11 默认控制台宿主是 Windows Terminal
（类名 CASCADIA_HOSTING_WINDOW_CLASS），用类名过滤会漏掉。
"""
import ctypes
import json
import subprocess
import sys
import tempfile
import time
import urllib.request
from ctypes import wintypes
from pathlib import Path

# 输出重定向到文件时 Windows 默认 GBK，打印 ✓/✕ 会 UnicodeEncodeError 直接崩；
# 统一强制 UTF-8，让 CI/重定向场景与真实控制台行为一致。
sys.stdout.reconfigure(encoding="utf-8", errors="replace")

ROOT = Path(__file__).resolve().parents[2]
EXE = ROOT / "wp8f-gui.exe"
# S1 布局：仓库根只放控制台；FM 解析可执行文件在 binary/ 下（gui/src/env.rs 从那里找它）
FM_EXE = ROOT / "binary" / "flightmodel.exe"
PORT = 8899

# 控制台宿主窗口的类名（小写）。子进程被分配控制台时出现的就是这几种：
# conhost（ConsoleWindowClass）、Win11 的 Windows Terminal（CASCADIA_HOSTING_WINDOW_CLASS）、
# 以及 WT 的伪控制台窗口。只认它们，避免把用户同期打开的别的窗口算进来。
CONSOLE_CLASSES = {"consolewindowclass", "cascadia_hosting_window_class", "pseudoconsolewindow"}

u32 = ctypes.WinDLL("user32", use_last_error=True)
EnumProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
u32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
u32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]


def visible_windows() -> dict:
    """{hwnd: (class, title, pid)} 仅可见顶层窗口"""
    out: dict = {}

    def cb(hwnd, _lp):
        if not u32.IsWindowVisible(hwnd):
            return True
        cls = ctypes.create_unicode_buffer(256)
        u32.GetClassNameW(hwnd, cls, 256)
        t = ctypes.create_unicode_buffer(512)
        u32.GetWindowTextW(hwnd, t, 512)
        pid = wintypes.DWORD()
        u32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        out[int(hwnd)] = (cls.value, t.value.strip(), int(pid.value))
        return True

    u32.EnumWindows(EnumProc(cb), 0)
    return out


def conhost_count() -> int:
    out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq conhost.exe"],
                         capture_output=True, text=True, errors="replace")
    return sum(1 for ln in (out.stdout or "").splitlines() if ln.lower().startswith("conhost.exe"))


def wt_count() -> int:
    out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq WindowsTerminal.exe"],
                         capture_output=True, text=True, errors="replace")
    return sum(1 for ln in (out.stdout or "").splitlines()
               if ln.lower().startswith("windowsterminal.exe"))


# 本次实例的 stdout+stderr 落临时文件（无管道：实例若一直活着，管道写满会把双方都挂住）
_CHILD_LOG = tempfile.TemporaryFile(mode="w+", encoding="utf-8", errors="replace")


def child_out() -> str:
    """本次实例的输出（只在它退出后读；失败时用来区分"被拒"还是"端口被别人占"）"""
    _CHILD_LOG.flush()
    _CHILD_LOG.seek(0)
    return _CHILD_LOG.read()


def runtime_pid() -> int:
    """运行态文件里的 pid（读不到 = -1）：端口上的服务者是不是**本次** spawn 的实例，靠它对账"""
    try:
        return int(json.loads((ROOT / "logs" / "console-host.json")
                              .read_text(encoding="utf-8"))["pid"])
    except Exception:  # noqa: BLE001
        return -1


def gui_pids() -> list[int]:
    """本机已在跑的 wp8f-gui.exe。

    控制台自己的单实例判据是**端口独占**（不再有全机命名互斥体）：探针用的是 PORT 那个
    专用端口，用户那个 8765 的控制台因此不会"顶掉"本次实例 —— 但两者**共用同一份
    logs/console-host.json 与控制台日志**，探针会把用户那份顶掉（之后双击就唤不出窗口）。
    所以仍旧必须先清场（退出码 2），不能因为"端口不同、实例起得来"就开跑。
    """
    try:
        out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq wp8f-gui.exe"],
                             capture_output=True, text=True, errors="replace")
    except Exception:  # noqa: BLE001
        return []
    pids = []
    for ln in (out.stdout or "").splitlines():
        p = ln.split()
        if len(p) >= 2 and p[0].lower() == "wp8f-gui.exe":
            try:
                pids.append(int(p[1].replace(",", "")))
            except ValueError:
                pass
    return pids


# 前置：已有控制台在跑就先退出（见 gui_pids 的说明）
_OTHER_GUI = gui_pids()
if _OTHER_GUI:
    print(f"[SKIP] 已有控制台在跑（pid={_OTHER_GUI}）—— 本次探针会与它共用 "
          f"logs/console-host.json 与控制台日志，先退出它再跑测试", flush=True)
    raise SystemExit(2)

# 前置：本探针靠连续调 FM 接口（flightmodel 子进程）来采样是否闪黑框；FM 可执行文件在
# binary/ 下（S1 布局）。缺了它调用会立刻 500，"没闪黑框"就成了恒真 —— 必须挡在前面。
if not FM_EXE.is_file():
    print(f"[SKIP] 找不到 {FM_EXE}（先跑 bash scripts/build.sh）", flush=True)
    raise SystemExit(2)

proc = subprocess.Popen([str(EXE), "--root", str(ROOT), "--port", str(PORT), "--no-window",
                         "--no-tray-promote"],
                        cwd=str(ROOT), stdout=_CHILD_LOG, stderr=subprocess.STDOUT)
url = f"http://127.0.0.1:{PORT}"
try:
    for _ in range(100):
        try:
            urllib.request.urlopen(url + "/api/health", timeout=0.4)
            break
        except Exception:
            time.sleep(0.1)

    # 单实例判据 = 端口独占（没有全机命名互斥体）。但"端口上有服务"
    # 不等于"端口上的服务就是本次 spawn 的实例"：已有控制台会秒回 /api/health，而本次实例
    # 还要走完「唤出它的窗口 → 打印提示 → 退出」才消失 —— 只 poll() 一次会输给这个竞态，
    # 探测就打到**用户那个实例**上（假绿，实测踩过）。所以先按运行态文件里的 pid 对账。
    if proc.poll() is None and runtime_pid() != proc.pid:
        try:
            proc.wait(timeout=5)          # 被拒时它会在这段时间里退出
        except subprocess.TimeoutExpired:
            pass
    if proc.poll() is not None:
        out = child_out()
        # 针脚是**英文固定串**：控制台 stdout 的那三条诊断不随 i18n 界面语言变
        # （gui/src/main.rs 的 port_busy_exit）；"已有控制台"这条同时要求退出码 0。
        if proc.returncode == 0 and "already running" in out:
            print(f"[SKIP] 端口 {PORT} 上已有控制台在运行（本次实例唤出它的窗口后退出）；"
                  f"请先退出控制台再跑测试", flush=True)
            raise SystemExit(2)
        if "occupied by another program" in out or "could not be woken up" in out:
            print(f"[SKIP] 端口 {PORT} 不可用，控制台实例没起来：{out.strip()}", flush=True)
            raise SystemExit(2)
        print(f"[FAIL] 本次实例启动即退出（exit={proc.returncode}）："
              f"{out.strip() or '(无输出)'}", flush=True)
        raise SystemExit(1)
    if runtime_pid() != proc.pid:
        print(f"[FAIL] 端口 {PORT} 上的服务者不是本次实例（运行态 pid={runtime_pid()}，"
              f"本次 pid={proc.pid}）—— 探测会打到别人身上，先退出已有控制台", flush=True)
        raise SystemExit(1)

    time.sleep(1.5)

    before = visible_windows()
    ch0, wt0 = conhost_count(), wt_count()
    print(f"触发前：可见窗口 {len(before)} 个，conhost={ch0}, WindowsTerminal={wt0}")

    # 控制台窗口只存在到 flightmodel 退出（~0.9s），必须在调用期间**持续采样**。
    # 只认"控制台宿主"类窗口：这个 bug 的签名是 conhost / Windows Terminal
    # （CASCADIA_*）在子进程存活期间闪出；不过滤类名的话，用户这十几秒里随手打开
    # 浏览器/其它程序都会被算成"新窗口"（Chrome_WidgetWin_1 就是这么误报的）。
    import threading
    seen: dict = {}
    stop = threading.Event()

    def sampler():
        while not stop.is_set():
            for h, v in visible_windows().items():
                # 排除本探针自己的窗口（若控制台真的建窗，出现时间可能晚于 before 快照）
                if h not in before and v[2] != proc.pid and (v[0] or "").lower() in CONSOLE_CLASSES:
                    seen[h] = v
            time.sleep(0.02)

    th = threading.Thread(target=sampler, daemon=True)
    th.start()
    try:
        for fuel in (20, 45, 70, 95, 55, 30):
            urllib.request.urlopen(f"{url}/api/fm/a-20g?fuel_pct={fuel}&extra_weight=0", timeout=60).read()
            urllib.request.urlopen(f"{url}/api/fm/a-20g/curves?fuel_pct={fuel}&extra_weight=0", timeout=60).read()
            time.sleep(0.15)
    finally:
        stop.set()
        th.join(timeout=2)

    after = visible_windows()
    after.update(seen)
    ch1, wt1 = conhost_count(), wt_count()
    new = dict(seen)   # 采样期间出现过的所有新窗口（含一闪而过的）
    print(f"触发后：可见窗口 {len(after)} 个（新增 {len(new)}），conhost={ch1}, WindowsTerminal={wt1}")
    for h, (cls, title, pid) in new.items():
        print(f"  新增窗口 hwnd={h} class={cls} title={title!r} pid={pid}")

    verdict = (not new) and ch1 <= ch0 and wt1 <= wt0
    print("\n结论:", "未弹出任何新窗口 ✓" if verdict else "出现新窗口/控制台 ✗")
finally:
    proc.kill()

# 失败必须非 0：闪黑框是已修 bug 的回归守卫，不能只打印结论
raise SystemExit(0 if verdict else 1)
