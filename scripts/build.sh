#!/usr/bin/env bash
# =============================================================================
# wp8f 交叉编译脚本（在 Linux / WSL 下跑，产出 Windows 可执行文件）
#
#   wp8f-gui.exe            控制台（托盘 + 本地 HTTP API + wry/WebView2 窗口），仓库根唯一入口
#   WebView2Loader.dll      上面那个控制台的运行期依赖（GNU 目标动态链接 loader，MSVC 才静态），
#                           必须与 exe 同目录 —— 同样落在仓库根
#   binary/wp8f.exe         HUD 主程序（core + disp）
#   binary/flightmodel.exe  FM 解析/曲线（GUI 直接读 binary/）
#   binary/test-server.exe  模拟游戏服务端（拖拽预览用）
#   binary/datalink-*.exe   数据链服务端与测试客户端（Windows）
#   binary/datalink-*       同上，Linux 版（服务端多跑在 Linux 上，跟随 `LINUX_TARGET`）
#   binary/wp8f             Linux 本机构建的 HUD（--native / --linux-only）
#
# 其余产物一律在 `binary/`；`test-server/scenarios` 与 `resource/**` 原位不动
#（前者由 test-server 按 cwd 解析，后者由 HUD/GUI 按仓库根定位）。
# 默认 feature `wp8f-flightmodel/fm-json`；legacy 解析器仅作备份，要 `fm-legacy` 才编译。
#
# 用法：
#   scripts/build.sh                 # 默认：交叉编译 Windows 全套 + 两个平台的数据链服务端
#   scripts/build.sh --native        # 同时构建本机 Linux 版本（binary/wp8f）
#   scripts/build.sh --linux-only    # 只构建 Linux 版本
#   scripts/build.sh --webui-only    # 只构建控制台（仓根 wp8f-gui.exe）
#   scripts/build.sh --debug         # debug 构建
#   scripts/build.sh --clean         # 先 cargo clean
#   scripts/build.sh --install-tools # 缺 rustup target 时自动安装
#   scripts/build.sh -j 8 --online   # 并行度 / 允许联网拉依赖（默认 --offline）
#
# 退出码：0 成功；非 0 表示有构建步骤失败
# =============================================================================
set -euo pipefail

# ---------------------------------------------------------------- 常量/参数 --
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

WIN_TARGET="x86_64-pc-windows-gnu"
LINUX_TARGET="x86_64-unknown-linux-gnu"
TARGET="$WIN_TARGET"
PROFILE="release"
PROFILE_FLAG="--release"
JOBS=""
CARGO_NET="--offline"
BUILD_WIN=1
BUILD_LINUX=0
# 数据链服务端：默认两个平台都编（服务端多跑在 Linux 上），跟随各平台开关收窄：
# `--windows-only` 只留 Windows 版，`--linux-only` 只留 Linux 版，`--webui-only` 两个都不编。
DL_WIN=1
DL_LINUX=1
WEBUI_ONLY=0
DO_CLEAN=0
INSTALL_TOOLS=0

# 需要构建的 Rust 包（wp8f-core 是 HUD 主程序；-p disp 显式带上）
WIN_PKGS=(wp8f-core wp8f-disp wp8f-flightmodel test-server wp8f-gui)
WEBUI_PKGS=(wp8f-gui)
LINUX_PKGS=(wp8f-core wp8f-disp wp8f-flightmodel test-server)

C_OK=$'\033[32m'; C_WARN=$'\033[33m'; C_ERR=$'\033[31m'; C_HL=$'\033[36m'; C_OFF=$'\033[0m'
step()  { echo; echo "${C_HL}==> $*${C_OFF}"; }
ok()    { echo "    ${C_OK}✓${C_OFF} $*"; }
warn()  { echo "    ${C_WARN}!${C_OFF} $*"; }
fail()  { echo "    ${C_ERR}✗${C_OFF} $*" >&2; }

usage() { awk 'NR>1 && /^#/ { sub(/^# ?/, ""); print; next } NR>1 { exit }' "${BASH_SOURCE[0]}"; exit 0; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --native)        BUILD_LINUX=1 ;;
        --linux-only)    BUILD_WIN=0; BUILD_LINUX=1; DL_WIN=0 ;;
        --windows-only)  BUILD_WIN=1; BUILD_LINUX=0; DL_LINUX=0 ;;
        --webui-only)    WEBUI_ONLY=1; DL_WIN=0; DL_LINUX=0 ;;
        --debug)         PROFILE="debug"; PROFILE_FLAG="" ;;
        --clean)         DO_CLEAN=1 ;;
        --offline)       CARGO_NET="--offline" ;;
        --online)        CARGO_NET="" ;;
        --install-tools) INSTALL_TOOLS=1 ;;
        -j|--jobs)       JOBS="${2:?$1 需要参数}"; shift ;;
        --target)        TARGET="${2:?--target 需要参数}"; shift ;;
        -h|--help)       usage ;;
        *) fail "未知参数：$1（--help 看用法）"; exit 2 ;;
    esac
    shift
done

# --webui-only：只构建控制台两个进程（Windows 目标）
if [[ $WEBUI_ONLY -eq 1 ]]; then
    BUILD_WIN=1
    BUILD_LINUX=0
fi

have() { command -v "$1" >/dev/null 2>&1; }
size_of() { [[ -f "$1" ]] && du -h "$1" 2>/dev/null | cut -f1 || echo "?"; }
# S1 布局迁移：旧版本的产物在仓库根（或 test-server/ 里），根目录现在只保留 wp8f-gui.exe。
# 旧副本会让人双击到上一次构建的 exe，所以产出新位置后清掉；
# 删不掉（文件正在运行 / 权限）只警告——构建本身没失败，别把整个脚本拖挂。
drop_legacy() {
    for f in "$@"; do
        [[ -e "$f" ]] || continue
        if rm -f "$f" 2>/dev/null; then
            ok "已清理旧布局产物 $f（S1：它现在在 binary/）"
        else
            warn "旧布局产物 $f 删不掉（可能正在运行）——请手动删除"
        fi
    done
}
T0=$SECONDS

# ------------------------------------------------------------ 0. 环境检查 --
step "0/4 环境检查"
have cargo  || { fail "找不到 cargo（装 Rust: https://rustup.rs）"; exit 1; }
ok "cargo $(cargo --version | awk '{print $2}')  rustc $(rustc --version | awk '{print $2}')"
have rustup || warn "没有 rustup，跳过 target 检查"

if have rustup; then
    if ! rustup target list --installed | grep -qx "$WIN_TARGET"; then
        if [[ $INSTALL_TOOLS -eq 1 && $BUILD_WIN -eq 1 ]]; then
            warn "安装 target $WIN_TARGET …"; rustup target add "$WIN_TARGET"
        elif [[ $BUILD_WIN -eq 1 ]]; then
            fail "缺少 target $WIN_TARGET（rustup target add $WIN_TARGET，或加 --install-tools）"; exit 1
        fi
    fi
    [[ $BUILD_WIN -eq 1 ]] && ok "Windows target: $WIN_TARGET"
    if [[ $BUILD_LINUX -eq 1 ]] && ! rustup target list --installed | grep -qx "$LINUX_TARGET"; then
        warn "缺少 target $LINUX_TARGET（本机通常已随 rustup 安装）"
    fi
fi

if [[ $BUILD_WIN -eq 1 ]] && have rustup; then
    if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
        warn "找不到 x86_64-w64-mingw32-gcc（链接 DLL 可能需要：apt install mingw-w64）"
    else
        ok "mingw-w64 工具链就绪"
    fi
fi

[[ $DO_CLEAN -eq 1 ]] && { step "cargo clean"; cargo clean; }

# ------------------------------------------------------------- 1. Rust 构建 --
COMMON=(build)
[[ -n "$CARGO_NET" ]] && COMMON+=("$CARGO_NET")
# 语音由运行期配置项 voice_warnings_enabled 控制，不是 cargo feature。
# `fm-json` 是默认 feature（上游下载的数据与用户手上的 `resource/data` 都是 JSON 版），
# 这里显式再声明一次，保证改默认值也不会让发布产物读不了 JSON。
# `fm-legacy` 仅作备份、不参与发布；两个都开时按内容分派（首个非空字节 `{` 即 JSON）。
FM_JSON_FEAT=(--features wp8f-flightmodel/fm-json)
if [[ "$PROFILE" == "release" ]]; then
    COMMON+=("--release")            # --release 与 --profile 互斥
else
    COMMON+=("--profile" "$PROFILE")
fi
[[ -n "$JOBS" ]] && COMMON+=("-j" "$JOBS")

if [[ $BUILD_WIN -eq 1 ]]; then
    step "1/3 Rust：交叉编译 Windows 全套（$WIN_TARGET, $PROFILE）"
    if [[ $WEBUI_ONLY -eq 1 ]]; then
        PKG_LIST=("${WEBUI_PKGS[@]}")
    else
        PKG_LIST=("${WIN_PKGS[@]}")
    fi
    PKG_ARGS=(); for p in "${PKG_LIST[@]}"; do PKG_ARGS+=("-p" "$p"); done
    echo "    cargo ${COMMON[*]} ${FM_JSON_FEAT[*]} --target $WIN_TARGET ${PKG_ARGS[*]}"
    cargo "${COMMON[@]}" "${FM_JSON_FEAT[@]}" --target "$WIN_TARGET" "${PKG_ARGS[@]}"
    WIN_OUT="target/$WIN_TARGET/$PROFILE"

    # WebView2 loader：webview2-com-sys 只在 **MSVC** 目标静态链接 loader，非 MSVC（我们）生成的是
    # 对 WebView2Loader.dll 的**普通导入**，所以它必须和 wp8f-gui.exe 同目录。cargo 只把 SDK 里那份
    # 复制进依赖自己的 OUT_DIR，不会放到 exe 旁边 —— 少了它用户双击就报"找不到 WebView2Loader.dll"
    # （issue #1）。开发机 PATH 里若恰好有别人的副本（如 Windows Performance Toolkit）会掩盖缺失，
    # 所以这一步是硬要求，不做"找不到就算了"。
    loader_src=""
    while IFS= read -r -d '' candidate; do
        if [[ -z "$loader_src" ]]; then
            loader_src="$candidate"
        elif ! cmp -s "$loader_src" "$candidate"; then
            # 依赖缓存里有多份不同内容（换过 crate 版本）：取最近构建的那份，别为这点歧义要求全量重建
            if [[ "$candidate" -nt "$loader_src" ]]; then
                loader_src="$candidate"
            fi
            warn "缓存里有多份不同的 WebView2Loader.dll，取最近构建的一份：$loader_src"
        fi
    done < <(find "$WIN_OUT/build" -type f \
        -path '*/webview2-com-sys-*/out/x64/WebView2Loader.dll' -print0)
    if [[ -z "$loader_src" ]]; then
        fail "找不到 x64 WebView2Loader.dll（GNU 目标的 GUI 运行期必需）：加 --clean 重新构建"
        exit 1
    fi
    cp -f "$loader_src" WebView2Loader.dll
    ok "WebView2Loader.dll           $(size_of WebView2Loader.dll)（x64，与 GUI 同目录）"

    if [[ $WEBUI_ONLY -eq 0 ]]; then
        mkdir -p binary
        cp -f "$WIN_OUT/wp8f-core.exe" binary/wp8f.exe
        ok "binary/wp8f.exe            $(size_of binary/wp8f.exe)"
        drop_legacy wp8f.exe
        if [[ -f "$WIN_OUT/flightmodel.exe" ]]; then
            cp -f "$WIN_OUT/flightmodel.exe" binary/flightmodel.exe
            ok "binary/flightmodel.exe     $(size_of binary/flightmodel.exe)"
        else
            warn "未产出 flightmodel.exe"
        fi
        if [[ -f "$WIN_OUT/test-server.exe" ]]; then
            cp -f "$WIN_OUT/test-server.exe" binary/test-server.exe
            ok "binary/test-server.exe     $(size_of binary/test-server.exe)"
            drop_legacy test-server/test-server.exe
        else
            warn "未产出 test-server.exe"
        fi
    fi
    # 控制台是唯一"人可能正在双击运行"的产物：Windows 上占用中的 exe 覆盖会失败
    # （WSL1 只报 `cp: cannot remove ...: Input/output error`，看不出真正原因）→ 明说怎么办。
    if ! cp -f "$WIN_OUT/wp8f-gui.exe" wp8f-gui.exe 2>/dev/null; then
        fail "写入 wp8f-gui.exe 失败：它多半正在运行 —— 先退出控制台（托盘右键 → 退出）再重跑"
        fail "本次新编出来的控制台在 $WIN_OUT/wp8f-gui.exe，其它产物已就位"
        exit 1
    fi
    ok "wp8f-gui.exe                $(size_of wp8f-gui.exe)（控制台：托盘 + API + WebView2 窗口；唯一入口）"
    if [[ $WEBUI_ONLY -eq 1 ]]; then
        echo; echo "${C_OK}控制台构建完成${C_OFF}（wp8f-gui.exe）"
        exit 0
    fi
fi

if [[ $BUILD_LINUX -eq 1 ]]; then
    step "2/3 Rust：本机构建 Linux 版本"
    PKG_ARGS=(); for p in "${LINUX_PKGS[@]}"; do PKG_ARGS+=("-p" "$p"); done
    cargo "${COMMON[@]}" "${FM_JSON_FEAT[@]}" "${PKG_ARGS[@]}"
    LIN_OUT="target/$PROFILE"
    mkdir -p binary
    cp -f "$LIN_OUT/wp8f-core" binary/wp8f
    ok "binary/wp8f                 $(size_of binary/wp8f)"
    drop_legacy wp8f
    if [[ -f "$LIN_OUT/flightmodel" ]]; then
        cp -f "$LIN_OUT/flightmodel" binary/flightmodel
        ok "binary/flightmodel          $(size_of binary/flightmodel)"
    fi
else
    step "2/3 Rust：跳过 Linux 本机构建（--native 可开启）"
fi

# ------------------------------------------------------- 2.5 数据链服务端 --
# `datalink` 的两个二进制要 `--features bins` 才编（默认只编库）。单独一步：它的 feature
# 与 `FM_JSON_FEAT` 无关，混进上面的包列表会让 cargo 报 "none of the selected packages
# contains these features"。
DL_BINS=(datalink-server datalink-test-client)
if [[ $DL_WIN -eq 1 || $DL_LINUX -eq 1 ]]; then
    step "2.5/3 Rust：数据链服务端（Windows + Linux）"
    mkdir -p binary

    if [[ $DL_WIN -eq 1 ]]; then
        cargo "${COMMON[@]}" --target "$WIN_TARGET" -p datalink --features bins
        for b in "${DL_BINS[@]}"; do
            src="target/$WIN_TARGET/$PROFILE/$b.exe"
            if [[ -f "$src" ]]; then
                cp -f "$src" "binary/$b.exe"
                ok "binary/$b.exe            $(size_of "binary/$b.exe")"
            else
                warn "未产出 $b.exe"
            fi
        done
    fi

    if [[ $DL_LINUX -eq 1 ]]; then
        # Linux 版不传 --target：直接编本机默认 target（产物在 target/$PROFILE），
        # 免得还要装一份显式 target；服务端二进制是自包含的，拷走即可用。
        cargo "${COMMON[@]}" -p datalink --features bins
        for b in "${DL_BINS[@]}"; do
            src="target/$PROFILE/$b"
            if [[ -f "$src" ]]; then
                cp -f "$src" "binary/$b"
                ok "binary/$b                $(size_of "binary/$b")"
            else
                warn "未产出 $b"
            fi
        done
    fi
fi

# ------------------------------------------------------------------ 4. 汇总 --
step "3/3 汇总（用时 $((SECONDS - T0))s）"
# 中文是双宽字符，用 %-Ns 对不齐，直接一行一项最省心
summary() {
    [[ -f "$1" ]] || return 0
    printf '    %s  (%s)  %s\n' "$1" "$(size_of "$1")" "$2"
    return 0
}
echo "    产物（本次构建）"
if [[ $BUILD_WIN -eq 1 ]]; then
    summary wp8f-gui.exe                "控制台（托盘 + API + WebView2 窗口，单进程；仓库根唯一入口）"
    summary WebView2Loader.dll          "控制台的 WebView2 loader（与上面那个 exe 同目录，随包分发）"
    summary binary/wp8f.exe             "HUD 主程序"
    summary binary/flightmodel.exe      "FM 解析（GUI 直接用）"
    summary binary/test-server.exe      "模拟服务端"
fi
[[ $BUILD_LINUX -eq 1 ]] && summary binary/wp8f "Linux HUD 主程序"
[[ $DL_WIN -eq 1 ]] && {
    summary binary/datalink-server.exe       "数据链服务端（Windows）"
    summary binary/datalink-test-client.exe  "数据链测试客户端（Windows）"
}
[[ $DL_LINUX -eq 1 ]] && {
    summary binary/datalink-server           "数据链服务端（Linux）"
    summary binary/datalink-test-client      "数据链测试客户端（Linux）"
}
cat <<EOF

    下一步（Windows 上）：
        .\\wp8f-gui.exe                   # 双击：控制台（托盘 + 配置窗口）
        .\\binary\\wp8f.exe --port 8111 …  # 直接跑 HUD（参数见 --help）

    数据链（可选，详见 README「数据链」一节）：
        .\\binary\\datalink-server.exe --port 2887 --key <密钥>   # 有公网 IP 的那台跑服务端
        .\\binary\\datalink-server --help                          # 全部参数
EOF
echo "${C_OK}构建完成${C_OFF}"
