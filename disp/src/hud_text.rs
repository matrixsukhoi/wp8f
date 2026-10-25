//! HUD 文本的**唯一格式化处**。
//!
//! 三段流水：core 的更新线程读 8111 → 算数值（`core::display::updater` 的各个 `update_*`）→
//! **格式化（本模块）** → 整帧提交进环；渲染线程只 `as_str()` 落屏，一次格式化都不做。
//!
//! 为什么归这里、而不是写在 core 里：文本宽度、前后缀、取整位数是**HUD 版式**，属于显示层；
//! core 只该提供数值。两个 HUD 要哪些文本、各自怎么排，全部只在这一个文件里定义。
//!
//! 为什么不是运行期回调：注册 `fn` 指针或 trait 对象，等于每个字段一次间接调用；这里是 core
//! **静态调用**的普通函数（跨 crate 可内联），字段与格式在编译期定死 —— 没有虚调用、没有注册表、
//! 没有初始化顺序问题，HUD 想加一个数字只改本文件（core 的调用点一个字都不用动）。
//!
//! `fill` 是**一串互不依赖的写入**（同一操作、不同数据）：编译器可以把它们当成独立的
//! 访存 + 格式化流水起来；也**不按 `hud_type` 分支** —— 两套文本都算，渲染器各取所需
//! （HUD 类型一个进程内不变，把分支留在热路径里反而更慢）。

use super::state::{DisplayData, MapDisplay};

/// 一次算好两个 HUD 需要的所有文本：圆环 10 项 + miniHUD 5 项（`DisplayData.hud_text`）。
///
/// 与 [`HudText`](super::state::HudText) 的字段一一对应 —— 加字段时两处一起改。
#[inline]
pub fn fill(d: &mut DisplayData) {
    // 先把标量取出来：都是 `Copy`，既省得反复借 `d`，也把"这一段只读这几个量"写在明面上。
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
    let fuel_time = d.fuel_time_text.as_str();

    let text = &mut d.hud_text;

    // —— 圆环 HUD ——
    text.circle_alt.write_fmt_str(format_args!("H {:>5.0}", altitude));
    text.circle_radio_alt.write_fmt_str(format_args!("R {:>5.0}", radio_altitude));
    text.circle_ias.write_fmt_str(format_args!("{:>4}", ias));
    text.circle_mach.write_fmt_str(format_args!("{:>5.2}", mach));
    text.circle_aoa.write_fmt_str(format_args!("α {:.1}", aoa));
    text.circle_energy.write_fmt_str(format_args!("E{:>9.0}", energy_height));
    text.circle_sep.write_fmt_str(format_args!("SEP{:>+5.0}", sep));
    text.circle_g.write_fmt_str(format_args!("G{:>+7.1}", ny));
    text.circle_fuel.write_fmt_str(format_args!("L {}", fuel_time));
    text.circle_heading.write_fmt_str(format_args!("{:>3.0}°", heading));

    // —— miniHUD（同样的量，版式不同）——
    text.mini_mach.write_fmt_str(format_args!("M{:.2}", mach));
    text.mini_aoa.write_fmt_str(format_args!("α{:>3}", aoa as i32));
    text.mini_radio_alt.write_fmt_str(format_args!("{:.0}R", radio_altitude));
    text.mini_g.write_fmt_str(format_args!("{:.1}", ny));
    text.throttle.write_fmt_str(format_args!("{:.0}", throttle));
}

/// POI 的两个版本文本（`MapDisplay`）：**箭头规则两个 HUD 不同**，版式（宽度、取整、前导空格）
/// 也各一份 —— 都在这里算，渲染器只取值。
#[inline]
pub fn fill_map(map: &mut MapDisplay) {
    if map.nearest_poi_dist >= f64::MAX {
        // 没有 POI：清空，免得渲染器将来改条件时读到上一帧的旧文本。
        map.poi_text.clear();
        map.mini_poi_text.clear();
        map.poi_speed_text.clear();
        map.mini_poi_speed_text.clear();
        return;
    }
    let dist = (map.nearest_poi_dist / 1000.0).round();
    let bearing = map.nearest_poi_bearing;
    let rel_bearing_adj = bearing - 180.0;

    // 圆环 HUD：正前 `-`、左 `←`、右 `→`
    let circle_prefix = if rel_bearing_adj.abs() > 177.5 {
        "-"
    } else if rel_bearing_adj > 0.0 {
        "←"
    } else {
        "→"
    };
    map.poi_text.write_fmt_str(format_args!("{:<2}{:>3.0}", circle_prefix, dist));
    map.poi_speed_text.write_fmt_str(format_args!("{:+.0}", map.nearest_poi_rel_speed));

    // miniHUD：多了"几乎正左/正右"两档（`<` / `>`）
    let mini_prefix = if rel_bearing_adj.abs() > 177.5 {
        "-"
    } else if rel_bearing_adj > 0.0 {
        if bearing > 355.0 { "<" } else { "←" }
    } else if bearing < 5.0 {
        ">"
    } else {
        "→"
    };
    map.mini_poi_text.write_fmt_str(format_args!("{:<2}{:>3.0}", mini_prefix, dist));
    map.mini_poi_speed_text.write_fmt_str(format_args!(" {:+.0}", map.nearest_poi_rel_speed));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DisplayData {
        let mut d = DisplayData::default();
        d.altitude = 1234.0;
        d.radio_altitude = 420.0;
        d.ias = 345.0;
        d.mach = 0.86;
        d.aoa = 11.25;
        d.energy_height = 123.4;
        d.sep = 145.0;
        d.ny = 7.5;
        d.heading = 90.0;
        d.engine_throttle[0] = 105.0;
        d.fuel_time_text.assign(b"23:45");
        d
    }

    /// 这些字符串就是**渲染器落屏的契约**（宽度/前后缀/取整都在里面）：
    /// 格式化搬到 compute 阶段后，改格式必须同时看这里 —— 以前它们散在渲染器里，没人钉住。
    #[test]
    fn fill_writes_the_exact_texts_the_renderers_draw() {
        let mut d = sample();
        fill(&mut d);
        assert_eq!(d.hud_text.circle_alt.as_str(), "H  1234");
        assert_eq!(d.hud_text.circle_radio_alt.as_str(), "R   420");
        assert_eq!(d.hud_text.circle_ias.as_str(), " 345");
        assert_eq!(d.hud_text.circle_mach.as_str(), " 0.86");
        assert_eq!(d.hud_text.circle_aoa.as_str(), "α 11.2");
        assert_eq!(d.hud_text.circle_energy.as_str(), "E      123");
        assert_eq!(d.hud_text.circle_sep.as_str(), "SEP +145");
        assert_eq!(d.hud_text.circle_g.as_str(), "G   +7.5");
        assert_eq!(d.hud_text.circle_fuel.as_str(), "L 23:45");
        assert_eq!(d.hud_text.circle_heading.as_str(), " 90°");
        // miniHUD 是同一批量数据的不同版式
        assert_eq!(d.hud_text.mini_mach.as_str(), "M0.86");
        assert_eq!(d.hud_text.mini_aoa.as_str(), "α 11");
        assert_eq!(d.hud_text.mini_radio_alt.as_str(), "420R");
        assert_eq!(d.hud_text.mini_g.as_str(), "7.5");
        assert_eq!(d.hud_text.throttle.as_str(), "105");
    }

    #[test]
    fn fill_map_formats_poi_and_clears_it_when_there_is_none() {
        let mut m = MapDisplay::default();
        m.nearest_poi_dist = 12345.0;
        m.nearest_poi_bearing = 90.0;
        m.nearest_poi_rel_speed = -123.0;
        fill_map(&mut m);
        assert_eq!(m.poi_text.as_str(), "→  12", "圆环：箭头 + 右对齐距离");
        assert_eq!(m.poi_speed_text.as_str(), "-123");
        assert_eq!(m.mini_poi_text.as_str(), "→  12", "同一方位，mini 版式此刻与圆环相同");
        assert_eq!(m.mini_poi_speed_text.as_str(), " -123", "mini 版式带前导空格");

        // 没有 POI（哨兵值 f64::MAX）：清空，别让渲染器读到上一帧的旧文本
        m.nearest_poi_dist = f64::MAX;
        fill_map(&mut m);
        assert!(m.poi_text.is_empty() && m.mini_poi_text.is_empty());
        assert!(m.poi_speed_text.is_empty() && m.mini_poi_speed_text.is_empty());
    }
}
