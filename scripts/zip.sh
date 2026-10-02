#!/usr/bin/env bash
# =============================================================================
# wp8f 打包：解压后双击 wp8f-gui.exe 就能跑（产出在仓库根）。
#
#   bash scripts/zip.sh              # 打包（含 resource/data），产出 wp8f-<日期>.zip
#   bash scripts/zip.sh -h           # 打印本说明
#
# 清单 = 三组：
#   ① 可执行文件：wp8f-gui.exe（根，唯一入口）与它旁边的 WebView2Loader.dll（GNU 目标的
#      WebView2 loader，缺了双击就报错，见 issue #1）、binary/{wp8f,flightmodel,test-server}.exe
#      + test-server/scenarios；
#   ② 运行期读的磁盘资源：gui/static、resource/fonts（只带随包两份字体 + OFL 文本）、
#      resource/voice（按包：`<语音包>/*.wav`）、resource/lang（HUD 标签表）、
#      resource/i18n（十五种界面语言，Rust 与前端共用）、resource/data（飞机性能数据，约 540 MB）；
#   ③ LICENSE / **config/*.json（布局配置，控制台与 HUD 都用）**
#      + 打包时生成的 VERSION.txt（构建日期）。
# 路径口径：`resource/**` 按仓库根定位；`test-server/scenarios` 按 cwd（控制台把 cwd 设成 test-server/）。
#
# 排除项（第 3 步有反查）：
#   * target/（构建中间产物，包里的 exe 已由 build.sh 拷好）；
#   * logs/（飞行记录与日志等运行期产物）；
#   * resource/{data_new,data_old}/ 与 .wp8f_update_tmp/（更新器的暂存与回滚目录）；
#   * resource/fonts/ 下除 FONT_FILES 之外的一切（那里是用户自己放字体的地方）。
#
# 字体只认 FONT_FILES 三个文件（名字以 `disp/src/font.rs` 为准，改代码要同步改这里）：
# 字体不再内嵌，缺了解压出来一跑就退出。
#
# 数据：`resource/data` 是上游 War-Thunder-Datamine（无许可证）的产物，不在 git 里，
# 约 540 MB / 26000 文件 —— 按用户口径**始终打进包**（它是必需件，缺件直接失败）。
# 合规提醒：上游没有提供许可证，含数据的包**仅限自用/私下分发**，不要公开发布。
#
# 硬规则：① 先 rm 目标 zip（`zip -r` 是更新语义，同名包会残留已删文件）；
#   ② 显式清单，不整目录通吃；③ 打完用 `unzip -Z1` 逐项校验，缺任何一项非 0 退出；
#   ④ 先打到临时目录、校验通过才 mv 到仓库根（zip 会在输出目录建临时文件）。
#
# 退出码：0 成功；1 缺件/校验失败；2 环境不满足（没有 zip/unzip / 参数不对）
# =============================================================================
set -uo pipefail

cd "$(dirname "$0")/.."

usage() { awk 'NR>1 && /^#/ { sub(/^# ?/, ""); print; next } NR>1 { exit }' "${BASH_SOURCE[0]}"; exit 0; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        -h|--help)   usage ;;
        *) echo "✗ 未知参数：$1（--help 看用法）" >&2; exit 2 ;;
    esac
    shift
done

# 字体：**显式列文件**，不带 `resource/fonts` 整目录 ——
# 那目录是用户放自备字体的地方，整目录打包会把来源/许可不明的字体（D20）发出去。
# 文件名与 `disp/src/font.rs` 的 DEFAULT_TEXT_FILE / ICON_FILE 同源。
FONT_FILES=(
    "resource/fonts/SarasaMonoSC-Regular.ttf"   # 默认正文字体（HUD 启动时从磁盘读入）
    "resource/fonts/icons.ttf"                  # 图标字体（地图 POI 等）
    "resource/fonts/OFL.txt"                    # 上面那份字体的 OFL 许可文本（随字体必须带）
)

# 必需件：缺任何一个都直接失败，不做"残包"（能解压但少了关键功能最坑人）
REQUIRED=(
    "wp8f-gui.exe"                          # 控制台（仓库根唯一入口：托盘 + API + WebView2 窗口）
    "WebView2Loader.dll"                    # 上面那个 exe 的运行期依赖，必须同目录（GNU 目标，见 issue #1）
    "binary/wp8f.exe"                       # HUD 主程序
    "binary/flightmodel.exe"                # FM 解析/曲线（控制台「飞行模型」页签直接调它）
    "binary/test-server.exe"                # 模拟服务端（控制台拖拽预览用）
    "binary/datalink-server.exe"            # 数据链服务端（Windows；给有公网 IP 的那台跑）
    "binary/datalink-test-client.exe"       # 数据链测试客户端（连不通时先用它验证链路）
    "binary/datalink-server"                # 数据链服务端（Linux；服务端多跑在 Linux 上）
    "binary/datalink-test-client"           # 数据链测试客户端（Linux）
    "gui/static"                            # 控制台前端（index.html/style.css/前端 js/vendor）
    "test-server/scenarios"                 # 模拟服务端场景（按 cwd 读，留在原位）
    "${FONT_FILES[@]}"                      # HUD 字体（只带这三个，见上）
    "resource/voice"                        # 语音包**按目录**打（每个子目录 = 一个语音包，D 起）
    "resource/lang"                         # HUD 的标签表（中文/英文；C 起与 i18n 分开）
    "resource/i18n"                         # 十五种语言文案（P4/P6；缺了界面以键名上屏）
    "LICENSE"
    "config"                                # 布局配置：config/*.json 全部随包（GUI 与 HUD 都读它）
    "resource/data"                        # 飞机性能数据（约 540 MB，始终随包，见文件头「数据」）
)
# 缺数据时给一条指路提示用（数据不在 git 里，刚 clone 的仓库没有）
DATA_ITEM="resource/data"

# ---- 1) 打包前先查清单里的文件在不在（别打出一个缺件的包） ----
missing=()
for item in "${REQUIRED[@]}"; do
    [[ -e "$item" ]] || missing+=("$item")
done
if (( ${#missing[@]} > 0 )); then
    echo "✗ 打包中止：以下必需项在仓库里不存在" >&2
    for m in "${missing[@]}"; do echo "    - $m" >&2; done
    # 缺可执行文件 = 还没构建（或只跑过 --webui-only，那个模式不产 binary/ 里的三个）
    for m in "${missing[@]}"; do
        if [[ "$m" == "wp8f-gui.exe" || "$m" == "WebView2Loader.dll" || "$m" == binary/* ]]; then
            cat >&2 <<'EOS'
    → 这几项是构建产物、不是源码，先跑 `bash scripts/build.sh`：
      wp8f-gui.exe 与 WebView2Loader.dll 落在**仓库根**（两者必须同目录），
      wp8f.exe / flightmodel.exe / test-server.exe 落在 **binary/**（S1 布局）。
      只跑过 `bash scripts/build.sh --webui-only` 的话，只有仓库根这两项存在。
EOS
            break
        fi
    done
    # resource/data 是最常见的缺件：它是用户本地数据，不在 git 里
    for m in "${missing[@]}"; do
        if [[ "$m" == "$DATA_ITEM" ]]; then
            cat >&2 <<'EOS'
    → resource/data 是**你本地的**飞机性能数据（.blkx/JSON 等），不在 git 里，
      刚 clone 出来的仓库没有它。GUI 的「飞行模型」页签要用它（HUD 也要）。
      数据根已扁平化：gamedata/、weapons/、templates/、version 都直接在 resource/data/ 下，
      没有中间的 aces/。从旧版本升级：上一版在 resource/data/aces/，更早是
      resource/fm/data 或 flightmodel/data，都需手动提到 resource/data（这一步不会自动做）。
      （Windows PowerShell: Move-Item resource\data\aces\* resource\data\；
        WSL:                mv resource/data/aces/* resource/data/）。
EOS
        fi
    done
    exit 1
fi

command -v zip >/dev/null 2>&1   || { echo "✗ 找不到 zip（WSL/Ubuntu: sudo apt install zip）" >&2; exit 2; }
command -v unzip >/dev/null 2>&1 || { echo "✗ 找不到 unzip（校验包内容要用：sudo apt install unzip）" >&2; exit 2; }

# ---- 1.5) 非系统 DLL 依赖审计 ----
# 显式清单是人手维护的，漏一个**运行期 DLL** 的代价是用户双击直接报错（issue #1 的
# WebView2Loader.dll 就是这么漏的：GNU 目标的 GUI 动态依赖它，zip 的实参列表里却没有）。
# 有 objdump 就把每个 exe 的导入表列出来，凡不是系统 DLL、又不在清单里的直接失败。
OBJDUMP="${OBJDUMP:-x86_64-w64-mingw32-objdump}"
# 系统 DLL（Windows 自带 + 编译器随产物静态链进去的那几类），不随包分发
SYS_DLL_RE='^(kernel32|user32|gdi32|advapi32|shell32|ole32|oleaut32|comctl32|shlwapi|ws2_32|ntdll|msvcrt|dwmapi|uxtheme|imm32|bcryptprimitives|userenv|dbghelp|version|winmm|psapi|crypt32|iphlpapi|secur32|setupapi|opengl32|ucrtbase|api-ms-win-[a-z0-9-]+|vcruntime[0-9_]*|msvcp[0-9_]*|libgcc_s_[a-z0-9_-]+|libwinpthread-[0-9]+)\.dll$'
in_manifest() {   # 这个 DLL 是否随包（按文件名比对清单，路径无所谓）
    local base item
    base="$(basename "$1")"
    for item in "${REQUIRED[@]}"; do
        [[ "$(basename "$item")" == "$base" ]] && return 0
    done
    return 1
}
if command -v "$OBJDUMP" >/dev/null 2>&1; then
    audit_bad=()
    for exe in wp8f-gui.exe binary/*.exe; do
        [[ -f "$exe" ]] || continue
        deps="$("$OBJDUMP" -p "$exe" 2>/dev/null | sed -n 's/^[[:space:]]*DLL Name:[[:space:]]*//p' | sort -u)"
        while IFS= read -r dll; do
            [[ -n "$dll" ]] || continue
            grep -qiE "$SYS_DLL_RE" <<<"$dll" && continue
            in_manifest "$dll" || audit_bad+=("$exe 依赖 $dll（不在清单里）")
        done <<<"$deps"
    done
    # loader 拿错架构比缺文件更难查：顺手确认它是 x64 PE
    if [[ -f WebView2Loader.dll ]] && ! "$OBJDUMP" -f WebView2Loader.dll 2>/dev/null | grep -q 'pei-x86-64'; then
        audit_bad+=("WebView2Loader.dll 不是 x64 PE（架构不对，GUI 会加载失败）")
    fi
    if (( ${#audit_bad[@]} > 0 )); then
        echo "✗ 打包中止：以下非系统 DLL 依赖没有随包" >&2
        for a in "${audit_bad[@]}"; do echo "    - $a" >&2; done
        cat >&2 <<'EOS'
    → 真要随包：build.sh 里部署到 exe 旁边 + 本脚本 REQUIRED 里登记；
      确实是系统自带：补进本段的 SYS_DLL_RE。
EOS
        exit 1
    fi
    echo "· DLL 依赖审计通过（$(ls wp8f-gui.exe binary/*.exe 2>/dev/null | wc -l) 个 exe，非系统依赖全部在清单内）"
else
    echo "! 找不到 $OBJDUMP，跳过 DLL 依赖审计（apt install binutils-mingw-w64-x86-64）" >&2
fi

# ---- 2) 打新包（先删旧包杜绝残留条目；先在临时目录成型再 mv 到位） ----
OUT="wp8f-$(date +%Y%m%d).zip"
rm -f "$OUT"
STAGE="$(mktemp -d "${TMPDIR:-/tmp}/wp8f-pack.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
TMP_OUT="$STAGE/$OUT"

# 打包时生成的条目（不进仓库）：VERSION.txt = 这一包的构建日期（包名里也有）
GEN_DIR="$STAGE/pkg"
mkdir -p "$GEN_DIR"
printf 'wp8f build %s\n' "$(date +%Y-%m-%d)" > "$GEN_DIR/VERSION.txt"

echo "· 含 $DATA_ITEM（$(du -sh "$DATA_ITEM" 2>/dev/null | cut -f1)）：该数据来自 War-Thunder-Datamine"
echo "  （上游无许可证）—— 含数据的包仅限自用/私下分发，不要公开发布。"
echo "打包 → $OUT（先落到 $STAGE，校验通过后移入仓库根）"
zip -q -r "$TMP_OUT" "${REQUIRED[@]}"
# 追加打包时生成的两个条目（cwd 换到 $GEN_DIR，让包内路径就是 VERSION.txt / config/default.json）
( cd "$GEN_DIR" && zip -q -r "$TMP_OUT" VERSION.txt )

# ---- 3) 校验：清单里每一项都要真的在包里 ----
LIST="$(unzip -Z1 "$TMP_OUT")"
bad=()
for item in "${REQUIRED[@]}" "VERSION.txt"; do
    # 文件按整行精确匹配；目录（清单里是目录名）按 "名字/" 前缀匹配
    if grep -qxF -- "$item" <<<"$LIST" || grep -qF -- "$item/" <<<"$LIST"; then
        continue
    fi
    bad+=("$item")
done

# 反向检查：私有/中间产物绝不能混进包里。
# 注意 `binary/` **不在**这个正则里：S1 起它是 exe 的正规去处（见 REQUIRED）。
# `resource/data_old/` = 更新器换根后留下的**上一版**（固定单槽）。
LEAK_RE='^(logs/|target/|resource/data_new/|resource/data_old/|resource/\.wp8f_update_tmp/)'
leaked="$(grep -E "$LEAK_RE" <<<"$LIST" || true)"
# resource/fonts/ 下**只允许** FONT_FILES 三个文件：用户自备字体（来源/许可不明）不进包。
# 这条是反查，防的是"将来有人把 REQUIRED 改回整目录"——清单对了不代表包里就对。
allow_re="$(printf '%s\n' "${FONT_FILES[@]}" | sed 's/\./\\./g' | paste -sd'|' -)"
font_leaked="$(grep -E '^resource/fonts/' <<<"$LIST" | grep -vE "^(${allow_re})$" || true)"
if [[ -n "$font_leaked" ]]; then
    leaked="$(printf '%s\n%s\n' "$leaked" "$font_leaked" | grep . || true)"
fi
# resource/voice/ 下**只允许两种条目**：`<语音包>/`（目录，D 起语音按包放）与目录项本身。
# 反查的是"散在语音根目录上的 wav"——那些**不会被加载**（`voice_path` 指的是一个子目录），
# 打进包里只会让人以为有声音。控制台的语音包面板也会把 loose 列出来。
voice_loose="$(grep -E '^resource/voice/[^/]+$' <<<"$LIST" | grep -viE '^resource/voice/[^/]+/$' || true)"
if [[ -n "$voice_loose" ]]; then
    leaked="$(printf '%s\n%s\n' "$leaked" "$voice_loose" | grep . || true)"
fi

if (( ${#bad[@]} > 0 )) || [[ -n "$leaked" ]]; then
    if (( ${#bad[@]} > 0 )); then
        echo "✗ 包内缺少以下必需项：" >&2
        for m in "${bad[@]}"; do echo "    - $m" >&2; done
    fi
    if [[ -n "$leaked" ]]; then
        echo "✗ 包里混进了不该打包的内容：" >&2
        sed 's/^/    - /' <<<"$leaked" >&2
    fi
    echo "✗ 校验未通过：没有产出 $OUT（临时包随 trap 一起删掉）" >&2
    exit 1
fi

mv -f "$TMP_OUT" "$OUT"        # 只有校验通过才在仓库根出现成品
entries=$(grep -c . <<<"$LIST" || true)
size=$(du -h "$OUT" 2>/dev/null | cut -f1)
echo "✓ 校验通过：$entries 个条目，${size:-?}"
echo "  内容清单（顶层）："
cut -d/ -f1 <<<"$LIST" | sort -u | sed 's/^/    /'
echo "Done: $OUT"
