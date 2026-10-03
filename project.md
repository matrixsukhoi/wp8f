# project.md —— 面向开发者

[`README.md`](README.md) 面向"想用的人"（装、跑、调配置、常见问题）；
本文件面向"想改代码的人"：架构、构建、测试、代码规范、扩展点、发布。
**模块内部细节在各 crate 自己的 `README.md`**（见下面的索引），这里只讲全局与约定。
（开发过程中的 `docs/`、`3rdparty/` 是本地目录、不入库，外部克隆看不到它们。）

## 模块文档索引

| 模块 | 文档 | 一句话 |
|---|---|---|
| `core` | [core/README.md](core/README.md) | 8111 取数 → 解析 → 状态计算 → 写 disp 帧槽（bin `wp8f-core`） |
| `ring` | [ring/README.md](ring/README.md) | 无锁"最新值"环形缓冲区（零依赖，HUD 帧缓冲与数据链槽共用） |
| `disp` | [disp/README.md](disp/README.md) | HUD 绘制 + 窗口 + 共享状态（环、底图、布局配置、拖拽、字体、文案） |
| `logger` | [logger/README.md](logger/README.md) | `.wpr` 记录容器 + 记录线程 + 三种导出转换器 |
| `flightmodel` | [flightmodel/README.md](flightmodel/README.md) · [SPEC.md](flightmodel/SPEC.md) | FM 解析 / 气动 / 发动机 / EM / 数据更新器 |
| `datalink` | [datalink/README.md](datalink/README.md) | Link-8 数据链：协议 + 服务端 + 客户端 + 友军追踪 |
| `gui` | [gui/README.md](gui/README.md) | 控制台：托盘 + 本地 HTTP API + WebView2 窗口 + 子进程监管 |
| `test-server` | [test-server/README.md](test-server/README.md) | 模拟 8111 服务端（拖拽预览） |
| `scripts` | [scripts/README.md](scripts/README.md) | 构建 / 测试 / 打包 / 探针 / logo 生成 |

## 架构总览

### crate 与依赖方向

```
gui ──► flightmodel（FM 页签调二进制 + 数据根解析）
 │
 └─（子进程）─► wp8f-core ──► disp ──► datalink ──► ring
                   │            └────► ring
                   ├──► logger ──► disp / ring
                   └──► flightmodel

test-server   独立（只依赖 std + toml/clap/image）
ring          零依赖（谁都能依赖它）
```

* `disp` **依赖** `datalink`（取协议类型与默认值），所以 `datalink` 不能反向依赖 `disp` ——
  两边共享的无锁环才被抽成零依赖的 `ring`。
* `core` 是唯一的"数据生产者"；`gui` 不含任何飞行计算，只编排（含拉起 `binary/wp8f.exe`）。

### 数据流（一帧）

```
8111（游戏 / test-server）
   │ HTTP（tokio）
   ▼
core: Channel ─► parser/flat_json ─► FlightContext + flightmodel ─► DisplayData
   │                                                                    │
   │ begin_frame()/commit_frame()                                       │ 每帧
   ▼                                                                    ▼
disp: Ring<DisplayData>（ring.frames 槽） ──► 窗口线程 draw_hud（circle / mini）
   │                                          └─► 地图面板 / 友军面板
   ├── FrameReader::poll_latest ──► logger 记录线程 ──► logs/wp8f_<unix_ms>.wpr
   └── link_ring::publish_own ──► datalink 线程 ──UDP──► 服务端 ──► 友军快照 ──► HUD
```

### 进程与线程

* **进程**：`wp8f-gui.exe`（控制台，唯一入口）+ `binary/wp8f.exe`（HUD，控制台的子进程）
  + `binary/test-server.exe`（仅拖拽预览时）。控制台把子进程挂进 kill-on-close 作业对象，
  自己怎么死都会带走它们；同时只回收"自己启动的"（按 pid 记录 + 映像名复核）。
* **线程**：core 主循环（tokio 多线程运行时）；disp 的窗口/渲染线程；logger 记录线程；
  datalink 线程。**跨线程只走 `ring`**（无锁、无 channel、无 `Arc` 共享可变状态），
  唯一的 channel 是 logger 给"其他飞机"预留的 `SyncSender`（`try_send`，满了就丢）。

### 几个必须知道的契约

| 契约 | 含义 | 出处 |
|---|---|---|
| 载荷必须是 POD | 环里放的是借用；带堆指针的类型会被读者在脚下释放 | [ring/README.md](ring/README.md) |
| 单生产端 | 一个环只有一个写者（多写者不设防） | 同上 |
| 发布—直读 | 写端只写"下一个槽"，读端只读"已发布的槽"，没有保留/重试 | 同上 |
| 地图快照槽例外 | `Ring<MapDisplay>` 的 `map_objects: Vec` 是堆指针，靠 4 Hz 采样频率规避绕圈 | [disp/README.md](disp/README.md) |
| 导出面收窄 | `wp8f_core` 只导出 bin 真正用到的符号（否则 rustc 无法判定死代码） | `core/src/lib.rs` |
| 默认 0 告警 | `cargo check --workspace --all-targets` 必须 0 warning | 本文件「代码规范」 |
| `flat_json` 不可动 | 8111 专用优化解析器，改它风险大于收益（用户口径） | [core/README.md](core/README.md) |

## 构建

```bash
git clone https://github.com/matrixsukhoi/wp8f.git
cd wp8f
git submodule update --init gui/static/vendor/wt-missile-simulator   # 控制台的离线导弹仿真

bash scripts/build.sh                 # 默认：交叉编译 Windows 整套
bash scripts/build.sh --native        # 同时构建本机 Linux 版（binary/wp8f）
bash scripts/build.sh --webui-only    # 只构建控制台（仓库根 wp8f-gui.exe），调 GUI 最快
bash scripts/build.sh --linux-only    # 只构建 Linux 版
bash scripts/build.sh --debug         # debug 构建
bash scripts/build.sh --clean         # 先 cargo clean
bash scripts/build.sh --install-tools # 缺 rustup target 时自动安装
bash scripts/build.sh -j 8 --online   # 并行度 / 允许联网（默认 --offline）
```

产出（S1 布局：**仓库根只放唯一入口**）：

| 产物 | 说明 |
|---|---|
| `wp8f-gui.exe` | 控制台（托盘 + HTTP API + WebView2 窗口）。**双击它就行** |
| `WebView2Loader.dll` | 上面那个控制台的运行期依赖（GNU 目标动态链接 loader，MSVC 目标才静态）。**必须与 `wp8f-gui.exe` 同目录**，随包分发 |
| `binary/wp8f.exe` | HUD 主程序（core + disp，bin 名 `wp8f-core`） |
| `binary/flightmodel.exe` | FM 解析/曲线（控制台直接读 `binary/`） |
| `binary/test-server.exe` | 模拟服务端（场景仍在 `test-server/scenarios`，按 **cwd** 解析） |
| `binary/datalink-server.exe` | 数据链服务端（Windows；`--features bins` 才编） |
| `binary/datalink-test-client.exe` | 数据链测试客户端（Windows，联调用） |
| `binary/datalink-server` | 数据链服务端（Linux；服务端多跑在 Linux 上） |
| `binary/datalink-test-client` | 数据链测试客户端（Linux） |
| `binary/wp8f`、`binary/flightmodel` | Linux 本机构建产物（`--native` / `--linux-only`） |

* 数据链的两个二进制要 `--features bins` 才编（默认只编库）：`build.sh` 里是**独立一步**
  （它的 feature 与 `FM_JSON_FEAT` 无关，混进包列表会让 cargo 报
  "none of the selected packages contains these features"），
  默认两个平台都编，`--windows-only` / `--linux-only` / `--webui-only` 会相应收窄。

* 整条链就是 `cargo build --target x86_64-pc-windows-gnu`：不需要 Python、不需要 PyInstaller。
* **图标链**：`scripts/make_icon.py` → `gui/static/logo.svg`（矢量，兼 favicon 与 README 插图）
  + `gui/logo.ico`（七档，`gui/build.rs` 用 `windres` 把它编进 `wp8f-gui.exe`；
  `tray.rs` 的退回路径也 `include_bytes!` 它）。找不到 `windres` 只警告不失败。
  图标放 `gui/` 而不是 `resource/`：它是**构建输入**，必须随仓库走
  （`resource/` 只入库 `lang` 与 `i18n`，其余是本地资源）。
* **构建常见坑**：`wp8f.exe` / `wp8f-gui.exe` 正在运行会锁住 exe，`cp` 报
  `Input/output error` —— 先退出控制台（托盘右键 → 退出）再构建。
* **GNU 目标的 GUI 必须有 `WebView2Loader.dll` 陪跑**：`webview2-com-sys` 只在 MSVC 目标静态链接
  loader（那份 `WebView2LoaderStatic.lib` 是 MSVC C++ 目标文件，要 `__security_cookie`、
  `_Init_thread_*`、MSVC 的 `operator new` 等 CRT 内部符号，GNU ld 链不了），非 MSVC 目标生成的是
  对 `WebView2Loader.dll` 的普通导入 ⇒ 它必须与 `wp8f-gui.exe` 同目录。`build.sh` 从依赖的
  OUT_DIR 取 x64 那份复制到仓库根，`zip.sh` 把它当必需件并按导入表审计（历史：issue #1 漏了它，
  发布包双击报"找不到 WebView2Loader.dll"）。**验证发布包要在干净目录里跑，并清掉 PATH 中可能存在的
  同名 DLL** —— 开发机装了 Windows Performance Toolkit 之类会带一份旧副本，正好把缺失掩盖掉。
* **Linux 状态：可编译，未测试**。HUD 在 WSL/Linux 能编译通过，但没有实机验证过运行；
  已知 WSL1 + Xming 跑不起来（winit 0.31 的 X11 后端要求 XInput2）。
  控制台依赖 wry/tao/windows-sys，**不打算支持 Linux**。
* 手动编译（不带脚本）：`cargo build --release -p wp8f-core [--target x86_64-pc-windows-gnu]`；
  无需任何 cargo feature —— 语音/提示音永远编译进程序，是否出声由
  `voice_warnings_enabled` 决定（默认开；关掉后视觉告警照旧）。

## 测试

```bash
bash scripts/test.sh                     # Rust 单测（严格：任何失败都退出码 1）
bash scripts/test.sh --allow-known-fail  # 放行白名单里的历史失败（本地日常）
bash scripts/test.sh --gui               # 只打印 Windows 侧探针的跑法（不在 WSL 里驱动 GUI）
```

* 一条 `cargo test --offline --release -p …` 覆盖 6 个 crate；编译/链接失败立刻退出码 1
  并打印最后 40 行，**一个用例都没解析到也按失败处理**（不会出现"0 个用例 ✓ 全部通过"）。
* 控制台 GUI 的端到端测试**必须在 Windows 上跑**（真托盘/真窗口/WebView2/真子进程）；
  从 WSL 互操作驱动会因进程环境差异不稳。

| 探针 | 用途 | 是否改动用户可见状态 |
|---|---|---|
| `tray_e2e.py` | 托盘/窗口/子进程全链路（约 2 分钟，最全） | 会真跑一次 HUD；开始备份、结束逐字节还原 `config/*.json` |
| `tray_click_probe.py` | 托盘点击链路（消息级注入） | 同上；跑之前已有 wp8f/test-server/控制台则退出码 2 |
| `ui_checks.py` | 控制台 UI：导航 / FM / 回放 / 配置器 / 导弹页签 | 改 `config/default.json`（结束还原）、上传样本（结束删）、截图 |
| `layout_check.py` | 配置器布局：溢出 / 同排等高 / 一屏放下 + FM 图真渲染 | 仅截图（`logs/_probe/layout_check/`） |
| `tab_check.py` | 页签切换 + FM 图（6 张通用 + 喷气专属的 EM）+ JS 异常断言 | 仅截图 |
| `security_check.py` | 安全回归：跨站 Origin / Content-Type / 体积上限 / 路径解码 | 上传样本到 `logs/`（结束删） |
| `window_flash_check.py` | 调 FM 时不闪控制台黑框 | 否 |
| `api_parity.py selftest` | 与 `scripts/tests/golden_python` 逐字段比对（另有 `capture`/`compare`） | 重写 `logs/golden_rust/` |
| `ooda.py` | 前端 DOM/JS 自检（Playwright + Edge 无头） | 截图/日志在 `logs/_probe/ooda/` |
| `window_size.py` | 控制台窗口 16:9（客户区，偏差 ≤5%） | 否 |
| `wpr_replay.py` | `.wpr` 回放链路 | 会拉起控制台 |
| `i18n_keys.py` | 语言键一致性（15 语言 × 全键） | 否 |
| `diag_console.py` | 对**已在运行**的控制台做只读体检 | 否 |

* 探针退出码统一：**0 = 通过，1 = 有用例失败，2 = 前置不满足**（已有实例在跑 / 产物缺失）。
  跑之前先退出正在运行的控制台 —— 单实例判据是端口独占，但探针与用户实例共用
  `logs/console-host.json` 与控制台日志。
* 探针产物统一落 `logs/_probe/<脚本名>/`：全部通过时自动删除，失败才保留并打印路径。
* 已知历史失败：`tab_check` 的 5/6 项（"6 FM charts" 文案过时）、`api_parity selftest` 2 处漂移、
  `window_size.py`/`update_check.py` 需要可见控制台/无头 shell。

## 代码规范

* **注释**：只写"这段代码要满足什么硬约束/为什么不能那样写"，短、密、不留考古
  （不写"曾经如何、某某评审说过什么"——那是台账的事）。发现与文档冲突时**以代码为准**并改文档。
* **日志**：一句话说清问题与下一步（`[RECORD] cannot create dir X: …`），
  不打印成功路径的噪声；同一站点只报一次（`channel.rs` 的 `warn` 按站点去重）。
* **测试**：断言**行为/结构**，不断言提示文案；一个测试只证一件事；
  纯逻辑（如 `FrameClock`）必须抽出来直测，不靠"复刻一份主循环"。
* **纯函数优先**：能抽成"喂输入拿输出"的就抽（`FrameClock`/`mapobj::period_ms`/`convert`），
  副作用集中到边界（I/O、窗口、声音、配置读写）。
* **默认 0 告警**：`cargo check --offline --workspace --all-targets` 与 Windows 目标 check
  都必须 0 warning；新增 `pub use` 前先问"谁会用"（否则死代码告警就回来了）。
* **错误处理**：能恢复的降级并记日志（缺语音包、缺底图），不能恢复的启动期明确失败
  （缺字体、配置解析失败）；不用 `unwrap()` 赌运行期。
* **文档**：每个 crate 的 `README.md` 按统一结构写（职责 / 实现原理 / 结构图 / 代码目录 /
  关键函数 Specification / 约束与取舍 / 测试）；改行为要同步改对应 README 与台账。
* **提交信息**：中文主题 + 说明段（`git commit -F <文件>`，别用 `-m` —— 长中文会被截断）。

## 内部数据与格式

### `DisplayData`（每帧载荷，`disp/src/lib.rs` 是权威定义）

| 组 | 字段（节选） |
|---|---|
| 头部 | `magic`(0x57503846) · `version` · `timestamp`(ns) · `frame` · `valid` |
| 飞行 | `altitude` `radio_altitude` `ias` `tas` `mach` `aoa` `aos` `ny` `vy` `wx` `sep` |
| 姿态 | `heading` `bearing` `roll` `pitch` `has_attitude` |
| 操纵 | `aileron` `elevator` `rudder` `trimmer` `flaps` `gear` `airbrake` `wing_sweep` |
| 燃油 | `fuel_kg` `fuel_percent` `fuel_time` `fuel1_kg` `fuel1_time` `has_fuel1` |
| 发动机 | `engine_count`(≤ `MAX_ENGINES = 8`) + 六个 `[f64; 8]` 数组（油门/功率/转速/推力/水温/油温）+ `is_jet` |
| 性能 | `engine_efficiency` `thrust_to_weight` `power_to_weight` `total_thrust` `total_hp` `thrust_percent` `energy_height` `turn_rate` `turn_radius` |
| 限制 | `vne` `vne_mach` `max_aoa` `min_aoa` `maneuver_margin` `gear_armed` |
| 告警 | `overspeed_warning`(0/1/2) `mach_warning`(0/1/2) `g_load_warning`(0/1) `voice_alarm` `brake_caution` |
| 文本 | `*_text` 与 `aircraft_type` 都是 **`FixedBytes<N>`**（定长、无堆 —— 环要求 POD） |

* 字符串一律 `FixedBytes`（`as_str()` / `assign()` / `clear()`），新增字段时**不要**用 `String`。
* 数值文本在 `core/src/display/updater.rs` 里格式化（`format_sig_figs` / `format_adaptive`）；
  HUD 只画不格式化。
* `frame` 是记录起点的判据（`frame > RecordConfig::start_frame`）。

### 环形缓冲与地图采样（配置键）

| 键 | 默认 | 语义 |
|---|---|---|
| `ring.frames` | 32 | 帧环槽数（`DisplayData`）。消费者能落后多少数据帧；30 Hz 下 32 ≈ 1 s |
| `ring.map_samples` | 32 | 地图环槽数（`MapDisplay`）。4 Hz 采样下 32 ≈ 8 s |
| `map_obj_record_every_frames` | 8 | **每多少个数据帧**记录一次地图对象；钳 `1..=refresh_hz` |

* 槽数钳 `2..=256`（`RingConfig::*_clamped`）；调大只多占内存（启动时一次性分配）。
* `overwritten=` 统计按**实际槽数**判定：一轮轮询之间的新帧数 `skipped + 1 > slots`。
* 地图记录间隔的唯一换算入口是 `core/src/mapobj.rs::period_ms`（毫秒 = `帧数 × 1000 / refresh_hz`）；
  记录起点、主循环闸门、`FlightContext` 初值三处必须同源。
* **没有单独的"记录频率"配置**（旧键 `record.poll_ms` 已删除）：记录线程的采样周期就用
  `period_ms(生效帧数, refresh_hz)` = 地图刷新周期。地图坐标每 `map_obj_record_every_frames`
  个数据帧才更新一次，记录得比它更快只会写下坐标完全相同的重复帧。

### `.wpr` 飞行记录

一次飞行一个文件 `logs/wp8f_<unix_ms>.wpr`：**JSON 元信息 + N 组 CSV + 地图底图**，
容器版本 2（只有一个版本，读到别的直接报错）。组 0 = 玩家。记录期间不碰磁盘
（每组一个内存池，结束才拼一个文件）。容器布局、列义与小数位见
[logger/README.md](logger/README.md) 与 `logger/src/wpr.rs` 的 `CSV_HEADER` / `COL_DECIMALS`。

**回放只认 `.wpr`**：旧 `.acmi` / `.csv` 的读路径已删除；导出 ACMI / TacView CSV / FlatCSV
走 `logger::convert`（那是唯一做单位与符号换算的地方）。回放**不做轨迹平滑** ——
直接画原始采样点（想让轨迹密，就调小 `map_obj_record_every_frames`）。

## 扩展点

| 想做什么 | 要改哪里 |
|---|---|
| 新增 HUD 元素/字段 | `disp::DisplayData` 加字段（POD！）→ `core/display/updater.rs` 填值 → 各 panel 绘制 →（要显示文案就加 i18n 键）→ 要进记录就加 `logger::wpr::CSV_HEADER` 列 |
| 新增配置项 | `disp::HudLayoutConfig` 加字段 + `#[serde(default)]` 默认值 → `core/src/main.rs::Args`（要 CLI 覆盖就进 `apply_overrides` 表）→ `gui/static/config-form.js` 表单 → `config/default.json` 内置默认 |
| 新增界面语言 | `resource/i18n/<lang>.json` + `gui/src/i18n.rs::SUPPORTED` + `disp::i18n` 的表；跑 `scripts/tests/i18n_keys.py` 校键齐 |
| 换/加字体 | 字体文件放 `resource/fonts/` → `disp/src/font.rs` 的 `DEFAULT_TEXT_FILE`/`ICON_FILE` → **同步 `scripts/zip.sh` 的 `FONT_FILES`**（三处必须一致） |
| 新增 HUD 样式 | `disp::HudType` 加一档（现在是 `Off`/`Mini`/`Circle` → 配置数字 `0/1/2`，序列化见 `HudType` 的 `Serialize`/`Deserialize`）+ 新 `xxx_hud.rs` 的 `draw_*` + `hud::draw_hud` 的 `match` + `Panel`/`Slot` 的拖拽落点（`Slot` 要比 `Panel` 多一格） |
| 新增 8111 字段 | `core/src/parser.rs` 结构体 + `flat_json` 取值 → `updater` 使用；`test-server` 的 `scenarios/*.toml` 补字段以便无游戏调试 |
| 新增告警开关 | `disp::HudLayoutConfig` 加 `bool`（`default_true`）→ core 读取处接线（如机动告警声看 `maneuver_tone_enabled`）→ `gui/static/config-form.js` 的 `check("alert", t("cfg.lbl.xxx"), "键", true)` → 15 个 `resource/i18n/*.json` 补 `cfg.lbl.xxx`（跑 `scripts/tests/i18n_keys.py` 校键齐） |
| HUD 本体开关 | **只有配置键 `hud_type` 一个**（0 关闭 / 1 MiniHUD / 2 圆环）：别再引入第二个 bool —— 历史上 `circle_enabled`/`minihud_enabled` 与它并存过，"改了类型没改开关"就是整屏空白（这两个键已删）。也**没有命令行参数**：`--hud-type` 与"控制台硬编码传 `--hud-type circle`"都踩过坑（命令行覆盖发生在加载配置之后，用户怎么改都没用） |
| 新增数据链字段 | `datalink::StatePacket`（**定长、线上 4 字节对齐**）+ `link_ring::OwnState` + 两端读写；改线上格式要同时升 `DATALINK_VERSION` |
| 新增探针 | `scripts/tests/<name>.py`（照抄现成探针的退出码 0/1/2 与 `logs/_probe/` 约定）+ `scripts/README.md` 与 `test.sh --gui` 的清单 |
| 改 logo/图标 | 只改 `scripts/make_icon.py` 的几何/配色常量，然后 `python scripts/make_icon.py`（`--check` 自查） |

## 发布与打包

```bash
bash scripts/build.sh          # 先出全套产物
bash scripts/zip.sh            # 打包 → 仓库根 wp8f-<日期>.zip（不带版本号）
```

* 清单三组：① 可执行文件 + `test-server/scenarios`；② 运行期资源
  （`gui/static`、`resource/{fonts,voice,lang,i18n,data}`、`config/*.json`）；
  ③ `LICENSE` / `VERSION.txt`（构建日期，打包时生成）。
  可执行文件含 `binary/{wp8f,flightmodel,test-server,datalink-server,datalink-test-client}.exe`
  与两个平台的 `datalink-server` / `datalink-test-client`（Linux 版服务端也随包发）。
* **`resource/data`（约 540 MB / 26000 文件）始终打进包**（必需件，缺件直接失败）；
  它不在 git 里（上游 War-Thunder-Datamine 无许可证），所以含数据的包**仅限自用/私下分发**。
* **明确不打**：`target/`、`logs/`、`resource/{data_new,data_old}/`、`resource/.wp8f_update_tmp/`，
  以及 `resource/fonts/` 下除 `FONT_FILES` 三个文件以外的一切（用户自备字体来源/许可不明）。
* 硬规则：先删旧 zip（`zip -r` 是更新语义，会残留已删文件）→ 显式清单不整目录通吃 →
  先打到临时目录、`unzip -Z1` 逐项校验 + 泄漏反查（缺项或混入都非 0 退出）→ 通过才 `mv`。
* 版本口径：包名只带日期；包内 `VERSION.txt` 写 `wp8f build <日期>`；
  FM 数据版本另见控制台「飞行模型」页签（读 `resource/data/version`）。

## 许可与第三方

* 本项目 **GPL-3.0-only**，全文见 [`LICENSE`](LICENSE)。
* 随包字体带 `OFL.txt`（必须随字体分发）；`gui/static/vendor/` 下的 ECharts 等文件内嵌许可头；
  离线导弹仿真子模块有自己的许可（见其仓库）。
* 飞机性能数据来自第三方公开拆包数据库（War-Thunder-Datamine，**上游无许可证**）；
  拆包行为本身可能违反游戏 EULA，本项目不参与拆包、不含游戏本体文件。
