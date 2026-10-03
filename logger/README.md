# logger —— 飞行记录与导出转换（`wp8f-logger`）

## 职责

三块互不依赖的能力，用 cargo feature 切开：

| 模块 | 做什么 | 谁用 |
|---|---|---|
| `wpr` | `.wpr` 容器读写：地图信息 + 底图 + N 组逐帧 CSV | 所有人（**唯一无重依赖的部分**） |
| `thread`（feature `thread`，默认） | 记录线程：消费 `disp` 的帧环形缓冲区 → 内存池 → 收尾落盘 | core |
| `convert`（feature `convert`） | 纯函数转换器：`.wpr` → TacView ACMI / TacView CSV / FlatCSV | 控制台（`wp8f-gui.exe`） |

控制台以 `default-features = false` 依赖本 crate 解析 `.wpr`，因此 **HUD 那套重依赖
（disp / winit / 内嵌字体）不会被拖进 GUI 进程**。

## 实现原理

* **一份记录 = 一个文件**：meta（JSON）+ 底图（原样字节，通常 JPEG）+ G 份 CSV。
  只存 wp8f 自己的口径（归一化地图坐标、km/h、百分比操纵面、UNIX 纳秒、HUD 俯仰符号），
  **不含任何外部格式的概念**；单位/符号换算全部推迟到 `convert` 里做。
* **记录期间不碰磁盘**。每帧只往该组的 `CsvPool` 追加一行 CSV，飞行结束才把
  meta + 各组 CSV + 底图拼成一个 `.wpr` 一次写出 —— 既没有逐帧 I/O，也不用在结尾再做
  一遍全量序列化。内存池按 `pool_mb` 预分配（默认 8 MiB/组），写满自动扩容。
* **采样口径 = 轮询口径**。线程只取环形缓冲区里**最新**一帧，从不 drain 历史帧，
  所以「记录频率 = 轮询频率」，帧率高于记录频率时多出来的帧计入
  `skipped`（正常现象，不是丢帧）。一轮轮询之间写端绕过一整圈（超过槽数）导致读到
  混合帧时计入 `overwritten`。
* **`poll_ms` 由调用方（core）给出，不是配置项**：core 直接取地图刷新周期
  （`period_ms(map_obj_record_every_frames, refresh_hz)`，默认 8 数据帧 @30 Hz = 266 ms）——
  地图坐标每这么多帧才更新一次，记更快只会写下坐标完全相同的重复帧。logger 自己不认识
  任何配置默认值；`RecordConfig::default()` 里的 100 ms 只是给独立使用者的兜底。
* **起始门槛**（`map_ready`）：`DisplayData.frame > start_frame` 才开始记。本机地图坐标
  在地图对象采样到位前是初值 `(0.5, 0.5)`，不设门槛的话录制开头会记成地图中心、
  回放时「闪现」到真实位置。`start_frame` 由 core 传入（= 生效的地图记录间隔帧数），
  logger 自己不认识任何配置默认值。
* **多机是预留接口**。组 0 恒为玩家（core 只喂这一组，记录线程自己从环形缓冲区读）；
  其他飞机走 `register_group()` 领组号（恒 ≥ 1）+ `write_row()` 追加行。记录线程独占内存池，
  所以这两步走 channel：`write_row` 用 `try_send`，队满/线程已停就返回 `false` 并计入
  `dropped_other` —— **记录绝不反过来拖慢调用方**。
* **重连可见**。环形缓冲区的代数前进（`reset`）会被 `poll()` 报出来并累计到 `reset`；
  收尾时抓最新地图（信息 + 底图字节）写进容器。

## 结构图

```
 core 主循环                                   wp8f-logger
 ┌───────────────┐   publish     ┌───────────────────────────────────────────────┐
 │ DisplayData   │──────────────►│ Ring<DisplayData>（wp8f-disp，默认 32 槽）    │
 └───────────────┘               └───────────────┬───────────────────────────────┘
                                                 │ FrameReader::poll（只取最新）
                                                 ▼
  其他飞机（预留接口）              ┌──────────────────────────────┐
  RecordHandle::write_row ──ch──► │ 记录线程 run()               │
  RecordHandle::register_group ──►│  · map_ready 门槛            │
                                  │  · to_csv_line 追加一行      │
                                  └───────┬──────────────────────┘
                                          ▼
                       groups[0..G] = CsvPool（预分配 + 自动扩容，全程内存）
                                          │ stop / Drop
                                          ▼
                       meta + csv[0..G] + img ──encode──► logs/wp8f_<unix_ms>.wpr
                                                                    │
                                             ┌──────────────────────┴───────────────────┐
                                             ▼                                          ▼
                                  控制台回放（gui/static/replay.js）        convert：ACMI / CSV
```

容器布局（小端，版本 2）：

```
offset 0   magic  4B  "WPR1"
offset 4   u32       容器版本（= 2）
offset 8   u32       meta_len     JSON 元信息长度（UTF-8）
offset 12  u32       group_count  CSV 组数（恒 ≥ 1；组 0 = 玩家）
offset 16  u32       img_len      底图长度（原样字节）
offset 20  u32 × G   csv_len[]    每组 CSV 长度（含表头），按组号排列
offset 20+4G ...     meta | csv[0] | csv[1] | … | csv[G-1] | img
```

组数表放在 meta 之前：先读长度表就能定位每一段，不必先解 JSON。meta 里另有一份
`groups` / `group_info[]`（机型 + 帧数），解码时与头里的 `group_count` 交叉校验，
不一致直接报错、不猜。列义与小数位见 `src/wpr.rs` 的 `CSV_HEADER` 与 `COL_DECIMALS`
（改列只动这三处 + 写入端）。

## 代码目录

| 路径 | 内容 |
|---|---|
| `src/lib.rs` | feature 门面：导出 `wpr`；`thread` / `convert` 按 feature 编译 |
| `src/wpr.rs` | 容器与行：常量（魔数/版本/表头/小数位）、`RecordRow`、`CsvPool`、`Record`、`ContainerHead`、`MapMeta` |
| `src/thread.rs` | 记录线程：`RecordConfig`、`RecordStats`、`RecordHandle`、`spawn_recorder`、`map_ready`、收尾写盘 |
| `src/convert.rs` | 三种导出格式的纯函数转换器 + `Origin`（零点经纬度） |

依赖方向：`thread` → `wp8f-disp`（帧环形缓冲区、`DisplayData`、`map_image`）；
`wpr` / `convert` 只依赖 `serde` / `serde_json`。测试都在 `src/*.rs` 的 `mod tests` 内
（`wpr.rs` 覆盖容器往返与列对齐，`thread.rs` 覆盖门槛/统计/预留接口）。

## 关键函数 Specification

### `spawn_recorder(cfg: RecordConfig, reader: FrameReader) -> Option<RecordHandle>`
* 输入：`cfg` —— `{ enabled, output_dir, poll_ms, pool_mb, start_frame }`（core 从布局配置搬来：
  `enabled` / `output_dir` / `pool_mb` 取自 `record` 段，`poll_ms` 与 `start_frame` 由 core 按
  地图刷新周期现算）；`reader` —— 帧环形缓冲区的消费端（core 在窗口建好后给出）。
* 输出：`Some(句柄)`；`enabled == false` 时 `None`（调用方据此跳过记录，无需自己判配置）。
* 前置：`reader` 指向的环在生产端仍在写（单生产端契约由 `ring` 保证）。
* 后置：线程已启动，输出目录在**收尾时**才创建；`poll_ms == 0` 钳到 1 ms，
  `pool_mb == 0` 取默认 8、上限 4096。
* 错误：线程创建失败 → `eprintln!` 一行并返回 `None`（本次连接不记录，不影响飞行）。
* 副作用：起一个 OS 线程、分配 G × 池容量内存、启动时打一行 `[RECORD]` 说明。

### `RecordHandle` 上的方法
* `stats() -> RecordStats`：读共享原子计数（`recorded` / `skipped` / `overwritten` /
  `reset` / `waiting_sample` / `other_rows` / `dropped_other`）。无副作用、无错误。
* `poll_hz() -> f64`：`1000 / max(poll_ms, 1)`，给 GUI/日志显示记录频率。
* `register_group(aircraft: &str) -> u64`：领组号（`>= 1`，组 0 是玩家不可占用）。
  入队失败（队满/线程已停）**仍返回组号**，后续 `write_row` 会如实返回 `false` —— 不静默丢数据。
* `write_row(group: u64, row: RecordRow) -> bool`：`group == 0` 直接 `false` 并计入
  `dropped_other`；否则 `try_send`，成功计入 `other_rows`，队满/断开返回 `false`。
  前置：同组的 `Add`（即 `register_group`）先发出 —— 同一发送端顺序有保证。
* `stop_and_write(self)` / `Drop`：置停止标志 → `join` → 收尾写盘。**幂等**
  （`JoinHandle` 只 `take` 一次），`Drop` 也会走同一条路，忘调也不会丢记录。

### `RecordRow::to_csv_line(&self) -> String` / `from_csv_line(line: &str) -> Result<Self, String>`
* 输入：`to_csv_line` 无；`from_csv_line` 一行 CSV。
* 输出：逗号分隔的一行（`time_ns,type,43 个数值列`）；解析结果或错误信息。
* 前置：`v` 与 `CSV_HEADER` 第 3 列起一一对应。
* 后置：写入端把非有限值（NaN/±inf）归零、机型里的逗号替换掉，保证列数恒为 45；
  读端列数不符 → `Err`，空字段按 0.0 处理。
* 错误：仅 `from_csv_line` —— 列数不符 / `time_ns` 非法 / 某列不是数字，均为 `Err(String)`。
* 副作用：无（纯函数）。

### `Record::encode(&self) -> Result<Vec<u8>, String>` / `decode(bytes: &[u8]) -> Result<Self, String>`
* 输入：一份记录（meta + 各组 + 底图）/ 完整文件字节。
* 输出：完整 `.wpr` 字节（见布局）；解码结果。
* 前置：`group_count >= 1` 且组 0 是玩家。
* 后置：`decode` 校验魔数、版本（**只认 2**）、各段长度与 `group_count` 一致性。
* 错误：`Err(String)` —— 魔数/版本不符、长度越界、meta 里的组数与头不一致、JSON/CSV 解析失败。
* 副作用：无。

### `ContainerHead::decode(bytes) -> Result<Self, String>` / `img_len()` / `image(bytes) -> &[u8]`
* 只解固定头 + 组数表，**不解 meta/CSV** —— 控制台列表页据此快速取机型、帧数、底图。
* `image()` 返回原样底图字节的借用；越界/头不完整 → 解码期即报 `Err`，`image()` 不再失败。

### `CsvPool::{new(capacity_bytes), push(&RecordRow), into_group(), rows(), bytes(), text()}`
* 每组的行缓冲：`push` 追加一行 CSV（超出容量翻倍扩容），`into_group()` 转成 `RecordGroup`
  （含机型 = 首行 `type` 列）。
* 错误：无 —— 扩容失败即进程 OOM（不做可恢复设计）。
* 副作用：只改自身内存。

### `convert` 三个导出函数（feature `convert`）
| 函数 | 输出 | 零点经纬度 |
|---|---|---|
| `flat_csv(&Record) -> String` | 记录里组 0 的那份 CSV（wp8f 口径，**零换算**） | 不需要 |
| `to_tacview_acmi(&Record, Origin) -> String` | TacView ACMI 2.2，多机组按时间合流 | 需要 |
| `to_tacview_csv(&Record, Origin) -> String` | TacView Real-life CSV（18 列，**只导玩家组**） | 需要 |

* 前置：`Origin`（默认莫斯科市中心 `55.7558, 37.6173`，与旧配置默认值一致）代表地图中心
  `(0.5, 0.5)` 的真实经纬度；`to_lon_lat` 用 `MapMeta::norm_to_world_m` 换算。
* 后置：纳秒 → 秒/毫秒、km/h → m/s、百分比 → ratio、`pitch` 取负（ACMI 抬头为正）
  都在这里完成。
* 错误：三个函数都**不返回 `Result`** —— 缺组/缺帧时输出空表或跳过该组。
* 副作用：无（纯函数，不做 I/O）。

### `MapMeta::norm_to_km` / `norm_to_world_m` / `norm_to_world_km` / `world_rect_km`
归一化地图坐标（0..1）↔ 千米/米：`norm_to_world_m` 是唯一原始换算，其余是它的派生
（`km = m / 1000`）。HUD 地图面板与 `convert` 共用同一口径。

## 约束与取舍

* **只有一个版本**：`VERSION = 2`（v1 的兼容分支已彻底删除）。读到版本 ≠ 2 直接报错。
* **定长数组列**：`RecordRow::v: [f64; 43]` + `CSV_HEADER` + `COL_DECIMALS` 三处必须同步；
  增删列只动这三处 + 写入端，读写都是循环（`column_index_matches_header` 测试逐列核对）。
* **非有限值一律归零**（`fin()`）写进 CSV，避免 `NaN` 传染下游回放/转换。
* **写盘失败只报错不重试**：`[RECORD] write failed (...)` 一行 `eprintln!`，本次记录丢失，
  不影响飞行；目录创建失败同理。
* 记录频率就是轮询频率（`poll_ms`，core 按地图刷新周期给出，默认 266 ms ≈ 3.8 Hz）：
  帧率高于它时多出来的帧计入 `skipped`，这不是「丢帧」，是设计口径。
* 配置项在布局配置的 `record` 段（`disp::HudLayoutConfig`）：`enabled` / `output_dir`
  （空 = `./logs`）/ `pool_mb`；**没有记录频率这一项**（跟随地图刷新）；文件名 `wp8f_<unix_ms>.wpr`。

## 测试

* 单元测试（`cargo test -p wp8f-logger`）：`wpr.rs` —— 容器 encode/decode 往返、列名与
  下标对齐、CSV 行往返与非法输入、`CsvPool` 扩容；`thread.rs` —— `map_ready` 门槛、
  统计计数（`waiting_sample` / `other_rows` / `dropped_other`）、组 0 拒写。
* 端到端：`scripts/test.sh` 跑全工作区单测（含 `-p wp8f-logger`，默认 feature）；
  Windows 侧的 `python scripts\tests\wpr_replay.py` 用真实录制做回放校验，
  控制台回放页由 `scripts/tests/ui_checks.py` 覆盖。
