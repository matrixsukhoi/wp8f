use crate::draw::{
    draw_rectangle_semi, draw_rotated_square_with_cross_semi, draw_runway_semi,
    draw_text_raw, draw_x_marker, measure_text_raw,
};
use crate::hud_common::blend_pixel_cov;
use crate::{hud_layout, DisplayData, MapDisplay, MapObjectDisplay, FONT, ICON_FONT};

fn bytes_to_str<const N: usize>(buf: &[u8; N]) -> &str {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(N);
    std::str::from_utf8(&buf[..end]).unwrap_or("")
}

/// 航向箭头（导航飞镖形）——浮点多边形逐像素 3×3 超采样渲染：
/// - 顶点保持浮点不取整，按覆盖率抗锯齿：旋转平滑、无闪烁锯齿
/// - 填充严格限定在多边形内部：不出界
/// - 描边按到边距离平滑混合：任意底色上轮廓清晰
/// 玩家（大、青）与 Link-8 友军（小、绿）共用。
fn draw_player_arrow(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    cx: i32,
    cy: i32,
    dx: f32,
    dy: f32,
    size: i32,
    color: u32,
) {
    const STROKE_HALF_W: f32 = 0.6;
    const STROKE: u32 = 0xB0000000;

    let angle = dy.atan2(dx);
    let (sin_a, cos_a) = (angle.sin(), angle.cos());
    let s = size as f32;
    // 尖端 / 左翼 / 尾部凹口 / 右翼 —— 简约直观的"纸飞镖"轮廓
    let pts = [
        (s, 0.0),
        (-0.5 * s, -0.55 * s),
        (-0.2 * s, 0.0),
        (-0.5 * s, 0.55 * s),
    ];
    let v: [(f32, f32); 4] = [
        rot_pt(pts[0], sin_a, cos_a, cx, cy),
        rot_pt(pts[1], sin_a, cos_a, cx, cy),
        rot_pt(pts[2], sin_a, cos_a, cx, cy),
        rot_pt(pts[3], sin_a, cos_a, cx, cy),
    ];

    // 像素包围盒（多留 2px 给描边）
    let min_x = v.iter().map(|p| p.0.floor() as i32 - 2).min().unwrap_or(cx);
    let max_x = v.iter().map(|p| p.0.ceil() as i32 + 2).max().unwrap_or(cx);
    let min_y = v.iter().map(|p| p.1.floor() as i32 - 2).min().unwrap_or(cy);
    let max_y = v.iter().map(|p| p.1.ceil() as i32 + 2).max().unwrap_or(cy);

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let mut fill_cov = 0u32;
            let mut edge_cov = 0u32;
            // 3×3 子像素采样：覆盖率 0..9，边缘平滑过渡
            for sy in 0..3 {
                for sx in 0..3 {
                    let px = x as f32 + (sx as f32 + 0.5) / 3.0;
                    let py = y as f32 + (sy as f32 + 0.5) / 3.0;
                    if point_in_polygon(px, py, &v) {
                        fill_cov += 1;
                    }
                    if dist_to_polygon(px, py, &v) < STROKE_HALF_W {
                        edge_cov += 1;
                    }
                }
            }
            if fill_cov > 0 {
                blend_pixel_cov(buffer, buf_width, buf_height, x, y, color, fill_cov as f32 / 9.0);
            }
            if edge_cov > 0 {
                blend_pixel_cov(buffer, buf_width, buf_height, x, y, STROKE, edge_cov as f32 / 9.0);
            }
        }
    }
}

#[inline]
fn rot_pt(p: (f32, f32), sin_a: f32, cos_a: f32, cx: i32, cy: i32) -> (f32, f32) {
    (
        p.0 * cos_a - p.1 * sin_a + cx as f32,
        p.0 * sin_a + p.1 * cos_a + cy as f32,
    )
}

/// 射线法判断点是否在多边形内（支持尾部凹口）。
fn point_in_polygon(px: f32, py: f32, v: &[(f32, f32); 4]) -> bool {
    let mut inside = false;
    let mut j = 3;
    for i in 0..4 {
        let (xi, yi) = v[i];
        let (xj, yj) = v[j];
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// 点到多边形边界的最短距离。
fn dist_to_polygon(px: f32, py: f32, v: &[(f32, f32); 4]) -> f32 {
    let mut best = f32::MAX;
    for i in 0..4 {
        let (x1, y1) = v[i];
        let (x2, y2) = v[(i + 1) % 4];
        let dx = x2 - x1;
        let dy = y2 - y1;
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 {
            (((px - x1) * dx + (py - y1) * dy) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let ex = x1 + t * dx - px;
        let ey = y1 + t * dy - py;
        let d2 = ex * ex + ey * ey;
        if d2 < best {
            best = d2;
        }
    }
    best.sqrt()
}

const MAP_PLAYER_COLOR: u32 = 0xCC00FFFF;
/// Link-8 友军机型标签保留的字符数（如 f_16a_block_10 → f_16）。
const TYPE_LABEL_CHARS: usize = 4;
const REF_MAP_SIZE: f32 = 240.0;

fn draw_text_with_bg(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    font: &ab_glyph::FontRef,
    x: f32,
    y: f32,
    text: &str,
    size: f32,
    text_color: u32,
    _bg_color: u32,
) {
    draw_text_raw(buffer, buf_width, buf_height, font, x, y, text, size, text_color);
}

fn draw_map_img(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    map_x: usize,
    map_y: usize,
    map_w: usize,
    map_h: usize,
    map: &MapDisplay,
    img: &crate::MapImage,
) {
    let img_width = img.w;
    let img_height = img.h;
    if img_width == 0 || img_height == 0 || img.rgb.is_empty() { return; }

    let mx = map_x as i32;
    let my = map_y as i32;
    let mw = map_w as i32;
    let mh = map_h as i32;
    let inv_w = 1.0 / mw as f32;
    let inv_h = 1.0 / mh as f32;
    let player_x_adj = map.player_map_x as f32 - 0.5;
    let player_y_adj = map.player_map_y as f32 - 0.5;
    let img_width_f = img_width as f64;
    let img_height_f = img_height as f64;

    for dy in 0..mh {
        let py = my + dy;
        if py < 0 || py >= buf_height as i32 { continue; }
        let norm_y = player_y_adj + dy as f32 * inv_h;
        for dx in 0..mw {
            let px = mx + dx;
            if px < 0 || px >= buf_width as i32 { continue; }
            let norm_x = player_x_adj + dx as f32 * inv_w;
            let sx = (norm_x as f64 * img_width_f) as i32;
            let sy = (norm_y as f64 * img_height_f) as i32;
            if sx < 0 || sx >= img_width as i32 || sy < 0 || sy >= img_height as i32 {
                continue;
            }
            let pixel_idx = (sy as u32 * img_width + sx as u32) as usize * 3;
            if pixel_idx + 2 >= img.rgb.len() { continue; }
            let r = img.rgb[pixel_idx];
            let g = img.rgb[pixel_idx + 1];
            let b = img.rgb[pixel_idx + 2];
            let idx = py as usize * buf_width + px as usize;
            buffer[idx] = (0x40u32 << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
        }
    }
}

fn icon_char(icon: &str) -> Option<char> {
    Some(match icon {
        "Assault" => 'a',
        "Bomber" => 'b',
        "Fighter" => 'f',
        "Tracked" | "MediumTank" => '5',
        "Airdefence" | "SPAA" => '4',
        "Wheeled" | "LightTank" => 'w',
        "TorpedoBoat" => 't',
        "Ship" => 's',
        "bombing_point" => '8',
        "defending_point" => '9',
        _ => return None,
    })
}

const ICON_OFFSET_X: f32 = 1.0;
const ICON_OFFSET_Y: f32 = 0.0;

fn draw_icon_char(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    cx: i32,
    cy: i32,
    ch: char,
    size: f32,
    color: u32,
) {
    // 图标颜色是**代码里写死的**（对象色 | 0x80、描边 0x99000000），不是配置里的补偿值，
    // 所以要在绘制前补一次旧口径的预乘 —— 否则图标比改造前更亮/更实（P9 用户实测）。
    // 补偿函数与理由见 `crate::hud_common::legacy_glyph_color`（**别把这两行删掉**）。
    let color = crate::hud_common::legacy_glyph_color(color);
    let halo = crate::hud_common::legacy_glyph_color(0x99000000);
    ICON_FONT.with(|font| {
        let text = &ch.to_string()[..];
        let w = measure_text_raw(font, text, size);
        let x = cx as f32 - w / 2.0 + ICON_OFFSET_X;
        let y = cy as f32 + size * 0.4 + ICON_OFFSET_Y;
        for (ox, oy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            draw_text_raw(buffer, buf_width, buf_height, font,
                x + ox as f32, y + oy as f32, text, size, halo);
        }
        draw_text_raw(buffer, buf_width, buf_height, font, x, y, text, size, color);
    });
}

fn draw_map_object(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    ix: i32,
    iy: i32,
    obj: &MapObjectDisplay,
    icon_size: f32,
    map_icon_size: f32,
    hint_color: u32,
) {
    let semi_color = (obj.color & 0x00FFFFFF) | 0x80000000;

    if obj.is_player {
        let dx = obj.dx as f32;
        let dy = obj.dy as f32;
        draw_player_arrow(buffer, buf_width, buf_height, ix, iy, dx, dy, (map_icon_size / 1.8).round() as i32, MAP_PLAYER_COLOR);
        return;
    }

    let icon = bytes_to_str(&obj.icon);
    let obj_type = bytes_to_str(&obj.obj_type);

    if let Some(ch) = icon_char(icon) {
        return draw_icon_char(buffer, buf_width, buf_height, ix, iy, ch, icon_size, semi_color);
    }

    if obj_type.contains("point_of_interest") {
        // 用配置里的**警示色 hint_color**（原 poi_color 已删除；以前还硬编码过黄色）
        return draw_x_marker(buffer, buf_width, buf_height, ix, iy, (icon_size * 7.5 / 24.0).round() as i32, hint_color);
    }

    if icon.contains("none") || icon.contains("airfield") {
        let heading = if obj.dx != 0.0 || obj.dy != 0.0 {
            -(obj.dy.atan2(obj.dx) * 180.0 / std::f64::consts::PI) as f32
        } else {
            0.0
        };
        draw_rotated_square_with_cross_semi(
            buffer, buf_width, buf_height, ix, iy, (map_icon_size * 2.0 / 3.0).round() as i32, heading, semi_color,
        );
    } else {
        draw_rectangle_semi(buffer, buf_width, buf_height, ix, iy, (map_icon_size / 9.0).round() as i32, (map_icon_size / 18.0).round() as i32, semi_color);
    }
}

fn draw_map_background(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    map_x: usize,
    map_y: usize,
    map_w: usize,
    map_h: usize,
    map: &MapDisplay,
    img: &crate::MapImage,
    layout: &crate::HudLayoutConfig,
) {
    let mx = map_x as i32;
    let my = map_y as i32;
    let mw = map_w as i32;
    let mh = map_h as i32;
    for y in my..(my + mh) {
        if y < 0 || y >= buf_height as i32 { continue; }
        let x0 = mx.max(0).min(buf_width as i32);
        let x1 = (mx + mw).max(0).min(buf_width as i32);
        if x0 < x1 {
            let row = y as usize * buf_width;
            buffer[row + x0 as usize..row + x1 as usize].fill(layout.map_bg_color);
        }
    }

    if !img.rgb.is_empty() {
        draw_map_img(buffer, buf_width, buf_height, map_x, map_y, map_w, map_h, map, img);
    }

    let border_color = layout.data_color;
    for x in 0..map_w {
        let idx = map_y * buf_width + (map_x + x);
        if idx < buffer.len() {
            buffer[idx] = border_color;
        }
        let idx = (map_y + map_h - 1) * buf_width + (map_x + x);
        if idx < buffer.len() {
            buffer[idx] = border_color;
        }
    }
    for y in 0..map_h {
        let idx = (map_y + y) * buf_width + map_x;
        if idx < buffer.len() {
            buffer[idx] = border_color;
        }
        let idx = (map_y + y) * buf_width + map_x + map_w - 1;
        if idx < buffer.len() {
            buffer[idx] = border_color;
        }
    }
}

fn draw_range_rings(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    map: &MapDisplay,
    map_x: usize,
    map_y: usize,
    map_w: usize,
    map_h: usize,
    map_font_size: f32,
    ring_color_base: u32,
) {
    if map.map_maxsize[0] <= 0.0 || map.map_maxsize[1] <= 0.0 { return; }

    let player_px = map_x as i32 + (map_w as i32 / 2);
    let player_py = map_y as i32 + (map_h as i32 / 2);
    let scale = map_w as f64 / map.map_maxsize[0];
    let map_x0 = map_x as i32;
    let map_y0 = map_y as i32;
    let map_x1 = (map_x + map_w) as i32;
    let map_y1 = (map_y + map_h) as i32;

    let label_color = 0xFFFFFFFF;
    for (i, &dist_km) in [80.0, 60.0, 40.0, 30.0, 20.0, 10.0].iter().enumerate() {
        let radius = (dist_km * 1000.0 * scale) as i32;
        if radius < 5 { continue; }
        let alpha = 0x40 + i as u32 * 0x20;
        let ring_color = (ring_color_base & 0x00FFFFFF) | (alpha << 24);

        // 抗锯齿同心圆（3×3 超采样覆盖率），裁剪在地图矩形内
        crate::hud_common::draw_arc_band_aa(
            buffer, buf_width, buf_height,
            player_px as f32, player_py as f32,
            radius as f32, 1.0, ring_color,
            0.0, 360.0,
            Some((map_x0, map_y0, map_x1, map_y1)),
        );

        let label = format!("{:.0}", dist_km / 10.0);
        FONT.with(|font| {
            let tw = measure_text_raw(font, &label, map_font_size);
            // 只在圆环**上方**标一次距离（用户要求：下方那个重复标注已移除）
            let lx = player_px as f32 - tw / 2.0;
            let ly = player_py as f32 - radius as f32;
            if lx >= map_x0 as f32 && lx + tw < map_x1 as f32 && ly >= map_y0 as f32 && ly < map_y1 as f32 {
                draw_text_raw(buffer, buf_width, buf_height, font, lx, ly, &label, map_font_size, label_color);
            }
        });
    }
}

fn draw_map_info_text(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    map: &MapDisplay,
    heading: f64,
    map_x: usize,
    map_y: usize,
    map_w: usize,
    _map_h: usize,
    data_color: u32,
    font_size: f32,
) {
    let loc_text = format!("Zone {} {:.0}°",
        String::from_utf8_lossy(map.map_zone_text.as_bytes()), heading);
    let airfield_text = String::from_utf8_lossy(map.nearest_airfield_text.as_bytes());
    let poi_info_text = String::from_utf8_lossy(map.nearest_poi_text.as_bytes());

    let poi_line = if map.nearest_poi_neighbor_dist < f64::MAX {
        format!("{} / {:.0}m", poi_info_text, map.nearest_poi_neighbor_dist)
    } else {
        poi_info_text.to_string()
    };

    let text_right_x = map_x as f32 + map_w as f32 - font_size * 0.3;
    let text_y1 = map_y as f32 + font_size * 0.25;
    let text_y2 = text_y1 + font_size * 1.1;
    let text_y3 = text_y2 + font_size * 1.1;
    FONT.with(|font| {
        let tw_loc = measure_text_raw(font, &loc_text, font_size);
        let tw_af = measure_text_raw(font, &airfield_text, font_size);
        let tw_poi = measure_text_raw(font, &poi_line, font_size);

        let max_tw = tw_loc.max(tw_af).max(tw_poi);

        if max_tw > 0.0 {
            draw_text_with_bg(buffer, buf_width, buf_height, font,
                text_right_x - tw_loc, text_y1 + font_size / 2.0,
                &loc_text, font_size, data_color, 0);

            draw_text_with_bg(buffer, buf_width, buf_height, font,
                text_right_x - tw_af, text_y2 + font_size / 2.0,
                &airfield_text, font_size, data_color, 0);

            draw_text_with_bg(buffer, buf_width, buf_height, font,
                text_right_x - tw_poi, text_y3 + font_size / 2.0,
                &poi_line, font_size, data_color, 0);
        }
    });
}

pub fn draw_map_panel(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    map: &MapDisplay,
    img: &crate::MapImage,
    data: &DisplayData,
    link_tracks: &[datalink::StatePacket],
    map_x: usize,
    map_y: usize,
    map_w: usize,
    map_h: usize,
) {
    let layout = hud_layout();
    let data_color = layout.data_color;
    let map_font_size = layout.map_font_size;
    let map_icon_size = layout.map_icon_size;
    let ring_color_base = layout.data_color;

    draw_map_background(buffer, buf_width, buf_height, map_x, map_y, map_w, map_h, map, img, layout);

    // 注意：对象列表为空时**只跳过对象绘制**。原来这里直接 return，
    // 把下面的友军链路（datalink，与对象列表无依赖）、距离环和 Zone/机场文本
    // 一起吃掉，面板只剩底图。
    let icon_size = map_icon_size * map_w as f32 / REF_MAP_SIZE;
    let player_x = map.player_map_x;
    let player_y = map.player_map_y;

    let mut draw_obj = |obj: &MapObjectDisplay| {
        let airfield = obj.ex != 0.0 || obj.ey != 0.0;

        let rel_x = (obj.x - player_x + 0.5) as f32;
        let rel_y = (obj.y - player_y + 0.5) as f32;
        let obj_screen_x = map_x as f32 + rel_x * map_w as f32;
        let obj_screen_y = map_y as f32 + rel_y * map_h as f32;

        if airfield {
            let end_rel_x = (obj.ex - player_x + 0.5) as f32;
            let end_rel_y = (obj.ey - player_y + 0.5) as f32;
            let end_screen_x = map_x as f32 + end_rel_x * map_w as f32;
            let end_screen_y = map_y as f32 + end_rel_y * map_h as f32;

            let sx1 = obj_screen_x.min(end_screen_x).max(map_x as f32);
            let sx2 = obj_screen_x.max(end_screen_x).min((map_x + map_w) as f32);
            let sy1 = obj_screen_y.min(end_screen_y).max(map_y as f32);
            let sy2 = obj_screen_y.max(end_screen_y).min((map_y + map_h) as f32);
            if sx1 > sx2 || sy1 > sy2 {
                return;
            }

            let runway_w = (icon_size / 12.0).round() as i32;
            draw_runway_semi(buffer, buf_width, buf_height,
                obj_screen_x as i32, obj_screen_y as i32,
                end_screen_x as i32, end_screen_y as i32,
                runway_w, (obj.color & 0x00FFFFFF) | 0x80000000);
            return;
        }

        if obj_screen_x < map_x as f32
            || obj_screen_x >= (map_x + map_w) as f32
            || obj_screen_y < map_y as f32
            || obj_screen_y >= (map_y + map_h) as f32
        {
            return;
        }

        draw_map_object(buffer, buf_width, buf_height, obj_screen_x as i32, obj_screen_y as i32,
                    obj, icon_size, map_icon_size, layout.hint_color);
    };

    let is_top = |obj: &MapObjectDisplay| -> bool {
        if obj.is_player { return true; }
        let icon = bytes_to_str(&obj.icon);
        if icon.contains("none") || icon.contains("airfield") { return true; }
        let obj_type = bytes_to_str(&obj.obj_type);
        obj_type.contains("point_of_interest")
    };

    if !map.map_objects.is_empty() {
        for obj in &map.map_objects {
            if !is_top(obj) { draw_obj(obj); }
        }
        for obj in &map.map_objects {
            if is_top(obj) { draw_obj(obj); }
        }
    }

    draw_datalink_tracks(buffer, buf_width, buf_height, link_tracks, player_x, player_y, map_x, map_y, map_w, map_h);

    draw_range_rings(buffer, buf_width, buf_height, map, map_x, map_y, map_w, map_h, map_font_size, ring_color_base);
    draw_map_info_text(buffer, buf_width, buf_height, map, data.heading, map_x, map_y, map_w, map_h, data_color, map_font_size);
}

/// Link-8 友军：与地图对象同一投影公式（地图以玩家为中心），
/// 画小号绿色三角箭头，**旋转方向 = 航向**（0°=北/上，顺时针）。
/// 自己由服务端回填 FLAG_SENDER，跳过。
fn draw_datalink_tracks(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    link_tracks: &[datalink::StatePacket],
    player_x: f64,
    player_y: f64,
    map_x: usize,
    map_y: usize,
    map_w: usize,
    map_h: usize,
) {
    if link_tracks.is_empty() {
        return;
    }
    let layout = hud_layout();
    let color = (layout.data_color & 0x00FFFFFF) | 0xCC000000;
    // 比玩家箭头（map_icon_size/1.8）更小一号
    let size = (layout.map_icon_size / 2.5).round().max(3.0) as i32;
    let label_size = layout.map_font_size * 0.8;

    for t in link_tracks {
        if t.flags & datalink::FLAG_SENDER != 0 {
            continue;
        }
        let rel_x = (t.x - player_x + 0.5) as f32;
        let rel_y = (t.y - player_y + 0.5) as f32;
        let sx = map_x as f32 + rel_x * map_w as f32;
        let sy = map_y as f32 + rel_y * map_h as f32;
        if sx < map_x as f32
            || sx >= (map_x + map_w) as f32
            || sy < map_y as f32
            || sy >= (map_y + map_h) as f32
        {
            continue;
        }
        // 航向 0°=北（屏幕上方）、顺时针
        let rad = t.heading.to_radians();
        let (dx, dy) = (rad.sin() as f32, -rad.cos() as f32);
        draw_player_arrow(buffer, buf_width, buf_height, sx as i32, sy as i32, dx, dy, size, color);

        // 机型标签：只保留前 TYPE_LABEL_CHARS 个字符（UTF-8 边界安全），
        // 居中显示在单位正上方
        let name = t.type_str();
        let end = name
            .char_indices()
            .nth(TYPE_LABEL_CHARS)
            .map(|(i, _)| i)
            .unwrap_or(name.len());
        let label = &name[..end];
        FONT.with(|font| {
            let tw = measure_text_raw(font, label, label_size);
            draw_text_raw(
                buffer,
                buf_width,
                buf_height,
                font,
                sx - tw * 0.5,
                sy - size as f32 - label_size,
                label,
                label_size,
                color,
            );
        });
    }

    // 友军 POI：绿色十字（poi_enabled 时显示；与飞机标记解耦，
    // 友机在面板外也可显示其 POI）
    let arm = size + 2;
    for t in link_tracks {
        if t.flags & datalink::FLAG_SENDER != 0 || t.poi_enabled == 0 {
            continue;
        }
        let rel_x = (t.poi_x - player_x + 0.5) as f32;
        let rel_y = (t.poi_y - player_y + 0.5) as f32;
        let px = map_x as f32 + rel_x * map_w as f32;
        let py = map_y as f32 + rel_y * map_h as f32;
        if px < map_x as f32
            || px >= (map_x + map_w) as f32
            || py < map_y as f32
            || py >= (map_y + map_h) as f32
        {
            continue;
        }
        // 十字（+ 形）
        for i in -arm..=arm {
            blend_pixel_cov(buffer, buf_width, buf_height, px as i32 + i, py as i32, color, 1.0);
            blend_pixel_cov(buffer, buf_width, buf_height, px as i32, py as i32 + i, color, 1.0);
        }
    }
}
