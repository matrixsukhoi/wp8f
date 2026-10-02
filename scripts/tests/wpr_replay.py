#!/usr/bin/env python3
""".wpr（飞行记录）接口端到端：**Python 侧独立造夹具** → 列表 / 加载 / 底图 / 转换 + 错误路径。

夹具在 Python 里按容器格式重新拼一遍（`WPR1` + 20 字节头 + meta JSON + CSV + JPEG 字节），
**故意不复用 Rust 的编码器**：Rust 侧把格式改坏（magic/版本/表头/列数/meta 字段名）时，
只有独立实现才能发现 —— 拿 logger 自己编出来的字节去喂 logger 自己，永远是绿的。

界面层（xoy 底面 / 转换区 DOM）归 ui_checks.py；这里只打接口。

用法：
    python scripts/tests/wpr_replay.py        # 用仓库根的 wp8f-gui.exe（build.sh 之后）
    python scripts/tests/wpr_replay.py --exe target/x86_64-pc-windows-gnu/debug/wp8f-gui.exe
"""
import argparse
import json
import struct
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

# 输出重定向到文件时 Windows 默认 GBK，打印 ✓/✕ 会 UnicodeEncodeError 直接崩
sys.stdout.reconfigure(encoding="utf-8", errors="replace")

ROOT = Path(__file__).resolve().parents[2]
PORT = 8801
FIXTURE = "wp8f_probe_wpr.wpr"
# 「记录没带底图」的夹具：一份"有元数据、没字节"的记录。wpr 不带底图就不铺底面，
# 回放 JSON 里连 `fallback_image` 字段都没有（借图逻辑已删）。
# 几何故意与主夹具（100000×80000 / 1024×819）不同：断言才确定。
NOMAP = "wp8f_probe_nomap.wpr"
FB_W_M, FB_H_M, FB_IMG_W, FB_IMG_H = 200_000.0, 100_000.0, 2048, 1024

# 与 logger/src/wpr.rs 的 CSV_HEADER / NUM_COLS 故意各抄一份（契约副本，改坏了这里会红）
CSV_HEADER = ("time_ns,type,frame,x,y,altitude,radio_altitude,ias,tas,mach,aoa,aos,ny,vy,wx,"
              "heading,roll,pitch,aileron,elevator,rudder,trimmer,flaps,gear,airbrake,throttle,"
              "wing_sweep,turn_rate,turn_radius,sep,energy_height,thrust_to_weight,total_thrust,"
              "total_hp,thrust_percent,total_drag,manifold_pressure,fuel_kg,fuel_percent,fuel1_kg,"
              "overspeed_warning,mach_warning,g_load_warning,voice_alarm,brake_caution")
NUM_COLS = 43
IDX = {"frame": 0, "x": 1, "y": 2, "altitude": 3, "ias": 5, "tas": 6, "mach": 7, "aoa": 8,
       "ny": 10, "heading": 13, "roll": 14, "pitch": 15}
IMG = bytes([0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0xAB, 0xCD])

RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" · {detail}" if detail else ""), flush=True)


def row(t_ns, x, y, alt=3000.0, hdg=90.0, pitch=-2.5, aircraft="f_16c"):
    v = [0.0] * NUM_COLS
    v[IDX["frame"]] = 42.0
    v[IDX["x"]], v[IDX["y"]] = x, y
    v[IDX["altitude"]] = alt
    v[IDX["ias"]] = 540.0
    v[IDX["tas"]] = 560.0
    v[IDX["mach"]] = 0.82
    v[IDX["aoa"]] = 3.5
    v[IDX["ny"]] = 1.2
    v[IDX["heading"]] = hdg
    v[IDX["pitch"]] = pitch
    return f"{t_ns},{aircraft}," + ",".join(f"{c:g}" for c in v)


def build_fixture(path: Path) -> tuple:
    """按**版本 2**容器格式拼字节（独立实现，故意不复用 Rust 编码器）：

    magic | version=2 | meta_len | group_count | img_len | csv_len[G] | meta | csv[0..G] | img

    返回 (玩家组 CSV 文本, 友机组 CSV 文本)。
    """
    t0 = 1_700_000_000_000_000_000
    g0 = (CSV_HEADER + "\n" + row(t0, 0.5, 0.5) + "\n"
          + row(t0 + 100_000_000, 0.6, 0.25, hdg=95.0) + "\n")
    g1 = CSV_HEADER + "\n" + row(t0 + 50_000_000, 0.55, 0.4, hdg=270.0, aircraft="su_27") + "\n"
    meta = {
        "container": 2,
        "created_ms": 1_700_000_000_000,
        "groups": 2,                     # 元数据里也有一份组数（与头交叉校验）
        "frames": 2,                     # 玩家组帧数
        "poll_hz": 10.0,
        # 地图换算数据（与 HUD 地图面板同口径）：归一化 × 尺寸 = 米，+ min_* = 世界坐标
        "map": {"width_m": 100_000.0, "height_m": 80_000.0,
                "min_x_m": -50_000.0, "min_y_m": -40_000.0,   # 中心 = 世界 0 点
                "grid_zero": [6494.3, 19547.5], "grid_steps": [5500.0, 5500.0],
                "img_w": 1024, "img_h": 819, "img_mime": "image/jpeg"},
        "generator": "wp8f-probe",
        "aircraft": "f_16c",
        "group_info": [{"aircraft": "f_16c", "frames": 2}, {"aircraft": "su_27", "frames": 1}],
    }
    mj = json.dumps(meta, ensure_ascii=False).encode("utf-8")
    gs = [g0.encode("utf-8"), g1.encode("utf-8")]
    blob = (b"WPR1" + struct.pack("<I", 2) + struct.pack("<III", len(mj), len(gs), len(IMG))
            + b"".join(struct.pack("<I", len(g)) for g in gs)
            + mj + b"".join(gs) + IMG)
    path.write_bytes(blob)
    return g0, g1


def build_nomap_fixture(path: Path, *, width_m: float, height_m: float, img_w: int, img_h: int) -> None:
    """版本 2、**没有底图字节**（`img_len = 0`）的记录：地图元数据齐全，只是字节是空的
    —— 用户实测的 `logs/wp8f_1790841951594.wpr` 就是这样（记录端漏传原始字节）。"""
    meta = {
        "container": 2,
        "created_ms": 1_700_000_000_000,
        "groups": 1,
        "frames": 1,
        "poll_hz": 30.0,
        "map": {"width_m": width_m, "height_m": height_m,
                "min_x_m": -width_m / 2, "min_y_m": -height_m / 2,
                "grid_zero": [0.0, 0.0], "grid_steps": [0.0, 0.0],
                "img_w": img_w, "img_h": img_h, "img_mime": "application/octet-stream"},
        "generator": "wp8f-probe-nomap",
        "aircraft": "f_16c",
        "group_info": [{"aircraft": "f_16c", "frames": 1}],
    }
    csv = CSV_HEADER + "\n" + row(1_700_000_000_000_000_000, 0.5, 0.5) + "\n"
    mj = json.dumps(meta, ensure_ascii=False).encode("utf-8")
    body = csv.encode("utf-8")
    path.write_bytes(b"WPR1" + struct.pack("<I", 2) + struct.pack("<III", len(mj), 1, 0)
                     + struct.pack("<I", len(body)) + mj + body)


def get(path, base):
    """返回 (状态码, 响应体 bytes, content-type)"""
    try:
        with urllib.request.urlopen(base + path, timeout=20) as r:
            return r.status, r.read(), r.headers.get("Content-Type", "")
    except urllib.error.HTTPError as e:
        return e.code, e.read(), e.headers.get("Content-Type", "")


def _gui_pids() -> list[int]:
    """本机已在跑的 wp8f-gui.exe（探针与用户控制台共用 logs/，必须先清场 —— 见 main()）"""
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


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=str(ROOT / "wp8f-gui.exe"))
    args = ap.parse_args()
    exe = Path(args.exe)
    if not exe.is_file():
        print(f"[SKIP] 找不到 {exe}（先 bash scripts/build.sh，或用 --exe 指定调试构建）")
        return 2
    # 前置：已有控制台在跑就先退出。单实例判据 = 端口独占，
    # 用户那个 8765 的控制台不会"顶掉"本次实例（探针用 PORT 专用端口），但两者**共用同一份
    # logs/console-host.json 与控制台日志** —— 跑下去会把用户那份顶掉（之后双击唤不出窗口）。
    other = _gui_pids()
    if other:
        print(f"[SKIP] 已有控制台在跑（pid={other}）—— 本次探针会与它共用 "
              f"logs/console-host.json 与控制台日志，先退出它再跑测试")
        return 2

    logs = ROOT / "logs"
    logs.mkdir(exist_ok=True)
    fixture = logs / FIXTURE
    produced = [logs / (Path(FIXTURE).stem + s) for s in (".csv", "_tacview.csv", ".acmi")]
    # 旧格式样本（故意放进 logs/）：回放只支持 .wpr —— 它既不该出现在列表里，也不该能加载
    legacy = logs / "wp8f_probe_legacy.acmi"
    legacy.write_text("FileType=text/acmi/tacview\nFileVersion=2.2\n#0\n"
                      "0,ReferenceLongitude=37.6,ReferenceLatitude=55.7\n"
                      "#1.0\n-1,T=0.1|0.2|3000|0|0|90\n", encoding="utf-8")
    player_csv, peer_csv = build_fixture(fixture)
    # 没带底图字节的新记录（地图元数据齐全、字节是空的）
    nomap = logs / NOMAP
    build_nomap_fixture(nomap, width_m=FB_W_M, height_m=FB_H_M,
                        img_w=FB_IMG_W, img_h=FB_IMG_H)
    base = f"http://127.0.0.1:{PORT}"

    # 本次实例的 stdout+stderr 落临时文件（无管道：实例若一直活着，管道写满会把双方都挂住）
    child_log = tempfile.TemporaryFile(mode="w+", encoding="utf-8", errors="replace")
    proc = subprocess.Popen([str(exe), "--root", str(ROOT), "--no-window", "--no-tray-promote",
                             "--port", str(PORT)],
                            cwd=str(ROOT), stdout=child_log, stderr=subprocess.STDOUT)
    try:
        for _ in range(120):
            try:
                urllib.request.urlopen(base + "/api/health", timeout=0.4)
                break
            except Exception:
                time.sleep(0.1)
        # 单实例判据 = 端口独占（没有全机命名互斥体）。但"端口上有服务"
        # 不等于"端口上的服务就是本次 spawn 的实例"：已有控制台会秒回 /api/health，而本次
        # 实例还要走完「唤出它的窗口 → 打印提示 → 退出」才消失 —— 只 poll() 一次会输给这个
        # 竞态，探测就打到**用户那个实例**上（假绿）。所以先按运行态文件里的 pid 对账。
        def runtime_pid() -> int:
            try:
                return int(json.loads((ROOT / "logs" / "console-host.json")
                                      .read_text(encoding="utf-8"))["pid"])
            except Exception:  # noqa: BLE001
                return -1

        if proc.poll() is None and runtime_pid() != proc.pid:
            try:
                proc.wait(timeout=5)      # 被拒时它会在这段时间里退出
            except subprocess.TimeoutExpired:
                pass
        if proc.poll() is not None:
            child_log.flush()
            child_log.seek(0)
            out = child_log.read()
            # 针脚是**英文固定串**：控制台 stdout 的那三条诊断不随 i18n 界面语言变
            # （gui/src/main.rs 的 port_busy_exit）；"已有控制台"这条同时要求退出码 0。
            if proc.returncode == 0 and "already running" in out:
                print(f"[SKIP] 端口 {PORT} 上已有控制台在运行（本次实例唤出它的窗口后退出）；"
                      f"请先退出控制台再跑测试")
                return 2
            if "occupied by another program" in out or "could not be woken up" in out:
                print(f"[SKIP] 端口 {PORT} 不可用，控制台实例没起来：{out.strip()}")
                return 2
            print(f"[FAIL] 本次实例启动即退出（exit={proc.returncode}）：{out.strip() or '(无输出)'}")
            return 1
        if runtime_pid() != proc.pid:
            print(f"[FAIL] 端口 {PORT} 上的服务者不是本次实例（运行态 pid={runtime_pid()}，"
                  f"本次 pid={proc.pid}）—— 探测会打到别人身上，先退出已有控制台")
            return 1

        # ① 列表：只列 .wpr（没有 kind 字段）
        code, body, _ = get("/api/replay/list", base)
        items = json.loads(body) if code == 200 else []
        hit = next((it for it in items if it.get("name") == FIXTURE), None)
        check("列表能列出 .wpr 记录", code == 200 and hit is not None, f"code={code} n={len(items)}")
        check("列表项形状 = {name,size,mtime}（不再有 kind）",
              bool(hit) and sorted(hit.keys()) == ["mtime", "name", "size"], str(hit))
        check("列表里只有 .wpr（旧 .acmi/.csv 不再收录）",
              all(it.get("name", "").lower().endswith(".wpr") for it in items),
              str([it.get("name") for it in items][:5]))
        check("logs/ 里的旧 .acmi 不再出现在列表里",
              legacy.name not in [it.get("name") for it in items], legacy.name)
        # 旧 .acmi 仍可作**导出产物**存在（转换器写出来的就是它），但不再能当回放输入
        code, body, _ = get(f"/api/replay/load?file={legacy.name}", base)
        txt = body.decode("utf-8", "replace")
        check("加载旧 .acmi 被明确拒绝（400 且说明只支持 .wpr）",
              code == 400 and ".wpr" in txt, f"code={code} {txt[:100]!r}")

        # ② 加载：**多组** .wpr → 每组一个对象；坐标 = 世界 km（归一化 × 尺寸 + min_*）
        code, body, _ = get(f"/api/replay/load?file={FIXTURE}", base)
        d = json.loads(body) if code == 200 else {}
        check("加载 .wpr 成功且 format=wpr / planar=true",
              code == 200 and d.get("format") == "wpr" and d.get("planar") is True,
              f"code={code} {str(d)[:120]}")
        objs = d.get("objects") or []
        check("两组 CSV → 两个对象（组 0 = 玩家）",
              len(objs) == 2 and objs[0].get("role") == "player"
              and objs[1].get("role") == "other" and objs[1].get("name") == "su_27",
              f"{[(o.get('name'), o.get('role')) for o in objs]}")
        check("元数据带 groups 组数", d.get("groups") == 2, str(d.get("groups")))
        check("坐标口径写在响应里（world_km_north_up）",
              d.get("coords") == "world_km_north_up", str(d.get("coords")))
        frames = (objs or [{}])[0].get("frames") or []
        check("帧数 = 夹具行数", len(frames) == 2, f"{len(frames)}")
        if len(frames) == 2:
            f0, f1 = frames[0], frames[1]
            ok_t = f0[0] == 1_700_000_000_000 and f1[0] == 1_700_000_000_100
            check("时间列 = UNIX 毫秒", ok_t, f"{f0[0]} / {f1[0]}")
            # 首帧 (0.5,0.5) = 地图中心；min_* = ±尺寸/2 ⇒ 世界坐标恰好是 0,0
            check("世界坐标 = min_* + 归一化 × 尺寸（首帧落在世界 0 点）",
                  abs(f0[1]) < 1e-6 and abs(f0[2]) < 1e-6, f"{f0[1]} / {f0[2]}")
            # x 0.6 → −50 + 0.6×100 = 10；y 0.25 → −40 + (1−0.25)×80 = 20
            check("第二帧 = 东 10 / 北 20 km（mappanel 口径）",
                  abs(f1[1] - 10.0) < 1e-6 and abs(f1[2] - 20.0) < 1e-6, f"{f1[1]} / {f1[2]}")
            check("pitch 抬头为正（记录 -2.5 → 回放 2.5）", abs(f0[5] - 2.5) < 1e-9, str(f0[5]))
            check("速度换算成 m/s（IAS 540 km/h → 150）", abs(f0[7] - 150.0) < 1e-9, str(f0[7]))
        if len(objs) == 2:
            pf = (objs[1].get("frames") or [[]])[0]
            # 友机 x 0.55 → 东 5；y 0.40 → 北 48−40 = 8
            check("友机帧同样按世界坐标换算（东 5 / 北 8 km）",
                  abs(pf[1] - 5.0) < 1e-6 and abs(pf[2] - 8.0) < 1e-6, f"{pf[1]} / {pf[2]}")
        m = d.get("map") or {}
        check("地图矩形 = 世界坐标四角（±50 / ±40 km）",
              [m.get("x0_km"), m.get("x1_km"), m.get("y0_km"), m.get("y1_km")] == [-50.0, 50.0, -40.0, 40.0],
              str({k: m.get(k) for k in ("x0_km", "x1_km", "y0_km", "y1_km")}))
        check("换算数据原样带给前端（尺寸 + min_* + 网格）",
              m.get("width_m") == 100_000.0 and m.get("height_m") == 80_000.0
              and m.get("min_x_m") == -50_000.0 and m.get("min_y_m") == -40_000.0
              and m.get("grid_steps") == [5500.0, 5500.0], str(m))
        check("底图元信息完整（像素尺寸 + MIME + has_image）",
              m.get("img_w") == 1024 and m.get("img_h") == 819
              and m.get("mime") == "image/jpeg" and m.get("has_image") is True, str(m))
        check("底图字节不再内联进回放 JSON（体积取舍）",
              "image" not in m and len(body) < 4096, f"响应 {len(body)} 字节")
        check("机型 / 记录频率来自 meta",
              d.get("aircraft") == "f_16c" and abs((d.get("poll_hz") or 0) - 10.0) < 1e-9,
              f"{d.get('aircraft')} / {d.get('poll_hz')}")

        # ③ 底图端点：原始字节 + MIME
        code, img, ctype = get(f"/api/replay/map?file={FIXTURE}", base)
        check("底图端点返回原始字节（与夹具逐字节一致）",
              code == 200 and img == IMG, f"code={code} {len(img)} 字节")
        check("底图 Content-Type 取记录里的 img_mime",
              ctype.startswith("image/jpeg"), ctype)

        # ③b) 记录没带底图 ⇒ 不铺底面：JSON 里没有 fallback_image
        #      底图端点也只能 404。
        code, body, _ = get(f"/api/replay/load?file={FIXTURE}", base)
        dm = (json.loads(body).get("map") or {}) if code == 200 else {}
        check("自己带底图的记录：has_image=true，且没有 fallback_image 字段",
              code == 200 and dm.get("has_image") is True and "fallback_image" not in dm,
              str({k: dm.get(k) for k in ("has_image", "fallback_image")}))
        code, body, _ = get(f"/api/replay/load?file={NOMAP}", base)
        dnm = json.loads(body) if code == 200 else {}
        dnm_map = dnm.get("map") or {}
        check("没带底图的记录：has_image=false，且**不再**给出任何回退底图",
              code == 200 and dnm_map.get("has_image") is False
              and "fallback_image" not in dnm_map,
              f"code={code} map_keys={sorted(dnm_map)}")
        code, body, _ = get(f"/api/replay/map?file={NOMAP}", base)
        check("没带底图的记录自己也没有底图字节（404，不伪造也不外借）",
              code == 404 and "没有地图底图" in body.decode("utf-8", "replace"), f"code={code}")

        # ④ 错误路径：别静默、别越界
        code, body, _ = get("/api/replay/map?file=..%2Fdefault.json", base)
        check("底图端点拒绝带路径分隔符的文件名（400）", code == 400, f"code={code} {body[:80]!r}")
        code, body, _ = get("/api/replay/map?file=default.json", base)
        check("底图端点拒绝非 .wpr（404）", code == 404, f"code={code} {body[:80]!r}")
        code, body, _ = get(f"/api/replay/convert?file={FIXTURE}&format=bogus", base)
        check("未知导出格式报 400（且提示支持哪几种）",
              code == 400 and "未知导出格式" in body.decode("utf-8", "replace"),
              f"code={code} {body[:80]!r}")
        code, body, _ = get(f"/api/replay/convert?file={FIXTURE}&format=acmi&lat=999&lon=0", base)
        check("零点纬度越界报 400", code == 400, f"code={code} {body[:80]!r}")

        # ⑤ 三种导出：落盘到 logs/，内容可核
        for fmt, expect in (("flat", Path(FIXTURE).stem + ".csv"),
                            ("csv", Path(FIXTURE).stem + "_tacview.csv"),
                            ("acmi", Path(FIXTURE).stem + ".acmi")):
            code, body, _ = get(f"/api/replay/convert?file={FIXTURE}&format={fmt}"
                                f"&lat=55.7558&lon=37.6173", base)
            ok_json = code == 200
            info = json.loads(body) if ok_json else {}
            target = logs / expect
            check(f"导出 {fmt} 写出 {expect}", ok_json and info.get("name") == expect and target.is_file(),
                  f"code={code} {info.get('name')}")

        flat = produced[0].read_text(encoding="utf-8") if produced[0].is_file() else ""
        lines = flat.splitlines()
        check("FlatCSV = 记录自带 CSV（表头逐字一致 + 每帧一行）",
              bool(lines) and lines[0] == CSV_HEADER and len(lines) == 3, f"{len(lines)} 行")
        if len(lines) == 3:
            c = lines[1].split(",")
            check("FlatCSV 第 1 行坐标/高度与夹具一致（不做任何换算）",
                  len(c) == NUM_COLS + 2 and abs(float(c[3]) - 0.5) < 1e-9
                  and abs(float(c[4]) - 0.5) < 1e-9 and abs(float(c[5]) - 3000.0) < 1e-9,
                  f"列数 {len(c)} x={c[3]} y={c[4]} alt={c[5]}")
        tv = produced[1].read_text(encoding="utf-8") if produced[1].is_file() else ""
        check("TacView CSV 表头正确（Latitude/Longitude 由零点还原）",
              tv.startswith("Time,Longitude,Latitude,Altitude,Roll,") and len(tv.splitlines()) == 3,
              tv.splitlines()[0] if tv else "(空)")
        ac = produced[2].read_text(encoding="utf-8") if produced[2].is_file() else ""
        check("ACMI 带上两架飞机（每组一个对象声明）",
              ac.startswith("FileType=text/acmi/tacview")
              and "-1,Type=Air+FixedWing,Name=f_16c," in ac
              and "-2,Type=Air+FixedWing,Name=su_27," in ac
              and ac.count("Type=Air+FixedWing") == 2,
              f"{len(ac.splitlines())} 行")
        # 时间戳必须单调不减（多机组按时间合流后不能倒着写）
        times = [float(x[1:]) for x in ac.splitlines() if x.startswith("#")]
        check("ACMI 时间戳单调不减", times == sorted(times) and len(times) == 3, str(times))

        # 夹具 CSV 与 FlatCSV 都是探针产物，必须清干净（logs/ 是用户可见目录）
        check("探针没往 logs/ 留下别的文件",
              {p.name for p in logs.glob("wp8f_probe_*")}
              <= {fixture.name, nomap.name, legacy.name} | {p.name for p in produced},
              str(sorted(p.name for p in logs.glob("wp8f_probe_*"))))
        print(f"（夹具 玩家 {len(player_csv)} + 友机 {len(peer_csv)} 字节 CSV + {len(IMG)} 字节底图 → 三种导出均已核对）")
    finally:
        proc.kill()
        for p in [fixture, nomap, legacy] + produced:
            try:
                p.unlink(missing_ok=True)
            except OSError as e:
                print("清理失败:", p.name, e)

    passed = sum(RESULTS)
    print(f"\n===== {passed}/{len(RESULTS)} 通过 =====")
    return 0 if RESULTS and passed == len(RESULTS) else 1


if __name__ == "__main__":
    sys.exit(main())
