//! `wp8f-disp` 的门面：只放模块声明与 `pub use`，实现都在各子模块里。
//!
//! 从 `lib.rs` 拆出的模块（P3）：`fixed_bytes`（定长文本）/ `clamp`（配置派生量区间）/
//! `hud_type` / `config`（布局配置与缺省值）/ `state`（环里的快照）/ `drag`（拖拽）/
//! `layout`（配置全局与落盘）/ `window`（窗口与帧环消费端）。
//! **对外路径保持不变**：`wp8f_disp::HudLayoutConfig`、`::DisplayData`、`::FONT` 等照旧可用。

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

pub use crate::components::HUD_COLORS;
pub use draw::{draw_line, draw_text_raw, measure_text_raw, set_pixel};
pub use font::{FONT, ICON_FONT};
pub use geometry::mini_hud_rect;

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
pub use state::{
    map_image, set_map_image, DisplayData, MapDisplay, MapImage, MapObjectDisplay, WarningStage,
};

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
    signal_redraw, DispError, FrameReader, PollOutcome,
};
pub(crate) use window::get_window_size;
