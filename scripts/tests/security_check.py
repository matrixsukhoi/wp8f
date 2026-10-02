#!/usr/bin/env python3
"""批 1 安全修复的行为验证：跨站 Origin / 简单请求 Content-Type / 请求体上限 /
路径百分号解码。全部走真实 HTTP，对 --no-window 实例。
上传用例会往 logs/ 写一个样本文件（用户可见：GUI「录制文件」下拉会列出来），
结束时（含失败路径）删除。
"""
import json, struct, subprocess, sys, tempfile, time, urllib.error, urllib.parse, urllib.request
from pathlib import Path

# 输出重定向到文件时 Windows 默认 GBK，打印 ✓/✕ 会 UnicodeEncodeError 直接崩；
# 统一强制 UTF-8，让 CI/重定向场景与真实控制台行为一致。
sys.stdout.reconfigure(encoding="utf-8", errors="replace")


def _find_root() -> Path:
    """仓库根：从本文件向上找 Cargo.toml（脚本位于 scripts/tests/ 下）"""
    for probe in [Path(__file__).resolve().parent, *Path(__file__).resolve().parents]:
        if (probe / "Cargo.toml").is_file() and (probe / "gui").is_dir():
            return probe
    return Path(__file__).resolve().parents[2]


ROOT = _find_root()
PORT = 8861
EXE = ROOT / "wp8f-gui.exe"
UPLOAD_GLOB = "b1_upload*.wpr"
# 上传用例开始前就存在的同名文件（历史遗留/用户自己的）：结束时不能误删
PRE_EXISTING = {p.name for p in (ROOT / "logs").glob(UPLOAD_GLOB)}
PASS, FAIL = [], []


def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" · {detail}" if detail else ""), flush=True)


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

    控制台自己的单实例判据是**端口独占**（不再有全机命名互斥体）：探针用的是 8861 这类
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


_OTHER_GUI = gui_pids()
if _OTHER_GUI:
    print(f"[SKIP] 已有控制台在跑（pid={_OTHER_GUI}）—— 本次探针会与它共用 "
          f"logs/console-host.json 与控制台日志，先退出它再跑测试", flush=True)
    sys.exit(2)


proc = subprocess.Popen([str(EXE), "--root", str(ROOT), "--no-window", "--no-tray-promote",
                         "--port", str(PORT)],
                        cwd=str(ROOT), stdout=_CHILD_LOG, stderr=subprocess.STDOUT)
url = f"http://127.0.0.1:{PORT}"
try:
    for _ in range(150):
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
            print(f"端口 {PORT} 上已有控制台在运行（本次实例唤出它的窗口后退出），无法验证")
            sys.exit(2)
        if "occupied by another program" in out or "could not be woken up" in out:
            print(f"端口 {PORT} 不可用，控制台实例没起来：{out.strip()}")
            sys.exit(2)
        print(f"本次实例启动即退出（exit={proc.returncode}）：{out.strip() or '(无输出)'}")
        sys.exit(1)
    if runtime_pid() != proc.pid:
        print(f"端口 {PORT} 上的服务者不是本次实例（运行态 pid={runtime_pid()}，"
              f"本次 pid={proc.pid}）—— 探测会打到别人身上，先退出已有控制台再跑")
        sys.exit(1)

    def raw(path, method="GET", data=None, headers=None):
        req = urllib.request.Request(url + path, data=data, method=method, headers=headers or {})
        try:
            with urllib.request.urlopen(req, timeout=15) as r:
                return r.status, r.read()[:120].decode("utf-8", "replace")
        except urllib.error.HTTPError as e:
            return e.code, e.read()[:160].decode("utf-8", "replace")

    # ① 跨站 Origin → 403
    st, body = raw("/api/config/save", "POST", b'{"path":"x.json","content":"{}"}',
                   {"Content-Type": "application/json", "Origin": "http://evil.example"})
    check("跨站 Origin 被拒（403）", st == 403, f"{st} {body[:60]}")

    # ② 本机同源 Origin → 放行（用非法文件名，保证 400 而不是真写文件：
    #    （这条测试不得往仓库里写 config/__nope__.json））
    st, _ = raw("/api/config/save", "POST", b'{"path":"../nope.json","content":"{}"}',
                {"Content-Type": "application/json", "Origin": f"http://127.0.0.1:{PORT}"})
    check("同源 Origin 放行（非 403/415，非法名仍 400）", st == 400, f"{st}")

    # ③ 表单类"简单请求"Content-Type → 415（这是 CSRF 的实际入口）
    st, body = raw("/api/config/save", "POST", b'{"path":"x.json","content":"{}"}',
                   {"Content-Type": "application/x-www-form-urlencoded"})
    check("表单 Content-Type 被拒（415）", st == 415, f"{st} {body[:60]}")
    st, _ = raw("/api/config/save", "POST", b"x", {"Content-Type": "text/plain"})
    check("text/plain 被拒（415）", st == 415, f"{st}")
    st, _ = raw("/api/config/save", "POST", b"x", {"Content-Type": "multipart/form-data; boundary=x"})
    check("multipart 被拒（415）", st == 415, f"{st}")

    # ④ 正常 JSON POST 仍然可用（配置保存）
    # 这里故意给 content:null（非 JSON 字符串）→ 处理逻辑应判 400。
    # 断言不能写成 `st not in (403, 415)`：500 也会算通过 ——
    # 现在只接受预期状态码，5xx 必须失败并把响应体打出来。
    st, body = raw("/api/config/save", "POST", b'{"path":"default.json","content":null}',
                   {"Content-Type": "application/json"})
    check("JSON POST 未被安全策略误伤（到达处理逻辑并返回 400，不是 403/415/5xx）",
          st == 400, f"{st} {body[:80]}")

    # ⑤ 请求体上限 → 413（声明 20MB，不发数据）
    import socket
    s = socket.create_connection(("127.0.0.1", PORT), timeout=10)
    s.sendall(b"POST /api/replay/upload?name=x.acmi HTTP/1.1\r\nHost: 127.0.0.1\r\n"
              b"Content-Type: application/octet-stream\r\nContent-Length: 20971520\r\n\r\n")
    resp = s.recv(200).decode("utf-8", "replace")
    s.close()
    check("超大请求体被拒（413）", " 413 " in resp, resp.split("\r\n")[0])

    # ⑥ 路径百分号解码：中文/编码路径要被解析（而不是把 %E4%B8%AD 当字面量）
    st, _ = raw("/api/config/" + urllib.parse.quote("default.json"))
    check("URL 编码路径可解析（200）", st == 200, f"{st}")
    st, body = raw("/api/config/" + urllib.parse.quote("测试.json"))
    check("中文文件名被解码（由 400 变为 404 未找到）", st == 404, f"{st} {body[:40]}")

    # ⑦ 上传仍然可用（带被允许的 CT）
    # 上传只接受 .wpr —— 样本名与内容都按 .wpr 来。
    # logs/ 下没有 .wpr 时**不能静默跳过**（"没测过"会被误当成"测过且通过"）：
    # 自己造一段最小合法输入 —— 上传接口只校验文件名后缀 + 内容非空，按原始字节落盘。
    sample = next((ROOT / "logs").glob("*.wpr"), None)
    if sample:
        payload, src_desc = sample.read_bytes()[:100000], sample.name
    else:
        # 注意：bytes 字面量只能放 ASCII，注释就用英文写
        payload = (b"WPR1" + struct.pack("<I", 2) + struct.pack("<III", 2, 1, 0)
                   + struct.pack("<I", 0)
                   + b"{}")   # 最小容器：magic + 版本 2 + 空 meta/单组空 CSV/无底图
        src_desc = "合成样本（logs/ 下没有 .wpr）"
    st, body = raw("/api/replay/upload?name=" + urllib.parse.quote("b1_upload.wpr"), "POST",
                   payload, {"Content-Type": "application/octet-stream"})
    check("二进制上传仍可用（200）", st == 200 and "b1_upload" in body,
          f"{st} {body[:60]} · 样本={src_desc}")
finally:
    proc.kill()
    # 兜底（上传失败/断言失败也走到这里）：删掉本次上传进 logs/ 的 b1_upload*.wpr。
    # 只删"本次运行开始时还不存在"的那些（重名会加序号，不许靠猜文件名）。
    for _p in (ROOT / "logs").glob(UPLOAD_GLOB):
        if _p.name in PRE_EXISTING:
            continue
        try:
            _p.unlink(missing_ok=True)
        except OSError as e:
            print("删除上传样本失败:", _p.name, e)

print(f"\n===== {len(PASS)}/{len(PASS) + len(FAIL)} 通过 =====")
sys.exit(0 if not FAIL else 1)
