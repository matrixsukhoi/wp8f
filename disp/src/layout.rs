//! 布局配置的**全局快照**与生灭：启动时定一次（`init_hud_layout`），运行期只读
//! （`hud_layout()`），拖拽/控制台改动经 `save_*` 落盘。
//!
//! 从 `lib.rs` 拆出的一块；对外路径不变（`lib.rs` 有 `pub use layout::{...}`）。
//! 配置本身的类型与缺省值在 `config.rs`；这里只负责"读进内存 / 写出文件 / 给渲染线程一个
//! `&'static` 快照"。

use std::sync::OnceLock;

use super::{font, geometry, mini_hud, HudLayoutConfig, HudType};

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
