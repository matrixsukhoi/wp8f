use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RgbaColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl RgbaColor {
    pub fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// 内部打包：`0xAARRGGBB`（**不是**配置文件里的 `#RRGGBBAA` 顺序）。
    /// 配置文件只在 `crate::hex_color` 里按 RGBA 读写，其余代码一律用这个打包。
    pub fn to_argb(&self) -> u32 {
        ((self.a as u32) << 24) | ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    /// 内部打包 `0xAARRGGBB` → 通道（与 `to_argb` 对称，见上）。
    pub fn from_argb(argb: u32) -> Self {
        Self {
            a: ((argb >> 24) & 0xFF) as u8,
            r: ((argb >> 16) & 0xFF) as u8,
            g: ((argb >> 8) & 0xFF) as u8,
            b: (argb & 0xFF) as u8,
        }
    }

    pub fn green() -> Self {
        Self::new(0, 255, 0, 255)
    }

    pub fn white() -> Self {
        Self::new(255, 255, 255, 255)
    }

    pub fn yellow() -> Self {
        Self::new(255, 255, 0, 255)
    }

    pub fn red() -> Self {
        Self::new(255, 0, 0, 255)
    }

    pub fn gray() -> Self {
        Self::new(128, 128, 128, 255)
    }

    pub fn transparent() -> Self {
        Self::new(0, 0, 0, 0)
    }
}

impl Default for RgbaColor {
    fn default() -> Self {
        Self::green()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct HudColors {
    pub data: RgbaColor,
    pub label: RgbaColor,
    pub unit: RgbaColor,
    pub hint: RgbaColor,
    pub alert: RgbaColor,
    pub poi: RgbaColor,
    pub stroke: RgbaColor,
    pub background: RgbaColor,
}

impl HudColors {
    fn palette() -> Self {
        Self {
            data: RgbaColor::new(80, 255, 130, 162),
            label: RgbaColor::new(50, 200, 90, 102),
            unit: RgbaColor::new(160, 160, 160, 102),
            hint: RgbaColor::new(255, 240, 60, 162),
            alert: RgbaColor::new(255, 60, 40, 162),
            poi: RgbaColor::new(255, 240, 60, 162),
            stroke: RgbaColor::new(0, 40, 20, 102),
            background: RgbaColor::new(0, 0, 0, 0),
        }
    }

    /// Build the palette from the layout config, so one JSON file controls
    /// every HUD color (the config is the single source of truth).
    pub fn from_config(cfg: &crate::HudLayoutConfig) -> Self {
        Self {
            data: RgbaColor::from_argb(cfg.data_color),
            label: RgbaColor::from_argb(cfg.label_color),
            unit: RgbaColor::from_argb(cfg.unit_color),
            hint: RgbaColor::from_argb(cfg.hint_color),
            alert: RgbaColor::from_argb(cfg.alert_color),
            // POI 文字/十字走的仍是 `hint_color`（**警示色**）：配置键 `poi_color` 已删除，
            // 这里保留 `poi` 这个"颜色角色"只是为了不动 4 处调用点（值 = hint）。
            poi: RgbaColor::from_argb(cfg.hint_color),
            stroke: RgbaColor::from_argb(cfg.stroke_color),
            background: RgbaColor::new(0, 0, 0, 0),
        }
    }

    #[inline]
    pub fn data_u32(&self) -> u32 {
        self.data.to_argb()
    }

    #[inline]
    pub fn label_u32(&self) -> u32 {
        self.label.to_argb()
    }

    #[inline]
    pub fn unit_u32(&self) -> u32 {
        self.unit.to_argb()
    }

    #[inline]
    pub fn hint_u32(&self) -> u32 {
        self.hint.to_argb()
    }

    #[inline]
    pub fn alert_u32(&self) -> u32 {
        self.alert.to_argb()
    }

    #[inline]
    pub fn poi_u32(&self) -> u32 {
        self.poi.to_argb()
    }

    #[inline]
    pub fn stroke_u32(&self) -> u32 {
        self.stroke.to_argb()
    }
}

impl Default for HudColors {
    fn default() -> Self {
        Self::palette()
    }
}

// Colors come from the layout config (loaded before the first draw), so a
// single JSON file styles every HUD. Falls back to `palette()` defaults.
pub static HUD_COLORS: LazyLock<HudColors, fn() -> HudColors> =
    LazyLock::new(|| HudColors::from_config(crate::hud_layout()));

