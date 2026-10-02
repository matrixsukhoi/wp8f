#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""P3 更新面板的端到端探针（Windows 侧，Playwright）。

**前提**：已经有一个控制台实例在 `127.0.0.1:<port>` 上服务。自己起一个（不碰用户的实例）：

    .\\wp8f-gui.exe --port 8799 --no-window      # 只常驻托盘（会多一个托盘图标，测完关掉它）

跑：

    python scripts\\tests\\update_check.py 8799

忽略
* 接口层：`/api/update/status`、`/api/update/check`（真去上游取一次版本号，无副作用）、
  以及"暂存没就绪时 `/api/update/apply` 必须失败"这道闸门；
* 浏览器里：面板挂载、版本号显示、点「检查更新」后面板自己刷新出"有新版本"、
  未就绪时「更新到下载新版本」**禁用**（只有下载完成后才可选）、
  点它弹的是**页内确认框**（不依赖 WebView2 原生对话框）、
  合规说明在场、`[UPDATE]` 日志可展开、**没有 JS 异常**；
* 「停止 HUD」这个动作不存在（没有按钮、接口返回未知动作），
  取而代之的是「清除下载」（取消要能清掉 `data_new` 与 `.wp8f_update_tmp`）；
* 截图落在 `logs/_probe/update_panel_<port>.png`（logs/ 不入库）。

**绝不点「开始下载」**：那会真的去下 361.6 MB（规格红线：验证时只允许 1–2 个文件或本地假源；
真实全量下载留给用户点按钮时执行）。**也绝不在有暂存数据时点「清除下载」**（那是用户下的东西）。

退出码：0 全过；1 有断言失败。
"""
import json
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8799
URL = f"http://127.0.0.1:{PORT}"

# Windows 控制台默认是 GBK：不打这行，下面的 ✓/✗ 会直接 UnicodeEncodeError 把探针炸掉
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass


def api(path, data=None):
    req = urllib.request.Request(URL + path, method="POST" if data is not None else "GET")
    if data is not None:
        req.add_header("Content-Type", "application/json")
        req.data = json.dumps(data).encode()
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.loads(r.read().decode())


def main() -> int:
    fails = []

    def check(name, ok, detail=""):
        print(f"  {'✓' if ok else '✗'} {name}{'' if ok else '  → ' + str(detail)}")
        if not ok:
            fails.append(name)

    print(f"=== 接口层（{URL}）===")
    try:
        st = api("/api/update/status")["status"]
    except Exception as e:  # 控制台没起 / 端口不对
        print(f"  ✗ 拿不到 /api/update/status：{e}")
        print("    先起一个控制台：wp8f-gui.exe --port 8799 --no-window")
        return 1
    check("GET /api/update/status 可用", isinstance(st, dict) and "phase" in st, st)
    check("本地版本已读出", bool(st.get("local_version")), st.get("local_version"))
    check("暂存未就绪 → staging_ready=false", st.get("staging_ready") is False, st.get("staging_ready"))
    check("json_supported=true（构建带 fm-json）", st.get("json_supported") is True, st.get("json_supported"))
    check("源顺序 = github→api（P9：第三方 CDN 已删，只剩 GitHub 自己）",
          [s["id"] for s in st.get("sources", [])] == ["github", "api"], st.get("sources"))
    check("hud_running 字段在场（切换前要它别占目录）", "hud_running" in st)

    r = api("/api/update/check", {})
    check("POST /api/update/check ok", r.get("ok") is True, r.get("error"))
    st = r.get("status", {})
    check("远端版本非空且可比较", bool(st.get("remote_version")), st.get("remote_version"))
    if st.get("remote_version") and st.get("local_version"):
        remote_newer = st.get("update_available") in (True, False)
        check("update_available 与版本号一致", remote_newer,
              (st.get("local_version"), st.get("remote_version"), st.get("update_available")))

    r = api("/api/update/apply", {})
    check("未就绪时 /apply 必须失败（「先下载完成」这道闸门）", r.get("ok") is False, r.get("ok"))
    check("失败原因分类明确", (r.get("error") or {}).get("kind") in ("verify", "io", "hud_running"),
          r.get("error"))

    # P6：『停止 HUD』这条动作整个删了 —— 不能再有"更新器替用户杀 HUD"的路径
    r = api("/api/update/stop-hud", {})
    check("『停止 HUD』接口已删除（未知动作，且不返回 killed）",
          r.get("ok") is False and "killed" not in r, str(r)[:160])
    # 『清除下载』：没暂存数据时是幂等的 no-op；**有**暂存数据时探针不碰（那是用户下的东西）
    if st.get("staging_version") is None and not (st.get("progress") or {}).get("files_done"):
        r = api("/api/update/discard", {})
        check("『清除下载』可用且幂等（没有下载目录时也是 ok）", r.get("ok") is True, str(r)[:160])
    else:
        check("『清除下载』跳过：本机有未清除的下载目录，探针不动用户数据", True,
              f"staging={st.get('staging_version')}")

    print("=== 浏览器（Playwright）===")
    from playwright.sync_api import sync_playwright
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(viewport={"width": 1280, "height": 800})
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.on("console", lambda m: errors.append(f"console.{m.type}: {m.text}") if m.type == "error" else None)
        page.goto(URL, wait_until="load")
        page.click('#tabs .tab[data-tab="fm"]')
        page.wait_for_selector("#fm-update", timeout=15000)
        check("面板已挂载进飞行模型页签", page.is_visible("#fm-update"))
        title = page.inner_text("#fm-update .up-title")
        check("标题走 i18n（默认中文）", title == "飞行模型数据库更新", title)
        check("合规说明在场（仅本地自用、不随包分发）",
              "不随包分发" in page.inner_text("#up-legal"), page.inner_text("#up-legal"))
        check("未就绪时『更新到下载新版本』按钮禁用（只有下载完成后才可选）",
              page.is_disabled("#up-apply"))
        check("『停止 HUD』按钮已删除", page.query_selector("#up-stop") is None)
        check("『清除下载』按钮在场", page.query_selector("#up-clear") is not None)
        check("『开始下载』按钮可用", not page.is_disabled("#up-start"))

        page.click("#up-check")
        page.wait_for_function(
            "() => document.querySelector('#up-latest').textContent.trim() !== '未知'", timeout=30000)
        latest = page.inner_text("#up-latest")
        note = page.inner_text("#up-note")
        check("点『检查更新』后面板显示远端版本", bool(latest) and latest != "未知", latest)
        check("面板给出结论（有新版本 / 已是最新）",
              ("有新版本" in note) or ("已是最新" in note), note)

        # 二次确认闸门：按钮此刻是禁用的（暂存未就绪）→ 临时解锁只为验证"确认框"这一层
        page.evaluate("() => { document.getElementById('up-apply').disabled = false; }")
        page.click("#up-apply")
        check("点『更新到下载新版本』弹**页内**确认框（不依赖原生对话框）", page.is_visible("#up-confirm"))
        text = page.inner_text("#up-confirm-text") if page.is_visible("#up-confirm") else ""
        check("确认文案讲清代价（只保留上一版 data_old、可直接删；HUD 要重启）",
              ("data_old" in text) and ("重启" in text) and ("删" in text), text)
        page.click("#up-confirm-no")
        check("『再想想』收起确认框", not page.is_visible("#up-confirm"))

        page.click("#up-log-toggle")
        check("日志区可展开", page.is_visible("#up-log"))
        log = page.inner_text("#up-log")
        check("[UPDATE] 日志里有检查更新的记录", "检查更新" in log, log[:120])

        shot = ROOT / "logs" / "_probe" / f"update_panel_{PORT}.png"
        shot.parent.mkdir(parents=True, exist_ok=True)
        page.screenshot(path=str(shot))
        print(f"  截图：{shot}")
        check("没有 JS 异常", not errors, errors[:3])
        browser.close()

    print()
    if fails:
        print(f"失败 {len(fails)} 项：{fails}")
        return 1
    print("全部通过")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
