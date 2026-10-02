#!/usr/bin/env python3
"""界面自检（真实渲染 + 真实 HTTP，不靠肉眼）。分组：
   1) 导航含「飞行模型 / 回放 / 导弹模拟」
   2) FM：燃油比例与额外重量在同一行
   3) 底部「预览 / 开始」靠右下角
   4) 回放：加入多个文件 → 同坐标系叠加 → 单个移除 / 清空
   5) 上传接口可用
   6) 配置器：布局/字体拆卡、字号 ≤72、尺寸 ≤1440、滑块右侧数字可手输并夹紧、
      色块在输入框左侧且按 `#RRGGBBAA` 语义换算、颜色键 7 个
   6b) 联动：HUD 类型互斥、记录频率上限 = 刷新率、地图记录间隔上限 = 1 秒的数据帧数
   7) 回放 km 尺度：轴名 / 默认 50×50 km / 盒宽深比 / 视野缩放 / 全程自适应
   8) 速度矢量箭头（杆 + 两组倒刺）与飞机姿态线框
   9) 导弹模拟页签（离线 HTML）真的渲染
  10) 拖油量不清空「加入对比」；非配置器页签的提示以 toast 可见
  10b) xoy 底面只有原图纹理一种画法；记录没带底图 → 不铺底面（只留 z=0 网格线）
  11) 轨迹 = 原始采样点（无平滑控件/插值核/状态）
  12) 用户可见文案里不许出现 `--` 参数、`/api/` 路由、下划线字段名与"自动保存教程"这类元说明
  13) i18n：十五种语言键集与 en 一致、未知语言 404、切语言后界面文本确实变、无键名残留
  14) 字体：`/api/fonts` 只列目录里真实存在的字体（无 builtin/default 字段、无合成条目）、
      选项值 = 完整相对路径、非等宽给警告、`font_path` 留空或指向不存在的文件 → HUD 报错退出
  15) FM 页签文案随语言变（en 下无中日韩字符、无 `fm.*` 残留）
  16) 更新器：没有「停止 HUD」、下载未完成按钮禁用、HUD 在跑禁用并说明原因、未就绪时拒绝更新
  17) 回放页签动态状态随语言变（en 下无中日韩字符、无 `rp.*` 残留）
  18) 配置器记住上次选中的配置文件；那份配置没了就回退并清键
  19) HUD 语言与语音包：`/api/hud-lang` 列 `resource/lang/*.json`、`/api/voice-packs` 列
      `resource/voice/` 的子目录（根上不许散落 wav）、「面板」的语言下拉值 = 完整路径而显示
      只有文件名主干、写回 `hud_lang_path`、「告警」的语音包下拉写回 `voice_path`、
      改 HUD 语言不影响 GUI 语言

用例会改动 config/default.json（按字节还原）、临时往 resource/fonts/ 放探针字体（结束时删）、
自带 `.wpr` 夹具（带底图 / 不带底图各一份）、往 logs/ 上传样本并写截图（全绿时整目录删除）。
"""
import json
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib
from pathlib import Path

# 输出重定向到文件时 Windows 默认 GBK，打印 ✓/✕ 会 UnicodeEncodeError 直接崩；
# 统一强制 UTF-8，让 CI/重定向场景与真实控制台行为一致。
sys.stdout.reconfigure(encoding="utf-8", errors="replace")

sys.path.insert(0, str(Path(__file__).resolve().parent))
# CSV 表头/列数从 wpr_replay.py 借（同一份"契约副本"，两边不必各抄一遍）
from wpr_replay import CSV_HEADER, NUM_COLS  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PORT = 8799
EXE = ROOT / "wp8f-gui.exe"
# S1 布局：仓库根只放控制台；FM 解析可执行文件在 binary/ 下（gui/src/env.rs 从那里找它）
FM_EXE = ROOT / "binary" / "flightmodel.exe"
CFG = ROOT / "config" / "default.json"
CFG_BACKUP = CFG.read_bytes() if CFG.exists() else None
# 探针截图统一放这里：logs/ 是用户可见目录（GUI「录制文件」下拉就扫它），
# 全绿时整目录删除，别把一堆截图攒在用户脸上。
PROBE_DIR = ROOT / "logs" / "_probe" / "ui_checks"
UPLOADS: list[Path] = []          # 本用例上传到 logs/ 的文件（重名会加序号，以接口返回为准）
PROBE_FONTS: list[Path] = []      # 本用例临时放进 resource/fonts/ 的字体（跑完删除）
RESULTS: list[tuple[str, bool, str]] = []

# 自带的 `.wpr` 夹具（**不依赖用户 logs/ 里有什么**）：带底图 / 不带底图 / **有元数据但没字节** 各一份。
# 底图是真 PNG（浏览器要能解码成像素：xoy 底面就是拿它逐点着色的）。
FIXTURE_MAP = "ui_check_map.wpr"                  # 底图字节 + 元数据都有 → 自己铺
FIXTURE_NOMAP = "ui_check_nomap.wpr"              # 连地图元数据都没有（img_w=0）→ 无可回退 → 网格线
FIXTURE_NOBYTES = "ui_check_nobytes.wpr"          # 元数据说有底图、字节是空的（= 用户实测那种记录）→ 借
MAP_W_M, MAP_H_M = 100_000.0, 80_000.0
MAP_MIN_X_M, MAP_MIN_Y_M = -50_000.0, -40_000.0
IMG_PX = 64                          # 底图 64×64：够看出"颜色来自底图"，体积只有几百字节


def make_map_png(px: int = IMG_PX) -> bytes:
    """64×64 PNG：四象限 + 对角渐变 + 每 8 像素白网格线（不依赖 Pillow）。"""
    rows = []
    for y in range(px):
        row = bytearray(b"\x00")     # 每行前面是 filter 字节
        for x in range(px):
            gx, gy = x * 255 // (px - 1), y * 255 // (px - 1)
            left, top = x < px // 2, y < px // 2
            rgb = (gx, gy, 60) if top else (255 - gx, 255 - gy, 200)
            if left and top:
                rgb = (gx, 40, gy)
            elif not left and not top:
                rgb = (250, gy, 90)
            if x % 8 == 0 or y % 8 == 0:
                rgb = (255, 255, 255)
            row += bytes(rgb)
        rows.append(bytes(row))
    raw = b"".join(rows)

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", px, px, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b""))


def wpr_row(t_ns: int, i: int, steps_every: int = 8) -> str:
    """一帧 CSV：x/y 每 `steps_every` 帧才变一次（复刻真实记录的"阶梯"，平滑测试要用）。"""
    v = [0.0] * NUM_COLS
    step = i // steps_every
    v[0] = float(i)                              # frame
    v[1] = 0.50 + 0.0004 * step                  # x（归一化，东向）
    v[2] = 0.50 - 0.0003 * step                  # y（归一化，南向）
    v[3] = 3000.0 + (i % 50) * 2.0               # altitude
    v[5] = 500.0                                 # ias
    v[6] = 520.0                                 # tas
    v[7] = 0.78                                  # mach
    v[8] = 3.0                                   # aoa
    v[10] = 1.1                                  # ny
    v[13] = 90.0 + (i % 30)                      # heading
    v[15] = -2.0                                 # pitch
    return f"{t_ns},{'f_16c'}," + ",".join(f"{c:g}" for c in v)


def write_wpr(path: Path, frames: int = 200, with_map: bool = True,
              with_img_meta: bool | None = None) -> None:
    """拼一份版本 2 的 `.wpr`（magic + 20 字节头 + meta + CSV + 底图）。

    * `with_map`：写不写**底图字节**（容器头里的 `img_len`）；
    * `with_img_meta`：meta 里写不写 `img_w`/`img_h`（默认跟随 `with_map`）。
      两者**分开**才能造出"元数据说有底图（img_w=2048）、字节却是空的"那种记录 ——
      用户实测的 `logs/wp8f_1790841951594.wpr` 正是这样（记录端漏传原始字节）。
    """
    if with_img_meta is None:
        with_img_meta = with_map
    t0 = 1_700_000_000_000_000_000
    dt = 50_000_000                              # 20 Hz
    csv = CSV_HEADER + "\n" + "\n".join(
        wpr_row(t0 + i * dt, i) for i in range(frames)) + "\n"
    img = make_map_png() if with_map else b""
    meta = {
        "container": 2,
        "created_ms": 1_700_000_000_000,
        "groups": 1,
        "frames": frames,
        "poll_hz": 20.0,
        "map": {"width_m": MAP_W_M, "height_m": MAP_H_M,
                "min_x_m": MAP_MIN_X_M, "min_y_m": MAP_MIN_Y_M,
                "grid_zero": [0.0, 0.0], "grid_steps": [5_500.0, 5_500.0],
                "img_w": IMG_PX if with_img_meta else 0,
                "img_h": IMG_PX if with_img_meta else 0,
                "img_mime": "image/png" if with_img_meta else "application/octet-stream"},
        "generator": "wp8f-probe",
        "aircraft": "f_16c",
        "group_info": [{"aircraft": "f_16c", "frames": frames}],
    }
    mj = json.dumps(meta, ensure_ascii=False).encode("utf-8")
    body = csv.encode("utf-8")
    blob = (b"WPR1" + struct.pack("<I", 2) + struct.pack("<III", len(mj), 1, len(img))
            + struct.pack("<I", len(body)) + mj + body + img)
    path.write_bytes(blob)


def track_upload(saved: dict) -> None:
    """记录上传接口实际落盘的文件名 —— 结束时按这个名单逐个删除。"""
    name = (saved or {}).get("name") or ""
    if name:
        UPLOADS.append(ROOT / "logs" / name)


def check(name: str, ok: bool, detail: str = "") -> bool:
    RESULTS.append((name, bool(ok), str(detail)))
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" · {detail}" if detail else ""), flush=True)
    return bool(ok)


def post_json(target: str, payload: dict) -> dict:
    """POST 一个 JSON body 并读回 JSON（更新器的那几个接口都是 HTTP 200 + `{ok,…}`）。"""
    req = urllib.request.Request(
        target, data=json.dumps(payload).encode(), method="POST",
        headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.loads(r.read().decode("utf-8"))


# ---- 界面文案口径：GUI 是图形界面，不是开发者界面（用户逐条要求） ----
# 用户可见文案里**允许**出现：产品/格式名（.wpr / TacView / ACMI / FlatCSV / HUD / MiniHUD /
# TAS / IAS / VNE / MNE / WEP / Mil / WebGL / xoy）、单位（km/h、m、kg、kgf、hp、Hz、MiB）、版本号。
# **不允许**出现：`--` 命令行参数、`/api/` 内部路由、`poll_ms` / `refresh_hz` / `img_len`
# 这类下划线字段名（= 开发者参数与实现细节）。
DEV_TOKEN_RE = re.compile(r"--|/api/|[A-Za-z][A-Za-z0-9]*_[A-Za-z0-9]+")

# 扫描时排除的容器 —— 它们显示的不是"文案"，而是**用户数据**或用户主动打开的原始文本：
#   #config-json                        「编辑配置 JSON」的原始 JSON（字段名本来就在里面）
#   #config-select / #config-current / #config-msg   配置文件名 + 字节数（用户数据）
#   #rp-add / #rp-files                 录制文件名（用户数据，如 wp8f_1790841951594.wpr）
#   #fm-aircraft / #fm-info .fm-kv b     机型名（游戏数据，如 a_109_eoa2）
#   #rp-telemetry .tel-name             对象名（记录数据，如 f_16c）
#   #notify                             toast 文案里会带上文件名
#   #ms-frame                           第三方模拟器 iframe（不在本仓库文案范围内）
COPY_SCAN_SKIP = ("#config-json, #config-select, #config-current, #config-msg, #rp-add, #rp-files, "
                  "#fm-aircraft, #fm-info .fm-kv b, #rp-telemetry .tel-name, #notify, #ms-frame")


def ui_copy_texts(page) -> list[str]:
    """界面上**看得见**的文案：文本节点 + title/placeholder/aria-label（隐藏元素/未激活页签不算）。"""
    return page.evaluate("""(skipSel) => {
      const skip = new Set(Array.from(document.querySelectorAll(skipSel)));
      const skipped = (el) => { for (let n = el; n; n = n.parentElement) if (skip.has(n)) return true; return false; };
      const shown = (el) => !!(el && el.getClientRects && el.getClientRects().length);
      const out = [];
      const w = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
      for (let n = w.nextNode(); n; n = w.nextNode()) {
        const t = (n.nodeValue || '').trim();
        if (!t || !n.parentElement || skipped(n.parentElement) || !shown(n.parentElement)) continue;
        out.push(t);
      }
      document.querySelectorAll('[title], [placeholder], [aria-label]').forEach((el) => {
        if (skipped(el) || !shown(el)) return;
        for (const a of ['title', 'placeholder', 'aria-label']) {
          const v = el.getAttribute(a);
          if (v && v.trim()) out.push(v.trim());
        }
      });
      return out;
    }""", COPY_SCAN_SKIP)


def dev_tokens(texts: list[str]) -> list[str]:
    """文案里命中的开发者标识符（返回 "命中 ← 原句" 便于定位）。"""
    hits = []
    for t in texts:
        for m in DEV_TOKEN_RE.finditer(t):
            hits.append(f"{m.group(0)} ← {t[:60]}")
    return sorted(set(hits))


# 用户逐条点名要删掉的措辞族：自动保存教程、下次启动生效、给开发者/给 AI 的元说明、内部字段名。
BANNED_COPY = ("无需手动保存", "下次启动生效", "自动保存", "按你的口径", "给用户看", "给我看",
               "给开发者", "按 FF 处理", "AA 就是 alpha", "img_len", "submodule")


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


# 前置：已有控制台在跑就先退出（见 gui_pids 的说明）—— 放在写夹具之前，
# 免得这条路径退出时把夹具留在 logs/（用户可见目录）
_OTHER_GUI = gui_pids()
if _OTHER_GUI:
    print(f"[SKIP] 已有控制台在跑（pid={_OTHER_GUI}）—— 本次探针会与它共用 "
          f"logs/console-host.json 与控制台日志，先退出它再跑测试", flush=True)
    raise SystemExit(2)

# S1 产物布局：FM 解析可执行文件在 binary/ 下。下面「飞行模型」页签的机型列表、曲线与
# 版本号全都要它 —— 缺了会一片空，这里作为一条**断言**记下来（不是跳过，也不放宽）。
check("binary/flightmodel.exe 存在（S1 产物布局）", FM_EXE.is_file(), str(FM_EXE))

# 夹具落盘（UPLOADS 兜底删除，含失败路径）
# 注意 `with_img_meta`：FIXTURE_NOBYTES 走"元数据说有底图、字节是空的"那条路（借底图），
# FIXTURE_NOMAP 连元数据都没有（img_w=0）→ 谁都借不到 → 网格线。
(ROOT / "logs").mkdir(exist_ok=True)
for _name, _kw in ((FIXTURE_MAP, {"with_map": True}),
                   (FIXTURE_NOMAP, {"with_map": False}),
                   (FIXTURE_NOBYTES, {"with_map": False, "with_img_meta": True})):
    _p = ROOT / "logs" / _name
    write_wpr(_p, **_kw)
    UPLOADS.append(_p)


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


# 单实例判据 = 端口独占。但"端口上有服务"可能是已有控制台，也可能是别的程序：
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

    # ---- 5) 上传接口（先测，纯 HTTP）----
    # 注意：这里写进 logs/ 的文件是**用户可见状态**（GUI 的「录制文件」下拉会列出来），
    # 所以每次上传都登记到 UPLOADS，结束时（含失败路径）逐个删除。
    sample = (ROOT / "logs") / FIXTURE_MAP      # 自带夹具：一定是可回放的 .wpr（v2 容器）
    src = sample if sample.is_file() else None
    if src:
        body = src.read_bytes()[:2_000_000]
        req = urllib.request.Request(
            url + "/api/replay/upload?name=" + urllib.parse.quote("ui_check_upload.wpr"),
            data=body, method="POST",
            headers={"Content-Type": "application/octet-stream"})
        with urllib.request.urlopen(req, timeout=30) as r:
            saved = json.loads(r.read())
        track_upload(saved)
        check("上传接口可用（写入 logs/）", r.status == 200 and saved.get("name"),
              f"name={saved.get('name')} size={saved.get('size')}")
        upd = json.loads(urllib.request.urlopen(url + "/api/replay/list", timeout=30).read())
        check("上传的文件出现在列表里",
              any(i["name"] == saved["name"] for i in upd), saved.get("name", ""))
# 列表项只认 .wpr：形状 = {name,size,mtime}
        check("列表只列 .wpr（无 kind 字段、无旧格式残留）",
              bool(upd) and all(i["name"].lower().endswith(".wpr") for i in upd)
              and all("kind" not in i for i in upd), f"{len(upd)} 项")
        # 再传一次同名 → 应自动改名而不是覆盖
        req2 = urllib.request.Request(
            url + "/api/replay/upload?name=" + urllib.parse.quote("ui_check_upload.wpr"),
            data=body, method="POST",
            headers={"Content-Type": "application/octet-stream"})
        with urllib.request.urlopen(req2, timeout=30) as r2:
            saved2 = json.loads(r2.read())
        track_upload(saved2)
        check("重名自动加序号（不覆盖）", saved2.get("name") != saved.get("name"),
              f"{saved.get('name')} → {saved2.get('name')}")
        # 旧格式必须被**明确拒绝**（400），而不是静默收下再解析失败
        legacy = ROOT / "logs" / "ui_check_upload.acmi"
        UPLOADS.append(legacy)          # 兜底：万一被收下了也要清掉（用户可见目录）
        try:
            bad_req = urllib.request.Request(
                url + "/api/replay/upload?name=" + urllib.parse.quote("ui_check_upload.acmi"),
                data=b"FileType=text/acmi/tacview\n", method="POST",
                headers={"Content-Type": "application/octet-stream"})
            urllib.request.urlopen(bad_req, timeout=30)
            bad_code = 200
        except urllib.error.HTTPError as e:
            bad_code = e.code
        check("上传旧 .acmi 被明确拒绝（400）", bad_code == 400, f"HTTP {bad_code}")
    else:
        check("上传接口可用（写入 logs/）", False, "logs/ 下没有 .wpr 可做样本")

    from playwright.sync_api import sync_playwright
    pw = sync_playwright().start()
    try:
        browser = pw.chromium.launch(channel="msedge", args=["--no-sandbox"])
        page = browser.new_page(viewport={"width": 1365, "height": 768})
        errs = []
        page.on("pageerror", lambda e: errs.append(f"PAGEERROR: {e}"))
        page.goto(url, wait_until="networkidle")
        page.wait_for_selector("#config-form .card", timeout=15000)

        # ---- 3) 底部按钮右下角 ----
        geo = page.evaluate("""() => {
          const bar = document.querySelector('.actionbar').getBoundingClientRect();
          const drag = document.querySelector('#wp8f-drag').getBoundingClientRect();
          const launch = document.querySelector('#wp8f-launch').getBoundingClientRect();
          return { barRight: Math.round(bar.right), dragText: document.querySelector('#wp8f-drag').textContent.trim(),
                   gapRight: Math.round(bar.right - launch.right),
                   launchRight: Math.round(launch.right), dragRight: Math.round(drag.right) };
        }""")
        check("「预览」按钮已改名", "预" in geo["dragText"] and "配" not in geo["dragText"],
              geo["dragText"])
        check("按钮靠右下角（距操作条右边 < 40px）", 0 <= geo["gapRight"] < 40,
              f"gap={geo['gapRight']} launchRight={geo['launchRight']} barRight={geo['barRight']}")

        # ---- 3c) 绘制契约：底部条与卡片标题行都不许越界 ----
        boxes = page.evaluate("""() => {
          const bar = document.querySelector('.actionbar');
          const cs = getComputedStyle(bar);
          const inner = bar.getBoundingClientRect().right - parseFloat(cs.paddingRight);
          const out = { barOver: bar.scrollWidth - bar.clientWidth, kidsOver: [], ringW: [], hintOver: 0 };
          for (const el of bar.children) {
            out.kidsOver.push(Math.round(el.getBoundingClientRect().right - inner));
          }
          for (const inp of document.querySelectorAll('.ring-num')) {
            out.ringW.push(Math.round(inp.getBoundingClientRect().width));
          }
          for (const card of document.querySelectorAll('#config-form .card')) {
            const h = card.querySelector('.h3-hint');
            if (!h) continue;
            const c = card.getBoundingClientRect();
            out.hintOver = Math.max(out.hintOver,
                                    Math.round(h.getBoundingClientRect().right - (c.right - 4)));
          }
          return out;
        }""")
        check("底部操作条不越界（内容宽度 ≤ 条宽，各控件右缘都在内边界内）",
              boxes["barOver"] <= 0 and all(v <= 0 for v in boxes["kidsOver"]),
              f"scroll-client={boxes['barOver']} 各控件右缘超出={boxes['kidsOver']}")
        check("卡片标题行的控件不越界（缓冲槽数两个窄数字框 ≤ 64px）",
              boxes["hintOver"] <= 0 and boxes["ringW"] and all(w <= 64 for w in boxes["ringW"]),
              f"标题行超出={boxes['hintOver']} 数字框宽度={boxes['ringW']}")

        # ---- 3b) 「命令行调试」勾选框：勾上后「开始」必须以 console=true 请求（不真启动） ----
        con = page.evaluate("""async () => {
          const box = document.querySelector('#wp8f-console');
          const label = box.parentElement.textContent.trim();
          // 拦下 fetch：只看请求体，不发出去（避免真的拉起 wp8f）
          const real = window.fetch;
          const seen = [];
          const closes = [];
          window.fetch = (url, opts) => {
            // 拦下启动请求；**连 /api/gui/window 一起拦**：否则「开始」成功 800ms 后前端会
            // 真去关窗口，后面的用例就没页面了（踩过）
            const u = String(url);
            if (u.includes('/api/wp8f/')) {
              seen.push(JSON.parse((opts && opts.body) || '{}'));
              return Promise.resolve(new Response('{"ok":true,"pid":1}', { status: 200 }));
            }
            if (u.includes('/api/gui/window')) {
              closes.push(JSON.parse((opts && opts.body) || '{}'));
              return Promise.resolve(new Response('{"ok":true}', { status: 200 }));
            }
            return real(url, opts);
          };
          box.checked = true;
          document.querySelector('#wp8f-launch').click();
          await new Promise((r) => setTimeout(r, 1200));   // 关窗请求在 800ms 后发出
          const toastConsole = !document.querySelector('#toast').classList.contains('hidden');
          const closesAfterConsole = closes.length;
          box.checked = false;
          document.querySelector('#wp8f-drag').click();
          await new Promise((r) => setTimeout(r, 1200));
          const toastDrag = !document.querySelector('#toast').classList.contains('hidden');
          const closesAfterDrag = closes.length;
          window.fetch = real;
          box.checked = false;
          return { label, seen, toastConsole, toastDrag, closesAfterConsole, closesAfterDrag, closes };
        }""")
        check("底部有「命令行调试」勾选框且默认不勾", "命令行" in con["label"],
              f"label={con['label']!r}")
        check("勾上后「开始」带 console=true 启动（预览也一样）",
              len(con["seen"]) == 2 and con["seen"][0].get("console") is True
              and con["seen"][0].get("drag") is False
              and con["seen"][1].get("console") is False and con["seen"][1].get("drag") is True,
              str(con["seen"]))
        check("ESC 提示只在预览模式出现（命令行调试 / 普通启动都不该提示 ESC 退出）",
              con["toastDrag"] is True and con["toastConsole"] is False,
              f"预览={con['toastDrag']} 命令行调试={con['toastConsole']}")
        check("命令行调试也走普通启动的关窗路径（预览不关窗）",
              con["closesAfterConsole"] == 1 and con["closesAfterDrag"] == 1
              and con["closes"][0].get("action") == "close",
              f"命令行={con['closesAfterConsole']} 预览={con['closesAfterDrag']} {con['closes']}")

        # ---- 1) 导航 ----
        tabs = page.eval_on_selector_all("#tabs .tab", "els => els.map(e => e.textContent.trim())")
        check("导航为 配置器/飞行模型/回放/导弹模拟",
              tabs == ["配置器", "飞行模型", "回放", "导弹模拟"], str(tabs))

        # ---- 2) FM 两个控件同一行 ----
        page.click('#tabs .tab[data-tab="fm"]')
        page.wait_for_timeout(500)
        row = page.evaluate("""() => {
          const a = document.querySelector('#fm-fuel').getBoundingClientRect();
          const b = document.querySelector('#fm-extra').getBoundingClientRect();
          return { sameRow: Math.abs(a.top - b.top) < 8, ay: Math.round(a.top), by: Math.round(b.top),
                   aRight: Math.round(a.right), bLeft: Math.round(b.left) };
        }""")
        check("燃油比例与额外重量在同一行", row["sameRow"],
              f"fuel.top={row['ay']} extra.top={row['by']}")

        # ---- 4) 回放：多文件叠加 ----
        page.click('#tabs .tab[data-tab="replay"]')
        page.wait_for_timeout(1500)
        opts = page.eval_on_selector_all("#rp-add option", "els => els.map(e => e.value).filter(Boolean)")
        check("下拉已填充录制文件", len(opts) >= 2, f"{len(opts)} 个")
        for name in opts[:2]:
            page.select_option("#rp-add", name)
            page.wait_for_timeout(1200)
        state = page.evaluate("""() => ({
          chips: document.querySelectorAll('#rp-files .chip').length,
          objects: (document.querySelector('#rp-objects') || {}).textContent,
          filesLabel: document.querySelector('#rp-files').textContent.trim().slice(0, 60),
        })""")
        check("加入 2 个文件 → 2 个标签", state["chips"] == 2, state["filesLabel"])
        check("对象数按多文件合并统计", "2 个文件" in (state["objects"] or ""), state["objects"])
        page.wait_for_timeout(800)
        series_info = page.evaluate("""() => {
          const el = document.getElementById('rp-chart');
          const inst = window.echarts ? echarts.getInstanceByDom(el) : null;
          return { canvas: !!(el && el.querySelector('canvas')),
                   disposed: inst ? inst.isDisposed() : null,
                   series: inst ? (inst.getOption().series || []).length : -1 };
        }""")
        check("3D 图真的挂上了轨迹 series", series_info["series"] >= 2 and series_info["canvas"],
              str(series_info))

        page.click("#rp-files .chip .x")     # 移除第一个
        page.wait_for_timeout(800)
        check("可单独移除已加入文件",
              page.eval_on_selector_all("#rp-files .chip", "els => els.length") == 1,
              page.eval_on_selector("#rp-files", "el => el.textContent.trim().slice(0,40)"))

        page.click("#rp-clear")
        page.wait_for_timeout(600)
        check("可一键清空", page.eval_on_selector_all("#rp-files .chip", "els => els.length") == 0
              and "尚未加入" in page.eval_on_selector("#rp-files", "el => el.textContent"))
        # 清空后再加入一个：图表必须还能画（曾因清空写 innerHTML 破坏实例而失效）
        page.select_option("#rp-add", opts[2] if len(opts) > 2 else opts[0])
        page.wait_for_timeout(1600)
        again = page.evaluate("""() => {
          const el = document.getElementById('rp-chart');
          const inst = window.echarts ? echarts.getInstanceByDom(el) : null;
          return { series: inst ? (inst.getOption().series || []).length : -1,
                   placeholder: !!el.querySelector('.empty') };
        }""")
        check("清空后再加入仍能绘图", again["series"] >= 2 and not again["placeholder"],
              str(again))

        check("无 JS 异常", not [e for e in errs if e.startswith("PAGEERROR")], str(errs[:2]))

        # ---- 6) 配置器：布局/字体拆卡 + 点标签手动输入 + 色块在输入框左边 ----
        page.click('#tabs .tab[data-tab="config"]')      # 必须切回配置页签：隐藏面板里元素不可见/几何为 0
        page.wait_for_timeout(800)
        check("存在独立的「布局」「字体」卡片",
              page.eval_on_selector_all("#config-form .card h3", "els => els.map(e => e.textContent)")
              and all(k in " ".join(page.eval_on_selector_all("#config-form .card h3", "els => els.map(e => e.textContent)"))
                      for k in ("布局", "字体")),
              str(page.eval_on_selector_all("#config-form .card h3", "els => els.map(e => e.textContent.trim())")))
        lim = page.evaluate("""() => {
          const g = (k) => { const r = document.querySelector(`input[type=range][data-key="${k}"]`); return r ? [r.min, r.max] : null; };
          return { font: g('font_size'), circle: g('circle_ring_radius'), mapw: g('map_size_x') };
        }""")
        check("字号上限 72 / 尺寸上限 1440",
              lim["font"] == ["8", "72"] and lim["circle"] == ["20", "1440"] and lim["mapw"] == ["100", "1440"],
              str(lim))
# 数字输入入口在**滑块右边的数字**上：点左侧标签不响应；
        # 点数字 → 出现输入框 → 回车写回；越界夹紧并提示
        cfg_before = page.eval_on_selector("input[type=range][data-key='font_size']", "el => el.value")
        # ① 点左侧标签：不该出现输入框（入口已移到右侧数字）
        page.evaluate("""() => {
          const r = document.querySelector('input[type=range][data-key="font_size"]');
          r.closest('.item').querySelector('label').click();
        }""")
        page.wait_for_timeout(120)
        check("点滑块左侧标签不再进入编辑（入口已移到右侧数字）",
              page.query_selector("input.lbl-input") is None, "点标签后不该有 input.lbl-input")
        # ② 点右侧数字：进入编辑
        page.click('b.sval-edit[data-edit="font_size"]')
        box = page.query_selector("input.lbl-input")
        check("点滑块右侧数字出现手动输入框", box is not None, "input.lbl-input")
        if box:
            box.fill("64"); box.press("Enter"); page.wait_for_timeout(500)
            after = page.evaluate("""() => {
              const r = document.querySelector('input[type=range][data-key="font_size"]');
              return { v: r.value, sval: r.parentElement.querySelector('.sval').textContent,
                       cfg: JSON.parse(document.getElementById('config-json').value).font_size };
            }""")
            check("手动输入 64 三处同步（range/读数/JSON）",
                  after["v"] == "64" and after["sval"] == "64" and after["cfg"] == 64, str(after))
            page.click('b.sval-edit[data-edit="font_size"]')
            box = page.query_selector("input.lbl-input"); box.fill("999"); box.press("Enter")
            page.wait_for_timeout(150)     # 夹紧提示会被 500ms 后的"已保存"覆盖，故只等 150ms
            clamped = page.evaluate("""() => ({ v: document.querySelector('input[type=range][data-key="font_size"]').value,
                                                msg: document.getElementById('config-msg').textContent })""")
            check("超出上限被夹紧到 72 并提示", clamped["v"] == "72" and "72" in clamped["msg"], str(clamped))
            # 还原（直接写回 range 并触发 input）
            page.evaluate("""(v) => { const r = document.querySelector('input[type=range][data-key="font_size"]');
                                      r.value = v; r.dispatchEvent(new Event('input', { bubbles: true })); }""", cfg_before)
            page.wait_for_timeout(700)
        swpos = page.evaluate("""() => {
          const inp = document.querySelector('input.colortext[data-key="data_color"]');
          const row = inp.closest('.sld');
          const sw = row.querySelector('.swatch').getBoundingClientRect();
          const box = inp.getBoundingClientRect();
          return { left: sw.right <= box.left + 1,
                   bg: row.querySelector('.swatch').style.background,
                   raw: inp.value };
        }""")
        check("颜色预览在输入框左边", swpos["left"], str(swpos))
        # 配置口径是 #RRGGBBAA（前 6 位 RGB、末两位 AA 就是透明度）：色块必须按 RGBA 读通道并换算成
        # CSS 颜色。旧 ARGB 口径下 #3EC8659C 会被读成 rgba(200,101,156,0.24)，预览色与 HUD 实际显示对不上 ——
        # 所以这里直接锁数值（CSSOM 会把 alpha 归一成 2 位小数，故 alpha 容差 0.005）。
        raw = str(swpos["raw"]).strip().lstrip("#")
        bg = swpos["bg"]
        parts = [x for x in bg[bg.find("(") + 1:bg.find(")")].split(",") if x.strip()] if "(" in bg else []
        got = (int(parts[0]), int(parts[1]), int(parts[2]),
               float(parts[3]) if len(parts) > 3 else 1.0) if len(parts) >= 3 else None
        want = (int(raw[0:2], 16), int(raw[2:4], 16), int(raw[4:6], 16),
                int(raw[6:8], 16) / 255 if len(raw) >= 8 else 1.0)
        check(f"色块按 #RRGGBBAA 语义换算（{swpos['raw']} → rgb{want[:3]} a={want[3]:.3f}）",
              got is not None and got[:3] == want[:3] and abs(got[3] - want[3]) <= 0.005,
              f"{bg}")
        # 颜色键简化（用户要求）：POI 十字改用警示色、地图/圆环/告警 X 统一用数字色 →
        # 四个旧键的控件必须消失（连同配置里的键一起删，serde 不留 alias）
        color_keys = page.eval_on_selector_all("input.colortext", "els => els.map(e => e.dataset.key)")
        check("颜色卡片已删掉 map_color / circle_color / poi_color / warning_x_color（7 个键）",
              not ({"map_color", "circle_color", "poi_color", "warning_x_color"} & set(color_keys))
              and {"data_color", "label_color", "unit_color", "hint_color", "alert_color",
                   "map_bg_color", "stroke_color"} <= set(color_keys),
              str(color_keys))
        check("告警卡片的「告警X色」控件已删除（告警 X 用数字色）",
              page.eval_on_selector_all("input.colortext", "els => els.map(e => e.dataset.key)").count("warning_x_color") == 0)

        # ---- 6b) 配置器改版：告警独立卡片 / HUD 类型互斥 / 刷新率与记录频率 / 标签对齐 ----
        cards = page.eval_on_selector_all("#config-form .card", """els => els.map(e => ({
              t: e.dataset.card || '',
              keys: Array.from(e.querySelectorAll('[data-key]')).map(i => i.dataset.key) }))""")
        by = {c["t"]: set(c["keys"]) for c in cards}
        check("告警是独立卡片（语音告警 / X闪烁频率 都在里面）",
              {"voice_warnings_enabled", "warning_blink_hz"} <= by.get("告警", set()),
              str(sorted(by.keys())))
        check("告警项已从「面板」卡片移走",
              not ({"voice_warnings_enabled", "warning_blink_hz", "warning_x_color"} & by.get("面板", set())),
              str(sorted(by.get("面板", set()))))
        check("「飞行记录」卡片：只有录制开关 + 记录频率（零点经纬度/格式键已删除）",
              "飞行记录" in by and "TacView" not in by
              and "record.enabled" in by["飞行记录"]
              and not [k for k in by["飞行记录"] if "origin" in k or k.endswith(".format")]
              and page.eval_on_selector_all('#config-form .card[data-card="飞行记录"] input[data-rec-hz]',
                                            "els => els.length") == 1,
              str(sorted(by.get("飞行记录", set()))))
        check("高级编辑只剩按钮（标签已移除）",
              page.eval_on_selector("#json-toggle", "el => el.closest('.item').querySelector('label')") is None)
        widths = page.evaluate("""() => {
          const allSame = (card) => {
            const ls = Array.from(document.querySelectorAll(`.card[data-card="${card}"] .grid .item > label`));
            return ls.length ? ls.every(l => Math.round(l.getBoundingClientRect().width) === Math.round(ls[0].getBoundingClientRect().width)) : false;
          };
          const w = (sel) => { const l = document.querySelector(sel); return l ? Math.round(l.getBoundingClientRect().width) : -1; };
          return { layout: w('.card[data-card="布局"] .item > label'), font: w('.card[data-card="字体"] .item > label'),
                   layoutSame: allSame('布局'), fontSame: allSame('字体'),
                   fontLabels: Array.from(document.querySelectorAll('.card[data-card="字体"] .item > label')).map(l => l.textContent.trim()) };
        }""")
        check("布局卡片标签按 6 个中文字符等宽对齐（75px）",
              widths["layout"] == 75 and widths["layoutSame"], str(widths))
        check("字体卡片标签按 4 个中文字符等宽对齐（50px）",
              widths["font"] == 50 and widths["fontSame"], str(widths))
        check("字体卡片里 miniHUD字号 已改名 HUD字号",
              widths["fontLabels"][-1] == "HUD字号", str(widths["fontLabels"]))
        # HUD 类型：MiniHUD / 圆环 HUD 互斥（写 hud_type 同时同步两个 *_enabled）
        page.check('#config-form input[type=radio][data-hudtype="mini"]')
        page.wait_for_timeout(700)
        ht = page.evaluate("""() => { const c = JSON.parse(document.getElementById('config-json').value);
          return { type: c.hud_type, mini: c.minihud_enabled, circle: c.circle_enabled }; }""")
        check("选 MiniHUD → hud_type=mini 且圆环 HUD 自动关闭（互斥）",
              ht == {"type": "mini", "mini": True, "circle": False}, str(ht))
        page.check('#config-form input[type=radio][data-hudtype="circle"]')
        page.wait_for_timeout(700)
        ht2 = page.evaluate("""() => { const c = JSON.parse(document.getElementById('config-json').value);
          return { type: c.hud_type, mini: c.minihud_enabled, circle: c.circle_enabled }; }""")
        check("选圆环 HUD → hud_type=circle 且 miniHUD 自动关闭（互斥）",
              ht2 == {"type": "circle", "mini": False, "circle": True}, str(ht2))
        hz0 = page.evaluate("""() => {
          const rr = document.querySelector('input[type=range][data-key="refresh_hz"]');
          const rec = document.querySelector('input[data-rec-hz]');
          return { refresh: rr ? [rr.min, rr.max, rr.value] : null, rec: rec ? [rec.min, rec.max, rec.value] : null };
        }""")
        check("面板卡片有「数据刷新 Hz」(5–60)，飞行记录有记录频率且上限 = 刷新率",
              bool(hz0["refresh"]) and hz0["refresh"][0] == "5" and hz0["refresh"][1] == "60"
              and bool(hz0["rec"]) and hz0["rec"][1] == hz0["refresh"][2], str(hz0))
        # 刷新率降到 10 Hz → 记录频率上限跟着降到 10，且 poll_ms 不小于 100ms（≤10Hz）
        page.evaluate("""() => { const rr = document.querySelector('input[type=range][data-key="refresh_hz"]');
          rr.value = '10'; rr.dispatchEvent(new Event('input', { bubbles: true })); }""")
        page.wait_for_timeout(700)
        hz1 = page.evaluate("""() => { const rec = document.querySelector('input[data-rec-hz]');
          const c = JSON.parse(document.getElementById('config-json').value);
          return { recMax: rec.max, recVal: rec.value, pollMs: c.record.poll_ms, refresh: c.refresh_hz }; }""")
        check("刷新率降到 10 Hz → 记录频率被夹到 ≤10 且 poll_ms ≥100ms（记录频率不超过刷新率）",
              hz1["recMax"] == "10" and int(hz1["recVal"]) <= 10 and hz1["pollMs"] >= 100, str(hz1))
        # 还原刷新率（后面的用例还要用配置）
        page.evaluate("""() => { const rr = document.querySelector('input[type=range][data-key="refresh_hz"]');
          rr.value = '30'; rr.dispatchEvent(new Event('input', { bubbles: true })); }""")
        page.wait_for_timeout(700)

        # ---- 6c) 地图记录间隔（map_obj_record_every_frames）：每多少**数据帧**记录一次 ----
        # 语义：最小 1 = 每帧记录（= 数据帧刷新率），默认 8。**上限 = 1 秒的数据帧数 = refresh_hz**
# （30 Hz → 30，5 Hz → 5）。与 record.poll_ms 无关（只看刷新率）。
        # （max(1, poll_ms × refresh_hz / 1000)）已按用户要求删除 —— 把记录频率拖到 5 Hz
        # （poll_ms=200，旧口径下上限只剩 6）也不该改变上限。
        mo0 = page.evaluate("""() => {
          const rec = document.querySelector('input[data-rec-hz]');
          rec.value = '5'; rec.dispatchEvent(new Event('input', { bubbles: true }));
          const rr = document.querySelector('input[type=range][data-key="refresh_hz"]');
          rr.value = '30'; rr.dispatchEvent(new Event('input', { bubbles: true }));
          const r = document.querySelector('input[type=range][data-key="map_obj_record_every_frames"]');
          const lbl = r.closest('.item').querySelector('label');
          const sval = r.parentElement.querySelector('.sval');
          const c = JSON.parse(document.getElementById('config-json').value);
          return { max: r.max, v: r.value, min: r.min, pollMs: c.record.poll_ms, hz: c.refresh_hz,
                   frames: c.map_obj_record_every_frames, label: lbl.textContent,
                   tip: sval.title, edit: sval.dataset.edit || '',
                   editable: sval.classList.contains('sval-edit'),
                   oldKey: 'map_obj_interval_frames' in c,
                   sval: sval.textContent };
        }""")
        check("地图刷新帧间隔滑杆存在、最小 1（每帧记录）",
              mo0["min"] == "1" and "帧间隔" in mo0["label"] and "刷新" in mo0["label"]
              and not mo0["oldKey"], str(mo0))
        check("地图刷新帧间隔的上限 = 1 秒的数据帧数（30 Hz → 30），与 record.poll_ms 无关",
              mo0["pollMs"] == 200 and mo0["hz"] == 30 and mo0["max"] == "30", str(mo0))
        check("上限说明写成“1 秒的数据帧数”，不再提采样周期约束与逐键教程",
              "1 秒的数据帧数" in mo0["tip"] and "最小 1" in mo0["tip"]
              and "采样周期" not in mo0["tip"]
              and "每多少个数据帧记录一次地图对象" not in mo0["tip"], mo0["tip"])
        check("数值输入入口挂在滑块右侧的数字上（不是左侧标签）",
              mo0["editable"] and mo0["edit"] == "map_obj_record_every_frames", str(mo0))
        # 顶到上限 30 → 刷新率降到 10 Hz：上限缩到 10、滑杆值被夹到 10 并写回 JSON
        # 提示要在同一次 evaluate 里读：500ms 后的"已保存"会把它覆盖掉
        mo1 = page.evaluate("""() => {
          const r = document.querySelector('input[type=range][data-key="map_obj_record_every_frames"]');
          r.value = r.max; r.dispatchEvent(new Event('input', { bubbles: true }));
          const rr = document.querySelector('input[type=range][data-key="refresh_hz"]');
          rr.value = '10'; rr.dispatchEvent(new Event('input', { bubbles: true }));
          const c = JSON.parse(document.getElementById('config-json').value);
          return { max: r.max, v: r.value, sval: r.parentElement.querySelector('.sval').textContent,
                   frames: c.map_obj_record_every_frames, msg: document.getElementById('config-msg').textContent };
        }""")
        check("刷新率降到 10 Hz → 地图记录间隔上限缩到 10 且被夹紧写回（上限 = 1 秒的数据帧数）",
              mo1["max"] == "10" and mo1["v"] == "10" and mo1["sval"] == "10"
              and mo1["frames"] == 10 and "1 秒的数据帧数" in mo1["msg"], str(mo1))
        # 记录频率（poll_ms）怎么改都不该动上限：拖到 1 Hz（poll_ms=1000）后上限仍是 10
        mo2 = page.evaluate("""() => {
          const rec = document.querySelector('input[data-rec-hz]');
          rec.value = '1'; rec.dispatchEvent(new Event('input', { bubbles: true }));
          const r = document.querySelector('input[type=range][data-key="map_obj_record_every_frames"]');
          const c = JSON.parse(document.getElementById('config-json').value);
          return { max: r.max, v: r.value, pollMs: c.record.poll_ms, frames: c.map_obj_record_every_frames };
        }""")
        check("记录频率改成 1 Hz（poll_ms=1000）也不改变地图记录间隔的上限（采样周期约束已删除）",
              mo2["pollMs"] == 1000 and mo2["max"] == "10" and mo2["v"] == "10"
              and mo2["frames"] == 10, str(mo2))
        # 还原：刷新率 30、记录频率 30（poll_ms=33）、地图记录间隔回到进来时的值
        page.evaluate("""(orig) => {
          const rr = document.querySelector('input[type=range][data-key="refresh_hz"]');
          rr.value = '30'; rr.dispatchEvent(new Event('input', { bubbles: true }));
          const rec = document.querySelector('input[data-rec-hz]');
          rec.value = '30'; rec.dispatchEvent(new Event('input', { bubbles: true }));
          const r = document.querySelector('input[type=range][data-key="map_obj_record_every_frames"]');
          r.value = String(orig); r.dispatchEvent(new Event('input', { bubbles: true }));
        }""", mo0["frames"])
        page.wait_for_timeout(700)

        # ---- 12) 界面文案口径（配置器页签）----
        # ① 可见文案里不许有开发者参数/实现细节；② 不许有自动保存教程与元说明。
        cfg_tokens = dev_tokens(ui_copy_texts(page))
        check("配置器文案里没有开发者标识符（-- 参数 / /api/ 路由 / 下划线字段名）",
              not cfg_tokens, str(cfg_tokens[:3]))
        cfg_banned = [p for p in BANNED_COPY if p in page.evaluate("() => document.body.innerText")]
        check("配置器界面没有自动保存教程 / 下次启动生效 / 元说明",
              not cfg_banned, str(cfg_banned))

        # ---- 7) 回放：km 尺度 + 视野缩放 + 速度矢量箭头 + 飞机姿态模型 ----
# 「飞机标记」= 当前位置点 + 姿态线框：只有亮黄一种颜色，
        # 深色描边去掉 —— 那层 #1c1c1c 粗线垫底 + 标记点的深色边框看起来就是"黑线"）；
        # 轨迹线 / 标签圆点 / 遥测左边框仍按对象配色（多对象时靠它区分）。
        # 轴上的数字按步长取整显示（用户实测：不取整会出现 43.674941952 这类 9 位小数）。
        page.click('#tabs .tab[data-tab="replay"]')
        page.wait_for_timeout(800)
        ax = page.evaluate("""() => {
          const inst = echarts.getInstanceByDom(document.getElementById('rp-chart'));
          const o = inst.getOption();
          const names = [o.xAxis3D[0].name, o.yAxis3D[0].name, o.zAxis3D[0].name];
          const series = o.series || [];
          const vec = series.filter(s => (s.name || '').includes('速度矢量'));
// 姿态线框按**稳定 id 前缀**取（name 会随语言变）
          const mdl = series.filter(s => String(s.id || '').startsWith('rp-marker'));
          const legacyEdge = series.filter(s => /series\\.edge|姿态描边|姿態描邊|attitude outline/.test(s.name || ''));
          const cur = series.find(s => s.name === 'current') || null;
          const curPt = (cur && (cur.data || [])[0]) || null;
          // 2D 箭头：所有点应在同一高度（水平面内）
          const shaft = (vec.find(s => (s.data || []).length === 2) || {}).data || [];
          const vecDz = shaft.length === 2 ? Math.abs(shaft[0][2] - shaft[1][2]) : null;
          const mdlSpan = (() => {
            const pts = (mdl.find(s => (s.data || []).length === 5) || {}).data || [];
            if (!pts.length) return null;
            const xs = pts.map(p => p[0]), ys = pts.map(p => p[1]);
            return [Math.max(...xs) - Math.min(...xs), Math.max(...ys) - Math.min(...ys)];
          })();
          const colorOf = (s) => (((s || {}).lineStyle || {}).color || null);
          const rgb = (c) => {
            const m = /^#?([0-9a-f]{6})$/i.exec(String(c || ''));
            if (!m) return null;
            const n = parseInt(m[1], 16);
            return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
          };
          // "深色中性色" = 明度低**且**几乎无饱和度 → 就是黑/深灰那类描边色。
          // 对象配色再深也带明显饱和度（如 #7a5c3e 的 max-min = 60），不会被误判。
          const isDarkNeutral = (c) => {
            const v = rgb(c);
            if (!v) return false;
            const mx = Math.max(...v), mn = Math.min(...v);
            return mx <= 140 && (mx - mn) <= 28;
          };
          const coordColor = (s) => colorOf(s) || ((s.itemStyle || {}).color) || null;
          // 顶层 const 不挂在 window 上：用 new Function 取全局词法作用域里的同名常量
          const g = (n) => { try { return new Function('return ' + n)(); } catch (e) { return null; } };
          const axisModel = (t) => { try { return inst.getModel().getComponent(t); } catch (e) { return null; } };
          const viewLabels = (t) => {
            try {
              const m = axisModel(t);
              return (m && m.axis && m.axis.getViewLabels)
                ? m.axis.getViewLabels().map(l => String(l.formattedLabel)) : null;
            } catch (e) { return null; }
          };
          const fmtOf = (t) => {
            const a = (o[t] && o[t][0]) || {};
            return ((a.axisLabel || {}).formatter) || null;
          };
          const probeFmt = (t) => {
            const f = fmtOf(t);
            return typeof f === 'function' ? [String(f(43.674941952)), String(f(-43.674941952)),
                                            String(f(0.4))] : null;
          };
          return { names, sx: o.xAxis3D[0].max - o.xAxis3D[0].min, sy: o.yAxis3D[0].max - o.yAxis3D[0].min,
                   boxW: o.grid3D[0].boxWidth, boxD: o.grid3D[0].boxDepth,
                   vec: vec.map(s => (s.data || []).length), mdl: mdl.map(s => (s.data || []).length),
                   edgeLeft: legacyEdge.map(s => s.name),
                   mdlColors: mdl.map(colorOf),
                   mdlPts: mdl.map(s => (s.data || []).length).sort(),
                   allColors: series.map(coordColor),
                   darkSeries: series.filter(s => isDarkNeutral(coordColor(s))).map(s => [s.name, coordColor(s)]),
                   // 实测清单（名称/类型/颜色/点数）—— 用户要的"到底画了哪些线"
                   inventory: series.map(s => ({ n: s.name, t: s.type, id: s.id || null,
                                                 c: coordColor(s), p: (s.data || []).length })),
                   palette: g('SERIES_COLORS') || [],
                   curColor: (curPt && curPt.itemStyle) ? curPt.itemStyle.color : null,
                   curBorder: (cur && cur.itemStyle && cur.itemStyle.borderColor) || null,
                   curBorderW: (cur && cur.itemStyle && cur.itemStyle.borderWidth) || 0,
                   curSize: cur ? cur.symbolSize : null,
                   markerColor: g('RP_MARKER_COLOR'), markerId: g('RP_MARKER_ID'),
                   markerIds: mdl.map(s => s.id),
                   markerW: g('RP_MARKER_MAIN_W'),
                   axisNums: { raw: [o.xAxis3D[0].min, o.xAxis3D[0].max, o.yAxis3D[0].min,
                                     o.yAxis3D[0].max, o.zAxis3D[0].min, o.zAxis3D[0].max],
                               intervals: [o.xAxis3D[0].interval, o.yAxis3D[0].interval, o.zAxis3D[0].interval],
                               probe: probeFmt('xAxis3D'),
                               labels: viewLabels('xAxis3D') },
                   vecFlatZ: vecDz !== null && vecDz < 0.01, vecDz, modelH: rp.modelH, modelV: rp.modelV,
                   mdlSpanKm: mdlSpan ? Math.round(Math.hypot(mdlSpan[0], mdlSpan[1]) * 1000) / 1000 : null };
        }""")
        check("坐标轴已改成 km 尺度", all("km" in n for n in ax["names"][:2]), str(ax["names"]))
        check("默认视野 50 × 50 km", abs(ax["sx"] - 50) < 0.01 and abs(ax["sy"] - 50) < 0.01,
              f"{ax['sx']:.1f} × {ax['sy']:.1f} km")
        check("3D 盒子宽深比跟随 km 跨度（俯视不拉伸）", abs(ax["boxW"] / ax["boxD"] - 1) < 0.05,
              f"{ax['boxW']}/{ax['boxD']}")
        check("速度矢量是水平面内的 2D 箭头（杆 2 点 + 倒刺 3 点，z 相同）",
              len(ax["vec"]) == 2 and sorted(ax["vec"]) == [2, 3]
              and ax.get("vecFlatZ"), f"{ax['vec']} 起伏={ax.get('vecDz')}m")
        check("飞机姿态线框（俯视轮廓 5 点 + 垂尾 2 点，比当前点标记略大）",
              len(ax["mdl"]) == 2 and sorted(ax["mdl"]) == [2, 5], str(ax["mdl"]))
# ---- 标记配色：只留亮黄，深色描边全删 ----
        low = lambda v: str(v or "").lower()          # noqa: E731
        check("飞机标记只有亮黄一种颜色（姿态线框 = 当前位置点 = RP_MARKER_COLOR）",
              low(ax["markerColor"]) == "#ffe600"
              and len(ax["mdlColors"]) == 2 and all(low(c) == low(ax["markerColor"]) for c in ax["mdlColors"])
              and ax["mdlPts"] == [2, 5] and low(ax["curColor"]) == low(ax["markerColor"]),
              f"线框 {ax['mdlColors']} 点数 {ax['mdlPts']} 点色 {ax['curColor']} 常量 {ax['markerColor']}")
        check("深色描边已删干净：没有深色/黑色 series，也没有 `rp.series.edge` 残留",
              not ax["darkSeries"] and not ax["edgeLeft"] and float(ax["markerW"] or 0) >= 2,
              f"深色 series {ax['darkSeries']} · 旧描边 series {ax['edgeLeft']} · 线宽 {ax['markerW']}")
        check("当前位置点没有深色边框（去掉边框后靠尺寸保证可见）",
              not ax["curBorder"] and float(ax["curSize"] or 0) >= 8,
              f"边框 {ax['curBorder']!r}/{ax['curBorderW']} 尺寸 {ax['curSize']}")
        # 唯一性：同一份 option 里 id 重复会让 ECharts **静默丢掉整组 series**（接线时真踩过：
        # 两条 `rp-marker` 让整张图 0 条 series，而控制台一个错都不报）
        check("姿态线框的 series id 唯一（id 重复会让 ECharts 静默丢掉整组 series）",
              len(ax["markerIds"]) == 2 and len(set(ax["markerIds"])) == 2
              and all(str(i or "").startswith(str(ax["markerId"])) for i in ax["markerIds"]),
              str(ax["markerIds"]))
        # series 清单（用户要的"到底画了哪些线"）：只有对象色、亮黄标记、底面三类，报错时原样列出
        _known = {str(c).lower() for c in (ax["palette"] or [])} | {low(ax["markerColor"])}
        def _light_neutral(c: str) -> bool:
            s = str(c or "").lstrip("#")
            if len(s) != 6:
                return False
            try:
                v = [int(s[i:i + 2], 16) for i in (0, 2, 4)]
            except ValueError:
                return False
            return max(v) >= 180 and (max(v) - min(v)) <= 40      # 底面网格线 #c2c2bb 那种浅灰
        _unknown = [s for s in ax["inventory"] if s["c"] and low(s["c"]) not in _known
                    and not _light_neutral(s["c"])]
        check("3D 图的 series 只有三类：对象配色（轨迹/速度矢量）、亮黄（飞机标记）、底面地图/网格线",
              not _unknown,
              f"{len(ax['inventory'])} 条 · 未知颜色 {_unknown} · 清单 "
              + str([(s["n"], s["t"], s["c"], s["p"]) for s in ax["inventory"]]))
        legend = page.evaluate("""() => {
          const el = document.getElementById('rp-marker-legend');
          if (!el) return null;
          const dot = el.querySelector('.mk-dot');
          const line = el.querySelector('.mk-line');
          const rgb = (n) => (n ? getComputedStyle(n).backgroundColor : null);
          return { text: (el.innerText || '').trim(), shown: el.getClientRects().length > 0,
                   dotBg: rgb(dot), lineBg: rgb(line),
                   dotShadow: dot ? getComputedStyle(dot).boxShadow : '',
                   lineShadow: line ? getComputedStyle(line).boxShadow : '' };
        }""")
        yellow = "rgb(255, 230, 0)"
        check("回放页签的标记图例与图上颜色一致（只有亮黄，没有深色描边）",
              bool(legend) and legend["shown"] and "当前位置" in legend["text"]
              and legend["dotBg"] == yellow and legend["lineBg"] == yellow
              and "28, 28, 28" not in legend["dotShadow"] and "28, 28, 28" not in legend["lineShadow"],
              str(legend))
        obj_colors = page.evaluate("""() => {
          const inst = echarts.getInstanceByDom(document.getElementById('rp-chart'));
          const hex = (css) => {
            const m = /rgba?\\((\\d+),\\s*(\\d+),\\s*(\\d+)/.exec(css || '');
            if (!m) return String(css || '').toLowerCase();
            return '#' + [1, 2, 3].map(i => Number(m[i]).toString(16).padStart(2, '0')).join('');
          };
          const trail = (inst.getOption().series || [])
            .filter(x => x.type === 'line3D' && !/姿态|速度矢量|xoy 底面/.test(x.name || ''))
            .map(x => String(((x.lineStyle || {}).color) || '').toLowerCase());
          return { trail: trail,
                   chips: Array.from(document.querySelectorAll('#rp-files .chip .dot'))
                     .map(d => hex(getComputedStyle(d).backgroundColor)),
                   tel: Array.from(document.querySelectorAll('#rp-telemetry .tel-row'))
                     .map(r => hex(getComputedStyle(r).borderLeftColor)) };
        }""")
        check("轨迹 / 标签圆点 / 遥测左边框同色（图例与图上颜色对得上），且都不是标记的亮黄",
              obj_colors["trail"] and obj_colors["chips"] and obj_colors["tel"]
              and obj_colors["chips"] == obj_colors["trail"][:len(obj_colors["chips"])]
              and obj_colors["tel"] == obj_colors["trail"][:len(obj_colors["tel"])]
              and low(ax["markerColor"]) not in obj_colors["trail"], str(obj_colors))
        # ---- 轴上的数字取整到"好看步长"的位数（用户实测：会出现 43.674941952）----
        ai = ax["axisNums"]
        dec = lambda s: len(str(s).split(".")[1]) if "." in str(s) else 0   # noqa: E731
        check("轴刻度步长是 1/2/5×10ⁿ（不是 ECharts 随机算出来的怪步长）",
              all(isinstance(v, (int, float)) and v > 0 for v in ai["intervals"])
              and abs(ai["intervals"][0] - 10) < 1e-9, str(ai["intervals"]))
        check("轴显示值按步长取整（43.674941952 → 44；范围/内部计算精度不变）",
              ai["probe"] == ["44", "-44", "0"]
              and abs(ax["sx"] - 50) < 0.01, f"{ai['probe']} 跨度 {ax['sx']:.3f}（范围原值 {ai['raw'][:2]}）")
        check("轴上真实印出的标签没有长小数（取自 ECharts 的 getViewLabels）",
              isinstance(ai["labels"], list) and len(ai["labels"]) >= 2
              and all(dec(s) <= 1 for s in ai["labels"]), str(ai["labels"]))
        check("机身尺寸按视野取比例（4%~6% 视野宽，不随高度量纲变形）",
              ax["mdlSpanKm"] is not None and abs(ax["mdlSpanKm"] / 50 - 0.048) < 0.012
              and ax["modelV"] < 200, f"机身 {ax['mdlSpanKm']}km / 视野 50km，垂直单位 {ax['modelV']:.1f}m")
        page.select_option("#rp-scale", "25"); page.wait_for_timeout(800)
        s25 = page.evaluate("""() => { const o = echarts.getInstanceByDom(document.getElementById('rp-chart')).getOption();
            return o.xAxis3D[0].max - o.xAxis3D[0].min; }""")
        check("视野可缩放到 25 km", abs(s25 - 25) < 0.01, f"{s25:.1f} km")
        page.select_option("#rp-scale", "0"); page.wait_for_timeout(800)
        sall = page.evaluate("""() => { const o = echarts.getInstanceByDom(document.getElementById('rp-chart')).getOption();
            return [o.xAxis3D[0].max - o.xAxis3D[0].min, o.yAxis3D[0].max - o.yAxis3D[0].min]; }""")
        check("「全程自适应」跨度=轨迹范围（不再是固定 50）",
              abs(sall[0] - 50) > 0.5 or abs(sall[1] - 50) > 0.5, f"{sall[0]:.1f} × {sall[1]:.1f} km")
        page.select_option("#rp-scale", "50"); page.wait_for_timeout(600)

        # ---- 8) 导弹模拟页签（git submodule 离线 HTML）----
        page.click('#tabs .tab[data-tab="missile"]')
        page.wait_for_timeout(4000)
        ms = page.evaluate("""() => {
          const fr = document.getElementById('ms-frame');
          const doc = fr && fr.contentDocument;
          return { hasFrame: !!fr, title: doc ? doc.title : null,
                   canvases: doc ? doc.querySelectorAll('canvas').length : -1,
                   panel: doc ? !!doc.getElementById('panel') : false,
                   box: fr ? [Math.round(fr.getBoundingClientRect().width), Math.round(fr.getBoundingClientRect().height)] : null };
        }""")
        check("导弹模拟页签加载了 iframe", ms["hasFrame"] and "仿真" in (ms["title"] or ""), str(ms["title"]))
        check("模拟器真的渲染（3 个 canvas + 参数面板）",
              ms["canvases"] >= 3 and ms["panel"], f"canvas={ms['canvases']} box={ms['box']}")
        ms_tokens = dev_tokens(ui_copy_texts(page))
        check("导弹模拟页签文案里没有开发者标识符（-- 参数 / /api/ 路由 / 下划线字段名）",
              not ms_tokens, str(ms_tokens[:3]))
        # 用户要求：「清空」与「操作说明」要有与其它按钮一致的可见边框（别只靠文字）
        borders = page.evaluate("""() => {
          const st = (sel) => { const e = document.querySelector(sel); if (!e) return null;
            const c = getComputedStyle(e);
            return { w: c.borderTopWidth, color: c.borderTopColor }; };
          return { clear: st('#rp-clear'), help: st('#ms-help') };
        }""")

        def visible_border(b):
            return (bool(b) and b["w"] not in ("0px", "")
                    and b["color"] not in ("transparent", "rgba(0, 0, 0, 0)"))

        check("「清空」与「操作说明」按钮有可见边框（与其它按钮一致）",
              visible_border(borders["clear"]) and visible_border(borders["help"]), str(borders))

        # 切出页签必须**卸载** iframe（否则 Three.js 渲染循环会一直在后台跑满 GPU/CPU）
        page.click('#tabs .tab[data-tab="config"]')
        page.wait_for_timeout(500)
        gone = page.evaluate("""() => ({ frame: !!document.getElementById('ms-frame'),
                                        tip: (document.querySelector('#ms-holder .tip')||{}).textContent || '' })""")
        check("切出导弹模拟页签后 iframe 被卸载（模拟器停跑）",
              not gone["frame"] and "模拟器" in gone["tip"], str(gone))
        # 切回来必须重新加载（卸载是"关闭"，不是"永久禁用"）
        page.click('#tabs .tab[data-tab="missile"]')
        page.wait_for_timeout(3500)
        back = page.evaluate("() => !!document.getElementById('ms-frame')")
        check("切回导弹模拟页签会重新加载模拟器", back, str(back))

        # ---- 10) 批 1 前端修复的回归守卫 ----
        # ① 拖油量不能清空「加入对比」的机型：固定 A → 切到 B → 拖油量，渲染集应仍是 2 个
        page.click('#tabs .tab[data-tab="fm"]')
        page.wait_for_timeout(4000)
        # FM 数据库版本：页签显著位置那一行读的就是本地数据里的 resource/data/version（纯文本一行）——
        # 与磁盘上的文件、与后端接口三边对齐；数据没搬到位时显示"未知"而不是编一个版本号
        ver_file = ROOT / "resource" / "data" / "version"
        want_ver = ver_file.read_text(encoding="utf-8").strip().splitlines()[0].strip() if ver_file.is_file() else None
        fm_ver = page.evaluate("""() => {
          const el = document.getElementById('fm-version');
          return { has: !!el, text: el ? (el.textContent || '').trim() : '',
                   shown: el ? el.getClientRects().length > 0 : false,
                   api: null };
        }""")
        try:
            fm_ver["api"] = json.loads(urllib.request.urlopen(url + "/api/fm/version", timeout=20).read())
        except Exception as e:  # noqa: BLE001
            fm_ver["api"] = {"error": str(e)}
        api_ver = (fm_ver["api"] or {}).get("version")
        check("FM 数据库版本显示在页签显著位置（机型与计算参数标题旁），与本地 version 文件一致",
              fm_ver["has"] and fm_ver["shown"] and want_ver is not None
              and fm_ver["text"] == f"FM 数据版本：{want_ver}" and api_ver == want_ver,
              f"界面 {fm_ver['text']!r} 接口 {api_ver!r} 文件 {want_ver!r}")
        check("版本来自本地数据的 version 文件（缺文件时接口 ok=false、界面写「未知」，不编造）",
              (fm_ver["api"] or {}).get("ok") is True
              and str((fm_ver["api"] or {}).get("source", "")).replace("\\", "/").endswith(
                  "resource/data/version"),
              str(fm_ver["api"]))
        ac = page.eval_on_selector_all("#fm-aircraft option", "els => els.map(e => e.value)")
        page.click("#fm-compare")                      # 固定 A（当前机型）
        page.wait_for_timeout(800)
        pinned_a = page.evaluate("() => Object.keys(fmPinned)")
        options = [v for v in ac if v]
        if len(options) >= 2:
            page.select_option("#fm-aircraft", options[1])   # 切到 B：渲染集 = A(固定) + B
            # 等**渲染集真的变成 2**，不要用固定 4s 睡眠：冷启动下一次性重画 6 张 ECharts
            #（含一个 WebGL 3D 曲面）可能超过 4s —— 那是探针的 flake，不是产品的 bug
            #（同一条断言在两次运行里一次是 1→1、一次是 1→2，被测代码没变）。
            # `wait_for_function` 条件一满足立刻返回，所以绿的时候不额外花时间；
            # 真等不到也只是这一条红（上限 15s），不会把整包挂死。
            try:
                page.wait_for_function("() => Object.keys(fmCompare).length >= 2", timeout=15000)
            except Exception:  # noqa: BLE001
                pass
        cnt_before = page.evaluate("() => Object.keys(fmCompare).length")
        page.eval_on_selector("#fm-fuel",
                              "el => { el.value = 30; el.dispatchEvent(new Event('input', {bubbles: true})); }")
        try:
            page.wait_for_function("() => Object.keys(fmCompare).length >= 2", timeout=10000)
        except Exception:  # noqa: BLE001
            pass
        cnt_after = page.evaluate("() => Object.keys(fmCompare).length")
        check("拖油量后「加入对比」的机型没被清空",
              cnt_before == 2 and cnt_after == 2, f"固定={pinned_a} 对比集 {cnt_before} → {cnt_after}")
        fm_tokens = dev_tokens(ui_copy_texts(page))
        check("飞行模型页签文案里没有开发者标识符（-- 参数 / /api/ 路由 / 下划线字段名）",
              not fm_tokens, str(fm_tokens[:3]))

        # ② 非配置器页签也要看得见提示（状态栏挂在只在配置器显示的底部操作条上）
        page.click('#tabs .tab[data-tab="replay"]')
        page.wait_for_timeout(500)
        bar_hidden = page.evaluate(
            "() => getComputedStyle(document.querySelector('.actionbar')).display === 'none'")
        page.click("#rp-clear")
        page.wait_for_timeout(400)
        page.evaluate("() => { document.getElementById('notify').innerHTML = ''; }")
        page.click("#rp-play")                          # 没有录制文件 → 应给出提示
        page.wait_for_timeout(700)
        toast = page.evaluate("() => (document.getElementById('notify') || {}).innerText || ''")
        check("回放页签的提示以 toast 可见（状态栏此时不可见）",
              bar_hidden and "录制文件" in toast, f"bar_hidden={bar_hidden} toast={toast[:40]!r}")

        # ③ `.wpr` 飞行记录：转换区 + 3D 里的 xoy 底面（结构层；底面渲染要真有 .wpr 才走得到）
        page.wait_for_timeout(300)
        wpr_ui = page.evaluate("""() => {
          const box = document.getElementById('rp-convert-box');
          const fmt = document.getElementById('rp-convert-format');
          const lat = document.getElementById('rp-convert-lat');
          const lon = document.getElementById('rp-convert-lon');
          const cssRules = [];
          for (const sh of Array.from(document.styleSheets)) {
            try { cssRules.push(...Array.from(sh.cssRules)); } catch (e) { /* 跨域表跳过 */ }
          }
          return {
            boxTag: box ? box.tagName.toLowerCase() : '',
            open: box ? box.open : null,
            btn: !!document.getElementById('rp-convert'),
            fmts: fmt ? Array.from(fmt.options).map(o => o.value) : [],
            lat: lat ? lat.value : '',
            lon: lon ? lon.value : '',
            accept: (document.getElementById('rp-file-input') || {}).accept || '',
            mapWrap: !!document.getElementById('rp-map-wrap'),
            mapEl: !!document.getElementById('rp-map'),
            mapCss: cssRules.filter(r => (r.selectorText || '').includes('rp-map')).length,
            ground: !!document.getElementById('rp-ground'),
            groundOn: (document.getElementById('rp-ground') || {}).checked,
          };
        }""")
        check("转换区默认收起且格式三选一（acmi / csv / flat）",
              wpr_ui["boxTag"] == "details" and wpr_ui["open"] is False and wpr_ui["btn"]
              and wpr_ui["fmts"] == ["acmi", "csv", "flat"], str(wpr_ui))
        check("转换区零点经纬度默认值与 logger 一致（55.7558 / 37.6173）",
              wpr_ui["lat"] == "55.7558" and wpr_ui["lon"] == "37.6173",
              f"lat={wpr_ui['lat']} lon={wpr_ui['lon']}")
        check("文件选择器只接受 .wpr（旧 .acmi/.csv 已下线）",
              wpr_ui["accept"].strip() == ".wpr", wpr_ui["accept"])
        check("回放页签的 2D 地图面板已删除（DOM + CSS 都没有残留）",
              not wpr_ui["mapWrap"] and not wpr_ui["mapEl"] and wpr_ui["mapCss"] == 0, str(wpr_ui))
        check("回放页签有「xoy 底面」开关且默认开",
              wpr_ui["ground"] and wpr_ui["groundOn"] is True, str(wpr_ui))

        shot = PROBE_DIR / f"ui_check_{int(time.time())}.png"
        PROBE_DIR.mkdir(parents=True, exist_ok=True)
        page.click('#tabs .tab[data-tab="replay"]')
        page.select_option("#rp-add", opts[0])
        page.wait_for_timeout(1500)
        page.screenshot(path=str(shot))
        print("截图:", shot.relative_to(ROOT))

        # ---- 10b) xoy 底面：**只有原图纹理一种画法**（4 顶点）；没底图 → **不铺底面**（只留网格线）----
        def ground_state(tag):
            return page.evaluate("""() => {
              const inst = echarts.getInstanceByDom(document.getElementById('rp-chart'));
              const opt = (inst && !inst.isDisposed()) ? inst.getOption() : null;
              const series = (opt && opt.series) || [];
              const g = series.find(s => (s.name || '') === 'xoy 底面');
              const data = (g && g.data) || [];
              const head = data.slice(0, 80);
              // surface 的 data = [x,y,z,色]；line3D 的 data = 折线数组 [[x,y,z],...]
              const zAll0 = head.length > 0 && head.every(p =>
                Array.isArray(p[0]) ? p.every(q => q[2] === 0) : p[2] === 0);
              const note = document.getElementById('rp-ground-note');
              const resNote = document.getElementById('rp-ground-res-note');
              // getOption() 里 colorMaterial 是**普通对象**（不是组件，不会被规整成数组）——
              // 但两种形状都容忍，免得将来 echarts 改了归一化方式就假红
              const cmRaw = (g && g.colorMaterial) || null;
              const cm = Array.isArray(cmRaw) ? cmRaw[0] : cmRaw;
              return {
                type: g ? g.type : null, n: data.length,
                zAll0: zAll0,
                texture: cm ? cm.detailTexture : null,
                zmin: (opt && opt.zAxis3D) ? opt.zAxis3D[0].min : null,
                stats: typeof rp !== 'undefined' ? rp.groundStats : null,
                note: note ? note.textContent : '', noteShown: note ? !note.hidden : false,
                resNote: resNote ? resNote.textContent : '',
              };
            }""")
        # 逐像素网格 / 1:2 / 1:4 / 1:8 / 自动网格 = 用户要求**整体删除**：
        # DOM（下拉）＋ 全局函数/常量 ＋ rp.ground* 状态 ＋ groundStats 字段 一样都不许留。
        # 注意：顶层 `const` 不是 window 的属性（只有 function 声明才是），所以这里用
        # new Function("return typeof X") 查全局词法作用域 —— 否则常量那两条会"永远为真"。
        grid_ui = page.evaluate("""() => {
          const declared = (n) => {
            try { return new Function('return typeof ' + n)() !== 'undefined'; }
            catch (e) { return true; }
          };
          return {
            sel: !!document.getElementById('rp-ground-res'),
            labels: Array.from(document.querySelectorAll('#tab-replay .item > label')).map(l => l.textContent.trim()),
            globals: ['rpGroundPlan', 'rpGroundEnsurePixels', 'RP_GROUND_MAX_POINTS', 'RP_GROUND_BUDGET',
                      'rpGroundTexture'].filter(declared),
            state: ['groundTex', 'groundRes', 'groundMaxPoints'].filter(k => k in rp),
          };
        }""")
        check("「底面画法」下拉已删除（底面只剩原图纹理一种画法）",
              not grid_ui["sel"] and "底面画法" not in grid_ui["labels"], str(grid_ui["labels"]))
        check("网格档代码路径已删除（rpGroundPlan / rpGroundEnsurePixels / 上限常量 / rp.ground* 状态）",
              grid_ui["globals"] == ["rpGroundTexture"] and grid_ui["state"] == [],
              f"仍在的全局：{grid_ui['globals']}（只该剩 rpGroundTexture）；状态：{grid_ui['state']}")
        page.click("#rp-clear")
        page.select_option("#rp-add", FIXTURE_MAP)
        page.wait_for_timeout(2000)      # 底图要异步加载（贴图由 GPU 采样）
        g_map = ground_state("map")
        check("有底图（唯一画法 = 原图纹理）：surface 四边形落在 z=0，贴图指向本记录的底图",
              g_map["type"] == "surface" and g_map["n"] == 4 and g_map["zAll0"]
              and isinstance(g_map["texture"], str)
              and f"/api/replay/map?file={FIXTURE_MAP}" in g_map["texture"],
              f"type={g_map['type']} n={g_map['n']} tex={str(g_map['texture'])[:60]}")
        check("原图纹理：恒定 4 个顶点、耗时记在 rp.groundStats（不随视野变慢）",
              (g_map["stats"] or {}).get("mode") == "texture"
              and g_map["stats"].get("points") == 4
              and g_map["stats"].get("srcPx") == [IMG_PX, IMG_PX]
              and g_map["stats"].get("ms", 1e9) < 500, str(g_map["stats"]))
        check("groundStats 只留纹理档字段（网格档的 res/cellM/srcKmPerPx/needPoints/maxPoints/capped/auto 全没了）",
              not ({"res", "cellM", "srcKmPerPx", "needPoints", "maxPoints", "capped", "auto"}
                   & set(g_map["stats"] or {}))
              and {"srcPx", "pxM", "coverKm", "texels", "points", "ms"} <= set(g_map["stats"] or {}),
              str(sorted((g_map["stats"] or {}).keys())))
        check("提示行写明「原图纹理 / 可见区 = 多少原图像素」",
              "原图纹理" in g_map["resNote"] and "原图像素" in g_map["resNote"]
              and str(IMG_PX) in g_map["resNote"], g_map["resNote"])
        check("自己带底图时不标注来源（imageFrom=本记录、提示行不显示）",
              (g_map["stats"] or {}).get("imageFrom") == FIXTURE_MAP and not g_map["noteShown"],
              f"{g_map['stats']} note={g_map['note']!r}")
        check("高度轴下沿含 0（底面画在盒子里，不落到盒子外）",
              g_map["zmin"] is not None and g_map["zmin"] <= 0, f"zmin={g_map['zmin']}")

        # 逐像素网格档（1:1）已整体删除：底图带底图时也该一直是 4 顶点的原图纹理，
# 底面只有原图纹理一种画法（4 顶点恒定开销），没有逐像素/多分辨率档；
        g_tex = ground_state("texture-only")
        check("有底图时始终是 4 顶点的原图纹理（不会退化成逐像素网格 surface）",
              g_tex["type"] == "surface" and g_tex["n"] == 4
              and (g_tex["stats"] or {}).get("mode") == "texture"
              and g_tex["stats"].get("points") == 4, str(g_tex["stats"]))
        check("回放页签可见文案里不再出现底面画法/网格档字样（下拉与选项一起消失）",
              not any(w in page.evaluate("() => document.getElementById('tab-replay').innerText")
                      for w in ("底面画法", "逐像素网格", "自动网格", "网格 1:2", "网格 1:4", "网格 1:8")),
              page.evaluate("() => document.getElementById('tab-replay').innerText")[:200])
        # ---- 12) 界面文案口径（回放页签：分辨率提示行 + 底图来源提示行都要在扫描范围内）----
        rp_tokens = dev_tokens(ui_copy_texts(page))
        check("回放页签文案里没有开发者标识符（-- 参数 / /api/ 路由 / 下划线字段名）",
              not rp_tokens, str(rp_tokens[:3]))

        page.click("#rp-clear")
        page.select_option("#rp-add", FIXTURE_NOMAP)
        page.wait_for_timeout(1500)
        g_none = ground_state("nomap")
        check("连地图元数据都没有（img_w=0）：不铺底面 → z=0 网格线（line3D）",
              g_none["type"] == "line3D" and g_none["n"] >= 2 and g_none["zAll0"]
              and (g_none["stats"] or {}).get("mode") == "grid", str(g_none["stats"]))
        check("退化原因写清「记录没带底图（不铺底面地图）」，并在界面上说明",
              "没带底图" in ((g_none["stats"] or {}).get("reason") or "")
              and g_none["noteShown"] and "网格线" in g_none["note"],
              f"{g_none['stats']} note={g_none['note']!r}")
        check("退化原因写清「记录没带底图（不铺底面地图）」，并在界面上说明（不是静默空白）",
              "没带底图" in ((g_none["stats"] or {}).get("reason") or "")
              and g_none["noteShown"] and "网格线" in g_none["note"],
              f"{g_none['stats']} note={g_none['note']!r}")
        # 「没带底图」这一档提示行正显示着（内部术语 img_len / "按你的口径" 就是从这里撤掉的）
        rp_note_tokens = dev_tokens(ui_copy_texts(page))
        check("没带底图的提示行不含内部术语与元说明（img_len / 按你的口径 / 重录指导）",
              not rp_note_tokens
              and not [p for p in BANNED_COPY if p in page.evaluate("() => document.body.innerText")],
              f"{rp_note_tokens[:3]} note={g_none['note']!r}")

        # 「元数据说有底图、字节是空的」= 用户实测那种记录（logs/wp8f_1790841951594.wpr）：
# 记录没带底图 → 不铺底面（也不借别的记录的图）→ 退化成网格线，界面上写明原因。
        # 注意 logs/ 里此刻确实躺着同尺寸、带底图的记录（本用例自己上传的 ui_check_upload*.wpr），
        # 所以这条断言同时证明"**没有**借图"。
        page.click("#rp-clear")
        page.select_option("#rp-add", FIXTURE_NOBYTES)
        page.wait_for_timeout(2200)
        g_borrow = ground_state("nobytes")
        check("元数据说有底图但字节为空：不铺底面地图（不借别的记录的图）→ z=0 网格线",
              g_borrow["type"] == "line3D" and (g_borrow["stats"] or {}).get("mode") == "grid"
              and g_borrow["zAll0"], f"{g_borrow['stats']}")
        check("不铺底面的原因写在界面提示行上（不是静默空白）",
              g_borrow["noteShown"] and "没带底图" in g_borrow["note"],
              repr(g_borrow["note"]))

        # 关掉开关 → 也是网格线；再打开 → 回到 surface
        page.click("#rp-clear")
        page.select_option("#rp-add", FIXTURE_MAP)
        page.wait_for_timeout(1800)
        page.uncheck("#rp-ground")
        page.wait_for_timeout(800)
        g_off = ground_state("off")
        check("关掉「xoy 底面」开关 → 退化成网格线",
              g_off["type"] == "line3D" and (g_off["stats"] or {}).get("mode") == "grid",
              str(g_off["stats"]))
        page.check("#rp-ground")
        page.wait_for_timeout(900)
        g_on = ground_state("on")
        check("重新打开 → 底面回来（仍是 z=0 平面）",
              g_on["type"] == "surface" and g_on["zAll0"], f"type={g_on['type']} n={g_on['n']}")

# ---- 11) 轨迹 = 原始采样点直接连线（无平滑控件/插值核） ----
        # 用户明确要求删掉整套平滑算法（Catmull-Rom / 二次 / 线性重采样 + 过冲保护 + 下拉），
        # 所以这里断言的是"**没有**这套东西"：控件消失、对象行不再报重采样统计、
        # 轨迹点数 = 原始采样点数（抽稀上限内）、速度矢量仍按"过去两点之差"画得出来。
        # 夹具本身就是"位置每 8 帧才变"的阶梯采样，正好验证"原样画原始采样"。
        wpr_opt = next((o for o in opts if o.lower().endswith(".wpr")), None)
        if wpr_opt:
            page.click("#rp-clear")
            page.select_option("#rp-add", wpr_opt)
            page.wait_for_timeout(1800)
            sm = page.evaluate("""() => {
              const inst = echarts.getInstanceByDom(document.getElementById('rp-chart'));
              const opt = (inst && !inst.isDisposed()) ? inst.getOption() : null;
              const series = (opt && opt.series) || [];
              const vec = series.find(s => (s.name || '').includes('速度矢量'));
              const o = (rp.objects || [])[0] || {};
              const frames = o.frames || [];
              // 轨迹点必须**逐点等于**原始采样点（同一抽稀规则下）：有插值点就不相等
              const step = Math.max(1, Math.ceil(frames.length / 2500));
              const want = [];
              for (let k = 0; k < frames.length; k += step) want.push(frames[k]);
              if (frames.length && (frames.length - 1) % step !== 0) want.push(frames[frames.length - 1]);
              const ld = (rp.lineData || [])[0] || [];
              const exact = ld.length === want.length && ld.every((p, i) =>
                p[0] === want[i][1] && p[1] === want[i][2] && p[2] === want[i][3]);
              return { interp: !!document.getElementById('rp-interp'),
                       interpGlobals: ['rpResampleFrames', 'rpCtrlPoint', 'rpCatmull', 'rpQuadratic',
                                       'rpResampleNote', 'rpEvalAt']
                         .filter(n => typeof window[n] !== 'undefined'),
                       smoothFlag: ('interp' in rp),
                       label: (document.querySelector('#rp-objects') || {}).textContent || '',
                       resampleField: ('resample' in o) || ('raw' in o),
                       rawPoints: frames.length, trailPts: ld.length, exact: exact,
                       vecPts: vec ? (vec.data || []).length : -1,
                       series: series.length };
            }""")
            check("「轨迹平滑」下拉与重采样/插值核已彻底删除（DOM + 全局函数 + rp.interp）",
                  sm["interp"] is False and sm["interpGlobals"] == [] and sm["smoothFlag"] is False
                  and sm["resampleField"] is False, str(sm))
            check("对象行不再报重采样统计（CR/二次/平滑关/→ 都不出现）",
                  "CR" not in sm["label"] and "→" not in sm["label"] and "平滑" not in sm["label"]
                  and "个文件" in sm["label"] and "个对象" in sm["label"], sm["label"])
            # 轨迹直接用原始采样点：装载后 frames 就是原始数组，渲染点逐点等于原始采样点
            check("轨迹直接用原始采样点连线（渲染点逐点 = 记录里的原始帧，无插值点）",
                  sm["rawPoints"] >= 100 and sm["trailPts"] >= 100 and sm["exact"],
                  f"raw={sm['rawPoints']} trail={sm['trailPts']} exact={sm['exact']}")
            check("速度矢量/姿态模型照旧绘制（取过去两点原始采样点之差）",
                  sm["vecPts"] in (2, 0) and sm["series"] >= 2, f"vec={sm['vecPts']} series={sm['series']}")
        else:
            check("轨迹平滑已删除（需要 logs/ 下有一份 .wpr）", False, "下拉里没有 .wpr 记录")

        # ---- 13) i18n（P4 骨架 / P6 十五种语言 / B 中文改码）：/api/i18n、语言下拉、
        #          切换后界面确实变、无缺失键、**逐语言断言 + 旧代码兼容** ----
        # 断言口径（用户要求）：① 切换语言后**界面文本确实变化** ② **没有缺失键**
        # ③ **逐语言**：每个 code 的表 `lang` == 请求的 code、键集与 en 完全一致；
        # ④ 旧代码（zh-CN/zh-TW/zh-Hans/zh-Hant/zh_TW/zh-HK）仍然可用 → 新代码（B 的兼容口径）。
        # 文案的唯一来源是 resource/i18n/*.json（Rust 侧读同一份），前端经 /api/i18n 一次取走。
# 中文两份叫 zh_simp / zh_trad（文件名同步），旧写法只做读取兼容。
        ZHS, ZHT = "zh_simp", "zh_trad"
        NEW_LANGS = [ZHT, "de", "pt", "ko", "pl", "vi"]      # P6 新增的 6 种
        BASE_LANGS = [ZHS, "en", "ru", "es", "it", "fr", "ja", "el", "tr"]
        i18n_base = json.loads(urllib.request.urlopen(url + "/api/i18n", timeout=30).read())
        langs = [x["code"] for x in i18n_base.get("languages", [])]
        check("i18n 语言清单 = 15 种（原 9 种 + zh_trad/de/pt/ko/pl/vi）",
              len(langs) == 15 and all(c in langs for c in BASE_LANGS + NEW_LANGS),
              str(langs))
        check("中文两份的 code 是 zh_simp / zh_trad（不再是 zh-CN / zh-TW）",
              ZHS in langs and ZHT in langs and "zh-CN" not in langs and "zh-TW" not in langs,
              str(langs))
        # 磁盘文件名与清单逐一对上（改名漏一个文件 = 这里红）
        disk_langs = sorted(p.stem for p in (ROOT / "resource" / "i18n").glob("*.json"))
        check("resource/i18n/ 下的文件名与语言清单逐一对应（zh_simp.json / zh_trad.json）",
              disk_langs == sorted(langs), f"{disk_langs} vs {sorted(langs)}")
        check("语言名随文件走（朝鲜语的显示名是「국어」，按用户要求）",
              [x["name"] for x in i18n_base.get("languages", []) if x["code"] == "ko"] == ["국어"],
              str([x for x in i18n_base.get("languages", []) if x["code"] == "ko"]))
        check("默认语言是 zh_simp（config 缺 language 键时的默认口径）",
              i18n_base.get("lang") == ZHS, str(i18n_base.get("lang")))
        check("i18n 表：键数 > 300、当前语言无缺失键、常驻键值字节是几十 KB 级",
              i18n_base.get("keys", 0) > 300 and not i18n_base.get("missing")
              and 0 < i18n_base.get("bytes", 0) < 400_000,
              f"keys={i18n_base.get('keys')} missing={i18n_base.get('missing')} bytes={i18n_base.get('bytes')}")
        # 逐语言：每个 code 都要能取到、lang 字段等于请求的 code、键集与 en 一致（无缺无多）
        miss = {}
        for code in langs:
            d = json.loads(urllib.request.urlopen(f"{url}/api/i18n/{code}", timeout=30).read())
            if d.get("missing") or d.get("extra") or d.get("lang") != code:
                miss[code] = {"lang": d.get("lang"), "missing": d.get("missing"), "extra": d.get("extra")}
        check("十五种语言逐语言都无缺失键 / 多余键 / lang 回错（回退链只该用得上 en）", not miss, str(miss))
        # 逐语言：中文两份必须是**简体/繁体**（拿同一批键的值比：zh_simp 与 zh_trad 不能是同一张表）
        zh_a = json.loads(urllib.request.urlopen(f"{url}/api/i18n/{ZHS}", timeout=30).read())
        zh_b = json.loads(urllib.request.urlopen(f"{url}/api/i18n/{ZHT}", timeout=30).read())
        diff_n = sum(1 for k, v in zh_a["strings"].items() if zh_b["strings"].get(k) != v)
        check("zh_simp 与 zh_trad 是两份不同的表（逐键比较有实质差异）",
              zh_a.get("lang") == ZHS and zh_b.get("lang") == ZHT and diff_n > 50,
              f"lang={zh_a.get('lang')}/{zh_b.get('lang')} 差异键数={diff_n}")
        # 旧代码兼容（B）：用户配置里写的还是 zh-CN/zh-TW/…，必须归一到新代码而不是掉到 en
        compat = {}
        for old, want in [("zh-CN", ZHS), ("zh_CN", ZHS), ("zh-Hans", ZHS), ("zh-SG", ZHS),
                          ("zh-TW", ZHT), ("zh_TW", ZHT), ("zh-Hant", ZHT), ("zh-HK", ZHT),
                          ("zh-MO", ZHT)]:
            try:
                d = json.loads(urllib.request.urlopen(f"{url}/api/i18n/{old}", timeout=30).read())
                got = d.get("lang")
            except Exception as e:  # noqa: BLE001
                got = f"异常 {e}"
            if got != want:
                compat[old] = f"{got}（应为 {want}）"
        check("旧中文代码（zh-CN/zh_TW/zh-Hans/zh-Hant/zh-HK/zh-MO/zh-SG）读取兼容到新代码",
              not compat, str(compat))
        unk = 200
        try:
            urllib.request.urlopen(url + "/api/i18n/klingon", timeout=10)
        except urllib.error.HTTPError as e:
            unk = e.code
        check("未知语言 404（不静默回退成 en）", unk == 404, f"HTTP {unk}")

        page.click('#tabs .tab[data-tab="config"]')
        page.wait_for_timeout(600)
        zh_tabs = page.eval_on_selector_all("#tabs .tab", "els => els.map(e => e.textContent.trim())")
        lang_opts = page.eval_on_selector_all("#lang-select option", "els => els.map(e => e.value)")
        check("左栏有语言下拉（「跟随系统」+ 十五种语言，值为新代码）",
              lang_opts[:1] == ["auto"] and set(langs) <= set(lang_opts)
              and "zh-CN" not in lang_opts and "zh-TW" not in lang_opts, str(lang_opts))
        page.select_option("#lang-select", "en")
        page.wait_for_timeout(1000)
        en_tabs = page.eval_on_selector_all("#tabs .tab", "els => els.map(e => e.textContent.trim())")
        en_cards = page.eval_on_selector_all("#config-form .card h3", "els => els.map(e => e.textContent.trim())")
        en_ui = page.evaluate("""() => ({
              lang: document.documentElement.lang, title: document.title,
              launch: document.getElementById('wp8f-launch').textContent.trim(),
              jsonToggle: document.getElementById('json-toggle').textContent.trim() })""")
        check("切到 en：导航页签变成英文（界面文本确实变化）",
              en_tabs == ["Configurator", "Flight model", "Replay", "Missile sim"], str(en_tabs))
        check("切到 en：配置器卡片标题跟着变（不是只换了下拉）",
              all(w in " ".join(en_cards) for w in ("Panels", "Warnings", "Layout", "Font", "Colors")),
              str(en_cards))
        check("切到 en：<html lang> / 窗口标题 / 底部按钮同步",
              en_ui["lang"] == "en" and "Console" in en_ui["title"]
              and en_ui["launch"] == "Start" and en_ui["jsonToggle"] == "Edit config JSON", str(en_ui))
        leftover = page.evaluate("""() => Array.from(document.querySelectorAll('[data-i18n]'))
              .map(e => e.textContent.trim()).filter(s => /^[a-z]+\\.[A-Za-z.]+$/.test(s))""")
        check("切语言后没有键名残留在界面上（漏翻会以键名上屏）", not leftover, str(leftover[:5]))
        en_w = page.evaluate("""() => {
              const l = document.querySelector('.card[data-card-id="layout"] .item > label');
              const f = document.querySelector('.card[data-card-id="font"] .item > label');
              return { layout: l ? Math.round(l.getBoundingClientRect().width) : -1,
                       font: f ? Math.round(f.getBoundingClientRect().width) : -1 }; }""")
        check("非中文界面下卡片定宽列仍生效（CSS 挂 data-card-id，不挂翻译后的标题）",
              en_w["layout"] == 100 and en_w["font"] == 75,
              f"en 下 布局/字体 标签列宽={en_w}（应为 8em/6em = 100/75px）")
        page.select_option("#lang-select", "zh_simp")
        page.wait_for_timeout(1000)
        back_tabs = page.eval_on_selector_all("#tabs .tab", "els => els.map(e => e.textContent.trim())")
        back_w = page.evaluate("""() => {
              const l = document.querySelector('.card[data-card-id="layout"] .item > label');
              return l ? Math.round(l.getBoundingClientRect().width) : -1; }""")
        check("切回 zh_simp：界面与列宽都恢复（可来回切）",
              back_tabs == zh_tabs and back_w == 75, f"{back_tabs} layout={back_w}px")

        # ---- 14) 字体：下拉只列 resource/fonts、写回完整相对路径、非法值明说会报错退出 ----
# 字体只由 config 的 `font_path` 决定，GUI 里没有"默认字体"概念：
        # 且值是**带 `resource/fonts/` 的完整相对路径**；后端 `builtin`/`is_default` 字段全部删掉。
        fonts = json.loads(urllib.request.urlopen(url + "/api/fonts", timeout=30).read())
        disk = sorted(p.name for p in (ROOT / "resource" / "fonts").iterdir()
                      if p.suffix.lower() in (".ttf", ".otf") and p.name.lower() != "icons.ttf")
        check("字体清单 = resource/fonts 下的 .ttf/.otf（排除图标字体 icons.ttf）",
              [f["name"] for f in fonts] == disk, f"{[f['name'] for f in fonts]} vs {disk}")
        check("响应里**没有**任何「默认/内置」合成条目与字段（builtin / is_default 都已删除）",
              all(("builtin" not in f and "is_default" not in f) for f in fonts), str(fonts)[:220])
        check("每一项都带 path = `resource/fonts/<文件名>`（前端直接拿它当 font_path 的值）",
              all(f.get("path") == f"resource/fonts/{f['name']}" for f in fonts),
              str(fonts)[:220])
        # 非等宽字体：探针临时放一个"文件名里没有 mono"的真字体（内容 = 默认那份，能解析）
        # —— 判据只看名字，所以这条同时验证"文件名判定 + mono=false"。
        # ⚠️ 名字里**任何位置**都不能出现 `mono`（"nonmono" 也含 mono，会判成等宽）；
        # 也不能撞上 `gui/src/api.rs` 的等宽白名单（consolas/sarasa/firacode/…）。
        probe_font = ROOT / "resource" / "fonts" / "probe-variable-pitch-test.ttf"
        shutil.copyfile(ROOT / "resource" / "fonts" / "SarasaMonoSC-Regular.ttf", probe_font)
        PROBE_FONTS.append(probe_font)
        fonts2 = json.loads(urllib.request.urlopen(url + "/api/fonts", timeout=30).read())
        nm = [f for f in fonts2 if f["name"] == probe_font.name]
        check("名字不含 mono 的字体被标成非等宽（mono=false）", bool(nm) and nm[0]["mono"] is False, str(nm))        # 界面：字体下拉（在「字体」卡片标题行，不额外占一行高度）
        page.reload(wait_until="networkidle")
        page.wait_for_selector("#config-form .card", timeout=15000)
        page.wait_for_timeout(700)
        fopts = page.evaluate("""() => Array.from(document.querySelectorAll('.card[data-card-id="font"] .h3-sel option'))
              .map(o => ({ v: o.value, t: o.textContent.trim(), mono: o.dataset.mono }))""")
        want_paths = {f"{f['path']}" for f in fonts2}
        check("字体下拉只列 resource/fonts 里的字体（值为 resource/fonts/<名字>，页面里没有 icons.ttf）",
              bool(fopts) and {o["v"] for o in fopts} == want_paths
              and not any("icons.ttf" in o["v"] for o in fopts), str(fopts)[:240])
        check("下拉里**没有**「默认/内置」这一项（占位项只在 font_path 无效时出现）",
              not any(o["v"] == "" for o in fopts)
              and not any(("默认" in o["t"]) or ("内置" in o["t"]) for o in fopts), str(fopts)[:240])
        check("下拉里每个字体都带 mono 标记（前端据此决定要不要警告）",
              all(o["mono"] in ("0", "1") for o in fopts), str(fopts))
        probe_path = f"resource/fonts/{probe_font.name}"
        page.select_option('.card[data-card-id="font"] .h3-sel', probe_path)
        page.wait_for_timeout(700)
        toast = page.evaluate("() => (document.getElementById('notify')||{}).innerText || ''")
        check("选非等宽字体 → 右下角弹警告（可能对不齐）",
              probe_path in toast, repr(toast[:140]))
        check("选字体写进配置的 font_path = **带 resource/fonts/ 的完整相对路径**",
              page.evaluate("() => JSON.parse(document.getElementById('config-json').value).font_path")
              == probe_path, page.evaluate("() => JSON.parse(document.getElementById('config-json').value).font_path"))
        # 非法字体：配置里写一个不存在的路径 → 界面上明说 **HUD 会因此报错退出**（不再"回退默认"）
        first_cfg = page.eval_on_selector("#config-select", "el => el.value")
        cfg_path = ROOT / "config" / first_cfg
        cfg_orig = cfg_path.read_bytes()
        cfg_now = json.loads(cfg_orig.decode("utf-8"))
        cfg_now["font_path"] = "resource/fonts/no_such_font_xyz.ttf"
        save_req = urllib.request.Request(
            url + "/api/config/save",
            data=json.dumps({"path": first_cfg,
                             "content": json.dumps(cfg_now, ensure_ascii=False, indent=2)}).encode(),
            method="POST", headers={"Content-Type": "application/json"})
        urllib.request.urlopen(save_req, timeout=30)
        page.reload(wait_until="networkidle")
        page.wait_for_selector("#config-form .card", timeout=15000)
        page.wait_for_timeout(900)
        toast2 = page.evaluate("() => (document.getElementById('notify')||{}).innerText || ''")
        check("font_path 指向不存在的文件 → 明说 HUD 会因此报错退出（不再说「回退默认字体」）",
              "no_such_font_xyz.ttf" in toast2 and ("报错退出" in toast2 or "退出" in toast2)
              and "回退" not in toast2, repr(toast2[:200]))
        bad_opts = page.evaluate("""() => Array.from(document.querySelectorAll('.card[data-card-id="font"] .h3-sel option'))
              .map(o => ({ v: o.value, t: o.textContent.trim(), sel: o.selected }))""")
        check("无效的 font_path 在下拉里以占位项显示（把问题摆在眼前，而不是静默选中别的字体）",
              bool(bad_opts) and bad_opts[0]["sel"] and "no_such_font_xyz.ttf" in bad_opts[0]["t"]
              and sum(1 for o in bad_opts if o["sel"]) == 1, str(bad_opts)[:240])
        # 逐字节还原这份配置（哪怕它不是 default.json —— finally 只兜 default.json）
        cfg_path.write_bytes(cfg_orig)

        # ---- 15) FM 页签文案已翻译（P6）：卡片随语言变、不残留 fm.* 键名 ----
# `fm-charts.js` 里可见文案全部走 t()；
        # 断言方式不查源码（读代码不算证据）：**真的切语言再看卡片上的字**。
        #
        # ⚠️ 等待预算只有 8s 且**失败即继续**（带真实 DOM / 接口响应进 detail）：
#   折叠的 <select> 里选项不可见 → 这里用 wait_for_function 等数量，不用 wait_for_selector。
        #   `<option>` 没有非空包围盒，Playwright 的 visible 判据永远不满足，于是 20s 超时把
        #   **整包探针挂死、连总结都没打印**。机型列表是异步拉的，这里只要"选项数 > 0"。
        page.click('#tabs .tab[data-tab="fm"]')
        fm_opts = 0
        try:
            page.wait_for_function(
                "() => document.querySelectorAll('#fm-aircraft option').length > 0", timeout=8000)
            fm_opts = page.eval_on_selector_all("#fm-aircraft option", "els => els.length")
        except Exception:  # noqa: BLE001 —— 超时就是失败，不往上抛
            fm_opts = page.eval_on_selector_all("#fm-aircraft option", "els => els.length")
        if not fm_opts:
            api_note = "?"
            try:
                api_note = str(urllib.request.urlopen(url + "/api/fm/aircraft", timeout=20).read()[:160])
            except Exception as e:  # noqa: BLE001
                api_note = f"接口也读不到：{e}"
            check("FM 机型下拉已填充（/api/fm/aircraft 有数据）", False,
                  f"option=0 · /api/fm/aircraft={api_note} · "
                  f"#fm-aircraft={page.eval_on_selector('#fm-aircraft', 'el => el.outerHTML')[:120]}")
        else:
            check("FM 机型下拉已填充（/api/fm/aircraft 有数据）", True, f"{fm_opts} 个机型")
        page.wait_for_timeout(900)
        fm_zh = page.inner_text("#fm-info")
        check("FM 机型信息卡片：默认（zh_simp）是中文原文",
              fm_opts > 0 and ("机型" in fm_zh) and ("重量" in fm_zh),
              fm_zh[:120].replace("\n", " / "))
        keyleft = re.findall(r"\bfm\.[a-z][A-Za-z0-9_.]*", page.inner_text("#tab-fm"))
        check("FM 页签没有残留的 fm.* 键名（漏翻会以键名上屏）", not keyleft, str(keyleft[:5]))

        page.click('#tabs .tab[data-tab="config"]')
        page.select_option("#lang-select", "en")
        page.wait_for_timeout(900)
        page.click('#tabs .tab[data-tab="fm"]')
        page.wait_for_timeout(900)
        fm_en = page.inner_text("#fm-info")
        cjk = re.findall(r"[\u3400-\u9fff\u3040-\u30ff\uac00-\ud7af]", fm_en)
        check("切到 en：FM 机型信息卡片整体变英文（卡片里一个中日韩字符都不剩）",
              fm_opts > 0 and ("Aircraft" in fm_en) and ("Weight" in fm_en) and not cjk,
              f"{fm_en[:120].replace(chr(10), ' / ')} · 残留={cjk[:8]}")
        fm_update_en = page.inner_text("#fm-update")
        if "更 新 到" in fm_update_en or "飞行模型数据库更新" in fm_update_en:
            # 面板文案是 JS 生成的：切语言后由 fmOnLangChange() 立即重刷；
            # 兜底再等一次轮询（飞行模型页签可见时 1.5s 一次），仍不改就是真漏翻。
            try:
                page.wait_for_function(
                    "() => !((document.getElementById('fm-update')||{}).innerText||'')"
                    ".includes('飞行模型数据库更新')", timeout=4000)
            except Exception:  # noqa: BLE001
                pass
            fm_update_en = page.inner_text("#fm-update")
        check("切到 en：更新面板文案（含新按钮）跟着变",
              "UPDATE" in fm_update_en.upper(),
              fm_update_en[:100].replace("\n", " / "))
        page.click('#tabs .tab[data-tab="config"]')
        page.select_option("#lang-select", "zh_simp")
        page.wait_for_timeout(900)

# ---- 15b) 活塞机型：三张推力图连标题一起隐藏 ----
        # 顺带验「引擎」那一格：i-16_type24 上游没有 `EngineType0.Main.Type` 键，
        # 老逻辑显示"未知"（用户实测报告）→ 现在必须靠 Power 键兜底成「活塞」。
        FMJ = '[data-fm-jet-only]'
        jet_only_n = page.eval_on_selector_all(FMJ, "els => els.length")
        check("被标记为「仅喷气」的图块 = 3 张图 ×（标题 + 容器）= 6 个元素",
              jet_only_n == 6, f"{jet_only_n} 个")

        def fm_visibility() -> dict:
            return page.evaluate("""() => {
              const rows = Array.from(document.querySelectorAll('[data-fm-jet-only]')).map(e => ({
                id: e.id || (e.getAttribute('data-i18n') || ''),
                disp: getComputedStyle(e).display,
                shown: e.getClientRects().length > 0,
              }));
              const kv = Array.from(document.querySelectorAll('#fm-info .fm-kv'))
                .find(r => (r.querySelector('span') || {}).textContent === '引擎');
              return { rows, engine: kv ? (kv.querySelector('b') || {}).textContent : null,
                       info: (document.getElementById('fm-info') || {}).innerText || '' };
            }""")

        page.click('#tabs .tab[data-tab="fm"]')
        page.select_option("#fm-aircraft", "i-16_type24")
        page.wait_for_timeout(2200)
        piston = fm_visibility()
        check("活塞机型（i-16_type24）：推力-真空速 / 推力-表速 / 推力 3D 曲面全部隐藏（连标题）",
              len(piston["rows"]) == 6
              and all(r["disp"] == "none" and not r["shown"] for r in piston["rows"]),
              str(piston["rows"]))
        check("活塞机型的「引擎」显示为活塞（缺 EngineType0.Main.Type 键 → 默认活塞，不再显示未知）",
              (piston["engine"] or "").strip() in ("活塞", "Piston"),
              f"engine={piston['engine']!r}")
        check("活塞机型保留的三张图仍在：CL-α / 真空速-高度 / 活塞功率-高度",
              all(page.eval_on_selector(sel, "e => e.getClientRects().length > 0")
                  for sel in ("#fm-chart-cl", "#fm-chart-tasalt", "#fm-chart-power")),
              str(page.eval_on_selector_all(".chart", "els => els.map(e => [e.id, e.getClientRects().length > 0])")))

        page.select_option("#fm-aircraft", "f_16c_block_50")
        page.wait_for_timeout(2200)
        jet = fm_visibility()
        check("切回喷气机型（f_16c_block_50）：三张推力图恢复显示",
              len(jet["rows"]) == 6
              and all(r["disp"] != "none" and r["shown"] for r in jet["rows"])
              and (jet["engine"] or "").strip() in ("喷气", "Jet"),
              f"engine={jet['engine']!r} rows={jet['rows']}")

# ---- 16) 更新器：没有「停止 HUD」、按下载完成启用、HUD 在跑就拒绝 ----
        page.click('#tabs .tab[data-tab="fm"]')
        page.wait_for_timeout(700)
        check("『停止 HUD』按钮已删除（不再有「停止 HUD」这个动作）", page.query_selector("#up-stop") is None,
              str(page.eval_on_selector_all("#fm-update .up-btns button",
                                            "els => els.map(e => e.id)")))
        apply_text = page.inner_text("#up-apply").replace(" ", "").replace("\u3000", "")
        check("按钮文案改成「更新到下载新版本」", apply_text == "更新到下载新版本", apply_text)
        check("下载未完成（staging_ready=false）→ 「更新到下载新版本」禁用",
              page.is_disabled("#up-apply"))
        check("『清除下载』按钮在场（取消要能清 data_new 与临时目录）",
              page.query_selector("#up-clear") is not None)

        # 「停止 HUD」这条路由必须一起删掉：命中兜底分支（未知动作），不能真的去杀进程
        stop_r = post_json(url + "/api/update/stop-hud", {})
        check("『停止 HUD』接口已删除（未知动作，不再结束任何进程）",
              stop_r.get("ok") is False and "killed" not in stop_r, str(stop_r)[:160])
        # 上一版数据是**固定单槽**：状态里只有"有没有 / 在哪"两个字段，
        # ⚠️ `/api/update/status` 的响应是 `{"ok":true,"status":{…}}`（**包了一层**，
        # 与 update_check.py 的 api(...)["status"] 同一口径；
        # 直接读 `["status"]` 之外的那层会让断言永远读到 None（恒红）。
        up_st = json.loads(urllib.request.urlopen(url + "/api/update/status", timeout=30).read())["status"]
        check("更新状态里的上一版信息 = 单槽 data_old（old_exists / old_root 两个字段）",
              isinstance(up_st.get("old_exists"), bool)
              and str(up_st.get("old_root", "")).endswith("data_old")
              and "old_roots" not in up_st,
              f"old_exists={up_st.get('old_exists')!r} old_root={up_st.get('old_root')!r} "
              f"old_roots={'old_roots' in up_st}")
        # 没就绪时拒绝更新：业务失败（HTTP 200 + ok:false），不是 HTTP 错误
        apply_r = post_json(url + "/api/update/apply", {})
        check("未就绪时 /api/update/apply 拒绝更新（业务失败，数据一个字节不动）",
              apply_r.get("ok") is False and (apply_r.get("error") or {}).get("kind") in
              ("verify", "io", "hud_running"), str(apply_r.get("error"))[:160])

        # 按钮的"启用/禁用"由状态驱动：直接喂两份状态给前端渲染函数（不依赖真的下载 361 MB）
        page.evaluate("""() => upRender({
              phase: 'ready', busy: false, staging_ready: true, hud_running: false,
              staging_version: '9.9.9.9', remote_version: '9.9.9.9', local_version: '1.0.0',
              update_available: true, json_supported: true, dropped_top_level: [],
              old_exists: false, old_root: 'resource/data_old',
              manifest: {note: '', authoritative: true}, log: [],
              progress: {files_total: 3, files_done: 3, bytes_done_h: '1 B', bytes_total_h: '1 B', retries: 0, files_failed: 0},
            });""")
        page.wait_for_timeout(200)
        check("下载完成（staging_ready=true）→ 「更新到下载新版本」可选",
              not page.is_disabled("#up-apply"),
              page.inner_text("#up-msg")[:80])
        page.evaluate("""() => upRender({
              phase: 'ready', busy: false, staging_ready: true, hud_running: true,
              staging_version: '9.9.9.9', remote_version: '9.9.9.9', local_version: '1.0.0',
              update_available: true, json_supported: true, dropped_top_level: [],
              old_exists: false, old_root: 'resource/data_old',
              manifest: {note: '', authoritative: true}, log: [],
              progress: {files_total: 3, files_done: 3, bytes_done_h: '1 B', bytes_total_h: '1 B', retries: 0, files_failed: 0},
            });""")
        page.wait_for_timeout(200)
        hud_msg = page.inner_text("#up-msg")
        check("HUD 在跑 → 更新按钮禁用 + 面板说明原因（请先关闭 wp8f.exe）",
              page.is_disabled("#up-apply") and ("HUD" in hud_msg) and ("关闭" in hud_msg),
              hud_msg[:120])

        # ---- 17) 回放页签文案已翻译（P8）：可见文本随语言变、不残留 rp.* 键名 ----
# `replay.js` 里由 JS 生成的文案：遥测键名 / 图例 / 底面提示 / 转换器 / 播控，
        # 下拉状态 / 图表轴名 / 空态 / 错误 / toast / title）全部走 t()。
        # 断言方式不查源码：**真的加载一份 .wpr，切到 en 再看页签上的字**。
        # 覆盖范围：`#tab-replay` 的可见文本（遥测面板、底面两行提示、对象统计行、播放按钮、
        # 下拉与文件标签）＋ 3D 图的坐标轴名（走 ECharts option，不是 DOM）。
        page.click('#tabs .tab[data-tab="replay"]')
        page.wait_for_timeout(600)
        page.click("#rp-clear")
        page.select_option("#rp-add", FIXTURE_MAP)
        page.wait_for_timeout(2200)          # 底图异步加载 + 遥测/底面提示写入
        rp_read = """() => {
          const inst = window.echarts ? echarts.getInstanceByDom(document.getElementById('rp-chart')) : null;
          const opt = (inst && !inst.isDisposed()) ? inst.getOption() : null;
          const ax = (opt && opt.xAxis3D && opt.xAxis3D[0]) ? opt.xAxis3D[0].name : '';
          return {
            text: (document.getElementById('tab-replay') || {}).innerText || '',
            tel: (document.getElementById('rp-telemetry') || {}).innerText || '',
            resNote: (document.getElementById('rp-ground-res-note') || {}).textContent || '',
            objects: (document.getElementById('rp-objects') || {}).textContent || '',
            play: (document.getElementById('rp-play') || {}).textContent || '',
            axis: ax || '',
          };
        }"""
        rp_zh = page.evaluate(rp_read)
        check("回放页签 zh_simp：遥测键名 / 底面提示 / 轴名都是中文原文",
              ("速度" in rp_zh["tel"]) and ("原图纹理" in rp_zh["resNote"])
              and ("东西" in rp_zh["axis"]) and ("个文件" in rp_zh["objects"]),
              f"tel={rp_zh['tel'][:60]!r} note={rp_zh['resNote'][:60]!r} axis={rp_zh['axis']!r}")
        page.click('#tabs .tab[data-tab="config"]')
        page.select_option("#lang-select", "en")
        page.wait_for_timeout(1000)
        page.click('#tabs .tab[data-tab="replay"]')
        page.wait_for_timeout(1200)
        rp_en = page.evaluate(rp_read)
        cjk = re.findall(r"[\u3400-\u9fff\u3040-\u30ff\uac00-\ud7af]", rp_en["text"])
        check("切到 en：回放页签可见文本里没有中日韩字符残留",
              not cjk, f"残留={cjk[:10]} · {rp_en['text'][:160]!r}")
        cjk_axis = re.findall(r"[\u3400-\u9fff\u3040-\u30ff\uac00-\ud7af]", rp_en["axis"])
        check("切到 en：3D 图坐标轴名也跟着变（不是只换了 DOM）",
              bool(rp_en["axis"]) and not cjk_axis, f"axis={rp_en['axis']!r}")
        # 键名残留沿用既有的"漏翻会以键名上屏"思路（复用 DEV_TOKEN_RE 的写法）
        keyleft = re.findall(r"\brp\.[a-z][A-Za-z0-9_.]*", rp_en["text"])
        check("切到 en：回放页签没有 rp.* 键名残留（漏翻会以键名上屏）",
              not keyleft, str(keyleft[:5]))
        check("切到 en：对象统计行/播放按钮也按新语言重写",
              ("files" in rp_en["objects"] and "objects" in rp_en["objects"])
              and rp_en["play"].strip().upper().startswith("P"),
              f"objects={rp_en['objects']!r} play={rp_en['play']!r}")
        page.click('#tabs .tab[data-tab="config"]')
        page.select_option("#lang-select", "zh_simp")
        page.wait_for_timeout(900)
        page.click('#tabs .tab[data-tab="replay"]')
        page.wait_for_timeout(700)
        rp_back = page.evaluate(rp_read)
        check("切回 zh_simp：回放页签恢复中文（可来回切）",
              ("速度" in rp_back["tel"]) and ("个文件" in rp_back["objects"]),
              f"tel={rp_back['tel'][:40]!r} objects={rp_back['objects']!r}")
        page.click("#rp-clear")               # 收尾：把回放状态清掉，别留给后面的收尾逻辑

        # ---- 18) 配置器：记住上次选中的配置文件（P9 用户要求）----
# 选了 mylayout.json，下次打开还停在它上面；那份配置被删/改名后回退 default.json 并清键。
        # 回退 default.json **并清掉记住的值**（否则每次启动都白走一遍回退）。
        # 只记**文件名**，配置内容仍走自动保存那条路。
        LAST_CFG_NAME = "ui_check_lastcfg.json"
        LAST_CFG = ROOT / "config" / LAST_CFG_NAME
        LAST_CFG_KEY = "wp8f.lastConfig"
        page.click('#tabs .tab[data-tab="config"]')
        page.wait_for_timeout(400)
        # 先清干净（失败路径也在 finally 里兜底删文件 + 清键）
        page.evaluate(f"() => localStorage.removeItem({LAST_CFG_KEY!r})")
        if LAST_CFG.exists():
            LAST_CFG.unlink()
        base_cfg = json.loads((ROOT / "config" / "default.json").read_text(encoding="utf-8"))
        save_req = urllib.request.Request(
            url + "/api/config/save",
            data=json.dumps({"path": LAST_CFG_NAME,
                             "content": json.dumps(base_cfg, ensure_ascii=False, indent=2)}).encode(),
            method="POST", headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(save_req, timeout=30) as r:
            check("探针能新建一份配置（/api/config/save）", r.status == 200 and LAST_CFG.is_file(),
                  f"status={r.status} exists={LAST_CFG.is_file()}")
        page.reload(wait_until="networkidle")
        page.wait_for_selector("#config-form .card", timeout=15000)
        page.wait_for_timeout(700)
        names_now = page.eval_on_selector_all("#config-select option", "els => els.map(e => e.value)")
        check("新建的配置出现在下拉里", LAST_CFG_NAME in names_now, str(names_now))
        page.select_option("#config-select", LAST_CFG_NAME)
        page.wait_for_timeout(900)
        stored = page.evaluate(f"() => localStorage.getItem({LAST_CFG_KEY!r})")
        check("在下拉里切换配置 → 记住这个文件名（localStorage）",
              stored == LAST_CFG_NAME, f"stored={stored!r}")
        # 刷新：仍停在这份配置上
        page.reload(wait_until="networkidle")
        page.wait_for_selector("#config-form .card", timeout=15000)
        page.wait_for_timeout(900)
        after = page.evaluate("""() => ({
            sel: document.getElementById('config-select').value,
            cur: (document.getElementById('config-current') || {}).textContent || '',
            stored: localStorage.getItem('wp8f.lastConfig'),
        })""")
        check("刷新后仍停在记住的那份配置（不是回退到 default.json）",
              after["sel"] == LAST_CFG_NAME and after["stored"] == LAST_CFG_NAME
              and LAST_CFG_NAME in after["cur"], str(after))
        # 从磁盘删掉它 → 刷新应回退 default.json，且**清掉**记住的值
        LAST_CFG.unlink()
        page.reload(wait_until="networkidle")
        page.wait_for_selector("#config-form .card", timeout=15000)
        page.wait_for_timeout(900)
        gone = page.evaluate("""() => ({
            sel: document.getElementById('config-select').value,
            cur: (document.getElementById('config-current') || {}).textContent || '',
            stored: localStorage.getItem('wp8f.lastConfig'),
        })""")
        check("记住的配置已不存在 → 回退 default.json 并清掉记住的值",
              gone["sel"] == "default.json" and gone["stored"] is None
              and "default.json" in gone["cur"], str(gone))
        check("回退后不再写回记住的值（避免每次启动都走一遍回退）",
              page.evaluate(f"() => localStorage.getItem({LAST_CFG_KEY!r})") is None, str(gone))

        # ---- 19) HUD 语言文件（C）+ 语音包（D）：取值来源、控件位置与**写回** ----
        # 断言口径（用户要求）：① HUD 语言**只有一个键** `hud_lang_path`（路径），
# 旧键 hud_language/lang_json 不许再出现；② 控件在**「面板」卡片**里；
        # ③ 候选 = `resource/lang/` 下真实存在的文件；④ 语音包下拉列 `resource/voice/` 的子目录。
        # ⚠️ 还额外钉一条**独立性**：改 HUD 语言文件不许动 GUI 自己的语言（两者互不影响）。
        hud = json.loads(urllib.request.urlopen(url + "/api/hud-lang", timeout=30).read())
        hf = hud.get("files", [])
        disk_langs = sorted(p.name for p in (ROOT / "resource" / "lang").glob("*.json"))
        check("/api/hud-lang：列出的就是 resource/lang/ 下真实存在的 *.json（含用户自建的）",
              [x.get("file") for x in hf] == disk_langs, str(hf)[:220])
        check("每项都带 path（= resource/lang/<文件名>，直接当 hud_lang_path 的值）与 name（文件里的 _name）",
              all(x.get("path") == f"resource/lang/{x['file']}" and x.get("name")
                  and (ROOT / x["path"]).is_file() for x in hf), str(hf)[:220])
        check("清单里不再有 code/exists/default 那套「中文/英文二选一」的字段",
              all("code" not in x and "exists" not in x for x in hf)
              and "languages" not in hud and "default" not in hud, str(hud)[:220])
        vp = json.loads(urllib.request.urlopen(url + "/api/voice-packs", timeout=30).read())
        disk_packs = sorted(p.name for p in (ROOT / "resource" / "voice").iterdir() if p.is_dir())
        check("/api/voice-packs：列出的就是 resource/voice/ 下的子目录（每个 = 一个语音包）",
              sorted(x["name"] for x in vp.get("packs", [])) == disk_packs,
              f"{[x['name'] for x in vp.get('packs', [])]} vs {disk_packs}")
        DEFAULT_PACK = "zh_xiaoxuan_calm_youngadultfemale_1_04"
        pack0 = [x for x in vp.get("packs", []) if x["name"] == DEFAULT_PACK]
        check(f"默认语音包 {DEFAULT_PACK} 在清单里，且带 path/wavs/bytes（>0）",
              bool(pack0) and pack0[0]["path"] == f"resource/voice/{DEFAULT_PACK}"
              and pack0[0]["wavs"] > 0 and pack0[0]["bytes"] > 0, str(pack0))
        check("语音包 wav 数与磁盘一致（D：整个包会被预载进内存）",
              bool(pack0) and pack0[0]["wavs"]
              == len(list((ROOT / "resource" / "voice" / DEFAULT_PACK).glob("*.wav"))),
              f"{pack0[0]['wavs'] if pack0 else '?'}")
        check("resource/voice/ 根目录上没有散落的 wav（D 起语音按包放，散着不会被加载）",
              vp.get("loose") == [], str(vp.get("loose")))
# 界面：语言下拉在「面板」卡片里（不在「告警」），语音包下拉在「告警」里；
        # 写回的是**配置键**（不是别的），而且配置里不该再有 hud_language/lang_json
        page.click('#tabs .tab[data-tab="config"]')
        page.wait_for_timeout(500)
        dom = page.evaluate("""() => {
              const panel = document.querySelector('.card[data-card-id="panel"]');
              const alert = document.querySelector('.card[data-card-id="alert"]');
              const lp = panel && panel.querySelector('select[data-key="hud_lang_path"]');
              const vp = alert && alert.querySelector('select[data-key="voice_path"]');
              const cfgNow = JSON.parse(document.getElementById('config-json').value);
              return {
                hasLangPath: !!lp, inAlert: !!(alert && alert.querySelector('[data-key="hud_lang_path"]')),
                langVals: lp ? Array.from(lp.options).map(o => o.value) : [],
                langTexts: lp ? Array.from(lp.options).map(o => o.textContent.trim()) : [],
                langSel: lp ? lp.value : null,
                hasDatalist: !!document.getElementById('hud-lang-files'),
                hasTextInput: !!(panel && panel.querySelector('input[data-key="hud_lang_path"]')),
                hasVoice: !!vp, voiceOpts: vp ? Array.from(vp.options).map(o => o.value) : [],
                oldKeys: ['hud_language', 'lang_json'].filter(k => k in cfgNow),
                hasLangSel: !!document.querySelector('select[data-key="hud_language"]'),
              }; }""")
        check("「面板」卡片里有 HUD 语言下拉，且**不在**「告警」卡片里",
              dom["hasLangPath"] and not dom["inAlert"], str(dom)[:260])
        check("下拉的**值**是完整相对路径（resource/lang/<文件名>）",
              sorted(dom["langVals"]) == sorted(f"resource/lang/{n}" for n in disk_langs),
              str(dom["langVals"])[:220])
        check("下拉的**显示**只有文件名主干（`zh`/`en`：不带路径、不带 .json、不带括号注释）",
              sorted(dom["langTexts"]) == sorted(p.stem for p in
                      (ROOT / "resource" / "lang").glob("*.json"))
              and all("/" not in tx and ".json" not in tx and "（" not in tx
                      for tx in dom["langTexts"]),
              str(dom["langTexts"]))
        check("已经换成下拉（旧的输入框 + datalist 都删了）",
              not dom["hasTextInput"] and not dom["hasDatalist"], str(dom)[:260])
        check("配置里已经没有 hud_language / lang_json（只有一个 hud_lang_path）",
              dom["oldKeys"] == [] and not dom["hasLangSel"], str(dom)[:260])
        check("下拉当前选中项 = 配置里的那份语言文件",
              dom["langSel"] == (json.loads(
                  page.evaluate("() => document.getElementById('config-json').value"))["hud_lang_path"]),
              str(dom["langSel"]))
        check("「告警」卡片里有语音包下拉，选项值 = resource/voice/<子目录名>",
              dom["hasVoice"] and set(dom["voiceOpts"]) == {f"resource/voice/{n}" for n in disk_packs},
              str(dom["voiceOpts"])[:220])
        # 写回：切语言文件 → `hud_lang_path`（写完整路径）；选语音包 → `voice_path`
        gui_lang_before = page.evaluate("() => document.documentElement.lang")
        lang_before = page.evaluate(
            "() => JSON.parse(document.getElementById('config-json').value).hud_lang_path")
        # 语音包下拉里挑一个**与当前不同的**值（只有默认包时就是它自己 → 仍要写回）
        target_pack = f"resource/voice/{disk_packs[-1]}" if disk_packs else ""
        if target_pack:
            page.select_option('.card[data-card-id="alert"] select[data-key="voice_path"]', target_pack)
            page.wait_for_timeout(800)
        page.select_option('.card[data-card-id="panel"] select[data-key="hud_lang_path"]',
                           "resource/lang/en.json")
        page.wait_for_timeout(900)
        back = page.evaluate("""() => { const c = JSON.parse(document.getElementById('config-json').value);
              return { lang: c.hud_lang_path, voice: c.voice_path, gui: document.documentElement.lang,
                       old: ['hud_language', 'lang_json'].filter(k => k in c) }; }""")
        check("选语言文件 → 写回配置键 `hud_lang_path`（**完整路径**，不是文件名）",
              back["lang"] == "resource/lang/en.json", str(back))
        check("写回后配置里仍然没有 hud_language / lang_json（旧键不会被回收进来）",
              back["old"] == [], str(back))
        check("选语音包 → 写回配置键 `voice_path`（resource/voice/<子目录名>）",
              bool(target_pack) and back["voice"] == target_pack, str(back))
        check("改 HUD 语言文件**不影响 GUI 自己的语言**（两个设置互不影响）",
              back["gui"] == gui_lang_before, f"{gui_lang_before} → {back['gui']}")
        # 还原：把两个键写回探针开始前的值（配置在 finally 里还会整份字节还原）
        page.select_option('.card[data-card-id="panel"] select[data-key="hud_lang_path"]',
                           lang_before)
        if disk_packs:
            page.select_option('.card[data-card-id="alert"] select[data-key="voice_path"]',
                               f"resource/voice/{DEFAULT_PACK}")
        page.wait_for_timeout(900)

        browser.close()
    finally:
        pw.stop()
finally:
    proc.kill()
    # 兜底：用例改过配置（font_size），恢复原文件，避免污染用户配置
    if CFG_BACKUP is not None:
        try:
            CFG.write_bytes(CFG_BACKUP)
        except Exception as e:
            print("恢复配置失败:", e)
    # 兜底（失败路径也走到这里）：删掉本用例上传进 logs/ 的样本，
    # 否则它们会一直出现在 GUI「录制文件」下拉里
    for _p in UPLOADS:
        try:
            _p.unlink(missing_ok=True)
        except OSError as e:
            print("删除上传样本失败:", _p.name, e)
    # 兜底：删掉本用例临时放进 resource/fonts/ 的探针字体（那是源目录，绝不能被污染）
    for _p in PROBE_FONTS:
        try:
            _p.unlink(missing_ok=True)
        except OSError as e:
            print("删除探针字体失败:", _p.name, e)
    # 兜底：第 18 组临时新建的配置文件（新建 → 选中 → 删掉；失败路径也可能留一份）
    for _n in ("ui_check_lastcfg.json",):
        try:
            (ROOT / "config" / _n).unlink(missing_ok=True)
        except OSError as e:
            print("删除探针配置失败:", _n, e)

passed = sum(1 for _n, ok, _d in RESULTS if ok)
all_ok = bool(RESULTS) and passed == len(RESULTS)
if all_ok:
    shutil.rmtree(PROBE_DIR, ignore_errors=True)   # 全绿：截图不留
    try:
        PROBE_DIR.parent.rmdir()                   # logs/_probe 空了也一并收掉
    except OSError:
        pass
elif PROBE_DIR.is_dir():
    print(f"\n有失败项，截图保留在 {PROBE_DIR.relative_to(ROOT)}（排查完可整个删掉）")
print(f"\n===== {passed}/{len(RESULTS)} 通过 =====")
sys.exit(0 if all_ok else 1)
