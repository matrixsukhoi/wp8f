#!/usr/bin/env python3
"""查控制台主窗口的实际像素尺寸与屏幕比例（验证 16:9）。

窗口标题自 P5（i18n 接入）起来自 `resource/i18n/<lang>.json` 的 `gui.window.title`
（`gui/src/main.rs` 用 `i18n::t(...)` 设的），**不再有硬编码中文** —— 所以这里按当前生效
语言取期望标题：语言解析与 `gui/src/i18n.rs` 一致（config 的 `language` 键；`auto`/空 =
跟随系统；config 里没有这个键时用默认 zh_simp）。

⚠️ 中文两份代码自 B 起是 **`zh_simp` / `zh_trad`**（旧写法 `zh-CN`/`zh-TW`/`zh-Hans`/
`zh-Hant`/`zh_TW`/`zh-HK` 仍被接受，见 `gui/src/i18n.rs::LEGACY_ALIASES`）——
下面的别名表与 Rust 侧**同口径**，改一处要改两处。
"""
import ctypes
import json
import sys
from ctypes import wintypes
from pathlib import Path

# 输出重定向到文件时 Windows 默认 GBK，打印 ✓/✕ 会 UnicodeEncodeError 直接崩；
# 统一强制 UTF-8，让 CI/重定向场景与真实控制台行为一致。
sys.stdout.reconfigure(encoding="utf-8", errors="replace")

ROOT = Path(__file__).resolve().parent.parent.parent

# ---- 期望窗口标题：与 gui/src/i18n.rs 同一套语言解析 ----
SUPPORTED = ["zh_simp", "zh_trad", "en", "ru", "es", "it", "fr", "ja", "el", "tr"]
DEFAULT_LANG = "zh_simp"        # 配置里没写 language 时用的语言（i18n.rs 的 DEFAULT_LANG）
# 旧代码 → 新代码（同 i18n.rs::LEGACY_ALIASES；表里的写法已小写、`_` 已换成 `-`）
LEGACY_ALIASES = {"zh-cn": "zh_simp", "zh-hans": "zh_simp", "zh-hans-cn": "zh_simp",
                  "zh-sg": "zh_simp", "zh-tw": "zh_trad", "zh-hant": "zh_trad",
                  "zh-hant-tw": "zh_trad", "zh-hk": "zh_trad", "zh-mo": "zh_trad"}
# GetUserDefaultUILanguage 的 LANGID 主语言 ID → code（同 i18n.rs::system_lang）
PRIMARY = {0x04: "zh_simp", 0x08: "el", 0x09: "en", 0x0A: "es", 0x0C: "fr",
           0x10: "it", 0x11: "ja", 0x19: "ru", 0x1F: "tr"}
# 完整 LANGID → 中文简繁（同 i18n.rs::system_lang 的 0x0404/0x0C04/0x1404 判定）
ZH_LANGIDS = {0x0404: "zh_trad", 0x0C04: "zh_trad", 0x1404: "zh_trad"}


def normalize(tag: str):
    """语言标签归一（同 i18n.rs::normalize）：en-US→en、zh_TW/zh-TW→zh_trad、未知→None"""
    t = tag.strip().replace("_", "-")
    if not t:
        return None
    low = t.lower()
    for code in SUPPORTED:
        if low == code.replace("_", "-"):
            return code
    if low in LEGACY_ALIASES:
        return LEGACY_ALIASES[low]
    primary = low.split("-")[0]
    if primary == "zh":
        trad = ("hant" in low) or low.endswith("-tw") or low.endswith("-hk") or low.endswith("-mo")
        return "zh_trad" if trad else "zh_simp"
    for code in SUPPORTED:
        if code.split("-")[0] == primary:
            return code
    return None


def system_lang() -> str:
    try:
        k32 = ctypes.WinDLL("kernel32")
        langid = int(k32.GetUserDefaultUILanguage())
        if langid & 0x03FF == 0x04:
            return ZH_LANGIDS.get(langid, "zh_simp")
        return PRIMARY.get(langid & 0x03FF, "en")
    except Exception:  # noqa: BLE001
        return "en"


def effective_lang() -> str:
    """排序后第一个带 `language` 键的 config/*.json（同 i18n.rs::resolve_from_config）"""
    for f in sorted((ROOT / "config").glob("*.json")):
        try:
            v = json.loads(f.read_text(encoding="utf-8"))
        except Exception:  # noqa: BLE001
            continue
        if "language" not in v:
            continue
        raw = str(v["language"]).strip()
        if not raw or raw.lower() == "auto":
            return system_lang()
        return normalize(raw) or "en"
    return DEFAULT_LANG


def expected_title() -> str:
    """当前语言的 `gui.window.title`（读不到就退 en，再读不到就用 zh_simp 原文）"""
    code = effective_lang()
    for lang in (code, "en"):
        try:
            d = json.loads((ROOT / "resource" / "i18n" / f"{lang}.json").read_text(encoding="utf-8"))
        except Exception:  # noqa: BLE001
            continue
        if d.get("gui.window.title"):
            return d["gui.window.title"]
    return "wp8f 控制台"


TITLE = expected_title()
print(f"config 语言={effective_lang()} → 期望窗口标题「{TITLE}」")

u32 = ctypes.WinDLL("user32", use_last_error=True)
u32.FindWindowW.restype = wintypes.HWND
u32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
u32.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
u32.IsWindowVisible.argtypes = [wintypes.HWND]
u32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]

EnumProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
found = []


def cb(hwnd, _lparam):
    buf = ctypes.create_unicode_buffer(512)
    u32.GetWindowTextW(hwnd, buf, 512)
    if buf.value.strip() == TITLE:
        r = wintypes.RECT()
        u32.GetWindowRect(hwnd, ctypes.byref(r))
        c = wintypes.RECT()
        u32.GetClientRect(hwnd, ctypes.byref(c))
        found.append({
            "hwnd": hwnd,
            "visible": bool(u32.IsWindowVisible(hwnd)),
            "outer": (r.right - r.left, r.bottom - r.top, r.left, r.top),
            "client": (c.right - c.left, c.bottom - c.top),
        })
    return True


u32.EnumWindows(EnumProc(cb), 0)
if not found:
    print(f"没找到标题为「{TITLE}」的窗口（控制台没在跑？）")
    raise SystemExit(2)

main = max(found, key=lambda f: f["outer"][0] * f["outer"][1])
ow, oh, x, y = main["outer"]
cw, ch = main["client"]
print(f"同名窗口数: {len(found)}（其中最大的是主窗口）")
print(f"外框:   {ow}x{oh}  比例={ow/oh:.3f}  位置=({x},{y})  可见={main['visible']}")
if ch:
    print(f"客户区: {cw}x{ch}  比例={cw/ch:.3f}")
print(f"16:9 = {16/9:.3f}  |  16:10 = {1.6:.3f}")

# ---- 断言（只打印会让"比例不对"也一样绿）----
results = []


def check(name: str, ok: bool, detail: str = "") -> None:
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" · {detail}" if detail else ""), flush=True)


TARGET = 16 / 9
check("找到主窗口且外框宽高 > 0", ow > 0 and oh > 0, f"外框 {ow}x{oh}")
check("客户区宽高 > 0", cw > 0 and ch > 0, f"客户区 {cw}x{ch}")
# 16:9 约束的是**客户区**：gui/src/main.rs 用 with_inner_size(16:9) 设的就是它，
# 外框还含标题栏和边框，比例天然偏大，只作参考不计入判定。
ratio = (cw / ch) if ch else 0.0
dev = abs(ratio - TARGET) / TARGET if ratio else 1.0
check("客户区比例 16:9（偏差 ≤5%）", dev <= 0.05,
      f"实测 {ratio:.3f}（16:9 = {TARGET:.3f}，偏差 {dev * 100:.1f}%）"
      f"；若你手动拖过窗口大小，这一项会失败")

raise SystemExit(0 if results and all(results) else 1)
