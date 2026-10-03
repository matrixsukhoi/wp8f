# core —— 数据主循环（`wp8f-core`，二进制 `wp8f-core` → 打包为 `binary/wp8f.exe`）

## 职责

HUD 的**唯一数据生产端**：从本地 8111 端口拉取游戏状态 → 解析 → 用飞行模型算状态 →
把结果写进 `disp` 的帧槽；顺带管窗口、飞行记录线程、数据链线程、语音告警与机动告警声。
它自己不画一个像素。

## 实现原理

* **异步主循环 + 纯帧调度**。`tokio` 多线程运行时跑 `data_process_loop`：每轮
  `wait_until(ts_next)` 睡到本帧时刻，把 `now` 喂给 `FrameClock` 拿一个
  `Tick { frame, timestamp_ns, interval_ns, nominal_interval_ns, mapobj_interval_ns, dropped }`。
  帧号、实测间隔、丢帧判定、地图采样闸门这些**整数运算全在 `frame_clock.rs`**（纯函数、可直测），
  主循环只负责「喂时间、按闸门决定这帧要不要采地图对象」。刷新率 `refresh_hz` 来自配置
  （5..60，缺省 30）。
* **拉取与重连**。`Channel` 是 8111 的 HTTP 客户端：`/state`、`/indicators`、`/map_info.json`、
  `/map_obj.json`、`/map.img`（底图一次）。`try_connect()` 按 `--port` 列表轮换探测，
  连上后立刻 `reset_ring_buffer()`（帧序号断流）与 `link_ring::reset_tracks()`（换局清友军），
  再开始按帧取数；连接中断累计到阈值由 `check_invalid()` 给出 `ExitReason`。
* **解析分两层**。`parser.rs` 定义报文结构（`FlightState`/`Indicators`/`MapInfo`/`MapObjData`）
  与顶层字段解析；`parser/flat_json.rs` 是**为 8111 报文手写的专用解析器**（键有序、
  空白固定、字段名带空格与逗号），比通用 JSON 快得多，**用户口径：不可改动**。
* **状态计算集中在 `display/updater.rs`**。每帧把解析结果与 `FlightContext`（跨帧状态：
  重量/油耗基线、发动机工作计时、过热、气压/滑油等）合成 `DisplayData`：定长文本
  走 `FixedBytes` + `set_text*`，数值走 `fmt::{format_sig_figs, format_adaptive}`。
  油量以**连接开始时的 fuel0 为基线**守卫（避免游戏未读到油量时算出负数）；
  单位按 `imperial` 开关分叉（表速/高度/马压/温度）。
* **告警两条独立音层**。`warnings.rs` 管**语音告警**（`VoicePack` 启动时把
  `resource/voice/**/*.wav` 预加载进内存，`rodio` 播放；带冷却与「闪烁 X」显示），
  `maneuver_tone.rs` 管 **F-18 风格机动告警声**（`m = MAX(攻角比, 过载比, 超速项)`，
  70% 起音、67% 迟滞、2→12 Hz 加密、≥100% 连续长鸣）—— 两者并存，时间都由调用方注入。
* **记录与数据链各一个线程，主循环零 I/O**。`spawn_recorder_thread()` 起 logger 的记录线程
  （拿到 `RecordHandle`），`spawn_link_thread()` 起数据链线程（拿到 `LinkHandle`）；
  主循环每帧只 `link_ring::publish_own(...)`（无锁、无分配），退出时
  `stop_and_write()` / `stop()`。
* **窗口由 disp 持有**：`create_window()` 一次，此后主循环只
  `begin_frame()` → 填 → `commit_frame()`；退出路径 `destroy_window()`。
* **导出面刻意收窄**（`lib.rs` 的模块文档）：`wp8f_core` 只 `pub use` bin 真正用到的那一份，
  这样 rustc 才能继续判定死代码 —— **默认 `cargo check` 恒 0 告警**是硬要求。

## 结构图

```
  8111（游戏 / test-server）
        │ HTTP
        ▼
  ┌───────────────┐   解析    ┌──────────────────────────┐
  │ channel.rs    │──────────►│ parser.rs + flat_json.rs │
  │ Channel       │           │ FlightState / Indicators │
  │ 端口轮换/重连 │           │ MapInfo / MapObjData     │
  └───────────────┘           └────────────┬─────────────┘
                                            │
                                            ▼
                       ┌─────────────────────────────────────────┐
                       │ FlightContext（跨帧：重量/油耗/过热…）  │
                       │ + flightmodel（气动/发动机/过载限值）   │
                       │ display/updater.rs → DisplayData        │
                       └───────┬───────────────────────┬─────────┘
                               │ begin_frame/commit    │ 告警
                               ▼                       ▼
                 Ring<DisplayData>（disp）      warnings.rs 语音告警（rodio）
                    │              │            maneuver_tone.rs 机动告警声
        HUD 渲染线程 │              │ FrameReader
        （disp 窗口） │              ▼
                       │        logger 记录线程（.wpr）
                       └─ FrameClock 决定帧号/间隔/丢帧/地图采样闸门
                          link_ring::publish_own → datalink 线程 → 友军快照
```

## 代码目录

| 路径 | 内容 |
|---|---|
| `src/main.rs` | bin：`Args`、`main()`、`data_process_loop`、`pre_process`、`try_connect`、`check_invalid`、`print_fm_report`、记录/数据链线程的启动 |
| `src/lib.rs` | 库门面（收窄的导出面，见上） |
| `src/channel.rs` | `Channel`/`ChannelConfig`：8111 HTTP 取数、端口轮换、一次性告警（`warn`/`warn_bytes` 按站点去重） |
| `src/parser.rs` | 报文结构与 `parse_state`/`parse_indicators`/`parse_map_info`/`parse_map_obj_data` |
| `src/parser/flat_json.rs` | 8111 专用 JSON 解析器（**不可改动**） |
| `src/context.rs` | `FlightContext`：跨帧状态、飞行模型持有、温度/过热初值、时间戳工具 |
| `src/display/updater.rs` | `DisplayData` 的全部计算：飞行/发动机/燃油/性能指标/告警分级 |
| `src/warnings.rs` | 语音告警：语音包解析与预加载、告警判定、冷却、闪烁 X |
| `src/maneuver_tone.rs` | F-18 风格机动告警声（分级/迟滞/长鸣，纯逻辑 + 注入时间） |
| `src/frame_clock.rs` | `FrameClock`/`Tick`：帧调度（纯整数运算） |
| `src/mapobj.rs` | 地图记录间隔（**数据帧 ↔ 毫秒**）的唯一换算入口 `period_ms` |
| `src/math/` | `angle`（角度/罗盘/颜色解析）、`map`（地图坐标、距离、机场/点位取点）、`average`（环形滑动平均） |
| `src/fmt/` | `value`（有效数字 / 自适应小数）、`time`（时长格式化） |
| `src/constants.rs` | 物理与单位常量（`GRAVITY` 直接取自 flightmodel，避免两份） |
| `tests/map_image_verbatim.rs` | 集成测试：底图原始字节必须**原样**进记录 |

## 关键函数 Specification

### `async fn main()`（bin）
* 输入：CLI（`--ip`、`--port 8111,9222`、`--font-size`、各面板位置
  `--flight-info-x/y`、`--engine-info-x/y`、`--minihud-x/y`、`--map-x/y`、`--map-size-x/y`、
  列数 `--flight-info-col`、`--engine-info-col`、`--window-width/--window-height`、
  `--data-dir`、`--drag`、`--layoutconfig`；每个选项由 `Args::apply_overrides(&mut cfg)` 按
  **同名逐字**覆盖到布局配置上，加参数只需在那张表里补个名字）。
  **HUD 类型（`hud_type`）故意没有命令行参数** —— 它只从配置读（见下）。
* 输出：进程退出码（`ExitReason` 可 `Display`，打印人话原因）。
* 前置：`config/default.json`（或 `--layoutconfig` 指定文件）可读；字体在
  `resource/fonts` 下存在（缺了 HUD 明确失败）。
* 后置：窗口已建、记录/数据链线程按配置启动、主循环跑在 `refresh_hz` 上；
  退出时停线程、`destroy_window()`。
* 错误：配置/字体/窗口失败 → 报错退出（非 0）；8111 连不上 → 持续重连（不是致命错误）。
* 副作用：建窗口与线程、占 8111 连接、写 `logs/`、按配置播声音。

### `async fn data_process_loop(...)`（bin，主循环）
* 输入：`Channel`、`FlightContext`、`FrameClock`、刷新间隔。
* 输出：无（`ExitReason` 通过返回值/退出路径体现）。
* 前置：`create_window()` 成功（否则 `begin_frame()` 拿不到槽）。
* 后置：每帧 `begin_frame()` → `pre_process()` → `commit_frame()`；地图采样闸门到点才取
  `/map_obj.json` 与写 `MapDisplay`；`link_ring::publish_own()` 每帧一次。
* 错误：单帧解析失败只跳过该帧（不中断循环）；连接中断累计到阈值 → 重连或退出。
* 副作用：写帧槽、写地图槽、发告警声；**不做任何阻塞 I/O**（取数在异步任务里）。

### `fn try_connect(channel: &mut Channel) -> bool`（bin）
* 输入：`Channel`（持有端口列表与当前端口）。
* 输出：是否连上。
* 后置：连上时重置帧环与友军快照（换局语义），并按新连接重置 `fuel0` 等基线。
* 错误：无 `Result` —— 返回 `false` 即调用方继续轮换/等待。
* 副作用：网络探测 + 日志。

### `fn pre_process(...)`（bin）
* 每帧把解析结果推进 `FlightContext` 与 `DisplayData`：计算地图坐标与区域格、
  必要时写底图（`wp8f_disp::set_map_image`，**每次连接只写一次**）、
  跑 `updater::update_display_from_state`。
* 错误：无；不完整数据按 `or_state()`/守卫回退（如油量未读到时不更新）。
* 副作用：写 `DisplayData`/`MapDisplay`/底图槽、可能触发告警音。

### `fn effective_map_obj_frames() -> u64` / `mapobj::period_ms(frames, refresh_hz) -> u64`（bin / `mapobj.rs`）
* `effective_map_obj_frames()` 是**夹紧后的唯一定值来源**（`HudLayoutConfig::
  map_obj_record_every_frames_clamped()` 的结果），主循环闸门、`FlightContext` 初值、
  记录起点 `RecordConfig::start_frame` **三处必须同源**。
* `period_ms`：`frames × 1000 / refresh_hz`（整数除法，向下取整；8 帧 @30 Hz → 266 ms；
  `refresh_hz = 0` 按 1 Hz 处理，不除零）。记录线程的采样周期也用它（见下）。
* 错误：无。副作用：无（纯函数）。

### `fn spawn_recorder_thread(map_obj_frames: u64) -> Option<RecordHandle>`（bin）
* 前置：窗口已建（`wp8f_disp::frame_reader()` 才有值）；配置 `record.enabled`。
* 输出：`None` = 未启用或拿不到帧读句柄（不记录，不影响飞行）。
* 后置：记录线程按**地图刷新周期**采样（`mapobj::period_ms(生效帧数, refresh_hz)`，
  默认 8 帧 @30 Hz = 266 ms —— **记录频率不开放配置**：地图坐标每这么多帧才更新一次，
  记更快只会写下坐标相同的重复帧）；`map_obj_frames` 作为记录起点（跳过本机地图坐标
  还是初值的开头几帧）。错误：不返回 `Result`，失败在 logger 侧打日志。

### `fn spawn_link_thread(l8: &DataLinkConfig) -> Option<LinkHandle>`（bin）
* 前置：`link_ring::init_slots()` 已调用；配置 `datalink.enabled`。
* 输出：`None` = 未启用；否则数据链线程句柄（`stop()` 停止）。
* 后置：主循环此后每帧 `publish_own()`；线程按 `send_hz` 上报、按 50 ms 排空应答。

### `FrameClock` / `Tick`（`frame_clock.rs`，纯）
* `FrameClock::new(ts_start: u64, nominal_interval_ns: u64, map_obj_frames: u64)`：初值钳位，
  不产生第 0 帧的除零/下溢（这正是当年内联在主循环里最容易错的地方）。
* `peek(&self, now: u64) -> Tick`：**只算不推进** —— 给定 `now` 得到本帧的帧号、
  实测间隔、名义间隔、地图采样间隔与是否丢帧（`dropped`）。
* `advance(&mut self, &Tick)` / `accept(&mut self, &Tick)`：把这一次 `peek` 的结果落实为
  新的基准（`accept` 用 tick 里已经算好的间隔，避免重算导致漂移）。
* 错误：无 —— 时间回退按「不前进」处理（`dropped`/间隔钳位），不 panic。副作用：无。

### 解析（`parser.rs`）
`parse_state(&mut FlightState, &str)` / `parse_indicators(&mut Indicators, &str)` /
`parse_map_info(&mut MapInfo, &str)` / `parse_map_obj_data(&mut MapObjData, &str)`
→ 全部返回 `Result<(), ParseError>`
* 前置：输入是 8111 的报文原文（**就地写入**调用方提供的结构体，不分配大对象）。
* 后置：**部分字段更新** —— 报文里没有的键保持原值（游戏会在不同状态下省字段）。
* 错误：`ParseError`（缺必需键/类型不符）；单帧失败只跳过该帧。
* 副作用：无（纯解析，`flat_json` 不做堆分配以外的动作）。

### 状态计算（`display/updater.rs`）
| 接口 | 语义 |
|---|---|
| `update_display_from_state(...)` | 每帧总入口：飞行 → 发动机 → 燃油 → 性能指标 → 告警分级 |
| `warning_stage(value: f64, limit: f64) -> u8` | 通用分级（0 = 正常，值越大越严重） |
| `g_load_warning_level(ny, neg_limit, pos_limit) -> u8` | 过载分级（正负向各按自身限值） |

* 前置：`ctx` 已 `load_flight_model()`（否则 FM 相关字段按无效处理）。
* 后置：文本字段一律经 `set_text*` 写入定长 `FixedBytes`（**不分配**，超长截断）；
  单位按 `imperial` 分叉。
* 错误：无 —— 数据无效时写占位（`---`）而不是报错。副作用：只写传入的 `DisplayData`。

### 语音告警（`warnings.rs`）
`resolve_dir(voice_path) -> Result<PathBuf, String>` / `dir_bases() -> Vec<PathBuf>` /
`preload(voice_path) -> &'static VoicePack` / `pack() -> Option<&'static VoicePack>` /
`pack_summary() -> Option<(String, usize, usize)>` / `VoiceWarning`（冷却与闪烁状态机）
* `resolve_dir`：按路径基准找语音包目录；找不到返回 `Err`（**只报一次**，见
  `report_missing_voice`），此后静默降级为无声。
* `preload`：把整包 wav 读进内存（`is_riff_wave` 校验 RIFF 头），只读表 `'static`。
* `VoiceWarning`：判定 + 冷却 + 「闪烁 X」；`WarningThresholds` 是阈值表（配置可覆盖）。
* 错误：缺包/非 wav → 记日志并跳过该条，不让飞行失败。副作用：读磁盘、`rodio` 播放。

### 机动告警声（`maneuver_tone.rs`）
* `m = MAX(攻角比, 过载比, 超速项)`；`m < 67%` 静默（起音 70%、迟滞到 67% 才停）、
  `70% ≤ m < 100%` 蜂鸣且频率 2→12 Hz 线性加密、`m ≥ 100%` 连续长鸣
  （回落到 97% 以下恢复蜂鸣）。
* 配置项 `maneuver_tone_enabled`（默认开）关掉后**连状态机都不推进**（主循环读一次就定下来）；
  与语音告警（`voice_warnings_enabled`）是两个独立音层，互不影响。
* 时间由调用方注入（`Instant`），因此可单元测试；不进语音冷却/X 体系。错误：无。副作用：播放蜂鸣音。

### 小工具
* `fmt::format_sig_figs(value, sigfigs) -> String`：有效数字格式化（HUD 数值列）。
* `fmt::format_adaptive(v, base_decimals) -> String`：自适应小数位（用于 SFC 等跨量级值）。
* `fmt::format_duration(total_secs) -> String`：秒 → `hh:mm:ss`。
* `math::angle::{normalize_angle_deg, angle_difference_deg, compass_deg, target_bearing_deg,
  round_sep, parse_hex_color}`；`math::map::{normalize_to_map_coords, calc_distance_to_player,
  calc_weighted_distance, calc_map_zone, extract_*_coords}`；`math::average::SimpleMovingAverage`。

## 约束与取舍

* **`flat_json` 不可改动**（用户口径）：它是 8111 专用优化解析器；解析口径变更请先对齐游戏报文。
* **单生产端**：`DisplayData` 只有 core 写，多线程写环是未设防的（`ring` 契约）。
* **地图采样闸门只有一个来源**：`effective_map_obj_frames()`；别在别处再抄一份默认值。
* **油量基线守卫**：以连接起始 `fuel0` 为基线，读不到油量时不更新（避免负数/跳变）。
* **导出面收窄是刻意的**：新增 `pub use` 前先问「谁会用」—— 放开会让死代码重新混进告警。
* **`async` 只在取数与等待**：状态计算、解析、绘制发布都是同步纯逻辑；帧调度抽成
  `FrameClock` 就是为了让这些整数运算能被直接测。
* 退出语义：连不上 8111 会持续重连；累计到阈值（`check_invalid`）才带 `ExitReason` 退出。

## 测试

* 单元测试覆盖：`frame_clock`（稳态/跳拍/时间回退/闸门）、`parser` 与 `flat_json`（真实报文样本）、
  `updater`（告警分级、单位分叉、油耗分支）、`warnings`（阈值/冷却）、`maneuver_tone`（分级/迟滞）、
  `math`/`fmt`/`mapobj`。
* `tests/map_image_verbatim.rs`：底图字节必须原样进记录（不被解码-重编码改写）。
* `scripts/test.sh` 带 `-p wp8f-core` 一起跑；Windows 侧 `scripts/tests/wpr_replay.py`
  用真录制验证 HUD 记录口径。
