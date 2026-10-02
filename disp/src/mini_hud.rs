use super::{draw_text_raw, measure_text_raw, set_pixel, FONT, HUD_COLORS};
use crate::MapDisplay;

const SMALL_FONT_RATIO: f32 = 0.707;

macro_rules! text {
    ($data:expr, $field:ident) => {
        String::from_utf8_lossy($data.$field.as_bytes())
    };
}

pub struct MiniHudPosition {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// 量出 miniHUD **相对锚点往左 / 往上多画出去多少**（比例：每 1px mini 字号对应多少 px）。
///
/// `init_hud_layout` 调一次，交给 [`crate::geometry::set_mini_hud_pads`]；
/// 擦除与拖拽命中据此把矩形外扩，避免"框外的像素擦不掉"（用户报的上边缘/右边缘残影）。
pub fn measure_pads(font: &ab_glyph::FontRef, mini_font_size: f32) -> (f32, f32) {
    use ab_glyph::Font;
    let size = mini_font_size.max(1.0);
    // 左侧：油门数字列 + 它下面的标尺线（`draw_throttle_vertical` 用 char_w * 3）
    let char_w = measure_text_raw(font, "0", size * SMALL_FONT_RATIO);
    let left = (char_w * 3.0 + 2.0) / size;
    // 上侧：首行基线在"锚点 + 0.5×字号"，字形的上伸部还要再高一点
    let ascent = font.ascent_unscaled() * size / font.height_unscaled();
    let top = ((ascent - 0.5 * size).max(0.0) + 1.0) / size;
    (left, top)
}

/// 画 miniHUD。`px`/`py` 是**面板左上角**，由调用方按
/// [`crate::geometry::hud_anchor`] 解析好（含"0 = 自动居中"的回退）——
/// 这里不再读配置，免得绘制与命中测试各有一套位置口径。
pub fn draw_mini_hud(buffer: &mut [u32], width: usize, height: usize, data: &super::DisplayData, map: &MapDisplay, px: f32, py: f32) {
    let cfg = crate::hud_layout();
    let font_size = crate::geometry::mini_hud_font(cfg);
    let (w, h) = crate::geometry::mini_hud_size(cfg);
    let pos = MiniHudPosition { x: px, y: py, width: w, height: h };
    let colors = &HUD_COLORS;

    FONT.with(|font| {
        let start_x = pos.x;
        let start_y = pos.y + 0.5 * font_size;
        let line_spacing = font_size;
        let info_x = start_x + font_size;
        let throttle_h = 5.0 * font_size;

        // Vertical throttle bar with number on left and underline below at fill level
        let throttle_x = start_x;
        debug_assert!(data.engine_count > 0);
        let throttle = data.engine_throttle[0].max(0.0).min(110.0);
        let throttle_color = colors.data_u32();
        draw_throttle_vertical(
            buffer,
            width,
            height,
            font,
            throttle_x,
            start_y,
            font_size * 0.2718,
            throttle_h,
            font_size,
            throttle,
            throttle_color,
            colors.unit_u32(),
        );

        // Line 0: SPD
        // Two-stage overspeed warning (yellow at 95% VNE, red at 97.5%) plus the
        // speed-brake caution (yellow while the brake is deployed).
        let spd_color = crate::hud_common::ias_warning_color(data, colors.data_u32());
        draw_mini_info_line_right_align(
            buffer,
            width,
            height,
            font,
            info_x,
            start_y,
            "I",
            &text!(data, ias_text),
            spd_color,
            font_size,
        );

        // Mach number: colored by its own MNE stage, shown near/over the Mach
        // limit or once it enters a warning stage.
        if data.mach >= 0.95 || data.mach_warning > 0 {
            let mach_text = format!("M{:.2}", data.mach);
            let mach_color = crate::hud_common::warning_level_color(data.mach_warning, colors.data_u32());
            draw_mini_text(
                buffer,
                width,
                height,
                font,
                info_x + 4.0 * font_size,
                start_y,
                &mach_text,
                mach_color,
                font_size,
            );
        }

        // AoA bar below SPD
        let aoa_bar_x = info_x;
        let aoa_bar_y = start_y + line_spacing;
        let aoa_bar_w = 3.25 * font_size;
        let aoa_available = if data.valid == 1 && data.max_aoa > 0.0 { data.max_aoa - data.aoa } else { 0.0 };
        let aoa_ratio = if data.max_aoa > 0.0 { (aoa_available / data.max_aoa).clamp(0.0, 1.0) } else { 1.0 };
        let aoa_color = if data.valid != 1 || aoa_available < data.max_aoa * 0.2 {
            colors.alert_u32()
        } else if aoa_available < data.max_aoa * 0.4 {
            colors.hint_u32()
        } else {
            colors.data_u32()
        };
        draw_aoa_bar_right_to_left(
            buffer,
            width,
            height,
            (aoa_bar_x + aoa_bar_w) as i32,
            (aoa_bar_y - font_size * 0.33) as i32,
            aoa_bar_w as i32,
            (font_size * 0.2718) as i32,
            aoa_ratio as f32,
            aoa_color,
            colors.unit_u32(),
        );
        let aoa_val = data.aoa as i32;
        let aoa_text = format!("α{:>3}", aoa_val);
        draw_mini_text(
            buffer,
            width,
            height,
            font,
            info_x + 4.0 * font_size,
            aoa_bar_y - font_size * 0.07,
            &aoa_text,
            aoa_color,
            font_size * SMALL_FONT_RATIO,
        );

        // Line 1: ALT
        let alt_y = start_y + line_spacing * 2.0;
        let alt_color =
            if data.valid == 1 && (data.altitude < 58.0 || (data.radio_altitude > 0.0 && data.radio_altitude < 58.0)) {
                colors.alert_u32()
            } else {
                colors.data_u32()
            };
        if data.radio_altitude > 0.0 && data.radio_altitude <= 500.0 {
            draw_mini_info_line_right_align(
                buffer,
                width,
                height,
                font,
                info_x,
                alt_y,
                " ",
                &format!("{:.0}R", data.radio_altitude),
                alt_color,
                font_size,
            );
        } else {
            draw_mini_info_line_right_align(
                buffer,
                width,
                height,
                font,
                info_x,
                alt_y,
                " ",
                &text!(data, altitude_text),
                alt_color,
                font_size,
            );
        }
        draw_mini_text(buffer, width, height, font,
            info_x + 4.0 * font_size, alt_y, &text!(data, energy_height_text), colors.data_u32(), font_size * SMALL_FONT_RATIO);

        // Line 2: Config (flaps, wing, brake, gear)
        let config_y = start_y + line_spacing * 3.0;
        let config_items = crate::hud_common::status_items(data, colors);
        if !config_items.is_empty() {
            crate::hud_common::draw_inline_items(buffer, width, height, font, info_x, config_y, &config_items, font_size);
        }

        // Line 3: SEP + T/W（≤0 黄，< -300 红）
        let sep_y = start_y + line_spacing * 4.0;
        let sep_color = crate::hud_common::warning_level_color(
            crate::hud_common::sep_warning_level(data.sep),
            colors.data_u32(),
        );

        let sep_val_text = text!(data, sep_text);
        let sep_x = info_x;
        draw_mini_info_line_right_align(
            buffer,
            width,
            height,
            font,
            sep_x,
            sep_y,
            " ",
            &sep_val_text,
            sep_color,
            font_size,
        );

        let tw_text = text!(data, thrust_to_weight_text).to_string();
        let tw_x = sep_x + 4.0 * font_size;
        draw_mini_text(
            buffer,
            width,
            height,
            font,
            tw_x,
            sep_y,
            &tw_text,
            colors.data_u32(),
            font_size * SMALL_FONT_RATIO,
        );

        // Line 4: G or Elapsed Time
        let line4_y = start_y + line_spacing * 5.0;
        if data.ny.abs() > 1.5 {
            // 过载统一告警逻辑：≥90% 允许过载时红色
            let g_color = crate::hud_common::g_warning_color(data.g_load_warning, colors.data_u32());
            draw_mini_info_line_right_align(
                buffer,
                width,
                height,
                font,
                info_x,
                line4_y,
                "G",
                &format!("{:.1}", data.ny),
                g_color,
                font_size,
            );
        } else {
            draw_mini_info_line_right_align(
                buffer,
                width,
                height,
                font,
                info_x,
                line4_y,
                "T",
                &text!(data, elapsed_time_text),
                colors.data_u32(),
                font_size,
            );
        }

        // POI distance on right side of G/T line, aligned with afterburner fuel time
        if map.nearest_poi_dist < f64::MAX {
            let poi_dist = map.nearest_poi_dist / 1000.0;
            let poi_bearing = map.nearest_poi_bearing;
            let rel_bearing_adj = poi_bearing - 180.0;
            let ahead = |b: f64| -> bool { b.abs() > 177.5 };
            let (prefix, color) = if ahead(rel_bearing_adj) {
                ("-", colors.poi_u32())
            } else if rel_bearing_adj > 0.0 {
                if poi_bearing > 355.0 { ("<", colors.poi_u32()) } else { ("←", colors.poi_u32()) }
            } else {
                if poi_bearing < 5.0 { (">", colors.poi_u32()) } else { ("→", colors.poi_u32()) }
            };
            let poi_text = format!("{:<2}{:>3.0}", prefix, poi_dist.round());
            let poi_text_w = measure_text_raw(font, &poi_text, font_size);
            let speed_text = format!(" {:+.0}", map.nearest_poi_rel_speed);
            let speed_text_w = measure_text_raw(font, &speed_text, font_size * 0.75);
            // 这一段是往右长出去的：**夹在面板框内**（信息列右边界 = 锚点 + 面板宽），
            // 免得又画到擦除框外边去（框外的像素擦不掉，会在拖动后留残影）。
            let poi_x = (info_x + 4.0 * font_size)
                .min(pos.x + pos.width - poi_text_w - speed_text_w - 2.0)
                .max(pos.x);
            draw_mini_text(
                buffer,
                width,
                height,
                font,
                poi_x,
                line4_y,
                &poi_text,
                color,
                font_size,
            );
            draw_mini_text(
                buffer,
                width,
                height,
                font,
                poi_x + poi_text_w,
                line4_y + font_size * 0.125,
                &speed_text,
                colors.poi_u32(),
                font_size * 0.75,
            );
        }

        // Line 5: Fuel time + Afterburner fuel time
        let fuel_time_y = start_y + line_spacing * 6.0;
        draw_mini_info_line_right_align(
            buffer,
            width,
            height,
            font,
            info_x,
            fuel_time_y,
            "L",
            &text!(data, fuel_time_text),
            colors.data_u32(),
            font_size,
        );

        let abft_text = text!(data, afterburner_fuel_time_text);
        draw_mini_text(
            buffer,
            width,
            height,
            font,
            info_x + 4.0 * font_size,
            fuel_time_y,
            &abft_text,
            colors.data_u32(),
            font_size,
        );

        // Voice alarm overlay: flash a green X over the whole miniHUD at 4 Hz
        crate::hud_common::draw_voice_alarm_x(
            buffer,
            width,
            height,
            data,
            pos.x,
            pos.y,
            pos.width,
            pos.height,
            font_size,
        );
    });
}

fn draw_throttle_vertical(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    font: &ab_glyph::FontRef,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    font_size: f32,
    throttle: f64,
    throttle_color: u32,
    border_color: u32,
) {
    let throttle_text = format!("{:.0}", throttle);

    let fill_ratio = (throttle / 110.0).clamp(0.0, 1.0) as f32;

    let bar_x = x as i32;
    let bar_top_y = y as i32;
    let bar_bottom_y = (y + h) as i32;
    let bar_height_px = h as i32;
    let bar_w = w as i32;

    let fill_h_px = (fill_ratio * bar_height_px as f32) as i32;
    let scale_line_y = bar_bottom_y - fill_h_px;

    let bg_color = 0x40000000u32;

    for dy in 0..bar_height_px {
        let py = bar_top_y + dy;
        for dx in 0..bar_w {
            set_pixel(buffer, buf_width, buf_height, bar_x + dx, py, bg_color);
        }
    }

    for dy in 0..fill_h_px {
        let py = bar_bottom_y - dy;
        for dx in 0..bar_w {
            set_pixel(buffer, buf_width, buf_height, bar_x + dx, py, throttle_color);
        }
    }

    for dx in 0..bar_w {
        set_pixel(buffer, buf_width, buf_height, bar_x + dx, bar_bottom_y, border_color);
    }

    for dy in 0..bar_height_px {
        let py = bar_top_y + dy;
        if py < 0 || py >= buf_height as i32 {
            continue;
        }
        for dx in [0, bar_w - 1] {
            let px = bar_x + dx;
            if px >= 0 && px < buf_width as i32 {
                let idx = (py as usize) * buf_width + (px as usize);
                if idx < buffer.len() {
                    buffer[idx] = border_color;
                }
            }
        }
    }

    for dx in 0..bar_w {
        let px = bar_x + dx;
        let py = scale_line_y;
        if px >= 0 && px < buf_width as i32 && py >= 0 && py < buf_height as i32 {
            let idx = (py as usize) * buf_width + (px as usize);
            if idx < buffer.len() {
                buffer[idx] = border_color;
            }
        }
    }

    let char_w = measure_text_raw(font, "0", font_size * SMALL_FONT_RATIO) as i32;
    let num_x = bar_x;
    let num_y = scale_line_y;
    draw_text_raw(
        buffer,
        buf_width,
        buf_height,
        font,
        (num_x - char_w * 3) as f32,
        num_y as f32,
        &throttle_text,
        font_size * SMALL_FONT_RATIO,
        throttle_color,
    );

    let line_y = scale_line_y;
    let line_w = char_w * 3;
    for dx in 0..line_w {
        let px = bar_x - line_w + dx;
        if px >= 0 && px < buf_width as i32 && line_y >= 0 && line_y < buf_height as i32 {
            let idx = (line_y as usize) * buf_width + (px as usize);
            if idx < buffer.len() {
                buffer[idx] = border_color;
            }
        }
    }
}

fn draw_aoa_bar_right_to_left(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    ratio: f32,
    fill_color: u32,
    border_color: u32,
) {
    let fill_w = (w as f32 * ratio) as i32;

    for dy in 0..h {
        for dx in 0..w {
            let px = x - dx;
            let py = y + dy;
            if px >= 0 && px < buf_width as i32 && py >= 0 && py < buf_height as i32 {
                let idx = (py as usize) * buf_width + (px as usize);
                if idx < buffer.len() {
                    let on_border = dy == 0 || dy == h - 1;
                    buffer[idx] = if on_border {
                        border_color
                    } else if dx < fill_w {
                        fill_color
                    } else {
                        0x40000000
                    };
                }
            }
        }
    }
}

fn draw_mini_info_line_right_align(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    font: &ab_glyph::FontRef,
    x: f32,
    y: f32,
    label: &str,
    value: &str,
    color: u32,
    font_size: f32,
) {
    let label_width = measure_text_raw(font, label, font_size);
    draw_text_raw(
        buffer, buf_width, buf_height, font, x, y, label, font_size, color,
    );
    let value_width = measure_text_raw(font, value, font_size);
    let field_width = 3.0 * font_size;
    let value_x = x + label_width + field_width - value_width;
    draw_text_raw(
        buffer, buf_width, buf_height, font, value_x, y, value, font_size, color,
    );
}

fn draw_mini_text(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    font: &ab_glyph::FontRef,
    x: f32,
    y: f32,
    text: &str,
    color: u32,
    font_size: f32,
) {
    draw_text_raw(
        buffer, buf_width, buf_height, font, x, y, text, font_size, color,
    );
}


