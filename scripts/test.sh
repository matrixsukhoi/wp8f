#!/usr/bin/env bash
# =============================================================================
# wp8f 测试（Linux / WSL）：只跑 **Rust 单测**（本机原生，最快）。
#
#   scripts/test.sh                    # 严格：任何失败（含已知历史失败）都算失败 → 退出码 1
#   scripts/test.sh --allow-known-fail # 容忍白名单里的历史失败（本地日常用）
#   scripts/test.sh --gui              # 打印 Windows 侧 GUI 测试的跑法（不在 WSL 里驱动 GUI）
#   scripts/test.sh -h
#
# 默认严格：把白名单里的失败当通过会让脚本 exit 0（挂 CI 就是假绿）。
# 白名单只是记录"这个失败是历史遗留、与本轮改动无关"，不该改变退出码。
#
# 控制台 GUI 的端到端测试（真实托盘/窗口/WebView2/子进程）请在 **Windows** 上跑，
# 见 --gui 打印的清单。从 WSL 互操作去驱动那些脚本会因进程环境差异而不稳，故不支持。
#
# 退出码：0 通过；1 失败；2 环境不满足
# =============================================================================
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

ALLOW_KNOWN=0
SHOW_GUI=0

C_OK=$'\033[32m'; C_WARN=$'\033[33m'; C_ERR=$'\033[31m'; C_HL=$'\033[36m'; C_OFF=$'\033[0m'
step() { echo; echo "${C_HL}==> $*${C_OFF}"; }
ok()   { echo "    ${C_OK}✓${C_OFF} $*"; }
warn() { echo "    ${C_WARN}!${C_OFF} $*"; }
bad()  { echo "    ${C_ERR}✗${C_OFF} $*"; }

usage() { awk 'NR>1 && /^#/ { sub(/^# ?/, ""); print; next } NR>1 { exit }' "${BASH_SOURCE[0]}"; exit 0; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --allow-known-fail) ALLOW_KNOWN=1 ;;
        --strict)           ALLOW_KNOWN=0 ;;   # 兼容旧参数：现在这就是默认行为
        --rust-only)        ;;                 # 兼容旧参数（现在本来就只跑 Rust）
        --gui)              SHOW_GUI=1 ;;
        -h|--help)          usage ;;
        *) bad "未知参数：$1（--help 看用法）"; exit 2 ;;
    esac
    shift
done

if [[ $SHOW_GUI -eq 1 ]]; then
    step "控制台 GUI 测试（需在 Windows 上跑）"
    cat <<'EOF'
    在 **Windows 终端**直接跑（各自独立，输出实时可见）：
        python scripts\tests\tray_e2e.py            # 托盘/窗口/子进程全链路（约 2 分钟）
        python scripts\tests\tray_click_probe.py    # 托盘点击链路
        python scripts\tests\ui_checks.py           # 控制台 UI（导航/FM/回放/配置器/导弹页签）
        python scripts\tests\update_check.py 8799   # FM 数据库更新面板（P3；需先起一个控制台实例）
        python scripts\tests\layout_check.py        # 配置器布局（溢出/等高/一屏放下 + FM 真渲染）
        python scripts\tests\tab_check.py           # 页签切换（active 面板 / JS 异常 / FM canvas）
        python scripts\tests\security_check.py      # 安全回归（Origin/Content-Type/体积/路径解码）
        python scripts\tests\window_flash_check.py  # 调 FM 时不闪控制台黑框
        python scripts\tests\api_parity.py selftest # API 与黄金样本比对
        python scripts\tests\ooda.py                # 前端 DOM/JS 自检（Playwright）
        python scripts\tests\window_size.py         # 窗口比例检查（16:9）
        python scripts\tests\diag_console.py        # 对着**已在运行**的控制台做体检（只读，不拉新实例）

    注意：
    * 每个脚本失败都会以非 0 退出码结束；脚本启动前请确认没有别的 wp8f-gui 在跑
      （单实例判据是端口独占，但探针与用户控制台共用 logs/console-host.json 与日志 →
      探针会先查"本机有没有别的 wp8f-gui.exe"，有就报"已有控制台在跑"并退出码 2）。
      例外：`update_check.py <端口>` 要**自己指定一个已经起好的控制台实例**（例如
      `wp8f-gui.exe --port 8799 --no-window`），它只读接口 + 点「检查更新」（一次网络请求），
      **不会点「开始下载」**（那会真下 361.6 MB）。
    * 除 diag_console.py（只读）外，别的脚本都会拉起 wp8f-gui.exe / wp8f.exe：
      tray_e2e.py 与 tray_click_probe.py 会真跑一次 HUD（退出时还原 config/*.json），
      ui_checks.py / security_check.py 会写 logs/ 下的上传样本（用例结束自行删除），
      layout_check.py / tab_check.py / ui_checks.py / ooda.py 的截图统一落在 logs/_probe/。
    * 别用 PowerShell 的 `| Out-String`/`$(...)` 去「捕获」这些脚本的输出 ——
      它们会拉起 wp8f-gui.exe，子进程继承 stdout 管道会让捕获一直等到 GUI 退出（表现为"卡住"）。
EOF
    exit 0
fi

step "Rust 单测（Linux 本机）"
command -v cargo >/dev/null || { bad "找不到 cargo"; exit 2; }

# 白名单已清空：唯一的已知失败（干扰弹重量口径：flare+chaff）在 2026-09 对齐后转绿。
# 机制保留 —— 将来出现"确实与本轮无关的历史失败"时可显式登记，但默认严格：
# 白名单里的失败同样影响退出码（避免 CI 假绿）。
KNOWN_FAIL=""
KNOWN_FAIL_MAX=0
TEST_OUT="$(cargo test --offline --release \
    -p wp8f-core -p wp8f-disp -p wp8f-logger -p wp8f-flightmodel -p datalink -p wp8f-ring 2>&1)"
# ⚠️ 上面这条用的是**默认 feature**（= `fm-json`，见 flightmodel/Cargo.toml）：
# 用户手上的 `resource/data` 自 2.59.0.43 起是 JSON 版 blkx，默认构建必须能读它。
# 必须紧跟捕获语句取值：编译/链接失败时 cargo 只把错误打到输出里，
# 两类计数都是 0 时必须失败（否则会打印「全部通过：0 个用例」并 exit 0）。
RC=$?
if [[ $RC -ne 0 ]]; then
    bad "cargo test 退出码 $RC —— 编译/链接失败（不是测试失败），最后 40 行如下："
    tail -n 40 <<<"$TEST_OUT" | sed 's/^/        /'
    echo; echo "${C_ERR}失败${C_OFF}"
    exit 1
fi
# 注意：别把这个变量叫 GROUPS —— 那是 bash 的特殊变量（用户组列表），
# 赋值会被静默忽略，展开出来是 gid（曾因此把 14 个测试组显示成 1000）。
# SUITES = 汇总行（test result: ok/FAILED. …）条数；UNITS = 真正的用例行条数。
# 用例行形如 "test tests::foo ... ok"，汇总行是 "test result: ok. 12 passed"，
# 用 '^test <名字> ... ' 的前缀把汇总行排除掉（旧口径 ^test [^ ] 把 14 行汇总也算成用例）。
SUITES=$(grep -cE '^test result:' <<<"$TEST_OUT" || true)
UNITS=$(grep -cE '^test .+ \.\.\. ' <<<"$TEST_OUT" || true)
# 只取单测失败行（"test xxx ... FAILED"）；不要匹配汇总行 "test result: FAILED. ..."
FAILS=$(grep -F ' ... FAILED' <<<"$TEST_OUT" || true)
FAIL_COUNT=$(grep -c . <<<"$FAILS" || true)
UNKNOWN=0
while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    # 白名单为空时 [[ str == *""* ]] 恒真 —— 会把任意失败都当"已知历史失败"。
    # 加 -n 前置条件，避免将来调大 KNOWN_FAIL_MAX 时把真失败放行。
    if [[ -n "$KNOWN_FAIL" && "$line" == *"$KNOWN_FAIL"* ]]; then continue; fi
    UNKNOWN=1
done <<<"$FAILS"

# 即使 cargo 退出码为 0，也要确认真的解析到了测试结果：
# cargo 输出格式变化 / 包名写错导致一个用例都没跑时，绝不能静默变绿。
if [[ $SUITES -eq 0 || $UNITS -eq 0 ]]; then
    bad "没有解析到任何测试结果（用例 $UNITS 个 / 测试组 $SUITES 组）—— cargo 输出格式可能已变，请检查上面的输出"
    echo; echo "${C_ERR}失败${C_OFF}"
    exit 1
fi

# 默认 feature 是 `fm-json`（用户手上的 resource/data 自 2.59.0.43 起就是 JSON 版 blkx），
# 上面那轮覆盖 JSON 路径；这一轮叠加 `fm-legacy`（两个 feature 都开）：
#   * legacy 解析器只有显式打开才编译 —— 不在这里跑一次，它会悄悄烂掉；
#   * `test_p2_parity::vs_json` 的"legacy ↔ JSON 键值等价"回归**要求两个 feature 同时打开**
#     才会被编译（`#[cfg(all(feature = "fm-legacy", feature = "fm-json"))]`）。
# 注意：**legacy-only**（`--no-default-features --features fm-legacy`）不在 CI 里跑 ——
# 现在的数据是 JSON，legacy-only 构建读不了它，数据类用例必红（这是预期，不是回归）。
step "Rust 单测：fm-legacy 叠加（两条解析路径的键值等价回归）"
JSON_OUT="$(cargo test --offline --release -p wp8f-flightmodel \
    --features fm-legacy --no-fail-fast 2>&1)"
RC=$?
if [[ $RC -ne 0 ]]; then
    bad "cargo test --features fm-legacy 退出码 $RC —— 编译/链接失败（不是测试失败），最后 40 行如下："
    tail -n 40 <<<"$JSON_OUT" | sed 's/^/        /'
    echo; echo "${C_ERR}失败${C_OFF}"
    exit 1
fi
JSON_UNITS=$(grep -cE '^test .+ \.\.\. ' <<<"$JSON_OUT" || true)
JSON_FAILS=$(grep -F ' ... FAILED' <<<"$JSON_OUT" || true)
if [[ -n "$JSON_FAILS" ]]; then
    bad "fm-legacy 叠加组合有失败用例："; sed 's/^/        /' <<<"$JSON_FAILS"
    echo; echo "${C_ERR}失败${C_OFF}"
    exit 1
fi
if [[ $JSON_UNITS -eq 0 ]]; then
    bad "fm-legacy 叠加组合没有解析到任何用例 —— cargo 输出格式可能已变，绝不能静默变绿"
    echo; echo "${C_ERR}失败${C_OFF}"
    exit 1
fi
ok "fm-legacy 叠加组合（fm-json + fm-legacy 两条路径）：$JSON_UNITS 个用例全部通过"

if [[ -z "$FAILS" ]]; then
    ok "全部通过：$UNITS 个用例 / $SUITES 个测试组"
    echo; echo "${C_OK}通过${C_OFF}"
    exit 0
elif [[ $UNKNOWN -eq 0 && $FAIL_COUNT -le $KNOWN_FAIL_MAX ]]; then
    warn "仅已知历史失败：$KNOWN_FAIL（预先存在，与本轮改动无关）"
    echo "    其余 $UNITS 个用例全部通过"
    if [[ $ALLOW_KNOWN -eq 1 ]]; then
        echo; echo "${C_WARN}通过（--allow-known-fail）${C_OFF}"; exit 0
    fi
    echo "    需要本地放行请加 --allow-known-fail；CI 应保持默认（严格）"
    echo; echo "${C_ERR}失败${C_OFF}"; exit 1
else
    bad "失败用例："; sed 's/^/        /' <<<"$FAILS"
    echo; echo "${C_ERR}失败${C_OFF}"
    exit 1
fi
