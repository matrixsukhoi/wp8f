use std::net::Ipv4Addr;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};
use tracing_subscriber;
use wp8f_core::warnings::VoiceWarning;
use std::path::Path;
use wp8f_core::{
    channel::async_channel::AsyncChannel, channel::Channel, format_adaptive,
    frame_clock::FrameClock, get_timestamp_ns, parser, update_display_from_state,
    update_map_display, DisplayData, FlightContext, FlightState, Indicators, MapInfo, MapObjData,
    GRAVITY,
};
use wp8f_disp;
use wp8f_flightmodel;
use wp8f_logger::{spawn_recorder, RecordConfig, RecordHandle};
use clap::Parser;

#[derive(Debug, Clone, Copy)]
pub enum ExitReason {
    StateInvalid,
    ReadStateFailed,
}

impl std::fmt::Display for ExitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExitReason::StateInvalid => write!(f, "State invalid"),
            ExitReason::ReadStateFailed => write!(f, "Read state failed"),
        }
    }
}

fn has_engine_activity(state: &wp8f_core::parser::FlightState) -> bool {
    state.engine_count > 0 && state.engines[..state.engine_count].iter().any(|e| e.thrust > 0.0 && e.rpm > 0.0)
}

async fn wait_until(ts_next: u64) {
    let now = get_timestamp_ns();
    tokio::time::sleep(Duration::from_nanos(ts_next.saturating_sub(now))).await;
}

fn try_connect(channel: &mut Channel) -> bool {
    eprintln!("Waiting for engine activity...");
    let mut state = FlightState::default();
    let mut indic = Indicators::default();
    let mut state_buf = Vec::with_capacity(4096);
    let mut indic_buf = Vec::with_capacity(4096);
    loop {

        let state_valid = channel.read_state(&mut state_buf)
            .ok()
            .and_then(|()| {
                let state_str = String::from_utf8_lossy(&state_buf);
                parser::parse_state(&mut state, &state_str).ok()
            })
            .map(|()| state.valid && has_engine_activity(&state))
            .unwrap_or(false);

        if state_valid {
            let has_activity = channel.read_indicators(&mut indic_buf)
                .ok()
                .and_then(|()| {
                    let indic_str = String::from_utf8_lossy(&indic_buf);
                    parser::parse_indicators(&mut indic, &indic_str).ok()
                })
                .map(|()| {
                    if indic.army.as_str() != "air" {
                        false
                    } else if indic.aircraft_type.as_str() == "dummyplane" {
                        false
                    } else {
                        true
                    }
                })
                .unwrap_or(true);

            if has_activity {
                return true;
            }
        }

        std::thread::sleep(Duration::from_millis(500));
    }
}

fn pre_process(
    channel: &mut Channel,
    fm_data_dir: &str,
    map_obj_frames: u64,
) -> (FlightContext, DisplayData) {
    // 帧率与地图对象采样周期都来自配置；采样周期由调用方算好（`1..=refresh_hz` 已夹紧），
    // 记录起点的 `start_frame` 用的是同一个值 —— 三处口径必须一致。
    let mut ctx = FlightContext::with_map_obj_frames(map_obj_frames);
    let mut display_data = DisplayData::default();

    let mut map_info_buf = Vec::new();
    let mut map_info = MapInfo::default();
    match channel.read_map_info(&mut map_info_buf) {
        Ok(()) => {
            let s = String::from_utf8_lossy(&map_info_buf);
            if parser::parse_map_info(&mut map_info, &s).is_ok() {
                if map_info.valid {
                    println!("MapInfo: valid={}, hud_type={}, map_generation={}, grid_size=[{:.0}, {:.0}], grid_steps=[{:.0}, {:.0}], grid_zero=[{:.0}, {:.0}], map_max=[{:.0}, {:.0}], map_min=[{:.0}, {:.0}], maxsize=[{:.0}, {:.0}]",
                        map_info.valid, map_info.hud_type, map_info.map_generation,
                        map_info.grid_size[0], map_info.grid_size[1],
                        map_info.grid_steps[0], map_info.grid_steps[1],
                        map_info.grid_zero[0], map_info.grid_zero[1],
                        map_info.map_max[0], map_info.map_max[1],
                        map_info.map_min[0], map_info.map_min[1],
                        map_info.maxsize[0], map_info.maxsize[1]);
                    ctx.map_info = Some(map_info.clone());
                }
            }
        }
        Err(_e) => {}
    }

    let mut map_img_buf = Vec::new();
    match channel.read_map_img(&mut map_img_buf) {
        Ok(()) if !map_img_buf.is_empty() => {
            if let Ok(img) = image::load_from_memory(&map_img_buf) {
                let rgb = img.to_rgb8();
                let (w, h) = rgb.dimensions();
                let rgb_vec: Vec<u8> = rgb.into_raw();
                let map_img_arc = std::sync::Arc::new(rgb_vec);
                // 原始字节（JPEG）也留一份：飞行记录要把原图直接塞进 .wpr（解码再编码会失真且更大）
                let raw_arc = std::sync::Arc::new(map_img_buf.clone());
                // 底图**一局写一次**（三份一起写：解码 RGB / 原始字节 / 像素尺寸）——
                // 它不在每采样的地图环形槽里，所以这里写完就够，不必每次发布重写。
                wp8f_disp::set_map_image(
                    std::sync::Arc::clone(&map_img_arc),
                    std::sync::Arc::clone(&raw_arc),
                    w,
                    h,
                );
                ctx.map_img_w = w;
                ctx.map_img_h = h;
            }
        }
        Ok(()) => {
            eprintln!("[DEBUG] map_img returned OK but empty body");
        }
        Err(e) => {
            eprintln!("[DEBUG] map_img error: {:?}", e);
        }
    }

    let mut pre_indic = Indicators::default();
    let mut indic_buf = Vec::with_capacity(4096);
    if channel.read_indicators(&mut indic_buf).is_ok() {
        let indic_str = String::from_utf8_lossy(&indic_buf);
        if parser::parse_indicators(&mut pre_indic, &indic_str).is_ok() {
            display_data.aircraft_type.assign(pre_indic.aircraft_type.as_bytes());
            if pre_indic.army.as_str() == "air" && !pre_indic.aircraft_type.is_empty() {
                let blkx_path = format!("{}/{}.blkx", fm_data_dir, pre_indic.aircraft_type.as_str());
                println!("Loading FM from: {}", blkx_path);
                println!(
                    "  （飞机性能数据是本地数据、不在 git 里；默认应放在 ./resource/data/，可用 --data-dir 覆盖）"
                );
                let fm = wp8f_flightmodel::parse_aircraft_or_default(pre_indic.aircraft_type.as_str(), fm_data_dir);
                ctx.load_flight_model(&fm);
                print_fm_report(&fm, &ctx);
            }
        }
    }

    (ctx, display_data)
}


/// 启动时把机型数据打成一份可读报告（只打印，不改任何状态）。
fn print_fm_report(fm: &wp8f_flightmodel::FlightModel, ctx: &FlightContext) {
    let wep_display = if fm.is_jet {
        format!("wep_thrust_max={:.0}kgf", ctx.wep_thrust_max)
    } else {
        format!("wep_power_max={:.0}hp", fm.wep_power_max())
    };
    println!(
        "FM loaded: {}  empty_weight={:.0}kg, max_fuel={:.0}kg, {}, vne={:.0}km/h, aoa_no_flaps={:.1}, aoa_full_flaps={:.1}",
        fm.fm_path, fm.empty_weight(), fm.max_fuel(), wep_display, ctx.vne, ctx.no_flaps_aoa, ctx.full_flaps_aoa
    );
    println!(
        "Weight: empty={:.0}  oil={:.0}  nitro={:.0}  pilot={:.0}  ammo={:.0}  countermeasure={:.0}  load={:.0}  empty_flight={:.0}",
        fm.empty_weight(), fm.oil_mass(), fm.max_nitro(), fm.pilot_mass(),
        fm.cannon_ammo_weight(), fm.countermeasure_weight(), fm.external_weapons_weight(),
        fm.flight_weight(0.0),
    );
    if !fm.is_jet {
        let has_wep = wp8f_flightmodel::has_any_wep(fm.engines());
        let pw = |thr| wp8f_flightmodel::powers_at(fm.engines(), 0.0, 0.0, thr);
        let p_mil = pw(100.0);
        if has_wep {
            let p_wep = pw(110.0);
            println!("Power: Mil={:.0}hp, T/W={:.2}  WEP={:.0}hp, T/W={:.2}", p_mil, p_mil / fm.empty_weight(), p_wep, p_wep / fm.empty_weight());
        } else {
            println!("Power: Mil={:.0}hp, T/W={:.2}", p_mil, p_mil / fm.empty_weight());
        }
    }
    let (neg_empty, pos_empty) = fm.limit_load_factor_range(fm.empty_weight());
    let (neg_full, pos_full) = fm.limit_load_factor_range(fm.flight_weight(fm.max_fuel()));
    println!(
        "Load factor: [{:.1}, {:.1}]g / [{:.1}, {:.1}]g",
        neg_empty, pos_empty, neg_full, pos_full
    );
    fm.print_weapon_info();
    // Per-part static FM parameters (voidmei style)
    let print_polar = |label: &str, p: &wp8f_flightmodel::aero::PolarData| {
        println!("    {}  CdMin:{:.4} Cl0:{:.4} AoACrit:[{:.1},{:.1}] ClCrit:[{:.3},{:.3}] ClSlope:{:.4} Oswalds:{:.3}",
            label, p.cd_min, p.cl0, p.alpha_crit_low, p.alpha_crit_high, p.cl_crit_low, p.cl_crit_high, p.cl_slope(), p.oswalds_efficiency);
    };
    let print_best_ld = |label: &str, p: &wp8f_flightmodel::aero::PolarData, ar: f64| {
        if let Some((alpha, cl, cd, ld)) = p.best_ld(ar) {
            println!("    {}  Best L/D: α={:.1}°  Cl={:.2}  Cd={:.3}  L/D={:.1}", label, alpha, cl, cd, ld);
        }
        let (cl_neg, cd_neg) = p.cl_cd_at(p.alpha_crit_low, ar);
        let ld_neg = if cd_neg > 0.0 { cl_neg / cd_neg } else { 0.0 };
        let (cl_pos, cd_pos) = p.cl_cd_at(p.alpha_crit_high, ar);
        let ld_pos = if cd_pos > 0.0 { cl_pos / cd_pos } else { 0.0 };
        println!("    {}  Crit AoA(-): α={:.1}°  Cl={:.2}  Cd={:.3}  L/D={:.1}", label, p.alpha_crit_low, cl_neg, cd_neg, ld_neg);
        println!("    {}  Crit AoA(+): α={:.1}°  Cl={:.2}  Cd={:.3}  L/D={:.1}", label, p.alpha_crit_high, cl_pos, cd_pos, ld_pos);
    };
    let print_part = |name: &str, part: &wp8f_flightmodel::aero::AeroSurface| {
        let ar = if part.area > 0.0 { part.span * part.span / part.area } else { 0.0 };
        println!("  === {} [Angle:{:.1}° Area:{:.2}m\u{00B2} AR:{:.2}] ===", name, part.angle, part.area, ar);
        print_polar("(base)", &part.polar);
        print_best_ld("(base)", &part.polar, ar);
        if let Some(ref p) = part.flaps_polar_1 {
            print_polar("(FullFlaps)", p);
            print_best_ld("(FullFlaps)", p, ar);
        }
    };
    let print_wing_geo = |label: &str, wg: &wp8f_flightmodel::aero::WingGeometry| {
        let ar = if wg.wing_area > 0.0 { wg.span * wg.span / wg.wing_area } else { 0.0 };
        println!("    --- WingGeo {} (sweep:{:.0}% AR:{:.2}) ---", label, wg.sweep_percent * 100.0, ar);
        print_polar("(NoFlaps)", &wg.no_flaps_polar);
        print_best_ld("(NoFlaps)", &wg.no_flaps_polar, ar);
        if let Some(ref p) = wg.full_flaps_polar {
            print_polar("(FullFlaps)", p);
            print_best_ld("(FullFlaps)", p, ar);
        }
        if let Some(ref p) = wg.high_flaps_polar {
            print_polar("(HighFlaps)", p);
            print_best_ld("(HighFlaps)", p, ar);
        }
    };
    if let Some(ref w) = fm.wing {
        print_part("Wing", w);
    }
    if let Some(ref f) = fm.fuselage {
        print_part("Fuselage", f);
    }
    if let Some(ref s) = fm.hor_stab {
        print_part("HorStab", s);
    }
    // Variable sweep positions
    if fm.aerodynamics.wing_geometry.is_variable_sweep() {
        println!("  === Variable Sweep Wing ===");
        print_wing_geo("Sweep0", &fm.aerodynamics.wing_geometry.wing_sweep0);
        if let Some(ref wg) = fm.aerodynamics.wing_geometry.wing_sweep1 {
            print_wing_geo("Sweep1", wg);
        }
        if let Some(ref wg) = fm.aerodynamics.wing_geometry.wing_sweep2 {
            print_wing_geo("Sweep2", wg);
        }
        if let Some(ref wg) = fm.aerodynamics.wing_geometry.wing_sweep3 {
            print_wing_geo("Sweep3", wg);
        }
    }
    // Fuel consumption coefficients (jet only)
    if fm.is_jet {
        let fmt_sig = |v: f64| if v == 0.0 { "0".to_string() } else { format_adaptive(v, 3) };
        println!("Fuel Consumption Coefficient ((kg/h)/kgf): Mil={}  WEP={}",
            fmt_sig(fm.fuel_consumption_coefficient(false)),
            fmt_sig(fm.fuel_consumption_coefficient(true)));
        let speeds = [0, 300, 600, 900, 1200, 1500];
        let full_weight = fm.flight_weight(fm.max_fuel());
        let fuel_5min_800_102 = wp8f_flightmodel::fuel_rate_at(&fm.engines(), 800.0, 0.0, 102.0) * 300.0;
        let fuel_5min_800_105 = wp8f_flightmodel::fuel_rate_at(&fm.engines(), 800.0, 0.0, 105.0) * 300.0;
        let fuel_5min_800_108 = wp8f_flightmodel::fuel_rate_at(&fm.engines(), 800.0, 0.0, 108.0) * 300.0;
        let weight_5min_102 = fm.flight_weight(fuel_5min_800_102);
        let weight_5min_105 = fm.flight_weight(fuel_5min_800_105);
        let weight_5min_108 = fm.flight_weight(fuel_5min_800_108);
        println!("T/W @ sea level:");
        for speed in speeds {
            let thrust_mil = fm.thrust(0.0, speed as f64 / 3.6) / GRAVITY;
            let thrust_wep = fm.afterburner_thrust(0.0, speed as f64 / 3.6) / GRAVITY;
            let thrust_102 = wp8f_flightmodel::thrusts_at(&fm.engines(), speed as f64, 0.0, 102.0);
            let thrust_105 = wp8f_flightmodel::thrusts_at(&fm.engines(), speed as f64, 0.0, 105.0);
            let thrust_108 = wp8f_flightmodel::thrusts_at(&fm.engines(), speed as f64, 0.0, 108.0);
            let fuel_5min_wep = fm.get_fuel_rate_at(speed as f64, 0.0, true) * 300.0;
            let weight_5min = fm.flight_weight(fuel_5min_wep);
            let tw_mil_empty = if fm.empty_weight() > 0.0 { thrust_mil / fm.empty_weight() } else { 0.0 };
            let tw_wep_empty = if fm.empty_weight() > 0.0 { thrust_wep / fm.empty_weight() } else { 0.0 };
            let tw_mil_full = if full_weight > 0.0 { thrust_mil / full_weight } else { 0.0 };
            let tw_wep_full = if full_weight > 0.0 { thrust_wep / full_weight } else { 0.0 };
            let tw_wep_5min = if weight_5min > 0.0 { thrust_wep / weight_5min } else { 0.0 };
            let tw_102_5min = if weight_5min_102 > 0.0 { thrust_102 / weight_5min_102 } else { 0.0 };
            let tw_105_5min = if weight_5min_105 > 0.0 { thrust_105 / weight_5min_105 } else { 0.0 };
            let tw_108_5min = if weight_5min_108 > 0.0 { thrust_108 / weight_5min_108 } else { 0.0 };
            println!(
                "  {:4.0}km/h: WEP=[{:.2}, {:.2}] MW=[{:.2}, {:.2}] WEP5min={:.2} 108%5min={:.2} 105%5min={:.2} 102%5min={:.2}",
                speed as f64, tw_wep_empty, tw_wep_full, tw_mil_empty, tw_mil_full, tw_wep_5min, tw_108_5min, tw_105_5min, tw_102_5min
            );
        }
        let fuel_5min_800 = fm.get_fuel_rate_at(800.0, 0.0, true) * 300.0;
        println!("5min WEP fuel @ 800km/h: {:.0} kg", fuel_5min_800);
        println!("5min fuel @ 800km/h, 108% throttle: {:.0} kg", fuel_5min_800_108);
        println!("5min fuel @ 800km/h, 105% throttle: {:.0} kg", fuel_5min_800_105);
        println!("5min fuel @ 800km/h, 102% throttle: {:.0} kg", fuel_5min_800_102);
    }
    fm.print_flight_limits();
}

const INVALID_STATE_THRESHOLD: u64 = 4;

fn check_invalid(reason: ExitReason, msg: &str, count: &mut u64) -> Option<ExitReason> {
    *count += 1;
    if *count > INVALID_STATE_THRESHOLD {
        eprintln!("[WARN] {} count={}/{}", msg, *count, INVALID_STATE_THRESHOLD);
        Some(reason)
    } else {
        eprintln!("[WARN] {}, count={}/{}", msg, *count, INVALID_STATE_THRESHOLD);
        None
    }
}

/// 生效的地图记录间隔（数据帧）—— core 侧唯一取值点（主循环闸门、`FlightContext` 初值、
/// `elapsed` 换算、记录起点 `start_frame` 都用它）。
///
/// 1. 钳制：`1..=refresh_hz`（夹紧实现在 `wp8f_disp` 侧，与 `refresh_hz` 放一起）；
/// 2. 真被改动时打一行 `[MAPOBJ]`，顺带用 `wp8f_core::mapobj::period_ms` 报出毫秒口径。
///
/// 它同时是**记录线程的采样周期**（见 [`spawn_recorder_thread`]）：记录频率不开放配置，
/// 只跟随地图刷新 —— 地图坐标每这么多帧才更新一次，记更快只会得到重复坐标。
/// 每次连接算一次（配置是启动时读的，但重连后理论上可被控制台改写）。
fn effective_map_obj_frames() -> u64 {
    let cfg = wp8f_disp::hud_layout();
    let want = cfg.map_obj_record_every_frames;
    let frames = cfg.map_obj_record_every_frames_clamped();
    if frames != want {
        eprintln!(
            "[MAPOBJ] map_obj_record_every_frames={} exceeds the cap (data frames per second = {} @{} Hz); recording map objects every {} data frames ({} ms)",
            want,
            frames,
            cfg.refresh_hz_clamped(),
            frames,
            wp8f_core::mapobj::period_ms(frames, cfg.refresh_hz_clamped())
        );
    }
    frames
}

/// 创建一次连接的飞行记录线程 —— core 侧唯一的记录接口。
///
/// core 只做三件事：把布局配置的 `record` 段搬进 [`RecordConfig`]、算**记录采样周期**、
/// 把 `DisplayData` 环形缓冲区的消费端交出去。
/// 单位换算/经纬度/文件格式全在 `wpr` crate 与 GUI 转换器里，主循环里没有文件句柄、也没有写盘点。
///
/// **记录频率不开放配置**：地图坐标每 `map_obj_record_every_frames` 个数据帧才更新一次，
/// 记录得比它更快只会写下坐标完全相同的重复帧 —— 所以采样周期直接取地图刷新周期
/// （[`wp8f_core::mapobj::period_ms`]，8 帧 @30 Hz = 266 ms）。顺带保证它 ≥ 一个主循环节拍
/// （帧数下限是 1），不会出现"一轮询只取到同一帧"的空转。
///
/// `None` = 未启用，或窗口没建起来（`RING_BUFFER` 未初始化，此时 `begin_frame()` 也拿不到数据）。
fn spawn_recorder_thread(map_obj_frames: u64) -> Option<RecordHandle> {
    let cfg = wp8f_disp::hud_layout();
    let r = &cfg.record;
    let reader = wp8f_disp::frame_reader()?;
    // 记录采样周期 = 地图刷新周期（唯一换算入口；`max(1)` 兜住 refresh_hz=0 的极端配置）
    let poll_ms = wp8f_core::mapobj::period_ms(map_obj_frames, cfg.refresh_hz_clamped()).max(1);
    spawn_recorder(
        RecordConfig {
            enabled: r.enabled,
            output_dir: r.output_dir.clone(),
            poll_ms,
            pool_mb: r.pool_mb,
            // 记录起点 = 生效的地图对象采样周期：`pos_x/pos_y` 在此之前还是初值 (0.5,0.5)
            start_frame: map_obj_frames,
        },
        reader,
    )
}

/// 创建**进程级**的数据链线程 —— core 侧唯一的数据链接口。
///
/// core 只做两件事：把布局配置的 `datalink` 段搬进 `LinkConfig`，然后每帧
/// `link_ring::publish_own(...)` 一次（无锁、无分配，没有 I/O）。
/// socket 收发、限频计时、友军快照发布都在线程里。
///
/// 与 logger 一样是"一个进程一个线程"：`client_id` 跨重连不变，重连时只调
/// `link_ring::reset_tracks()` 清掉上一局的友军快照。
///
/// `None` = 配置未启用，或建 socket 失败（线程侧打一行日志，程序继续跑）。
fn spawn_link_thread(l8: &wp8f_disp::DataLinkConfig) -> Option<datalink::thread::LinkHandle> {
    datalink::thread::spawn_link(datalink::thread::LinkConfig {
        enabled: l8.enabled,
        server: l8.server.clone(),
        port: l8.port,
        key: l8.key.clone(),
        session_id: l8.session_id,
        send_hz: l8.send_hz,
        peer_timeout_secs: l8.peer_timeout_secs,
    })
}

// 热路径告警限流：每个站点一个时间戳（见 `wp8f_core::warn_throttled`）。
static WARN_INTERVAL0: AtomicU64 = AtomicU64::new(0);
static WARN_READALL: AtomicU64 = AtomicU64::new(0);
static WARN_MAPOBJ_PARSE: AtomicU64 = AtomicU64::new(0);
static WARN_INDIC_INVALID: AtomicU64 = AtomicU64::new(0);
static WARN_DROPPED: AtomicU64 = AtomicU64::new(0);

async fn data_process_loop(
    channel: &AsyncChannel,
    ctx: &mut FlightContext,
    map_obj_frames: u64,
    mut voice_warning: Option<VoiceWarning>,
) -> ExitReason {

    // F-18 风格机动告警声状态机（蜂鸣节拍/长鸣迟滞）
    let mut maneuver_tone = wp8f_core::maneuver_tone::ManeuverTone::new();
    // 开关读一次就定下来（配置运行期不变）：关掉后连状态机都不推进
    let maneuver_tone_enabled = wp8f_disp::hud_layout().maneuver_tone_enabled;

    // core 数据刷新频率来自布局配置（`refresh_hz`，默认 30，钳 5..=60）：
    // 主循环节拍、帧计数、SMA 窗口与"飞行时长"都用它换算（帧计数类常量按帧算，
    // 所以改了刷新率，它们的**时间口径一起变** —— 见 `HudLayoutConfig::refresh_hz` 注释）。
    let interval_ns_cfg = wp8f_disp::hud_layout().refresh_interval_ns();

    let ts_start = get_timestamp_ns();
    let mut clock = FrameClock::new(ts_start, interval_ns_cfg, map_obj_frames);
    let mut ts_next  = ts_start + interval_ns_cfg;

    let mut invalid_state_count: u64 = 0;

    // Link-8 数据链：主循环只维护"本机状态快照"（上报内容）——
    // 收发、限频、排空应答全在 datalink 线程里（`datalink::thread::spawn_link`）。
    let mut datalink_pos: (f64, f64) = (0.5, 0.5);
    // 本机 POI 同步：(启用, x, y)，归一化地图坐标（与 MapDisplay.nearest_poi_x/y 同约定）
    let mut datalink_poi: (bool, f64, f64) = (false, 0.0, 0.0);

    let mut state = FlightState::default();
    let mut indic = Indicators::default();
    let mut map_obj = MapObjData::default();

    wait_until(ts_next).await;

    loop {
        let tick = clock.peek(get_timestamp_ns());
        if tick.interval_ns == 0 {
            wp8f_core::warn_throttled(&WARN_INTERVAL0, ||
                format!("[WARN] interval_ns=0, frame={}, now={}", tick.frame, tick.timestamp_ns));
            wait_until(ts_next).await;
            continue;
        }
        clock.advance(&tick);

        let result = channel
            .read_all(&mut state, &mut indic, &mut map_obj, tick.mapobj_interval_ns > 0)
            .await;

        let parse_result = match result {
            Ok(r) => r,
            Err(e) => {
                if let Some(reason) = check_invalid(ExitReason::ReadStateFailed, "read_all error", &mut invalid_state_count) {
                    wp8f_core::warn_throttled(&WARN_READALL, ||
                        format!("[WARN] read_all error: {e:?}"));
                    return reason;
                }
                continue;
            }
        };

        let state = match parse_result.state {
            Ok(()) => &mut state,
            Err(_) => {
                if let Some(reason) = check_invalid(ExitReason::StateInvalid, "state parse failed", &mut invalid_state_count) {
                    return reason;
                }
                continue;
            }
        };

        let indic = match parse_result.indicators {
            Ok(()) => &mut indic,
            Err(_) => {
                if let Some(reason) = check_invalid(ExitReason::StateInvalid, "indicators parse failed", &mut invalid_state_count) {
                    return reason;
                }
                continue;
            }
        };

        let _map_obj = match parse_result.map_objects {
            Ok(true) => {
                ctx.map_obj = Some(std::mem::take(&mut map_obj)); // 不深拷贝：read_all 下一轮会重填
                true
            }
            Ok(false) => false,
            Err(_) => {
                wp8f_core::warn_throttled(&WARN_MAPOBJ_PARSE, ||
                    "[WARN] map_obj parse failed, skipping".to_string());
                false
            }
        };

        if !state.valid {
            if let Some(reason) = check_invalid(ExitReason::StateInvalid, "state.valid=false", &mut invalid_state_count) {
                return reason;
            }
            continue;
        }

        if !indic.valid {
            wp8f_core::warn_throttled(&WARN_INDIC_INVALID, ||
                "[WARN] indic.valid=false, reconnecting".to_string());
            return ExitReason::StateInvalid;
        }

        if tick.dropped {
            wp8f_core::warn_throttled(&WARN_DROPPED, ||
                format!("[WARN] dropped frame: interval_ns={}, frame={}", tick.interval_ns, tick.frame));
        }

        invalid_state_count = 0;

        let display_data = wp8f_disp::begin_frame().unwrap();

        update_display_from_state(state, indic, display_data, ctx, &tick);

        // Warning checks must run BEFORE commit_frame(): they write
        // `display_data.voice_alarm`, and publishing first would let the render
        // thread read stale state (and write into an already-published slot).
        if let Some(ref mut vw) = voice_warning {
            vw.check_all(display_data, ctx.flight_model(), tick.frame);
        }

        // F-18 风格机动告警声（独立音效层）：m≥75% 蜂鸣、≥100% 长鸣，
        // 与语音告警并存，不进语音冷却体系。配置 `maneuver_tone_enabled` 可整个关掉。
        if maneuver_tone_enabled {
            if let Some(ref mut vw) = voice_warning {
                let tone_out = maneuver_tone.update(display_data.maneuver_margin, Instant::now());
                if tone_out.solid_stop {
                    vw.stop_maneuver_solid();
                }
                if tone_out.solid_start {
                    vw.start_maneuver_solid();
                }
                if tone_out.beep {
                    vw.play_maneuver_blip();
                }
            }
        }

        wp8f_disp::commit_frame();

        if tick.mapobj_interval_ns > 0 {
            let map_slot = wp8f_disp::begin_map_frame().unwrap();
            update_map_display(map_slot, ctx, tick.mapobj_interval_ns);
            // Link-8：缓存本机归一化坐标与 POI 用于上报（友军快照由 datalink 线程自己发布）
            datalink_pos = (map_slot.player_map_x, map_slot.player_map_y);
            datalink_poi = if map_slot.nearest_poi_dist < f64::MAX {
                (true, map_slot.nearest_poi_x, map_slot.nearest_poi_y)
            } else {
                (false, 0.0, 0.0)
            };
            wp8f_disp::commit_map_frame();
        }

        // Link-8：主循环只把本机最新状态发布进数据链槽（无锁、无分配、没有 socket、没有等待）。
        // 上报节拍、排空应答、写友军快照全在 datalink 线程里，
        // HUD 侧用 `datalink::link_ring::latest_tracks()` 取最新消费。
        // `is_active()` 前置判断：数据链没开时连 `OwnState` 都不拼。
        if datalink::link_ring::is_active() {
            let mut own = datalink::link_ring::OwnState {
                ias: display_data.ias,
                tas: display_data.tas,
                altitude: display_data.altitude,
                mach: display_data.mach,
                heading: display_data.heading,
                vy: display_data.vy,
                x: datalink_pos.0,
                y: datalink_pos.1,
                poi_enabled: datalink_poi.0,
                poi_x: datalink_poi.1,
                poi_y: datalink_poi.2,
                ..Default::default()
            };
            own.set_type(display_data.aircraft_type.as_str());
            datalink::link_ring::publish_own(own);
        }

        // 记录全在记录线程里：主循环只写 `DisplayData` 环形缓冲区，本循环内没有任何磁盘 I/O。

        ts_next = ts_next + tick.interval_ns;
        clock.accept(&tick);
        wait_until(ts_next).await;
    }
}

#[derive(Parser, Debug)]
#[command(name = "wp8f-core")]
#[command(author = "wp8f contributors")]
#[command(version = "0.1.0")]
#[command(about = "War Thunder flight data visualization")]
struct Args {
    /// IP address to connect to (default: 127.0.0.1)
    #[arg(short, long, default_value = "127.0.0.1")]
    ip: Ipv4Addr,

    /// Comma-separated list of ports to rotate through (e.g., 8111,9222,11111)
    /// Default: 8111,9222
    #[arg(short, long, use_value_delimiter = true, value_delimiter = ',')]
    port: Option<Vec<u16>>,

    /// Base font size (default: 16)
    #[arg(long)]
    font_size: Option<f32>,

    /// Flight info panel X position (auto if not set)
    #[arg(long)]
    flight_info_x: Option<i32>,

    /// Flight info panel Y position (auto if not set)
    #[arg(long)]
    flight_info_y: Option<i32>,

    /// Engine info panel X position (auto if not set)
    #[arg(long)]
    engine_info_x: Option<i32>,

    /// Engine info panel Y position (auto if not set)
    #[arg(long)]
    engine_info_y: Option<i32>,

    /// Mini HUD X position (auto if not set)
    #[arg(long)]
    minihud_x: Option<i32>,

    /// Mini HUD Y position (auto if not set)
    #[arg(long)]
    minihud_y: Option<i32>,

    /// Flight info columns per row (default: 6)
    #[arg(long)]
    flight_info_col: Option<u32>,

    /// Engine info columns per row (default: 4)
    #[arg(long)]
    engine_info_col: Option<u32>,

    /// Map panel X position
    #[arg(long)]
    map_x: Option<i32>,

    /// Map panel Y position
    #[arg(long)]
    map_y: Option<i32>,

    /// Map panel width (0 = auto from font_size)
    #[arg(long)]
    map_size_x: Option<i32>,

    /// Map panel height (0 = auto from font_size)
    #[arg(long)]
    map_size_y: Option<i32>,

    /// Window width (default: 2560)
    #[arg(long)]
    window_width: Option<f32>,

    /// Window height (default: 1440)
    #[arg(long)]
    window_height: Option<f32>,

    /// Flight model data directory
    #[arg(long)]
    data_dir: Option<String>,

    /// Enable drag mode (move panels with mouse)
    #[arg(long)]
    drag: bool,

    /// Load layout config from JSON file
    #[arg(long)]
    layoutconfig: Option<String>,
}

impl Args {
    /// 命令行里给了的参数覆盖配置：**同名逐字覆盖**（`--font-size` → `cfg.font_size`）。
    /// 表就是下面这串标识符 —— 加一个同名参数只在这里补一个名字，不用再抄一行 `if let`。
    fn apply_overrides(&self, cfg: &mut wp8f_disp::HudLayoutConfig) {
        macro_rules! same_name {
            ($($field:ident),* $(,)?) => { $( if let Some(v) = self.$field { cfg.$field = v; } )* };
        }
        same_name!(
            font_size,
            flight_info_x,
            flight_info_y,
            flight_info_col,
            engine_info_x,
            engine_info_y,
            engine_info_col,
            minihud_x,
            minihud_y,
            map_x,
            map_y,
            map_size_x,
            map_size_y,
        );
        // HUD 类型（`hud_type` 0 关 / 1 MiniHUD / 2 圆环）**只从配置读**：
        // 没有对应的命令行参数 —— 有过的 `--hud-type` 会在启动时盖掉配置里的选择
        // （控制台曾硬编码传 `--hud-type circle`，用户在 GUI 里改了也没用）。
    }
}

#[tokio::main]
async fn main() {
    let mut args = Args::parse();

    let ports = args.port.take().unwrap_or_else(|| vec![8111, 9222]);
    let host = args.ip;

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    println!("WP8F Core - Starting...");
    println!("Connecting to {}:{:?}", host, ports);
    println!("Press Ctrl+C to stop");

    // 数据根解析一次（HUD 与 GUI 共用 `wp8f_flightmodel::resolve_data_root`）：
    // `--data-dir` > `$WP8F_DATA_DIR` > cwd/exe 附近的 `resource/data`（退到 `data_new`）。
    // 找不到不 panic：打一条错误，HUD 照常启动（FM 退化成占位模型）。
    let fm_data_dir = match wp8f_flightmodel::resolve_data_root(args.data_dir.as_deref().map(Path::new)) {
        Ok(root) => wp8f_flightmodel::flightmodels_dir(&root),
        Err(e) => {
            eprintln!("[FM] {e}；本次用占位模型（性能数值不可用）");
            wp8f_flightmodel::flightmodels_dir(Path::new("./resource/data"))
        }
    };
    let fm_data_dir = fm_data_dir.to_string_lossy().to_string();
    println!("FM data: {}", fm_data_dir);

    let w = args.window_width.unwrap_or(2560.0);
    let h = args.window_height.unwrap_or(1440.0);
    wp8f_disp::set_window_size(w, h);

    let mut cfg = if let Some(ref path) = args.layoutconfig {
        wp8f_disp::set_layout_config_path(path.clone());
        wp8f_disp::load_layout_config(path).unwrap_or_else(|| {
            eprintln!("Failed to load layout config from {}, using defaults", path);
            wp8f_disp::HudLayoutConfig::default()
        })
    } else {
        wp8f_disp::HudLayoutConfig::default()
    };
    // 命令行里给了的参数覆盖配置（同名逐字覆盖，表在 `Args::apply_overrides`）
    args.apply_overrides(&mut cfg);
    // HUD 标签语言表：core 在初始化时读一次（配置刚读完、字体/语音/窗口/渲染线程都还没起来）。
    // 读不到 / 解析失败 → 报错退出。
    match wp8f_disp::i18n::init(&cfg.hud_lang_path) {
        Ok(info) => println!(
            "[LANG] {} lang={} 标签 {} 条；读入 {} B → 常驻 {} B",
            info.file, info.lang, info.keys, info.json_bytes, info.bytes
        ),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    }

    // 字体同样是运行期从磁盘读入的：缺字体 HUD 一个字都画不出来 → 报错退出（退出码 2）。
    if let Err(e) = wp8f_disp::init_hud_layout(cfg) {
        eprintln!("{e}");
        std::process::exit(2);
    }

    // Link-8 数据链线程：core 只调用一次。句柄必须活到进程结束（Drop = 停线程），所以绑定在 main 里。
    let datalink_cfg = wp8f_disp::hud_layout().datalink.clone();
    let _datalink_thread = spawn_link_thread(&datalink_cfg);

    // 语音包在启动时一次性预载进内存（`voice_path` 目录里的 *.wav → `&'static [u8]`），
    // 之后播放零磁盘 I/O。缺目录/缺文件/坏文件只警告：语音是增强功能，不让 HUD 起不来。
    // 放在重连循环外面：换局不需要重新读盘。
    {
        let voice_path = wp8f_disp::hud_layout().voice_path.clone();
        let pack = wp8f_core::warnings::preload(&voice_path);
        if pack.count == 0 {
            eprintln!("[VOICE] 语音包为空：{}", pack.dir);
        } else {
            println!("[VOICE] 预载 {} 个语音 {} B：{}", pack.count, pack.total_bytes, pack.dir);
        }
    }

    if args.drag {
        wp8f_disp::set_drag_mode(true);
    }

    if let Err(e) = wp8f_disp::create_window() {
        tracing::error!("Failed to create display window: {}", e);
    }

    wp8f_disp::set_window_visible(false);

    loop {
        let mut sync_channel = Channel::with_ip_and_ports(host, ports.clone());

        if !try_connect(&mut sync_channel) {
            continue;
        }

        wp8f_disp::set_window_visible(true);
        wp8f_disp::reset_ring_buffer();
        // Link-8：新一局清掉上一局的友军快照（generation 前进，HUD 侧不会把旧友军留在屏幕上）
        datalink::link_ring::reset_tracks();

        eprintln!("Connected!");

        // 数据根在 main 开头就解析好了（启动日志里已经打印）；这里不再每次重连都重算一遍。

        // 语音告警：音频永远编译进程序（D 起是**启动时预载的整包内存**），是否出声由布局配置的
        // `voice_warnings_enabled` 决定（默认开，启动时读一次）。关掉后仍然照常做告警检查并
        // 驱动 HUD 的闪烁 X，只是不播放任何声音、也不初始化音频设备。
        // ⚠️ 这里**不再读磁盘**：字节来自上面 `warnings::preload` 的常驻包。
        let mut voice_warning =
            VoiceWarning::new(wp8f_disp::hud_layout().voice_warnings_enabled).ok();

        if let Some(ref mut vw) = voice_warning {
            vw.play_startup();
        }

        // 生效的地图对象采样周期（帧）：配置值按 `1..=refresh_hz` 夹紧，**本局只算一次**
        // （取值点 = [`effective_map_obj_frames`]，配置值被改动时打一行 `[MAPOBJ]`）。
        // 三处口径都用它：主循环闸门、FlightContext 的初值/elapsed 换算、记录起点 start_frame。
        let map_obj_frames = effective_map_obj_frames();

        let (mut ctx, _) = pre_process(&mut sync_channel, &fm_data_dir, map_obj_frames);

        // 飞行记录线程：core 只调用这一个接口（配置 + 环形缓冲区消费端），
        // 之后主循环里再没有任何文件句柄或写盘点。
        let recorder = spawn_recorder_thread(map_obj_frames);

        let async_channel = AsyncChannel::with_ip_and_ports(host, ports.clone());

        let exit_reason = data_process_loop(
            &async_channel,
            &mut ctx,
            map_obj_frames,
            voice_warning,
        )
        .await;

        // 停线程并把这一段飞行写成一个 `.wpr`（地图信息 + 底图 + 逐帧 CSV）。
        if let Some(recorder) = recorder {
            recorder.stop_and_write();
        }
        eprintln!("Post process done");

        wp8f_disp::set_window_visible(false);

        eprintln!("Exit reason: {}", exit_reason);
        eprintln!("Connection lost, retrying...");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 生效的地图记录间隔走完整条链路：缺键默认 8 数据帧 @30 Hz = 266 ms，在 `1..=30` 内
    /// → 原样生效、不夹也不打 `[MAPOBJ]`。记录线程的采样周期取的就是这个 266 ms。
    ///
    /// 这里读 `HudLayoutConfig::default()`（bin 的测试进程没人 `init_hud_layout`），
    /// 所以断言的是"缺键默认值"这一档，不受 `config/*.json` 影响。
    #[test]
    fn effective_map_obj_frames_is_only_capped_by_refresh_hz() {
        let cfg = wp8f_disp::hud_layout();
        assert_eq!(cfg.map_obj_record_every_frames_clamped(), 8, "内置默认 = MAP_OBJ_INTERVAL_FRAME");
        assert_eq!(cfg.refresh_hz_clamped(), 30);

        let frames = effective_map_obj_frames();
        assert_eq!(frames, 8);
        assert_eq!(wp8f_core::mapobj::period_ms(frames, 30), 266, "8 帧 @30 Hz = 266 ms");
    }
}
