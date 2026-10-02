# flightmodel —— 飞行模型解析与气动计算（`wp8f-flightmodel`）

War Thunder 飞行模型（FM）的**解析 + 计算 + 数据更新**：HUD（`wp8f-core`）每帧拿它算
过载限值/推力/阻力/重量，控制台（`wp8f-gui`）用它的 CLI 出曲线，两侧共用同一份实现。

## 职责

* 读 `resource/data` 下的机型数据（`.blkx` 文本，JSON 或旧格式），扁平化成一套键值；
* 把键值算成气动/发动机/质量/武器数据（`FlightModel` 及其子结构）；
* 供 HUD 查询：`allowed_load_factor`（结构过载限值）、`get_cl_aoa_max`、`get_vne`、
  `thrusts_at`/`powers_at`、`fuel_rate_at`、`flight_weight` 等；
* 提供独立 CLI（列机型、解析、曲线、EM、阻力分解、最大平飞速度）与 FM 数据库更新器（P3）。

## 实现原理

* **两套数据源，一个出口**。扁平化后是**同一套键值**（实测键集合 100% 相同）：
  JSON（GitHub `War-Thunder-Datamine`，**默认** feature `fm-json`）与旧游戏解包
  `.blkx` 文本（`fm-legacy`，**仅备份**，要显式打开才编译）。分派在
  `parser::parse_fm_text` 里按**内容**判断（首个非空白字节 `{` 即 JSON）——
  上层（`flight_model.rs`、武器、GUI、logger）完全不需要知道数据是哪一种。
  两个 feature 都关掉会在编译期 `compile_error!`（没有解析器不是合法配置）。
* **数据根只有一处实现**。`fm_paths::resolve_data_root` 是 HUD 与 GUI 共用的唯一入口
  （`$WP8F_DATA_DIR` → cwd/exe 附近的 `resource/data` → 更新器暂存根 `resource/data_new`）；
  `normalize_blkx_ref` 是 `.blk → .blkx` + 小写的唯一入口（`fmFile` 写的是 `.blk`，
  实际文件是 `.blkx`，Linux 上大小写敏感会直接读不到）。
* **解析层与计算层分开**。`parser`（+`json_adapter`）只负责文本 → `BlkxValue` 键值表；
  `flight_model`/`aero`/`engine_model`/`weapon_model` 负责把键值算成领域结构；
  `em` 在最上层做能量机动求解。三者都不做 I/O 之外的副作用，路径解析集中在 `fm_paths`。
* **气动按 Dagor 口径**：大气/声速（`calculate_density`、`calculate_sound_speed`）、
  极曲线阻力（CX0 马赫波阻 + 诱导阻力 + 失速后增长）、`CY_MAX` 马赫曲线（`cl_max_mach_corrected`）、
  结构限值 `n = sign(F)·(2|F|/(m·g) − 1)`（`CritOverload` 单位是牛顿）。单位统一 SI，
  对外给 HUD 的盘旋率是 deg/s。
* **可变后掠翼是一等公民**：`wing_sweep0..3` 各有自己的几何/极曲线，查询接口都带
  `sweep` 参数；`is_variable_sweep()` 决定 HUD 显示哪些档。
* **更新器是 A/B 分区**：`update/` 负责版本检查（`version.rs`）、清单（`manifest.rs`）、
  逐文件下载与校验（`download.rs`/`net.rs`）、切换（`switch.rs`），暂存根就是
  `resource/data_new` —— 与 `fm_paths` 同处一个 crate，因为**数据根的唯一实现在这里**。

## 结构图

```
 resource/data（用户本地数据，不入 git）
   ├── gamedata/flightmodels/<机型>.blkx      主文件（含 fmFile 引用）
   ├── gamedata/flightmodels/fm/<机型>.blkx   FM 文件
   └── gamedata/weapons/<武器>.blkx
        │                        ▲
        │ 读                     │ 更新器（P3，A/B 分区）
        ▼                        │ resource/data_new ──切换──► resource/data
 ┌──────────────────────┐   ┌────┴──────────────┐
 │ parser（按内容分派） │   │ update/           │
 │  ├ json_adapter      │   │ version/manifest  │
 │  └ legacy blkx       │   │ download/net      │
 │   → BlkxValue 键值   │   │ switch            │
 └──────────┬───────────┘   └───────────────────┘
            ▼
 ┌───────────────────────────────────────────────────────────────┐
 │ 计算层（都用同一份键值）                                      │
 │  aero: 大气/声速/极曲线/CY_MAX/可变后掠几何/Vne               │
 │  engine_model: 活塞·喷气·涡桨 → 功率/推力/油耗/增压级         │
 │  flight_model: 质量与重量、过载限值、油耗、起飞重量           │
 │  weapon_model: 武器与弹链                                     │
 │  em: Ps（单位剩余功率）/ 瞬时与持续盘旋率 / corner speed      │
 └───────────────┬───────────────────────────────┬───────────────┘
                 ▼                               ▼
      HUD（core 每帧查询）               CLI flightmodel（曲线/EM/阻力）
```

## 代码目录

| 路径 | 内容 |
|---|---|
| `src/lib.rs` | 门面：模块声明 + 统一导出 + `parse_aircraft()` |
| `src/main.rs` | CLI `flightmodel`（列机型 / 解析 / 各种曲线 / `--em` 的 EM 数据 / `--curves-json`） |
| `scripts/plot_flight_model.py` | **matplotlib 参考出图**（推力/功率/EM 能量机动图等）：控制台的 EM 图就是照它的 `plot_em` 画法实现的（X=表速、Y=盘旋率、Ps 色带 ±400、瞬时/持续包线、峰值标注、等过载线 n=2/3/4/6/8/12、VNE 竖线、ylim 0..33） |
| `src/parser.rs` | FM 文本解析入口：`parse_fm_text`（内容分派）、`BlkxValue`、取值辅助 |
| `src/json_adapter.rs` | JSON 版 blkx → 键值（`fm-json`） |
| `src/fm_paths.rs` | 数据根解析 + `.blk→.blkx`/大小写归一（HUD/GUI 共用） |
| `src/flight_model.rs` | `FlightModel`：质量、过载限值、油耗、与 HUD 的查询接口 |
| `src/aero.rs` | 气动：极曲线、大气与声速、Mach、可变后掠翼几何、`AeroForces` |
| `src/engine_model.rs` | 发动机：活塞/喷气/涡桨、功率与推力表、油耗、增压级 |
| `src/weapon_model.rs` | 武器与弹链（`Weapon`/`BeltInfo`/`BulletInfo`） |
| `src/em.rs` | EM 求解器：Ps、瞬时/持续盘旋率、corner speed |
| `src/update/` | FM 数据库更新器（P3）：版本/清单/下载/校验/切换 + mock 与测试 |
| `src/test_fm_parsing.rs` | 大量机型解析回归（含 legacy 数据） |
| `src/test_p2_parity.rs` | legacy ↔ JSON 键值等价回归（P2，两者都开时才跑） |
| `tests/json_release.rs` | JSON 中间对象必须及时释放（计数分配器） |
| `SPEC.md` | **详细 API 规格**（结构字段、函数签名、口径与出处） |
| `docs/{fm_explain,aerodyn,em}.md` | 参数详解 / 气动原理 / EM 推导 |

## 命令行用法

### 列出所有机型

```bash
# 列出指定目录下的所有 blkx 文件
./target/release/flightmodel --list --data-dir ./resource/data
```

### 解析飞机 FM 数据

```bash
# 解析指定飞机的飞行模型数据
./target/release/flightmodel --aircraft yak-3 --data-dir ./resource/data

# 解析喷气机
./target/release/flightmodel --aircraft su_30sm2 --data-dir ./resource/data --verbose
```

### 输出格式

```bash
# JSON 格式
./target/release/flightmodel --aircraft yak-3 --data-dir ./resource/data --format json

# TSV 格式
./target/release/flightmodel --aircraft yak-3 --data-dir ./resource/data --format tsv
```

### 生成图表数据

```bash
# 生成喷气机推力曲线
./target/release/flightmodel --aircraft su_30sm2 --data-dir ./resource/data --plot

# 生成活塞机功率曲线
./target/release/flightmodel --aircraft yak-3 --data-dir ./resource/data --plot
```

### 高级分析

```bash
# EM 曲线数据（只喷气机）：写 <机型>_em.txt（Ps 网格）+ _curves.txt（包线）+ _summary.txt
./target/release/flightmodel --aircraft su_30sm2 --data-dir ./resource/data --em
# 这三份 TSV 就是 flightmodel/scripts/plot_flight_model.py 的输入

# 控制台曲线 JSON（同一个 --curves-json 里带 em 段，喷气机才有）：
#   em.alts[i] = { alt, ias_per_tas, grid{v,w,ps_wep,ps_mil}, curve{v,n_av,n_sus,w_inst,w_sus_wep,w_sus_mil,ps_level}, sum{...} }
./target/release/flightmodel --aircraft su_30sm2 --data-dir ./resource/data --curves-json

# 阻力分解
./target/release/flightmodel --aircraft su_30sm2 --data-dir ./resource/data --drag

# 最大平飞速度
./target/release/flightmodel --aircraft su_30sm2 --data-dir ./resource/data --max-speed
```

## 数据说明

### 提取的飞行模型参数

| 参数 | 说明 |
|------|------|
| EmptyWeight | 空重 (kg) |
| MaxFuelMass0 | 最大燃油 (kg) |
| Vne | 最大允许表速 (km/h) |
| VneMach | 最大允许马赫数 |
| Wingspan | 翼展 (m) |
| WingArea | 机翼面积 (m²) |
| AspectRatio | 展弦比 |

### 喷气机参数

| 参数 | 说明 |
|------|------|
| ThrustMax | 最大推力 (kgf) |
| ThrustMaxCoeff | 推力系数矩阵 (高度×速度) |
| ThrAftMaxCoeff | 加力推力系数矩阵 |
| AfterburnerBoost | 加力推力倍数 |
| Mode{N}.ThrustMult | 发动机模式推力倍数 (用于WEP计算) |

**WEP推力计算**: WEP推力 = ThrustMax0 × AfterburnerBoost × 最后EngineMode的ThrustMult

### 活塞机参数

| 参数 | 说明 |
|------|------|
| Power | 额定功率 (hp) |
| CompressorStages | 增压器档数 |
| ThrottleBoost | 油门增加倍数 |

## 数据来源

- 主 BLKX 文件: `resource/data/gamedata/flightmodels/{aircraft}.blkx`
- FM 文件: `resource/data/gamedata/flightmodels/fm/{aircraft}.blkx`

> 数据根目录（`--data-dir` 的取值）就是 `resource/data`，即 `gamedata/` 的**上一层**。
> **不传 `--data-dir` 时**由 `wp8f_flightmodel::resolve_data_root` 解析（与 HUD、GUI 共用同一份）：
> `$WP8F_DATA_DIR` → 当前工作目录/可执行文件附近的 `resource/data` → 不存在则用更新器 A/B 分区的
> 暂存根 **`resource/data_new`**；都找不到时打印一条"找过哪些路径 + 该放什么"的错误并以退出码 2 结束
> （不再像以前那样把 `./resource/data` 写死在 clap 里、从别处启动就读不到）。
> 这些数据是**用户本地数据、不在 git 里**：上一版它们躺在 `resource/data/aces/` 下，
> 2026-10 起整体提上来一级（更早是 `resource/fm/data`、`flightmodel/data`）——
> 见仓库根 `README.md`「资源目录」的迁移命令。

## 两套数据源（feature 开关）

同一份上层代码能读两种格式，用 cargo feature 选（**不新建 crate**）：

```bash
cargo build -p wp8f-flightmodel                      # 默认：fm-json（GitHub JSON 数据源）
cargo build -p wp8f-flightmodel --features fm-legacy # 再叠加旧 blkx 解析器（回归对照用）
cargo build -p wp8f-flightmodel --no-default-features --features fm-legacy  # 只带 legacy（备份路径）
```

| feature | 数据 | 实现 |
|---|---|---|
| `fm-json`（**默认**） | GitHub `War-Thunder-Datamine` 的 JSON | `json_adapter::parse_json_fm` |
| `fm-legacy`（仅备份，要显式打开） | 旧游戏解包 `.blkx` 文本 | `parser::parse_legacy_blkx_string` |

**为什么默认是 JSON**：用户手上的 `resource/data` 自 **2.59.0.43** 起就是 JSON 版 blkx；
legacy 解析器按用户口径**保留但仅作备份**（不参与默认编译/发布）。

两个都打开时按**内容**分派（首个非空白字节 `{` → JSON，见 `parser::parse_fm_text`）——
`fmFile` 引用的 `.blk` → `.blkx` 与大小写归一也由 `fm_paths` 一处负责（Linux 上大小写敏感会 404）。

实测（tag `2.58.0.35`，20 机型 × 2 文件 / 86,535 个键）：两条路径**键集合完全一致**，
值 13 处已知差异（0.015%，全部来自"重复标量行 ↔ 一行 `pN`"这一 JSON 侧结构性歧义，
且这些键名语义层一个都不读）——清单冻结在 `src/test_p2_parity.rs`，规则推导见
`src/json_adapter.rs` 模块文档。
（`frozen_legacy_key_counts` 的键数冻结值**跟随数据版本**：现在是 2.59.0.43，
比 2.58.0.35 时多 4 个机型的漂移，更新 FM 数据后要重新采集。）

## 相关文档

- [Flight Model 参数详解](./docs/fm_explain.md)
- [气动原理](./docs/aerodyn.md) · [EM 推导](./docs/em.md) · [API 规格](./SPEC.md)

## 关键函数 Specification

### `parse_aircraft(aircraft_name: &str, data_dir: &str) -> Result<FlightModel, String>`
* 输入：机型名（大小写不敏感，内部过 `normalize_blkx_ref` 补扩展名）；`data_dir` =
  **机型目录**（`<数据根>/gamedata/flightmodels`）。
* 输出：算好的 `FlightModel`（含气动/发动机/质量/武器引用）。
* 前置：文件存在且能被当前 feature 的解析器读懂。
* 后置：`fmFile` 引用（写 `fm/a-20g.blk`）归一后读；没有 `fmFile` 时退到 `fm/<name>.blkx`。
* 错误：`Err(String)` —— 文件不存在 / 解析失败 / 关键字段缺失（带机型名与人话原因）。
* 副作用：读磁盘；不做缓存（调用方决定缓存策略）。

### 数据根与路径（`fm_paths`）
* `resolve_data_root(explicit: Option<&Path>) -> Result<PathBuf, String>`：
  `explicit` → `$WP8F_DATA_DIR` → 按 `data_root_candidates_in()`（`resource/data` 与
  `resource/data_new`）逐个试 → 全失败返回 `Err`（消息里列出找过的路径与该放什么）。
* `resolve_data_root_in(repo_root)` / `data_root_candidates_in(repo_root)` /
  `is_data_root(path)`：纯路径判断，供 GUI 与测试用（不读环境变量）。
* `flightmodels_dir` / `weapons_dir` / `version_file(data_root)`：三个固定子路径。
* `normalize_blkx_ref(&str) -> String` / `strip_blkx_extension` / `normalize_weapon_ref` /
  `weapon_ref_stem`：引用归一（补 `.blkx`、转小写、去扩展名）。
* 错误：只有 `resolve_data_root*` 返回 `Result`，其余是不失败纯函数。副作用：无（不读盘）。

### 解析（`parser`）
* `parse_fm_text(text: &str) -> Result<HashMap<String, BlkxValue>, String>`：
  **内容分派**（首个非空白字节 `{` → JSON）—— 这是两套数据源唯一的汇合点。
* `detect_fm_text_kind(text) -> FmTextKind`：只看内容判断格式（不解析）。
* `FM_JSON_ENABLED` / `FM_LEGACY_ENABLED`：编译期常量，供上层显示支持情况。
* `get_f64_first` / `get_f64_multi` / `get_string_first`：按键取值（多值键取第一个/最后一个），
  缺失返回 `None`（**不报错**，调用方给默认值）。
* 错误：`Err(String)`（语法损坏）；缺键不算错误。副作用：无。

### 计算层（`flight_model` / `aero` / `engine_model` / `em` / `weapon_model`）
| 接口 | 输入 → 输出 | 说明 |
|---|---|---|
| `FlightModel::allowed_load_factor(weight_kg) -> (f64, f64)` | 重量 → (负, 正) 过载限值 | `n = sign(F)·(2·|F|/(m·g) − 1)`，g = 9.81；重量 ≤ 0 或数据缺失返回 (0, 0) |
| `Aerodynamics::{get_cl_aoa_max, get_aoa_min, get_vne, get_ias_warning_line, get_mach_warning_line}(flaps, sweep)` | 襟翼比例 + 后掠档 → 限值 | 可变后掠按档插值；缺数据回默认 |
| `WingGeometry::vne() / vne_mach() / is_variable_sweep()` | — → 限制速度 | — |
| `engine_model::{thrusts_at, powers_at, fuel_rate_at, total_fuel_rates, has_any_wep, engine_count}` | 高度/速度/加力档 → 推力/功率/油耗 | 活塞机给功率、喷气给推力，上层按机型分叉 |
| `FlightModel::flight_weight(wep_fuel_mass)` | 加力耗油 → 总重 | 质量属性 `MassProperties` |
| `em::*` | 高度/速度/过载 → Ps（m/s）、盘旋率（deg/s）、corner speed | SI 内部、对外 deg/s |
| `weapon_model::{load_weapon, load_weapon_with_preset, load_weapons_from_fm}` | 武器名/预设 → `Weapon` | 找不到返回 `None` |

* 前置：`FlightModel` 已由 `parse_aircraft` 构造完（子结构都有默认值，字段缺失不 panic）。
* 后置：查询接口都是 `&self` 纯计算，可任意频次调用（HUD 每帧在调）。
* 错误：**查询类接口不返回 `Result`** —— 数据缺失一律回退到安全默认值（HUD 不能因为
  某机型少个键就崩）。
* 副作用：无（只有 `load_weapon*` 读磁盘）。

### 更新器（`update`）
* `UpdateManager` + `Status` + `Phase` + `UpdatePaths`：版本检查 → 逐文件下载 → 校验 →
  A/B 分区切换；`Phase` 描述当前阶段（供 GUI 进度条），`Status` 是状态快照。
* 前置：数据根可写（切换要落 `resource/data_new` 再改名）。
* 错误：网络/校验失败 → 保持当前数据不变（**绝不半途切换**）；错误进 `Status`。
* 副作用：下载文件、改目录名（切换）。

## 约束与取舍

* **发布产物不带 legacy**：`fm-legacy` 只是备份/回归对照；默认构建必须是 `fm-json`
  （用户手上的 `resource/data` 自 2.59.0.43 起就是 JSON 版 blkx）。
* **测试数据不在 crate 内**：统一放仓库根 `resource/data`（用户本地数据，不入 git）；
  外部克隆需自备，否则依赖数据的用例会失败。
* **两套解析器的键集合冻结在测试里**：`test_p2_parity.rs` 记录已知差异（重复标量行 ↔ 一行
  `pN` 的结构性歧义，语义层不读这些键）；更新 FM 数据后要重新采集冻结值。
* `data_dir` 的语义是**机型目录**而不是数据根（根由 `resolve_data_root` 解析）——
  传错会表现为「找不到机型」而不是崩溃。
* 版本口径：`resource/data/version` 是数据版本，GUI 的「检查更新」按它比对。

## 测试

* 单元测试：`test_fm_parsing.rs`（真实机型解析回归，含 legacy 数据、可变后掠、
  直升机/双翼/火箭等特例）、`test_p2_parity.rs`（legacy ↔ JSON 等价）、`em`/`aero`/
  `engine_model` 的口径测试、`fm_paths` 的路径归一。
* `tests/json_release.rs`：JSON 中间对象（文本 + `Value`）必须及时释放（计数分配器）。
* `scripts/test.sh` 带 `-p wp8f-flightmodel`（默认 feature）一起跑；`fm-legacy` 组合单独
  编译验证（不进默认发布）。
