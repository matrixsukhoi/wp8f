#!/usr/bin/env python3
"""控制台 DOM 自检：用 Playwright 打开窗口页面，dump DOM 事实与 JS 异常。

控制台现在是单进程 Rust（wp8f-gui.exe）：本脚本用 --no-window 起一个只有后端的实例，
再用 Edge 无头打开页面 —— 不需要 Python 侧的任何服务代码。

自己拉实例时带 `--no-tray-promote`：探针不该改用户的
`HKCU\\Control Panel\\NotifyIconSettings\\IsPromoted`。控制台的单实例判据是**端口独占**：
已有实例占着端口时，新实例会唤醒它的窗口后自己退出（退出码 0），此时明确报错退出码 2，
而不是把探测打到用户那个实例上；端口被别的程序占住（新实例根本没起来）同样报 2 但文案不同。

截图与实例日志都落在 logs/_probe/ooda/（logs/ 是用户可见目录），
全部通过时整目录删除，有失败/跳过才保留并在末尾打印路径。

用法：
    python scripts/tests/ooda.py                       # 自行拉起 wp8f-gui.exe --no-window（推荐）
    python scripts/tests/ooda.py --url http://127.0.0.1:8765   # 打到已在跑的实例
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

# 输出重定向到文件时 Windows 默认 GBK，打印 ✓/✕ 会 UnicodeEncodeError 直接崩；
# 统一强制 UTF-8，让 CI/重定向场景与真实控制台行为一致（本脚本 stderr 也打印中文）。
sys.stdout.reconfigure(encoding="utf-8", errors="replace")
sys.stderr.reconfigure(encoding="utf-8", errors="replace")


def _find_root() -> Path:
    """仓库根：从本文件向上找 Cargo.toml（脚本位于 scripts/tests/ 下）"""
    here = Path(__file__).resolve()
    for probe in [here.parent, *here.parents]:
        if (probe / "Cargo.toml").is_file() and (probe / "gui").is_dir():
            return probe
    return here.parents[2]


ROOT = _find_root()
PROBE_DIR = ROOT / "logs" / "_probe" / "ooda"
OUT = str(PROBE_DIR)
PORT = 8799
EXES = [ROOT / "wp8f-gui.exe",
        ROOT / "target" / "x86_64-pc-windows-gnu" / "release" / "wp8f-gui.exe"]


def _gui_pids() -> list[int]:
    """本机已在跑的 wp8f-gui.exe（探针与用户控制台共用 logs/，必须先清场 —— 见调用处）"""
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


def _finish(code: int) -> int:
    """收尾：全绿（0）时删掉 logs/_probe/ooda/，否则保留并打印路径（便于排查）。"""
    if code == 0:
        shutil.rmtree(PROBE_DIR, ignore_errors=True)
        try:
            PROBE_DIR.parent.rmdir()          # logs/_probe 空了也一并收掉
        except OSError:
            pass
    elif PROBE_DIR.is_dir():
        print(f"\n探针产物（截图/日志）保留在 {PROBE_DIR.relative_to(ROOT)}（排查完可整个删掉）")
    return code


def dump(page, label):
    print(f"\n===== {label} =====")
    facts = page.evaluate("""() => ({
      cfgOptions: document.querySelectorAll('#config-select option').length,
      cfgOptionTexts: Array.from(document.querySelectorAll('#config-select option')).slice(0,3).map(o => o.textContent),
      cfgMsg: (document.querySelector('#config-msg') || {}).textContent,
      switchRow: !!document.querySelector('.item.switch'),
      notifyBox: !!document.querySelector('#notify'),
      fmOptions: document.querySelectorAll('#fm-select option').length,
      appSrc: (document.querySelector('script[src*=app]') || {}).src || '<none>',
      bodyLen: document.body.innerText.length,
    })""")
    for k, v in facts.items():
        print(f"  {k}: {v}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="")
    args = ap.parse_args()

    proc = None
    url = args.url
    if not url:
        exe = next((e for e in EXES if e.is_file()), None)
        if exe is None:
            print("找不到 wp8f-gui.exe（先构建：bash scripts/build.sh --webui-only）", file=sys.stderr)
            return _finish(1)
        # 前置：已有控制台在跑就先退出。单实例判据 = 端口独占，
        # 用户那个 8765 的控制台不会"顶掉"本次实例（本脚本用 PORT 专用端口），但两者**共用
        # 同一份 logs/console-host.json 与控制台日志** —— 跑下去会把用户那份顶掉。
        other = _gui_pids()
        if other:
            print(f"[SKIP] 已有控制台在跑（pid={other}）—— 本次探针会与它共用 "
                  f"logs/console-host.json 与控制台日志，先退出它再跑本脚本", file=sys.stderr)
            return _finish(2)
        PROBE_DIR.mkdir(parents=True, exist_ok=True)
        # "w+"：实例的输出写在这（无管道，实例活着也不会写满），失败时要读回来判断原因
        log = open(PROBE_DIR / "ooda_window.log", "w+", encoding="utf-8")
        # --no-tray-promote：探针不碰 HKCU\Control Panel\NotifyIconSettings\IsPromoted
        proc = subprocess.Popen([str(exe), "--root", str(ROOT), "--port", str(PORT),
                                 "--no-window", "--no-tray-promote"],
                                cwd=str(ROOT), stdout=log, stderr=subprocess.STDOUT)
        url = f"http://127.0.0.1:{PORT}"
        import urllib.request
        ready = False
        for _ in range(80):
            if proc.poll() is not None:
                break
            try:
                urllib.request.urlopen(url + "/api/health", timeout=0.3)
                ready = True
                break
            except Exception:
                time.sleep(0.1)

        def runtime_pid() -> int:
            """运行态文件里的 pid（读不到 = -1）：端口上的服务者是不是本次 spawn 的实例"""
            try:
                return int(json.loads((ROOT / "logs" / "console-host.json")
                                      .read_text(encoding="utf-8"))["pid"])
            except Exception:  # noqa: BLE001
                return -1

        # 单实例判据 = 端口独占（没有全机命名互斥体）。但"端口上有服务"
        # 不等于"端口上的服务就是本次 spawn 的实例"：已有控制台会秒回 /api/health，而本次
        # 实例还要走完「唤出它的窗口 → 打印提示 → 退出」才消失 —— 只 poll() 一次会输给这个
        # 竞态，探测就打到**用户那个实例**上（假绿 + 可能改动用户状态）。所以按运行态对账。
        if ready and proc.poll() is None and runtime_pid() != proc.pid:
            try:
                proc.wait(timeout=5)      # 被拒时它会在这段时间里退出
            except subprocess.TimeoutExpired:
                pass
        if proc.poll() is not None:
            log.flush()
            log.seek(0)
            out = log.read()
            # 针脚是**英文固定串**：控制台 stdout 的那三条诊断不随 i18n 界面语言变
            # （gui/src/main.rs 的 port_busy_exit）；"已有控制台"这条同时要求退出码 0。
            if proc.returncode == 0 and "already running" in out:
                print(f"[SKIP] 端口 {PORT} 上已有控制台在运行（本次实例唤出它的窗口后退出）；"
                      f"请先退出控制台再跑本脚本", file=sys.stderr)
                return _finish(2)
            if "occupied by another program" in out or "could not be woken up" in out:
                print(f"[SKIP] 端口 {PORT} 不可用，控制台实例没起来：{out.strip()}",
                      file=sys.stderr)
                return _finish(2)
            print(f"[FAIL] 本次实例启动即退出（exit={proc.returncode}）："
                  f"{out.strip() or '(无输出)'}", file=sys.stderr)
            return _finish(1)
        if not ready:
            print("后端未就绪", file=sys.stderr)
            proc.kill()
            return _finish(1)
        if runtime_pid() != proc.pid:
            print(f"[FAIL] 端口 {PORT} 上的服务者不是本次实例（运行态 pid={runtime_pid()}，"
                  f"本次 pid={proc.pid}）—— 探测会打到别人身上，先退出已有控制台",
                  file=sys.stderr)
            return _finish(1)
    print(f"目标：{url}")

    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        print("SKIP: 未安装 playwright（pip install playwright）")
        if proc:
            proc.kill()
        return _finish(3)

    os.makedirs(OUT, exist_ok=True)
    code = 0
    pw = None
    try:
        try:
            pw = sync_playwright().start()
        except Exception as e:  # noqa: BLE001
            print(f"SKIP: Playwright 驱动无法启动（{e}）—— 重装试试：pip install -U playwright")
            return _finish(3)
        try:
            browser = pw.chromium.launch(channel="msedge", args=["--no-sandbox"])
        except Exception:
            try:
                browser = pw.chromium.launch(args=["--no-sandbox"])
            except Exception as e:  # noqa: BLE001
                print(f"SKIP: 浏览器无法启动（{e}）")
                return _finish(3)
        # 视口与真实窗口一致（16:9 = 1365x768）
        page = browser.new_page(viewport={"width": 1365, "height": 768})
        errors = []
        page.on("pageerror", lambda e: errors.append(f"PAGEERROR: {e}"))
        page.on("console", lambda m: errors.append(f"CONSOLE-{m.type}: {m.text}")
                if m.type in ("error", "warning") else None)
        page.goto(url, wait_until="networkidle")
        time.sleep(1.5)
        print("===== JS 异常 =====")
        for e in errors[:12]:
            print(" ", e)
        dump(page, "配置页签")
        page.screenshot(path=os.path.join(OUT, "ooda_config.png"))
        for tab in ("fm", "replay"):
            page.click(f'#tabs .tab[data-tab="{tab}"]')
            # FM 曲线要先跑 flightmodel 计算（1~3s），等久一点才能看到真实渲染
            time.sleep(5.0 if tab == "fm" else 2.5)
            dump(page, f"{tab} 页签")
            page.screenshot(path=os.path.join(OUT, f"ooda_{tab}.png"))
        code = 1 if any("PAGEERROR" in e for e in errors) else 0
        browser.close()
    finally:
        if pw is not None:
            try:
                pw.stop()
            except Exception:  # noqa: BLE001
                pass
        if proc:
            proc.kill()
    if code == 0:
        print("\n截图：无（全部通过，logs/_probe/ooda/ 已清理）")
    return _finish(code)


if __name__ == "__main__":
    raise SystemExit(main())
