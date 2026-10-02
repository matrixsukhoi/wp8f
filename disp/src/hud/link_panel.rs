//! Link-8 友军数据标签 Panel（五行式）：每个友军一行
//! `机型 | 表速 | 高度 | 航向 | 距离 | 区域`。全面板统一字号（与飞行面板标签同号）。
//!
//! 表头标签自 P6 起**走 i18n 查表**（`hud.link.*` 键，见 `disp/src/i18n.rs`）：
//! zh-CN 的值就是原来的中文（`机型/表速/高度/航向/距离/区域`，D24 的国际缩写已按用户要求回退），
//! 其它语言由 `resource/i18n/<lang>.json` 给。列位置是**固定倍率**（下面的 `COL_*`），
//! 与标签文本无关 —— 换标签不会挪动任何一列。

use crate::draw::draw_text_raw;
use crate::i18n::t;
use crate::{MapDisplay, HUD_COLORS};

/// 面板最多显示的友军行数（不含表头两行）。
pub const MAX_FRIENDLY_ROWS: usize = 8;

/// 面板统一字号倍率：与飞行信息面板标签字号（font_size × 1.25）一致。
pub const FONT_SCALE: f32 = 1.25;

/// 表头 6 个**标签键**（文案在 i18n 表里）：机型 / 表速 / 高度 / 航向 / 距离 / 区域。
/// `pub` 是给 `hud.rs` 的标签宽度回归测试与 i18n 的"键必须存在"测试用的。
pub const HEADER_LABEL_KEYS: [&str; 6] = [
    "hud.link.type", "hud.link.ias", "hud.link.alt", "hud.link.hdg", "hud.link.dist",
    "hud.link.zone",
];

/// 行列宽（以统一字号为单位）。
const COL_TYPE: f32 = 0.0;
const COL_IAS: f32 = 6.5;
const COL_ALT: f32 = 11.0;
const COL_HDG: f32 = 15.5;
const COL_DIST: f32 = 19.5;
const COL_ZONE: f32 = 24.0;
const ROW_WIDTH_UNITS: f32 = 29.0;

/// 由 HUD 基础字号得到面板统一字号。
pub fn panel_font(base_font_size: f32) -> f32 {
    base_font_size * FONT_SCALE
}

/// 面板宽度（像素），供 clear_rect / 拖曳命中使用。
pub fn panel_width(font_size: f32) -> f32 {
    ROW_WIDTH_UNITS * font_size
}

/// 面板最大高度（像素）：表头两行 + 最多 MAX_FRIENDLY_ROWS 行。
pub fn panel_height(font_size: f32) -> f32 {
    (MAX_FRIENDLY_ROWS + 2) as f32 * 1.2 * font_size
}

/// 绘制友军数据标签面板（`font_size` 为统一字号）。跳过自己（FLAG_SENDER）。
/// `tracks` 是**调用方每帧取一次**的数据链最新快照（`datalink::link_ring::latest_tracks()`）。
pub fn draw_link_panel(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    font: &ab_glyph::FontRef,
    map: &MapDisplay,
    tracks: &[datalink::StatePacket],
    x: f32,
    y: f32,
    font_size: f32,
) {
    let colors = &HUD_COLORS;
    let line_h = font_size * 1.2;

    // 表头：列标签 / 单位（同一字号；标签走 i18n 查表 —— 启动时固化，每帧零分配）
    let head: [&str; 6] = [
        t(HEADER_LABEL_KEYS[0]),
        t(HEADER_LABEL_KEYS[1]),
        t(HEADER_LABEL_KEYS[2]),
        t(HEADER_LABEL_KEYS[3]),
        t(HEADER_LABEL_KEYS[4]),
        t(HEADER_LABEL_KEYS[5]),
    ];
    draw_text_raw(buffer, buf_width, buf_height, font, x + COL_TYPE * font_size, y, head[0], font_size, colors.label_u32());
    draw_text_raw(buffer, buf_width, buf_height, font, x + COL_IAS * font_size, y, head[1], font_size, colors.label_u32());
    draw_text_raw(buffer, buf_width, buf_height, font, x + COL_ALT * font_size, y, head[2], font_size, colors.label_u32());
    draw_text_raw(buffer, buf_width, buf_height, font, x + COL_HDG * font_size, y, head[3], font_size, colors.label_u32());
    draw_text_raw(buffer, buf_width, buf_height, font, x + COL_DIST * font_size, y, head[4], font_size, colors.label_u32());
    draw_text_raw(buffer, buf_width, buf_height, font, x + COL_ZONE * font_size, y, head[5], font_size, colors.label_u32());

    let unit_y = y + line_h * 0.85;
    draw_text_raw(buffer, buf_width, buf_height, font, x + COL_IAS * font_size, unit_y, "km/h", font_size, colors.unit_u32());
    draw_text_raw(buffer, buf_width, buf_height, font, x + COL_ALT * font_size, unit_y, "m", font_size, colors.unit_u32());
    draw_text_raw(buffer, buf_width, buf_height, font, x + COL_HDG * font_size, unit_y, "deg", font_size, colors.unit_u32());
    draw_text_raw(buffer, buf_width, buf_height, font, x + COL_DIST * font_size, unit_y, "km", font_size, colors.unit_u32());

    let mut row = 0usize;
    for t in tracks {
        if t.flags & datalink::FLAG_SENDER != 0 {
            continue;
        }
        if row >= MAX_FRIENDLY_ROWS {
            break;
        }
        let ry = y + (row as f32 + 2.0) * line_h;

        // 距离：归一化地图坐标差 × 地图尺寸（米）
        let dx = t.x - map.player_map_x;
        let dy = t.y - map.player_map_y;
        let dist_km = (dx * dx + dy * dy).sqrt() * map.map_maxsize[0] / 1000.0;

        draw_text_raw(buffer, buf_width, buf_height, font, x + COL_TYPE * font_size, ry, t.type_str(), font_size, colors.data_u32());
        draw_text_raw(buffer, buf_width, buf_height, font, x + COL_IAS * font_size, ry, &format!("{:.0}", t.ias), font_size, colors.data_u32());
        draw_text_raw(buffer, buf_width, buf_height, font, x + COL_ALT * font_size, ry, &format!("{:.0}", t.altitude), font_size, colors.data_u32());
        draw_text_raw(buffer, buf_width, buf_height, font, x + COL_HDG * font_size, ry, &format!("{:.0}", t.heading), font_size, colors.data_u32());
        draw_text_raw(buffer, buf_width, buf_height, font, x + COL_DIST * font_size, ry, &format!("{:.1}", dist_km), font_size, colors.poi_u32());

        // 区域：由归一化坐标 + 本地地图网格推导（如 D3）
        let zone = crate::hud_common::map_zone_of(map, t.x, t.y);
        draw_text_raw(buffer, buf_width, buf_height, font, x + COL_ZONE * font_size, ry, &zone, font_size, colors.poi_u32());
        row += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MapDisplay;

    const W: usize = 640;
    const H: usize = 200;

    fn map_with_player() -> MapDisplay {
        MapDisplay {
            player_map_x: 0.5,
            player_map_y: 0.5,
            map_maxsize: [100_000.0, 100_000.0],
            ..MapDisplay::default()
        }
    }

    fn draw(tracks: &[datalink::StatePacket]) -> Vec<u32> {
        let mut buf = vec![0u32; W * H];
        let map = map_with_player();
        crate::FONT.with(|font| {
            draw_link_panel(&mut buf, W, H, font, &map, tracks, 0.0, 0.0, 16.0);
        });
        buf
    }

    /// 表头文案取自**当前语言**（进程级状态），而 cargo 的测试线程是并行的 ——
    /// 每个用例先拿 `i18n::TEST_LOCK` 并钉住语言：既不会被别的用例中途切走，
    /// 同一用例的两次 `draw` 之间也不会变（否则"两次渲染应当相同"会变成假失败）。
    fn pin_lang() -> std::sync::MutexGuard<'static, ()> {
        let g = crate::i18n::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ = crate::i18n::set_lang(crate::i18n::DEFAULT);
        g
    }

    fn drawn(buf: &[u32]) -> usize {
        buf.iter().filter(|p| **p != 0).count()
    }

    /// 面板画的是**传进来的快照**（数据源已从 `MapDisplay.link_tracks` 换成数据链槽）。
    #[test]
    fn panel_renders_rows_from_the_passed_snapshot() {
        let _g = pin_lang();
        let empty = draw(&[]);
        assert!(drawn(&empty) > 0, "表头本来就该画（机型/表速/…）");

        let mut peer = datalink::StatePacket::new(42);
        peer.set_type("f_16c");
        peer.ias = 512.0;
        peer.altitude = 3200.0;
        peer.heading = 271.0;
        peer.x = 0.60;
        peer.y = 0.50;
        let with_peer = draw(&[peer]);
        assert!(
            drawn(&with_peer) > drawn(&empty),
            "传入 1 个友军后应多出这一行的像素：empty={} peer={}",
            drawn(&empty),
            drawn(&with_peer)
        );
    }

    /// 自己那一行（服务端回填 FLAG_SENDER）必须被跳过：与空快照逐像素相同。
    #[test]
    fn sender_row_is_skipped() {
        let _g = pin_lang();
        let empty = draw(&[]);
        let mut me = datalink::StatePacket::new(7);
        me.set_type("f_16c");
        me.flags = datalink::FLAG_SENDER;
        me.ias = 512.0;
        assert_eq!(draw(&[me]), empty, "只有自己时不该多画任何一行");
    }

    /// 最多 `MAX_FRIENDLY_ROWS` 行：超出的不画（面板高度是固定的）。
    #[test]
    fn rows_capped_at_max_friendly_rows() {
        let _g = pin_lang();
        let peers: Vec<datalink::StatePacket> = (0..MAX_FRIENDLY_ROWS as u64 + 3)
            .map(|i| {
                let mut p = datalink::StatePacket::new(i + 1);
                p.set_type("f_16c");
                p.ias = 400.0 + i as f64;
                p.altitude = 3000.0;
                p.x = 0.55;
                p.y = 0.55;
                p
            })
            .collect();
        let capped = draw(&peers[..MAX_FRIENDLY_ROWS]);
        let overflow = draw(&peers);
        assert_eq!(overflow, capped, "第 {} 行之后不该再画", MAX_FRIENDLY_ROWS);
    }
}
