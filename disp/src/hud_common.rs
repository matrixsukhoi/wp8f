use crate::components::HudColors;
use crate::draw::{draw_line, draw_text_raw, measure_text_raw};
use crate::{DisplayData, MapDisplay};
use std::sync::OnceLock;
use std::time::Instant;

/// SEP 危险阈值（m/s）：SEP 低于该值变红（miniHUD / circle HUD 共用）。
pub const SEP_ALERT_THRESHOLD: f64 = -300.0;

/// SEP 告警级别：`0` 正常，`1` ≤ 0（黄），`2` < `SEP_ALERT_THRESHOLD`（红）。
pub fn sep_warning_level(sep: f64) -> u8 {
    if sep < SEP_ALERT_THRESHOLD {
        2
    } else if sep <= 0.0 {
        1
    } else {
        0
    }
}

/// 由归一化地图坐标推导地图区域格（如 "D3"），与游戏地图网格一致：
/// `abs = norm × maxsize + map_min`，列 = (abs_x - grid_zero_x)/grid_steps_x + 1，
/// 行 = (abs_y + grid_zero_y)/grid_steps_y → 行字母 'A' + 行号。单一实现，
/// core 的玩家区域与友军面板区域都走这里。
pub fn map_zone_label(
    x_norm: f64,
    y_norm: f64,
    maxsize: [f64; 2],
    map_min: [f64; 2],
    grid_zero: [f64; 2],
    grid_steps: [f64; 2],
) -> String {
    if grid_steps[0] == 0.0 || grid_steps[1] == 0.0 || maxsize[0] == 0.0 || maxsize[1] == 0.0 {
        return String::new();
    }
    let abs_x = x_norm * maxsize[0] + map_min[0];
    let abs_y = y_norm * maxsize[1] + map_min[1];
    let col = ((abs_x - grid_zero[0]) / grid_steps[0] + 1.0).floor() as i32;
    let row = ((abs_y + grid_zero[1]) / grid_steps[1]).floor() as i32;
    let letter = (b'A'.wrapping_add(row as u8)) as char;
    format!("{}{}", letter, col.max(1).min(20))
}

/// `map_zone_label` 的 MapDisplay 便捷版（友军面板使用）。
pub fn map_zone_of(map: &MapDisplay, x: f64, y: f64) -> String {
    map_zone_label(x, y, map.map_maxsize, map.map_min, map.grid_zero, map.grid_steps)
}

/// 告警 X 的闪烁闸门：`warning_blink_hz` 次/秒（默认 4 Hz、50% 占空比）；
/// `<= 0` 表示不闪、告警期间常亮。按墙钟计时，与渲染帧率无关。
pub fn warning_blink_visible() -> bool {
    static BLINK_EPOCH: OnceLock<Instant> = OnceLock::new();
    let hz = crate::hud_layout().warning_blink_hz;
    if hz <= 0.0 {
        return true;
    }
    let epoch = BLINK_EPOCH.get_or_init(Instant::now);
    let period_ms = 1000.0 / hz.min(60.0) as f64;
    let phase_ms = epoch.elapsed().as_secs_f64() * 1000.0 % period_ms;
    phase_ms < period_ms * 0.5
}

/// Map a warning level to its color: `0` = normal, `1` = yellow (hint),
/// `2` = red (alert).
pub fn warning_level_color(level: u8, normal: u32) -> u32 {
    match level {
        2 => crate::HUD_COLORS.alert_u32(),
        1 => crate::HUD_COLORS.hint_u32(),
        _ => normal,
    }
}


/// 过载颜色：`g_load_warning` 非 0（|Ny| ≥ 90% 允许过载）→ 红色告警，否则正常。
/// 各 panel 的过载着色统一走这里（判据见 core 的 `g_load_warning_level`）。
pub fn g_warning_color(level: u8, normal: u32) -> u32 {
    if level > 0 {
        crate::HUD_COLORS.alert_u32()
    } else {
        normal
    }
}


/// Combined IAS warning level: two-stage overspeed (yellow at 95% VNE, red at
/// 97.5%) raised by the speed-brake caution (yellow at most). `0` while data
/// is invalid. Single source for every IAS color decision.
pub fn ias_warning_level(data: &DisplayData) -> u8 {
    if data.valid != 1 {
        return 0;
    }
    let brake_level = if data.brake_caution { 1 } else { 0 };
    data.overspeed_warning.max(brake_level)
}

/// IAS display color for `ias_warning_level`.
pub fn ias_warning_color(data: &DisplayData, normal: u32) -> u32 {
    warning_level_color(ias_warning_level(data), normal)
}

/// 图标字形（`icons.ttf`）的**观感补偿**：绘制前把颜色按旧口径预乘一次。
///
/// 图标颜色是代码里写死的（不如文字色那样能在配置里补偿），字形管线按覆盖率衰减 RGB+alpha 后
/// 会比旧观感亮一档，所以这里施加与文字色完全相同的补偿：
/// ```text
/// 新 RGB = 旧 RGB × 旧 A / 255        新 A = 旧 A × 旧 A / 255
/// ```
/// 满覆盖像素与旧管线逐位相同。`A = 0xFF` 时是恒等变换（本来就不透明的颜色不必区别对待）。
///
/// ⚠️ 这是**有意补偿，不是忘了去掉的 alpha 预乘**：图元 / 圆弧 / 地图对象走
/// [`blend_pixel_cov`]（直通 alpha、RGB 不预乘），两条路分开，不要统一。细节见 `draw/glyph_cache.rs`。
pub fn legacy_glyph_color(color: u32) -> u32 {
    let a = (color >> 24) & 0xFF;
    let ch = |shift: u32| (((color >> shift) & 0xFF) * a) / 255;
    let (r, g, b) = (ch(16), ch(8), ch(0));
    let na = (a * a) / 255;
    (na << 24) | (r << 16) | (g << 8) | b
}

/// 按覆盖率做 straight-alpha "over" 混合（不用 `set_pixel_blend`：它会强制输出不透明 alpha）。
/// `cov` ∈ [0,1]。
///
/// **图元 / 圆弧 / 地图对象走这里**：覆盖率只写进 alpha、RGB 保持原值不预乘，
/// 满覆盖（背景透明）时落屏就是配置色。
///
/// ⚠️ **字形不走这里**（走 `draw::glyph_cache::blit_cached_glyph`）：本函数在透明背景上写出的
/// RGB 与覆盖率无关，字的边缘会和字心一样实、看起来变粗变硬。要动字形先读那边的注释，
/// 别"顺手统一"；配置里 5 个文字色的补偿值与字形的覆盖率衰减是一对，别只改一半。
pub fn blend_pixel_cov(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    x: i32,
    y: i32,
    color: u32,
    cov: f32,
) {
    if x < 0 || y < 0 || x >= buf_width as i32 || y >= buf_height as i32 {
        return;
    }
    let idx = y as usize * buf_width + x as usize;
    if idx >= buffer.len() {
        return;
    }
    let fa = ((color >> 24) & 0xFF) as f32 / 255.0 * cov;
    if fa <= 0.0 {
        return;
    }
    let bg = buffer[idx];
    let ba = ((bg >> 24) & 0xFF) as f32 / 255.0;
    let out_a = fa + ba * (1.0 - fa);
    let mix = |f: u32, b: u32| -> u32 {
        let fv = (f & 0xFF) as f32;
        let bv = (b & 0xFF) as f32;
        let v = if out_a > 0.0 {
            (fv * fa + bv * ba * (1.0 - fa)) / out_a
        } else {
            0.0
        };
        v.round().clamp(0.0, 255.0) as u32
    };
    let r = mix((color >> 16) & 0xFF, (bg >> 16) & 0xFF);
    let g = mix((color >> 8) & 0xFF, (bg >> 8) & 0xFF);
    let b = mix(color & 0xFF, bg & 0xFF);
    let a = (out_a * 255.0).round() as u32;
    buffer[idx] = (a << 24) | (r << 16) | (g << 8) | b;
}

/// 抗锯齿圆弧带（3×3 超采样覆盖率 + straight-alpha over，与玩家箭头同一口径）。
/// 在半径 `radius`、厚 `thickness` 的径向带内绘制角度区间 `[start_deg, end_deg]`。
/// 角度约定与 `sincos` 一致：0°=+x、90°=+y（屏幕向下）；区间可跨 0°、可超 360°。
/// `clip` 为半开裁剪矩形 (x0, y0, x1, y1)。
pub fn draw_arc_band_aa(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    cx: f32,
    cy: f32,
    radius: f32,
    thickness: f32,
    color: u32,
    start_deg: f32,
    end_deg: f32,
    clip: Option<(i32, i32, i32, i32)>,
) {
    if radius <= 0.0 || thickness <= 0.0 {
        return;
    }
    let half = thickness * 0.5;
    // 逐像素粗筛（平方距离，无 sqrt）：环带外像素直接跳过，只对候选像素超采样
    let r_in2 = (radius - half - 1.0).max(0.0).powi(2);
    let r_out2 = (radius + half + 1.0).powi(2);
    let x0 = (cx - radius - half - 1.0).floor() as i32;
    let x1 = (cx + radius + half + 1.0).ceil() as i32;
    let y0 = (cy - radius - half - 1.0).floor() as i32;
    let y1 = (cy + radius + half + 1.0).ceil() as i32;
    let full = end_deg - start_deg >= 360.0;
    let mid = (start_deg + end_deg) * 0.5;

    for py in y0..=y1 {
        for px in x0..=x1 {
            if let Some((cx0, cy0, cx1, cy1)) = clip {
                if px < cx0 || px >= cx1 || py < cy0 || py >= cy1 {
                    continue;
                }
            }
            let dx = px as f32 + 0.5 - cx;
            let dy = py as f32 + 0.5 - cy;
            let r2 = dx * dx + dy * dy;
            if r2 < r_in2 || r2 > r_out2 {
                continue;
            }
            let mut hit = 0u32;
            for sy in 0..3 {
                for sx in 0..3 {
                    let fx = px as f32 + (sx as f32 + 0.5) / 3.0 - cx;
                    let fy = py as f32 + (sy as f32 + 0.5) / 3.0 - cy;
                    let rr = (fx * fx + fy * fy).sqrt();
                    if (rr - radius).abs() > half {
                        continue;
                    }
                    if !full {
                        // 样本角平移到离区间中点最近的等价角（处理跨 0°）
                        let mut a = fy.atan2(fx).to_degrees();
                        a += ((mid - a) / 360.0).round() * 360.0;
                        if a < start_deg || a > end_deg {
                            continue;
                        }
                    }
                    hit += 1;
                }
            }
            if hit > 0 {
                blend_pixel_cov(buffer, buf_width, buf_height, px, py, color, hit as f32 / 9.0);
            }
        }
    }
}

/// Draw the warning X across the given rectangle (data color strokes, dark outline).
/// `stroke_ref` is the owning HUD's font size and scales the stroke thickness.
pub fn draw_warning_x(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    stroke_ref: f32,
) {
    let x1 = x as i32;
    let y1 = y as i32;
    let x2 = (x + w) as i32;
    let y2 = (y + h) as i32;

    // 告警 X 用**数字色 data_color**（配置键 `warning_x_color` 已删除：整台 HUD 只有一套数字色）
    let color = crate::HUD_COLORS.data_u32();
    let stroke = crate::HUD_COLORS.stroke_u32();

    // Stroke rings first, colored rings last: where the diagonal families
    // cross, the X color must win. The outermost ring keeps at least a 1px
    // dark outline even for small fonts (inner is derived from outer).
    let outer = (stroke_ref / 15.0).round().max(1.0) as i32;
    let inner = (outer - 1).max(0);

    for offset in -outer..=outer {
        if offset.abs() <= inner {
            continue;
        }
        draw_x_ring(buffer, buf_width, buf_height, x1, y1, x2, y2, offset, stroke);
    }
    for offset in -inner..=inner {
        draw_x_ring(buffer, buf_width, buf_height, x1, y1, x2, y2, offset, color);
    }
}

/// Draw the four arms of the X as one parallel ring shifted by `offset`.
fn draw_x_ring(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    offset: i32,
    color: u32,
) {
    draw_line(buffer, buf_width, buf_height, x1 + offset, y1, x2 + offset, y2, color);
    draw_line(buffer, buf_width, buf_height, x1, y1 + offset, x2, y2 + offset, color);
    draw_line(buffer, buf_width, buf_height, x2 + offset, y1, x1 + offset, y2, color);
    draw_line(buffer, buf_width, buf_height, x2, y1 + offset, x1, y2 + offset, color);
}

/// miniHUD / circle HUD alarm overlay: while a voice alarm is active
/// (`DisplayData::voice_alarm`), flash a green X over the HUD at
/// `warning_blink_hz` (default 4 Hz).
pub fn draw_voice_alarm_x(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    data: &DisplayData,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    stroke_ref: f32,
) {
    if data.valid == 1 && data.voice_alarm && warning_blink_visible() {
        draw_warning_x(buffer, buf_width, buf_height, x, y, w, h, stroke_ref);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 混色只有一个实现（`blend_pixel_cov`）：通道顺序与 alpha 语义必须固定住
    /// （R 在 R 位、B 在 B 位、半透明色不被丢弃）。
    #[test]
    fn blend_pixel_cov_keeps_channels_and_alpha() {
        let mut buf = vec![0u32];
        // 半透明红（straight alpha）画到空缓冲上：R 就该在 R 位、B=0
        blend_pixel_cov(&mut buf, 1, 1, 0, 0, 0x80FF_0000, 1.0);
        let px = buf[0];
        assert_eq!((px >> 16) & 0xFF, 0xFF, "R 通道被换位了");
        assert_eq!(px & 0xFF, 0x00, "B 通道被换位了");
        assert!(((px >> 24) & 0xFF) >= 0x7E && ((px >> 24) & 0xFF) <= 0x82,
                "alpha 应约等于 0x80，实际 {:#x}", px >> 24);

        // 画到蓝色背景上：蓝背景的 B 不能被换到 R 位
        let mut buf2 = vec![0x0000_00FFu32];
        blend_pixel_cov(&mut buf2, 1, 1, 0, 0, 0x0000_00FF, 1.0);
        assert_eq!(buf2[0] & 0xFF, 0xFF, "纯蓝叠加后 B 应仍为 FF");
        assert_eq!((buf2[0] >> 16) & 0xFF, 0x00, "纯蓝叠加后 R 应为 0");
    }

    /// 这个实现是 straight alpha：覆盖率只写进 alpha，RGB 保持原色不变。
    /// ⚠️ 字形**不走这里**（按覆盖率同时衰减 RGB+alpha，见 `draw/glyph_cache.rs`）；
    /// 配置里 5 个文字色的补偿值与那套衰减配套，别把配置"修正"回亮色。
    #[test]
    fn blend_pixel_cov_respects_coverage() {
        let mut buf = vec![0x0000_0000u32];
        blend_pixel_cov(&mut buf, 1, 1, 0, 0, 0xFF_FFFF_FF, 0.5);
        let px = buf[0];
        for shift in [16, 8, 0] {
            assert_eq!((px >> shift) & 0xFF, 0xFF, "straight alpha 下 RGB 不应被覆盖率压暗");
        }
        assert!(((px >> 24) & 0xFF as u32) == 0x7F || ((px >> 24) & 0xFF) == 0x80,
                "alpha 应约 0x80，实际 {:#x}", px >> 24);
    }

    /// 旧（改造前）字形混合，**逐行复刻**自 `0ea99f74:disp/src/draw/glyph_cache.rs`：
    ///   `fa = cov×A/255`；`out_ch = (src_ch×fa + bg_ch×(255-fa))/255`；alpha 同式。
    /// 图标补偿就是对着这条老管线验的（文字色那条在 `glyph_cache.rs` 里同口径）。
    fn old_glyph_pixel(color: u32, cov: u32, bg: u32) -> u32 {
        let src_a = (color >> 24) & 0xFF;
        let fa = cov * src_a / 255;
        if fa == 0 {
            return bg;
        }
        let ba = 255 - fa;
        let ch = |shift: u32| (((color >> shift) & 0xFF) * fa + ((bg >> shift) & 0xFF) * ba) / 255;
        let a_ch = (src_a * fa + ((bg >> 24) & 0xFF) * ba) / 255;
        (a_ch & 0xFF) << 24 | (ch(16) & 0xFF) << 16 | (ch(8) & 0xFF) << 8 | (ch(0) & 0xFF)
    }

    /// 1×1 字形走**真正的**字形管线（`draw::glyph_cache::blit_cached_glyph`）。
    fn blit_one(name: &str, color: u32, cov: u8, bg: u32) -> u32 {
        let glyph = crate::draw::glyph_cache::CachedGlyph {
            width: 1,
            height: 1,
            coverage: vec![cov],
            bounds_min_y: 0.0,
        };
        let mut buf = vec![bg];
        crate::draw::glyph_cache::blit_cached_glyph(&mut buf, 1, 1, &glyph, 0.0, 0.0, color);
        let _ = name;
        buf[0]
    }

    /// `legacy_glyph_color` 的算术：新 RGB = 旧 RGB×旧 A/255、新 A = 旧 A²/255；
    /// **A = 0xFF 是恒等变换**（这类颜色"本来就不透明"，不需要补偿）。
    #[test]
    fn legacy_glyph_color_math() {
        // 图标主色：对象色 | 0x80 → A 0x80→0x40、RGB 各减半
        assert_eq!(legacy_glyph_color(0x80FF_0000), 0x40_80_00_00, "A=0x80 的红：R=0x80、A=0x40");
        assert_eq!(legacy_glyph_color(0x8000_FF00), 0x40_00_80_00);
        assert_eq!(legacy_glyph_color(0x8000_00FF), 0x40_00_00_80);
        // 图标描边 0x99000000：RGB 全是 0，只有 alpha 变 0x99²/255 = 0x5B
        assert_eq!(legacy_glyph_color(0x9900_0000), 0x5B00_0000);
        // 全不透明 = 恒等（地图标签 0xFFFFFFFF、玩家箭头一类）
        for c in [0xFFFF_FFFFu32, 0xFF11_2233, 0xFF00_0000] {
            assert_eq!(legacy_glyph_color(c), c, "A=0xFF 必须原样返回（无需补偿）");
        }
        // 全透明：任何 RGB 都乘成 0
        assert_eq!(legacy_glyph_color(0x00FF_FFFF), 0);
        // A=0xCC（数据链标签色那种）：R=0xFF×0xCC/255=0xCC、A=0xCC²/255=0xA3
        assert_eq!(legacy_glyph_color(0xCCFF_FFFF), 0xA3CC_CCCC);
    }

    /// **像素级回归（关键，别删）**：图标颜色补偿后走新字形管线，落屏值必须等于旧管线
    /// 满覆盖逐位相同、半覆盖每通道 ≤1/255、覆盖率 0 不动背景。
    /// 同时反向验证"忘了补偿"时必须差很多 —— 否则补偿被删掉这条测试仍会变绿。
    #[test]
    fn icon_glyph_pixels_match_pre_migration_pipeline() {
        // map_panel.rs::draw_icon_char 真正会传进来的两种颜色（对象色 | 0x80、描边 0x9900_0000）
        const ICON_COLORS: [u32; 4] =
            [0x8000_FF00, 0x80FF_0000, 0x80A0_A0A0, 0x9900_0000];
        const COVERAGES: [u32; 8] = [0, 1, 8, 64, 128, 200, 254, 255];

        let mut inexact: Vec<(u32, u32)> = Vec::new();
        for &icon in ICON_COLORS.iter() {
            // 未补偿时必须明显偏亮/偏实（A<FF 的图标色都满足）
            let a = (icon >> 24) & 0xFF;
            if a != 0xFF {
                let naive = blit_one("naive", icon, 255, 0);
                assert_ne!(
                    naive, old_glyph_pixel(icon, 255, 0),
                    "未补偿就与老管线相同？那说明补偿没生效或测试口径串了（icon={icon:#010X}）"
                );
            }
            for cov in COVERAGES {
                let want = old_glyph_pixel(icon, cov, 0);
                let got = blit_one("icon", legacy_glyph_color(icon), cov as u8, 0);
                for (name, shift) in [("B", 0u32), ("G", 8), ("R", 16), ("A", 24)] {
                    let dev = (((got >> shift) & 0xFF) as i32
                        - ((want >> shift) & 0xFF) as i32)
                        .abs();
                    assert!(
                        dev <= 1,
                        "icon={icon:#010X} cov={cov} {name}：与改造前老管线差 {dev}/255 \
                         （got={got:#010X} want={want:#010X}）"
                    );
                }
                if cov == 0 || cov == 255 {
                    assert_eq!(
                        got, want,
                        "icon={icon:#010X} cov={cov}：两端必须**逐位相同**"
                    );
                }
                if got != want {
                    inexact.push((icon, cov));
                }
            }
        }
        // 实测差异组（只为把"取整差 ≤1/255"钉死；变了就说明口径被动过）。
        // 注意：4 个图标主色（A=0x80）在全部 8 档覆盖率下**逐位相同** —— 通道值都 ≤0x80、
        // 两次整除恰好可交换；只有描边色 0x99000000 在 200/254 两档差 1/255。
        assert_eq!(
            inexact,
            vec![(0x9900_0000, 200), (0x9900_0000, 254)],
            "与改造前老管线的差异组变了 —— 图标补偿或字形覆盖率口径被动过，需复核"
        );
    }

    #[test]
    fn status_items_follow_edge_state() {
        // BRK/GEA 与告警共用边沿语义：放开中显示、收回中（下降沿）隐藏
        let colors = &crate::HUD_COLORS;
        let mut d = DisplayData::default();
        d.gear = 50.0;
        d.gear_armed = true;
        d.airbrake = 100.0;
        d.brake_caution = true;
        let items = status_items(&d, colors);
        assert!(items.iter().any(|(s, _)| s == "GEA"), "放开中应显示 GEA");
        assert!(items.iter().any(|(s, _)| s == "BRK"), "放开中应显示 BRK");

        // 下降沿（收回中）：整项隐藏
        d.gear = 30.0;
        d.gear_armed = false;
        d.airbrake = 40.0;
        d.brake_caution = false;
        let items = status_items(&d, colors);
        assert!(!items.iter().any(|(s, _)| s == "GEA"), "收回中应隐藏 GEA");
        assert!(!items.iter().any(|(s, _)| s == "BRK"), "收回中应隐藏 BRK");
    }

    #[test]
    fn map_zone_label_matches_game_grid() {
        // 与 core 解析测试同一张地图参数（grid_zero/grid_steps/map_min/maxsize）
        let maxsize = [131072.0, 131072.0];
        let map_min = [-65536.0, -65536.0];
        let grid_zero = [6494.300781250, 19547.50];
        let grid_steps = [5500.0, 5500.0];
        // abs(20000, 0) → 列 3、行 D
        let zone = map_zone_label(85536.0 / 131072.0, 0.5, maxsize, map_min, grid_zero, grid_steps);
        assert_eq!(zone, "D3");
        // 中心 abs(0,0) → 列钳到 1、行 D
        assert_eq!(map_zone_label(0.5, 0.5, maxsize, map_min, grid_zero, grid_steps), "D1");
        // 网格缺失时安全返回空
        assert_eq!(map_zone_label(0.5, 0.5, maxsize, map_min, grid_zero, [0.0, 0.0]), "");
    }

    #[test]
    fn sep_warning_level_two_tier() {
        assert_eq!(sep_warning_level(100.0), 0);
        assert_eq!(sep_warning_level(0.0), 1, "≤ 0 黄");
        assert_eq!(sep_warning_level(-299.0), 1);
        assert_eq!(sep_warning_level(-300.0), 1, "恰好 -300 仍为黄（红是严格小于）");
        assert_eq!(sep_warning_level(-301.0), 2, "< -300 红");
        assert_eq!(sep_warning_level(-500.0), 2);
    }

    #[test]
    fn warning_x_draws_data_color_strokes() {
        let w = 40usize;
        let h = 40usize;
        let mut buf = vec![0u32; w * h];
        draw_warning_x(&mut buf, w, h, 0.0, 0.0, 39.0, 39.0, 15.0);

        // 告警 X 的颜色 = 数字色（配置键 warning_x_color 已删除）
        let x_color = crate::HUD_COLORS.data_u32();
        // Both diagonals pass through the center and the quarter points.
        assert_eq!(buf[20 * w + 20], x_color, "center must be the data color");
        assert_eq!(buf[10 * w + 10], x_color, "main diagonal must be the data color");
        assert_eq!(buf[10 * w + 29], x_color, "anti diagonal must be the data color");
        // The outermost stroke ring keeps the dark outline, even at small fonts.
        let stroke = crate::HUD_COLORS.stroke_u32();
        assert_eq!(buf[30 * w + 31], stroke, "outermost ring must be the outline");
        // Far away from both diagonals nothing is drawn.
        assert_eq!(buf[20 * w + 4], 0, "off-diagonal pixels must stay clear");
    }

    #[test]
    fn warning_x_respects_voice_alarm_flag() {
        let w = 40usize;
        let h = 40usize;
        let mut buf = vec![0u32; w * h];
        let mut data = DisplayData::default();
        data.valid = 1;
        data.voice_alarm = false;
        draw_voice_alarm_x(&mut buf, w, h, &data, 0.0, 0.0, 39.0, 39.0, 15.0);
        assert!(buf.iter().all(|&p| p == 0), "no overlay without voice_alarm");
    }
}

pub fn status_items(data: &DisplayData, colors: &HudColors) -> Vec<(String, u32)> {
    let mut items = Vec::new();
    if data.flaps > 0.0 {
        items.push((format!("F{:.0}", data.flaps), colors.data_u32()));
    }
    if data.wing_sweep > 0.0 {
        items.push((format!("W{:.0}", (data.wing_sweep * 100.0).round()), colors.data_u32()));
    }
    // BRK/GEA 与告警共用同一套边沿判定：上升沿/保持（放开中）显示、100% 绿色，
    // 放开未到位灰色；下降沿（收回中）整项隐藏（不再以灰色显示）
    if data.brake_caution && data.airbrake > 0.0 {
        let color = if data.airbrake >= 100.0 { colors.data_u32() } else { colors.unit_u32() };
        items.push(("BRK".to_string(), color));
    }
    if data.gear_armed && data.gear > 0.0 {
        let color = if data.gear >= 100.0 { colors.data_u32() } else { colors.unit_u32() };
        items.push(("GEA".to_string(), color));
    }
    items
}

pub fn draw_inline_items(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    font: &ab_glyph::FontRef,
    x: f32,
    y: f32,
    items: &[(String, u32)],
    font_size: f32,
) {
    let mut x_offset = 0.0;
    for (i, (text, color)) in items.iter().enumerate() {
        let display = if i > 0 { format!(" {}", text) } else { text.clone() };
        let width = measure_text_raw(font, &display, font_size);
        draw_text_raw(buffer, buf_width, buf_height, font, x + x_offset, y, &display, font_size, *color);
        x_offset += width;
    }
}
