//! 渲染线程的**格式化阶段**：把环里的数值写成 [`HudTexts`](super::hud_texts::HudTexts)。
//!
//! 流水：core 读 8111 → core 算数值（环里只有数值）→ **本模块：渲染线程每帧格式化一次** →
//! 渲染器只 `as_str()` 落屏。
//!
//! 登记是**编译期**的：下面的宏把表展开成顺序直写调用（`f1(t, d, m); f2(t, d, m); …`），
//! 不经过 `fn` 指针、trait 对象或运行期注册表。加一个 HUD 模块 = 表里加一行。
//!
//! 谁该登记在这里：**所有把数值变成文本的代码**。渲染器里不许出现 `format_args!`/`format!`；
//! 不登记的只有颜色/布局这类"画的时候才知道"的决定。

use super::hud_texts::HudTexts;
use super::{DisplayData, FixedBytes, MapDisplay};

/// 显示侧格式化登记表。
macro_rules! hud_formatters {
    ($($formatter:path),* $(,)?) => {
        /// 一帧的格式化总入口（顺序直写调用，编译期展开）。
        #[inline]
        pub fn format_all(t: &mut HudTexts, d: &DisplayData, m: &MapDisplay) {
            $( $formatter(t, d, m); )*
        }
    };
}

hud_formatters!(format_panel, format_hud_type, format_map);

// ------------------------------------------------------------------ 基础 --
/// 燃油时间"无限"的哨兵（与 `core::constants::INFINITE_FUEL_TIME` 同值：那边用于计算，
/// 这边只用于"别把它格式成 9 小时"的判断）。
const INFINITE_FUEL_TIME: f64 = 32768.0;
/// 燃油时间按 10 秒取整（与 `core::constants::FUEL_TIME_ROUNDING` 同值）。
const FUEL_TIME_ROUNDING: f64 = 10.0;
/// 增压表换算（与 `core::constants::PSI_PER_ATM` 同值）。
const PSI_PER_ATM: f64 = 14.696;
/// mmHg / atm（与 `core::constants::MMHG_PER_ATM` 同值）。
const MMHG_PER_ATM: f64 = 760.0;
/// mm / inch（与 `core::constants::MM_PER_INCH` 同值）。
const MM_PER_INCH: f64 = 25.4;

fn set_text<const N: usize>(field: &mut FixedBytes<N>, args: std::fmt::Arguments<'_>) {
    field.write_fmt_str(args);
}

fn set_text_str<const N: usize>(field: &mut FixedBytes<N>, text: &str) {
    field.assign(text.as_bytes());
}

/// 有值就按格式写数字，没值就写占位符：`text_or!(字段, 条件, "占位符", "格式", 参数…)`。
macro_rules! text_or {
    ($field:expr, $cond:expr, $placeholder:literal, $fmt:literal $(, $arg:expr)*) => {
        if $cond {
            set_text($field, format_args!($fmt $(, $arg)*));
        } else {
            set_text_str($field, $placeholder);
        }
    };
}

fn set_time_or_dash<const N: usize>(field: &mut FixedBytes<N>, time_val: f64, min: f64, max: f64) {
    if time_val > min && time_val < max {
        let secs = (time_val / FUEL_TIME_ROUNDING).round() * FUEL_TIME_ROUNDING;
        set_duration(field, secs);
    } else {
        set_text_str(field, "-");
    }
}

fn write_sig_figs(out: &mut impl std::fmt::Write, value: f64, sigfigs: usize) -> std::fmt::Result {
    if value <= 0.0 {
        return out.write_str("-");
    }
    let magnitude = value.abs().log10().floor() as i32;
    let factor = 10_f64.powi(magnitude - sigfigs as i32 + 1);
    write!(out, "{:.0}", (value / factor).round() * factor)
}

fn write_adaptive(out: &mut impl std::fmt::Write, v: f64, base_decimals: i32) -> std::fmt::Result {
    if !v.is_finite() || v <= 0.0 {
        return out.write_str("-");
    }
    let prec = (base_decimals - v.abs().log10().floor() as i32).max(0) as usize;
    write!(out, "{:.prec$}", v, prec = prec)
}

fn write_duration(out: &mut impl std::fmt::Write, total_secs: f64) -> std::fmt::Result {
    if !total_secs.is_finite() || total_secs < 0.0 {
        return out.write_str("-");
    }
    let mins = (total_secs / 60.0).floor() as u32;
    let secs = (total_secs % 60.0) as u32;
    write!(out, "{}'{:02.0}", mins, secs)
}

fn set_sig<const N: usize>(field: &mut FixedBytes<N>, value: f64, sigfigs: usize) {
    field.clear();
    let _ = write_sig_figs(field, value, sigfigs);
}

fn set_adaptive<const N: usize>(field: &mut FixedBytes<N>, value: f64, base_decimals: i32) {
    field.clear();
    let _ = write_adaptive(field, value, base_decimals);
}

fn set_duration<const N: usize>(field: &mut FixedBytes<N>, total_secs: f64) {
    field.clear();
    let _ = write_duration(field, total_secs);
}

/// SEP 按量级取整（与 `core::math::angle::round_sep` 同式）。
fn round_sep(sep: f64) -> f64 {
    let abs_sep = sep.abs();
    let step = if abs_sep <= 30.0 {
        1.0
    } else if abs_sep <= 50.0 {
        2.0
    } else if abs_sep <= 100.0 {
        5.0
    } else if abs_sep <= 200.0 {
        10.0
    } else if abs_sep <= 300.0 {
        15.0
    } else if abs_sep <= 400.0 {
        20.0
    } else if abs_sep <= 600.0 {
        50.0
    } else {
        100.0
    };
    (sep / step).round() * step
}

fn write_manifold_pressure_value<const N: usize>(
    field: &mut FixedBytes<N>,
    manifold_atm: f64,
    imperial: bool,
) {
    field.clear();
    use std::fmt::Write;
    if imperial {
        let _ = write!(field, "{:+.1}", (manifold_atm - 1.0) * PSI_PER_ATM);
    } else {
        let _ = write!(field, "{:.2}", manifold_atm);
    }
}

fn write_manifold_pressure_unit<const N: usize>(
    field: &mut FixedBytes<N>,
    manifold_atm: f64,
    imperial: bool,
) {
    field.clear();
    if imperial {
        use std::fmt::Write;
        let _ = write!(field, "P/{:.0}''", manifold_atm * MMHG_PER_ATM / MM_PER_INCH);
    } else {
        set_text_str(field, "Ata");
    }
}

fn write_remain_or_dash(field: &mut FixedBytes<16>, secs: f64) {
    if secs < 3600.0 {
        let total_secs = secs as u64;
        let m = total_secs / 60;
        let s = total_secs % 60;
        set_text(field, format_args!("℃/{}'{:02}", m, (s / 10) * 10));
    } else {
        set_text_str(field, "℃/-");
    }
}

/// 最近目标一行：`AF 17.5km / 172°`（`/ ` 分隔，可读性优先）。
fn write_format_dist<const N: usize>(field: &mut FixedBytes<N>, prefix: &str, dist: f64, bearing: f64) {
    field.clear();
    use std::fmt::Write;
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

// -------------------------------------------------------------- 飞行面板 --
/// 飞行/发动机/友军面板的语义文本（原 `core::display::updater::format_display_text` 的文本部分）。
fn format_panel(t: &mut HudTexts, d: &DisplayData, _m: &MapDisplay) {
    // 能量高度：位数按量级自适应（`eh == 0`/NaN 时不能进 log10）
    let eh = d.energy_height;
    let eh_decade = if eh.is_finite() && eh.abs() > f64::MIN_POSITIVE {
        eh.abs().log10().floor() as i32
    } else {
        0
    };
    let eh_scale = 10.0_f64.powi(2 - eh_decade);
    let eh_rounded = (eh * eh_scale).round() / eh_scale;
    let eh_prec = (2 - eh_decade).max(0) as usize;
    set_text(&mut t.energy_height_text, format_args!("E {:.eh_prec$}", eh_rounded, eh_prec = eh_prec));

    set_text(&mut t.altitude_text, format_args!("{:.0}", d.altitude));
    set_text(&mut t.tas_text, format_args!("{:.0}", d.tas));
    set_text(&mut t.ias_text, format_args!("{:.0}", d.ias));
    set_text(&mut t.mach_text, format_args!("{:.2}", d.mach));
    set_text(&mut t.vy_text, format_args!("{:+.1}", d.vy));
    set_text(&mut t.sep_text, format_args!("{:+.0}", round_sep(d.sep)));
    text_or!(&mut t.turn_rate_text, d.turn_rate < 999.0, "---", "{:.1}", d.turn_rate);
    text_or!(&mut t.turn_radius_text, d.turn_radius < 99999.0, "-", "{:.0}", d.turn_radius);
    // 单位栏：VNE / MNE
    if d.vne > 0.0 {
        set_text(&mut t.vne_text, format_args!("/{:.0}", d.vne));
    } else {
        t.vne_text.clear();
    }
    if d.vne_mach > 0.0 {
        set_text(&mut t.mne_text, format_args!("/{:.2}", d.vne_mach));
    } else {
        t.mne_text.clear();
    }
    set_text(&mut t.fuel_kg_text, format_args!("{:.0}", d.fuel_kg));
    set_text(&mut t.fuel_percent_text, format_args!("{:.0}", d.fuel_percent));
    set_time_or_dash(&mut t.fuel_time_text, d.fuel_time, 0.0, INFINITE_FUEL_TIME);
    text_or!(&mut t.fuel1_kg_text, d.has_fuel1, "-", "{:.0}", d.fuel1_kg);
    set_time_or_dash(&mut t.fuel1_time_text, d.fuel1_time, 0.0, INFINITE_FUEL_TIME);
    set_adaptive(&mut t.fuel_sfc_text, d.fuel_sfc, 3);
    set_adaptive(&mut t.fuel1_sfc_text, d.fuel1_sfc, 3);
    set_duration(&mut t.elapsed_time_text, d.elapsed_secs);

    text_or!(&mut t.engine_rpm_text, d.max_rpm > 0.0, "0", "{:.0}", d.max_rpm);
    set_sig(&mut t.engine_power_text, d.total_hp, 3);
    let eff = d.engine_efficiency as i32;
    if eff > 0 && !d.is_jet {
        set_text(&mut t.power_unit_text, format_args!("Hp/{}%", eff));
    } else {
        t.power_unit_text.clear();
    }
    if d.rpm_max_allowed < f64::MAX {
        set_text(&mut t.engine_rpm_unit, format_args!("/{}", d.rpm_max_allowed as u32));
    } else {
        t.engine_rpm_unit.clear();
    }
    let rated_power = if !d.is_jet { d.rated_power } else { 0.0 };
    text_or!(&mut t.rated_power_text, rated_power > 0.0, "-", "{:.0}", rated_power);
    set_text(&mut t.thrust_to_weight_text, format_args!("{:.2}", d.thrust_to_weight));
    set_text(&mut t.power_to_weight_text, format_args!("{:.2}", d.power_to_weight));
    text_or!(&mut t.engine_throttle_text, d.engine_count > 0, "0", "{:.0}", d.engine_throttle[0]);
    text_or!(&mut t.engine_temp_water_text, d.engine_temp_water[0] > 0.0, "-", "{:.0}", d.engine_temp_water[0]);
    text_or!(&mut t.engine_temp_oil_text, d.engine_temp_oil[0] > 0.0, "-", "{:.0}", d.engine_temp_oil[0]);
    set_sig(&mut t.total_thrust_text, d.total_thrust, 3);
    set_text(&mut t.thrust_percent_text, format_args!("{:.0}", d.thrust_percent));
    set_text(&mut t.roll_rate_text, format_args!("{:>+4.1}", d.wx));
    set_text(&mut t.pitch_text, format_args!("{:>+4.1}", -d.pitch));
    set_text(&mut t.aoa_format_text, format_args!("{:>+4.1}", d.aoa));
    set_text(&mut t.g_load_text, format_args!("{:>+4.1}", d.ny));
    // 单位栏 "/15.4"（与表速格 VNE 同一风格）
    text_or!(&mut t.g_limit_text, d.pos_limit > 0.0, "-", "/{:.1}", d.pos_limit);

    // 增压表：有值才画（英制时换成 psi / inHg）
    if d.manifold_pressure > 0.0 {
        write_manifold_pressure_value(&mut t.manifold_pressure_text, d.manifold_pressure, d.is_imperial);
        write_manifold_pressure_unit(&mut t.manifold_pressure_unit, d.manifold_pressure, d.is_imperial);
    } else {
        t.manifold_pressure_text.clear();
        t.manifold_pressure_unit.clear();
    }
    // 过热：剩余可承受时间（≥1 小时当作"不会过热"）
    write_remain_or_dash(&mut t.water_crit_temp_text, d.water_remain_secs);
    write_remain_or_dash(&mut t.oil_crit_temp_text, d.oil_remain_secs);
    // 加力剩余时间：当前没有数据源（与迁移前一致，恒空）
    t.afterburner_fuel_time_text.clear();
    // 状态项（F/W）：只在有值时才写，渲染侧按"文本是否为空"决定画不画
    if d.flaps > 0.0 {
        set_text(&mut t.flaps_text, format_args!("F{:.0}", d.flaps));
    } else {
        t.flaps_text.clear();
    }
    if d.wing_sweep > 0.0 {
        set_text(&mut t.wing_sweep_text, format_args!("W{:.0}", (d.wing_sweep * 100.0).round()));
    } else {
        t.wing_sweep_text.clear();
    }
    set_sig(&mut t.total_drag_text, d.total_drag, 2);
}

// ------------------------------------------------------------ 两个 HUD --
fn format_hud_type(t: &mut HudTexts, d: &DisplayData, _m: &MapDisplay) {
    let altitude = d.altitude;
    let radio_altitude = d.radio_altitude;
    let ias = d.ias as i32;
    let mach = d.mach;
    let aoa = d.aoa;
    let energy_height = d.energy_height;
    let sep = d.sep as i32;
    let ny = d.ny;
    let heading = d.heading;
    let throttle = d.engine_throttle[0].clamp(0.0, 110.0);
    let fuel_time = t.fuel_time_text.as_str();

    // —— 圆环 HUD ——
    set_text(&mut t.circle_alt, format_args!("H {:>5.0}", altitude));
    set_text(&mut t.circle_radio_alt, format_args!("R {:>5.0}", radio_altitude));
    set_text(&mut t.circle_ias, format_args!("{:>4}", ias));
    set_text(&mut t.circle_mach, format_args!("{:>5.2}", mach));
    set_text(&mut t.circle_aoa, format_args!("α {:.1}", aoa));
    set_text(&mut t.circle_energy, format_args!("E{:>9.0}", energy_height));
    set_text(&mut t.circle_sep, format_args!("SEP{:>+5.0}", sep));
    set_text(&mut t.circle_g, format_args!("G{:>+7.1}", ny));
    set_text(&mut t.circle_fuel, format_args!("L {}", fuel_time));
    set_text(&mut t.circle_heading, format_args!("{:>3.0}°", heading));

    // —— miniHUD（同样的量，版式不同）——
    set_text(&mut t.mini_mach, format_args!("M{:.2}", mach));
    set_text(&mut t.mini_aoa, format_args!("α{:>3}", aoa as i32));
    set_text(&mut t.mini_radio_alt, format_args!("{:.0}R", radio_altitude));
    set_text(&mut t.mini_g, format_args!("{:.1}", ny));
    set_text(&mut t.throttle, format_args!("{:.0}", throttle));
}

// -------------------------------------------------------------- 地图面板 --
fn format_map(t: &mut HudTexts, d: &DisplayData, m: &MapDisplay) {
    write_format_dist(&mut t.nearest_airfield_text, "AF", m.nearest_airfield_dist, m.nearest_airfield_bearing);
    write_format_dist(&mut t.nearest_bombing_point_text, "BP", m.nearest_bombing_dist, m.nearest_bombing_bearing);
    write_format_dist(&mut t.nearest_poi_text, "POI", m.nearest_poi_dist, m.nearest_poi_bearing);
    // 区域格：算法单一实现在 `hud_common::map_zone_label`（友军面板共用）
    let zone = crate::hud_common::map_zone_of(m, m.player_map_x, m.player_map_y);
    set_text_str(&mut t.map_zone_text, zone.as_str());
    set_text(&mut t.map_zone_line, format_args!("Zone {} {:.0}°", zone.as_str(), d.heading));
    if m.nearest_poi_neighbor_dist < f64::MAX {
        set_text(&mut t.map_poi_line,
                 format_args!("{} / {:.0}m", t.nearest_poi_text.as_str(), m.nearest_poi_neighbor_dist));
    } else {
        set_text_str(&mut t.map_poi_line, t.nearest_poi_text.as_str());
    }

    if m.nearest_poi_dist >= f64::MAX {
        t.poi_text.clear();
        t.mini_poi_text.clear();
        t.poi_speed_text.clear();
        t.mini_poi_speed_text.clear();
        return;
    }
    let dist = (m.nearest_poi_dist / 1000.0).round();
    let bearing = m.nearest_poi_bearing;
    let rel = bearing - 180.0;

    // 圆环 HUD：正前 `-`、左 `←`、右 `→`
    let circle_prefix = if rel.abs() > 177.5 {
        "-"
    } else if rel > 0.0 {
        "←"
    } else {
        "→"
    };
    set_text(&mut t.poi_text, format_args!("{:<2}{:>3.0}", circle_prefix, dist));
    set_text(&mut t.poi_speed_text, format_args!("{:+.0}", m.nearest_poi_rel_speed));

    // miniHUD：多了"几乎正左/正右"两档（`<` / `>`）
    let mini_prefix = if rel.abs() > 177.5 {
        "-"
    } else if rel > 0.0 {
        if bearing > 355.0 { "<" } else { "←" }
    } else if bearing < 5.0 {
        ">"
    } else {
        "→"
    };
    set_text(&mut t.mini_poi_text, format_args!("{:<2}{:>3.0}", mini_prefix, dist));
    set_text(&mut t.mini_poi_speed_text, format_args!(" {:+.0}", m.nearest_poi_rel_speed));
}

/// 友军面板每行的数值列（数据来自数据链快照，不进环）。
pub fn format_link(t: &mut HudTexts, tracks: &[datalink::StatePacket], map: &MapDisplay) {
    let mut row = 0usize;
    for p in tracks {
        if p.flags & datalink::FLAG_SENDER != 0 {
            continue;
        }
        if row >= super::hud_texts::MAX_LINK_ROWS {
            break;
        }
        let dx = p.x - map.player_map_x;
        let dy = p.y - map.player_map_y;
        let dist_km = (dx * dx + dy * dy).sqrt() * map.map_maxsize[0] / 1000.0;
        let r = &mut t.link_rows[row];
        set_text(&mut r.ias, format_args!("{:.0}", p.ias));
        set_text(&mut r.alt, format_args!("{:.0}", p.altitude));
        set_text(&mut r.hdg, format_args!("{:.0}", p.heading));
        set_text(&mut r.dist, format_args!("{:.1}", dist_km));
        row += 1;
    }
    for r in t.link_rows[row..].iter_mut() {
        r.ias.clear();
        r.alt.clear();
        r.hdg.clear();
        r.dist.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (DisplayData, MapDisplay) {
        let mut d = DisplayData::default();
        d.altitude = 1234.0;
        d.ias = 345.0;
        d.mach = 0.86;
        d.aoa = 11.25;
        d.energy_height = 123.4;
        d.sep = 145.0;
        d.ny = 7.5;
        d.heading = 90.0;
        d.engine_count = 1;
        d.engine_throttle[0] = 105.0;
        d.fuel_time = 20.0 * 60.0;
        d.engine_temp_water[0] = 88.0;
        d.turn_rate = 12.3;
        d.g_load_warning = crate::WarningStage::Caution;
        d.pos_limit = 15.4;
        d.elapsed_secs = 754.0;
        d.max_rpm = 2850.0;
        let mut m = MapDisplay::default();
        m.nearest_poi_dist = 12345.0;
        m.nearest_poi_bearing = 90.0;
        m.nearest_poi_rel_speed = -123.0;
        (d, m)
    }

    /// 文本契约：这些字符串就是渲染器落屏的内容，改格式必须同时看这里。
    #[test]
    fn format_all_writes_the_texts_the_renderers_draw() {
        let (d, m) = sample();
        let mut t = HudTexts::default();
        format_all(&mut t, &d, &m);

        // 飞行面板
        assert_eq!(t.ias_text.as_str(), "345");
        assert_eq!(t.mach_text.as_str(), "0.86");
        assert_eq!(t.vy_text.as_str(), "+0.0");
        assert_eq!(t.turn_rate_text.as_str(), "12.3");
        assert_eq!(t.g_load_text.as_str(), "+7.5");
        assert_eq!(t.g_limit_text.as_str(), "/15.4");
        assert_eq!(t.elapsed_time_text.as_str(), "12'34");
        assert_eq!(t.engine_rpm_text.as_str(), "2850");
        assert_eq!(t.engine_temp_water_text.as_str(), "88");
        // 两个 HUD 的版式
        assert_eq!(t.circle_alt.as_str(), "H  1234");
        assert_eq!(t.circle_radio_alt.as_str(), "R     0");
        assert_eq!(t.circle_ias.as_str(), " 345");
        assert_eq!(t.circle_sep.as_str(), "SEP +145");
        assert_eq!(t.circle_g.as_str(), "G   +7.5");
        assert_eq!(t.mini_mach.as_str(), "M0.86");
        assert_eq!(t.mini_aoa.as_str(), "α 11");
        assert_eq!(t.throttle.as_str(), "105");
        // 地图
        assert_eq!(t.poi_text.as_str(), "→  12");
        assert_eq!(t.poi_speed_text.as_str(), "-123");
        assert_eq!(t.mini_poi_speed_text.as_str(), " -123");
    }

    #[test]
    fn format_map_clears_poi_when_there_is_none() {
        let mut m = MapDisplay::default();
        m.nearest_poi_dist = f64::MAX;
        let d = DisplayData::default();
        let mut t = HudTexts::default();
        format_all(&mut t, &d, &m);
        assert!(t.poi_text.is_empty() && t.mini_poi_text.is_empty());
        assert!(t.poi_speed_text.is_empty() && t.mini_poi_speed_text.is_empty());
        assert_eq!(t.nearest_poi_text.as_str(), "POI -", "没有目标时写占位符");
    }
}
