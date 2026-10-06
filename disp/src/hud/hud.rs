use std::borrow::Cow;

use crate::components::{draw_grid, grid_size, DrawContext, DrawItem, RgbaColor, TextScale};
use crate::draw::clear_rect;
use crate::i18n::t;
use crate::mini_hud;
use crate::{get_map_data, DisplayData, MapDisplay, FONT};

/// 发动机面板的一项：取值 / 动态单位 / 是否显示都是**函数指针**，
/// 表里写下的就是全部 —— 没有"id 打错 → 静默空白"的 `_ => b""` 分支。
struct EngineField {
    /// 标签键（文案在 `resource/lang/*.json` 的 `hud.*`）
    label: &'static str,
    /// 动态单位为空时的缺省单位
    default_unit: &'static str,
    value: fn(&DisplayData) -> &[u8],
    unit: fn(&DisplayData) -> &[u8],
    visible: fn(&DisplayData) -> bool,
}

// 表里的噪音抑制：没有动态单位 / 恒显示 / 只在活塞机显示 / 只在有第二油箱时显示。
const NO_UNIT: fn(&DisplayData) -> &[u8] = |_| b"";
const ALWAYS: fn(&DisplayData) -> bool = |_| true;
const NOT_JET: fn(&DisplayData) -> bool = |d| !d.is_jet;
const HAS_FUEL1: fn(&DisplayData) -> bool = |d| d.has_fuel1;

const fn field(
    label: &'static str,
    default_unit: &'static str,
    value: fn(&DisplayData) -> &[u8],
    unit: fn(&DisplayData) -> &[u8],
    visible: fn(&DisplayData) -> bool,
) -> EngineField {
    EngineField { label, default_unit, value, unit, visible }
}

macro_rules! item {
    ($label:expr, $value:expr, $unit:expr, $dw:expr, $lw:expr) => {
        DrawItem::new($label, core::str::from_utf8($value).unwrap_or(""), core::str::from_utf8($unit).unwrap_or(""), $dw, $lw)
    };
}

// —— 标签全部走 i18n 查表 ——
//
// 这里只有**键**，文案在 `resource/lang/<name>.json` 的 `hud.*` 键里（`zh.json` 就是原来的
// 中文标签，全角空格原样保留；`en.json` 用 IAS/ALT/AOA 这类缩写）。查表在启动时一次性完成
// （`crate::i18n::init`），`t()` 返回 `&'static str`、不分配 —— 每帧画 HUD 一个 `String` 都不新建。
//
// 布局：`draw_grid` 的标签列是定宽 + 右对齐（`label_width = label_size * 3.2`），列宽/格宽与文本
// 无关，换标签不会挪动面板、命中框或清屏矩形；但中文标签比缩写宽，所以"标签塞得进这一列"
// 由 tests 里的 `translated_labels_fit_label_column` 逐语言逐字号量（真实字体）。
//
// 飞行信息面板（12 格）：表　速 / 真空速 / 马赫数 / 高　度 / 爬升率 / SEP /
//                        转弯率（无姿态仪时 = 盘旋率）/ 滚转率 / 俯仰角 / 攻　角 / 过　载 / 转半径
pub const FLIGHT_LABEL_KEYS: [&str; 12] = [
    "hud.f.ias", "hud.f.tas", "hud.f.mach", "hud.f.alt", "hud.f.vy", "hud.f.sep",
    "hud.f.turn", "hud.f.roll", "hud.f.pitch", "hud.f.aoa", "hud.f.g", "hud.f.radius",
];
/// 第 7 格是**两种口径互斥**的同一个位置：有姿态仪 = 转弯率，没有 = 盘旋率。
pub const FLIGHT_LABEL_KEY_TURN: &str = "hud.f.turn";
pub const FLIGHT_LABEL_KEY_ORBIT: &str = "hud.f.orbit";

// 发动机面板（喷气 12 项 / 活塞 14 项）。水/油温格的"单位"栏显示的是各自的**临界温度**。
const ENGINE_ITEMS_JET: &[EngineField] = &[
    field("hud.e.temp",        "°C",   |d| d.engine_temp_water_text.as_bytes(), |d| d.water_crit_temp_text.as_bytes(), ALWAYS),
    field("hud.e.oil",         "°C",   |d| d.engine_temp_oil_text.as_bytes(),   |d| d.oil_crit_temp_text.as_bytes(),   ALWAYS),
    field("hud.e.rpm",         "RPM",  |d| d.engine_rpm_text.as_bytes(),        |d| d.engine_rpm_unit.as_bytes(),      ALWAYS),
    field("hud.e.tw_jet",      "",     |d| d.thrust_to_weight_text.as_bytes(), NO_UNIT, ALWAYS),
    field("hud.e.thrust",      "Kgf",  |d| d.total_thrust_text.as_bytes(),      NO_UNIT, ALWAYS),
    field("hud.e.drag",        "Kgf*", |d| d.total_drag_text.as_bytes(),        NO_UNIT, ALWAYS),
    field("hud.e.power",       "Hp",   |d| d.engine_power_text.as_bytes(),      |d| d.power_unit_text.as_bytes(),      ALWAYS),
    field("hud.e.fuel",        "Kg",   |d| d.fuel_kg_text.as_bytes(),           NO_UNIT, ALWAYS),
    field("hud.e.fuel_time",   "Min",  |d| d.fuel_time_text.as_bytes(),         |d| d.fuel_sfc_text.as_bytes(),        ALWAYS),
    field("hud.e.fuel1",       "Kg",   |d| d.fuel1_kg_text.as_bytes(),          NO_UNIT, HAS_FUEL1),
    field("hud.e.fuel1_time",  "Min",  |d| d.fuel1_time_text.as_bytes(),        |d| d.fuel1_sfc_text.as_bytes(),       HAS_FUEL1),
    field("hud.e.elapsed",     "Min",  |d| d.elapsed_time_text.as_bytes(),      NO_UNIT, ALWAYS),
];

const ENGINE_ITEMS_PISTON: &[EngineField] = &[
    field("hud.e.temp",        "°C",   |d| d.engine_temp_water_text.as_bytes(), |d| d.water_crit_temp_text.as_bytes(), ALWAYS),
    field("hud.e.oil",         "°C",   |d| d.engine_temp_oil_text.as_bytes(),   |d| d.oil_crit_temp_text.as_bytes(),   ALWAYS),
    field("hud.e.rpm",         "RPM",  |d| d.engine_rpm_text.as_bytes(),        |d| d.engine_rpm_unit.as_bytes(),      ALWAYS),
    field("hud.e.tw_piston",   "",     |d| d.thrust_to_weight_text.as_bytes(), NO_UNIT, ALWAYS),
    field("hud.e.thrust",      "Kgf",  |d| d.total_thrust_text.as_bytes(),      NO_UNIT, ALWAYS),
    field("hud.e.drag",        "Kgf*", |d| d.total_drag_text.as_bytes(),        NO_UNIT, ALWAYS),
    field("hud.e.map",         "",     |d| d.manifold_pressure_text.as_bytes(), |d| d.manifold_pressure_unit.as_bytes(), NOT_JET),
    field("hud.e.rated",       "Hp",   |d| d.rated_power_text.as_bytes(),       NO_UNIT, NOT_JET),
    field("hud.e.power",       "Hp",   |d| d.engine_power_text.as_bytes(),      |d| d.power_unit_text.as_bytes(),      ALWAYS),
    field("hud.e.fuel",        "Kg",   |d| d.fuel_kg_text.as_bytes(),           NO_UNIT, ALWAYS),
    field("hud.e.fuel_time",   "Min",  |d| d.fuel_time_text.as_bytes(),         |d| d.fuel_sfc_text.as_bytes(),        ALWAYS),
    field("hud.e.fuel1",       "Kg",   |d| d.fuel1_kg_text.as_bytes(),          NO_UNIT, HAS_FUEL1),
    field("hud.e.fuel1_time",  "Min",  |d| d.fuel1_time_text.as_bytes(),        |d| d.fuel1_sfc_text.as_bytes(),       HAS_FUEL1),
    field("hud.e.elapsed",     "Min",  |d| d.elapsed_time_text.as_bytes(),      NO_UNIT, ALWAYS),
];

/// 全部 HUD 标签**键**（飞行 12 + 盘旋率 + 发动机两套 + 友军面板 6），去重前。
/// `pub` 是给 i18n 的"每个标签键都得在表里"与宽度回归测试遍历用的。
pub fn all_label_keys() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = FLIGHT_LABEL_KEYS.to_vec();
    v.push(FLIGHT_LABEL_KEY_ORBIT);
    v.extend(ENGINE_ITEMS_JET.iter().map(|f| f.label));
    v.extend(ENGINE_ITEMS_PISTON.iter().map(|f| f.label));
    v.extend(crate::hud::link_panel::HEADER_LABEL_KEYS);
    v
}

/// HUD 本体（圆环 / miniHUD）的落点：拖拽模式下取拖拽槽、否则取配置值，
/// 再交给 [`crate::geometry::hud_anchor`] 做"0 = 自动"的回退 ——
/// 与拖拽命中测试共用同一份口径（两处各写一遍就漂过）。
fn hud_anchor_of(
    drag: &crate::DragState,
    layout: &crate::HudLayoutConfig,
    drag_mode: bool,
    width: usize,
    height: usize,
) -> (f32, f32) {
    let circle = layout.hud_type == crate::HudType::Circle;
    let (sx, sy) = if drag_mode {
        drag.slot_pos(if circle { crate::Slot::Circle } else { crate::Slot::MiniHud })
    } else if circle {
        (layout.circle_x, layout.circle_y)
    } else {
        (layout.minihud_x, layout.minihud_y)
    };
    crate::geometry::hud_anchor(layout.hud_type, sx, sy, layout, (width as f32, height as f32))
}

pub fn draw_hud(buffer: &mut [u32], width: usize, height: usize, data: &DisplayData, drag: &crate::DragState) {
    let map = get_map_data().unwrap_or_else(|| {
        static EMPTY_MAP: std::sync::OnceLock<MapDisplay> = std::sync::OnceLock::new();
        EMPTY_MAP.get_or_init(|| MapDisplay::default())
    });
    // Link-8 友军快照：每帧**只取一次**最新（写端在 datalink 线程里，这里不等待、
    // 也不克隆历史版本）。数据链没启用时是空列表 → 友军面板/标记自然不画。
    let link_tracks = datalink::link_ring::latest_tracks();

    let scale = TextScale::new(crate::hud_layout().font_size).with_scale(1.25);
    let padding = scale.label_size() * 0.5;
    let h_spacing = scale.label_size() * 0.1;
    let v_spacing = scale.label_size() * 0.25;
    let drag_mode = crate::is_drag_mode();
    let layout = crate::hud_layout();
    let dw = scale.data_size() * 13.0 / 4.0;
    let lw = scale.label_size() * 3.2;

    let panel_x = |drag_val: i32, cfg_val: i32| -> f32 { if drag_mode { drag_val as f32 } else { cfg_val as f32 } };
    let panel_y = |drag_val: i32, cfg_val: i32| -> f32 { if drag_mode { drag_val as f32 } else { cfg_val as f32 } };

    let flight_cols = layout.flight_info_col.max(1) as usize;
    let flight_rows = (12 + flight_cols - 1) / flight_cols;
    let (drag_flight_x, drag_flight_y) = drag.slot_pos(crate::Slot::Flight);
    let (drag_engine_x, drag_engine_y) = drag.slot_pos(crate::Slot::Engine);
    let (drag_map_x, drag_map_y) = drag.slot_pos(crate::Slot::Map);
    let (drag_link_x, drag_link_y) = drag.slot_pos(crate::Slot::Link);

    let flight_x = panel_x(drag_flight_x, layout.flight_info_x);
    let flight_y = panel_y(drag_flight_y, layout.flight_info_y);
    let flight_items: [DrawItem; 12] = [
        // Two-stage overspeed warning (yellow/red) + speed-brake caution (yellow).
        item!(t(FLIGHT_LABEL_KEYS[0]), data.ias_text.as_bytes(), data.vne_text.as_bytes(), dw, lw).with_color(
            RgbaColor::from_argb(crate::hud_common::ias_warning_color(data, layout.data_color))),
        item!(t(FLIGHT_LABEL_KEYS[1]), data.tas_text.as_bytes(), b"Km/h", dw, lw),
        // Mach colored by its own MNE warning stage.
        item!(t(FLIGHT_LABEL_KEYS[2]), data.mach_text.as_bytes(), data.mne_text.as_bytes(), dw, lw).with_color(
            RgbaColor::from_argb(crate::hud_common::warning_level_color(data.mach_warning, layout.data_color))),
        item!(t(FLIGHT_LABEL_KEYS[3]), data.altitude_text.as_bytes(), b"M", dw, lw),
        item!(t(FLIGHT_LABEL_KEYS[4]), data.vy_text.as_bytes(), b"M/s", dw, lw),
        // SEP 两级告警（≤0 黄，< -300 红）
        item!(t(FLIGHT_LABEL_KEYS[5]), data.sep_text.as_bytes(), b"M/s", dw, lw).with_color(
            RgbaColor::from_argb(crate::hud_common::warning_level_color(
                crate::hud_common::sep_warning_level(data.sep),
                layout.data_color,
            ))),
        // 姿态仪机型：转弯率（速度矢量合力法）；无姿态仪：盘旋率（罗盘变化率）——
        // 两者互斥、同一个格子，所以是两个键（中文标签本来就不同）
        item!(
            if data.has_attitude { t(FLIGHT_LABEL_KEY_TURN) } else { t(FLIGHT_LABEL_KEY_ORBIT) },
            data.turn_rate_text.as_bytes(), b"\xc2\xb0/s", dw, lw),
        item!(t(FLIGHT_LABEL_KEYS[7]), data.roll_rate_text.as_bytes(), b"\xc2\xb0/s", dw, lw),
        item!(t(FLIGHT_LABEL_KEYS[8]), data.pitch_text.as_bytes(), b"\xc2\xb0", dw, lw),
        item!(t(FLIGHT_LABEL_KEYS[9]), data.aoa_format_text.as_bytes(), b"\xc2\xb0", dw, lw),
        // 单位栏显示当前重量下的允许正过载，如 "/15.4"（风格同 VNE）
        item!(t(FLIGHT_LABEL_KEYS[10]), data.g_load_text.as_bytes(), data.g_limit_text.as_bytes(), dw, lw)
            // 过载统一告警逻辑：≥90% 允许过载时红色
            .with_color(RgbaColor::from_argb(crate::hud_common::g_warning_color(
                data.g_load_warning,
                layout.data_color,
            ))),
        item!(t(FLIGHT_LABEL_KEYS[11]), data.turn_radius_text.as_bytes(), b"M", dw, lw),
    ];
    let margin = scale.label_size();
    let flight_items = {
        let cap = flight_rows * flight_cols;
        flight_items.into_iter().take(cap).collect::<Vec<_>>()
    };
    let (flight_w, flight_h) = grid_size(&flight_items, flight_rows, flight_cols, padding, h_spacing, v_spacing, &scale);
    clear_rect(buffer, width, height, flight_x - margin, flight_y - margin, flight_w + margin * 2.0, flight_h + margin * 2.0);

    // 尺寸/回退统一由 geometry 提供（命中测试用的是同一份公式）
    let (map_w_f, map_h_f) = crate::geometry::map_panel_size(layout);
    let map_w = map_w_f as usize;
    let map_h = map_h_f as usize;
    let map_x = panel_x(drag_map_x, layout.map_x);
    let map_y = if drag_mode {
        drag_map_y as f32
    } else if layout.map_y > 0 {
        layout.map_y as f32
    } else {
        crate::geometry::map_panel_default_y(layout, map_h as f32)
    };
    if map_w > 0 && map_h > 0 {
        clear_rect(buffer, width, height, map_x as f32 - margin, map_y as f32 - margin, map_w as f32 + margin * 2.0, map_h as f32 + margin * 2.0);
    }

    // Link-8 友军数据标签面板（drag 模式下显示并可拖曳定位）
    let link_font = crate::hud::link_panel::panel_font(layout.font_size);
    let link_x = panel_x(drag_link_x, layout.datalink_x);
    let link_y = panel_y(drag_link_y, layout.datalink_y);
    let show_link = drag_mode || !link_tracks.is_empty();
    if show_link {
        clear_rect(
            buffer,
            width,
            height,
            link_x - margin,
            link_y - margin,
            crate::hud::link_panel::panel_width(link_font) + margin * 2.0,
            crate::hud::link_panel::panel_height(link_font) + margin * 2.0,
        );
    }

    let items_def = if data.is_jet { ENGINE_ITEMS_JET } else { ENGINE_ITEMS_PISTON };

    let engine_items: Vec<DrawItem> = items_def.iter()
        .filter(|f| (f.visible)(data))
        .map(|f| {
            let unit_bytes = (f.unit)(data);
            let value: Cow<'_, str> = Cow::Borrowed(core::str::from_utf8((f.value)(data)).unwrap_or(""));
            let unit_str: Cow<'_, str> = if unit_bytes.is_empty() {
                Cow::Borrowed(f.default_unit)
            } else {
                Cow::Borrowed(core::str::from_utf8(unit_bytes).unwrap_or(""))
            };
            DrawItem {
                // 启动时固化的只读表 → 这里仍是 `Cow::Borrowed`：每帧零分配
                label: Cow::Borrowed(t(f.label)),
                value,
                unit: unit_str,
                color: None,
                data_width: dw,
                label_width: lw,
            }
        })
        .collect();

    if !engine_items.is_empty() {
        let engine_cols = layout.engine_info_col as usize;
        let engine_rows = (engine_items.len() + engine_cols - 1) / engine_cols;
        let engine_x = panel_x(drag_engine_x, layout.engine_info_x);
        let engine_y = panel_y(drag_engine_y, layout.engine_info_y);
        let (engine_w, engine_h) = grid_size(&engine_items, engine_rows, engine_cols, padding, h_spacing, v_spacing, &scale);
        clear_rect(buffer, width, height, engine_x - margin, engine_y - margin, engine_w + margin * 2.0, engine_h + margin * 2.0);
    }

    // HUD 本体：0 = 关闭（连擦除都不做），1 = MiniHUD，2 = 圆环。
    match layout.hud_type {
        crate::HudType::Off => {}
        crate::HudType::Circle => {
            let (cx, cy) = hud_anchor_of(drag, layout, drag_mode, width, height);
            let r = crate::geometry::circle_radius(layout);
            clear_rect(buffer, width, height, cx - r, cy - r, r * 2.0, r * 2.0);
        }
        crate::HudType::Mini => {
            // 擦除范围 = miniHUD 的**真实足迹**（往左/往上含数字列与字形上伸部，见 mini_hud_rect）
            let (mx, my) = hud_anchor_of(drag, layout, drag_mode, width, height);
            let (rx, ry, rw, rh) = crate::geometry::mini_hud_rect(layout, mx, my);
            clear_rect(buffer, width, height, rx, ry, rw, rh);
        }
    }

    FONT.with(|font| {
        let mut ctx = DrawContext::new(buffer, width, height, font);

        if layout.flight_info_enabled {
            draw_grid(&flight_items, flight_rows, flight_cols, &mut ctx, flight_x, flight_y, padding, h_spacing, v_spacing, &scale);
        }

        if layout.engine_info_enabled && !engine_items.is_empty() {
            let engine_cols = layout.engine_info_col.max(1) as usize;
            let engine_rows = (engine_items.len() + engine_cols - 1) / engine_cols;
            let engine_x = panel_x(drag_engine_x, layout.engine_info_x);
            let engine_y = panel_y(drag_engine_y, layout.engine_info_y);
            draw_grid(&engine_items, engine_rows, engine_cols, &mut ctx, engine_x, engine_y, padding, h_spacing, v_spacing, &scale);
        }

        if layout.map_enabled && map_w > 0 && map_h > 0 {
            crate::hud::map_panel::draw_map_panel(
                buffer, width, height, map, &crate::map_image(), data, &link_tracks,
                map_x as usize, map_y as usize, map_w, map_h,
            );
        }

        if layout.datalink_panel_enabled && show_link {
            crate::hud::link_panel::draw_link_panel(
                buffer,
                width,
                height,
                font,
                map,
                &link_tracks,
                link_x,
                link_y,
                link_font,
            );
        }
    });

    // 画哪种 HUD 只看 `hud_type`（0 关 / 1 mini / 2 圆环）—— 没有第二套开关，
    // 也没有命令行参数来盖它：配置怎么写就怎么画。
    match layout.hud_type {
        crate::HudType::Off => {}
        crate::HudType::Circle => {
            let (cx, cy) = hud_anchor_of(drag, layout, drag_mode, width, height);
            crate::circle_hud::draw_circle_hud(buffer, width, height, data, map, cx, cy);
        }
        crate::HudType::Mini => {
            let (px, py) = hud_anchor_of(drag, layout, drag_mode, width, height);
            mini_hud::draw_mini_hud(buffer, width, height, data, map, px, py);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::TextScale;
    use crate::i18n;

    /// 全部 HUD 标签**键**（飞行 12 + 盘旋率 + 发动机两套 + 友军面板 6），去重。
    fn all_label_keys_unique() -> Vec<&'static str> {
        let mut v = all_label_keys();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// 某个语言下全部标签的**实际文案**（键 → 表里的值）。
    fn labels_in(lang: &str) -> Vec<(&'static str, &'static str)> {
        let t = i18n::table(lang).unwrap_or_else(|| panic!("读不到 resource/lang/{lang}.json"));
        all_label_keys_unique()
            .into_iter()
            .map(|k| (k, t.get(k).unwrap_or_else(|| panic!("{lang} 表里没有 {k}"))))
            .collect()
    }

    /// 标签列的**实测宽度**（真实字体、与 `draw_hud` 同一口径 `label_size × 3.2`）。
    fn measure_labels(labels: &[(&'static str, &'static str)], base: f32) -> Vec<(String, f32, f32)> {
        let scale = TextScale::new(base).with_scale(1.25);
        let label_size = scale.label_size();
        let column = label_size * 3.2;
        crate::FONT.with(|font| {
            labels
                .iter()
                .map(|(k, l)| {
                    (format!("{k}={l:?}"), crate::measure_text_raw(font, l, label_size), column)
                })
                .collect()
        })
    }

    /// **D24 回退的回归护栏**：中文表的值必须**逐条等于**改缩写之前的中文标签
    ///（原文见 `git show 0eb3053a^:disp/src/hud/hud.rs`）。改标签 = 改这里，逼着人确认。
    /// 唯一例外：`hud.f.sep` 按用户要求用**全角** `ＳＥＰ`（与"表　速""高　度"等三字标签同宽）。
    #[test]
    fn chinese_labels_are_the_original_strings() {
        let got: Vec<(&str, &str)> = labels_in("zh");
        let want: &[(&str, &str)] = &[
            ("hud.f.ias", "表　速"),
            ("hud.f.tas", "真空速"),
            ("hud.f.mach", "马赫数"),
            ("hud.f.alt", "高　度"),
            ("hud.f.vy", "爬升率"),
            ("hud.f.sep", "ＳＥＰ"),
            ("hud.f.turn", "转弯率"),
            ("hud.f.orbit", "盘旋率"),
            ("hud.f.roll", "滚转率"),
            ("hud.f.pitch", "俯仰角"),
            ("hud.f.aoa", "攻　角"),
            ("hud.f.g", "过　载"),
            ("hud.f.radius", "转半径"),
            ("hud.e.temp", "温　度"),
            ("hud.e.oil", "油　温"),
            ("hud.e.rpm", "转　速"),
            ("hud.e.tw_jet", "推重比"),
            ("hud.e.tw_piston", "功重比"),
            ("hud.e.thrust", "推　力"),
            ("hud.e.drag", "阻　力"),
            ("hud.e.map", "进气压"),
            ("hud.e.rated", "功　率"),
            ("hud.e.power", "实功率"),
            ("hud.e.fuel", "燃油量"),
            ("hud.e.fuel_time", "燃油时"),
            ("hud.e.fuel1", "燃油量'"),
            ("hud.e.fuel1_time", "燃油时'"),
            ("hud.e.elapsed", "时　间"),
            ("hud.link.type", "机型"),
            ("hud.link.ias", "表速"),
            ("hud.link.alt", "高度"),
            ("hud.link.hdg", "航向"),
            ("hud.link.dist", "距离"),
            ("hud.link.zone", "区域"),
        ];
        let mut want_sorted: Vec<(&str, &str)> = want.to_vec();
        want_sorted.sort_unstable();
        assert_eq!(got.len(), want_sorted.len(), "标签键集合变了（多了或少了）");
        for ((gk, gv), (wk, wv)) in got.iter().zip(want_sorted.iter()) {
            assert_eq!(gk, wk, "标签键集合与期望不一致");
            assert_eq!(gv, wv, "中文表的 {gk} 必须是原来的中文标签 {wv:?}（D24 已按用户要求回退）");
        }
        // 直白地再锁一遍"确实回退了"：不许再是 D24 的国际缩写
        assert_eq!(i18n::table("zh").unwrap().get("hud.f.ias"), Some("表　速"));
        assert_eq!(i18n::table("en").unwrap().get("hud.f.ias"), Some("IAS"));
    }

    /// 标签必须塞得进定宽标签列（`label_width = label_size × 3.2`）：用真实字体在 6 档字号下
    /// 逐语言逐标签量，不允许任何宽限 —— 哪门语言加一个更长的标签这里立刻红。
    #[test]
    fn translated_labels_fit_label_column() {
        let mut longest: Vec<(String, f32)> = Vec::new();
        for lang in &i18n::lang_files() {
            let labels = labels_in(lang);
            for base in [8.0f32, 16.0, 24.0, 32.0, 48.0, 72.0] {
                for (desc, w, column) in measure_labels(&labels, base) {
                    assert!(
                        w <= column,
                        "{lang} 的标签 {desc} 在字号 {base} 下宽 {w:.1}px > 列宽 {column:.1}px \
                         —— 会戳出定宽标签列、与数值列挤在一起"
                    );
                    longest.push((format!("{lang}/{desc}"), w / column));
                }
            }
        }
        // 顺带给出"哪一条最贴边"（排障时不用重现）
        longest.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        eprintln!("标签/列宽 比值的 top5：{:?}", &longest[..5.min(longest.len())]);
    }

    /// 定宽列的容量：`label_size × 3.2` 恰好装得下"3 个全角 + 1 个半角"（中文表里最宽的
    /// `燃油量'` / `燃油时'` 就是这一档），再多半个字就装不下。
    /// 钉住它 = 别把撇号当溢出、顺手去改列宽（列宽一改，`grid_size`、面板命中框、清屏矩形一起挪）。
    #[test]
    fn label_column_holds_three_full_width_plus_one_half() {
        let scale = TextScale::new(16.0).with_scale(1.25);
        let label_size = scale.label_size();
        let column = label_size * 3.2;
        let zh = i18n::table("zh").unwrap();
        let w = |s: &str| crate::FONT.with(|font| crate::measure_text_raw(font, s, label_size));
        // 实测：定宽列 = 3.2 em；Sarasa Mono SC 的 CJK 是 1 em、ASCII 是半角（≈0.43 em，按行高归一）
        for k in ["hud.e.fuel1", "hud.e.fuel1_time"] {
            let l = zh.get(k).unwrap();
            assert!(
                w(l) <= column,
                "{k}={l:?} 宽 {:.2}px > 列宽 {column:.2}px（3 全角 + 1 半角本该刚好放得下）",
                w(l)
            );
        }
        // 反例（**保持可失败**）：再来一个半角字符就装不下了 —— 谁把标签写得更长，
        // `translated_labels_fit_label_column` 会先红；这条说明"红"的边界在哪。
        let too_wide = format!("{}''", zh.get("hud.e.fuel1").unwrap());
        assert!(
            w(&too_wide) > column,
            "4 个全角 + 2 个半角居然还塞得进列宽 —— 说明列宽被改大了（布局会跟着挪）"
        );
    }

    /// 网格几何不因标签变化而变：`grid_size` 只用 `data_width + label_width`（定宽），
    /// 与标签文本无关 —— 这条锁住"改标签不会挪动面板/命中框"。
    #[test]
    fn grid_width_is_independent_of_label_text() {
        let scale = TextScale::new(16.0).with_scale(1.25);
        let (dw, lw) = (scale.data_size() * 13.0 / 4.0, scale.label_size() * 3.2);
        let a = DrawItem::new("IAS", "123", "Km/h", dw, lw);
        let b = DrawItem::new("RADIUS", "123", "Km/h", dw, lw);
        let c = DrawItem::new("表　速", "123", "Km/h", dw, lw);
        assert_eq!(a.width(), b.width(), "标签文本不得影响格宽（右对齐 + 定宽列）");
        assert_eq!(a.width(), c.width(), "中文标签同样不得影响格宽");
        assert_eq!(a.width(), dw + lw);
    }

    /// **像素级对齐证据**：真的把格子画进缓冲区，量每一列的墨迹范围。
    ///
    /// 几何契约（`draw_context.rs::draw_grid`）：数值右对齐到 `item_x + data_width`；
    /// 标签与单位右对齐到 `item_x + data_width + label_width`；三列共用一条基线（`draw_text` 的 y
    /// 是**基线**，所以标签的墨迹大多落在格子原点上方，而数值的字身高会让它向上探进标签那几行
    /// —— 想按行带把三列分开是不成立的，这正是本测试用"另一列为空"来隔离的原因）。
    ///
    /// 与 `translated_labels_fit_label_column`（只证明宽度不超列宽）互补：这条证明真画出来是对齐的。
    #[test]
    fn grid_columns_align_at_pixel_level() {
        let w = 640usize;
        let h = 240usize;
        let y0 = 80.0f32;                        // 基线：给上方（标签）与下方（数值）都留足空间
        let scale = TextScale::new(16.0).with_scale(1.25);   // 与 draw_hud 同一口径
        let (dw, lw) = (scale.data_size() * 13.0 / 4.0, scale.label_size() * 3.2);
        let data_edge = dw as usize;             // 数值列的右边界
        let edge = (dw + lw) as usize;           // 标签/单位列的右边界
        let zh = i18n::table("zh").unwrap();
        // 回退后的中文标签（短：表　速 / 长：转半径），单位仍是 ASCII
        let (short_label, long_label) = (
            zh.get("hud.f.ias").unwrap(),        // 表　速
            zh.get("hud.f.radius").unwrap(),     // 转半径
        );

        // 画一个 item 并返回它的墨迹包围盒（min_x, max_x, min_y, max_y）
        let draw_one = |label: &str, value: &str, unit: &str| {
            let mut buf = vec![0u32; w * h];
            let item = DrawItem::new(label, value, unit, dw, lw);
            crate::FONT.with(|font| {
                let mut ctx = DrawContext::new(&mut buf, w, h, font);
                draw_grid(&[item], 1, 1, &mut ctx, 0.0, y0, 0.0, 0.0, 0.0, &scale);
            });
            let mut bbox: Option<(usize, usize, usize, usize)> = None;
            for y in 0..h {
                for x in 0..w {
                    if buf[y * w + x] != 0 {
                        bbox = Some(match bbox {
                            None => (x, x, y, y),
                            Some((x0, x1, ya, yb)) => (x0.min(x), x1.max(x), ya.min(y), yb.max(y)),
                        });
                    }
                }
            }
            bbox
        };

        // ① 标签 + 单位（数值留空 → 这一格里只有标签列的墨迹）
        let lu = draw_one(short_label, "", "Km/h").expect("标签与单位必须画出来");
        assert!(
            lu.0 >= data_edge && lu.1 >= edge - 4 && lu.1 <= edge,
            "标签/单位列没贴住定宽列的右边 {edge}（墨迹 x ∈ [{}, {}]）—— 或侵入了数据列 {data_edge}",
            lu.0, lu.1
        );
        // 标签比单位高（字号不同）：两者的最右墨迹必须落在同一条右对齐边上
        let label_only = draw_one(short_label, "", "").expect("标签必须画出来");
        let unit_only = draw_one("", "", "Km/h").expect("单位必须画出来");
        assert!(
            label_only.1.abs_diff(unit_only.1) <= 2,
            "标签最右 {} 与单位最右 {} 不在同一条右对齐边上",
            label_only.1, unit_only.1
        );
        assert!(
            label_only.1.abs_diff(lu.1) <= 2 && unit_only.1.abs_diff(lu.1) <= 2,
            "合起来画时最右墨迹 {} 与单独画（标签 {} / 单位 {}）不一致",
            lu.1, label_only.1, unit_only.1
        );
        assert!(label_only.2 < unit_only.2, "标签的墨迹应当比单位靠上（同一基线、字号更大）");

        // ② 数值（标签/单位留空 → 这一格里只有数据列的墨迹）
        let v = draw_one("", "1234", "").expect("数值必须画出来");
        assert!(
            v.1 >= data_edge - 4 && v.1 <= data_edge && v.0 < data_edge,
            "数值没贴住数据列的右边 {data_edge}（墨迹 x ∈ [{}, {}]）",
            v.0, v.1
        );
        assert!(v.1 < edge, "数值侵入了标签列（最右 {} ≥ {edge}）", v.1);

        // ③ 最长的中文标签（3 个全角 = 3 em ≤ 3.2 em 的列宽）也必须待在自己的列里
        let long = draw_one(long_label, "", "°").expect("最长的中文标签必须画出来");
        assert!(
            long.0 >= data_edge && long.1 <= edge,
            "{long_label:?} 越出定宽标签列（墨迹 x ∈ [{}, {}] vs 列 [{data_edge}, {edge}]）",
            long.0, long.1
        );
        // ④ 三种中文标签（2 字 + 全角空格 / 3 字 / 3 字）的**右沿必须完全对齐**
        for k in ["hud.f.ias", "hud.f.tas", "hud.f.radius"] {
            let l = zh.get(k).unwrap();
            let b = draw_one(l, "", "").expect("标签必须画出来");
            assert!(
                b.1.abs_diff(label_only.1) <= 2,
                "{k}={l:?} 的右沿 {} 与基准 {} 没对齐（标签列右对齐失效）",
                b.1, label_only.1
            );
        }
    }

    /// 查表**不得**把键名画上屏：每一门语言的每一个标签键都必须查得到值。
    ///（`i18n::t` 的兜底链最后一级是键名本身，那是"漏翻一眼可见"的设计 —— 这条拦住漏翻。）
    #[test]
    fn no_label_key_leaks_to_the_screen() {
        for lang in &i18n::lang_files() {
            for (k, v) in labels_in(lang) {
                assert_ne!(k, v, "{lang} 的 {k} 查表返回了键名本身（漏翻）");
                assert!(!v.trim().is_empty(), "{lang} 的 {k} 是空串（会让网格塌掉）");
            }
        }
    }
}
