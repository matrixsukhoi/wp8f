#!/usr/bin/env python3
"""页签切换自检：点击后哪个 panel 处于 active、FM 是否真的渲染、有无 JS 异常。

诊断信息照旧全部打印，但下面这些是**影响退出码的断言**（以前只打印，退出码只看
panels/pageerror，于是"控制台一堆 JS 异常、FM 图一张没画"照样绿）：
  * 6 张 FM 图都挂上了非空 canvas、页签里没有残留占位符
  * 无 CONSOLE-error（页面 JS 异常）、无 REQFAIL / HTTP>=400（favicon 除外）
  * 无 PAGEERROR、切换后只有 tab-fm 处于 active
截图落在 logs/_probe/tab_check/，全部通过时整目录删除，有失败才保留。
"""
import json
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

# 输出重定向到文件时 Windows 默认 GBK，打印 ✓/✕ 会 UnicodeEncodeError 直接崩；
# 统一强制 UTF-8，让 CI/重定向场景与真实控制台行为一致。
sys.stdout.reconfigure(encoding="utf-8", errors="replace")

ROOT = Path(__file__).resolve().parents[2]
PORT = 8799
EXE = ROOT / "wp8f-gui.exe"
# S1 布局：仓库根只放控制台；FM 解析可执行文件在 binary/ 下（gui/src/env.rs 从那里找它）
FM_EXE = ROOT / "binary" / "flightmodel.exe"
PROBE_DIR = ROOT / "logs" / "_probe" / "tab_check"
RESULTS: list[tuple[str, bool, str]] = []


def check(name: str, ok: bool, detail: str = "") -> bool:
    RESULTS.append((name, bool(ok), str(detail)))
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" · {detail}" if detail else ""), flush=True)
    return bool(ok)


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

# 前置：本探针要断言"FM 页签 6 张图都出非空 canvas"，曲线来自 binary/flightmodel.exe
# （S1 布局）。缺了它那几条必然失败 —— 那属于环境不满足（退出码 2），不是界面回归。
if not FM_EXE.is_file():
    print(f"[SKIP] 找不到 {FM_EXE}（先跑 bash scripts/build.sh）", flush=True)
    raise SystemExit(2)

proc = subprocess.Popen([str(EXE), "--root", str(ROOT), "--no-window", "--no-tray-promote",
                         "--port", str(PORT)],
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

    from playwright.sync_api import sync_playwright
    pw = sync_playwright().start()
    try:
        browser = pw.chromium.launch(channel="msedge", args=["--no-sandbox"])
        page = browser.new_page(viewport={"width": 1365, "height": 768})
        errs = []
        page.on("pageerror", lambda e: errs.append(f"PAGEERROR: {e}"))
        page.on("console", lambda m: errs.append(f"CONSOLE-{m.type}: {m.text}")
                if m.type == "error" else None)
        page.on("requestfailed", lambda r: errs.append(f"REQFAIL: {r.url} {r.failure}"))
        page.on("response", lambda r: errs.append(f"HTTP{r.status}: {r.url}")
                if r.status >= 400 and "favicon" not in r.url else None)
        page.on("console", lambda m: errs.append(f"CONSOLE-{m.type}: {m.text}"))
        page.goto(url, wait_until="networkidle")
        page.wait_for_selector("#config-form .card", timeout=15000)
        print("配置页卡片数:", page.eval_on_selector_all("#config-form .card", "els => els.length"))

        page.click('#tabs .tab[data-tab="fm"]')
        # EM 能量机动图**只喷气机有**（活塞没有 EM 数据，整块连标题一起隐藏）：
        # 先切一个喷气机型，否则那张图本来就不该出现。
        try:
            page.wait_for_function(
                "() => document.querySelectorAll('#fm-aircraft option').length > 0",
                timeout=15000)
            picked = page.evaluate("""() => {
                const sel = document.getElementById("fm-aircraft");
                if (!sel) return "";
                const o = Array.from(sel.options).find(
                    (x) => /f_16c_block_50|fa_18a|f-16c|mig_29/i.test(x.value));
                if (!o) return "";
                sel.value = o.value;
                sel.dispatchEvent(new Event("change", { bubbles: true }));
                return o.value;
            }""")
            print("FM 机型:", picked or "(没找到喷气机型，保持默认)")
        except Exception as e:  # noqa: BLE001
            print("等机型下拉超时:", type(e).__name__)
        # FM 曲线要先跑 flightmodel 计算（1~3s）：先有界地等 7 张图都出非空 canvas，
        # 超时也继续（下面的断言会把"空的图"列出来并判失败），避免单纯 sleep 造成的抖动。
        # `fm-chart-em` 是喷气专属（`data-fm-jet-only`），隐藏时不参与判定。
        try:
            page.wait_for_function("""() => {
                const ids = ["fm-chart-cl", "fm-chart-power", "fm-chart-3d",
                             "fm-chart-tasalt", "fm-chart-thrust-tas", "fm-chart-thrust-ias",
                             "fm-chart-em"];
                return ids.every(id => {
                    const el = document.getElementById(id);
                    if (!el || el.style.display === "none") return true;
                    const c = el.querySelector('canvas');
                    return !!c && c.width > 0 && c.height > 0;
                });
            }""", timeout=20000)
        except Exception as e:  # noqa: BLE001
            print("等待 FM 图表就绪超时（下面按实际渲染结果判定）:", type(e).__name__)
        time.sleep(0.5)
        panels = page.evaluate(
            "() => Array.from(document.querySelectorAll('.panel.active')).map(p => p.id)")
        print("active 面板:", panels)
        print("active 页签:", page.evaluate(
            "() => (document.querySelector('#tabs .tab.active')||{}).textContent"))
        fm = page.evaluate("""() => {
            // FM 页签：6 张通用图 + EM（喷气专属）。隐藏的图（活塞）不参与判定。
            const ids = ["fm-chart-cl", "fm-chart-power", "fm-chart-3d",
                         "fm-chart-tasalt", "fm-chart-thrust-tas", "fm-chart-thrust-ias",
                         "fm-chart-em"];
            const visible = ids.filter(id => {
                const el = document.getElementById(id);
                return el && el.style.display !== "none";
            });
            const emptyCharts = visible.filter(id => {
                const el = document.getElementById(id);
                const c = el ? el.querySelector('canvas') : null;
                return !c || c.width <= 0 || c.height <= 0;
            });
            return {
                infoCards: document.querySelectorAll('#fm-info .fm-card').length,
                aircraftOptions: document.querySelectorAll('#fm-aircraft option').length,
                canvases: document.querySelectorAll('#tab-fm canvas').length,
                placeholders: document.querySelectorAll('#tab-fm .empty').length,
                visibleCharts: visible.length,
                emShown: visible.includes("fm-chart-em"),
                emptyCharts,
                bodyLen: document.body.innerText.length,
            };
        }""")
        print("FM 事实:", fm)
        PROBE_DIR.mkdir(parents=True, exist_ok=True)
        fresh = PROBE_DIR / f"shot_fm_check_{int(time.time())}.png"
        page.screenshot(path=str(fresh))
        print("截图:", fresh.relative_to(ROOT))
        print("#fm-info 内容:", page.eval_on_selector("#fm-info", "el => el.innerHTML.slice(0,200)"))
        print("fm 相关事件:", [e for e in errs if "fm" in e.lower() or "REQFAIL" in e or "HTTP" in e][:8])
        print("全部控制台:", [e for e in errs if e.startswith("CONSOLE")][:8])

        # ---- 影响退出码的断言 ----
        console_errors = [e for e in errs if e.startswith("CONSOLE-error")]
        page_errors = [e for e in errs if e.startswith("PAGEERROR")]
        # favicon 的 404/请求失败已在上面按原有约定排除（浏览器会自动去要它）
        req_fails = [e for e in errs if e.startswith("REQFAIL") and "favicon" not in e]
        http_errs = [e for e in errs if e.startswith("HTTP")]
        check("切换到 FM 页签后只有 tab-fm 处于 active", panels == ["tab-fm"], str(panels))
        check("FM 图全部画出非空 canvas（6 张通用图 + 喷气专属的 EM）",
              not fm["emptyCharts"] and fm["canvases"] >= 6 and fm["emShown"],
              f"canvas={fm['canvases']} 可见图={fm['visibleCharts']} EM显示={fm['emShown']} 空的={fm['emptyCharts']}")
        check("FM 页签无占位符（图表不是空的）", fm["placeholders"] == 0, str(fm["placeholders"]))
        check("无页面 JS 异常（CONSOLE-error）", not console_errors, str(console_errors[:2]))
        check("无未处理异常（PAGEERROR）", not page_errors, str(page_errors[:2]))
        check("无请求失败 / HTTP>=400", not req_fails and not http_errs,
              f"REQFAIL={req_fails[:2]} HTTP={http_errs[:2]}")
        browser.close()
    finally:
        pw.stop()
finally:
    proc.kill()

passed = sum(1 for _n, ok, _d in RESULTS if ok)
all_ok = bool(RESULTS) and passed == len(RESULTS)
if all_ok:
    shutil.rmtree(PROBE_DIR, ignore_errors=True)      # 全绿：截图不留
    try:
        PROBE_DIR.parent.rmdir()                      # logs/_probe 空了也一并收掉
    except OSError:
        pass
elif PROBE_DIR.is_dir():
    print(f"\n有失败项，截图保留在 {PROBE_DIR.relative_to(ROOT)}（排查完可整个删掉）")
print(f"\n===== {passed}/{len(RESULTS)} 通过 =====")
raise SystemExit(0 if all_ok else 1)
