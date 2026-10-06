# disp —— HUD 绘制、窗口与共享状态（`wp8f-disp`）

## 职责

HUD 的**显示层**，也是 core 与 HUD/logger 之间的**共享状态中转站**：

* 拥有窗口与渲染线程（winit + softbuffer，软件光栅到 `u32` 像素缓冲）；
* 把 core 发布的帧数据画成 HUD（两种样式：`circle` 圆环 / `mini` 紧凑面板）；
* 提供进程级的环、底图、布局配置、拖拽状态、字体与界面文案。

它**不产生任何飞行数据**：core 填 `DisplayData`/`MapDisplay`，disp 只负责画和转发。

## 实现原理

* **数据流是「发布—直读」，不是消息传递**。core 每帧 `begin_frame()` 拿到环里下一个待写槽
  （`&'static mut DisplayData`），填完 `commit_frame()` 发布；渲染线程直接
  `ring.latest()` 读，logger 用 `FrameReader::poll_latest()` 读。零拷贝、无锁、无 channel，
  发布端不回写已发布的槽（契约见 `ring/`）。
* **两个环 + 一块底图**。帧环 `Ring<DisplayData>`（`ring.frames`，缺省 32 槽 ≈ 1 s @30 Hz）、
  地图环 `Ring<MapDisplay>`（`ring.map_samples`，地图采样频率写、渲染每帧读）；
  底图 `MapImage` **不进环** —— 它走 `OnceLock<Mutex<MapImage>>` + 两个 `Arc`
  （解码后的 RGB 给绘制、原始字节给记录），换图不打断正在画的读者。
* **窗口线程是唯一的绘制者**。winit 事件循环跑在 disp 自己起的线程里，每帧取
  `ring_buffer.latest()` + `get_map_data()`，调 [`hud::draw_hud`] 画满整块缓冲，
  再由 softbuffer present；core 只在数据侧调 `signal_redraw()` 催一帧。
* **`hud::draw_hud` 是唯一渲染入口**：按配置的 `hud_type` 三态分派 ——
  `0` 关闭（不画 HUD 本体）、`1` `mini_hud::draw_mini_hud`（紧凑面板）、
  `2` `circle_hud::draw_circle_hud`（圆环）；地图面板与友军数据标签面板由它自己调
  `map_panel` / `link_panel`（与 HUD 类型无关，始终按各自开关画）。
  两种样式共用的判定（告警分级/配色/区域格/闪烁）都在 `hud_common`，避免两份口径。
* **配置一个进程级 `OnceLock`**：`HudLayoutConfig`（serde）由 core 从
  `config/default.json` 读入后 `init_hud_layout()` 装上；运行期只读（`hud_layout()`），
  GUI 拖拽后由 GUI 侧写回 JSON。颜色在内存里是 `0xAARRGGBB` 打包值，配置文件里写
  `#RRGGBBAA`，换算只在一处做。
* **字体是 mmap 的，不是全量驻留**。启动时 `font::init()` 把字体文件 `memmap2` 映射
  （`Box::leak` 拿到 `'static` 字节），字形按需从映射里取；`resident_bytes()` 报的是
  **映射长度**，不是 RSS。缺字体明确失败（`FontSource::Missing`），不静默降级。
* **界面文案两层**：`resource/i18n/*` 的键值表（Rust 与前端共用）+ 内嵌 `en` 兜底；
  `t(key)` 取当前语言，缺键回落 `en`。

## 结构图

```
            core（生产端，单线程）                          disp 内部
  ┌───────────────────────────────────────┐   ┌─────────────────────────────────────────┐
  │ begin_frame()  → &mut DisplayData     │   │ Ring<DisplayData>（ring.frames，32）    │
  │   … 填飞行/发动机/告警字段 …          │──►│                                         │
  │ commit_frame()                        │   └───────┬─────────────────────┬───────────┘
  │                                       │           │ latest()（零拷贝）  │ poll_latest()
  │ begin_map_frame() → &mut MapDisplay   │           ▼                     ▼
  │ commit_map_frame()                    │   ┌──────────────────┐   ┌───────────────────┐
  │ set_map_image(rgb, raw, w, h)         │   │ 渲染线程         │   │ logger 线程       │
  │ signal_redraw()                       │   │ （winit 事件循环）│   │ （.wpr 记录）     │
  └───────────────────────────────────────┘   │  draw_hud(buf,   │   └───────────────────┘
                                              │    w, h, latest, │
  OnceLock<Mutex<MapImage>> ◄─────────────────│    drag_state)   │
  OnceLock<Ring<MapDisplay>> ◄────────────────│      │           │
  OnceLock<HudLayoutConfig> ◄── init_hud_layout│      ▼           │
  font（mmap）/ i18n（键值表）────────────────►│ 像素缓冲 → softbuffer → 窗口                │
                                              └─────────────────────────────────────────┘
                                                   │            │            │
                                    circle_hud / mini_hud   map_panel    link_panel
```

拖拽（`--drag`）时同一入口换个用法：窗口把鼠标事件交给 `DragState`，命中哪个 `Panel`
就改哪个 `Slot` 的位置，松手写回布局配置。

## 代码目录

| 路径 | 内容 |
|---|---|
| `src/lib.rs` | 门面：窗口与帧循环、两个环、底图、`DisplayData`/`MapDisplay`、布局配置、拖拽、`FixedBytes` |
| `src/hud/hud.rs` | `draw_hud`：样式分派 + 两种样式共用的面板排布 |
| `src/hud/map_panel.rs` | 地图面板：底图缩放/旋转、网格与区域格、地图对象与友军标记、POI |
| `src/hud/link_panel.rs` | 友军数据标签面板（`panel_font/panel_width/panel_height` + 行渲染） |
| `src/circle_hud.rs` | 圆环样式（Rafale 风格）：刻度、指针、状态行 |
| `src/mini_hud.rs` | 紧凑面板样式（默认）：飞行/发动机数据块 |
| `src/hud_common.rs` | 两种样式共用的判定与画法：告警分级与配色、闪烁、区域格标签、AA 弧带、状态行 |
| `src/draw/` | 光栅原语：`primitives`（线/像素/AA 混合）、`text`、`glyph_cache`（字形缓存与 alpha 混色） |
| `src/components/` | 绘制上下文 `draw_context`、字号缩放 `text_scale`、主题 `theme` |
| `src/font.rs` | 字体查找/`mmap`/选择结果（`FontSource`、`FontChoice`） |
| `src/i18n.rs` | 界面语言：`resource/i18n` 表 + 内嵌 `en` 兜底、`t()`/`tf()` |
| `src/paths.rs` | 共享路径基准（cwd → 父目录 → exe 目录 → exe 父目录，去重） |
| `src/geometry.rs` | 内部几何工具（crate 私有） |
| `tests/hud_lang_release.rs` | 集成测试：语言文件读完后中间对象必须释放（计数分配器） |

## 关键函数 Specification

### 窗口与帧循环（`lib.rs`）
| 接口 | 输入 → 输出 | 前置 / 后置 / 错误 / 副作用 |
|---|---|---|
| `create_window() -> Result<(), DispError>` | — → 窗口线程已启动 | 全进程只调一次（`OnceLock`）；重复调用返回 `Err(DispError)`。副作用：起线程、建窗口、按 `ring.*` 分配两个环 |
| `set_window_size(w: f32, h: f32)` | 逻辑尺寸 | 供 GUI 改窗口比例；无错误 |
| `set_window_visible(visible: bool)` / `signal_redraw()` | — | 只置标志/投递重绘，不阻塞；可在任意线程调 |
| `destroy_window()` | — | 停事件循环并 `join` 线程；`Drop` 之外显式调用（退出路径） |
| `DispError` | — | `thiserror` 枚举：窗口创建失败、事件循环失败等；不 panic |

### 帧数据发布（生产者侧，core 调）
* `begin_frame() -> Option<&'static mut DisplayData>`：取环形缓冲区下一个待写槽
  （未重置为该帧的 `Default`）。`None` = **窗口还没建起来**（`RING_BUFFER` 未初始化），
  调用方必须处理。不阻塞、无错误。
* `commit_frame()`：发布刚写好的那一槽（`commit()`）。**必须在 `begin_frame()` 之后**
  （core 的告警检查也要求在 `commit_frame()` 之前写完）。
* `begin_map_frame() -> Option<&'static mut MapDisplay>` / `commit_map_frame()`：同上的地图版，
  按地图采样频率调用（不必每帧）。
* `get_map_data() -> Option<&'static MapDisplay>`：渲染侧直读最新地图快照，不推进任何游标。
* `reset_ring_buffer()`：换连接时推进环的代数（`reset()`），让读者识别断流（帧序号作废）。
* `set_map_image(rgb: Arc<Vec<u8>>, raw: Arc<Vec<u8>>, w: u32, h: u32)`：**core 在地图加载时
  调一次**；`rgb` 给绘制、`raw` 给记录（原样字节，重编码会失真且更大）。
  `map_image() -> MapImage` 每次 clone 两个 `Arc`（换图前的旧读者仍安全），不阻塞绘制。

### 帧数据消费（logger 侧）
`frame_reader() -> Option<FrameReader>` + `FrameReader::poll_latest() -> PollOutcome`
* 输出：`PollOutcome { frame: Option<&'static DisplayData>, skipped: u64,
  overwritten: bool, reset: bool }`。
* 前置：`frame_reader()` 只能在窗口建好后调（否则 `None`）；句柄 `Send`，内部只有
  `&'static` 与游标 —— 起点与「当前」同步（**不补发**创建之前的历史帧）。
* 后置：只取最新一帧的借用，不推进生产端、不清历史帧；`skipped` = 跳过的历史帧数，
  `overwritten` = 一轮轮询之间的新帧数超过环容量（`skipped + 1 > slots`），
  `reset` = 生产端重置过（此时 `frame = None`，重置本身不算一帧）。
* 错误：无。副作用：只推进自己的游标。

### 布局配置
`init_hud_layout(cfg: HudLayoutConfig) -> Result<(), String>` / `hud_layout() -> &'static HudLayoutConfig`
* 前置：全进程只装一次（`OnceLock`）；重复装返回 `Err`（不静默覆盖）。
* 后置：此后所有绘制读同一份配置；`HudLayoutConfig` 覆盖位置、颜色、字号、
  `hud_type`、`ring`、`record`、`datalink`、字体与语言路径。
* **`hud_type` 是 HUD 本体的唯一开关**（`HudType`：`0` 关闭 / `1` MiniHUD / `2` 圆环，缺省 `1`）：
  `0` 时只画飞行/发动机/地图/友军面板，圆环与 miniHUD 都不画、拖拽也不给这一格。
  配置里写数字；为兼容旧配置同时接受 `"off"`/`"mini"`/`"circle"`。
  历史包袱：曾并存 `circle_enabled` / `minihud_enabled` 两个 bool，于是"改了类型没改开关"
  会画出空白屏 —— 这两个键**已删除**（配置里若还有，被当成未知键忽略）。
* 声音两个独立开关（都默认开）：`voice_warnings_enabled`（语音包播报）与
  `maneuver_tone_enabled`（F-18 风格机动告警声）；关掉只影响声音，视觉告警照旧。
  机动告警声另有**表速门限**：表速 ≤ 64 km/h（`core::maneuver_tone::MIN_IAS_KMH`）不出声，
  免得地面滑跑时低速大攻角误触发。
* 派生量（都带钳位）：`refresh_hz_clamped() -> u32`（`REFRESH_HZ_MIN = 5` ..
  `REFRESH_HZ_MAX = 60`，缺省 30）、`refresh_interval_ns()`、
  `map_obj_record_every_frames_clamped()`（`1..=refresh_hz`，缺省 `MAP_OBJ_INTERVAL_FRAME = 8`）。
* `RingConfig::{frames_clamped, map_samples_clamped}`：槽数钳在 `2..=256`（缺省 32）；
  下限来自 `Ring::MIN_SLOTS`，上限即 `RING_SLOTS_MAX`。
* `load_layout_config(path) -> Option<HudLayoutConfig>` / `save_layout_config(&cfg)` /
  `set_layout_config_path(String)`：读写 JSON；解析失败返回 `None`（调用方回默认），
  保存失败**静默**（用户裁定，改动风险不值当）。

### 拖拽（`--drag`）
* `set_drag_mode(bool)` / `is_drag_mode() -> bool`：进程级开关（原子）。
* `Panel`（`Flight`/`Engine`/`Map`/`Hud`/`Link`，顺序 = 命中优先级）与
  `Slot`（`Flight`/`Engine`/`Map`/`MiniHud`/`Circle`/`Link` —— 比面板多一格：两种样式的
  HUD 位置各存各的）：内部 `Panel::slot(hud_type)` 把 `Hud` 按样式分叉，其余一一对应。
* `DragState { active: Option<Panel>, pos: [[i32; 2]; 6] }`：
  `from_config(&HudLayoutConfig)` 取初值、`slot_pos(Slot)`/`panel_pos(Panel, HudType)` 读、
  `set_panel_pos(...)` 写、`handle_press(mx, my)` 命中测试并开始拖拽。
* `save_drag_state(&DragState)`：拖拽结束写回配置（失败静默）。

### 绘制入口
| 接口 | 说明 |
|---|---|
| `hud::draw_hud(buffer: &mut [u32], width, height, data: &DisplayData, drag: &DragState)` | **唯一渲染入口**；按 `hud_type` 分派，并画地图/友军面板 |
| `circle_hud::draw_circle_hud(buffer, width, height, data, map, cx, cy)` | 圆环样式（`cx/cy` = 圆心） |
| `mini_hud::draw_mini_hud(buffer, width, height, data, map, drag_mode, drag_x, drag_y)` | 紧凑样式（默认） |
| `hud::map_panel::draw_map_panel(...)` | 地图面板（底图 + 网格 + 对象 + 友军） |
| `hud::link_panel::{draw_link_panel, panel_font, panel_width, panel_height}` | 友军数据标签面板及其尺寸 |
| `hud::all_label_keys() -> Vec<&'static str>` | 面板用到的全部文案键（`i18n` 自检/覆盖检查用） |
| `draw::{draw_line, set_pixel, draw_text_raw, measure_text_raw}` | 光栅原语与文本度量（crate 外也可用） |

* 前置：`buffer.len() >= width * height`；`init_hud_layout()` 已成功；字体已初始化
  （`font::init()`），否则字画不出来。
* 后置：只写 `buffer`；不改 `data`/`map`（都是 `&`）。
* 错误：无 `Result` —— 越界坐标由各画法自行剪裁；字体缺失在启动期就报错。
* 副作用：字形缓存（`glyph_cache`）会被填充（首次绘制某字号时）。

### 字体
`font::{init(font_path: &str) -> &'static FontChoice, icon_init() -> &'static FontChoice,
data() -> &'static [u8], icon_data(), choice(), icon_choice(), font_dirs() -> Vec<PathBuf>,
missing_error() -> Option<String>, resident_bytes() -> usize}`
* `init`/`icon_init` 幂等（`OnceLock`）：正文按配置 `font_path`、图标固定 `ICON_FILE`。
* `FontSource::{Config, Icon, Missing}`：`Missing` = **明确失败**（`missing_error()` 给原因），
  不静默降级成别的字体；`FontChoice` 记录请求值/实际解析到的文件/读入字节数/
  是否疑似非等宽/失败原因，供启动日志与 GUI 的 `/api/fonts` 使用。
* `font_dirs()`：按 `paths::bases()` 找 `<基准>/resource/fonts`（去重）。
* `resident_bytes()`：**映射长度**（mmap 按需驻留），不是进程 RSS。
* 错误：`init` 不返回 `Result` —— 失败体现在 `FontChoice.source == Missing` 与
  `missing_error()` 上。副作用：`mmap` 文件、泄漏映射以获得 `'static` 生命周期。

### 界面文案（`i18n`）
* `init(hud_lang_path: &str) -> Result<LangInfo, String>` / `set_lang(&str) -> Result<LangInfo, String>`：
  启动时初始化、运行期切换；语言来自 `resource/i18n`，内嵌 `en` 兜底（`embedded_en()`）。
* `t(key) -> &'static str` / `tf(key, args) -> String`：取文案；**缺键回落 `en`**。
* `table(lang) -> Option<Arc<Table>>`、`lang_files() -> Vec<String>`、`dir()`、
  `pick(hud_lang_path) -> (String, String, String)`（解析配置给出的语言路径候选）、
  `active_bytes() -> usize`（当前语言表占用）、`Table`/`LangInfo`。
* 错误：`set_lang` 对不存在的语言返回 `Err`（保持原语言不变）。
* 副作用：读磁盘、缓存已加载的表（`Arc<Table>`）。

### HUD 公共判定（`hud_common`，两种样式共用）
| 接口 | 语义 |
|---|---|
| `sep_warning_level(sep: f64) -> u8` | 特定能量告警分级（供配色） |
| `ias_warning_level(&DisplayData) -> u8` / `ias_warning_color(&DisplayData, normal) -> u32` | 表速告警分级/配色 |
| `warning_level_color(level, normal)` / `g_warning_color(level, normal)` | 通用告警色 / 过载告警色 |
| `warning_blink_visible() -> bool` | 告警闪烁相位（按配置的 `warning_blink_hz`） |
| `map_zone_of(&MapDisplay, x, y) -> String` / `map_zone_label(...)` | 归一化坐标 → 区域格（如 D3） |
| `status_items(&DisplayData, &HudColors) -> Vec<(String, u32)>` | 状态行条目（文案 + 颜色） |
| `legacy_glyph_color(u32) -> u32` / `blend_pixel_cov(...)` | 旧观感补偿 / 覆盖率混合 |
| `draw_arc_band_aa(...)` / `draw_warning_x(...)` / `draw_voice_alarm_x(...)` / `draw_inline_items(...)` | 共用画法（AA 弧带、警示叉、语音告警叉、行内条目） |

* 错误：无。副作用：无（纯计算；画法只写传入的缓冲）。

## 约束与取舍

* **`DisplayData` 必须是 POD**：它进环，读者拿的是借用。所以字符串一律用
  `FixedBytes<N>`（`as_str()/assign()/clear()`，定长、无堆），`map_objects` 那种
  变长数据只出现在**地图环**（`MapDisplay`）里 —— 那是对 `ring` POD 契约的
  **有意例外**（写端约 4 Hz，读者落后 32 个采样 ≈ 8 s 才会撞上，见槽声明处的注释）。
* **颜色两套表示**：内存 `0xAARRGGBB` 打包，配置文本 `#RRGGBBAA`；文本色默认值按
  「预乘 + alpha 自乘」的旧观感补偿过 —— 改它们等于改观感，别按直觉改。
* **字体缺失即失败**：不内嵌、不降级（打包脚本因此只带 `FONT_FILES` 三个文件）。
  mmap 让字库不整份进内存，但**不要**把 `resident_bytes()` 当 RSS 读。
* **配置只读一次**：`init_hud_layout` 之后运行期不再变（除拖拽写回文件）；
  GUI 改配置走「重启 HUD」而不是热更新。
* **`save_layout_config` 写失败静默**（用户裁定）：磁盘满/只读时不打断用户操作。
* 依赖方向：`disp` → `datalink`（取协议类型与常量）、`disp` → `ring`；
  **`datalink` 不反向依赖 `disp`**，所以本机状态用 `link_ring::OwnState` 中转。

## 测试

* 单元测试在 `src/*.rs` 的 `mod tests`：配置钳位与缺键默认、拖拽命中与槽位分叉、
  环消费（`skipped`/`overwritten`/`reset` 口径）、区域格与告警分级、颜色换算。
* `tests/hud_lang_release.rs`：装计数分配器量「读完语言文件后只剩那张 `Box::leak` 的只读表」
  —— 文件文本与 `serde_json::Value` 中间对象必须被释放（与 `flightmodel/tests/json_release.rs`
  同一套做法）。
* `scripts/test.sh` 带 `-p wp8f-disp` 一起跑；HUD 真渲染由 Windows 侧
  `scripts/tests/{layout_check,tab_check,ui_checks}.py` 覆盖。
