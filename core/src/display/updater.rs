use crate::math::angle::{
    angle_difference_deg, compass_deg, normalize_angle_deg, parse_hex_color, round_sep,
    target_bearing_deg,
};
use crate::math::average::SimpleMovingAverage;
use crate::math::map::{
    extract_airfield_origin_coords, extract_map_object_coords, extract_point_coords,
};
use crate::context::FlightContext;
use crate::frame_clock::Tick;
use crate::fmt::time::format_duration;
use crate::fmt::value::{format_adaptive, format_sig_figs};
use crate::parser::{FlightState, Indicators, MapObjData};
use crate::constants::{
    FEET_TO_METERS, FUEL_TIME_ROUNDING, GRAVITY, HP_CONVERSION,
    INFINITE_FUEL_TIME, MS_TO_KMH, SEC_PER_HOUR,
    SMOOTHING_FACTOR_NEW, SMOOTHING_FACTOR_OLD, NSEC_PER_SEC,
};
use crate::DisplayData;
use wp8f_disp::FixedBytes;
use wp8f_disp::MapDisplay;
use wp8f_disp::MapObjectDisplay;
use wp8f_flightmodel::EngineLoad;

fn set_text<const N: usize>(field: &mut FixedBytes<N>, args: std::fmt::Arguments<'_>) {
    field.clear();
    use std::fmt::Write;
    let _ = write!(field, "{}", args);
}

fn set_text_str<const N: usize>(field: &mut FixedBytes<N>, text: &str) {
    field.assign(text.as_bytes());
}

fn set_time_or_dash<const N: usize>(field: &mut FixedBytes<N>, time_val: f64, min: f64, max: f64) {
    if time_val > min && time_val < max {
        let secs = (time_val / FUEL_TIME_ROUNDING).round() * FUEL_TIME_ROUNDING;
        set_text_str(field, &format_duration(secs));
    } else {
        set_text_str(field, "-");
    }
}

fn write_manifold_pressure_value<const N: usize>(field: &mut FixedBytes<N>, manifold_atm: f64, imperial: bool) {
    field.clear();
    use std::fmt::Write;
    if imperial {
        let _ = write!(field, "{:+.1}", (manifold_atm - 1.0) * crate::constants::PSI_PER_ATM);
    } else {
        let _ = write!(field, "{:.2}", manifold_atm);
    }
}

fn write_manifold_pressure_unit<const N: usize>(field: &mut FixedBytes<N>, manifold_atm: f64, imperial: bool) {
    field.clear();
    use std::fmt::Write;
    if imperial {
        let _ = write!(field, "P/{:.0}''", manifold_atm * crate::constants::MMHG_PER_ATM / crate::constants::MM_PER_INCH);
    } else {
        set_text_str(field, "Ata");
    }
}

#[inline]
fn tas_kmh_to_ms(tas_kmh: f64) -> f64 {
    tas_kmh / MS_TO_KMH
}

#[inline]
fn compute_tas_ms(indic: &Indicators, state: &FlightState, ctx: &FlightContext) -> f64 {
    if indic.speed > 0.0 && ctx.ias_tas_ratio_sma.n() > 0 {
        let ratio = ctx.ias_tas_ratio_sma.average();
        if ratio > 0.0 {
            indic.speed / ratio
        } else {
            tas_kmh_to_ms(state.tas)
        }
    } else {
        tas_kmh_to_ms(state.tas)
    }
}

#[inline]
fn or_state(indic_val: f64, state_val: f64) -> f64 {
    if indic_val > 0.0 {
        indic_val
    } else {
        state_val
    }
}

#[inline]
fn or_state_scale(indic_val: f64, state_val: f64, scale: f64) -> f64 {
    if indic_val > 0.0 {
        indic_val * scale
    } else {
        state_val
    }
}

/// 本帧的全部时间量都由调用方算好（[`Tick`]）：更新器自己不读墙钟、不读全局配置。
pub fn update_display_from_state(
    state: &FlightState,
    indic: &Indicators,
    display_data: &mut DisplayData,
    ctx: &mut FlightContext,
    tick: &Tick,
) {
    let (frame, interval_ns, mapobj_interval_ns) =
        (tick.frame, tick.interval_ns, tick.mapobj_interval_ns);
    // Phase 1: Update raw data only (no formatting)
    update_flight_data(state, indic, display_data, ctx);
    update_engine_display(state, indic, display_data, ctx);
    update_jet_signature(state, ctx, display_data);
    update_performance_metrics(state, ctx, display_data);
    calculate_fuel(state, indic, display_data, ctx, interval_ns);
    calculate_fuel1(state, display_data, ctx, interval_ns);
    let tas_ms = compute_tas_ms(indic, state, ctx);
    update_sep_calculation(ctx, display_data, interval_ns, tas_ms);
    display_data.turn_rate = ctx.turn_rate;
    display_data.turn_radius = ctx.turn_radius;
    if mapobj_interval_ns > 0 {
        update_turn_calculation(display_data, ctx, tas_ms, mapobj_interval_ns);
    }
    calculate_overheat(state, indic, display_data, ctx, interval_ns);

    display_data.frame = frame;
    display_data.valid = 1;
    // 帧时间（UNIX **纳秒**）：飞行记录原样存进 `.wpr` 的时间列，与帧号同一拍
    display_data.timestamp = tick.timestamp_ns;

    // 记录线程重建一行记录所需的字段（HUD 渲染不读）：单值油门 + 本机归一化地图坐标。
    // 油门口径兼容 0..1 与 0..100（原 main.rs 的 ACMI 拼装逻辑原样搬到这里，数值不变）；
    // 坐标取 ctx.player_map_pos —— 与 `MapDisplay.player_map_x/y` 同源（上一次地图对象采样）。
    display_data.throttle = if indic.throttle > 1.5 {
        indic.throttle / 100.0
    } else {
        indic.throttle
    };
    display_data.pos_x = ctx.player_map_pos.0;
    display_data.pos_y = ctx.player_map_pos.1;

    // Phase 2: Format text from computed values
    format_display_text(display_data, ctx, tick);
}

/// 过载告警阈值：|Ny| 达到允许过载的该比例即告警（语音 + 红色，各 panel 统一）
pub const G_WARN_RATIO: f64 = 0.9;

/// 过载告警级别（**各 panel/语音的唯一判据**）：
/// `|Ny|` 达到允许过载（按方向取正/负限值）的 `G_WARN_RATIO`（90%）→ 1，否则 0。
pub fn g_load_warning_level(ny: f64, neg_limit: f64, pos_limit: f64) -> u8 {
    let lim = if ny >= 0.0 { pos_limit } else { neg_limit.abs() };
    if lim > 0.0 && ny.abs() >= lim * G_WARN_RATIO {
        1
    } else {
        0
    }
}

/// Stage-1 warning line: 95% of the limit (VNE for IAS, MNE for Mach).
/// This is the same line the voice warnings use (`get_ias_warning_line`).
pub const WARNING_STAGE1_RATIO: f64 = 0.95;

/// Stage-2 warning line: 99% of the limit (red).
pub const WARNING_STAGE2_RATIO: f64 = 0.99;

/// Two-stage overspeed warning level for a value against its limit:
/// `0` normal, `1` >= 95% of limit (yellow), `2` >= 97.5% of limit (red).
pub fn warning_stage(value: f64, limit: f64) -> u8 {
    if limit <= 0.0 || !value.is_finite() {
        return 0;
    }
    if value >= limit * WARNING_STAGE2_RATIO {
        2
    } else if value >= limit * WARNING_STAGE1_RATIO {
        1
    } else {
        0
    }
}

/// 放开/收回的边沿判定（减速板提醒、起落架告警共用）：
/// - **下降沿**（相比前值减少，哪怕只减一点）→ 告警清除并锁存；
/// - **上升沿**（增加）→ 告警；
/// - **同值帧** → 保持原状态。
///
/// 同值帧保持是关键：帧循环 30fps 高于游戏遥测写入率，收回中途大量帧读到
/// 与上帧相同的值 —— 若同值视为"未下降"，会在收回过程中不断重新告警。
/// 无防抖死区/迟滞/峰值跟踪（按需求简化；位置值非负，无需负值防御）。
#[derive(Debug, Default, Clone, Copy)]
pub struct EdgeArmed {
    prev: f64,
    warn: bool,
}

impl EdgeArmed {
    /// 推进一帧，返回当前帧是否告警。
    pub fn update(&mut self, value: f64) -> bool {
        if value < self.prev {
            self.warn = false; // 下降沿（收回指令）→ 清除并锁存
        } else if value > self.prev {
            self.warn = true; // 上升沿（放开/继续放开）→ 告警
        }
        self.prev = value;
        self.warn
    }
}

fn update_flight_data(
    state: &FlightState,
    indic: &Indicators,
    display_data: &mut DisplayData,
    ctx: &mut FlightContext,
) {
    ctx.aircraft_type = indic.aircraft_type.clone();
    display_data.altitude = state.altitude;
    display_data.tas = ctx.tas_sma.add(state.tas);
    display_data.ias = or_state_scale(indic.speed, state.ias, MS_TO_KMH);
    if state.tas > 0.0 {
        let ias_ms = indic.speed;
        let tas_ms = tas_kmh_to_ms(state.tas);
        let ratio = ias_ms / tas_ms;
        ctx.ias_tas_ratio_sma.add(ratio);
    }
    display_data.mach = or_state(indic.mach, state.M);
    display_data.aoa = or_state(indic.aoa, state.aoa);
    display_data.aos = state.aos;
    display_data.ny = state.Ny;
    display_data.vy = state.vy;

    display_data.roll = indic.roll;
    display_data.pitch = indic.pitch;
    display_data.has_attitude = indic.has_attitude;
    display_data.wx = state.wx;
    display_data.heading = compass_deg(ctx.player_dx, ctx.player_dy);

    display_data.aileron = state.aileron;
    display_data.elevator = state.elevator;
    display_data.rudder = state.rudder;
    display_data.flaps = state.flaps;
    display_data.gear = state.gear;
    // 起落架边沿判定（与减速板共用简化逻辑）：下降沿=正在收回 → 抑制起落架告警
    display_data.gear_armed = ctx.gear_edge.update(state.gear);
    display_data.airbrake = state.airbrake;
    // Speed-brake caution: yellow IAS warning while the brake is deployed,
    // cleared immediately when retraction starts (see `BrakeCaution`).
    display_data.brake_caution = ctx.brake_caution.update(state.airbrake);
    ctx.alt_mult = if indic.altitude_10k > state.altitude * 2.0 {
        FEET_TO_METERS
    } else {
        1.0
    };
    display_data.radio_altitude = indic.radio_altitude * ctx.alt_mult;
    display_data.trimmer = indic.trimmer;

    display_data.wing_sweep = indic.wing_sweep_indicator;
    let wing_sweep = indic.wing_sweep_indicator;
    let flaps = state.flaps / 100.0;
    let max_aoa = ctx.flight_model().aerodynamics.get_cl_aoa_max(flaps, wing_sweep).1;
    display_data.max_aoa = max_aoa;
    // 负向临界攻角（机动告警声负向分支）：alpha_crit_low，口径同 max_aoa
    display_data.min_aoa = ctx.flight_model().aerodynamics.get_aoa_min(flaps, wing_sweep);
    display_data.vne = ctx.flight_model().aerodynamics.get_vne(wing_sweep);
    display_data.vne_mach = ctx.flight_model().aerodynamics.wing_geometry.get_vne_mach(wing_sweep);
    if display_data.vne > 0.0 {
        set_text(&mut display_data.vne_text, format_args!("/{:.0}", display_data.vne));
    } else {
        display_data.vne_text.clear();
    }
    if display_data.vne_mach > 0.0 {
        set_text(&mut display_data.mne_text, format_args!("/{:.2}", display_data.vne_mach));
    } else {
        display_data.mne_text.clear();
    }

    // Two-stage overspeed warnings (shared by miniHUD / circle HUD / flight panel):
    // stage 1 (yellow) at the warning line, stage 2 (red) 2.5 points above it.
    display_data.overspeed_warning = warning_stage(display_data.ias, display_data.vne);
    display_data.mach_warning = warning_stage(display_data.mach, display_data.vne_mach);

    if state.vy.abs() > 0.1 {
        if indic.altitude_10k > state.altitude * 2.0 {
            ctx.imperial_check_acc = (ctx.imperial_check_acc + 1).min(100);
        } else {
            ctx.imperial_check_acc = (ctx.imperial_check_acc - 1).max(-100);
        }
    }
    ctx.is_imperial = ctx.imperial_check_acc > 10;
}

fn update_engine_display(state: &FlightState, indic: &Indicators, display_data: &mut DisplayData, ctx: &mut FlightContext) {
    display_data.engine_count = state.engine_count;

    let mut thrust_sum = 0.0_f64;
    let mut throttle_sum = 0.0_f64;
    let mut max_rpm = 0.0_f64;

    for (i, engine) in state.engines[..state.engine_count].iter().enumerate() {
        engine.copy_to_display(
            indic,
            &mut display_data.engine_throttle,
            &mut display_data.engine_power,
            &mut display_data.engine_rpm,
            &mut display_data.engine_thrust,
            &mut display_data.engine_temp_water,
            &mut display_data.engine_temp_oil,
            i,
        );
        thrust_sum += engine.thrust;
        throttle_sum += engine.throttle;
        max_rpm = max_rpm.max(engine.rpm);
    }
    debug_assert!(state.engine_count > 0);
    let engine = &state.engines[0];
    display_data.manifold_pressure = if indic.manifold_pressure > 0.0 { indic.manifold_pressure } else { engine.manifold };

    ctx.total_thrust = thrust_sum;
    display_data.total_thrust = thrust_sum;
    ctx.current_max_rpm = max_rpm;

    let tas_ms = tas_kmh_to_ms(state.tas);
    let total_hp_eff = thrust_sum * GRAVITY * tas_ms / HP_CONVERSION;
    display_data.total_hp = total_hp_eff;

    let throttle_avg = if state.engine_count > 0 {
        throttle_sum / state.engine_count as f64
    } else {
        0.0
    };

    if throttle_avg >= 100.0 {
        if thrust_sum > ctx.max_thrust {
            ctx.max_thrust = ctx.max_thrust * SMOOTHING_FACTOR_OLD + thrust_sum * SMOOTHING_FACTOR_NEW;
        }
        if total_hp_eff > ctx.max_power {
            ctx.max_power = ctx.max_power * SMOOTHING_FACTOR_OLD + total_hp_eff * SMOOTHING_FACTOR_NEW;
        }
    }

    let reference_thrust = if ctx.is_jet && ctx.wep_thrust_max > 0.0 {
        ctx.wep_thrust_max
    } else if ctx.max_thrust > 0.0 {
        ctx.max_thrust
    } else {
        0.0
    };

    display_data.thrust_percent = if reference_thrust > 0.0 {
        thrust_sum / reference_thrust * 100.0
    } else {
        0.0
    };
}

fn advance_work_times(
    cur: &mut Vec<f64>,
    loads: &[EngineLoad],
    cur_load: usize,
    eng_off: bool,
    dt_ms: f64,
) {
    if cur.len() != loads.len() {
        *cur = loads.iter().map(|l| l.work_time * 1000.0).collect();
    }
    for i in 0..loads.len() {
        let load = &loads[i];
        if i < cur_load {
            cur[i] = (cur[i] - dt_ms).max(0.0);
        } else if eng_off && (cur_load == 0 || loads[cur_load.saturating_sub(1)].work_time < 0.1) {
            cur[i] = load.work_time * 1000.0;
        } else if load.recover_time > 0.0 && cur[i] < load.work_time * 1000.0 {
            cur[i] = (cur[i] + dt_ms * load.work_time / load.recover_time).min(load.work_time * 1000.0);
        }
    }
}

fn remain_secs(cur: &[f64], loads: &[EngineLoad], cur_load: usize) -> f64 {
    (0..cur_load)
        .filter_map(|i| if loads[i].work_time > 0.0 { Some((cur[i] / 1000.0).max(0.0)) } else { None })
        .fold(f64::INFINITY, f64::min)
        .min(99999.0)
}

fn calculate_overheat(
    state: &FlightState,
    indic: &Indicators,
    _display_data: &mut DisplayData,
    ctx: &mut FlightContext,
    interval_ns: u64,
) {
    if ctx.engine_loads.is_empty() {
        return;
    }

    let water_temp = indic.temperature.round().max(0.0);
    let oil_temp = indic.oil_temp.round().max(0.0);
    let dt_ms = interval_ns as f64 / 1_000_000.0;
    let n_loads = ctx.engine_loads.len();

    let cur_w_load = ctx.engine_loads.iter().position(|l| water_temp < l.water_limit).unwrap_or(n_loads);
    let cur_o_load = ctx.engine_loads.iter().position(|l| oil_temp < l.oil_limit).unwrap_or(n_loads);

    let mut eng_power_sum = 0.0_f64;
    let mut eng_any_throttle = false;
    for e in &state.engines[..state.engine_count] {
        eng_power_sum += e.power;
        if e.throttle > 0.0 { eng_any_throttle = true; }
    }
    let eng_off = eng_power_sum == 0.0 && eng_any_throttle;

    advance_work_times(
        &mut ctx.cur_water_work_times,
        &ctx.engine_loads, cur_w_load, eng_off, dt_ms,
    );
    advance_work_times(
        &mut ctx.cur_oil_work_times,
        &ctx.engine_loads, cur_o_load, eng_off, dt_ms,
    );

    ctx.water_remain_secs = remain_secs(&ctx.cur_water_work_times, &ctx.engine_loads, cur_w_load);
    ctx.oil_remain_secs = remain_secs(&ctx.cur_oil_work_times, &ctx.engine_loads, cur_o_load);
}

fn update_jet_signature(
    state: &FlightState,
    ctx: &mut FlightContext,
    display_data: &mut DisplayData,
) {
    let is_jet_signature = state
        .engines
        .iter()
        .any(|e| e.power == 0.0 && e.thrust > 0.0);

    ctx.engine_type_counter = if is_jet_signature {
        (ctx.engine_type_counter + 1).min(crate::constants::ENGINE_TYPE_COUNTER_MAX)
    } else {
        (ctx.engine_type_counter - 1).max(-crate::constants::ENGINE_TYPE_COUNTER_MAX)
    };

    ctx.is_jet = ctx.engine_type_counter > 0;
    display_data.is_jet = ctx.is_jet;
}

fn update_performance_metrics(
    state: &FlightState,
    ctx: &mut FlightContext,
    display_data: &mut DisplayData,
) {
    let tas_ms = tas_kmh_to_ms(state.tas);
    display_data.energy_height = display_data.altitude + (tas_ms * tas_ms) / (2.0 * GRAVITY);
    let rated_power: f64 = display_data.engine_power[..display_data.engine_count].iter().sum();
    ctx.rated_power = rated_power;
    let efficiency = if rated_power > 0.0 && !ctx.is_jet {
        (display_data.total_hp / rated_power * 100.0).min(100.0)
    } else {
        0.0
    };
    display_data.engine_efficiency = efficiency;
    display_data.is_fm_valid = ctx.fm_is_valid;

    let current_weight = if let Some(fm) = &ctx.flight_model {
        fm.flight_weight(ctx.fuel)
    } else {
        ctx.fuel
    };
    if !ctx.fm_is_valid || current_weight <= 0.0 {
        display_data.thrust_to_weight = 0.0;
        display_data.power_to_weight = 0.0;
    } else {
        if ctx.is_jet {
            display_data.thrust_to_weight = display_data.total_thrust / current_weight;
        } else {
            display_data.thrust_to_weight = display_data.total_hp / current_weight;
        }
        display_data.power_to_weight = display_data.total_hp / current_weight;
    }

    let tas_ms = tas_kmh_to_ms(display_data.tas);
    let drag = if tas_ms > 0.0 && current_weight > 0.0 && ctx.fm_is_valid {
        display_data.total_thrust - display_data.sep * current_weight / tas_ms
    } else {
        0.0
    };
    display_data.total_drag = drag.max(0.0);
}

fn update_fuel_branch(
    fuel_val: f64,
    last_fuel: &mut f64,
    flow_sma: &mut SimpleMovingAverage,
    frame_flow_sma: &mut SimpleMovingAverage,
    change_acc_ns: &mut u64,
    high_precision: bool,
    total_thrust: f64,
    interval_ns: u64,
) -> (f64, f64, f64) {
    *change_acc_ns += interval_ns;

    let frame_delta = *last_fuel - fuel_val;
    let frame_flow_avg = frame_flow_sma.add(frame_delta);

    if fuel_val != *last_fuel && fuel_val < *last_fuel {
        let dfuel = *last_fuel - fuel_val;
        let change_flow = dfuel * NSEC_PER_SEC as f64 / *change_acc_ns as f64;
        flow_sma.add(change_flow);
        *change_acc_ns = 0;
    }

    let flow = if high_precision { frame_flow_avg * NSEC_PER_SEC as f64 / interval_ns as f64 } else { flow_sma.average() };
    let sfc = if total_thrust > 0.0 && flow > 0.0 { flow * SEC_PER_HOUR / total_thrust } else { -1.0 };
    *last_fuel = fuel_val;
    let fuel_time = if flow > 0.0 { fuel_val / flow } else { INFINITE_FUEL_TIME };
    (fuel_time, flow, sfc)
}

fn calculate_fuel(
    state: &FlightState,
    indic: &Indicators,
    display_data: &mut DisplayData,
    ctx: &mut FlightContext,
    interval_ns: u64,
) {
    ctx.fuel = if indic.fuel > 0.0 {
        indic.fuel
    } else if indic.fuel_tank_count > 0 {
        indic.fuels[..indic.fuel_tank_count].iter().sum()
    } else {
        state.fuel
    };
    display_data.fuel_kg = ctx.fuel;
    // fuel0 = 0（机型数据缺失）时百分比无意义：写 0，别让 NaN 一路进屏幕与 `.wpr`
    display_data.fuel_percent = if state.fuel0 > 0.0 { ctx.fuel / state.fuel0 * 100.0 } else { 0.0 };
    ctx.high_precision_fuel = ctx.is_jet || indic.fuel > 0.0 || indic.fuel_tank_count > 0;

    let (fuel_time, flow, sfc) = update_fuel_branch(
        ctx.fuel, &mut ctx.last_fuel,
        &mut ctx.fuel_flow_sma, &mut ctx.fuel_flow_frame_sma,
        &mut ctx.fuel_change_acc_ns,
        ctx.high_precision_fuel, ctx.total_thrust, interval_ns,
    );
    let throttle = display_data.engine_throttle[0];
    if throttle != ctx.last_throttle {
        ctx.last_throttle = throttle;
        ctx.throttle_stable_frames = 0;
        ctx.peak_fuel_flow = 0.0;
        ctx.peak_sfc = 0.0;
    }
    ctx.throttle_stable_frames += 1;
    if ctx.throttle_stable_frames >= ctx.fuel_flow_frame_sma.n() as u32 {
        if flow > ctx.peak_fuel_flow { ctx.peak_fuel_flow = flow; }
        if sfc > ctx.peak_sfc { ctx.peak_sfc = sfc; }
    }
    display_data.fuel_time = if ctx.peak_fuel_flow > 0.0 {
        (ctx.fuel / ctx.peak_fuel_flow).min(fuel_time)
    } else {
        fuel_time
    };
    display_data.fuel_sfc = if sfc > 0.0 { sfc.max(ctx.peak_sfc) } else { -1.0 };
}

fn calculate_fuel1(
    state: &FlightState,
    display_data: &mut DisplayData,
    ctx: &mut FlightContext,
    interval_ns: u64,
) {
    ctx.fuel1 = state.fuel1;
    display_data.fuel1_kg = ctx.fuel1;
    display_data.has_fuel1 = ctx.fuel1 > 0.0;

    let (fuel1_time, _, sfc1) = update_fuel_branch(
        ctx.fuel1, &mut ctx.last_fuel1,
        &mut ctx.fuel1_flow_sma, &mut ctx.fuel1_flow_frame_sma,
        &mut ctx.fuel1_change_acc_ns,
        ctx.high_precision_fuel, ctx.total_thrust, interval_ns,
    );
    display_data.fuel1_time = fuel1_time;
    if sfc1 > ctx.peak_sfc1 { ctx.peak_sfc1 = sfc1; }
    display_data.fuel1_sfc = if sfc1 > 0.0 { sfc1.max(ctx.peak_sfc1) } else { -1.0 };
}

fn update_sep_calculation(
    ctx: &mut FlightContext,
    display_data: &mut DisplayData,
    interval_ns: u64,
    tas_ms: f64,
) {
    let diff = tas_ms - ctx.last_tas_raw;
    let sep_raw = (tas_ms + ctx.last_tas_raw) * (diff * NSEC_PER_SEC)
        / (2.0 * GRAVITY * interval_ns as f64)
        + display_data.vy;
    display_data.sep = ctx.sep_sma.add(sep_raw);
    ctx.last_tas_raw = tas_ms;
}

fn update_turn_calculation(
    display_data: &mut DisplayData,
    ctx: &mut FlightContext,
    tas_ms: f64,
    interval_ns: u64,
) {
    let rate = if display_data.has_attitude {
        // 姿态仪机型：由 AoA/AoS 求速度矢量方向，合力（重力 + 法向过载）
        // 解速度矢量转弯率
        velocity_turn_rate_deg(
            display_data.heading,
            display_data.pitch,
            display_data.roll,
            display_data.aoa,
            display_data.aos,
            display_data.ny,
            tas_ms,
        )
    } else {
        // 无姿态仪机型：罗盘变化率（盘旋率）
        let dcompass = angle_difference_deg(ctx.last_compass, display_data.heading);
        dcompass * NSEC_PER_SEC / interval_ns as f64
    };
    let smoothed = ctx.turn_rate_sma.add(rate.abs());
    ctx.turn_rate = smoothed;
    let rad = smoothed.abs() * std::f64::consts::PI / 180.0;
    if rad > 0.0 { ctx.turn_radius = ctx.turn_radius_sma.add(tas_ms / rad); }
    ctx.last_compass = display_data.heading;
}

/// 姿态仪机型的转弯率 [deg/s]：速度矢量转动率（合力法）。
///
/// 1. 由 AoA/AoS 得速度矢量在机体系的方向 `(cosα·cosβ, sinβ, sinα·cosβ)`，
///    经姿态（航向/俯仰/滚转）转到 NED（北-东-地）坐标系；
/// 2. 合力加速度 `a⃗ = Ny·g·n̂ + g⃗`（`n̂` = 机体法向=机体向上，`g⃗` = 重力向下）；
/// 3. 速度矢量转动率 `ω⃗ = (v̂ × a⃗) / V`，取幅值换算为 deg/s。
///
/// 校验：水平 45° 坡度转弯 = g·tanφ/V；正拉杆 = (n−1)·g/V（见单元测试）。
/// 姿态角符号约定：pitch 取负（抬头为正），roll/aos 按游戏原值——输出为幅值，
/// 姿态符号不影响结果。
fn velocity_turn_rate_deg(
    heading_deg: f64,
    pitch_deg: f64,
    roll_deg: f64,
    aoa_deg: f64,
    aos_deg: f64,
    ny: f64,
    tas_ms: f64,
) -> f64 {
    if tas_ms <= 0.0 {
        return 0.0;
    }
    let d = std::f64::consts::PI / 180.0;
    let (sh, ch) = (heading_deg * d).sin_cos();
    let (sp, cp) = (-pitch_deg * d).sin_cos(); // 抬头为正
    let (sr, cr) = (roll_deg * d).sin_cos();
    let (sa, ca) = (aoa_deg * d).sin_cos();
    let (sb, cb) = (aos_deg * d).sin_cos();

    // 机体系 → NED：Rz(heading)·Ry(pitch)·Rx(roll)
    let to_ned = |x: f64, y: f64, z: f64| {
        let (x1, y1, z1) = (x, y * cr - z * sr, y * sr + z * cr);
        let (x2, y2, z2) = (x1 * cp + z1 * sp, y1, -x1 * sp + z1 * cp);
        (x2 * ch - y2 * sh, x2 * sh + y2 * ch, z2)
    };

    // 速度矢量方向（AoA/AoS）与机体法向（上）
    let (vx, vy, vz) = to_ned(ca * cb, sb, sa * cb);
    let (nx, nyv, nz) = to_ned(0.0, 0.0, -1.0);

    // 合力 = 法向过载 + 重力（NED 下重力沿 +Z）
    let g = GRAVITY;
    let (ax, ay, az) = (ny * g * nx, ny * g * nyv, ny * g * nz + g);

    // ω⃗ = (v̂ × a⃗) / V
    let (wx, wy, wz) = (
        (vy * az - vz * ay) / tas_ms,
        (vz * ax - vx * az) / tas_ms,
        (vx * ay - vy * ax) / tas_ms,
    );
    (wx * wx + wy * wy + wz * wz).sqrt().to_degrees()
}

/// 本机在一次地图采样里的归一化位置与位移（`populate_map_objects` 的产物）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct PlayerXy {
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
}

/// 一次地图采样的输入（省掉 `update_map_nearest_targets` 的 10 参签名）。
struct MapSample<'a> {
    map_obj: &'a Option<MapObjData>,
    maxsize: (f64, f64),
    player: PlayerXy,
}

pub fn update_map_display(map: &mut MapDisplay, ctx: &mut FlightContext, dt_ns: u64) {
    let map_maxsize = match ctx.map_info.as_ref() {
        Some(mi) => {
            // 网格参数同步给 disp：友军面板据此推导区域格（如 D3）
            map.map_min = mi.map_min;
            map.grid_zero = mi.grid_zero;
            map.grid_steps = mi.grid_steps;
            mi.maxsize
        }
        None => return,
    };
    map.map_maxsize = map_maxsize;

    // 借用不到 `ctx.map_obj` 的同时又可变借 `ctx`：整体移出（O(1)，不是深拷贝），用完放回
    let map_obj = ctx.map_obj.take();
    let player = populate_map_objects(map, ctx, &map_obj);
    let sample = MapSample { map_obj: &map_obj, maxsize: (map_maxsize[0], map_maxsize[1]), player };

    let nearest_airfield_bearing = update_map_nearest_targets(map, &sample, ctx, dt_ns);
    ctx.map_obj = map_obj;

    ctx.player_dx = player.dx;
    ctx.player_dy = player.dy;
    // 本机归一化坐标：同一次地图采样的结果同步给 ctx，由 update_display_from_state
    // 每帧写进 DisplayData.pos_x/pos_y（与上面写进 map.player_map_x/y 的是同一对值）
    ctx.player_map_pos = (player.x, player.y);

    if let Some(ref map_info) = ctx.map_info {
        format_map_text(map, map_info, nearest_airfield_bearing, player.x, player.y);
    }
}

fn populate_map_objects(
    map: &mut MapDisplay,
    ctx: &FlightContext,
    map_obj: &Option<MapObjData>,
) -> PlayerXy {
    use crate::math::map::calc_distance_to_player;

    let map_info = match ctx.map_info {
        Some(ref mi) => mi,
        None => {
            return PlayerXy { x: 0.5, y: 0.5, dx: 0.0, dy: 0.0 };
        }
    };

    let mut player_x = 0.5;
    let mut player_y = 0.5;
    let mut player_dx = 0.0;
    let mut player_dy = 0.0;

    map.map_objects.clear();

    if let Some(ref m_obj) = map_obj {
        for item in &m_obj.objects {
            let Some(coords) = extract_map_object_coords(item) else {
                continue;
            };
            let obj_x = coords.x;
            let obj_y = coords.y;
            let obj_dx = coords.dx;
            let obj_dy = coords.dy;

            if item.icon.as_str() == "Player" {
                player_x = obj_x;
                player_y = obj_y;
                player_dx = item.dx.unwrap_or(0.0);
                player_dy = item.dy.unwrap_or(0.0);
            }

            let distance = calc_distance_to_player(player_x, player_y, obj_x, obj_y, map_info);
            let is_player = item.icon.as_str() == "Player";

            let obj_type: [u8; 32] =
                std::array::from_fn(|i| item.obj_type.as_bytes().get(i).copied().unwrap_or(0));

            let icon: [u8; 16] = std::array::from_fn(|i| item.icon.as_bytes().get(i).copied().unwrap_or(0));

            let color = parse_hex_color(item.color.as_str());

            let (ex, ey) = if item.obj_type.as_str() == "airfield" {
                (item.ex.unwrap_or(0.0), item.ey.unwrap_or(0.0))
            } else {
                (0.0, 0.0)
            };

            map.map_objects.push(MapObjectDisplay {
                obj_type,
                color,
                icon,
                x: obj_x,
                y: obj_y,
                ex,
                ey,
                dx: obj_dx,
                dy: obj_dy,
                distance,
                is_player,
            });
        }
    }

    PlayerXy { x: player_x, y: player_y, dx: player_dx, dy: player_dy }
}

fn write_format_dist<const N: usize>(field: &mut FixedBytes<N>, prefix: &str, dist: f64, bearing: f64) {
    field.clear();
    use std::fmt::Write;
    // 可读性优先：距离 / 方位角用 " / " 分隔（例 "POI 17.5km / 172°"），
    // 偏移距离由地图面板追加成 "POI 17.5km / 172° / 63m"。
    if dist < f64::MAX {
        if dist < 1000.0 {
            let _ = write!(field, "{} {:.0}m / {:03.0}°", prefix, dist, bearing);
        } else {
            let _ = write!(field, "{} {:.1}km / {:03.0}°", prefix, dist / 1000.0, bearing);
        }
    } else {
        let _ = write!(field, "{} -", prefix);
    }
}

fn update_map_nearest_targets(
    map: &mut MapDisplay,
    sample: &MapSample<'_>,
    ctx: &mut FlightContext,
    dt_ns: u64,
) -> f64 {
    use crate::math::map::calc_weighted_distance;

    // 解构回原来的局部名：下面的遍历逻辑一行没改
    let MapSample { map_obj, maxsize, player } = *sample;
    let (maxsize_x, maxsize_y) = maxsize;
    let (player_x, player_y, player_dx, player_dy) = (player.x, player.y, player.dx, player.dy);

    let player_angle = compass_deg(player_dx, player_dy);

    let mut best_airfield: Option<(f64, f64)> = None;
    let mut best_bombing: Option<(f64, f64)> = None;
    let mut best_poi: Option<(f64, f64)> = None;
    let mut best_poi_pos: Option<(f64, f64)> = None;
    let mut best_poi_neighbor: Option<(f64, f64, f64, f64, f64)> = None;

    if let Some(ref m_obj) = map_obj {
        for item in &m_obj.objects {
            let obj_coords = match item.obj_type.as_str() {
                "airfield" => extract_airfield_origin_coords(item),
                _ => extract_point_coords(item),
            };
            let Some(coords) = obj_coords else { continue };

            let to_player = calc_weighted_distance(player_x, player_y, coords.x, coords.y, maxsize_x, maxsize_y);

            match item.obj_type.as_str() {
                "airfield" => {
                    if best_airfield.map_or(true, |(d, _)| to_player < d) {
                        let bearing = target_bearing_deg(player_x, player_y, coords.x, coords.y);
                        best_airfield = Some((to_player, normalize_angle_deg(bearing - player_angle)));
                    }
                },
                "bombing_point" => {
                    if best_bombing.map_or(true, |(d, _)| to_player < d) {
                        let bearing = target_bearing_deg(player_x, player_y, coords.x, coords.y);
                        best_bombing = Some((to_player, normalize_angle_deg(bearing - player_angle)));
                    }
                },
                "point_of_interest" => {
                    if best_poi.map_or(true, |(d, _)| to_player < d) {
                        let bearing = target_bearing_deg(player_x, player_y, coords.x, coords.y);
                        best_poi = Some((to_player, normalize_angle_deg(bearing - player_angle)));
                        best_poi_pos = Some((coords.x, coords.y));
                    }
                },
                _ => {}
            }

            if let Some((px, py)) = best_poi_pos {
                let to_poi = calc_weighted_distance(px, py, coords.x, coords.y, maxsize_x, maxsize_y);
                if to_poi > 0.0 && best_poi_neighbor.map_or(true, |(d, ..)| to_poi < d) {
                    best_poi_neighbor = Some((to_poi, coords.x, coords.y, px, py));
                }
            }
        }
    }

    let (nearest_airfield_dist, nearest_airfield_bearing) =
        best_airfield.unwrap_or((f64::MAX, 0.0));
    let (nearest_bombing_dist, nearest_bombing_bearing) =
        best_bombing.unwrap_or((f64::MAX, 0.0));
    let (nearest_poi_dist, nearest_poi_bearing) = best_poi.unwrap_or((f64::MAX, 0.0));

    map.nearest_airfield_dist = nearest_airfield_dist;
    map.nearest_bombing_dist = nearest_bombing_dist;
    map.nearest_bombing_bearing = nearest_bombing_bearing;
    map.nearest_poi_dist = nearest_poi_dist;
    map.nearest_poi_bearing = nearest_poi_bearing;
    if nearest_poi_dist < f64::MAX && ctx.prev_poi_dist < f64::MAX && dt_ns > 0 {
        let dt_s = dt_ns as f64 / 1_000_000_000.0;
        let raw_speed = (ctx.prev_poi_dist - nearest_poi_dist) / dt_s;
        map.nearest_poi_rel_speed = ctx.poi_rel_speed_sma.add(raw_speed);
    } else {
        map.nearest_poi_rel_speed = 0.0;
    }
    ctx.prev_poi_dist = nearest_poi_dist;

    if let Some((d, nx, ny, px, py)) = best_poi_neighbor {
        map.nearest_poi_neighbor_dist = d;
        map.nearest_poi_neighbor_x = nx;
        map.nearest_poi_neighbor_y = ny;
        map.nearest_poi_x = px;
        map.nearest_poi_y = py;
    } else {
        map.nearest_poi_neighbor_dist = f64::MAX;
    }

    map.player_map_x = player_x;
    map.player_map_y = player_y;

    nearest_airfield_bearing
}

fn format_map_text(map: &mut MapDisplay, map_info: &crate::parser::MapInfo, nearest_airfield_bearing: f64, player_x: f64, player_y: f64) {
    write_format_dist(&mut map.nearest_airfield_text, "AF", map.nearest_airfield_dist, nearest_airfield_bearing);
    write_format_dist(&mut map.nearest_bombing_point_text, "BP", map.nearest_bombing_dist, map.nearest_bombing_bearing);
    write_format_dist(&mut map.nearest_poi_text, "POI", map.nearest_poi_dist, map.nearest_poi_bearing);
    set_text_str(&mut map.map_zone_text, &crate::math::map::calc_map_zone(player_x, player_y, map_info));
}

fn format_display_text(display_data: &mut DisplayData, ctx: &FlightContext, tick: &Tick) {
    let eh = display_data.energy_height;
    // `eh == 0`（或 NaN）时 `log10()` = -inf，`as i32` 饱和成 `i32::MIN`，`2 - MIN` 在 debug 下
    // 整数下溢直接 panic（release 静默回绕 → 打出 "E 0" / "E inf"）。能量高度为 0 只在
    // 退化输入下出现，但格式化没必要为此炸掉整个更新器。
    let eh_decade = if eh.is_finite() && eh.abs() > f64::MIN_POSITIVE {
        eh.abs().log10().floor() as i32
    } else {
        0
    };
    let eh_scale = 10.0_f64.powi(2 - eh_decade);
    let eh_rounded = (eh * eh_scale).round() / eh_scale;
    let eh_prec = (2 - eh_decade).max(0) as usize;
    set_text(&mut display_data.energy_height_text, format_args!("E {:.eh_prec$}", eh_rounded, eh_prec = eh_prec));
    set_text(&mut display_data.altitude_text, format_args!("{:.0}", display_data.altitude));
    set_text(&mut display_data.tas_text, format_args!("{:.0}", display_data.tas));
    set_text(&mut display_data.ias_text, format_args!("{:.0}", display_data.ias));
    set_text(&mut display_data.mach_text, format_args!("{:.2}", display_data.mach));
    set_text(&mut display_data.vy_text, format_args!("{:+.1}", display_data.vy));
    set_text(&mut display_data.sep_text, format_args!("{:+.0}", round_sep(display_data.sep)));
    if display_data.turn_rate < 999.0 {
        set_text(&mut display_data.turn_rate_text, format_args!("{:.1}", display_data.turn_rate));
    } else {
        set_text_str(&mut display_data.turn_rate_text, "---");
    }
    if display_data.turn_radius < 99999.0 {
        set_text(&mut display_data.turn_radius_text, format_args!("{:.0}", display_data.turn_radius));
    } else {
        set_text_str(&mut display_data.turn_radius_text, "-");
    }
    set_text(&mut display_data.fuel_kg_text, format_args!("{:.0}", display_data.fuel_kg));
    set_text(&mut display_data.fuel_percent_text, format_args!("{:.0}", display_data.fuel_percent));
    set_time_or_dash(&mut display_data.fuel_time_text, display_data.fuel_time, 0.0, INFINITE_FUEL_TIME);
    if display_data.has_fuel1 {
        set_text(&mut display_data.fuel1_kg_text, format_args!("{:.0}", display_data.fuel1_kg));
    } else {
        set_text_str(&mut display_data.fuel1_kg_text, "-");
    }
    set_time_or_dash(&mut display_data.fuel1_time_text, display_data.fuel1_time, 0.0, INFINITE_FUEL_TIME);

    // SFC text（小数位数按量级自适应）
    if display_data.fuel_sfc > 0.0 {
        set_text_str(&mut display_data.fuel_sfc_text, &format_adaptive(display_data.fuel_sfc, 3));
    } else {
        set_text_str(&mut display_data.fuel_sfc_text, "-");
    }
    if display_data.fuel1_sfc > 0.0 {
        set_text_str(&mut display_data.fuel1_sfc_text, &format_adaptive(display_data.fuel1_sfc, 3));
    } else {
        set_text_str(&mut display_data.fuel1_sfc_text, "-");
    }

    // 飞行时长 = 帧数 × 标称节拍（`refresh_hz`），不用每帧实测的 interval（会累积抖动）
    let elapsed_secs = if tick.frame > 0 {
        tick.frame as f64 * tick.nominal_interval_ns as f64 / NSEC_PER_SEC
    } else {
        0.0
    };
    set_text_str(&mut display_data.elapsed_time_text, &format_duration(elapsed_secs));

    if ctx.current_max_rpm > 0.0 {
        set_text(&mut display_data.engine_rpm_text, format_args!("{:.0}", ctx.current_max_rpm));
    } else {
        set_text_str(&mut display_data.engine_rpm_text, "0");
    }
    set_text_str(&mut display_data.engine_power_text, &format_sig_figs(display_data.total_hp, 3));
    let eff = display_data.engine_efficiency as i32;
    if eff > 0 && !ctx.is_jet {
        set_text(&mut display_data.power_unit_text, format_args!("Hp/{}%", eff));
    } else {
        display_data.power_unit_text.clear();
    }
    let fm = ctx.flight_model();
    if fm.rpm_max_allowed < f64::MAX {
        set_text(&mut display_data.engine_rpm_unit, format_args!("/{}", fm.rpm_max_allowed as u32));
    } else {
        display_data.engine_rpm_unit.clear();
    }
    let rated_power = if !fm.is_jet { ctx.rated_power } else { 0.0 };
    if rated_power > 0.0 {
        set_text(&mut display_data.rated_power_text, format_args!("{:.0}", rated_power));
    } else {
        set_text_str(&mut display_data.rated_power_text, "-");
    }
    set_text(&mut display_data.thrust_to_weight_text, format_args!("{:.2}", display_data.thrust_to_weight));
    set_text(&mut display_data.power_to_weight_text, format_args!("{:.2}", display_data.power_to_weight));
    if display_data.engine_count > 0 {
        set_text(&mut display_data.engine_throttle_text, format_args!("{:.0}", display_data.engine_throttle[0]));
    } else {
        set_text_str(&mut display_data.engine_throttle_text, "0");
    }
    debug_assert!(display_data.engine_count > 0);
    if display_data.engine_temp_water[0] > 0.0 {
        set_text(&mut display_data.engine_temp_water_text, format_args!("{:.0}", display_data.engine_temp_water[0]));
    } else {
        set_text_str(&mut display_data.engine_temp_water_text, "-");
    }
    if display_data.engine_temp_oil[0] > 0.0 {
        set_text(&mut display_data.engine_temp_oil_text, format_args!("{:.0}", display_data.engine_temp_oil[0]));
    } else {
        set_text_str(&mut display_data.engine_temp_oil_text, "-");
    }
    set_text_str(&mut display_data.total_thrust_text, &format_sig_figs(display_data.total_thrust, 3));
    set_text(&mut display_data.thrust_percent_text, format_args!("{:.0}", display_data.thrust_percent));
    set_text(&mut display_data.roll_rate_text, format_args!("{:>+4.1}", display_data.wx));
    set_text(&mut display_data.pitch_text, format_args!("{:>+4.1}", -display_data.pitch));
    set_text(&mut display_data.aoa_format_text, format_args!("{:>+4.1}", display_data.aoa));
    set_text(&mut display_data.g_load_text, format_args!("{:>+4.1}", display_data.ny));

    // 过载单位栏：按当前重量实时换算允许正过载；|Ny| ≥ 90% 允许过载时告警（红）
    // （flight_weight = 空机+滑油+弹药+对抗措施+当前燃油）
    let fm = ctx.flight_model();
    let weight = fm.flight_weight(display_data.fuel_kg + display_data.fuel1_kg);
    let (neg_lim, pos_lim) = fm.allowed_load_factor(weight);
    display_data.g_load_warning = g_load_warning_level(display_data.ny, neg_lim, pos_lim);
    // 机动告警参数：MAX(攻角比, 过载比, 超速项)，含负向（各按自身限值）。
    // 超速比 = MAX(表速/VNE, 马赫/MNE)，VNE/MNE 随后掠取值（与面板显示同源），
    // 限值缺失（≤0）时该向不参与；经 speed_margin 线性映射（95%→起音阈值、
    // 100%→长鸣阈值，连续线不硬切）后并入取最大。
    let r_ias = if display_data.vne > 0.0 {
        display_data.ias / display_data.vne
    } else {
        0.0
    };
    let r_mach = if display_data.vne_mach > 0.0 {
        display_data.mach / display_data.vne_mach
    } else {
        0.0
    };
    display_data.maneuver_margin = crate::maneuver_tone::maneuver_margin(
        display_data.aoa,
        display_data.max_aoa,
        display_data.min_aoa,
        display_data.ny,
        pos_lim,
        neg_lim,
        r_ias.max(r_mach),
    );
    if pos_lim > 0.0 {
        // 与表速格 "/1477"（VNE）的单位栏风格一致
        set_text(&mut display_data.g_limit_text, format_args!("/{:.1}", pos_lim));
    } else {
        set_text_str(&mut display_data.g_limit_text, "-");
    }

    // Manifold pressure
    if display_data.manifold_pressure > 0.0 {
        write_manifold_pressure_value(&mut display_data.manifold_pressure_text, display_data.manifold_pressure, ctx.is_imperial);
        write_manifold_pressure_unit(&mut display_data.manifold_pressure_unit, display_data.manifold_pressure, ctx.is_imperial);
    } else {
        display_data.manifold_pressure_text.clear();
        display_data.manifold_pressure_unit.clear();
    }

    // Overheat
    let fmt_remain = |field: &mut FixedBytes<16>, secs: f64| {
        if secs < 3600.0 {
            let total_secs = secs as u64;
            let m = total_secs / 60;
            let s = total_secs % 60;
            set_text(field, format_args!("℃/{}'{:02}", m, (s / 10) * 10));
        } else {
            set_text_str(field, "℃/-");
        }
    };
    fmt_remain(&mut display_data.water_crit_temp_text, ctx.water_remain_secs);
    fmt_remain(&mut display_data.oil_crit_temp_text, ctx.oil_remain_secs);

    // Aircraft type
    set_text_str(&mut display_data.aircraft_type, ctx.aircraft_type.as_str());

    // Drag text
    set_text_str(&mut display_data.total_drag_text, &format_sig_figs(display_data.total_drag, 2));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warning_stage_two_stage_thresholds() {
        let vne = 1000.0;
        assert_eq!(warning_stage(900.0, vne), 0, "below the 95% line stays normal");
        assert_eq!(warning_stage(949.9, vne), 0, "just below 95% stays normal");
        assert_eq!(warning_stage(950.0, vne), 1, "95% line turns yellow (stage 1)");
        assert_eq!(warning_stage(989.9, vne), 1, "below 99% stays yellow");
        assert_eq!(warning_stage(990.0, vne), 2, "99% line turns red (stage 2)");
        assert_eq!(warning_stage(1200.0, vne), 2, "far above the limit stays red");
    }

    #[test]
    fn warning_stage_handles_missing_limit() {
        assert_eq!(warning_stage(500.0, 0.0), 0);
        assert_eq!(warning_stage(500.0, -1.0), 0);
        assert_eq!(warning_stage(f64::NAN, 1000.0), 0);
    }

    #[test]
    fn warning_stage_mach_uses_mne() {
        let mne = 2.0;
        assert_eq!(warning_stage(1.89, mne), 0);
        assert_eq!(warning_stage(1.9, mne), 1);
        assert_eq!(warning_stage(1.97, mne), 1);
        assert_eq!(warning_stage(1.98, mne), 2);
    }

    #[test]
    fn g_load_warning_level_at_90_percent() {
        // 正向：90%×14 = 12.6 起告警
        assert_eq!(g_load_warning_level(12.5, -8.0, 14.0), 0);
        assert_eq!(g_load_warning_level(12.6, -8.0, 14.0), 1);
        assert_eq!(g_load_warning_level(13.5, -8.0, 14.0), 1);
        // 负向：按负限值 |−8|×0.9 = 7.2 起告警
        assert_eq!(g_load_warning_level(-7.0, -8.0, 14.0), 0);
        assert_eq!(g_load_warning_level(-7.2, -8.0, 14.0), 1);
        // 无限制数据不告警
        assert_eq!(g_load_warning_level(20.0, 0.0, 0.0), 0);
    }

    #[test]
    fn velocity_turn_rate_matches_classic_formulas() {
        let g = GRAVITY;
        let v = 200.0;
        // 水平 45° 坡度协调转弯（ny = 1/cos45°）：ω = g·tanφ / V ≈ 2.81°/s
        let ny_bank = std::f64::consts::FRAC_1_SQRT_2.recip();
        let bank = velocity_turn_rate_deg(0.0, 0.0, 45.0, 5.0, 0.0, ny_bank, v);
        let ideal = (g / v).to_degrees(); // tan(45°) = 1
        assert!((bank - ideal).abs() < 0.1, "45° 坡度转弯 {:.3} vs g·tanφ/V {:.3}", bank, ideal);
        // 正拉杆（ny=9，平飞出发）：ω = (n−1)·g / V（约 22.5°/s）
        let pull = velocity_turn_rate_deg(0.0, 0.0, 0.0, 5.0, 0.0, 9.0, v);
        let ideal_pull = (8.0 * g / v).to_degrees();
        assert!((pull - ideal_pull).abs() < 0.5, "拉杆 {:.3} vs (n−1)g/V {:.3}", pull, ideal_pull);
        // 平飞 ny=1：无转动
        let level = velocity_turn_rate_deg(0.0, 0.0, 0.0, 5.0, 0.0, 1.0, v);
        assert!(level < 0.2, "平飞不应有转弯率，got {:.3}", level);
    }

    #[test]
    fn edge_armed_rise_fall_and_hold() {
        let mut s = EdgeArmed::default();
        assert!(!s.update(0.0), "初始静默");
        assert!(s.update(50.0), "上升沿 → 告警");
        assert!(s.update(100.0), "继续上升 → 告警");
        assert!(s.update(100.0), "同值帧 → 保持告警");
        assert!(!s.update(90.0), "下降沿（相比前值减少）→ 立即清除");
        assert!(!s.update(80.0), "持续下降 → 保持清除");
        assert!(!s.update(80.0), "同值帧 → 保持清除（收回中途的停顿不重新告警）");
        assert!(!s.update(79.5), "再减 0.5 也算下降沿（无防抖死区）");
    }

    #[test]
    fn edge_armed_staircase_retraction_stays_off() {
        // 遥测阶梯（帧率 > 数据率）：收回中途大量同值帧，全程不得告警 ——
        // 这正是"收回状态不告警"的关键场景。
        let mut s = EdgeArmed::default();
        assert!(s.update(100.0), "放出");
        for v in [90.0, 90.0, 90.0, 80.0, 80.0, 70.0, 70.0, 70.0, 0.0, 0.0] {
            assert!(!s.update(v), "收回阶梯 {v} 不应告警");
        }
        assert!(s.update(5.0), "重新放出 → 告警");
    }

    #[test]
    fn edge_armed_full_retract_and_redeploy() {
        let mut s = EdgeArmed::default();
        assert!(!s.update(0.0));
        assert!(s.update(100.0), "放出");
        assert!(!s.update(0.0), "一步收到 0 → 清除");
        assert!(!s.update(0.0), "静止在 0 → 保持清除");
        assert!(s.update(1.0), "重新放出 → 告警");
    }

    /// 固定的 10 Hz 节拍（标称 = 实测），时间戳与帧号同源 —— 更新器不再自己读墙钟。
    fn tick(frame: u64) -> Tick {
        const STEP: u64 = 100_000_000;
        Tick {
            frame,
            timestamp_ns: 1_700_000_000_000_000_000 + frame * STEP,
            interval_ns: STEP,
            nominal_interval_ns: STEP,
            mapobj_interval_ns: 0,
            dropped: false,
        }
    }

    /// 没有机型数据（`Mfuel0` 缺失 → `state.fuel0 = 0`）时油量百分比写 0，不是 NaN
    /// （NaN 会一路进 HUD 与 `.wpr`）。
    #[test]
    fn missing_fuel_capacity_writes_zero_percent_not_nan() {
        let mut state = FlightState::default();
        state.engine_count = 1;
        state.fuel = 1200.0;
        state.fuel0 = 0.0;
        let indic = Indicators::default();
        let mut ctx = FlightContext::new();
        let mut d = DisplayData::default();
        update_display_from_state(&state, &indic, &mut d, &mut ctx, &tick(1));
        assert_eq!(d.fuel_percent, 0.0);
        assert!(d.fuel_percent.is_finite());
    }

    /// 记录起始判据靠 `DisplayData.frame`（帧号），所以帧号与时间戳必须逐帧原样写进
    /// DisplayData 且严格递增。
    #[test]
    fn frame_number_is_forwarded_and_increases() {
        // 更新器要求至少一台发动机（`state.engine_count > 0`），其余字段默认即可
        let mut state = FlightState::default();
        state.engine_count = 1;
        state.fuel0 = 3000.0;
        state.fuel = 3000.0;
        let indic = Indicators::default();
        let mut ctx = FlightContext::new();
        let mut d = DisplayData::default();
        let mut last = 0u64;
        for frame in [1u64, 2, 8, 9, 10_000] {
            let t = tick(frame);
            update_display_from_state(&state, &indic, &mut d, &mut ctx, &t);
            assert_eq!(d.frame, t.frame, "frame 必须原样写进 DisplayData");
            assert_eq!(d.timestamp, t.timestamp_ns, "时间戳必须与本拍同一基准");
            assert!(d.frame > last, "帧号必须递增（记录起点判据依赖它）：{last} → {}", d.frame);
            last = d.frame;
        }
    }
}
