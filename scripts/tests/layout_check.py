#!/usr/bin/env python3
"""布局自检：卡片/内容区是否溢出视口、两列是否对齐（用真实渲染度量，不靠肉眼）。

截图落在 logs/_probe/layout_check/（logs/ 是用户可见目录），全部通过时整目录删除，
有失败才保留并在末尾打印路径。
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
PROBE_DIR = ROOT / "logs" / "_probe" / "layout_check"

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

# 前置：本探针要断言"FM 曲线真的渲染出来"，曲线来自 binary/flightmodel.exe（S1 布局）。
# 缺了它下面 6 张图必然是空的 —— 那是环境不满足（退出码 2），不是布局断言失败。
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
        page.goto(url, wait_until="networkidle")
        # 等配置卡片真正渲染完（否则会量到"只有静态卡片"的半成品页面，断言会变得没有意义）
        page.wait_for_selector("#config-form .card", timeout=20000)
        page.wait_for_function(
            "() => document.querySelectorAll('.cfg-grid > .card, #config-form .card').length >= 6",
            timeout=20000)
        time.sleep(0.8)
        facts = page.evaluate("""() => {
          const vw = document.documentElement.clientWidth;
          const content = document.querySelector('.content');
          const cards = Array.from(document.querySelectorAll('.cfg-grid > .card, #config-form .card'));
          // 卡片内溢出：任何子元素超出所属卡片的内容盒
          const innerOverflow = [];
          for (const c of cards) {
            const cr = c.getBoundingClientRect();
            for (const el of c.querySelectorAll('*')) {
              const r = el.getBoundingClientRect();
              if (r.width === 0) continue;
              if (r.right > cr.right - 1 || r.left < cr.left - 1) {
                innerOverflow.push(((c.querySelector('h3')||{}).textContent||'?').trim() + ':' + el.tagName.toLowerCase());
              }
            }
          }
          // 同一排的卡片高度应一致（等高卡片，无锯齿状空洞）
          const byRow = {};
          for (const c of cards) {
            const t = Math.round(c.getBoundingClientRect().top);
            (byRow[t] = byRow[t] || []).push(
              ((c.querySelector('h3')||{}).textContent||'?').trim() + '=' + Math.round(c.getBoundingClientRect().height));
          }
          return {
            viewport: vw,
            contentClient: content.clientWidth,
            contentScroll: content.scrollWidth,
            hOverflow: content.scrollWidth - content.clientWidth,
            bodyScroll: document.body.scrollWidth,
            contentScrollH: content.scrollHeight,
            contentClientH: content.clientHeight,
            vScroll: content.scrollHeight - content.clientHeight,
            lastCardBottom: Math.round(Math.max(...cards.map(c => c.getBoundingClientRect().bottom))),
            barTop: Math.round(document.querySelector('.actionbar').getBoundingClientRect().top) || content.clientHeight,
            rowPairs: Object.values(byRow).map(r => r.join('  vs  ')),
            innerOverflow: innerOverflow.slice(0, 8),
            cards: cards.map(c => {
              const r = c.getBoundingClientRect();
              return { title: (c.querySelector('h3')||{}).textContent, left: Math.round(r.left),
                       right: Math.round(r.right), w: Math.round(r.width),
                       overflow: Math.round(r.right - content.getBoundingClientRect().right) };
            }),
            // 只看真正的滑杆行（色块行也带 .sld 但没有 range，直接取会抛 TypeError）
            sliders: Array.from(document.querySelectorAll('#config-form .sld'))
              .filter(s => s.querySelector('input[type=range]')).slice(0,3).map(s => {
              const r = s.getBoundingClientRect();
              const inp = s.querySelector('input[type=range]').getBoundingClientRect();
              return { sldRight: Math.round(r.right), inputW: Math.round(inp.width) };
            }),
          };
        }""")
        print(f"视口 {facts['viewport']}  内容区 client={facts['contentClient']} scroll={facts['contentScroll']}")
        print(f"横向溢出: {facts['hOverflow']}px   body.scrollWidth={facts['bodyScroll']}")
        print("卡片（left/right/width/超出内容区右边）:")
        for c in facts["cards"]:
            print(f"  {c['title']:<12} {c['left']:>5} {c['right']:>5} w={c['w']:>4} overflow={c['overflow']:>4}")
        print("滑杆前 3 行:", facts["sliders"])
        print("每排卡片（应等高）:")
        for row in facts["rowPairs"]:
            print("  ", row)
        print(f"纵向: 内容 {facts['contentScrollH']} / 可视 {facts['contentClientH']}"
              f"  需滚动={facts['vScroll'] > 1}  末卡底部={facts['lastCardBottom']}  操作条顶={facts['barTop']}")
        print("卡片内溢出:", facts["innerOverflow"] if facts["innerOverflow"] else "无")

        # 断言（1365x768 标准窗口下）：配置器一屏放下 + 卡片内不溢出 + 同排等高
        results = []
        def check(name, ok, detail=""):
            results.append(ok)
            print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" · {detail}" if detail else ""), flush=True)

        check("配置器无横向溢出", facts["hOverflow"] <= 0, f"{facts['hOverflow']}px")
        check("卡片内容不溢出卡片", not facts["innerOverflow"], str(facts["innerOverflow"]))
        check("一屏放下（无需滚动）", facts["vScroll"] <= 1,
              f"内容 {facts['contentScrollH']} vs 可视 {facts['contentClientH']}")
        check("末卡不被底部操作条遮挡", facts["lastCardBottom"] <= facts["barTop"] - 4,
              f"末卡 {facts['lastCardBottom']} / 操作条 {facts['barTop']}")
        for row in facts["rowPairs"]:
            hs = [int(x.split("=")[1]) for x in row.split("  vs  ")]
            check(f"同排等高: {row}", len(set(hs)) == 1)

        # FM 页签：等曲线算完，再用 DOM 事实确认真的画了（不靠肉眼）
        page.click('#tabs .tab[data-tab="fm"]')
        time.sleep(6)
        fm = page.evaluate("""() => {
          const out = {};
          for (const id of ['fm-chart-cl','fm-chart-power','fm-chart-tasalt','fm-chart-3d']) {
            const el = document.getElementById(id);
            const canv = el ? el.querySelector('canvas') : null;
            out[id] = { canvas: !!canv, w: canv ? canv.width : 0, placeholder: el ? !!el.querySelector('.empty') : false,
                        // P9：活塞机型把「推力 3D 曲面」等三张图**连容器一起隐藏**（data-fm-jet-only）
                        hidden: el ? (el.getClientRects().length === 0 || getComputedStyle(el).display === 'none') : true };
          }
          out.infoCards = document.querySelectorAll('#fm-info .fm-card').length;
          return out;
        }""")
        print("FM 渲染事实:", fm)
        # 用 DOM 事实断言真的画了（只 print 等于没检查）：
        # FM 白屏（图表全空）一样能绿。现在逐图断言并计入退出码。
        # 只查这 4 张：另外 2 张（推力-真空速 / 推力-表速）在 tab_check.py 里一起查，
        # 图数不是固定的 6 张：喷气 4 张 / 活塞 3 张。
        #
        # ⚠️ 第 4 张（3D 曲面）是**喷气机型专属**（活塞机型只保留三张图，
        # 连 `.sec` 标题一起隐藏，见 `index.html` 的 `data-fm-jet-only`）—— 默认机型是
        # 活塞（a-20g）时它**本来就不该有尺寸**，硬要求 `w > 0` 是自相矛盾的旧断言
        #（P9 之后这条一直红）。两种机型的完整可见性覆盖在 `ui_checks` 第 15 组
        #（活塞：6 个 jet-only 元素全隐藏；切回喷气：全部恢复）——
        # 这里只保证"**可见时**必须真画出来"，白屏照样红。
        for cid in ('fm-chart-cl', 'fm-chart-power', 'fm-chart-tasalt'):
            f = fm[cid]
            check(f"{cid} 真的画出了 canvas（尺寸 > 0、无占位符）",
                  f["canvas"] and f["w"] > 0 and not f["placeholder"], str(f))
        f3d = fm['fm-chart-3d']
        if f3d["hidden"]:
            check("fm-chart-3d 按 P9 口径隐藏（当前默认机型是活塞）—— 可见性由 ui_checks 第 15 组覆盖",
                  not f3d["placeholder"], str(f3d))
        else:
            check("fm-chart-3d 真的画出了 canvas（尺寸 > 0、无占位符）",
                  f3d["canvas"] and f3d["w"] > 0 and not f3d["placeholder"], str(f3d))
        check("FM 信息卡已渲染", fm["infoCards"] > 0, f"{fm['infoCards']} 张")
        PROBE_DIR.mkdir(parents=True, exist_ok=True)
        shot = PROBE_DIR / f"shot_fm_{int(time.time())}.png"
        page.screenshot(path=str(shot))
        print("新截图:", shot.relative_to(ROOT))
        print(f"\n===== {sum(results)}/{len(results)} 通过 =====")
        browser.close()
    finally:
        pw.stop()
finally:
    proc.kill()

# 失败必须非 0（只 print 会让 exit code 恒为 0）
all_ok = bool(results) and all(results)
if all_ok:
    shutil.rmtree(PROBE_DIR, ignore_errors=True)      # 全绿：截图不留
    try:
        PROBE_DIR.parent.rmdir()                      # logs/_probe 空了也一并收掉
    except OSError:
        pass
elif PROBE_DIR.is_dir():
    print(f"\n有失败项，截图保留在 {PROBE_DIR.relative_to(ROOT)}（排查完可整个删掉）")
raise SystemExit(0 if all_ok else 1)
