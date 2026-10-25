//! 环里流动的状态结构：`DisplayData`（每数据帧一帧）/ `MapDisplay`（每地图采样一份，含
//! `MapObjectDisplay`）/ `MapImage`（一局一份的底图）。
//!
//! 从 `lib.rs` 拆出的一块；对外路径不变（`lib.rs` 有 `pub use state::{...}`）。
//! 这些结构是**环里的 POD**：不持有堆上的可变状态、不含句柄，读端拿到的永远是完整快照；
//! 唯一的例外是 `MapImage` 的两个 `Arc<Vec<u8>>`（换图时读者手上的旧图仍然有效）。
//!
//! 这里只放数据与底图的发布/读取；谁填这些字段（core 的 updater / core 的地图解析）不在这里。

use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};

use super::fixed_bytes::FixedBytes;

/// 告警等级（**POD 的 1 字节**，与 `.wpr` CSV 列里的数字一一对应）。
///
/// 三个来源（超速 / 马赫 / 过载 / SEP …）各自算出等级后，**"取最严重"就是 `Ord::max`**：
/// 不再靠 `u8` 的 0/1/2 隐式约定去 `.max(1)`。
///
/// 数值口径（CSV 与旧数据的契约，别改顺序）：`0` 正常 / `1` 黄 / `2` 红。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
#[serde(from = "u8", into = "u8")]
pub enum WarningStage {
    /// 正常（无告警）
    #[default]
    Normal = 0,
    /// 黄：接近临界（超速 95%、过载 ≥90% 允许值、SEP ≤ 0 …）
    Caution = 1,
    /// 红：越界（超速 99%、SEP < 阈值 …）
    Alert = 2,
}

impl WarningStage {
    /// 写进 POD/CSV 的数值（`.wpr` 里存的就是它）。
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// 数值 → 等级：`>= 2` 一律当红（旧录制/越界值不至于让整块面板不显示告警）。
    pub const fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Normal,
            1 => Self::Caution,
            _ => Self::Alert,
        }
    }

    /// 两个来源里更严重的那个（`Ord::max` 的 `const` 版本，便于常量/纯函数里组链）。
    pub const fn worst(self, other: Self) -> Self {
        if (self as u8) >= (other as u8) {
            self
        } else {
            other
        }
    }

    /// 是否在告警（黄或红）。替代散落的 `> 0` 判断。
    pub const fn is_active(self) -> bool {
        (self as u8) > 0
    }
}

impl From<u8> for WarningStage {
    fn from(v: u8) -> Self {
        Self::from_u8(v)
    }
}

impl From<WarningStage> for u8 {
    fn from(s: WarningStage) -> u8 {
        s.as_u8()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DisplayData {
    pub magic: u32,
    pub version: u32,
    pub timestamp: u64,
    pub frame: u64,
    pub valid: u8,
    pub altitude: f64,
    pub tas: f64,
    pub ias: f64,
    pub mach: f64,
    pub aoa: f64,
    pub aos: f64,
    pub ny: f64,
    pub vy: f64,
    pub heading: f64,
    pub bearing: f64,
    pub roll: f64,
    pub pitch: f64,
    pub has_attitude: bool,
    pub wx: f64,
    pub aileron: f64,
    pub elevator: f64,
    pub rudder: f64,
    pub flaps: f64,
    pub gear: f64,
    pub airbrake: f64,
    pub radio_altitude: f64,
    pub trimmer: f64,
    pub fuel_kg: f64,
    pub fuel_percent: f64,
    pub fuel_time: f64,
    pub fuel1_kg: f64,
    pub fuel1_time: f64,
    pub has_fuel1: bool,
    pub engine_count: usize,
    pub engine_throttle: [f64; 8],
    pub engine_power: [f64; 8],
    pub engine_rpm: [f64; 8],
    pub engine_thrust: [f64; 8],
    pub engine_temp_water: [f64; 8],
    pub engine_temp_oil: [f64; 8],
    /// 喷气机？（core 实时判定：有推力无功率 = 喷气；有功率 = 活塞；**没有证据时保持上一次判断**，
    /// 所以地面关车不会把喷气翻成活塞。见 `core/src/display/updater.rs::update_jet_signature`）
    pub is_jet: bool,
    pub is_fm_valid: bool,
    pub engine_efficiency: f64,
    pub thrust_to_weight: f64,
    pub power_to_weight: f64,
    pub total_thrust: f64,
    pub total_hp: f64,
    pub thrust_percent: f64,
    pub sep: f64,
    pub energy_height: f64,
    pub turn_rate: f64,
    pub turn_radius: f64,
    /// 起落架边沿判定结果：false = 下降沿（正在收回）→ 起落架超速告警抑制
    pub gear_armed: bool,
    /// Two-stage overspeed warning level for IAS vs VNE:
    /// `Normal` / `Caution` (>= 95% VNE, yellow) / `Alert` (>= 99% VNE, red).
    pub overspeed_warning: WarningStage,
    /// Two-stage overspeed warning level for Mach vs MNE (same scale as
    /// `overspeed_warning`).
    pub mach_warning: WarningStage,
    /// Danger-class voice alarm active (overspeed family / critical AoA /
    /// G-load): miniHUD / circle HUD flash a warning X while true. System
    /// cautions (overheat, RPM, fuel, ...) do not set this flag.
    pub voice_alarm: bool,
    /// Speed-brake caution: brake deployed -> yellow IAS warning; cleared
    /// immediately on the retract edge.
    pub brake_caution: bool,
    pub vne: f64,
    pub vne_mach: f64,
    pub wing_sweep: f64,
    pub max_aoa: f64,
    /// 负向临界攻角（度，负值）：alpha_crit_low，襟翼/后掠插值口径同 max_aoa
    pub min_aoa: f64,
    /// 机动告警参数 m = MAX(攻角比, 过载比, 超速项)（含负向）：≥70% 蜂鸣起音，≥100% 长鸣。
    /// **表速 ≤ 64 km/h 时 core 把它压到 0**（地面滑跑不出声，见 `core::maneuver_tone::gate_by_ias`）。
    pub maneuver_margin: f64,
    pub aircraft_type: FixedBytes<64>,
    /// 过载告警等级：`Normal` = 正常，`Caution` = |Ny| 达到允许过载的 90%（红色告警）。
    /// 判据单一来源 core::display::updater::g_load_warning_level，各 panel 统一消费。
    pub g_load_warning: WarningStage,
    pub fuel_sfc: f64,
    pub fuel1_sfc: f64,
    pub manifold_pressure: f64,
    pub total_drag: f64,
    // —— 显示层格式化用的**原始数值** ——
    // core 只算数值（`format_display_text` 会搬去 `disp`，文本在渲染线程生成）；
    // 这几个量原先只存在 `core::display::FlightContext` 里，不发布的话显示层拿不到。
    /// 本次进气的最大转速（RPM；`0` = 没有涡轮数据）
    #[serde(default)]
    pub max_rpm: f64,
    /// 额定功率（Hp；喷气机为 0）
    #[serde(default)]
    pub rated_power: f64,
    /// 是否英制单位（决定歧管压力单位的写法）
    #[serde(default)]
    pub is_imperial: bool,
    /// 水冷"还能撑多久"（秒）
    #[serde(default)]
    pub water_remain_secs: f64,
    /// 油冷"还能撑多久"（秒）
    #[serde(default)]
    pub oil_remain_secs: f64,
    /// 本局已飞行秒数（`frame × 名义节拍`）
    #[serde(default)]
    pub elapsed_secs: f64,
    // —— 渲染层格式化需要的派生数值 ——
    /// 当前重量下的**允许正过载**（过载格单位栏 `/15.4` 用）
    #[serde(default)]
    pub pos_limit: f64,
    /// 发动机转速上限（RPM 单位栏 `/2850` 用；`f64::MAX` = 没有上限）
    #[serde(default)]
    pub rpm_max_allowed: f64,
    // —— logger 线程专用字段（HUD 渲染不读）——
    // 记录端只凭 `DisplayData` 重建帧，所以这里要有 ACMI 需要、其它字段导不出来的两项：
    // 单值油门与本机归一化地图坐标。
    /// 单值油门（ratio，0..1 口径；加力可 >1）。
    #[serde(default)]
    pub throttle: f64,
    /// 本机归一化地图坐标 x（0..1，东向增大；与 `MapDisplay.player_map_x` 同源同值）
    #[serde(default)]
    pub pos_x: f64,
    /// 本机归一化地图坐标 y（0..1，南向增大；与 `MapDisplay.player_map_y` 同源同值）。
    ///
    /// `pos_x/pos_y` 只在**地图对象被记录**的那几帧刷新（每
    /// [`MAP_OBJ_INTERVAL_FRAME`] 个数据帧一次），首次记录之前是初值 `(0.5, 0.5)`。
    #[serde(default)]
    pub pos_y: f64,
}

/// 底图：**一局一份**（不是每采样一份）。解码 RGB / 原始字节 / 像素尺寸**三份必须一起写**：
/// 分开赋值漏掉 `raw` 时 HUD 照旧显示，但记录线程取到的原始字节是空的 → 落盘的 `.wpr`
/// `img_len = 0`（回放没有底图）。
///
/// 它不在每采样的地图环形槽里：环形槽有 32 个、轮着用，底图一旦进环就得每次发布重写一遍，
/// 否则读者有 31/32 的概率读到空图。放在这里 = 地图加载时写一次，换图时覆盖。
#[derive(Debug, Clone, Default)]
pub struct MapImage {
    /// 解码后的 RGB（绘制用）
    pub rgb: Arc<Vec<u8>>,
    /// 游戏原样给的字节（通常 JPEG；记录用 —— 解码再编码会失真且更大）
    pub raw: Arc<Vec<u8>>,
    pub w: u32,
    pub h: u32,
}

static MAP_IMAGE: OnceLock<std::sync::Mutex<MapImage>> = OnceLock::new();

/// **core** 在地图加载时调用一次：写当前底图（三份一起写）。读者各持一份 `Arc`，
/// 换图不会打断它们正在画/正在写的那份。
pub fn set_map_image(rgb: Arc<Vec<u8>>, raw: Arc<Vec<u8>>, w: u32, h: u32) {
    let mut slot = map_image_slot().lock().unwrap_or_else(|e| e.into_inner());
    *slot = MapImage { rgb, raw, w, h };
}

/// 当前底图。每次调用 clone 两个 `Arc`（换图前的旧读者仍安全），不阻塞绘制。
pub fn map_image() -> MapImage {
    map_image_slot().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn map_image_slot() -> &'static std::sync::Mutex<MapImage> {
    MAP_IMAGE.get_or_init(|| std::sync::Mutex::new(MapImage::default()))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MapDisplay {
    pub player_map_x: f64,
    pub player_map_y: f64,
    pub map_maxsize: [f64; 2],
    /// 地图网格参数（推导区域格，如 D3）
    #[serde(default)]
    pub map_min: [f64; 2],
    #[serde(default)]
    pub grid_zero: [f64; 2],
    #[serde(default)]
    pub grid_steps: [f64; 2],
    pub map_objects: Vec<MapObjectDisplay>,
    pub nearest_poi_dist: f64,
    pub nearest_poi_bearing: f64,
    pub nearest_poi_rel_speed: f64,
    pub nearest_poi_neighbor_dist: f64,
    pub nearest_poi_neighbor_x: f64,
    pub nearest_poi_neighbor_y: f64,
    pub nearest_poi_x: f64,
    pub nearest_poi_y: f64,
    pub nearest_airfield_dist: f64,
    /// 最近机场的方位角（度，0..360；渲染层拼 `AF …` 行用）
    #[serde(default)]
    pub nearest_airfield_bearing: f64,
    pub nearest_bombing_dist: f64,
    pub nearest_bombing_bearing: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MapObjectDisplay {
    pub obj_type: [u8; 32],
    pub color: u32,
    pub icon: [u8; 16],
    pub x: f64,
    pub y: f64,
    pub ex: f64,
    pub ey: f64,
    pub dx: f64,
    pub dy: f64,
    pub distance: f64,
    pub is_player: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **记录端底图的回归守卫**：`set_map_image` 必须把"解码 RGB / 原始字节 / 像素尺寸"三份
    /// **一起**写 —— 只写 RGB/尺寸时落盘的 `.wpr` 会 `img_len = 0`（回放没有底图）。
    #[test]
    fn map_image_publishes_rgb_raw_and_size_together() {
        let rgb = Arc::new(vec![1u8, 2, 3]);
        let raw = Arc::new(vec![0xFFu8, 0xD8, 0xFF, 0xE0]);
        assert!(map_image().raw.is_empty(), "没写过时是空底图（不 panic）");

        set_map_image(Arc::clone(&rgb), Arc::clone(&raw), 2048, 2048);
        let got = map_image();
        assert_eq!(&got.rgb[..], &rgb[..]);
        assert_eq!(&got.raw[..], &raw[..], "原始字节是记录线程唯一要的东西，漏了就等于没底图");
        assert_eq!((got.w, got.h), (2048, 2048));
        assert_eq!((got.rgb.len(), got.raw.len()), (3, 4), "两份各自独立");
    }
}
