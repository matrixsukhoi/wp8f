use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use wp8f_disp::FixedBytes;
use wp8f_flightmodel::{EngineLoad, FlightModel};

use crate::math::average::SimpleMovingAverage;
use crate::parser::{MapInfo, MapObjData};
use crate::constants::{FIVE_MINUTES_SEC, NSEC_PER_SEC};

pub struct FlightContext {
    pub(crate) player_dx: f64,
    pub(crate) player_dy: f64,
    /// 本机归一化地图坐标（0..1，与 `MapDisplay.player_map_x/y` 同源同值，随地图对象采样刷新）。
    /// 每帧写进 `DisplayData.pos_x/pos_y` —— logger 线程据此换算 ACMI 经纬度，
    /// 使 core 主循环不必再参与日志拼装。
    pub(crate) player_map_pos: (f64, f64),
    pub(crate) fuel: f64,
    pub(crate) last_fuel: f64,
    pub(crate) fuel1: f64,
    pub(crate) last_fuel1: f64,
    pub(crate) fuel_flow_sma: SimpleMovingAverage,
    pub(crate) fuel1_flow_sma: SimpleMovingAverage,
    pub(crate) fuel_flow_frame_sma: SimpleMovingAverage,
    pub(crate) fuel1_flow_frame_sma: SimpleMovingAverage,

    pub(crate) fuel_change_acc_ns: u64,
    pub(crate) fuel1_change_acc_ns: u64,
    pub(crate) last_tas_raw: f64,
    pub(crate) max_thrust: f64,
    pub(crate) max_power: f64,
    pub(crate) tas_sma: SimpleMovingAverage,
    pub(crate) sep_sma: SimpleMovingAverage,
    pub(crate) turn_rate_sma: SimpleMovingAverage,
    pub(crate) turn_radius_sma: SimpleMovingAverage,
    pub(crate) ias_tas_ratio_sma: SimpleMovingAverage,
    pub(crate) poi_rel_speed_sma: SimpleMovingAverage,
    pub(crate) prev_poi_dist: f64,
    pub(crate) last_compass: f64,
    pub(crate) turn_rate: f64,
    pub(crate) turn_radius: f64,

    pub map_info: Option<MapInfo>,
    pub map_obj: Option<MapObjData>,
    pub map_img_w: u32,
    pub map_img_h: u32,
    pub(crate) engine_type_counter: i32,
    pub(crate) is_jet: bool,
    pub(crate) high_precision_fuel: bool,
    pub(crate) last_throttle: f64,
    pub(crate) peak_fuel_flow: f64,
    pub(crate) peak_sfc: f64,
    pub(crate) peak_sfc1: f64,
    pub(crate) throttle_stable_frames: u32,
    pub(crate) is_imperial: bool,
    pub(crate) imperial_check_acc: i32,
    pub flight_model: Option<FlightModel>,
    pub wep_thrust_max: f64,
    pub fm_is_valid: bool,
    pub vne: f64,
    pub no_flaps_aoa: f64,
    pub full_flaps_aoa: f64,
    pub alt_mult: f64,
    pub(crate) engine_loads: Vec<EngineLoad>,
    pub(crate) cur_water_work_times: Vec<f64>,
    pub(crate) cur_oil_work_times: Vec<f64>,
    pub(crate) water_remain_secs: f64,
    pub(crate) oil_remain_secs: f64,
    pub(crate) current_max_rpm: f64,
    pub(crate) rated_power: f64,
    pub(crate) total_thrust: f64,
    pub(crate) aircraft_type: FixedBytes<32>,
    /// Speed-brake caution (yellow IAS warning) edge detection.
    pub(crate) brake_caution: crate::display::updater::EdgeArmed,
    /// 起落架边沿判定（与减速板共用简化逻辑）：下降沿=收回中 → 抑制起落架告警
    pub(crate) gear_edge: crate::display::updater::EdgeArmed,
}

impl FlightContext {
    /// 按生效配置构造：帧率取 `refresh_hz`，地图记录间隔取 `map_obj_record_every_frames_clamped()`
    /// （两个 SMA 窗口的帧数换算跟着它们走）。
    pub fn new() -> Self {
        let map_obj_frames = wp8f_disp::hud_layout().map_obj_record_every_frames_clamped();
        Self::with_map_obj_frames(map_obj_frames)
    }

    /// 地图记录间隔（数据帧）由调用方算好传进来（core 侧唯一取值点 = `main::effective_map_obj_frames`），
    /// 帧率仍从布局配置取。
    pub fn with_map_obj_frames(map_obj_frames: u64) -> Self {
        let fps = (NSEC_PER_SEC as u64 / wp8f_disp::hud_layout().refresh_interval_ns()).max(1) as usize;
        Self::new_with(fps, map_obj_frames)
    }

    /// 显式给定帧率与地图记录间隔（帧）。夹紧在配置侧已完成（`1..=refresh_hz`）。
    ///
    /// * `fps`：主循环帧率（= `refresh_hz`），SMA 窗口按"1 秒"换算成帧数；
    /// * `map_obj_frames`：地图记录间隔（帧，>= 1）。三个按采样计数的 SMA 窗口 =
    ///   `fps / map_obj_frames`，**下限 1**：算出 0 会让 `SimpleMovingAverage::add` 除零 panic。
    pub fn new_with(fps: usize, map_obj_frames: u64) -> Self {
        // SMA 窗口按"秒"定义（1 秒 / 一个地图采样间隔），跟着 refresh_hz 走
        let map_obj_frames = map_obj_frames.max(1) as usize;
        let per_sample = (fps / map_obj_frames).max(1);
        Self {
            player_dx: 0.0,
            player_dy: 0.0,
            // 地图中心：与 main.rs 的 `datalink_pos` 初值、`populate_map_objects` 的兜底同口径
            player_map_pos: (0.5, 0.5),
            fuel: 0.0,
            last_fuel: 0.0,
            fuel1: 0.0,
            last_fuel1: 0.0,
            fuel_flow_sma: SimpleMovingAverage::new(4),
            fuel1_flow_sma: SimpleMovingAverage::new(4),
            fuel_flow_frame_sma: SimpleMovingAverage::new(64),
            fuel1_flow_frame_sma: SimpleMovingAverage::new(64),

            fuel_change_acc_ns: 0,
            fuel1_change_acc_ns: 0,
            last_tas_raw: 0.0,
            max_thrust: 0.0,
            max_power: 0.0,
            tas_sma: SimpleMovingAverage::new(fps),
            sep_sma: SimpleMovingAverage::new(fps),
            turn_rate_sma: SimpleMovingAverage::new(per_sample),
            turn_radius_sma: SimpleMovingAverage::new(per_sample),
            ias_tas_ratio_sma: SimpleMovingAverage::new(fps),
            poi_rel_speed_sma: SimpleMovingAverage::new(per_sample),
            prev_poi_dist: f64::MAX,
            last_compass: 0.0,
            turn_rate: 0.0,
            turn_radius: 0.0,

            map_info: None,
            map_obj: None,
            map_img_w: 0,
            map_img_h: 0,
            engine_type_counter: 0,
            is_jet: false,
            high_precision_fuel: false,
            last_throttle: 0.0,
            peak_fuel_flow: 0.0,
            peak_sfc: 0.0,
            peak_sfc1: 0.0,
            throttle_stable_frames: 0,
            is_imperial: false,
            imperial_check_acc: 0,
            flight_model: None,
            wep_thrust_max: 0.0,
            fm_is_valid: false,
            vne: 0.0,
            no_flaps_aoa: 15.0,
            full_flaps_aoa: 25.0,
            alt_mult: 1.0,
            engine_loads: Vec::new(),
            cur_water_work_times: Vec::new(),
            cur_oil_work_times: Vec::new(),
            water_remain_secs: 99999.0,
            oil_remain_secs: 99999.0,
            current_max_rpm: 0.0,
            rated_power: 0.0,
            total_thrust: 0.0,
            aircraft_type: FixedBytes::new(),
            brake_caution: Default::default(),
            gear_edge: Default::default(),
        }
    }

    pub fn flight_model(&self) -> &FlightModel {
        static DEFAULT_FM: LazyLock<FlightModel> = LazyLock::new(|| {
            FlightModel::default_placeholder()
        });
        self.flight_model.as_ref().unwrap_or(&DEFAULT_FM)
    }

    pub fn init_overheat_from_fm(&mut self, loads: &[EngineLoad]) {
        self.engine_loads = loads.to_vec();
        self.cur_water_work_times = loads.iter().map(|l| l.work_time * 1000.0).collect();
        self.cur_oil_work_times = loads.iter().map(|l| l.work_time * 1000.0).collect();
    }

    pub fn load_flight_model(&mut self, fm: &FlightModel) {
        self.wep_thrust_max = fm.wep_thrust_max();
        self.vne = fm.vne();
        self.no_flaps_aoa = fm.aerodynamics.get_cl_aoa_max(0.0, 0.0).1;
        self.full_flaps_aoa = fm.aerodynamics.get_cl_aoa_max(1.0, 0.0).1;
        self.flight_model = Some(fm.clone());
        self.fm_is_valid = true;
        self.init_overheat_from_fm(fm.engine_loads());
    }

    pub fn airbrake_drag_factor(&self) -> f64 {
        let airbrake_cd = self.flight_model().airbrake_cd();
        if airbrake_cd < 0.001 {
            return 0.0;
        }

        let wep_fuel_mass = self.flight_model().get_fuel_rate_at(800.0, 0.0, true) * FIVE_MINUTES_SEC;
        let total_drag_weight = self.flight_model().flight_weight(wep_fuel_mass);

        if total_drag_weight <= 0.0 {
            return 0.0;
        }

        airbrake_cd / total_drag_weight
    }
}

impl Default for FlightContext {
    fn default() -> Self {
        Self::new()
    }
}

pub fn get_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// 当前 UNIX 纳秒 —— **core 内部的时间基准**（主循环节拍、帧时间戳都用它）。
/// `DisplayData.timestamp` 也是这个口径（每帧写一次），飞行记录原样存进 `.wpr`，
/// 需要秒/毫秒时由读出方（回放/GUI 转换器）换算。
pub fn get_timestamp_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无参 `new()` 与采样周期无关的部分：SMA 窗口按帧率与采样周期各算各的。
    #[test]
    fn new_uses_the_configured_refresh_hz() {
        let ctx = FlightContext::new();
        let hz = wp8f_disp::hud_layout().refresh_hz_clamped() as usize;
        assert_eq!(ctx.tas_sma.n(), hz, "秒级窗口 = 一帧率");
        assert!(ctx.turn_rate_sma.n() >= 1, "按采样计数的窗口下限 1");
    }

    /// 采样周期超过帧率时 `fps / frames` 会算出 0，窗口为 0 的 `SimpleMovingAverage::add`
    /// 会除零 panic：最坏组合（5 Hz + 600 帧）必须兜到 1。
    #[test]
    fn per_sample_sma_window_never_collapses_to_zero() {
        let mut ctx = FlightContext::new_with(5, 600);
        assert_eq!(ctx.turn_rate_sma.n(), 1);
        assert_eq!(ctx.turn_radius_sma.n(), 1);
        assert_eq!(ctx.poi_rel_speed_sma.n(), 1);
        assert_eq!(ctx.turn_rate_sma.add(7.0), 7.0, "窗口 1 也要能加，不能 panic");
        // 60 Hz + 1 帧（每帧采样）：窗口 = 1 秒的帧数
        let ctx60 = FlightContext::new_with(60, 1);
        assert_eq!(ctx60.turn_rate_sma.n(), 60);
        // 30 Hz + 8 帧（历史默认）：窗口 = 30 / 8 = 3（向下取整，与解耦前一致）
        let ctx30 = FlightContext::new_with(30, 8);
        assert_eq!(ctx30.turn_rate_sma.n(), 3);
        assert_eq!(ctx30.tas_sma.n(), 30, "秒级窗口仍按帧率");
    }
}
