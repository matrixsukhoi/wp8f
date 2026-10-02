# scripts —— 构建、测试、打包与 Windows 探针（`scripts/`）

## 职责

仓库的**全部工程化入口**都在这里，且只做「编排」——真正的逻辑在各 crate 与
`gui/static/` 里：

| 脚本 | 职责 | 在哪跑 |
|---|---|---|
| `build.sh` | 交叉编译 Windows 全套（+ 可选 Linux 本机版），产物落 `binary/` 与仓库根 | Linux / WSL |
| `test.sh` | 跑 Rust 单测（本机原生，最快），并打印 Windows 侧 GUI 测试清单 | Linux / WSL |
| `zip.sh` | 按显式清单打包成 `wp8f-<日期>.zip`，逐项校验 | Linux / WSL |
| `make_icon.py` | 生成 wp8f logo：`gui/static/logo.svg` + `gui/logo.ico`（需要 Pillow） | 两边都行（Windows python / WSL python3） |
| `tests/*.py` | 控制台 GUI 的端到端探针（真托盘/真窗口/真子进程） | **Windows** |

## 实现原理

* **构建（`build.sh`）**。默认目标 `x86_64-pc-windows-gnu`（mingw 交叉编译），
  包清单 `WIN_PKGS=(wp8f-core wp8f-disp wp8f-flightmodel test-server wp8f-gui)`；
  编译完把产物**拷到 S1 布局**的位置：`wp8f-gui.exe` 在仓库根（唯一入口），
  `binary/{wp8f,flightmodel,test-server}.exe`；旧布局残留（仓库根或 `test-server/` 下的同名
  exe）由 `drop_legacy` 清掉。默认 `--offline`，`fm-json` 是默认 feature
  （`fm-legacy` 只是备份解析器）。
* **WebView2 loader（构建的第二步）**。`webview2-com-sys` 只在 **MSVC** 目标静态链接 loader ——
  `WebView2LoaderStatic.lib` 是 MSVC 编的 C++ 目标文件，要 `__security_cookie`、
  `_Init_thread_*`、MSVC 的 `operator new` 这些 CRT 内部符号，GNU ld 链不了；
  非 MSVC 目标生成的是对 `WebView2Loader.dll` 的**普通导入**，运行期必须有这个文件、且与
  exe 同目录。cargo 只把它放进依赖自己的 `OUT_DIR/<arch>/`，所以 `build.sh` 负责把 x64 那份
  复制到仓库根（`--webui-only` 同样复制；缓存里多份内容不一致时取最近构建的一份并 `warn`）。
* **测试（`test.sh`）**。一条 `cargo test --offline --release -p …` 覆盖 6 个 crate，
  靠 `^test result:` / `^test … ` 两套正则分别数「汇总行」与「用例行」—— 两类计数都为 0 视为
  编译/链接失败（否则会打印「全部通过：0 个用例」并假绿）。白名单默认严格：
  `KNOWN_FAIL` 非空时才容忍登记过的历史失败。**不在 WSL 里驱动 GUI**
  （进程环境差异会让托盘/WebView2 探针不稳），`--gui` 只打印 Windows 上的跑法。
* **打包（`zip.sh`）**。四条硬规则：① 先 `rm` 目标 zip（`zip -r` 是更新语义，同名包会残留
  已删文件）；② 显式清单，不整目录通吃（尤其 `resource/fonts` 只带 `FONT_FILES` 三个文件
  —— 那是用户放自备字体的地方）；③ 先打到临时目录，`unzip -Z1` 逐项反查后才 `mv` 到仓库根；
  ④ **非系统 DLL 依赖审计**：`x86_64-w64-mingw32-objdump -p` 列出每个 exe 的导入，
  非系统 DLL 又不在 `REQUIRED` 里的直接失败（issue #1 漏掉 `WebView2Loader.dll` 就是这个缺口），
  顺带确认它是 x64 PE；没有 objdump 只 `warn`。
  `LEAK_RE` 反查泄漏项（`target/`、`logs/`、更新器暂存/回滚目录等）。
* **探针（`tests/*.py`）**。共同契约：**退出码 2 = 前置不满足**（已有控制台/`wp8f.exe` 在跑、
  产物缺失），1 = 用例失败，0 = 通过；只清理**自己起出来的**进程（绝不 `taskkill` 用户正在用的
  HUD）；截图统一落 `logs/_probe/`。用 Playwright 驱动 WebView2 窗口做 DOM/JS 断言。

## 结构图

```
                        ┌───────────────────────── build.sh ─────────────────────────┐
   cargo (mingw)  ─────►│ WIN_PKGS → target/<triple>/release → binary/*.exe + 根 gui │
                        │ --native/--linux-only → binary/wp8f（Linux 版）            │
                        └───────────────────────────────────────────────────────────┘
                                              │ 产物就位（S1 布局）
                                              ▼
   cargo test  ────────► test.sh ──► 用例数 / 汇总数 / 失败数三线判定 ──► 退出码 0/1/2
                                              │
                                              ▼
                        ┌──────────────── zip.sh ──────────────────┐
                        │ 清单 = exe + 运行期资源 + config/*.json  │
                        │ 临时目录 → unzip -Z1 逐项校验 → mv 到根  │
                        └──────────────────────────────────────────┘
                                              │
                                              ▼
   Windows：python scripts\tests\*.py ──► wp8f-gui.exe（--port/--no-window）──► 真窗口断言
                                            └─ 拉起 binary/wp8f.exe + binary/test-server.exe
```

## 代码目录

| 路径 | 内容 |
|---|---|
| `build.sh` | 交叉编译 + S1 布局归位 + WebView2 loader 部署 + 旧产物清理（用法见文件头） |
| `test.sh` | Rust 单测驱动 + 结果计数 + Windows 测试清单 |
| `zip.sh` | 显式清单打包 + 非系统 DLL 依赖审计 + 泄漏反查（用法见文件头） |
| `make_icon.py` | logo 生成器：几何常量 → SVG / ICO / favicon（改图形只改这里） |
| `tests/api_parity.py` | 控制台 HTTP API 与黄金样本比对（`selftest` 自检模式可离线跑） |
| `tests/diag_console.py` | 对着**已在运行**的控制台做只读体检（唯一不拉新实例的探针） |
| `tests/i18n_keys.py` | 十五种界面语言的键一致性（Rust 表 ↔ 前端 ↔ `resource/i18n`） |
| `tests/layout_check.py` | 配置器布局：溢出/等高/一屏放下 + FM 真渲染 |
| `tests/ooda.py` | 前端 DOM/JS 自检（Playwright） |
| `tests/security_check.py` | 安全回归：Origin / Content-Type / 上传体积 / 路径解码 |
| `tests/tab_check.py` | 页签切换（active 面板 / JS 异常 / FM canvas） |
| `tests/tray_click_probe.py` | 托盘点击链路（消息级注入） |
| `tests/tray_e2e.py` | 托盘/窗口/子进程全链路（约 2 分钟，含预览的启动/回收） |
| `tests/ui_checks.py` | 控制台 UI：导航/FM/回放/配置器/导弹页签 |
| `tests/update_check.py` | FM 数据库更新面板（需先起一个控制台实例，端口作参数） |
| `tests/window_flash_check.py` | 调 FM 时不闪控制台黑框 |
| `tests/window_size.py` | 窗口比例检查（16:9） |
| `tests/wpr_replay.py` | `.wpr` 回放校验（真实录制 → 回放页） |
| `tests/golden_python/` | `api_parity.py` 的黄金样本（接口字段快照） |

## 关键函数 Specification

脚本与探针的「函数」就是**命令行 + 退出码契约**（没有可调用的库 API）。

### `scripts/build.sh [--native|--linux-only|--webui-only] [--debug] [--clean] [--install-tools] [-j N] [--online]`
* 输入：上述开关（默认 = 交叉编译 Windows 全套）。
* 输出：`wp8f-gui.exe`（仓库根）、`binary/{wp8f,flightmodel,test-server}.exe`；
  `--native`/`--linux-only` 另出 `binary/wp8f`。
* 前置：`rustup target add x86_64-pc-windows-gnu`（`--install-tools` 可代劳）；
  默认离线，故依赖必须已在本地 `~/.cargo` 缓存里。
* 后置：旧布局的同名 exe 被清理；汇总表打印每个产物及其大小。
* 错误：任何一步失败即非 0 退出（`set -euo pipefail`）；缺产物只 `warn`，不让构建失败。
* 副作用：写 `binary/` 与仓库根；**不能有 `wp8f.exe` / `wp8f-gui.exe` 正在运行**
  （Windows 会锁住 exe，`cp` 报 `Input/output error`）。

### `scripts/test.sh [--allow-known-fail] [--strict] [--rust-only] [--gui]`
* 输入：默认严格（任何失败 → 退出码 1）；`--allow-known-fail` 容忍 `KNOWN_FAIL` 白名单；
  `--strict`/`--rust-only` 是兼容旧参数的别名；`--gui` 只打印 Windows 测试清单后退出 0。
* 输出：`==>` 分步日志 + 用例数/组数；失败时打印明细。
* 前置：本机装了 `cargo`（否则退出码 2）。
* 后置：不改任何文件（只读测试）。
* 错误：`cargo test` 非 0 → 判定为编译/链接失败并打印最后 40 行；
  用例数为 0 也视为失败。
* 副作用：跑测试会写各自 crate 的 `target/`；不碰 `config/`、`logs/`。

### `scripts/zip.sh`（无参数，`-h` 看说明）
* 输出：仓库根 `wp8f-<日期>.zip`（**不带版本号**）+ 包内 `VERSION.txt`（`wp8f build <日期>`）。
* 清单：① 可执行文件 + `test-server/scenarios`；② 运行期资源
  （`gui/static`、`resource/{fonts,voice,lang,i18n,data}`、`config/*.json`）；
  ③ `LICENSE`。
* 前置：`build.sh` 已产出全套 exe；系统有 `zip` 与 `unzip`（否则退出码 2）。
* 后置：临时目录里打完 → `unzip -Z1` 逐项校验（缺任一项 → 退出码 1）→ `mv` 到仓库根。
* 错误：1 = 缺件/校验失败；2 = 参数或环境不满足。
* 副作用：写临时目录与仓库根 zip；`resource/data`（约 540 MB）**始终进包**。

### `scripts/make_icon.py [--check]`
* 输出：`gui/static/logo.svg`（矢量：favicon + README 插图）与 `gui/logo.ico`
  （16/24/32/48/64/128/256 七档，手写 ICO 容器 + 每档 PNG 记录）。
* 前置：需要 Pillow（`pip install pillow`）；**SVG 不依赖它，只有 ICO 与 `--check` 需要**
  （缺了给退出码 2 与安装提示）。
* 后置：两份产物由同一组几何常量画出来 —— 改图形只能改脚本里的常量，SVG 里也写了这句
  （手改 SVG 会在下次生成时被覆盖）。
* `--check`：只比对磁盘产物与脚本是否一致（CI/改图后自查用），不一致退出码 1。
* 错误：0 = 成功；1 = `--check` 不一致；2 = 缺 Pillow。
* 副作用：写上述两个文件；不联网、不读别的输入。

### `tests/*.py`
* 统一用法：`python scripts\tests\<探针>.py`（`update_check.py` 需带端口，`api_parity.py`
  支持 `selftest` 子命令）。
* 前置：Windows + 已构建的 S1 产物；**跑之前没有别的 `wp8f-gui.exe` / `wp8f.exe` /
  `test-server.exe` 在跑**（否则退出码 2，避免误杀用户进程）。
* 后置：只清理自己起的进程；截图落 `logs/_probe/`；`tray_e2e.py` / `tray_click_probe.py`
  真跑一次 HUD，退出时还原 `config/*.json`。
* 错误：**0 = 全部通过，1 = 有用例失败，2 = 前置不满足**（三者语义不可混用）。
* 副作用：会拉起真实 GUI/HUD 子进程、写 `logs/` 下的日志与截图。

## 约束与取舍

* **不要在 PowerShell 里捕获这些脚本的输出**（`| Out-String`、`$(...)`）：探针会拉起
  `wp8f-gui.exe`，子进程继承 stdout 管道会让「捕获」一直等到 GUI 退出（表现为卡住）。
* 构建/测试/打包脚本只在 **Linux / WSL** 跑；GUI 探针只在 **Windows** 跑 —— 两边不互相调用。
* `zip.sh` 的字体清单与 `disp/src/font.rs` 的 `DEFAULT_TEXT_FILE` / `ICON_FILE` **同源**，
  改代码要同步改脚本；字体不再内嵌，缺文件解压出来一跑就退出。
* `resource/data` 来自 War-Thunder-Datamine（上游无许可证），含数据的包**仅限自用/私下分发**。
* 探针的单实例判据是端口独占 + `tasklist` 映像名扫描，因此「用户自己开的实例」与
  「探针开的实例」必须能区分 —— 这也是 `2` 号退出码存在的原因。

## 测试

`scripts/` 自己不被测（shell 与探针就是测试工具）；它的正确性由
① `test.sh` 每条路径都被日常使用、② `zip.sh` 的 `unzip -Z1` 反查、③ `tray_e2e.py`
验证控制台真的能从 `binary/` 拉起 HUD 与 test-server 来保证。
