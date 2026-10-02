//! 面板几何的**单点真源**（纯函数，不碰窗口/缓冲，可直接单测）。
//!
//! 背景：擦除/绘制（`hud/hud.rs`）与拖拽命中测试（`lib.rs::DragState`）原本各自
//! 抄了一遍公式，于是漂移出三个真实 bug：
//!   1. 地图命中框把 `map_size_x` 当高度（非正方形地图拖拽起点偏移）
//!   2. miniHUD 命中框把字号写死成 `30/16`（用户改了 mini_font_size 就整体错位）
//!   3. 圆环命中半径只用 `circle_ring_radius`，而实际绘制半径还含字号项
//! 现在两边都调用这里的函数 —— 公式只存在一处。

use crate::{HudLayoutConfig, HudType};

/// 某个轴的落点：`v > 0` 用配置值，否则按 `fallback`（**`0` = 没设过 = 自动**）。
/// 全仓只有这一处定义"0 是自动"这条口径。
fn axis_or(v: i32, fallback: f32) -> f32 {
    if v > 0 {
        v as f32
    } else {
        fallback
    }
}

/// 地图面板尺寸 (宽, 高)：`map_size_*` 为 0 时按字号回退（15 个字宽）。
pub fn map_panel_size(cfg: &HudLayoutConfig) -> (f32, f32) {
    let w = if cfg.map_size_x > 0 { cfg.map_size_x as f32 } else { cfg.font_size * 15.0 };
    let h = if cfg.map_size_y > 0 { cfg.map_size_y as f32 } else { w };
    (w, h)
}

/// 地图面板默认 y（未显式配置时贴在飞行信息面板上方 10px）。
pub fn map_panel_default_y(cfg: &HudLayoutConfig, map_h: f32) -> f32 {
    (cfg.flight_info_y as f32 - map_h - 10.0).max(0.0)
}

/// miniHUD 的实际字号：全局字号 × (mini_font_size / 基准字号)。
pub fn mini_hud_font(cfg: &HudLayoutConfig) -> f32 {
    cfg.font_size * (cfg.mini_font_size / crate::DEFAULT_FONT_SIZE)
}

/// miniHUD 面板尺寸（与绘制用的 10×7 字宽一致）。
pub fn mini_hud_size(cfg: &HudLayoutConfig) -> (f32, f32) {
    let mf = mini_hud_font(cfg);
    (10.0 * mf, 7.0 * mf)
}

/// miniHUD **实际画出去的范围**相对锚点的外扩比例 `(左, 上)`，单位 = 每 1px 的 mini 字号。
///
/// 为什么需要它：锚点（`minihud_x/y`）是**油门竖条的左上角**，而
/// * 油门数字与它下面的标尺线画在竖条**左侧**（`draw_throttle_vertical` 里 `char_w * 3`）；
/// * 首行文本以基线定位，字形的上伸部高于锚点。
/// 只擦"锚点 + 10×7 字宽"那个矩形，这两条边上就会留下残影（用户报的"上边缘/右边缘没擦干净"）。
///
/// 比例由 `init_hud_layout` 用**真实字体**量一次（[`set_mini_hud_pads`]）；
/// 没量过（单测/异常路径）时用保守估计。
static MINI_PADS: std::sync::OnceLock<(f32, f32)> = std::sync::OnceLock::new();

/// 记录 miniHUD 的外扩比例（每 1px mini 字号对应多少 px），见 [`MINI_PADS`]。
pub fn set_mini_hud_pads(left_per_px: f32, top_per_px: f32) {
    MINI_PADS.set((left_per_px, top_per_px)).ok();
}

/// miniHUD 的外扩量（像素）：`(左, 上)`。
pub fn mini_hud_pads(cfg: &HudLayoutConfig) -> (f32, f32) {
    let mf = mini_hud_font(cfg);
    let (l, t) = MINI_PADS.get().copied().unwrap_or((0.42 * 3.0 + 0.05, 0.35));
    (l * mf, t * mf)
}

/// miniHUD 的**擦除 / 拖拽命中矩形** `(x, y, w, h)` —— 绘制、擦除、命中三处都用它。
///
/// 锚点仍是油门竖条左上角；矩形往左/往上扩 [`mini_hud_pads`] 那点距离，
/// 正好把数字列与字形上伸部包进来。
pub fn mini_hud_rect(cfg: &HudLayoutConfig, ax: f32, ay: f32) -> (f32, f32, f32, f32) {
    let (w, h) = mini_hud_size(cfg);
    let (pl, pt) = mini_hud_pads(cfg);
    (ax - pl, ay - pt, w + pl, h + pt)
}

/// HUD 本体（圆环 / miniHUD）的落点 —— **绘制与拖拽命中测试都必须走这里**。
///
/// `sx`/`sy` 传当前生效的那对坐标（拖拽模式下是拖拽槽，否则是配置值）：
/// * 圆环：坐标是**圆心**，`0` 的轴回退到窗口中心；
/// * miniHUD：坐标是**左上角**，`0` 的轴回退到"面板在窗口里居中"。
///
/// 两种样式各有各的坐标键与拖拽槽，互不影响；把回退规则放在这里，
/// 是为了避免"某个调用点忘了 `0` 要回退"—— 曾经 `--drag` 下圆环就因此画到 (0,0)。
pub fn hud_anchor(hud_type: HudType, sx: i32, sy: i32, cfg: &HudLayoutConfig, win: (f32, f32)) -> (f32, f32) {
    match hud_type {
        HudType::Circle => (axis_or(sx, win.0 / 2.0), axis_or(sy, win.1 / 2.0)),
        // Off 不画；与 Mini 同口径取个确定值，免得命中测试另写一套
        HudType::Mini | HudType::Off => {
            let (w, h) = mini_hud_size(cfg);
            (axis_or(sx, (win.0 - w) / 2.0), axis_or(sy, (win.1 - h) / 2.0))
        }
    }
}

/// 圆环 HUD 的绘制/命中半径：环半径 + 字号项（命中框必须与绘制一致）。
pub fn circle_radius(cfg: &HudLayoutConfig) -> f32 {
    cfg.circle_ring_radius + cfg.circle_font_size * 6.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> HudLayoutConfig {
        let mut c = HudLayoutConfig::default();
        c.font_size = 16.0;
        c.circle_ring_radius = 300.0;
        c.circle_font_size = 24.0;
        c.mini_font_size = 30.0;
        c
    }

    /// 回归：命中框曾经把 map_size_x 当成高度用
    #[test]
    fn map_panel_size_uses_both_axes() {
        let mut c = cfg();
        c.map_size_x = 360;
        c.map_size_y = 240;
        assert_eq!(map_panel_size(&c), (360.0, 240.0));
        // 两者都为 0 时按字号回退，且高宽相等
        let mut c0 = cfg();
        c0.map_size_x = 0;
        c0.map_size_y = 0;
        assert_eq!(map_panel_size(&c0), (240.0, 240.0));
        // 只给高度时宽度仍按字号回退
        let mut ch = cfg();
        ch.map_size_x = 0;
        ch.map_size_y = 500;
        assert_eq!(map_panel_size(&ch), (240.0, 500.0));
    }

    /// 回归：命中框曾经写死 mini_font_size=30
    #[test]
    fn mini_hud_font_follows_config() {
        let mut c = cfg();
        assert_eq!(mini_hud_font(&c), 30.0);
        c.mini_font_size = 48.0;
        assert_eq!(mini_hud_font(&c), 48.0, "改了 mini_font_size 命中框就该跟着变");
        assert_eq!(mini_hud_size(&c), (480.0, 336.0));
    }

    /// 回归：命中半径曾经漏掉字号项（300 vs 实际 444）
    #[test]
    fn circle_radius_includes_font_term() {
        let c = cfg();
        assert_eq!(circle_radius(&c), 444.0);
    }

    /// HUD 落点：圆环按圆心、mini 按左上角，`0` 都回退到"窗口里居中"；
    /// 两种样式各自的坐标互不影响（回归：`--drag` 下圆环曾经落到 (0,0)）。
    #[test]
    fn hud_anchor_falls_back_per_axis_and_keeps_styles_separate() {        let mut c = cfg();
        c.mini_font_size = 30.0; // mini 尺寸 = 300×210
        // 都没设过 → 各自居中
        assert_eq!(hud_anchor(HudType::Circle, 0, 0, &c, (1000.0, 600.0)), (500.0, 300.0));
        assert_eq!(hud_anchor(HudType::Mini, 0, 0, &c, (1000.0, 600.0)), (350.0, 195.0));
        // 各自的坐标只影响自己
        assert_eq!(hud_anchor(HudType::Circle, 700, 200, &c, (1000.0, 600.0)), (700.0, 200.0));
        assert_eq!(hud_anchor(HudType::Mini, 120, 340, &c, (1000.0, 600.0)), (120.0, 340.0));
        // 逐轴回退：只设了一半也各自回退
        assert_eq!(hud_anchor(HudType::Mini, 120, 0, &c, (1000.0, 600.0)), (120.0, 195.0));
        assert_eq!(hud_anchor(HudType::Circle, 0, 200, &c, (1000.0, 600.0)), (500.0, 200.0));
    }
}
