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
