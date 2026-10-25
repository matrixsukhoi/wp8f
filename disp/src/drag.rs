//! 可拖拽面板的**内存态**：命中测试、位置槽（`Slot`）、偏移与写回配置。
//!
//! 从 `lib.rs` 拆出的一块；对外路径不变（`lib.rs` 有 `pub use drag::{...}`）。
//! 这里只算"拖到哪"，不碰窗口、不碰盘：落盘在 `layout.rs`（`save_drag_state`）。

use super::{get_window_size, HudLayoutConfig, HudType};

static DRAG_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_drag_mode(enabled: bool) {
    DRAG_MODE.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub fn is_drag_mode() -> bool {
    DRAG_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

/// 可拖拽面板（顺序 = 命中测试的优先级）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Flight,
    Engine,
    Map,
    /// 圆环 / miniHUD 共用的那一格，落点由 `hud_type` 决定
    Hud,
    Link,
}

impl Panel {
    pub const ALL: [Panel; 5] = [Panel::Flight, Panel::Engine, Panel::Map, Panel::Hud, Panel::Link];

    /// 位置存哪个槽：`Hud` 按 `hud_type` 分叉，其余一一对应。
    /// 关闭 HUD（`Off`）时 `Hud` 那一格不存在（命中测试里已剔除），落到 mini 槽只为取个确定值。
    fn slot(self, hud_type: HudType) -> Slot {
        match (self, hud_type) {
            (Panel::Flight, _) => Slot::Flight,
            (Panel::Engine, _) => Slot::Engine,
            (Panel::Map, _) => Slot::Map,
            (Panel::Hud, HudType::Circle) => Slot::Circle,
            (Panel::Hud, HudType::Mini | HudType::Off) => Slot::MiniHud,
            (Panel::Link, _) => Slot::Link,
        }
    }
}

/// 拖拽位置的存储槽（比面板多一个：miniHUD 与圆环各存各的位置）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Flight,
    Engine,
    Map,
    MiniHud,
    Circle,
    Link,
}

impl Slot {
    const ALL: [Slot; 6] =
        [Slot::Flight, Slot::Engine, Slot::Map, Slot::MiniHud, Slot::Circle, Slot::Link];
    const N: usize = Self::ALL.len();

    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

#[derive(Debug, Clone)]
pub struct DragState {
    /// 正在拖的面板（`None` = 没在拖）
    pub active: Option<Panel>,
    pub off_x: i32,
    pub off_y: i32,
    pos: [[i32; 2]; Slot::N],
}

impl DragState {
    pub fn from_config(cfg: &HudLayoutConfig) -> Self {
        // 单点真源：与绘制侧（hud.rs）用同一个公式，别再各抄一遍
        let (_, map_h) = crate::geometry::map_panel_size(cfg);
        let map_y = if cfg.map_y > 0 { cfg.map_y } else { crate::geometry::map_panel_default_y(cfg, map_h) as i32 };
        let mut pos = [[0i32; 2]; Slot::N];
        let mut set = |slot: Slot, x: i32, y: i32| pos[slot.index()] = [x, y];
        set(Slot::Flight, cfg.flight_info_x, cfg.flight_info_y);
        set(Slot::Engine, cfg.engine_info_x, cfg.engine_info_y);
        set(Slot::Map, cfg.map_x, map_y);
        set(Slot::MiniHud, cfg.minihud_x, cfg.minihud_y);
        set(Slot::Circle, cfg.circle_x, cfg.circle_y);
        set(Slot::Link, cfg.datalink_x, cfg.datalink_y);
        Self { active: None, off_x: 0, off_y: 0, pos }
    }

    /// 某个槽的位置（绘制侧按槽取）。
    pub fn slot_pos(&self, slot: Slot) -> (i32, i32) {
        let [x, y] = self.pos[slot.index()];
        (x, y)
    }

    /// 面板位置。`hud_type` 由调用方传入 —— 纯访问器，不读全局。
    pub fn panel_pos(&self, panel: Panel, hud_type: HudType) -> (i32, i32) {
        self.slot_pos(panel.slot(hud_type))
    }

    pub fn set_panel_pos(&mut self, panel: Panel, hud_type: HudType, x: i32, y: i32) {
        self.pos[panel.slot(hud_type).index()] = [x, y];
    }

    /// 把各槽位置写回布局配置（纯函数：不读全局、不落盘 —— 落盘在 [`save_drag_state`]）。
    ///
    /// **每个面板只写自己的键**：miniHUD → `minihud_x/y`，圆环 → `circle_x/y`，
    /// 两者各有各的拖拽槽，任何一边拖动都不会碰到另一边（有单测钉着）。
    pub fn apply_to(&self, cfg: &mut HudLayoutConfig) {
        let put = |slot: Slot| self.slot_pos(slot);
        let (x, y) = put(Slot::Flight);
        cfg.flight_info_x = x;
        cfg.flight_info_y = y;
        let (x, y) = put(Slot::Engine);
        cfg.engine_info_x = x;
        cfg.engine_info_y = y;
        let (x, y) = put(Slot::Map);
        cfg.map_x = x;
        cfg.map_y = y;
        let (x, y) = put(Slot::MiniHud);
        cfg.minihud_x = x;
        cfg.minihud_y = y;
        let (x, y) = put(Slot::Circle);
        cfg.circle_x = x;
        cfg.circle_y = y;
        let (x, y) = put(Slot::Link);
        cfg.datalink_x = x;
        cfg.datalink_y = y;
    }

    pub fn handle_press(&mut self, mx: i32, my: i32) {
        let cfg = crate::hud_layout();
        // 命中半径必须与 hud.rs 的绘制半径同源（含字号项）
        let r = crate::geometry::circle_radius(cfg) as i32;
        let (ww, wh) = get_window_size();
        // HUD 本体的落点：与绘制走**同一个** `hud_anchor`（含"0 = 自动"的逐轴回退），
        // 拖拽模式下的输入就是各自的拖拽槽。曾经这里与 hud.rs 各写一套，漂过两次。
        let (hx_slot, hy_slot) = self.slot_pos(if cfg.hud_type == HudType::Circle { Slot::Circle } else { Slot::MiniHud });
        let (ax, ay) = crate::geometry::hud_anchor(cfg.hud_type, hx_slot, hy_slot, cfg, (ww, wh));
        let (hud_x, hud_y) = (ax as i32, ay as i32);
        // miniHUD 的命中框 = 它的真实足迹（与擦除/绘制同一个 `mini_hud_rect`）
        let (mini_x, mini_y, mini_w, mini_h) = crate::geometry::mini_hud_rect(cfg, ax, ay);
        let (map_w, map_h) = crate::geometry::map_panel_size(cfg);

        let (flight_x, flight_y) = self.slot_pos(Slot::Flight);
        let (engine_x, engine_y) = self.slot_pos(Slot::Engine);
        let (map_x, map_y) = self.slot_pos(Slot::Map);
        let (link_x, link_y) = self.slot_pos(Slot::Link);
        let lf = crate::hud::link_panel::panel_font(cfg.font_size);

        struct Rect { panel: Panel, x: i32, y: i32, w: u32, h: u32 }
        // HUD 本体那一格按 `hud_type` 变：关闭（0）时**没有这一格** —— 不画的东西不给拖。
        let hud_rect = match cfg.hud_type {
            HudType::Off => None,
            HudType::Circle => Some(Rect { panel: Panel::Hud, x: hud_x - r, y: hud_y - r, w: (2 * r) as u32, h: (2 * r) as u32 }),
            HudType::Mini => Some(Rect { panel: Panel::Hud, x: mini_x as i32, y: mini_y as i32, w: mini_w as u32, h: mini_h as u32 }),
        };
        let panels = [
            Rect { panel: Panel::Flight, x: flight_x, y: flight_y, w: (cfg.font_size * 12.0 * cfg.flight_info_col as f32 + 30.0) as u32, h: (cfg.font_size * 5.0 + 25.0) as u32 },
            Rect { panel: Panel::Engine, x: engine_x, y: engine_y, w: (cfg.font_size * 12.0 * cfg.engine_info_col as f32 + 30.0) as u32, h: (cfg.font_size * 10.0 + 25.0) as u32 },
            Rect { panel: Panel::Map, x: map_x, y: map_y, w: map_w as u32, h: map_h as u32 },
        ];
        let panels: Vec<Rect> = panels.into_iter().chain(hud_rect).chain([Rect {
            panel: Panel::Link,
            x: link_x,
            y: link_y,
            w: crate::hud::link_panel::panel_width(lf) as u32,
            h: crate::hud::link_panel::panel_height(lf) as u32,
        }]).collect();
        for p in &panels {
            if mx >= p.x && mx < p.x + p.w as i32 && my >= p.y && my < p.y + p.h as i32 {
                self.active = Some(p.panel);
                if p.panel == Panel::Hud && cfg.hud_type == HudType::Circle {
                    self.off_x = mx - hud_x;
                    self.off_y = my - hud_y;
                } else {
                    self.off_x = mx - p.x;
                    self.off_y = my - p.y;
                }
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
