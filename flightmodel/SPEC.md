# FlightModel Specification

## 项目概述

`flightmodel` 是 War Thunder BLKX 文件解析器，用于提取飞机飞行模型(FM)数据并进行性能分析。

### 项目职责

- 解析 BLKX/FM 游戏文件，提取飞机气动数据和发动机数据
- 支持多种飞机类型：活塞发动机、喷气发动机、涡桨发动机、火箭发动机
- 计算飞行性能指标：阻力分解、最大平飞速度、EM曲线、盘旋性能等
- 支持变后掠翼飞机的多后掠角数据插值

### 项目结构

```
flightmodel/
├── src/
│   ├── lib.rs              # 库入口，公共API导出
│   ├── main.rs             # 可执行文件入口
│   ├── flight_model.rs      # FlightModel 核心结构
│   ├── aero.rs             # 气动数据解析
│   ├── em.rs               # EM 求解器（Ps / 盘旋率 / corner speed）
│   ├── engine_model.rs      # 发动机数据解析
│   ├── weapon_model.rs      # 武器数据解析
│   ├── parser.rs            # FM 文本解析（统一入口 + legacy blkx 实现）
│   ├── json_adapter.rs      # JSON 适配器（`fm-json` feature）
│   ├── fm_paths.rs          # 数据根解析 + `.blk→.blkx`/大小写归一（HUD/GUI 共用）
│   ├── update/             # FM 数据库更新器（版本/清单/下载/校验/A-B 切换）
│   ├── test_fm_parsing.rs   # 测试用例
│   └── test_p2_parity.rs    # legacy ↔ JSON 键值等价回归（P2）
├── tests/json_release.rs    # JSON 中间对象及时释放（计数分配器）
├── docs/                   # 详细文档
├── Cargo.toml
└── SPEC.md                 # 本文档
```

> 测试数据**不在本 crate 内**：统一放在仓库根的 `resource/data/`（2026-10 把原来的
> `resource/data/aces/` 整体提上来一级；更早是 `resource/fm/data/`、`flightmodel/data/`）。
> 它是用户本地数据（datamine 产物）、**不在 git 里**，外部克隆者需自备，否则依赖它的用例会失败。

---

## 架构层次结构

```
┌─────────────────────────────────────────────────────────────────┐
│                         API Layer                                │
│  parse_aircraft(name, data_dir) → Result<FlightModel, String>  │
└─────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────┐
│                    FlightModel.parse()                           │
│  ┌─────────────┐  ┌──────────────┐  ┌─────────────────────────┐ │
│  │ Aerodynamics │  │  Propulsion  │  │    MassProperties      │ │
│  │   .parse()   │  │    .parse()  │  │      .parse()           │ │
│  └─────────────┘  └──────────────┘  └─────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
        │                   │                      │
        ▼                   ▼                      ▼
┌───────────────┐   ┌──────────────┐    ┌─────────────────┐
│ VariableSweep │   │   Engine[]   │    │ LoadoutWeight   │
│ WingGeometry  │   │  read_all_   │    │                 │
│ .read_from()  │   │   engines()  │    └─────────────────┘
└───────────────┘   └──────────────┘
        │
        ▼
┌───────────────────────────────────────────────────────┐
│              FM 文本层（两套数据源，一个出口）           │
│  parse_fm_text(content) → HashMap<String,BlkxValue>    │
│    ├─ fm-json（默认）：json_adapter::parse_json_fm()    │
│    └─ fm-legacy（备份，要显式打开）：                     │
│         parse_legacy_blkx_string()                     │
└───────────────────────────────────────────────────────┘
```

> feature 选择见 `Cargo.toml`（`fm-json` 默认 / `fm-legacy` 仅备份）；两个都开时按**内容**分派
> （首个非空白字节 `{` → JSON）。详见 §「两套数据源」。

---

## FM 数据解析

### 数据文件位置

| 文件类型 | 路径 |
|---------|------|
| 主BLKX文件 | `resource/data/gamedata/flightmodels/{aircraft}.blkx` |
| FM文件 | `resource/data/gamedata/flightmodels/fm/{aircraft}.blkx` |
| 武器数据 | `resource/data/gamedata/weapons/{weapon}.blk` |

### BLKX 文件解析

**统一入口**: `parse_fm_text(content: &str) -> Result<HashMap<String, BlkxValue>, String>`

- 支持嵌套块结构 `{ }`
- 支持类型后缀如 `:r`, `:i`, `:t`, `:b`
- 支持向量值 (空格或逗号分隔)
- **按内容分派**两套实现（`fm-json` 默认 / `fm-legacy` 仅备份），见下节

### 两套数据源（feature 开关，D12/D13）

| feature | 数据来源 | 实现 | 产物 |
|---|---|---|---|
| `fm-json`（**默认**） | GitHub `War-Thunder-Datamine` JSON | `json_adapter::parse_json_fm` | `HashMap<String, BlkxValue>` |
| `fm-legacy`（仅备份，要显式打开） | 旧游戏解包 `.blkx` 文本 | `parse_legacy_blkx_string` | **同一套** `HashMap<String, BlkxValue>` |

> 默认改成 `fm-json` 的原因：用户手上的 `resource/data` 自 **2.59.0.43** 起就是 JSON 版 blkx；
> legacy 按用户口径保留、但**仅作备份**（不参与默认编译，也不进发布产物）。

- 语义层（`flight_model.rs` / `aero.rs` / `engine_model.rs` / `weapon_model.rs`）**一行不用改**：
  实测两条路径扁平化后键集合 100% 相同，值按适配器的归一规则对齐。
- 实测对照（tag `2.58.0.35`，20 机型 × 2 文件 / 86,535 个键）：键集合全等，
  值 13 处已知差异（占 0.015%，全部是"重复标量行 ↔ 一行 pN"结构性歧义，语义层不读这些键），
  冻结在 `src/test_p2_parity.rs` 的 `FROZEN_DIVERGENCES`。
- 键数冻结清单（`FROZEN_KEY_COUNTS`）**跟随数据版本**：当前冻结在 **2.59.0.43**
  （相对 2.58.0.35 有 4 个机型漂移：f_15a/f_15e/fa_18a +9 键、harrier_gr7 +7 键），
  更新 FM 数据后要用用例打印的 `[冻结候选]` 重新采集。
- 归一规则表与歧义说明见 `src/json_adapter.rs` 模块文档。

### 数据根解析（HUD 与 GUI 共用）

`wp8f_flightmodel::fm_paths`：

| 函数 | 作用 |
|---|---|
| `resolve_data_root(explicit)` | `--data-dir` > `$WP8F_DATA_DIR` > cwd/可执行文件附近的 `resource/data`（缺失则 `resource/data_new`）；找不到给可读错误，不 panic |
| `resolve_data_root_in(repo_root)` | 在给定仓库根下解析（`resource/data` 优先、`resource/data_new` 兜底） |
| `flightmodels_dir` / `weapons_dir` / `version_file` | 数据根下的固定布局，避免各处手拼 |
| `normalize_blkx_ref` | `.blk` → `.blkx` + 整条路径小写（`fm/a-20g.blk` → `fm/a-20g.blkx`） |
| `normalize_weapon_ref` / `weapon_ref_stem` | 再去掉 `gameData/Weapons/`、`gameData/FlightModels/weaponPresets/` 前缀 |

---

## 接口语义

### parse_aircraft

**类型签名**:
```
parse_aircraft : String × String → Result<FlightModel, String>
```

**语义**:
- `parse_aircraft(name, data_dir)` 加载指定飞机的完整 FM 数据
- 首先读取主 BLKX 文件，再加载 FM 文件（如有）
- 解析所有子模块并返回完整的 `FlightModel` 结构

**前置条件**:
- `name` 非空字符串，表示飞机名称
- `data_dir` 指向有效的目录路径
- 目录下存在 `{name}.blkx` 文件

**后置条件**:
- 成功时返回 `Ok(fm)`，其中 `fm.name = name`
- 失败时返回 `Err(msg)`，包含错误描述

**副作用**:
- 可能读取多个文件（主BLKX + FM文件 + 武器数据）

---

### get_f64 / get_f64_first / get_f64_multi

**类型签名**:
```
get_f64      : HashMap<String, BlkxValue> × String → Option<f64>
get_f64_first : HashMap<String, BlkxValue> × [&str] → Option<f64>
get_f64_multi : [HashMap<String, BlkxValue>] × [&str] → Option<f64>
```

**语义**:
- `get_f64(data, key)` 返回 `data[key]` 的 f64 值（如果存在）
- `get_f64_first(data, keys)` 返回第一个匹配键的值
- `get_f64_multi(maps, keys)` 在多个数据源中按顺序查找第一个匹配

**前置条件**:
- `data` / `maps` 为有效的 BLKX 数据哈希表
- `keys` 为非空字符串数组

**后置条件**:
- 返回 `Some(v)` 当且仅当键存在于数据中且可转换为 f64
- 返回 `None` 当键不存在或类型不匹配

**回退键路径约定**:
```
VNE:      ["VNe", "Strength.VNE"]
MNE:      ["VNeMach", "Strength.MNE"]
EmptyMass: ["EmptyMass", "Mass.EmptyMass"]
CritOverload: ["Aerodynamics.WingPlane.Strength.CritOverload",
               "WingPlane.Strength.CritOverload",
               "Strength.Sweep0.CritOverload",
               "Strength.CritOverload"]
```

---

### crit_overload_vec

**类型签名**:
```
crit_overload_vec : FlightModel → Vec<f64>
```

**语义**:
- 返回临界过载向量 `[negative_g, positive_g]`
- 按 `CRIT_OVERLOAD_KEYS` 优先级查找数据
- 数据格式为两个 f64 值的向量（负过载，正过载）

**前置条件**:
- `FlightModel` 已正确初始化

**后置条件**:
- 返回长度为 2 的向量：`[neg_overload, pos_overload]`
- 若未找到数据，返回空向量

---

### warning_mach / warning_ias

**类型签名**:
```
warning_mach : FlightModel → f64
warning_ias  : FlightModel → f64
```

**语义**:
- `warning_mach()` 返回马赫警告线 = VNE_Mach × 0.95
- `warning_ias()` 返回 IAS 警告线 = VNE × 0.95

**前置条件**:
- VNE_Mach / VNE 已从 FM 数据正确读取

**后置条件**:
- 返回值 = 极限值 × 0.95
- 若极限值为 0 或无效，返回默认值（mach: 0.95, ias: 1000 km/h）

---

### vne_mach / vne

**类型签名**:
```
vne     : FlightModel → f64      // km/h
vne_mach : FlightModel → f64      // Mach number
```

**语义**:
- `vne()` 返回最大允许表速（km/h）
- `vne_mach()` 返回最大允许马赫数

**查找优先级**:
```
VNE: ["VNe", "Strength.VNE"] → 默认 1500.0 km/h
MNE: ["VNeMach", "Strength.MNE"] → 默认 1.0 Mach
```

---

## 数据流图

```
                        输入文件
                          │
                          ▼
┌───────────────────────────────────────────────────────────────┐
│  aircraft.blkx  ──→  parse_blkx_string()  ──→  data           │
│                              │                                │
│                              ▼                                │
│                    检查 fmFile 字段                            │
│                              │                                │
│              ┌───────────────┴───────────────┐                │
│              │                               │                  │
│         有 fmFile                        无 fmFile              │
│              │                               │                  │
│              ▼                               ▼                  │
│    fm/{name}.blkx                   自动查找 fm/                │
│         │                           /{name}.blkx               │
│         │                               │                       │
│         └───────────┬───────────────────┘                       │
│                     ▼                                           │
│          parse_blkx_string() → fm_data                         │
│                     │                                           │
│                     ▼                                           │
│            FlightModel::parse()                                 │
│                     │                                           │
│         ┌───────────┼───────────┬───────────────┐              │
│         ▼           ▼           ▼               ▼              │
│   Aerodynamics  Propulsion  MassProperties  LoadoutWeight     │
│      .parse()     .parse()     .parse()         .parse()        │
│         │           │           │               │              │
│         └───────────┴───────────┴───────────────┘              │
│                         │                                       │
│                         ▼                                       │
│                    FlightModel                                  │
└───────────────────────────────────────────────────────────────┘
```

---

## 状态转换图

### FlightModel 生命周期

```
                    ┌─────────────────┐
                    │   Uninitialized  │
                    └────────┬────────┘
                             │ parse_aircraft()
                             ▼
                    ┌─────────────────┐
            ┌───────│   Parsing       │───────┐
            │       └─────────────────┘       │
            │成功                           │失败
            ▼                               ▼
    ┌───────────────┐               ┌───────────────┐
    │    Ready      │               │    Error      │
    └───────┬───────┘               └───────────────┘
            │
            ▼
    可调用所有查询方法:
    - vne() / vne_mach()
    - warning_ias() / warning_mach()
    - crit_overload_vec()
    - drag_breakdown()
    - max_level_flight_speed()
    - etc.
```

---

## 主要模块/结构

### Aerodynamics 结构

```rust
pub struct Aerodynamics {
    pub wingspan: f64,              // 翼展 (m)
    pub wing_area: f64,             // 机翼面积 (m²)
    pub aspect_ratio: f64,          // 展弦比
    pub cd_min: f64,                // 最小阻力系数
    pub cl_max_no_flaps: f64,       // 无襟翼最大升力系数
    pub cl_max_full_flaps: f64,     // 全襟翼最大升力系数
    pub aoa_cl_max_no_flaps: f64,   // 无襟翼临界攻角 (°)
    pub aoa_cl_max_full_flaps: f64, // 全襟翼临界攻角 (°)
    pub oswalds_efficiency: f64,    // 奥斯瓦尔德效率因子
    pub radiator_cd: f64,           // 散热器阻力系数
    pub oil_radiator_cd: f64,       // 油冷器阻力系数
    pub airbrake_cd: f64,           // 减速板阻力系数
    pub elevator_effective_speed: f64,  // 升降舵有效速度 (km/h)
    pub aileron_effective_speed: f64,  // 副翼有效速度 (km/h)
    pub rudder_effective_speed: f64,    // 方向舵有效速度 (km/h)
    pub wing_geometry: VariableSweepWing, // 变后掠翼几何数据
}
```

### WingGeometry 结构

```rust
pub struct WingGeometry {
    pub sweep_percent: f64,         // 后掠百分比 (0.0, 0.5, 1.0)
    pub span: f64,                  // 翼展 (m)
    pub wing_area: f64,             // 机翼面积 (m²)
    pub taper_ratio: f64,           // 梢根比
    pub swept_angle: f64,           // 后掠角 (°)
    pub no_flaps_polar: PolarData,  // 无襟翼极曲线数据
    pub full_flaps_polar: Option<PolarData>, // 全襟翼极曲线
    pub high_flaps_polar: Option<PolarData>, // 高襟翼极曲线
    pub vne: f64,                   // 最大允许表速 (km/h)
    pub vne_mach: f64,              // 最大允许马赫数
    pub is_vwing: bool,             // 是否V翼
}
```

### VariableSweepWing 结构

```rust
pub struct VariableSweepWing {
    pub wing_sweep0: WingGeometry,  // 0% 后掠配置
    pub wing_sweep1: Option<WingGeometry>, // 50% 后掠配置
    pub wing_sweep2: Option<WingGeometry>, // 100% 后掠配置
}
```

**关键方法**:
- `is_variable_sweep() -> bool` - 是否为变后掠翼
- `get_vne(wing_sweep: f64) -> f64` - 插值获取指定后掠角的VNE
- `get_cl_aoa_max(flaps: f64, wing_sweep: f64) -> (f64, f64)` - 获取最大升力系数和攻角

### PolarData 结构

```rust
pub struct PolarData {
    pub oswalds_efficiency: f64,    // 奥斯瓦尔德效率
    pub line_cl_coeff: f64,         // 升力线斜率系数
    pub cl0: f64,                   // 零升力系数
    pub alpha_crit_high: f64,        // 正临界攻角
    pub alpha_crit_low: f64,         // 负临界攻角
    pub cl_crit_high: f64,           // 正临界升力系数
    pub cl_crit_low: f64,            // 负临界升力系数
    pub cd_min: f64,                 // 最小阻力系数
    pub after_crit_parab_angle: f64, // 失速后攻角
    pub after_crit_decline_coeff: f64, // 失速后升力衰减系数
    pub cl_after_crit_high: f64,     // 失速后最大升力系数
    pub cl_after_crit_low: f64,      // 失速后最小升力系数
    pub cx_after_coeff: f64,          // 失速后阻力系数
    pub mach_corrections: Vec<MachCorrection>, // 马赫修正组
}
```

### FlightModel 结构

```rust
pub struct FlightModel {
    pub name: String,                       // 飞机名称
    pub data: HashMap<String, BlkxValue>,    // 主BLKX数据
    pub fm_data: HashMap<String, BlkxValue>, // FM数据
    pub engine_type: EngineType,             // 发动机类型
    pub is_jet: bool,                        // 是否喷气机
    pub wing: Option<AeroSurface>,            // 机翼气动面
    pub fuselage: Option<AeroSurface>,        // 机身气动面
    pub hor_stab: Option<AeroSurface>,        // 水平尾翼
    pub reference_area: f64,                  // 参考面积
    pub aerodynamics: Aerodynamics,           // 气动数据
    pub propulsion: Propulsion,               // 推进系统
    pub mass: MassProperties,                // 质量属性
    pub loadout: LoadoutWeight,              // 武器载荷
}
```

---

## 关键函数

### get_f64 模式

#### `get_f64_first`

```rust
pub fn get_f64_first<'a>(
    data: &'a HashMap<String, BlkxValue>,
    keys: &[&str],
) -> Option<f64>
```

尝试多个键路径，返回第一个找到的值：

```rust
// 示例：查找VNE
let vne = get_f64_first(fm_data, &["VNe", "Strength.VNE", "Aerodynamics.VNe"]).unwrap_or(1500.0);
```

#### `get_f64_multi`

```rust
pub fn get_f64_multi<'a>(
    maps: &[&'a HashMap<String, BlkxValue>],
    keys: &[&str],
) -> Option<f64>
```

跨多个数据源查找，fm_data优先于data：

```rust
// 示例：查找空重
let weight = get_f64_multi(&[fm_data, data], &["EmptyMass", "Mass.EmptyMass"]).unwrap_or(0.0);
```

### crit_overload / crit_overload_vec

```rust
const CRIT_OVERLOAD_KEYS: [&str; 7] = [
    "Aerodynamics.WingPlane.Strength.CritOverload",
    "WingPlane.Strength.CritOverload",
    "Aerodynamics.WingPlaneSweep0.Strength.CritOverload",
    "WingPlaneSweep0.Strength.CritOverload",
    "Strength.CritOverload",
    "Mass.WingCritOverload",
    "WingCritOverload",
];

pub fn crit_overload(&self) -> f64
pub fn crit_overload_vec(&self) -> Vec<f64>
```

临界过载数据查找，按优先级尝试多个路径。返回 `(负, 正)` 向量。
**单位：牛顿**（机翼结构极限力，不是 G 数）。

**变后掠机型**（F-14/F-111/MiG-23 系/Su-17 系/Tornado 等 33 个）：数据在
`Aerodynamics.WingPlaneSweep{0..3}.Strength.CritOverload`，且**没有**顶层
`WingCritOverload` 兜底；`Sweep0` 键在前 = 取最小后掠档。
（历史键 `Strength.Sweep0.CritOverload` 全库 0 命中，已移除。）

### warning_mach / warning_ias

```rust
pub fn warning_mach(&self) -> f64  // 马赫警告线 = MNE * 0.95
pub fn warning_ias(&self) -> f64   // IAS警告线 = VNE * 0.95
```

返回飞行警告线值（95% of 极限值）。

### VNE/MNE 读取逻辑

**VNE 查找优先级**:
```rust
wing.vne = [
    "VNe",
    "Strength.VNE",
].find_map(...).unwrap_or(1500.0)

wing.vne_mach = [
    "VNeMach",
    "Strength.MNE",
].find_map(...).unwrap_or(1.0)
```

**最近更新**:
- `find_map` 模式重构：正确查找 `Strength.VNE` 而非默认 1500
- `Strength.MNE` 回退到 `VNeMach`
- F-15E: VNE=1629 km/h, MNE=2.55
- F-16C: VNE=1555 km/h, MNE=2.2

### 其他关键方法

```rust
// 限制过载计算（CritOverload 单位为牛顿，换算口径经逆向确定：
// n = sign(F) × (2·|F| / (m·g) − 1)，g=9.81）
pub fn limit_load_factor_range(weight_kg: f64) -> (f64, f64)  // (负过载, 正过载)

// 襟翼限制
pub fn flap_allow_speed(&self, flap_percent: f64, is_downing_flap: bool) -> f64
pub fn flap_allow_angle(&self, ias: f64, is_downing_flap: bool) -> f64

// 阻力计算
pub fn drag(&self, altitude_m: f64, velocity_ms: f64, lift_coeff: f64) -> f64
pub fn drag_level_flight(&self, altitude_m: f64, velocity_ms: f64, weight_n: f64) -> f64
pub fn drag_breakdown(...) -> DragBreakdown

// 飞行性能
pub fn max_level_flight_speed(&self, altitude_m: f64, weight_n: f64, use_wep: bool) -> f64
pub fn specific_excess_power(...) -> f64
pub fn turn_rate(...) -> f64
pub fn turn_radius(...) -> f64
```

---

## 阻力计算过程流程图

```
┌─────────────────────────────────────────────────────────────────┐
│                 drag_level_flight()                              │
│                 (altitude, velocity_ms, weight_n)                 │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
                    ┌─────────────────┐
                    │ 计算动压 q       │
                    │ q = 0.5ρV²      │
                    └────────┬────────┘
                             │
                             ▼
                    ┌─────────────────┐
                    │ 计算所需升力系数   │
                    │ Cl = W / (q×S)  │
                    └────────┬────────┘
                             │
                             ▼
            ┌───────────────────────────────────┐
            │ 计算寄生阻力 (CdS / S)             │
            │ + 诱导阻力 (k × Cl²)               │
            │ + 散热器阻力                         │
            │ + 油冷器阻力                         │
            │ + 波阻 (M > 0.85 时)                │
            └───────────────┬───────────────────┘
                            │
                            ▼
                    ┌─────────────────┐
                    │ 总阻力 D         │
                    │ D = q × S × Cd  │
                    └────────┬────────┘
                             │
                             ▼
                    ┌─────────────────┐
                    │     返回 D       │
                    └─────────────────┘
```

---

## 发动机模型

### EngineType 枚举

```rust
pub enum EngineType {
    Piston,    // 活塞发动机
    Jet,       // 喷气发动机
    Turboprop, // 涡桨发动机
    Rocket,    // 火箭发动机
    Unknown,   // 未知
}
```

### Engine 结构

```rust
pub struct Engine {
    pub engine_type: EngineType,
    pub thrust_max: f64,           // 最大推力 (kgf) - 喷气机
    pub power: f64,                // 功率 (hp) - 活塞机
    pub afterburner_boost: f64,    // 加力倍数
    pub throttle_boost: f64,       // 油门倍数
    pub nitro_consumption: f64,    // 硝基消耗 (L/s)
    pub consumption_omega_max: f64, // 燃油消耗系数
    pub compressor_stages: Vec<CompressorStage>,
    pub altitudes: Vec<f64>,        // 高度节点
    pub velocities: Vec<f64>,       // 速度节点
    pub coefficients: Vec<Vec<Vec<f64>>>, // [高度][速度][军推/加力]
    pub fuel_consumption_idle: f64,
    pub fuel_consumption_half: f64,
    pub fuel_consumption_full: f64,
    pub fuel_consumption_wep: f64,
    pub engine_modes: Vec<EngineMode>,
}
```

---

## 规范约定

### 数据结构 Invariants

**FlightModel**:
- `name` 非空字符串
- `engine_type` ∈ {Piston, Jet, Turboprop, Rocket, Unknown}
- `vne` > 0 且 ≤ 6000 km/h
- `vne_mach` > 0 且 ≤ 10.0 Mach

**WingGeometry**:
- `span` > 0 (否则使用默认值 10.0)
- `wing_area` > 0 (否则使用默认值 20.0)
- `vne` ≥ 0，默认为 1500.0
- `vne_mach` ≥ 0，默认为 1.0

**PolarData**:
- `cd_min` ≥ 0
- `oswalds_efficiency` ∈ (0, 1]
- `cl_crit_high` > 0
- `alpha_crit_high` > 0

### 函数前置条件/后置条件

| 函数 | 前置条件 | 后置条件 |
|------|---------|---------|
| `parse_aircraft` | `data_dir` 存在且可读 | 返回有效 FlightModel 或 Err |
| `get_f64` | `data` 非空 | 返回 Option<f64> |
| `crit_overload_vec` | FlightModel 已初始化 | 返回 `[neg, pos]` 或 `[]` |
| `warning_mach` | `vne_mach > 0` | 返回 `vne_mach * 0.95` |
| `drag_breakdown` | `velocity_ms > 0`, `altitude_m ≥ 0` | 返回 DragBreakdown |

### 错误处理约定

```rust
// 文件读取错误
Err("Failed to read {path}: {io_error}")

// BLKX 解析错误（不抛出，内部处理）
// 缺失字段使用默认值

// 数值转换错误
None (通过 Option<f64> 传播)
```

---

## 测试说明

### 测试文件位置

- 主测试文件: `src/test_fm_parsing.rs`（语义层回归，需要本地数据）
- 两套解析等价: `src/test_p2_parity.rs`（真实 GitHub 样本对照用
  `WP8F_FM_JSON_SAMPLE_DIR=<数据根>` 打开，不设则打印跳过原因）
- JSON 内存释放: `tests/json_release.rs`（计数分配器，`--features fm-json`）
- 测试数据目录: `../resource/data/gamedata/flightmodels/`（相对 crate 根；cargo 测试的 cwd = crate 根）

### 测试用例列表

| 测试函数 | 描述 |
|---------|------|
| `test_parse_multiple_aircraft` | 解析多个飞机验证基本数据 |
| `test_parsed_values_are_reasonable` | 验证解析值的合理性 |
| `test_piston_aircraft_parsing` | 活塞发动机飞机解析测试 |
| `test_rocket_engine_aircraft_parsing` | 火箭发动机飞机 (me-163b) |
| `test_variable_sweep_wing_aircraft_parsing` | 变后掠翼飞机测试 |
| `test_old_biplane_parsing` | 老式双翼机测试 |
| `test_helicopter_parsing` | 直升机测试 |
| `test_multi_engine_jet_aircraft_parsing` | 多发动机喷气机 (B-52) |
| `test_turboprop_bomber_parsing` | 涡桨轰炸机 (Tu-95) |
| `test_carrier_fighter_parsing` | 舰载战斗机 (F-4) |
| `test_vtol_stol_aircraft_parsing` | 垂直/短距起降飞机 |
| `test_f15e_mach_warning_line` | F-15E 马赫警告线验证 |
| `test_all_find_map_fields_correctly_parsed` | find_map 字段解析验证 |

### 运行测试

```bash
# 运行所有测试
cargo test

# 运行特定测试
cargo test test_f15e_mach_warning_line

# 运行测试并显示输出
cargo test -- --nocapture
```

### 验证的测试数据

| 飞机 | 类型 | VNE | 翼展 | 空重 |
|------|------|-----|------|------|
| f_15e | Jet | 1629 km/h | 13.0m+ | 15000kg+ |
| f_16c_block_50 | Jet | 1555 km/h | 10m+ | 8000kg+ |
| mig_23mld | Jet (变后掠) | 700+ km/h | - | 9000kg+ |
| yak-3 | Piston | 300+ km/h | 8-12m | 2000kg+ |
| fa_18a | Jet | - | - | 10000kg+ |
| fw_190f_8_hungary | Unknown | - | 8-15m | 3000kg+ |
| wyvern_s4 | Turboprop | - | - | 4000kg+ |

---

## 最近更新

### find_map 模式重构

**问题**: 之前 VNE/MNE 等字段使用默认值的硬编码，无法正确从 `Strength.VNE` 读取数据。

**修复**:
- `WingGeometry::read_from` 中使用 `find_map` 模式正确查找字段
- F-15E VNE 从默认 1500 正确读取为 1629 km/h
- F-16C VNE 从默认 1500 正确读取为 1555 km/h

### Strength.VNE / Strength.MNE Fallback

VNE 读取现在支持以下路径回退：
```rust
// VNE
["VNe", "Strength.VNE"]

// MNE  
["VNeMach", "Strength.MNE"]
```

### CritOverload 的 Strength.Sweep0.CritOverload Fallback

临界过载查找现在支持：
```rust
const CRIT_OVERLOAD_KEYS: [&str; 4] = [
    "Aerodynamics.WingPlane.Strength.CritOverload",
    "WingPlane.Strength.CritOverload",
    "Strength.Sweep0.CritOverload",  // 新增回退
    "Strength.CritOverload",
];
```

### GearDestructionIndSpeed 回退

起落架破坏速度支持：
```rust
["Mass.GearDestructionIndSpeed", "GearDestructionIndSpeed"]
```

### FuselagePlane.Polar.CdMin 回退

机身阻力系数支持：
```rust
[
    "FuselagePlane.Polar.CdMin",
    "FuselagePlane.CdMin",
    "Aerodynamics.FuselagePlane.Polar.CdMin",
    "Aerodynamics.Fuselage.CdMin",
]
```

---

## API 总结

### 库入口 (lib.rs)

```rust
// 解析函数
pub fn parse_aircraft(aircraft_name: &str, data_dir: &str) -> Result<FlightModel, String>
pub fn parse_blkx(blkx_path: &str) -> Result<FlightModel, String>

// 导出类型
pub use aero::{PolarData, VariableSweepWing, WingGeometry, ...}
pub use engine_model::{Engine, EngineType, CompressorStage, ...}
pub use flight_model::{Aerodynamics, FlightModel, DragBreakdown, ...}
pub use parser::{parse_fm_text, BlkxValue, get_f64_first, get_f64_multi}
#[cfg(feature = "fm-legacy")] pub use parser::parse_legacy_blkx_string;
#[cfg(feature = "fm-json")]  pub use json_adapter::parse_json_fm;
pub use fm_paths::{resolve_data_root, resolve_data_root_in, flightmodels_dir,
                   normalize_blkx_ref, normalize_weapon_ref};
```

### FlightModel 关键方法

| 方法 | 返回值 | 说明 |
|------|--------|------|
| `vne()` | f64 | 最大允许表速 (km/h) |
| `vne_mach()` | f64 | 最大允许马赫数 |
| `wingspan()` | f64 | 翼展 (m) |
| `wing_area()` | f64 | 机翼面积 (m²) |
| `empty_weight()` | f64 | 空重 (kg) |
| `max_fuel()` | f64 | 最大燃油 (kg) |
| `engine_count()` | usize | 发动机数量 |
| `engine_type_string()` | String | 发动机类型描述 |
| `crit_overload()` | f64 | 临界过载 |
| `crit_overload_vec()` | Vec<f64> | 临界过载向量 [负, 正] |
| `limit_load_factor()` | f64 | 限制过载 |
| `drag_breakdown(...)` | DragBreakdown | 阻力分解 |
| `max_level_flight_speed(...)` | f64 | 最大平飞速度 |

---

## 依赖

```toml
[package]
name = "wp8f-flightmodel"
version = "0.1.0"
edition = "2021"

[dependencies]
clap = { version = "4.0", features = ["derive"] }
thiserror = "1.0"
serde = { version = "1.0", features = ["derive"] }

[lib]
name = "wp8f_flightmodel"
path = "src/lib.rs"

[[bin]]
name = "flightmodel"
path = "src/main.rs"
```

---

## BlkxValue 类型系统

```
┌─────────────────────────────────────────────────────────────┐
│                     BlkxValue                                │
├─────────────────────────────────────────────────────────────┤
│  vtype: BlkxValueType                                       │
│  value: String                                              │
├─────────────────────────────────────────────────────────────┤
│  as_f64()      → Option<f64>    // 解析为浮点数              │
│  as_f64_vec()  → Vec<f64>       // 解析为浮点数向量          │
│  as_i64()      → Option<i64>    // 解析为整数                │
│  as_bool()     → bool           // 解析为布尔值              │
│  as_string()   → String         // 返回原始字符串            │
└─────────────────────────────────────────────────────────────┘

BlkxValueType:
  Bool | Text | Int | Real | Real2 | Real3 | Real4 | Real5 | Real6 | Unknown
```

---

## 形式化接口规范汇总

| 接口 | 类型签名 | 前置条件 | 后置条件 |
|------|----------|---------|---------|
| `parse_aircraft` | `String × String → Result<FlightModel, String>` | 目录存在 | 成功返回有效 FM |
| `parse_fm_text` | `String → Result<HashMap<String, BlkxValue>, String>` | 输入非空 | 按内容分派（`{` → JSON，否则 legacy）；实现缺失时给可读错误 |
| `get_f64` | `Map × String → Option<f64>` | Map 有效 | 找到返回 Some |
| `get_f64_first` | `Map × [&str] → Option<f64>` | keys 非空 | 返回首个匹配 |
| `get_f64_multi` | `[Map] × [&str] → Option<f64>` | maps 非空 | 跨源查找 |
| `crit_overload_vec` | `FlightModel → Vec<f64>` | FM 已初始化 | 返回 [neg, pos] |
| `warning_mach` | `FlightModel → f64` | vne_mach > 0 | 返回 0.95 × MNE |
| `warning_ias` | `FlightModel → f64` | vne > 0 | 返回 0.95 × VNE |
| `vne` | `FlightModel → f64` | - | 返回 km/h |
| `vne_mach` | `FlightModel → f64` | - | 返回 Mach |