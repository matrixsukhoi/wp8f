//! miniHUD 的**墨迹包围盒**必须落在 `mini_hud_rect()` 给的擦除框内。
//!
//! 背景（用户报的）：miniHUD 的上边缘与右边缘会留下没擦干净的像素 —— 说明有东西画到了
//! 框外（`hud.rs` 只擦那个框）。miniHUD 的真实足迹比"锚点 + 10×7 字宽"大一圈：
//! 油门数字列在竖条左侧、首行字形的上伸部高于锚点、POI 那段还会往右长。
//! 这条测试把 miniHUD 画进一张空缓冲，量出非透明像素的包围盒，和框比大小。
//!
//! 跑法：`cargo test --offline -p wp8f-disp --test mini_hud_bounds -- --nocapture`

use wp8f_disp::mini_hud::{draw_mini_hud, measure_pads};
use wp8f_disp::{
    init_hud_layout, mini_hud_rect, DisplayData, HudLayoutConfig, MapDisplay, DEFAULT_FONT_SIZE,
};

const PX: f32 = 300.0;
const PY: f32 = 300.0;

/// 一份"什么都在亮"的飞行数据：让 miniHUD 的每个元素都画出来（否则量不到它）。
fn sample() -> DisplayData {
    let mut d = DisplayData::default();
    d.valid = 1;
    d.ias_text.assign(b"1234");
    d.tas_text.assign(b"1300");
    d.mach_text.assign(b"1.20");
    d.mach = 1.2;
    d.mach_warning = 1;
    d.altitude_text.assign(b"8500");
    d.altitude = 8500.0;
    d.radio_altitude = 420.0;
    d.vy_text.assign(b"+12.3");
    d.aoa = 11.0;
    d.max_aoa = 22.0;
    d.sep_text.assign(b"145");
    d.sep = 145.0;
    d.ny = 7.5;
    d.g_load_warning = 1;
    d.has_attitude = true;
    d.turn_rate_text.assign(b"18.5");
    d.elapsed_time_text.assign(b"12:34");
    d.fuel_time_text.assign(b"23:45");
    d.afterburner_fuel_time_text.assign(b"01:23");
    d.fuel_kg = 1826.0;
    d.fuel_percent = 62.0;
    d.engine_count = 1;
    d.engine_throttle[0] = 105.0;
    d.engine_power[0] = 1200.0;
    d.engine_rpm[0] = 12500.0;
    d.engine_thrust[0] = 7200.0;
    d.aircraft_type.assign(b"f-16c");
    d
}

/// 带 POI 的地图数据：POI 那段（箭头 + 距离 + 相对速度）是往右长出去的那一块。
fn map_with_poi() -> MapDisplay {
    let mut m = MapDisplay::default();
    m.nearest_poi_dist = 12345.0;
    m.nearest_poi_bearing = 90.0;
    m.nearest_poi_rel_speed = -123.0;
    m
}

#[test]
fn mini_hud_ink_stays_inside_its_cleared_box() {
    let cfg = HudLayoutConfig::default();
    init_hud_layout(cfg.clone()).expect("字体与配置就位");

    let (w, h) = (1200usize, 800usize);
    for (name, map) in [("无 POI", MapDisplay::default()), ("有 POI", map_with_poi())] {
        let mut buf = vec![0u32; w * h];
        draw_mini_hud(&mut buf, w, h, &sample(), &map, PX, PY);

        let (mut min_x, mut min_y, mut max_x, mut max_y) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for y in 0..h {
            for x in 0..w {
                if buf[y * w + x] != 0 {
                    min_x = min_x.min(x as i32);
                    max_x = max_x.max(x as i32);
                    min_y = min_y.min(y as i32);
                    max_y = max_y.max(y as i32);
                }
            }
        }
        let (rx, ry, rw, rh) = mini_hud_rect(&cfg, PX, PY);
        let (rx1, ry1) = ((rx + rw) as i32, (ry + rh) as i32);
        println!(
            "[{name}] 框 = x[{}..{rx1}) y[{}..{ry1})；墨迹 = x[{min_x}..{}] y[{min_y}..{}]  \
             余量：左 {} 上 {} 右 {} 下 {}",
            rx as i32,
            ry as i32,
            max_x,
            max_y,
            min_x - rx as i32,
            min_y - ry as i32,
            rx1 - 1 - max_x,
            ry1 - 1 - max_y,
        );
        assert!(min_x <= max_x, "[{name}] miniHUD 什么都没画出来");
        assert!(min_x >= rx as i32, "[{name}] 左边缘越界 {} px", rx as i32 - min_x);
        assert!(min_y >= ry as i32, "[{name}] 上边缘越界 {} px", ry as i32 - min_y);
        assert!(max_x < rx1, "[{name}] 右边缘越界 {} px", max_x - rx1 + 1);
        assert!(max_y < ry1, "[{name}] 下边缘越界 {} px", max_y - ry1 + 1);
    }
}

/// 外扩量按字号线性缩放（擦除框跟着 mini 字号走）。
#[test]
fn mini_hud_rect_scales_with_font_size() {
    let mut small = HudLayoutConfig::default();
    small.mini_font_size = 20.0;
    let mut big = small.clone();
    big.mini_font_size = 40.0;

    let (sx, sy, sw, sh) = mini_hud_rect(&small, 100.0, 100.0);
    let (bx, by, bw, bh) = mini_hud_rect(&big, 100.0, 100.0);
    assert!(sx < 100.0 && sy < 100.0, "外扩必须体现在左上两侧");
    // 字号翻倍 → 尺寸与"往左上外扩的量"都翻倍
    assert!((bw - 2.0 * sw).abs() < 0.5, "宽度没按字号缩放：{sw} → {bw}");
    assert!((bh - 2.0 * sh).abs() < 0.5, "高度没按字号缩放：{sh} → {bh}");
    assert!(((100.0 - sx) * 2.0 - (100.0 - bx)).abs() < 0.5, "左侧外扩没按字号缩放");
    assert!(((100.0 - sy) * 2.0 - (100.0 - by)).abs() < 0.5, "上方外扩没按字号缩放");
}

/// 外扩量本身是"用真实字体量出来的"，不是拍脑袋的常数：左侧要覆盖 3 个字宽的数字列。
#[test]
fn measured_pads_cover_the_throttle_number_column() {
    let cfg = HudLayoutConfig::default();
    init_hud_layout(cfg.clone()).expect("字体与配置就位");
    let font = ab_glyph::FontRef::try_from_slice(wp8f_disp::font::data()).expect("字体可解析");
    let mf = DEFAULT_FONT_SIZE * (cfg.mini_font_size / DEFAULT_FONT_SIZE);
    let (l, t) = measure_pads(&font, mf);
    let (px_l, px_t) = (l * mf, t * mf);
    let char_w = wp8f_disp::measure_text_raw(&font, "0", mf * 0.707);
    assert!(px_l > char_w * 3.0, "左侧外扩 {px_l} 覆盖不了数字列 {}", char_w * 3.0);
    assert!(px_t > 0.0, "上方外扩应当为正（字形上伸部）");
}
