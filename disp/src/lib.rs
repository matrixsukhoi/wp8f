pub mod circle_hud;
pub mod components;
pub mod draw;
pub mod font;
pub(crate) mod geometry;
pub mod hud;
mod hud_common;
pub use hud_common::{map_zone_label, MapZone};
pub mod i18n;
pub mod mini_hud;
pub(crate) mod paths;

pub use draw::{draw_line, draw_text_raw, measure_text_raw, set_pixel};
pub use geometry::mini_hud_rect;

use ab_glyph::FontRef;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// —— 从 `lib.rs` 拆出的模块：对外路径不变（名字都从这里 `pub use` 出去）——
mod fixed_bytes;
pub use fixed_bytes::FixedBytes;

mod clamp;
pub use clamp::{Clamp, REFRESH_HZ_RANGE, RING_SLOTS_RANGE};

mod hud_type;
pub use hud_type::{default_hud_type, HudType};

mod config;
pub use config::{
    DataLinkConfig, HudLayoutConfig, RecordConfig, RingConfig, DEFAULT_FONT_SIZE,
    MAP_OBJ_INTERVAL_FRAME, MAP_OBJ_RECORD_EVERY_FRAMES_MIN, REFRESH_HZ_MAX, REFRESH_HZ_MIN,
};

mod state;
pub use state::{map_image, set_map_image, DisplayData, MapDisplay, MapImage, MapObjectDisplay};

mod drag;
pub use drag::{is_drag_mode, set_drag_mode, DragState, Panel, Slot};

mod layout;
pub use layout::{
    hud_layout, init_hud_layout, load_layout_config, save_drag_state, save_layout_config,
    set_layout_config_path,
};

mod window;
pub use window::{
    begin_frame, begin_map_frame, commit_frame, commit_map_frame, create_window, destroy_window,
    frame_reader, get_map_data, reset_ring_buffer, set_window_size, set_window_visible,
    signal_redraw, FrameReader, PollOutcome,
};
pub(crate) use window::get_window_size;

#[derive(Error, Debug)]
pub enum DispError {
    #[error("Window error: {0}")]
    WindowError(String),
}

pub use crate::components::HUD_COLORS;

/// 告警等级（**POD 的 1 字节**，与 `.wpr` CSV 列里的数字一一对应）。
///
/// 三个来源（超速 / 马赫 / 过载 / SEP …）各自算出等级后，**"取最严重"就是 `Ord::max`**：
/// 不再靠 `u8` 的 0/1/2 隐式约定去 `.max(1)`。
///
/// 数值口径（CSV 与旧数据的契约，别改顺序）：`0` 正常 / `1` 黄 / `2` 红。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
#[serde(from = "u8", into = "u8")]
pub enum WarningStage {
    /// 正常（无告警）
    #[default]
    Normal = 0,
    /// 黄：接近临界（超速 95%、过载 ≥90% 允许值、SEP ≤ 0 …）
    Caution = 1,
    /// 红：越界（超速 99%、SEP < 阈值 …）
    Alert = 2,
}

impl WarningStage {
    /// 写进 POD/CSV 的数值（`.wpr` 里存的就是它）。
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// 数值 → 等级：`>= 2` 一律当红（旧录制/越界值不至于让整块面板不显示告警）。
    pub const fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Normal,
            1 => Self::Caution,
            _ => Self::Alert,
        }
    }

    /// 两个来源里更严重的那个（`Ord::max` 的 `const` 版本，便于常量/纯函数里组链）。
    pub const fn worst(self, other: Self) -> Self {
        if (self as u8) >= (other as u8) {
            self
        } else {
            other
        }
    }

    /// 是否在告警（黄或红）。替代散落的 `> 0` 判断。
    pub const fn is_active(self) -> bool {
        (self as u8) > 0
    }
}

impl From<u8> for WarningStage {
    fn from(v: u8) -> Self {
        Self::from_u8(v)
    }
}

impl From<WarningStage> for u8 {
    fn from(s: WarningStage) -> u8 {
        s.as_u8()
    }
}

thread_local! {
    /// 正文字体：启动时从磁盘读入内存的那一份（[`font::init`]，`Box::leak` 后常驻）。
    /// 每个线程各自解析一次（`FontRef` 只是借用视图，构造很便宜），
    /// 所以几十处 `FONT.with(|font| …)` 调用点不用改；渲染路径不再有磁盘 I/O。
    ///
    /// 一个字节都没有时 panic 并带上"把字体放回 resource/fonts/"那条错误 ——
    /// 正常路径早在 [`init_hud_layout`] 里就 `exit(2)` 了，这里只是异常路径的兜底。
    pub static FONT: FontRef<'static> = FontRef::try_from_slice(font::data()).unwrap_or_else(|e| {
        panic!("{}（ab_glyph: {e}）", font::missing_error().unwrap_or_else(|| "字体加载失败".into()))
    });
    /// 图标字体（`resource/fonts/icons.ttf`），同上。
    pub static ICON_FONT: FontRef<'static> = FontRef::try_from_slice(font::icon_data()).unwrap_or_else(|e| {
        panic!("{}（ab_glyph: {e}）", font::missing_error().unwrap_or_else(|| "图标字体加载失败".into()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// 面板 → 槽的映射：`Hud` 一格按 `hud_type` 落到 miniHUD 或圆环，两条路互不干扰。
    /// （以前这个访问器读全局 `hud_type`：纯查询也得先把全局摆好。）
    #[test]
    fn panel_pos_routes_hud_panel_by_hud_type() {
        let mut drag = DragState::from_config(&HudLayoutConfig::default());
        drag.set_panel_pos(Panel::Hud, HudType::Mini, 11, 22);
        drag.set_panel_pos(Panel::Hud, HudType::Circle, 33, 44);
        assert_eq!(drag.panel_pos(Panel::Hud, HudType::Mini), (11, 22));
        assert_eq!(drag.panel_pos(Panel::Hud, HudType::Circle), (33, 44), "两个槽各存各的");
        assert_eq!(drag.slot_pos(Slot::MiniHud), (11, 22));
        assert_eq!(drag.slot_pos(Slot::Circle), (33, 44));

        // 其余面板与 hud_type 无关
        drag.set_panel_pos(Panel::Map, HudType::Mini, 7, 8);
        assert_eq!(drag.panel_pos(Panel::Map, HudType::Circle), (7, 8));
    }

    /// **miniHUD 与圆环的位置完全分离**：各自的配置键、各自的拖拽槽，
    /// 拖一边不会动到另一边，写回配置时也只写自己那两个键。
    /// （用户口径：两个 HUD 样式必须各用各的 x/y。）
    #[test]
    fn minihud_and_circle_positions_are_independent() {
        let mut cfg = HudLayoutConfig::default();
        cfg.minihud_x = 11;
        cfg.minihud_y = 22;
        cfg.circle_x = 33;
        cfg.circle_y = 44;

        // 起点：各自读自己的键
        let mut drag = DragState::from_config(&cfg);
        assert_eq!(drag.slot_pos(Slot::MiniHud), (11, 22));
        assert_eq!(drag.slot_pos(Slot::Circle), (33, 44));

        // 拖 miniHUD → 只动 mini 的槽
        drag.set_panel_pos(Panel::Hud, HudType::Mini, 100, 200);
        assert_eq!(drag.slot_pos(Slot::MiniHud), (100, 200));
        assert_eq!(drag.slot_pos(Slot::Circle), (33, 44), "拖 mini 不该碰到圆环的位置");

        // 拖圆环 → 只动圆环的槽
        drag.set_panel_pos(Panel::Hud, HudType::Circle, 300, 400);
        assert_eq!(drag.slot_pos(Slot::MiniHud), (100, 200), "拖圆环不该碰到 mini 的位置");
        assert_eq!(drag.slot_pos(Slot::Circle), (300, 400));

        // 写回配置：两边各写各的键
        let mut out = HudLayoutConfig::default();
        drag.apply_to(&mut out);
        assert_eq!((out.minihud_x, out.minihud_y), (100, 200));
        assert_eq!((out.circle_x, out.circle_y), (300, 400));

        // 落点：mini 用 mini 的键、圆环用圆环的键（同一窗口尺寸下互不影响）
        let win = (2560.0, 1440.0);
        assert_eq!(crate::geometry::hud_anchor(HudType::Mini, out.minihud_x, out.minihud_y, &out, win), (100.0, 200.0));
        assert_eq!(crate::geometry::hud_anchor(HudType::Circle, out.circle_x, out.circle_y, &out, win), (300.0, 400.0));
    }

    /// **拖拽松手必须真的落盘**，而且只改位置键：盘上其它键（控制台改的语言/颜色…）要保住。
    ///
    /// 这条钉的是用户报的那个现象：拖完 mini → 切圆环 → 切回 mini，位置被复原。
    /// 直接调 `save_drag_state`（松手/ESC/关窗都走它）再读回文件，不涉及窗口。
    /// 注：`LAYOUT_CONFIG_PATH` 是 `OnceLock`，本 crate 只有这一个用例会 set 它。
    #[test]
    fn drag_release_persists_positions_and_keeps_other_keys() {
        let dir = std::env::temp_dir().join(format!("wp8f-drag-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("layout.json");
        let mut disk = HudLayoutConfig::default();
        disk.minihud_x = 1180;
        disk.minihud_y = 908;
        disk.circle_x = 11;
        disk.circle_y = 22;
        disk.language = "ja".into(); // 假装控制台刚改过这个键
        std::fs::write(&file, serde_json::to_string_pretty(&disk).unwrap()).unwrap();
        crate::set_layout_config_path(file.display().to_string());

        // 拖 mini 到 (100,200)：圆环坐标与 language 必须保持盘上那份
        let mut drag = DragState::from_config(&disk);
        drag.set_panel_pos(Panel::Hud, HudType::Mini, 100, 200);
        crate::save_drag_state(&drag);

        let after: HudLayoutConfig =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!((after.minihud_x, after.minihud_y), (100, 200), "拖拽结果必须落盘");
        assert_eq!((after.circle_x, after.circle_y), (11, 22), "圆环的坐标不该被动到");
        assert_eq!(after.language, "ja", "盘上其它键（控制台改的）必须保住");

        // 再拖圆环：mini 的结果同样保住
        let mut drag2 = DragState::from_config(&after);
        drag2.set_panel_pos(Panel::Hud, HudType::Circle, 300, 400);
        crate::save_drag_state(&drag2);
        let after2: HudLayoutConfig =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!((after2.circle_x, after2.circle_y), (300, 400));
        assert_eq!((after2.minihud_x, after2.minihud_y), (100, 200), "圆环的拖动不该把 mini 复原");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `from_config` 的地图 y 回退与绘制/命中用同一个公式（配了就用配置值）。
    #[test]
    fn drag_state_map_y_falls_back_to_geometry() {
        let mut cfg = HudLayoutConfig::default();
        cfg.map_y = 0;
        let (_, map_h) = crate::geometry::map_panel_size(&cfg);
        assert_eq!(
            DragState::from_config(&cfg).slot_pos(Slot::Map).1,
            crate::geometry::map_panel_default_y(&cfg, map_h) as i32
        );
        cfg.map_y = 123;
        assert_eq!(DragState::from_config(&cfg).slot_pos(Slot::Map).1, 123);
    }

    /// **记录端底图的回归守卫**：`set_map_image` 必须把"解码 RGB / 原始字节 / 像素尺寸"三份
    /// **一起**写 —— 只写 RGB/尺寸时落盘的 `.wpr` 会 `img_len = 0`（回放没有底图）。
    #[test]
    fn map_image_publishes_rgb_raw_and_size_together() {
        let rgb = Arc::new(vec![1u8, 2, 3]);
        let raw = Arc::new(vec![0xFFu8, 0xD8, 0xFF, 0xE0]);
        assert!(map_image().raw.is_empty(), "没写过时是空底图（不 panic）");

        set_map_image(Arc::clone(&rgb), Arc::clone(&raw), 2048, 2048);
        let got = map_image();
        assert_eq!(&got.rgb[..], &rgb[..]);
        assert_eq!(&got.raw[..], &raw[..], "原始字节是记录线程唯一要的东西，漏了就等于没底图");
        assert_eq!((got.w, got.h), (2048, 2048));
        assert_eq!((got.rgb.len(), got.raw.len()), (3, 4), "两份各自独立");
    }
}
