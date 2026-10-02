#!/usr/bin/env python3
"""API 一致性采集/比对：同一组请求分别打到 Python 版与 Rust 版 webui，做结构比对。

用法：
    python scripts/tests/api_parity.py capture --url http://127.0.0.1:8899 --out logs/golden_python
    python scripts/tests/api_parity.py capture --url http://127.0.0.1:8899 --out logs/golden_rust
    python scripts/tests/api_parity.py compare logs/golden_python logs/golden_rust

比对规则：忽略易变字段（pid / 时间 / mtime / 绝对路径里的用户名等），
其余字段名、类型、数组长度、数值必须一致（数值允许极小浮点误差）。

退出码可信度约定：`capture` 会先清空 `--out` 目录（残留的上轮样本会被当成本轮结果），
并返回**采集失败条数**；`compare` 里"一侧样本缺失"算失败而不是跳过。
"""
import argparse
import json
import re
import shutil
import sys
import tempfile
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

# 采样的端点（只取 GET，避免副作用：不启动 wp8f）
ENDPOINTS = [
    ("health", "/api/health"),
    ("system", "/api/system"),
    ("resident", "/api/resident/status"),
    ("config_list", "/api/config/list"),
    ("config_default", "/api/config/default.json"),
    ("fm_aircraft", "/api/fm/aircraft"),
    ("fm_f16", "/api/fm/f_16c_block_50?fuel_pct=50&extra_weight=0"),
    ("fm_f16_curves", "/api/fm/f_16c_block_50/curves?fuel_pct=50&extra_weight=0"),
    ("fm_missing", "/api/fm/__no_such_aircraft__"),
    ("replay_list", "/api/replay/list"),
]

VOLATILE_KEYS = {
    "pid", "children", "job_assigned", "root", "config_dir", "logs_dir", "fm_dir",
    "resolved_at", "window_pid", "wp8f_pids", "test_server_pid", "mtime", "size",
    "resident_status", "window_error", "tray_error", "tray_available", "resident",
    "wp8f_exe", "wp8f_exe_exists", "fm_dir_exists", "wp8f_running", "platform",
    "job_object", "job_error", "window_exists", "window_visible", "window_url",
    "close", "reason", "session_id",
    # 单进程语义：控制台永远自带托盘/子进程监管（旧 Python 版在开发模式下是 false）
    "available", "error",
}

# 只比"形状"不比具体值的端点（内容随时间变化：录制文件会越攒越多）
SHAPE_ONLY = {"replay_list.json"}
# 内容是"JSON 文本套 JSON 文本"的端点：GUI 自动保存会用 JS 的 JSON.stringify 重写配置，
# 数值 16.0 → 16（数值相等、文本不同），因此按解析后的 JSON 语义比较
JSON_TEXT_ENDPOINTS = {"config_default.json"}
# 前端资源类抓取：本轮起前端允许演进，默认不比（--assets 打开）
ASSET_FILES = {"index.json", "app_js.json", "style_css.json"}


def same_json_text(a, b) -> bool:
    """比较 {name, content} 形状：content 按 JSON 解析后语义比较（数值 16.0 == 16）。"""
    try:
        ca = json.loads(a.get("content", ""))
        cb = json.loads(b.get("content", ""))
    except Exception:
        return False
    return normalize(ca) == normalize(cb)


def normalize(obj, key=None):
    """把易变字段整个丢掉，便于跨实现比对。"""
    if isinstance(obj, dict):
        # 兼容早期"掩码成 <volatile>"的黄金样本：两种形式都归一成"丢弃"
        return {k: normalize(v, k) for k, v in obj.items()
                if k not in VOLATILE_KEYS and v != "<volatile>"}
    if isinstance(obj, list):
        return [normalize(v, key) for v in obj]
    if isinstance(obj, str):
        s = obj.replace(str(ROOT), "<ROOT>").replace("\\", "/")
        s = re.sub(r"C:/Users/[^/\"']+", "<USER>", s)
        return s
    if isinstance(obj, float):
        return round(obj, 6)
    return obj


def fetch(url: str, timeout: float = 120.0):
    req = urllib.request.Request(url)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read().decode("utf-8", "replace")
            status = r.status
    except urllib.error.HTTPError as e:
        raw = e.read().decode("utf-8", "replace")
        status = e.code
    except Exception as e:  # noqa: BLE001
        return {"__error__": str(e)}
    try:
        return {"__status__": status, "body": normalize(json.loads(raw))}
    except json.JSONDecodeError:
        return {"__status__": status, "text_len": len(raw), "head": raw[:400],
                "is_html": raw.lstrip().lower().startswith("<!doctype")}


def replay_files() -> list[str]:
    """logs/ 下的回放样本 = **只算 `.wpr`**（回放只支持 wp8f 飞行记录）。

    旧的 `.acmi` / `.csv` 读路径已删除：它们既不出现在 /api/replay/list 里，
    也不再能被 /api/replay/load 打开，因此不能拿来当采集输入。
    """
    return sorted(p.name for p in (ROOT / "logs").glob("*.wpr"))


def capture(url: str, out: Path) -> int:
    """采集端点响应写入 out 目录，返回**采集失败条数**（0 = 全部成功）。

    以前这里固定 `return 0`：连不上实例时每条样本都是 {"__error__": ...}，
    调用方仍认为采集成功，比对自然一路"通过"。
    """
    # 复现采集前先清空：上一轮的样本留着会被当成本轮结果（端点少了也看不出来）
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True, exist_ok=True)
    base = url.rstrip("/")
    bad = 0
    for name, path in ENDPOINTS:
        data = fetch(base + path)
        (out / f"{name}.json").write_text(json.dumps(data, ensure_ascii=False, indent=1),
                                          encoding="utf-8")
        if "__error__" in data:
            bad += 1
            print(f"  {name:16} {'采集失败':<6} {path} —— {data['__error__']}")
        else:
            print(f"  {name:16} {'HTTP' + str(data.get('__status__')):8} {path}")
    # 静态资源 + 首页（只记形状，不比内容）
    for name, path in (("index", "/"), ("app_js", "/static/app.js"),
                       ("style_css", "/static/style.css")):
        data = fetch(base + path)
        (out / f"{name}.json").write_text(json.dumps(data, ensure_ascii=False, indent=1),
                                          encoding="utf-8")
        if "__error__" in data:
            bad += 1
            print(f"  {name:16} {'采集失败':<6} {path} —— {data['__error__']}")
        else:
            print(f"  {name:16} {'HTTP' + str(data.get('__status__')):8} {path}")
    # 回放加载（有 `.wpr` 飞行记录才有意义；.acmi/.csv 已不再支持）
    files = replay_files()
    (out / "_replay_files.txt").write_text("\n".join(files), encoding="utf-8")
    # 取**按名字排序后第一个能加载成功的**：不认的容器版本 / 坏文件会返回 500，
    # 按 mtime 取"最新那个"又会让黄金样本随着用户新录制漂移 ——
    # 名字排序 + 逐个试加载才是稳定且可复现的。
    picked = None
    for name in files:
        data = fetch(base + "/api/replay/load?file=" + urllib.parse.quote(name))
        if isinstance(data, dict) and data.get("__status__") == 200:
            picked = (name, data)
            break
    if picked:
        name, data = picked
        (out / "replay_load.json").write_text(json.dumps(data, ensure_ascii=False, indent=1),
                                              encoding="utf-8")
        print(f"  {'replay_load':16} {'HTTP' + str(data.get('__status__')):8} {name}")
    else:
        print(f"  {'replay_load':16} {'跳过':<6} logs/ 下没有可加载的 .wpr 飞行记录，"
              f"该端点本轮无法比对（放一个版本 2 的 .wpr 再跑）")
    return bad


def diff(a, b, path="") -> list[str]:
    out = []
    if type(a) is not type(b) and not (isinstance(a, (int, float)) and isinstance(b, (int, float))):
        return [f"{path or '.'}: 类型不同 {type(a).__name__} vs {type(b).__name__}"]
    if isinstance(a, dict):
        for k in sorted(set(a) | set(b)):
            if k not in a:
                out.append(f"{path}.{k}: Rust 多出该字段")
            elif k not in b:
                out.append(f"{path}.{k}: Rust 缺少该字段")
            else:
                out += diff(a[k], b[k], f"{path}.{k}")
    elif isinstance(a, list):
        if len(a) != len(b):
            out.append(f"{path}: 数组长度 {len(a)} vs {len(b)}")
        for i, (x, y) in enumerate(zip(a, b)):
            out += diff(x, y, f"{path}[{i}]")
            if len(out) > 25:
                return out
    elif isinstance(a, float) or isinstance(b, float):
        if abs(float(a) - float(b)) > 1e-6:
            out.append(f"{path}: {a} vs {b}")
    elif a != b:
        out.append(f"{path}: {a!r} vs {b!r}")
    return out


def same_shape(a, b) -> list[str]:
    """只比结构：类型一致、元素键集合一致（用于随时间增删的列表类端点）"""
    if type(a) is not type(b):
        return [f"类型不同 {type(a).__name__} vs {type(b).__name__}"]
    if isinstance(a, list):
        if bool(a) != bool(b):
            return ["一侧为空"]
        if a and b:
            return diff(sorted(a[0].keys()) if isinstance(a[0], dict) else a[0],
                        sorted(b[0].keys()) if isinstance(b[0], dict) else b[0], "[0]")
        return []
    return diff(a, b)


def compare(dir_a: Path, dir_b: Path, assets: bool = False) -> int:
    files = sorted({p.name for p in dir_a.glob("*.json")} | {p.name for p in dir_b.glob("*.json")})
    bad = 0
    for f in files:
        if f.startswith("_"):
            continue
        if not assets and f in ASSET_FILES:
            print(f"[SKIP] {f}（前端资源，可用 --assets 比对）")
            continue
        pa, pb = dir_a / f, dir_b / f
        if not pa.is_file() or not pb.is_file():
            # 一侧缺失**不算通过**（只 SKIP 会让"少了半个端点"照样绿）。
            # 常见原因：本轮采集失败（连不上实例）或 logs/ 下没有录制文件。
            missing = dir_a.name if not pa.is_file() else dir_b.name
            bad += 1
            print(f"[FAIL] {f}：{missing} 一侧没有这份样本"
                  f"（采集失败或缺输入？缺失的一侧不算通过）")
            continue
        a = json.loads(pa.read_text(encoding="utf-8"))
        b = json.loads(pb.read_text(encoding="utf-8"))
        if isinstance(a, dict) and "__status__" in a:
            if a.get("__status__") != b.get("__status__"):
                print(f"[FAIL] {f}: HTTP {a.get('__status__')} vs {b.get('__status__')}")
                bad += 1
                continue
            a, b = a.get("body", a), b.get("body", b)
        # 两侧都重新归一化：黄金样本可能是早期"掩码"格式，而新采集已丢字段
        a, b = normalize(a), normalize(b)
        if f in JSON_TEXT_ENDPOINTS and not same_json_text(a, b):
            bad += 1
            da = json.loads(a.get("content", "{}"))
            db = json.loads(b.get("content", "{}"))
            print(f"[FAIL] {f}（配置内容不一致）")
            for k in sorted(set(da) | set(db)):
                if normalize(da.get(k)) != normalize(db.get(k)):
                    print(f"        {k}: {da.get(k)!r} vs {db.get(k)!r}")
            continue
        if f in JSON_TEXT_ENDPOINTS:
            print(f"[PASS] {f}（配置内容语义一致）")
            continue
        d = same_shape(a, b) if f in SHAPE_ONLY else diff(a, b)
        if f in SHAPE_ONLY and not d:
            print(f"[PASS] {f}（结构一致；内容随时间变化，不比具体值）")
            continue
        if d:
            bad += 1
            print(f"[FAIL] {f}（{len(d)} 处差异）")
            for line in d[:12]:
                print("        " + line)
        else:
            print(f"[PASS] {f}")
    print(f"\n===== 一致性：{'全部通过' if bad == 0 else str(bad) + ' 个端点有差异'} =====")
    return 0 if bad == 0 else 1


def running_instances() -> list[int]:
    """已在运行的 wp8f-gui 进程（单实例判据＝端口独占：它占着端口，测试前必须先清场）"""
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


def selftest(golden: Path, port: int = 8899) -> int:
    """自己起一个 wp8f-gui.exe --no-window 实例，采集后与黄金样本比对（回归测试用）"""
    import subprocess
    import time

    exe = ROOT / "wp8f-gui.exe"
    if not exe.is_file():
        print(f"✗ 找不到 {exe} —— 先跑 bash scripts/build.sh --webui-only")
        return 1
    # FM 三个端点（fm_aircraft / fm_f16 / fm_f16_curves）要真调 flightmodel 子进程，它按
    # S1 布局在 binary/ 下；缺了会静默变成三个 500，比对结果就看不出真正原因。
    fm_exe = ROOT / "binary" / "flightmodel.exe"
    if not fm_exe.is_file():
        print(f"✗ 找不到 {fm_exe} —— 先跑 bash scripts/build.sh（只跑 --webui-only 不会产出 binary/ 里的三个）")
        return 1
    if not golden.is_dir():
        print(f"✗ 黄金样本目录不存在: {golden}")
        return 1
    if running_instances():
        print(f"✗ 已有 wp8f-gui 在运行（单实例判据＝端口独占，它会占着端口）—— 先退出它再跑 selftest")
        return 1
    # 实例的 stdout+stderr 落临时文件（无管道：实例若一直活着，管道写满会把双方都挂住）
    child_log = tempfile.TemporaryFile(mode="w+", encoding="utf-8", errors="replace")
    proc = subprocess.Popen([str(exe), "--root", str(ROOT), "--no-window", "--no-tray-promote",
                             "--port", str(port)],
                            cwd=str(ROOT), stdout=child_log, stderr=subprocess.STDOUT)
    url = f"http://127.0.0.1:{port}"
    try:
        ready = False
        for _ in range(150):
            if proc.poll() is not None:
                child_log.flush()
                child_log.seek(0)
                out = child_log.read()
                # 端口上已有控制台：本次实例唤醒它的窗口后以 0 退出（不是错误，是环境）；
                # 端口被别的程序占住 / 已有控制台但唤不醒：实例以 2/3 退出并打印原因。
                # 针脚是**英文固定串**（gui/src/main.rs 的 port_busy_exit），不随 i18n 变。
                if proc.returncode == 0 and "already running" in out:
                    print(f"✗ 端口 {port} 上已有控制台在运行（本次实例唤出它的窗口后退出）"
                          f"—— 先退出它再跑 selftest")
                    return 2
                print(f"✗ wp8f-gui 启动即退出（exit={proc.returncode}）："
                      f"{out.strip() or '(无输出，看 logs/console-host.json/日志)'}")
                return 2 if ("occupied by another program" in out
                             or "could not be woken up" in out) else 1
            try:
                urllib.request.urlopen(url + "/api/health", timeout=1.5)
                ready = True
                break
            except Exception:
                time.sleep(0.1)
        if not ready:
            print("✗ wp8f-gui 后端未就绪（见 logs/console-host.log）")
            return 1
        out = ROOT / "logs" / "golden_rust"
        # capture() 会先清空 out 目录（上一轮的样本残留会被当成本轮结果）
        capture_bad = capture(url, out)
        print()
        if capture_bad:
            print(f"✗ 采集有 {capture_bad} 个端点失败 —— 这份样本不能用来比对"
                  f"（看上面的『采集失败』行；实例没起来/端口不对都会这样）")
            return 1
        return compare(golden, out)
    finally:
        proc.kill()


def main() -> int:
    ap = argparse.ArgumentParser(description="API 一致性采集/比对")
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("capture", help="对某个运行中的实例采集响应")
    c.add_argument("--url", required=True)
    c.add_argument("--out", required=True)
    m = sub.add_parser("compare", help="比对两次采集")
    m.add_argument("a")
    m.add_argument("b")
    m.add_argument("--assets", action="store_true", help="连前端资源一起比")
    t = sub.add_parser("selftest", help="自动起实例并与黄金样本比对（推荐）")
    t.add_argument("--golden", default=str(Path(__file__).resolve().parent / "golden_python"))
    t.add_argument("--port", type=int, default=8899)
    args = ap.parse_args()
    if args.cmd == "capture":
        return capture(args.url, Path(args.out))
    if args.cmd == "selftest":
        return selftest(Path(args.golden), args.port)
    return compare(Path(args.a), Path(args.b), assets=args.assets)


if __name__ == "__main__":
    sys.exit(main())
