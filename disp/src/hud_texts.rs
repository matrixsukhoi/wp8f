//! 渲染层用的**文本缓冲**：一帧的格式化结果全在这里，**不进环形缓冲**。
//!
//! 环（`DisplayData` / `MapDisplay`）只存数值；渲染线程每帧先 `hud_fmt::format_all`
//! 把数值格式化成这些定长文本，再交给绘制。这样：
//! * 环的槽位里没有字符串（体积小、跨线程只拷数值）；
//! * 格式化只在渲染线程做一次（不是每个 draw 调用一次）；
//! * 渲染器里不再出现 `format_args!`。
//!
//! 字段名与原来的 `DisplayData.*_text` 一一对应（方便逐点替换），`is_empty()` 表示"这一格不画"。

use super::FixedBytes;

/// 友军面板一行（已格式化好的数值列）。
#[derive(Debug, Clone, Default)]
pub struct LinkRowText {
    pub ias: FixedBytes<8>,
    pub alt: FixedBytes<8>,
    pub hdg: FixedBytes<8>,
    pub dist: FixedBytes<8>,
}

/// 友军面板最多画几行（与 `hud::link_panel::MAX_FRIENDLY_ROWS` 一致）。
pub const MAX_LINK_ROWS: usize = 8;

#[derive(Debug, Clone, Default)]
pub struct HudTexts {
    // —— 飞行信息面板（12 格）与友军面板共用的语义文本 ——
    pub energy_height_text: FixedBytes<16>,
    pub altitude_text: FixedBytes<16>,
    pub tas_text: FixedBytes<16>,
    pub ias_text: FixedBytes<16>,
    pub mach_text: FixedBytes<16>,
    pub vy_text: FixedBytes<16>,
    pub sep_text: FixedBytes<16>,
    pub turn_rate_text: FixedBytes<16>,
    pub turn_radius_text: FixedBytes<16>,
    /// 表速格的单位栏：`/1477`（VNE）；无 VNE 时为空
    pub vne_text: FixedBytes<16>,
    /// 马赫格的单位栏：`/1.05`（MNE）；无 MNE 时为空
    pub mne_text: FixedBytes<16>,
    pub roll_rate_text: FixedBytes<16>,
    pub pitch_text: FixedBytes<16>,
    pub aoa_format_text: FixedBytes<16>,
    pub g_load_text: FixedBytes<16>,
    /// 过载格的单位栏：`/15.4`（当前重量下的允许正过载）
    pub g_limit_text: FixedBytes<16>,
    // —— 燃油 / 发动机面板 ——
    pub fuel_kg_text: FixedBytes<16>,
    pub fuel_percent_text: FixedBytes<16>,
    pub fuel_time_text: FixedBytes<16>,
    pub fuel1_kg_text: FixedBytes<16>,
    pub fuel1_time_text: FixedBytes<16>,
    /// 燃油时间格的单位栏（SFC）
    pub fuel_sfc_text: FixedBytes<16>,
    pub fuel1_sfc_text: FixedBytes<16>,
    pub elapsed_time_text: FixedBytes<16>,
    pub engine_rpm_text: FixedBytes<16>,
    pub engine_rpm_unit: FixedBytes<16>,
    pub engine_temp_water_text: FixedBytes<16>,
    pub engine_temp_oil_text: FixedBytes<16>,
    /// 水/油温格的单位栏：临界温度或剩余时间（`℃/-`）
    pub water_crit_temp_text: FixedBytes<16>,
    pub oil_crit_temp_text: FixedBytes<16>,
    pub manifold_pressure_text: FixedBytes<16>,
    pub manifold_pressure_unit: FixedBytes<16>,
    pub rated_power_text: FixedBytes<16>,
    pub engine_power_text: FixedBytes<16>,
    pub power_unit_text: FixedBytes<16>,
    pub power_to_weight_text: FixedBytes<16>,
    pub engine_throttle_text: FixedBytes<16>,
    pub thrust_percent_text: FixedBytes<16>,
    pub thrust_to_weight_text: FixedBytes<16>,
    pub total_thrust_text: FixedBytes<16>,
    pub total_drag_text: FixedBytes<16>,
    /// 加力燃烧室剩余时间（当前无数据源，恒空 —— 版式保留）
    pub afterburner_fuel_time_text: FixedBytes<16>,

    // —— 圆环 HUD（按版式排好的文本）——
    pub circle_alt: FixedBytes<8>,
    pub circle_radio_alt: FixedBytes<8>,
    pub circle_ias: FixedBytes<8>,
    pub circle_mach: FixedBytes<8>,
    pub circle_aoa: FixedBytes<12>,
    pub circle_energy: FixedBytes<16>,
    pub circle_sep: FixedBytes<12>,
    pub circle_g: FixedBytes<12>,
    pub circle_fuel: FixedBytes<24>,
    pub circle_heading: FixedBytes<8>,

    // —— miniHUD（同样的量，版式不同）——
    pub mini_mach: FixedBytes<8>,
    pub mini_aoa: FixedBytes<8>,
    pub mini_radio_alt: FixedBytes<8>,
    pub mini_g: FixedBytes<8>,
    pub throttle: FixedBytes<8>,

    // —— 地图面板 ——
    /// 最近机场：`AF 17.5km / 172°`
    pub nearest_airfield_text: FixedBytes<64>,
    pub nearest_bombing_point_text: FixedBytes<64>,
    pub nearest_poi_text: FixedBytes<64>,
    /// 区域格（`map_zone_label` 的结果，如 `D3`）
    pub map_zone_text: FixedBytes<16>,
    /// 圆环 HUD 的 POI（箭头 + 距离）
    pub poi_text: FixedBytes<16>,
    /// miniHUD 的 POI（箭头规则不同）
    pub mini_poi_text: FixedBytes<16>,
    pub poi_speed_text: FixedBytes<8>,
    pub mini_poi_speed_text: FixedBytes<8>,

    // —— 状态项 / 地图信息行 / 友军面板 ——
    /// 状态项 `F20`（襟翼；0 时为空）
    pub flaps_text: FixedBytes<8>,
    /// 状态项 `W100`（后掠百分比；0 时为空）
    pub wing_sweep_text: FixedBytes<8>,
    /// 地图信息第一行：`Zone D3 0°`
    pub map_zone_line: FixedBytes<40>,
    /// 地图信息第三行：`POI 17.5km / 172° / 63m`（无邻近目标时就是最近目标那行）
    pub map_poi_line: FixedBytes<96>,
    /// 友军面板每行的数值列
    pub link_rows: [LinkRowText; MAX_LINK_ROWS],
}

impl HudTexts {
    /// 清空（每帧格式化前调一次；各 formatter 只写自己那部分）。
    pub fn clear_all(&mut self) {
        *self = Self::default();
    }
}
