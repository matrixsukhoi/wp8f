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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;
use thiserror::Error;
use winit::event_loop::EventLoopProxy;

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

static HUD_LAYOUT: OnceLock<HudLayoutConfig> = OnceLock::new();

/// 启动时把布局配置 + **两份字体**定下来（**HUD 标签语言表不在这里**：那是
/// `core/src/main.rs` 的初始化步骤，见 [`i18n::init`]）。
///
/// `Err` = 字体一份都没读到（[`font::missing_error`]）：HUD 一个字都画不出来，调用方应当把
/// 这句话打印出来并 `exit(2)`。其它情况下永远是 `Ok(())`。
pub fn init_hud_layout(mut config: HudLayoutConfig) -> Result<(), String> {
    // 列数为 0 会让网格布局除零（渲染线程崩）
    config.flight_info_col = config.flight_info_col.max(1);
    config.engine_info_col = config.engine_info_col.max(1);
    // 字体在这里从磁盘读进内存，必须在 `HUD_LAYOUT.set` 之前：
    // `FONT` 的 thread_local 初始化器读的是 `font::data()`。
    let picked = font::init(&config.font_path);
    let icon = font::icon_init();
    for c in [picked, icon] {
        // 读不到的那一份按 warn 记（INFO 里混一条"⚠ …"会让人以为只是个提示）
        if c.source == font::FontSource::Missing {
            tracing::warn!("[DISP] font: {}", c.describe());
        } else {
            tracing::info!("[DISP] font: {}", c.describe());
        }
    }
    if let Some(msg) = font::missing_error() {
        // 缺字体 → 不启动。消息由调用方打印并 `exit(2)`：这里再打一遍就是两遍，
        // 而上面那两条 warn 已经把"缺哪一份"记进日志了。
        return Err(msg);
    }
    // miniHUD 的真实足迹比它的锚点矩形大一圈（左侧油门数字列 + 首行字形上伸部）：
    // 用**刚读进来的字体**量一次比例，交给 geometry —— 擦除与命中据此外扩，
    // 否则那两条边上的像素擦不掉（用户报的残影）。
    if let Ok(f) = ab_glyph::FontRef::try_from_slice(font::data()) {
        let (l, t) = mini_hud::measure_pads(&f, geometry::mini_hud_font(&config));
        geometry::set_mini_hud_pads(l, t);
    }
    tracing::info!(
        "[DISP] font 映射 {} 字节（正文 {} + 图标 {}；mmap 按需驻留，每帧查表不碰磁盘）",
        font::resident_bytes(),
        picked.bytes,
        icon.bytes
    );
    tracing::info!(
        "[DISP] init_hud_layout: hud_type={}({}), font_size={}, flight_info=({},{},col={}), engine_info=({},{},col={}), minihud=({},{}), circle=({},{}), map=({},{}), map_size=({},{})",
        config.hud_type.as_u8(),
        match config.hud_type {
            HudType::Off => "关闭",
            HudType::Mini => "MiniHUD",
            HudType::Circle => "CircleHUD",
        },
        config.font_size,
        config.flight_info_x,
        config.flight_info_y,
        config.flight_info_col,
        config.engine_info_x,
        config.engine_info_y,
        config.engine_info_col,
        config.minihud_x,
        config.minihud_y,
        config.circle_x,
        config.circle_y,
        config.map_x,
        config.map_y,
        config.map_size_x,
        config.map_size_y,
    );
    HUD_LAYOUT.set(config).ok();
    Ok(())
}

pub fn hud_layout() -> &'static HudLayoutConfig {
    HUD_LAYOUT.get_or_init(|| HudLayoutConfig::default())
}

static LAYOUT_CONFIG_PATH: OnceLock<String> = OnceLock::new();

pub fn set_layout_config_path(path: String) {
    LAYOUT_CONFIG_PATH.set(path).ok();
}

pub fn load_layout_config(path: &str) -> Option<HudLayoutConfig> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn save_layout_config(cfg: &HudLayoutConfig) {
    let Some(path) = LAYOUT_CONFIG_PATH.get() else {
        // 没传 `--layoutconfig` 时没有落盘目标：拖拽改动只留在内存里 —— 必须说清楚，
        // 否则用户"拖完切走再切回来位置复原"时会以为拖拽没生效。
        tracing::warn!("[DISP] 布局配置没有路径（未传 --layoutconfig），本次改动只留在内存里");
        return;
    };
    let json = match serde_json::to_string_pretty(cfg) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!("[DISP] 布局配置序列化失败：{e}");
            return;
        }
    };
    if let Err(e) = std::fs::write(path, json) {
        tracing::warn!("[DISP] 写布局配置失败（{path}）：{e}");
    }
}

/// 拖拽结束后把各槽位置写回布局配置并落盘。
///
/// **先读盘上最新的那份配置，再只改位置键**：HUD 在预览里跑着的时候，控制台那边可能
/// 刚改过别的键（颜色/告警开关…）并落了盘。直接拿启动时的内存快照整份写回，会把控制台
/// 的改动盖掉（反过来也一样：控制台别拿旧副本整份写回，它会在窗口重新获得焦点时重载配置）。
pub fn save_drag_state(drag: &crate::DragState) {
    let mut cfg = LAYOUT_CONFIG_PATH
        .get()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<HudLayoutConfig>(&s).ok())
        .unwrap_or_else(|| hud_layout().clone());
    drag.apply_to(&mut cfg);
    save_layout_config(&cfg);
    // 拖拽松手/ESC/关窗都会走到这里：打一行日志，用户与排查都能确认"确实落盘了"。
    tracing::info!(
        "[DISP] 拖拽已保存（{}）：minihud=({}, {}) circle=({}, {}) map=({}, {}) flight=({}, {}) engine=({}, {}) link=({}, {})",
        LAYOUT_CONFIG_PATH.get().map(String::as_str).unwrap_or("(无路径)"),
        cfg.minihud_x, cfg.minihud_y, cfg.circle_x, cfg.circle_y,
        cfg.map_x, cfg.map_y, cfg.flight_info_x, cfg.flight_info_y,
        cfg.engine_info_x, cfg.engine_info_y, cfg.datalink_x, cfg.datalink_y,
    );
}

#[derive(Error, Debug)]
pub enum DispError {
    #[error("Window error: {0}")]
    WindowError(String),
}

pub use crate::components::HUD_COLORS;

static WINDOW_VISIBLE_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_window_visible(visible: bool) {
    WINDOW_VISIBLE_REQUESTED.store(visible, std::sync::atomic::Ordering::Relaxed);
    if let Some(proxy) = PROXY.get() {
        let _ = proxy.wake_up();
    }
    tracing::debug!("[DISP] set_window_visible({})", visible);
}

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

struct DispWindow {
    running: Arc<std::sync::atomic::AtomicBool>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

type DataRingBuffer = wp8f_ring::Ring<DisplayData>;
type MapDataRingBuffer = wp8f_ring::Ring<MapDisplay>;

/// 一次轮询的统计结果（`FrameReader::poll_latest`）：`frame` 是最新一帧的**借用**
/// （直读缓冲区，零拷贝；本轮没有新帧则 `None`）；`skipped` 是被跳过的历史帧数
/// （`frame=Some` 时 = 本轮新帧数 - 1）；`overwritten` = 新帧数超过环形容量；
/// `reset` = 检测到生产端重置（帧序号断流）。
#[derive(Debug, Clone, Copy, Default)]
pub struct PollOutcome {
    pub frame: Option<&'static DisplayData>,
    pub skipped: u64,
    pub overwritten: bool,
    pub reset: bool,
}

/// 环形缓冲区的**消费端句柄**（logger 线程持有）。可 `Send`：内部只有一个 `&'static` 与游标。
///
/// 消费语义是"只取最新帧、不 drain 历史帧"：记录频率 = 轮询频率，logger 不会积压。
pub struct FrameReader {
    rb: &'static DataRingBuffer,
    cursor: wp8f_ring::Cursor,
}

impl FrameReader {
    fn new(rb: &'static DataRingBuffer) -> Self {
        // 起点与"当前"同步：本句柄只关心创建之后写入的帧（logger 每个连接新建一次，
        // 不该补记上一个连接的帧，也不该把创建前刚发生的重置报成本次连接的 reset）。
        Self { rb, cursor: rb.cursor() }
    }

    /// 只取最新一帧的**借用**（零拷贝、零锁）：不推进生产端游标、不清历史帧。
    ///
    /// 内存序：索引/计数按 `Acquire` 读、`commit()` 按 `Release` 发布；生产端只写
    /// `next_slot()` 的槽、不回写已发布的槽 → 拿到的总是已提交的完整帧。
    /// 例外：读者被延迟到生产端绕满一整圈（`ring.frames` 个槽，缺省 32 帧 ≈ 1 s @30 Hz）
    /// 后改写同一槽，
    /// 这时读到的是新旧混合的数据 —— **这是"零拷贝直读"接受的代价**（`DisplayData` 是 POD，
    /// 混读只影响这一帧）。
    pub fn poll_latest(&mut self) -> PollOutcome {
        let out = self.rb.poll(&mut self.cursor);
        if out.reset {
            // 生产端重置过（每次重连都会调 `reset_ring_buffer()`）：之前的帧序号全部失效 ——
            // 重置本身不是一帧数据
            return PollOutcome { reset: true, ..Default::default() };
        }
        match out.value {
            None => PollOutcome::default(),
            Some(frame) => PollOutcome {
                frame: Some(frame),
                skipped: out.skipped,
                overwritten: out.skipped + 1 > self.rb.slots() as u64,
                reset: false,
            },
        }
    }
}

/// 取 `DisplayData` 环形缓冲区的消费端（logger 线程用）。
/// 窗口未创建（`RING_BUFFER` 未初始化）时返回 `None`。
pub fn frame_reader() -> Option<FrameReader> {
    RING_BUFFER.get().map(|rb| FrameReader::new(rb.as_ref()))
}

pub fn reset_ring_buffer() {
    if let Some(rb) = RING_BUFFER.get() {
        rb.reset();
    }
}

static DISP_WINDOW: std::sync::Mutex<Option<DispWindow>> = std::sync::Mutex::new(None);
static RING_BUFFER: OnceLock<Arc<DataRingBuffer>> = OnceLock::new();
/// 地图快照槽（4 Hz 写、渲染线程每帧读）。载荷 `MapDisplay` 里的 `map_objects: Vec` 是堆指针，
/// 是对 `ring` 的 POD 契约的**有意例外**：读者要落后 32 个地图采样（≈8 s）才会撞上写端的
/// `clear()`，实测不可能发生（渲染线程每帧都来取），按可容许处理。
/// 底图**不在**这个槽里（见 [`MapImage`]：环形槽轮着用，进环就得每次发布重写）。
static MAP_RING_BUFFER: OnceLock<Arc<MapDataRingBuffer>> = OnceLock::new();

pub fn create_window() -> Result<(), DispError> {
    tracing::debug!("[DISP] create_window() called");

    {
        let mut disp = DISP_WINDOW.lock().unwrap();
        if let Some(old_disp) = disp.take() {
            tracing::warn!("[DISP] Window already exists, destroying first");
            old_disp
                .running
                .store(false, std::sync::atomic::Ordering::Relaxed);
            if let Some(handle) = old_disp.thread_handle {
                let _ = handle.join();
            }
        }
    }

    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    // 槽数来自配置（`ring` 段，已钳制）：这里是唯一的分配点，运行期不再分配
    let ring_buffer = Arc::new(DataRingBuffer::with_slots(hud_layout().ring.frames_clamped()));
    let ring_buffer_for_display = ring_buffer.clone();
    let ring_buffer_for_core = ring_buffer.clone();

    let thread_handle = thread::spawn({
        let running = running.clone();
        move || {
            run_window(ring_buffer_for_display, running);
        }
    });

    RING_BUFFER.set(ring_buffer_for_core).ok();
    MAP_RING_BUFFER
        .set(Arc::new(MapDataRingBuffer::with_slots(hud_layout().ring.map_samples_clamped())))
        .ok();
    {
        let mut disp = DISP_WINDOW.lock().unwrap();
        *disp = Some(DispWindow {
            running,
            thread_handle: Some(thread_handle),
        });
    }

    thread::sleep(Duration::from_millis(100));
    Ok(())
}

static WINDOW_SIZE: OnceLock<(f32, f32)> = OnceLock::new();

pub fn set_window_size(w: f32, h: f32) {
    WINDOW_SIZE.set((w, h)).ok();
}

pub(crate) fn get_window_size() -> (f32, f32) {
    *WINDOW_SIZE.get().unwrap_or(&(2560.0, 1440.0))
}

#[cfg(target_os = "windows")]
fn run_window(ring_buffer: Arc<DataRingBuffer>, running: Arc<std::sync::atomic::AtomicBool>) {
    use softbuffer::{Context, Surface};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::mem;
    use std::num::NonZeroU32;
    use winit::event_loop::EventLoop;
    use winit::platform::windows::EventLoopBuilderExtWindows;
    use winit::window::WindowId;

    let mut builder = EventLoop::builder();
    builder.with_any_thread(true);
    let event_loop = builder.build().expect("Failed to create event loop");

    PROXY.set(event_loop.create_proxy()).ok();
    tracing::debug!("[DISP] Window created successfully");

    let running = running.clone();

    thread_local! {
        static GC: RefCell<Option<GraphicsContext>> = const { RefCell::new(None) };
    }

    struct GraphicsContext {
        ctx: RefCell<Context<&'static dyn winit::window::Window>>,
        surf: HashMap<
            WindowId,
            Surface<&'static dyn winit::window::Window, &'static dyn winit::window::Window>,
        >,
    }

    impl GraphicsContext {
        fn new(w: &dyn winit::window::Window) -> Self {
            unsafe {
                Self {
                    ctx: RefCell::new(Context::new(mem::transmute(w)).expect("ctx")),
                    surf: HashMap::new(),
                }
            }
        }
        fn get_surf(
            &mut self,
            w: &dyn winit::window::Window,
        ) -> &mut Surface<&'static dyn winit::window::Window, &'static dyn winit::window::Window>
        {
            self.surf.entry(w.id()).or_insert_with(|| unsafe {
                Surface::new(&self.ctx.borrow(), mem::transmute(w)).expect("surf")
            })
        }
    }

    struct App {
        window: Option<Box<dyn winit::window::Window>>,
        ring_buffer: Arc<DataRingBuffer>,
        running: Arc<std::sync::atomic::AtomicBool>,
        last_visible: bool,
        drag_state: DragState,
    }

    impl winit::application::ApplicationHandler for App {
        fn resumed(&mut self, _: &dyn winit::event_loop::ActiveEventLoop) {}

        fn can_create_surfaces(&mut self, el: &dyn winit::event_loop::ActiveEventLoop) {
            use winit::window::WindowLevel;
            let (width, height) = get_window_size();
            let drag_mode = crate::is_drag_mode();

            let attr = winit::window::WindowAttributes::default()
                .with_title("WP8F HUD")
                .with_surface_size(winit::dpi::LogicalSize::new(width, height))
                .with_decorations(false)
                .with_transparent(true)
                .with_visible(true)
                .with_active(drag_mode)
                .with_window_level(WindowLevel::AlwaysOnTop);

            self.window = el.create_window(attr).ok();
            if let Some(ref w) = self.window {
                if !drag_mode {
                    let _ = w.set_cursor_visible(false);
                }

                #[cfg(target_os = "windows")]
                {
                    use winit::platform::windows::WindowExtWindows;
                    w.set_skip_taskbar(true);
                }

                if !drag_mode {
                    let _ = w.set_cursor_hittest(false);
                }

                let _ = w.set_outer_position(winit::dpi::Position::Physical(
                    winit::dpi::PhysicalPosition::new(0, 0),
                ));
            }
        }

        fn window_event(
            &mut self,
            tgt: &dyn winit::event_loop::ActiveEventLoop,
            _: WindowId,
            evt: winit::event::WindowEvent,
        ) {
            use winit::event::WindowEvent;
            match evt {
                WindowEvent::CloseRequested => {
                    // 预览里点窗口关闭（或 Alt+F4）也要先把拖拽结果落盘（ESC 那条早有保存）
                    if crate::is_drag_mode() {
                        crate::save_drag_state(&self.drag_state);
                    }
                    tgt.exit();
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if crate::is_drag_mode()
                        && event.state == winit::event::ElementState::Pressed
                        && event.physical_key
                            == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Escape)
                    {
                        // ESC：保存布局并直接退出进程
                        crate::save_drag_state(&self.drag_state);
                        std::process::exit(0);
                    }
                }
                WindowEvent::PointerMoved { position, .. } => {
                    if crate::is_drag_mode() {
                        if let Some(panel) = self.drag_state.active {
                            let (x, y) = (position.x as i32 - self.drag_state.off_x,
                                          position.y as i32 - self.drag_state.off_y);
                            self.drag_state.set_panel_pos(panel, crate::hud_layout().hud_type, x, y);
                        }
                    }
                }
                WindowEvent::PointerButton { state: winit::event::ElementState::Pressed, position, .. } => {
                    if crate::is_drag_mode() {
                        self.drag_state.handle_press(position.x as i32, position.y as i32);
                    }
                }
                WindowEvent::PointerButton { state: winit::event::ElementState::Released, .. } => {
                    if self.drag_state.active.is_some() {
                        crate::save_drag_state(&self.drag_state);
                    }
                    self.drag_state.active = None;
                }
                WindowEvent::RedrawRequested => {
                    GC.with(|gc| {
                        let Some(ref win) = self.window else { return };
                        let sz = win.surface_size();
                        let (Some(w), Some(h)) =
                            (NonZeroU32::new(sz.width), NonZeroU32::new(sz.height))
                        else {
                            return;
                        };
                        let ww = w.get() as usize;
                        let hh = h.get() as usize;
                        let mut gc = gc.borrow_mut();
                        let s = gc
                            .get_or_insert_with(|| GraphicsContext::new(win.as_ref()))
                            .get_surf(win.as_ref());
                        s.resize(w, h).ok();
                        let mut buf = s.buffer_mut().expect("buf");
                        crate::hud::draw_hud(&mut buf, ww, hh, self.ring_buffer.latest(), &self.drag_state);
                        buf.present().ok();
                    });
                }
                _ => {}
            }
        }

        fn about_to_wait(&mut self, tgt: &dyn winit::event_loop::ActiveEventLoop) {
            use std::sync::atomic::Ordering;
            let visible_requested =
                WINDOW_VISIBLE_REQUESTED.load(std::sync::atomic::Ordering::Relaxed);

            if let Some(ref w) = self.window {
                if visible_requested != self.last_visible {
                    self.last_visible = visible_requested;
                    let _ = w.set_visible(visible_requested);
                }
                if FRAME_READY.swap(false, Ordering::Acquire) {
                    w.request_redraw();
                }
                tgt.set_control_flow(winit::event_loop::ControlFlow::Wait);
            }
            if !self.running.load(std::sync::atomic::Ordering::Relaxed) {
                tgt.exit();
            }
        }
    }

    let _ = event_loop.run_app(App {
        window: None,
        ring_buffer,
        running,
        last_visible: true,
        drag_state: DragState::from_config(&crate::hud_layout()),
    });
}

#[cfg(not(target_os = "windows"))]
fn run_window(ring_buffer: Arc<DataRingBuffer>, running: Arc<std::sync::atomic::AtomicBool>) {
    use softbuffer::Surface;
    use std::cell::RefCell;
    use std::num::NonZeroU32;
    use winit::event_loop::EventLoop;
    use winit::window::WindowId;

    let event_loop = EventLoop::builder()
        .build()
        .expect("Failed to create event loop");
    PROXY.set(event_loop.create_proxy()).ok();
    let running = running.clone();

    thread_local! {
        static GC: RefCell<Option<GraphicsContext>> = const { RefCell::new(None) };
    }

    struct GraphicsContext {
        ctx: softbuffer::Context<&'static dyn winit::window::Window>,
    }

    impl GraphicsContext {
        fn new(w: &dyn winit::window::Window) -> Self {
            unsafe {
                Self {
                    ctx: softbuffer::Context::new(std::mem::transmute(w)).expect("ctx"),
                }
            }
        }
    }

    struct App {
        window: Option<Box<dyn winit::window::Window>>,
        ring_buffer: Arc<DataRingBuffer>,
        running: Arc<std::sync::atomic::AtomicBool>,
        last_visible: bool,
        drag_state: DragState,
    }

    impl winit::application::ApplicationHandler for App {
        fn resumed(&mut self, _: &dyn winit::event_loop::ActiveEventLoop) {}
        fn can_create_surfaces(&mut self, el: &dyn winit::event_loop::ActiveEventLoop) {
            use winit::window::WindowLevel;
            self.window = el
                .create_window(
                    winit::window::WindowAttributes::default()
                        .with_title("WP8F HUD")
                        .with_surface_size(winit::dpi::LogicalSize::new(
                            get_window_size().0,
                            get_window_size().1,
                        ))
                        .with_decorations(false)
                        .with_transparent(true)
                        .with_visible(true)
                        .with_window_level(WindowLevel::AlwaysOnTop),
                )
                .ok();
        }
        fn window_event(
            &mut self,
            tgt: &dyn winit::event_loop::ActiveEventLoop,
            _: WindowId,
            evt: winit::event::WindowEvent,
        ) {
            use winit::event::WindowEvent;
            match evt {
                WindowEvent::CloseRequested => {
                    // 预览里点窗口关闭（或 Alt+F4）也要先把拖拽结果落盘（ESC 那条早有保存）
                    if crate::is_drag_mode() {
                        crate::save_drag_state(&self.drag_state);
                    }
                    tgt.exit();
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if crate::is_drag_mode()
                        && event.state == winit::event::ElementState::Pressed
                        && event.physical_key
                            == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Escape)
                    {
                        // ESC：保存布局并直接退出进程
                        crate::save_drag_state(&self.drag_state);
                        std::process::exit(0);
                    }
                }
                WindowEvent::PointerMoved { position, .. } => {
                    if crate::is_drag_mode() {
                        if let Some(panel) = self.drag_state.active {
                            let (x, y) = (position.x as i32 - self.drag_state.off_x,
                                          position.y as i32 - self.drag_state.off_y);
                            self.drag_state.set_panel_pos(panel, crate::hud_layout().hud_type, x, y);
                        }
                    }
                }
                WindowEvent::PointerButton { state: winit::event::ElementState::Pressed, position, .. } => {
                    if crate::is_drag_mode() {
                        self.drag_state.handle_press(position.x as i32, position.y as i32);
                    }
                }
                WindowEvent::PointerButton { state: winit::event::ElementState::Released, .. } => {
                    if self.drag_state.active.is_some() {
                        crate::save_drag_state(&self.drag_state);
                    }
                    self.drag_state.active = None;
                }
                WindowEvent::RedrawRequested => {
                    GC.with(|gc| {
                        let Some(ref w) = self.window else { return };
                        let sz = w.surface_size();
                        let (Some(nw), Some(h)) =
                            (NonZeroU32::new(sz.width), NonZeroU32::new(sz.height))
                        else {
                            return;
                        };
                        let ww = nw.get() as usize;
                        let hh = h.get() as usize;
                        let mut gc = gc.borrow_mut();
                        let ctx = gc.get_or_insert_with(|| GraphicsContext::new(w.as_ref()));
                        let mut surf: softbuffer::Surface<
                            &dyn winit::window::Window,
                            &dyn winit::window::Window,
                        > = unsafe {
                            Surface::new(&ctx.ctx, std::mem::transmute(w.as_ref())).expect("surf")
                        };
                        surf.resize(nw, h).ok();
                        let mut buf = surf.buffer_mut().expect("buf");
                        crate::hud::draw_hud(&mut buf, ww, hh, self.ring_buffer.latest(), &self.drag_state);
                        buf.present().ok();
                    });
                }
                _ => {}
            }
        }
        fn about_to_wait(&mut self, tgt: &dyn winit::event_loop::ActiveEventLoop) {
            use std::sync::atomic::Ordering;
            let visible_requested =
                WINDOW_VISIBLE_REQUESTED.load(std::sync::atomic::Ordering::Relaxed);

            if let Some(ref w) = self.window {
                if visible_requested != self.last_visible {
                    self.last_visible = visible_requested;
                    let _ = w.set_visible(visible_requested);
                }
                if FRAME_READY.swap(false, Ordering::Acquire) {
                    w.request_redraw();
                }
                tgt.set_control_flow(winit::event_loop::ControlFlow::Wait);
            }
            if !self.running.load(std::sync::atomic::Ordering::Relaxed) {
                tgt.exit();
            }
        }
    }

    let _ = event_loop.run_app(App {
        window: None,
        ring_buffer,
        running,
        last_visible: true,
        drag_state: DragState::from_config(&crate::hud_layout()),
    });
}

pub fn destroy_window() {
    tracing::info!("[DISP] destroy_window() called");
    if let Ok(mut disp) = DISP_WINDOW.lock() {
        if let Some(old_disp) = disp.take() {
            old_disp
                .running
                .store(false, std::sync::atomic::Ordering::Relaxed);
            if let Some(proxy) = PROXY.get() {
                let _ = proxy.wake_up();
            }
            if let Some(handle) = old_disp.thread_handle {
                let _ = handle.join();
            }
        }
    }
    thread::sleep(Duration::from_millis(500));
}

static PROXY: OnceLock<EventLoopProxy> = OnceLock::new();
static FRAME_READY: AtomicBool = AtomicBool::new(false);

pub fn signal_redraw() {
    FRAME_READY.store(true, Ordering::Release);
}

pub fn begin_frame() -> Option<&'static mut DisplayData> {
    RING_BUFFER.get().map(|rb| rb.next_slot())
}

pub fn commit_frame() {
    if let Some(rb) = RING_BUFFER.get() {
        rb.commit();
        signal_redraw();
        if let Some(proxy) = PROXY.get() {
            let _ = proxy.wake_up();
        }
    }
}

pub fn begin_map_frame() -> Option<&'static mut MapDisplay> {
    MAP_RING_BUFFER.get().map(|rb| rb.next_slot())
}

pub fn commit_map_frame() {
    if let Some(rb) = MAP_RING_BUFFER.get() {
        rb.commit();
        signal_redraw();
        if let Some(proxy) = PROXY.get() {
            let _ = proxy.wake_up();
        }
    }
}

pub fn get_map_data() -> Option<&'static MapDisplay> {
    MAP_RING_BUFFER.get().map(|rb| rb.latest())
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

    /// 帧环槽数由配置决定（`ring.frames`）：`overwritten` 按**实际槽数**算，不再写死 32。
    #[test]
    fn frame_reader_overwrite_threshold_follows_configured_slots() {
        let rb: &'static DataRingBuffer = Box::leak(Box::new(DataRingBuffer::with_slots(4)));
        assert_eq!(rb.slots(), 4);
        let mut r = FrameReader::new(rb);
        write_frames(rb, 1, 4);
        let out = r.poll_latest();
        assert_eq!(out.frame.expect("最新帧").frame, 4);
        assert_eq!(out.skipped, 3);
        assert!(!out.overwritten, "落后 == 槽数还不算被覆盖");

        write_frames(rb, 5, 5);
        let out = r.poll_latest();
        assert_eq!(out.frame.expect("最新帧").frame, 9);
        assert!(out.overwritten, "落后 > 槽数必须报环形覆盖");
    }

    // —— 环形缓冲区消费端（logger 解耦）——

    /// 往本地 `DataRingBuffer` 里提交 `n` 帧，帧号 = `start..start+n`。
    fn write_frames(rb: &DataRingBuffer, start: u64, n: u64) {
        for i in 0..n {
            rb.next_slot().frame = start + i;
            rb.commit();
        }
    }

    /// 空缓冲区 → 没有新帧；生产 N 帧后 → 只返回**最后**一帧，其余 N-1 帧记为跳过。
    #[test]
    fn frame_reader_polls_only_latest_frame() {
        let rb: &'static DataRingBuffer = Box::leak(Box::new(DataRingBuffer::new()));
        let mut r = FrameReader::new(rb);

        let out = r.poll_latest();
        assert!(out.frame.is_none(), "还没写过帧，不应返回帧");
        assert_eq!((out.skipped, out.overwritten, out.reset), (0, false, false));

        write_frames(rb, 1, 5);
        let out = r.poll_latest();
        let f = out.frame.expect("有 5 个新帧，必须返回最新一帧");
        assert_eq!(f.frame, 5, "返回的必须是最后一帧（只取最新，不 drain 历史）");
        assert_eq!(out.skipped, 4, "其余 4 帧应计入 skipped");
        assert!(!out.overwritten, "5 帧 < 容量 32，没有发生环形覆盖");

        // 没有新帧 → 不重复返回同一帧（记录频率 = 轮询频率）
        let out = r.poll_latest();
        assert!(out.frame.is_none(), "两次轮询之间没有新帧，不应重复记录");
        assert_eq!((out.skipped, out.overwritten), (0, false));

        // 只取最新帧：不 drain、不推进生产端游标
        assert_eq!(rb.latest().frame, 5, "消费端不得改动生产端可见的最新帧");
        assert_eq!(rb.writes(), 5, "消费端不得改动写计数");
        write_frames(rb, 6, 3);
        let out = r.poll_latest();
        assert_eq!(out.frame.expect("新帧").frame, 8);
        assert_eq!(out.skipped, 2, "增量按上次轮询之后的写入计数");
    }

    /// 一轮之间写入超过容量 → 报环形覆盖（有帧在被读走前就被覆盖）。
    #[test]
    fn frame_reader_flags_ring_overwrite() {
        let rb: &'static DataRingBuffer = Box::leak(Box::new(DataRingBuffer::new()));
        let mut r = FrameReader::new(rb);

        write_frames(rb, 1, DataRingBuffer::SLOTS as u64 + 8);
        let out = r.poll_latest();
        let f = out.frame.expect("必须返回最新一帧");
        assert_eq!(f.frame, (DataRingBuffer::SLOTS + 8) as u64);
        assert_eq!(out.skipped, DataRingBuffer::SLOTS as u64 + 7);
        assert!(out.overwritten, "落后 > 容量（{} 帧）必须报环形覆盖", DataRingBuffer::SLOTS);
    }

    /// `reset_ring_buffer()`（每次重连都会调）后：计数不溢出、不当成新帧，之后继续正常工作。
    #[test]
    fn frame_reader_survives_ring_reset() {
        let rb: &'static DataRingBuffer = Box::leak(Box::new(DataRingBuffer::new()));
        let mut r = FrameReader::new(rb);

        write_frames(rb, 1, 5);
        assert_eq!(r.poll_latest().frame.expect("首轮").frame, 5);

        rb.reset(); // == reset_ring_buffer()：generation 前进（消费端据此识别断流）
        let out = r.poll_latest();
        assert!(out.reset, "必须识别出这是一次生产端重置");
        assert!(out.frame.is_none(), "重置本身不是一帧数据");
        assert_eq!((out.skipped, out.overwritten), (0, false), "重置不得被算成丢帧/覆盖");

        // 重置前已写过 5 帧：若消费端不识别 generation，这里的差值会算成"倒流"而溢出
        write_frames(rb, 100, 3);
        let out = r.poll_latest();
        assert_eq!(out.frame.expect("重置后的第 3 帧").frame, 102);
        assert_eq!(out.skipped, 2, "重置后只按新的写入增量计数");
        assert!(!out.reset, "没有新的重置");
    }

    /// 反复重置也不累加丢帧
    #[test]
    fn frame_reader_repeated_resets_do_not_accumulate() {
        let rb: &'static DataRingBuffer = Box::leak(Box::new(DataRingBuffer::new()));
        let mut r = FrameReader::new(rb);
        write_frames(rb, 1, 5);
        assert_eq!(r.poll_latest().frame.expect("首轮").frame, 5);
        for _ in 0..3 {
            rb.reset();
            let out = r.poll_latest();
            assert!(out.reset);
            assert_eq!((out.skipped, out.overwritten), (0, false));
        }
    }

    /// 新建的消费端从"当前"开始：创建之前写入的帧不补记，也不产生 reset 噪声
    /// （真实时序：main 先 `reset_ring_buffer()`、再 `frame_reader()` 建 logger 线程）。
    #[test]
    fn frame_reader_created_after_writes_starts_in_sync() {
        let rb: &'static DataRingBuffer = Box::leak(Box::new(DataRingBuffer::new()));
        write_frames(rb, 1, 5);
        rb.reset(); // 重连：generation 前进
        write_frames(rb, 6, 2);

        let mut r = FrameReader::new(rb);
        let out = r.poll_latest();
        assert!(
            out.frame.is_none() && !out.reset && out.skipped == 0,
            "创建时就在同步点：历史帧不补记，创建前的重置也不算本次连接的 reset"
        );

        write_frames(rb, 8, 4);
        let out = r.poll_latest();
        assert_eq!(out.frame.expect("创建后写入的新帧").frame, 11);
        assert_eq!(out.skipped, 3, "只统计创建之后的写入");
        assert!(!out.reset);
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
