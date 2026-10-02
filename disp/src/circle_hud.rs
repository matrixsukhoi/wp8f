use crate::{draw_text_raw, measure_text_raw, set_pixel, DisplayData, MapDisplay, FONT};
use std::sync::OnceLock;

static SIN_COS: OnceLock<[(f32, f32); 360]> = OnceLock::new();

fn sincos(deg: i32) -> (f32, f32) {
    let table = SIN_COS.get_or_init(|| {
        let mut t = [(0.0f32, 0.0f32); 360];
        for i in 0..360 {
            let rad = (i as f32).to_radians();
            t[i] = (rad.cos(), rad.sin());
        }
        t
    });
    let idx = ((deg % 360) + 360) as usize % 360;
    table[idx]
}

fn clock_pos(cx: f32, cy: f32, hours: f32, r: f32) -> (f32, f32) {
    let deg = (hours - 3.0) * 30.0;
    let rad = deg * std::f32::consts::PI / 180.0;
    (cx + r * rad.cos(), cy + r * rad.sin())
}

fn draw_mt(
    buffer: &mut [u32], w: usize, h: usize, font: &ab_glyph::FontRef,
    x: f32, y: f32, text: &str, color: u32, font_size: f32,
) {
    draw_text_raw(buffer, w, h, font, x, y, text, font_size, color);
}

/// 水平细条：`from_right` 为 true 时填充从右端向左生长（攻角余量条），
/// false 时从左端向右生长（油门条）。上下一行描边，其余留半透明槽底。
fn draw_hbar(
    buffer: &mut [u32], bw: usize, bh: usize,
    x: i32, y: i32, w: i32, h: i32, ratio: f32,
    fill: u32, border: u32, from_right: bool,
) {
    let fw = (w as f32 * ratio.clamp(0.0, 1.0)) as i32;
    for dy in 0..h {
        for dx in 0..w {
            let px = if from_right { x - dx } else { x + dx };
            let py = y + dy;
            if px >= 0 && px < bw as i32 && py >= 0 && py < bh as i32 {
                let idx = py as usize * bw + px as usize;
                if idx < buffer.len() {
                    buffer[idx] = if dy == 0 || dy == h - 1 { border } else if dx < fw { fill } else { 0x40000000 };
                }
            }
        }
    }
}

pub fn draw_circle_hud(buffer: &mut [u32], width: usize, height: usize, data: &DisplayData, map: &MapDisplay, cx: f32, cy: f32) {
    let layout = crate::hud_layout();
    let circle_font_size = layout.circle_font_size;
    let circle_ring_radius = layout.circle_ring_radius;
    // 圆环用**数字色 data_color**（原 circle_color 配置键已删除）
    let ring_color = layout.data_color;
    let small_sz = circle_font_size * 0.707;
    let line_gap = circle_font_size * 0.3;
    let aoa_w = circle_font_size * 3.33;
    let aoa_h = circle_font_size * 0.33;
    let col_w = circle_font_size * 3.5;
    let inner_r = circle_ring_radius - 8.0;
    let outer_r = circle_ring_radius + 8.0;

    let px = (circle_font_size / 24.0).max(1.0);
    let arc_thick = (5.0 * px).round() as i32;

    FONT.with(|font| {
        let colors = &crate::HUD_COLORS;
        draw_attitude(buffer, width, height, cx, cy, data, circle_ring_radius, ring_color, arc_thick);
        draw_speed_aoa(buffer, width, height, font, cx, cy, data, colors, circle_font_size, small_sz, line_gap, aoa_w, aoa_h, col_w, inner_r);
        draw_sep_gload(buffer, width, height, font, cx, cy, data, colors, circle_font_size, small_sz, line_gap, col_w, outer_r);
        draw_fuel(buffer, width, height, font, cx, cy, data, colors, circle_font_size, small_sz, line_gap, outer_r);
        draw_heading_zone(buffer, width, height, font, cx, cy, data, map, colors, circle_font_size, small_sz, line_gap, outer_r, 6.0);
        draw_poi(buffer, width, height, font, cx, cy, map, colors, circle_font_size, small_sz, line_gap, inner_r, 6.0);
    });

    // Voice alarm overlay: flash a green X inscribed in the HUD ring at 4 Hz.
    // The X corners lie on the ring, so the X never exceeds the circle HUD.
    let half = circle_ring_radius * std::f32::consts::FRAC_1_SQRT_2;
    crate::hud_common::draw_voice_alarm_x(
        buffer,
        width,
        height,
        data,
        cx - half,
        cy - half,
        half * 2.0,
        half * 2.0,
        circle_font_size,
    );
}


fn draw_attitude(
    buffer: &mut [u32], width: usize, height: usize,
    cx: f32, cy: f32, data: &DisplayData, ring_radius: f32, color: u32, thickness: i32,
) {
    if !data.has_attitude {
        return;
    }
    let pitch = -data.pitch as f32;
    let half_span = (90.0 - pitch).clamp(0.0, 180.0);
    if half_span <= 0.0 {
        return;
    }
    let center_deg = 90.0 + data.roll as f32;
    let start_deg = (center_deg - half_span) as i32;
    let end_deg = (center_deg + half_span) as i32;
    let half = thickness / 2;

    for angle in start_deg..=end_deg {
        let deg = ((angle % 360) + 360) % 360;
        let (cos_a, sin_a) = sincos(deg);
        for d in -half..=half {
            let x = cx + (ring_radius + d as f32) * cos_a;
            let y = cy + (ring_radius + d as f32) * sin_a;
            set_pixel(buffer, width, height, x as i32, y as i32, color);
        }
    }
}

fn draw_speed_aoa(
    buffer: &mut [u32], width: usize, height: usize, font: &ab_glyph::FontRef,
    cx: f32, cy: f32, data: &DisplayData, colors: &crate::components::HudColors,
    font_sz: f32, small_sz: f32, line_gap: f32, aoa_w: f32, aoa_h: f32, _col_w: f32, inner_r: f32,
) {
    let (x, y) = clock_pos(cx, cy, 9.0, inner_r);
    let gap = font_sz * 0.5;
    // Two-stage overspeed warning (yellow/red) plus the speed-brake caution.
    let ias_color = crate::hud_common::ias_warning_color(data, colors.data_u32());
    // Mach number is colored by its own MNE warning stage.
    let mach_color = crate::hud_common::warning_level_color(data.mach_warning, colors.data_u32());

    // Row 0: altitude (above center, no label, font_sz)
    let row0_y = y - (font_sz + line_gap);
    let (alt_text, alt_color) = if data.radio_altitude > 0.0 && data.radio_altitude < 500.0 {
        (format!("R {:>5.0}", data.radio_altitude), colors.hint_u32())
    } else if data.altitude < 58.0 {
        (format!("H {:>5.0}", data.altitude), colors.alert_u32())
    } else {
        (format!("H {:>5.0}", data.altitude), colors.data_u32())
    };
    draw_mt(buffer, width, height, font, x, row0_y, &alt_text, alt_color, font_sz);

    // Row 1: throttle bar（长度与攻角条一致、宽度（厚度）为其 1/2）+ 右侧油门百分比
    // （数字，无单位）—— 紧插在高度行与表速行之间，不留间隙
    let thr_w = aoa_w;
    let thr_h = (aoa_h * 0.5).max(1.0);
    let row_t_y = y - (font_sz + line_gap) * 0.5 - small_sz * 0.5;
    let throttle = data.engine_throttle[0].clamp(0.0, 110.0);
    let thr_color = colors.data_u32();
    // 油门数字：> 100（加力段）以告警色（黄）显示；条本身保持正常色
    let thr_text_color = if throttle > 100.0 { colors.hint_u32() } else { thr_color };
    draw_hbar(
        buffer, width, height,
        x as i32,
        (row_t_y - thr_h / 2.0) as i32,
        thr_w as i32, thr_h as i32,
        (throttle / 110.0) as f32, thr_color, colors.unit_u32(), false,
    );
    draw_mt(buffer, width, height, font, x + thr_w + gap, row_t_y, &format!("{:.0}", throttle), thr_text_color, small_sz);

    // Row 2: IAS number (font_sz) + Mach number (small_sz, no label) at 9 o'clock center
    let ias_text = format!("{:>4}", data.ias as i32);
    let ias_vw = measure_text_raw(font, &ias_text, font_sz);
    draw_mt(buffer, width, height, font, x, y, &ias_text, ias_color, font_sz);
    draw_mt(buffer, width, height, font, x + ias_vw + gap, y, &format!("{:>5.2}", data.mach), mach_color, small_sz);

    // Row 3: AoA bar + α (small_sz)
    let aoa_avail = data.max_aoa - data.aoa;
    let aoa_ratio = (aoa_avail / data.max_aoa.max(1.0)).clamp(0.0, 1.0) as f32;
    let aoa_color = if aoa_avail < data.max_aoa * 0.2 {
        colors.alert_u32()
    } else if aoa_avail < data.max_aoa * 0.4 {
        colors.hint_u32()
    } else {
        colors.data_u32()
    };
    let row2_y = y + font_sz + line_gap;
    let aoa_text = format!("{:.1}", data.aoa);
    let bar_right = x + aoa_w;
    draw_hbar(
        buffer, width, height,
        bar_right as i32,
        (row2_y - aoa_h / 2.0) as i32,
        aoa_w as i32, aoa_h as i32,
        aoa_ratio, aoa_color, colors.unit_u32(), true,
    );
    draw_mt(buffer, width, height, font, x + aoa_w + gap, row2_y, &format!("α {}", aoa_text), aoa_color, small_sz);

    // Row 4: status items (BRK, GEA, F, W)
    let row3_y = row2_y + font_sz + line_gap;
    let items = crate::hud_common::status_items(data, colors);
    if !items.is_empty() {
        crate::hud_common::draw_inline_items(buffer, width, height, font, x, row3_y, &items, font_sz);
    }
}

fn draw_sep_gload(
    buffer: &mut [u32], width: usize, height: usize, font: &ab_glyph::FontRef,
    cx: f32, cy: f32, data: &DisplayData, colors: &crate::components::HudColors,
    font_sz: f32, small_sz: f32, line_gap: f32, _col_w: f32, outer_r: f32,
) {
    let (x, y) = clock_pos(cx, cy, 9.0, outer_r);
    // SEP 两级告警（≤0 黄，< -300 红）
    let sep_color = crate::hud_common::warning_level_color(
        crate::hud_common::sep_warning_level(data.sep),
        colors.data_u32(),
    );

    // Row 0: E (small_sz), right-aligned above center (same y as altitude)
    let row0_y = y - (font_sz + line_gap);
    let e_text = format!("E{:>9.0}", data.energy_height);
    let tw = measure_text_raw(font, &e_text, small_sz);
    draw_mt(buffer, width, height, font, x - tw, row0_y, &e_text, colors.data_u32(), small_sz);

    // Row 1: SEP (font_sz), right-aligned at 9 o'clock center
    let sep_text = format!("SEP{:>+5.0}", data.sep as i32);
    let tw = measure_text_raw(font, &sep_text, font_sz);
    draw_mt(buffer, width, height, font, x - tw, y, &sep_text, sep_color, font_sz);

    // Row 2: G (font_sz), right-aligned below center (same y as AoA)
    // 过载统一告警逻辑：≥90% 允许过载时红色
    let g_color = crate::hud_common::g_warning_color(data.g_load_warning, colors.data_u32());
    let g_text = format!("G{:>+7.1}", data.ny);
    let tw = measure_text_raw(font, &g_text, font_sz);
    draw_mt(buffer, width, height, font, x - tw, y + font_sz + line_gap, &g_text, g_color, font_sz);
}

fn draw_fuel(
    buffer: &mut [u32], width: usize, height: usize, font: &ab_glyph::FontRef,
    cx: f32, cy: f32, data: &DisplayData, colors: &crate::components::HudColors,
    font_sz: f32, small_sz: f32, _line_gap: f32, outer_r: f32,
) {
    let (x, y) = clock_pos(cx, cy, 3.0, outer_r);
    let fuel_part = format!("L {}", data.fuel_time_text.as_str());
    let tw_part = data.thrust_to_weight_text.as_str().to_string();
    let fw = measure_text_raw(font, &fuel_part, font_sz);
    let gap = font_sz * 0.5;
    draw_mt(buffer, width, height, font, x, y, &fuel_part, colors.data_u32(), font_sz);
    draw_mt(buffer, width, height, font, x + fw + gap, y, &tw_part, colors.data_u32(), small_sz);
}

fn draw_heading_zone(
    buffer: &mut [u32], width: usize, height: usize, font: &ab_glyph::FontRef,
    cx: f32, cy: f32, data: &DisplayData, map: &MapDisplay, colors: &crate::components::HudColors,
    font_sz: f32, _small_sz: f32, _line_gap: f32, outer_r: f32, clock: f32,
) {
    let (x, y) = clock_pos(cx, cy, clock, outer_r);
    let y = y + font_sz;
    let heading_text = format!("{:>3.0}°", data.heading);
    let heading_w = measure_text_raw(font, &heading_text, font_sz);
    draw_mt(buffer, width, height, font, x - heading_w / 2.0, y, &heading_text, colors.data_u32(), font_sz);
    let zone = String::from_utf8_lossy(map.map_zone_text.as_bytes());
    if !zone.trim().is_empty() {
        let gap = font_sz * 0.5;
        draw_mt(buffer, width, height, font, x - heading_w / 2.0 + heading_w + gap, y, &zone, colors.hint_u32(), font_sz);
    }
}

fn draw_poi(
    buffer: &mut [u32], width: usize, height: usize, font: &ab_glyph::FontRef,
    cx: f32, cy: f32, map: &MapDisplay, colors: &crate::components::HudColors,
    font_sz: f32, small_sz: f32, _line_gap: f32, radius: f32, clock: f32,
) {
    let (x, y) = clock_pos(cx, cy, clock, radius);
    if map.nearest_poi_dist >= f64::MAX {
        return;
    }
    let poi_dist = map.nearest_poi_dist / 1000.0;
    let rel_bearing_adj = map.nearest_poi_bearing - 180.0;
    // POI 文字用**警示色 hint_color**（原 poi_color 配置键已删除）
    let (prefix, poi_ink) = if rel_bearing_adj.abs() > 177.5 {
        ("-", colors.poi_u32())
    } else if rel_bearing_adj > 0.0 {
        ("←", colors.poi_u32())
    } else {
        ("→", colors.poi_u32())
    };
    let poi_text = format!("{:<2}{:>3.0}", prefix, poi_dist.round());
    let speed_text = format!("{:+.0}", map.nearest_poi_rel_speed);
    let poi_w = measure_text_raw(font, &poi_text, font_sz);
    let speed_w = measure_text_raw(font, &speed_text, small_sz);
    let gap = font_sz * 0.5;
    let total_w = poi_w + gap + speed_w;
    let start_x = x - total_w / 2.0;
    draw_mt(buffer, width, height, font, start_x, y, &poi_text, poi_ink, font_sz);
    draw_mt(buffer, width, height, font, start_x + poi_w + gap, y, &speed_text, colors.poi_u32(), small_sz);
}
