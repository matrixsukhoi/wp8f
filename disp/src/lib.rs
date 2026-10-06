pub mod circle_hud;
pub mod components;
pub mod draw;
pub mod font;
pub(crate) mod geometry;
pub mod hud;
mod hud_common;
pub use hud_common::map_zone_label;
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

#[derive(Debug, Clone)]
pub struct FixedBytes<const N: usize> {
    buf: [u8; N],
    len: u8,
}

impl<const N: usize> FixedBytes<N> {
    pub const fn new() -> Self {
        Self { buf: [0u8; N], len: 0 }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len as usize]
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(self.as_bytes()).unwrap_or("")
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn assign(&mut self, src: &[u8]) {
        let n = src.len().min(N - 1);
        self.buf[..n].copy_from_slice(&src[..n]);
        self.len = n as u8;
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const N: usize> Default for FixedBytes<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> std::fmt::Write for FixedBytes<N> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let bytes = s.as_bytes();
        let remaining = (N - 1).saturating_sub(self.len as usize);
        let n = bytes.len().min(remaining);
        self.buf[self.len as usize..self.len as usize + n].copy_from_slice(&bytes[..n]);
        self.len += n as u8;
        Ok(())
    }
}

impl<const N: usize> Serialize for FixedBytes<N> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.as_bytes())
    }
}

impl<'de, const N: usize> Deserialize<'de> for FixedBytes<N> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FbVisitor<const M: usize>;
        impl<'de, const M: usize> serde::de::Visitor<'de> for FbVisitor<M> {
            type Value = FixedBytes<M>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "a byte array up to {} bytes", M)
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<FixedBytes<M>, E> {
                let mut fb = FixedBytes::new();
                fb.assign(v);
                Ok(fb)
            }
        }
        deserializer.deserialize_bytes(FbVisitor)
    }
}

pub const DEFAULT_FONT_SIZE: f32 = 16.0;

/// 颜色配置的字符串格式：**`#RRGGBBAA`**（前 6 位 RGB、末 2 位 alpha；只写 6 位视为不透明）。
/// 宽容规则与前端 `hud-colors.js::parseHudColor` 一致：可省略 `#`、大小写不限，其它长度报
/// serde 错误（不 panic、不静默取默认色）。
///
/// 内部一律打包成 `u32 = 0xAARRGGBB`（实现细节）：`#3EC8659C` ↔ `0x9C3EC865`，
/// 换算只发生在这个模块里。
mod hex_color {
    use serde::{de, Deserializer, Serializer};

    /// 序列化统一输出 `#RRGGBBAA`（8 位、大写、带 `#`）：配置器"存完再打开"必须是同一格式。
    pub fn serialize<S>(color: &u32, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let a = (color >> 24) & 0xFF;
        let r = (color >> 16) & 0xFF;
        let g = (color >> 8) & 0xFF;
        let b = color & 0xFF;
        serializer.serialize_str(&format!("#{:02X}{:02X}{:02X}{:02X}", r, g, b, a))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u32, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct HexOrInt;
        impl<'de> de::Visitor<'de> for HexOrInt {
            type Value = u32;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("hex color string like \"#RRGGBBAA\" or integer")
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<u32, E> {
                Ok(v as u32)
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<u32, E> {
                // 与前端同款宽容：可省略 '#'
                let digits = s.trim().trim_start_matches('#');
                let value = u32::from_str_radix(digits, 16).map_err(de::Error::custom)?;
                let (r, g, b, a) = match digits.len() {
                    // 8 位 `#RRGGBBAA`：前 6 位 RGB、末 2 位 alpha
                    8 => (
                        (value >> 24) & 0xFF,
                        (value >> 16) & 0xFF,
                        (value >> 8) & 0xFF,
                        value & 0xFF,
                    ),
                    // 6 位 `#RRGGBB`：不写 alpha 按不透明（不能当 0，否则颜色整块消失）
                    6 => ((value >> 16) & 0xFF, (value >> 8) & 0xFF, value & 0xFF, 0xFF),
                    n => {
                        return Err(de::Error::custom(format!(
                            "invalid hex color \"{}\": expected 6 or 8 hex digits (#RRGGBB / #RRGGBBAA), got {}",
                            s, n
                        )))
                    }
                };
                Ok((a << 24) | (r << 16) | (g << 8) | b)
            }
        }
        deserializer.deserialize_any(HexOrInt)
    }
}

fn default_map_font() -> f32 { 16.0 }
fn default_true() -> bool { true }
fn default_map_icon() -> f32 { 18.0 }
fn default_map_obj_font() -> f32 { 12.0 }
fn default_circle_font() -> f32 { 24.0 }
fn default_circle_radius() -> f32 { 300.0 }
fn default_mini_font() -> f32 { 30.0 }
fn default_grid_spacing() -> f32 { 7.0 }
fn default_grid_padding() -> f32 { 10.0 }
// 颜色默认值：内部是 `0xAARRGGBB` 打包（配置文本走 `#RRGGBBAA`，换算只在 hex_color 模块）。
fn default_map_bg_color() -> u32 { 0x40000000 }
// 这 5 个是**文字色**，值按"预乘 + alpha 自乘"的旧观感补偿过（字形现在走直通 alpha 混色，
// 见 `draw/glyph_cache.rs`）—— 改它们等于改默认观感，别照直觉改。
fn default_data_color() -> u32 { 0x9C3E_C865 }
fn default_label_color() -> u32 { 0x2814_5024 }
fn default_unit_color() -> u32 { 0x2840_4040 }
fn default_hint_color() -> u32 { 0x66A2_9826 }
fn default_alert_color() -> u32 { 0x66A2_2619 }
fn default_stroke_color() -> u32 { 0x66002814 }
fn default_warning_blink_hz() -> f32 { 4.0 }
/// core 数据刷新频率的下限 / 上限（Hz）。上限同时决定地图记录间隔的上限（1 秒的数据帧数）
/// 与记录采样率的上限（记录周期最小 = 一个主循环节拍）。
pub const REFRESH_HZ_MIN: u32 = 5;
pub const REFRESH_HZ_MAX: u32 = 60;
fn default_refresh_hz() -> u32 { 30 }
/// **地图记录间隔（数据帧）**的下限：1 = 每个数据帧都记录一次（写 0 会让闸门每帧触发）。
/// 上限 = `refresh_hz`（1 秒的数据帧数），见 [`HudLayoutConfig::map_obj_record_every_frames_clamped`]。
pub const MAP_OBJ_RECORD_EVERY_FRAMES_MIN: u64 = 1;
/// 配置缺键时的**地图记录间隔（数据帧）**：取自 [`MAP_OBJ_INTERVAL_FRAME`]，
/// **不许在这里抄字面量**（抄一份就会出现"常量改了、默认值没改"）。
fn default_map_obj_record_every_frames() -> u64 { MAP_OBJ_INTERVAL_FRAME }
fn default_datalink_server() -> String { "127.0.0.1".to_string() }
fn default_datalink_port() -> u16 { datalink::DEFAULT_PORT }
fn default_datalink_send_hz() -> u32 { 4 }
fn default_datalink_peer_timeout_secs() -> u32 { datalink::PEER_TIMEOUT_SECS }
fn default_datalink_session() -> u8 { datalink::DEFAULT_SESSION_ID }
fn default_datalink_x() -> i32 { 48 }
fn default_datalink_y() -> i32 { 300 }
/// 界面语言：`auto` = 跟随系统；缺省 `zh_simp`。中文两份的代码是 `zh_simp` / `zh_trad`
/// （旧写法 `zh-CN`/`zh-TW` 读取时仍兼容，见 `gui/src/i18n.rs::LEGACY_ALIASES`）。
/// **只影响控制台界面**：HUD 的语言看 [`HudLayoutConfig::hud_lang_path`]。
fn default_language() -> String { "zh_simp".to_string() }

/// **默认语音包**：`voice_path` 的 serde 默认值，与 `core/src/warnings.rs::DEFAULT_VOICE_PATH`
/// 必须一致（GUI 的 `/api/voice-packs` 与拖动保存也用这个值，改一处要改两处）。
fn default_voice_path() -> String {
    "resource/voice/zh_xiaoxuan_calm_youngadultfemale_1_04".to_string()
}

/// 飞行记录配置（布局配置 JSON 的 `record` 段）。
///
/// 记录写成 wp8f 自有的 `.wpr`（地图信息 + 底图 + N 组逐帧 CSV）；归一化地图坐标原样入档，
/// 导出 ACMI/CSV 时由 GUI 的转换器现算（容器与列义见 `logger`）。
///
/// **没有"记录频率"这一项**：记录线程的采样周期 = 地图刷新周期
/// （`map_obj_record_every_frames × 1000 / refresh_hz`，唯一换算入口是
/// `core/src/mapobj.rs::period_ms`）。地图坐标每 `map_obj_record_every_frames` 个数据帧才更新
/// 一次，记录得比它更快只会写下坐标完全相同的重复帧 —— 所以这条口径不开放配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordConfig {
    /// 启用飞行记录
    #[serde(default)]
    pub enabled: bool,
    /// 输出目录（空 = ./logs）
    #[serde(default)]
    pub output_dir: String,
    /// 每组 CSV 内存池的**初始**容量（MiB，默认 8）：记录期间只往内存里追加行、写满自动扩容。
    /// 按默认采样周期（8 数据帧 @30 Hz = 266 ms）估算：1 MiB ≈ 1500 行 ≈ 6.5 分钟。
    #[serde(default = "default_record_pool_mb")]
    pub pool_mb: u32,
}

fn default_record_pool_mb() -> u32 { 8 }

impl Default for RecordConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            output_dir: String::new(),
            pool_mb: default_record_pool_mb(),
        }
    }
}

/// 环形缓冲区配置（布局配置 JSON 的 `ring` 段）。
///
/// 槽数 = 消费者能落后多少个版本还不被写端追上。**帧环**按数据帧算
/// （`frames / refresh_hz` 秒），**地图环**按地图采样算（`map_samples × map_obj_record_every_frames / refresh_hz` 秒）。
/// 卡顿/断点调试时可以调大；调大只多占内存（槽在启动时一次分配）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RingConfig {
    /// `DisplayData` 帧环槽数（默认 32 ≈ 30 Hz 下 1 秒）
    #[serde(default = "default_ring_frames")]
    pub frames: u32,
    /// `MapDisplay` 地图环槽数（默认 32 ≈ 4 Hz 采样下 8 秒）
    #[serde(default = "default_ring_map_samples")]
    pub map_samples: u32,
}

/// 槽数上限：一帧 `DisplayData` 是几百字节，256 槽 ≈ 8 秒 @30 Hz，再多只是白占内存。
const RING_SLOTS_MAX: u32 = 256;

fn default_ring_frames() -> u32 { wp8f_ring::Ring::<()>::SLOTS as u32 }
fn default_ring_map_samples() -> u32 { wp8f_ring::Ring::<()>::SLOTS as u32 }

impl Default for RingConfig {
    fn default() -> Self {
        Self { frames: default_ring_frames(), map_samples: default_ring_map_samples() }
    }
}

impl RingConfig {
    /// 钳到 `2..=256`（1 个槽时"下一个待写槽"就是正在读的那个）。
    pub fn frames_clamped(&self) -> usize {
        (self.frames.clamp(wp8f_ring::Ring::<()>::MIN_SLOTS as u32, RING_SLOTS_MAX)) as usize
    }

    pub fn map_samples_clamped(&self) -> usize {
        (self.map_samples.clamp(wp8f_ring::Ring::<()>::MIN_SLOTS as u32, RING_SLOTS_MAX)) as usize
    }
}

/// Link-8 数据链客户端配置（并入布局配置 JSON 的 `datalink` 段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataLinkConfig {
    /// 是否启用 Link-8 上报/接收
    #[serde(default)]
    pub enabled: bool,
    /// 服务端 IP/主机名
    #[serde(default = "default_datalink_server")]
    pub server: String,
    /// 服务端端口
    #[serde(default = "default_datalink_port")]
    pub port: u16,
    /// 加密密钥（须与服务端一致）
    #[serde(default)]
    pub key: String,
    /// 对局编号（同对局才互相共享）
    #[serde(default = "default_datalink_session")]
    pub session_id: u8,
    /// 上报频率（Hz，整数）
    #[serde(default = "default_datalink_send_hz")]
    pub send_hz: u32,
    /// 友军快照超时（秒）：这么久没收到服务端应答就清空友军（地图标记 + 数据标签面板）；
    /// `0` = 一直保留最后一份快照。与服务端自己的追踪超时同口径。
    #[serde(default = "default_datalink_peer_timeout_secs")]
    pub peer_timeout_secs: u32,
}

impl Default for DataLinkConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server: default_datalink_server(),
            port: default_datalink_port(),
            key: String::new(),
            session_id: default_datalink_session(),
            send_hz: default_datalink_send_hz(),
            peer_timeout_secs: default_datalink_peer_timeout_secs(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HudLayoutConfig {
    #[serde(default = "default_hud_type")]
    pub hud_type: HudType,
    #[serde(default)]
    pub circle_x: i32,
    #[serde(default)]
    pub circle_y: i32,
    pub font_size: f32,
    pub flight_info_x: i32,
    pub flight_info_y: i32,
    pub flight_info_col: u32,
    pub engine_info_x: i32,
    pub engine_info_y: i32,
    pub engine_info_col: u32,
    pub minihud_x: i32,
    pub minihud_y: i32,
    pub map_x: i32,
    pub map_y: i32,
    pub map_size_x: i32,
    pub map_size_y: i32,
    // —— 面板开关（false = 隐藏该面板）——
    //    注意：**HUD 本体（圆环 / miniHUD）不在这里**，它只由 `hud_type` 决定
    //    （0 关 / 1 mini / 2 圆环）—— 两套开关并存过，结果就是"改了类型没改开关"
    //    画出空白屏（老键 `minihud_enabled`/`circle_enabled` 现在被忽略）。
    #[serde(default = "default_true")]
    pub flight_info_enabled: bool,
    #[serde(default = "default_true")]
    pub engine_info_enabled: bool,
    #[serde(default = "default_true")]
    pub map_enabled: bool,
    #[serde(default = "default_true")]
    pub datalink_panel_enabled: bool,
    /// 语音/提示音开关（默认开）。关掉后不出任何声音，但视觉告警（告警 X）照旧。
    #[serde(default = "default_true")]
    pub voice_warnings_enabled: bool,
    /// F-18 风格**机动告警声**开关（默认开）：与语音告警是两个独立音层，
    /// 关掉只停蜂鸣/长鸣，`voice_warnings_enabled` 与视觉告警都不受影响。
    #[serde(default = "default_true")]
    pub maneuver_tone_enabled: bool,
    /// **core 数据刷新频率（Hz）**：主循环读 8111 + 更新 `DisplayData` 的节拍，也决定面板重绘
    /// 频率。缺省 30，钳到 [`REFRESH_HZ_MIN`]..=[`REFRESH_HZ_MAX`]。
    ///
    /// ⚠️ 帧计数类常量（地图记录间隔、语音告警检查 8 帧、告警保持 30 帧）按**帧**算，
    /// 改本值它们的**时间口径一起变**；记录线程的采样周期（= 地图刷新周期）也跟着变。
    #[serde(default = "default_refresh_hz")]
    pub refresh_hz: u32,
    /// **地图记录间隔（数据帧）**：本机地图位置、友军快照、地图对象列表每多少数据帧记一次
    /// （core 闸门 `frame_count >= next_mapobj_frame`，初值 = 本值 → 首次记录在第 N 个数据帧）。
    ///
    /// * 单位是数据帧：毫秒 = `帧数 × 1000 / refresh_hz`（换算只在 core 的 `mapobj`）；
    /// * 取值 `1..=refresh_hz`（缺省 [`MAP_OBJ_INTERVAL_FRAME`]），夹紧见
    ///   [`Self::map_obj_record_every_frames_clamped`]；
    /// * 记录线程的起点也用本值（首个周期 `pos_x/pos_y` 还是初值 `(0.5, 0.5)`，记下来会闪）。
    #[serde(default = "default_map_obj_record_every_frames")]
    pub map_obj_record_every_frames: u64,
    #[serde(default = "default_map_font")]
    pub map_font_size: f32,
    #[serde(default = "default_map_icon")]
    pub map_icon_size: f32,
    #[serde(default = "default_map_obj_font")]
    pub map_obj_font_size: f32,
    #[serde(default = "default_circle_font")]
    pub circle_font_size: f32,
    #[serde(default = "default_circle_radius")]
    pub circle_ring_radius: f32,
    #[serde(default = "default_mini_font")]
    pub mini_font_size: f32,
    #[serde(default = "default_grid_spacing")]
    pub grid_spacing: f32,
    #[serde(default = "default_grid_padding")]
    pub grid_padding: f32,
    // —— 颜色（7 个键）：配置里一律 **`#RRGGBBAA`**，换算在 `hex_color` 模块里成内部 `0xAARRGGBB`。
    //    值**直通落屏、不预乘**（半透明合成交给窗口）；其中 data/label/unit/hint/alert 是
    //    **文字色**，值是补偿值 —— 改动前先看 `draw/glyph_cache.rs`。
    #[serde(default = "default_map_bg_color", with = "hex_color")]
    pub map_bg_color: u32,
    #[serde(default = "default_data_color", with = "hex_color")]
    pub data_color: u32,
    #[serde(default = "default_label_color", with = "hex_color")]
    pub label_color: u32,
    #[serde(default = "default_unit_color", with = "hex_color")]
    pub unit_color: u32,
    #[serde(default = "default_hint_color", with = "hex_color")]
    pub hint_color: u32,
    #[serde(default = "default_alert_color", with = "hex_color")]
    pub alert_color: u32,
    #[serde(default = "default_stroke_color", with = "hex_color")]
    pub stroke_color: u32,
    /// Warning X blink frequency in Hz (on/off cycles per second, 50% duty
    /// cycle; `<= 0` disables blinking and keeps the X visible).
    #[serde(default = "default_warning_blink_hz")]
    pub warning_blink_hz: f32,
    /// Link-8 数据链客户端配置
    #[serde(default, alias = "link8")]
    pub datalink: DataLinkConfig,
    /// 飞行记录配置（`.wpr`）
    #[serde(default)]
    pub record: RecordConfig,
    /// 环形缓冲区槽数（帧环 / 地图环）
    #[serde(default)]
    pub ring: RingConfig,
    /// 友军数据标签 Panel 位置（旧配置键 `link8_x`/`link8_y` 仍兼容）
    #[serde(default = "default_datalink_x", alias = "link8_x")]
    pub datalink_x: i32,
    #[serde(default = "default_datalink_y", alias = "link8_y")]
    pub datalink_y: i32,
    /// **界面语言**：`resource/i18n/<code>.json` 的 code（`zh_simp`/`zh_trad`/`en`/…，旧写法也认），
    /// `auto` = 跟随系统。消费方是 GUI/后端；**HUD 不看这个键**（看 `hud_lang_path`）。
    /// HUD 会把它原样读入、原样存回，别在保存时弄丢。
    #[serde(default = "default_language")]
    pub language: String,
    /// **HUD 标签语言文件**：相对安装根的路径，例如 `resource/lang/zh.json`（绝对路径也接受）。
    /// 自建 `resource/lang/xxx.json`（键集与随包两份一致）指过来就能换一套标签。
    ///
    /// * 空 = 随包的中文表 `resource/lang/zh.json`（不看 [`Self::language`]）；
    /// * 由 **core 初始化时读**（`core/src/main.rs`）：只留 `hud.*` 标签，JSON 与中间对象立刻释放；
    /// * 文件不存在 / 解析失败 → HUD 报错退出（`exit 2`），不静默回退。
    #[serde(default)]
    pub hud_lang_path: String,
    /// **语音包目录**：相对安装根的目录，例如 `resource/voice/<包名>`（绝对路径也接受）。
    /// 目录里放 `*.wav`（文件主干 = 代码里点播的名字，如 `warn_gear`）；**放一个新子目录就是一个新包**。
    ///
    /// * 启动时整目录读进内存（`core/src/warnings.rs::preload`），之后播放零磁盘 I/O；
    /// * 单个 wav 缺失/损坏 → 去重警告后跳过；目录找不到 → 警告后静音运行（语音是增强功能）。
    #[serde(default = "default_voice_path")]
    pub voice_path: String,
    /// **HUD 正文字体**：指向真实字体文件的路径，值自带 `resource/fonts/` 这一段（绝对路径也接受）。
    ///
    /// * **必填**：空、文件不存在或解析不了 → HUD 报错退出（`exit 2`），不会挑别的字体顶上；
    /// * 解析基准 = 仓库/安装根（cwd → 上一级 → exe 目录 → exe 目录上一级）；
    /// * 取值来源 = `GET /api/fonts`（GUI 的「字体」卡片）；细节见 `disp/src/font.rs`。
    #[serde(default = "default_font_path")]
    pub font_path: String,
}

/// `font_path` 的缺省值。与"显式写空字符串"是两回事：缺键 = 用随包那份；
/// 写了 `""` = 配置没写完 → HUD 退出（`config/default.json` 里显式写着同一个值）。
fn default_font_path() -> String {
    format!("{}/{}", font::FONT_DIR, font::DEFAULT_TEXT_FILE)
}

impl Default for HudLayoutConfig {
    fn default() -> Self {
        Self {
            hud_type: HudType::Mini,
            font_size: DEFAULT_FONT_SIZE,
            flight_info_x: 0,
            flight_info_y: 1235,
            flight_info_col: 6,
            engine_info_x: 1758,
            engine_info_y: 1190,
            engine_info_col: 4,
            minihud_x: 1180,
            minihud_y: 908,
            map_x: 50,
            map_y: 0,
            map_size_x: 0,
            map_size_y: 0,
            circle_x: 0,
            circle_y: 0,
            map_font_size: 16.0,
            map_icon_size: 18.0,
            map_obj_font_size: 12.0,
            circle_font_size: 24.0,
            circle_ring_radius: 300.0,
            mini_font_size: 30.0,
            grid_spacing: 7.0,
            grid_padding: 10.0,
            // 颜色一律走 default_*_color()：**单一来源**。这里原来自己抄了一份字面量，
            // 迁移文字色时"改了函数、漏了这份副本"就得到两套默认色（缺键配置与无配置文件不一致）。
            map_bg_color: default_map_bg_color(),
            data_color: default_data_color(),
            label_color: default_label_color(),
            unit_color: default_unit_color(),
            hint_color: default_hint_color(),
            alert_color: default_alert_color(),
            stroke_color: default_stroke_color(),
            warning_blink_hz: 4.0,
            datalink: DataLinkConfig::default(),
            flight_info_enabled: true,
            engine_info_enabled: true,
            map_enabled: true,
            datalink_panel_enabled: true,
            voice_warnings_enabled: true,
            maneuver_tone_enabled: true,
            refresh_hz: default_refresh_hz(),
            // 与 serde default 同源：**只调函数，不抄字面量**（缺键配置与无配置文件必须一致）
            map_obj_record_every_frames: default_map_obj_record_every_frames(),
            record: RecordConfig::default(),
            ring: RingConfig::default(),
            datalink_x: default_datalink_x(),
            datalink_y: default_datalink_y(),
            // 与 serde default 同源（缺键配置 == 无配置文件）
            language: default_language(),
            // 空 = 按 `language` 映射（老配置口径）；**默认值必须留空**：
            // 写成 `resource/lang/zh.json` 会让"老配置的 language=en"失效（serde 补默认值
            // 发生在 pick 之前），而 `config/default.json` 里是显式写路径的。
            hud_lang_path: String::new(),
            voice_path: default_voice_path(),
            font_path: default_font_path(),
        }
    }
}

/// HUD 显示模式（配置键 `hud_type`）：
///
/// | 值 | 含义 |
/// |---|---|
/// | `0` | 关闭 HUD（只画飞行/发动机/地图面板，不画圆环也不画 miniHUD） |
/// | `1` | MiniHUD（紧凑面板） |
/// | `2` | CircleHUD（圆环） |
///
/// 配置里**写数字**（缺键时按 [`default_hud_type`]）；为兼容旧配置也接受
/// `"off"` / `"mini"` / `"circle"` 三种字符串写法，保存回文件时统一写成数字。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudType {
    Off,
    Mini,
    Circle,
}

impl HudType {
    pub const fn as_u8(self) -> u8 {
        match self {
            HudType::Off => 0,
            HudType::Mini => 1,
            HudType::Circle => 2,
        }
    }

    /// 数字 → 模式：`0/1` 之外的一切都当圆环（越界值不该让 HUD 整屏空白）。
    pub const fn from_u8(v: u8) -> Self {
        match v {
            0 => HudType::Off,
            1 => HudType::Mini,
            _ => HudType::Circle,
        }
    }
}

impl std::str::FromStr for HudType {
    type Err = String;

    /// 字符串口径：数字 `0|1|2` 与旧配置里的 `off|mini|circle`（大小写不敏感）。
    /// 只服务于 `Deserialize` —— **没有对应的命令行参数**（HUD 类型只从配置读）。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "0" | "off" | "none" => Ok(HudType::Off),
            "1" | "mini" | "minihud" => Ok(HudType::Mini),
            "2" | "circle" | "circlehud" => Ok(HudType::Circle),
            other => Err(format!("HUD 类型 '{other}' 不认识：用 0=关闭 / 1=MiniHUD / 2=CircleHUD")),
        }
    }
}

impl Serialize for HudType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(self.as_u8())
    }
}

impl<'de> Deserialize<'de> for HudType {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct NumOrName;
        impl serde::de::Visitor<'_> for NumOrName {
            type Value = HudType;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("HUD 类型：0/1/2（或 \"off\"/\"mini\"/\"circle\"）")
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<HudType, E> {
                Ok(HudType::from_u8(v.min(u8::MAX as u64) as u8))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<HudType, E> {
                Ok(HudType::from_u8(v.clamp(0, u8::MAX as i64) as u8))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<HudType, E> {
                v.parse().map_err(E::custom)
            }
        }
        d.deserialize_any(NumOrName)
    }
}

fn default_hud_type() -> HudType {
    HudType::Mini
}

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

impl HudLayoutConfig {
    /// 钳制后的 core 数据刷新频率（Hz）。
    pub fn refresh_hz_clamped(&self) -> u32 {
        self.refresh_hz.clamp(REFRESH_HZ_MIN, REFRESH_HZ_MAX)
    }

    /// 主循环节拍（纳秒）—— core 侧"读 8111 / 更新 DisplayData"周期的**唯一来源**
    /// （`refresh_hz` 越界时按 [`REFRESH_HZ_MIN`]/[`REFRESH_HZ_MAX`] 钳制）。
    pub fn refresh_interval_ns(&self) -> u64 {
        1_000_000_000 / self.refresh_hz_clamped() as u64
    }

    /// 钳制后的**地图记录间隔（数据帧）**：`1..=refresh_hz_clamped()`（0 会让闸门每帧触发，
    /// 超过 1 秒的间隔已经不是"记录"而是"快照"）。调用方一律用这个值，不要直接读字段。
    pub fn map_obj_record_every_frames_clamped(&self) -> u64 {
        let max = self.refresh_hz_clamped() as u64;
        self.map_obj_record_every_frames
            .clamp(MAP_OBJ_RECORD_EVERY_FRAMES_MIN, max)
    }
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DisplayData {
    pub magic: u32,
    pub version: u32,
    pub timestamp: u64,
    pub frame: u64,
    pub valid: u8,
    pub altitude: f64,
    pub tas: f64,
    pub ias: f64,
    pub mach: f64,
    pub aoa: f64,
    pub aos: f64,
    pub ny: f64,
    pub vy: f64,
    pub heading: f64,
    pub bearing: f64,
    pub roll: f64,
    pub pitch: f64,
    pub has_attitude: bool,
    pub wx: f64,
    pub aileron: f64,
    pub elevator: f64,
    pub rudder: f64,
    pub flaps: f64,
    pub gear: f64,
    pub airbrake: f64,
    pub radio_altitude: f64,
    pub trimmer: f64,
    pub fuel_kg: f64,
    pub fuel_percent: f64,
    pub fuel_time: f64,
    pub fuel1_kg: f64,
    pub fuel1_time: f64,
    pub has_fuel1: bool,
    pub engine_count: usize,
    pub engine_throttle: [f64; 8],
    pub engine_power: [f64; 8],
    pub engine_rpm: [f64; 8],
    pub engine_thrust: [f64; 8],
    pub engine_temp_water: [f64; 8],
    pub engine_temp_oil: [f64; 8],
    pub is_jet: bool,
    pub is_fm_valid: bool,
    pub engine_efficiency: f64,
    pub thrust_to_weight: f64,
    pub power_to_weight: f64,
    pub total_thrust: f64,
    pub total_hp: f64,
    pub thrust_percent: f64,
    pub sep: f64,
    pub energy_height: f64,
    pub energy_height_text: FixedBytes<16>,
    pub turn_rate: f64,
    pub turn_radius: f64,
    /// 起落架边沿判定结果：false = 下降沿（正在收回）→ 起落架超速告警抑制
    pub gear_armed: bool,
    /// Two-stage overspeed warning level for IAS vs VNE:
    /// 0 = normal, 1 = >= 95% VNE (yellow), 2 = >= 97.5% VNE (red).
    pub overspeed_warning: u8,
    /// Two-stage overspeed warning level for Mach vs MNE (same scale as
    /// `overspeed_warning`).
    pub mach_warning: u8,
    /// Danger-class voice alarm active (overspeed family / critical AoA /
    /// G-load): miniHUD / circle HUD flash a warning X while true. System
    /// cautions (overheat, RPM, fuel, ...) do not set this flag.
    pub voice_alarm: bool,
    /// Speed-brake caution: brake deployed -> yellow IAS warning; cleared
    /// immediately on the retract edge.
    pub brake_caution: bool,
    pub vne: f64,
    pub vne_mach: f64,
    pub wing_sweep: f64,
    pub max_aoa: f64,
    /// 负向临界攻角（度，负值）：alpha_crit_low，襟翼/后掠插值口径同 max_aoa
    pub min_aoa: f64,
    /// 机动告警参数 m = MAX(攻角比, 过载比, 超速项)（含负向）：≥70% 蜂鸣起音，≥100% 长鸣。
    /// **表速 ≤ 64 km/h 时 core 把它压到 0**（地面滑跑不出声，见 `core::maneuver_tone::gate_by_ias`）。
    pub maneuver_margin: f64,
    pub aircraft_type: FixedBytes<64>,
    pub altitude_text: FixedBytes<16>,
    pub tas_text: FixedBytes<16>,
    pub ias_text: FixedBytes<16>,
    pub mach_text: FixedBytes<16>,
    pub vy_text: FixedBytes<16>,
    pub sep_text: FixedBytes<16>,
    pub turn_rate_text: FixedBytes<16>,
    pub turn_radius_text: FixedBytes<16>,
    pub fuel_kg_text: FixedBytes<16>,
    pub fuel_percent_text: FixedBytes<16>,
    pub fuel_time_text: FixedBytes<16>,
    pub fuel1_kg_text: FixedBytes<16>,
    pub fuel1_time_text: FixedBytes<16>,
    pub fuel_sfc_text: FixedBytes<16>,
    pub fuel1_sfc_text: FixedBytes<16>,
    pub elapsed_time_text: FixedBytes<16>,
    pub engine_rpm_text: FixedBytes<16>,
    pub engine_rpm_unit: FixedBytes<16>,
    pub engine_temp_water_text: FixedBytes<16>,
    pub engine_temp_oil_text: FixedBytes<16>,
    pub manifold_pressure_text: FixedBytes<16>,
    pub manifold_pressure_unit: FixedBytes<16>,
    pub water_crit_temp_text: FixedBytes<16>,
    pub oil_crit_temp_text: FixedBytes<16>,
    pub vne_text: FixedBytes<16>,
    pub mne_text: FixedBytes<16>,
    pub thrust_to_weight_text: FixedBytes<16>,
    pub total_thrust_text: FixedBytes<16>,
    pub total_drag_text: FixedBytes<16>,
    pub rated_power_text: FixedBytes<16>,
    pub engine_power_text: FixedBytes<16>,
    pub power_unit_text: FixedBytes<16>,
    pub power_to_weight_text: FixedBytes<16>,
    pub engine_throttle_text: FixedBytes<16>,
    pub thrust_percent_text: FixedBytes<16>,
    pub roll_rate_text: FixedBytes<16>,
    pub pitch_text: FixedBytes<16>,
    pub aoa_format_text: FixedBytes<16>,
    pub g_load_text: FixedBytes<16>,
    /// 当前重量下的允许正过载，显示在过载格单位栏（如 "/15.4"，风格同 VNE）
    pub g_limit_text: FixedBytes<16>,
    /// 过载告警级别：0 = 正常，1 = |Ny| 达到允许过载的 90%（红色告警）。
    /// 判据单一来源 core::display::updater::g_load_warning_level，各 panel 统一消费。
    pub g_load_warning: u8,
    pub fuel_sfc: f64,
    pub fuel1_sfc: f64,
    pub manifold_pressure: f64,
    pub total_drag: f64,
    pub afterburner_fuel_time_text: FixedBytes<16>,
    // —— logger 线程专用字段（HUD 渲染不读）——
    // 记录端只凭 `DisplayData` 重建帧，所以这里要有 ACMI 需要、其它字段导不出来的两项：
    // 单值油门与本机归一化地图坐标。
    /// 单值油门（ratio，0..1 口径；加力可 >1）。
    #[serde(default)]
    pub throttle: f64,
    /// 本机归一化地图坐标 x（0..1，东向增大；与 `MapDisplay.player_map_x` 同源同值）
    #[serde(default)]
    pub pos_x: f64,
    /// 本机归一化地图坐标 y（0..1，南向增大；与 `MapDisplay.player_map_y` 同源同值）。
    ///
    /// `pos_x/pos_y` 只在**地图对象被记录**的那几帧刷新（每
    /// [`MAP_OBJ_INTERVAL_FRAME`] 个数据帧一次），首次记录之前是初值 `(0.5, 0.5)`。
    #[serde(default)]
    pub pos_y: f64,
}

/// 地图记录间隔的**缺键默认值**：本机地图位置、友军快照、地图对象列表都按这个间隔记录
/// （8 数据帧 ≈ 30 Hz 下 4 Hz）。
///
/// 真正生效的是配置项 [`HudLayoutConfig::map_obj_record_every_frames`]，本常量只提供缺键默认值
/// 与 `logger` 的记录起点阈值 —— 默认值只此一处，别在别处再抄字面量。
pub const MAP_OBJ_INTERVAL_FRAME: u64 = 8;

/// 底图：**一局一份**（不是每采样一份）。解码 RGB / 原始字节 / 像素尺寸**三份必须一起写**：
/// 分开赋值漏掉 `raw` 时 HUD 照旧显示，但记录线程取到的原始字节是空的 → 落盘的 `.wpr`
/// `img_len = 0`（回放没有底图）。
///
/// 它不在每采样的地图环形槽里：环形槽有 32 个、轮着用，底图一旦进环就得每次发布重写一遍，
/// 否则读者有 31/32 的概率读到空图。放在这里 = 地图加载时写一次，换图时覆盖。
#[derive(Debug, Clone, Default)]
pub struct MapImage {
    /// 解码后的 RGB（绘制用）
    pub rgb: Arc<Vec<u8>>,
    /// 游戏原样给的字节（通常 JPEG；记录用 —— 解码再编码会失真且更大）
    pub raw: Arc<Vec<u8>>,
    pub w: u32,
    pub h: u32,
}

static MAP_IMAGE: OnceLock<std::sync::Mutex<MapImage>> = OnceLock::new();

/// **core** 在地图加载时调用一次：写当前底图（三份一起写）。读者各持一份 `Arc`，
/// 换图不会打断它们正在画/正在写的那份。
pub fn set_map_image(rgb: Arc<Vec<u8>>, raw: Arc<Vec<u8>>, w: u32, h: u32) {
    let mut slot = map_image_slot().lock().unwrap_or_else(|e| e.into_inner());
    *slot = MapImage { rgb, raw, w, h };
}

/// 当前底图。每次调用 clone 两个 `Arc`（换图前的旧读者仍安全），不阻塞绘制。
pub fn map_image() -> MapImage {
    map_image_slot().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn map_image_slot() -> &'static std::sync::Mutex<MapImage> {
    MAP_IMAGE.get_or_init(|| std::sync::Mutex::new(MapImage::default()))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MapDisplay {
    pub player_map_x: f64,
    pub player_map_y: f64,
    pub map_maxsize: [f64; 2],
    /// 地图网格参数（推导区域格，如 D3）
    #[serde(default)]
    pub map_min: [f64; 2],
    #[serde(default)]
    pub grid_zero: [f64; 2],
    #[serde(default)]
    pub grid_steps: [f64; 2],
    pub map_objects: Vec<MapObjectDisplay>,
    pub nearest_airfield_text: FixedBytes<64>,
    pub nearest_bombing_point_text: FixedBytes<64>,
    pub nearest_poi_text: FixedBytes<64>,
    pub nearest_poi_dist: f64,
    pub nearest_poi_bearing: f64,
    pub nearest_poi_rel_speed: f64,
    pub nearest_poi_neighbor_dist: f64,
    pub nearest_poi_neighbor_x: f64,
    pub nearest_poi_neighbor_y: f64,
    pub nearest_poi_x: f64,
    pub nearest_poi_y: f64,
    pub nearest_airfield_dist: f64,
    pub nearest_bombing_dist: f64,
    pub nearest_bombing_bearing: f64,
    pub map_zone_text: FixedBytes<16>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MapObjectDisplay {
    pub obj_type: [u8; 32],
    pub color: u32,
    pub icon: [u8; 16],
    pub x: f64,
    pub y: f64,
    pub ex: f64,
    pub ey: f64,
    pub dx: f64,
    pub dy: f64,
    pub distance: f64,
    pub is_player: bool,
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

    /// 记录间隔常量 = 配置缺键时的默认值（8 数据帧 ≈ 30 Hz 下 4 Hz）；
    /// 运行时真正生效的是 `map_obj_record_every_frames`（core 的闸门与记录起点都用它）。
    #[test]
    fn map_obj_interval_frame_is_the_default_source() {
        assert_eq!(MAP_OBJ_INTERVAL_FRAME, 8, "缺键默认值 = 每 8 数据帧记录一次地图对象");
        assert!(MAP_OBJ_INTERVAL_FRAME >= MAP_OBJ_RECORD_EVERY_FRAMES_MIN, "记录间隔至少 1 数据帧，否则闸门会每帧触发");
        assert_eq!(
            default_map_obj_record_every_frames(), MAP_OBJ_INTERVAL_FRAME,
            "serde 默认函数只许引用常量"
        );
    }

    /// 7 个颜色键（顺序 = HudLayoutConfig 里的声明顺序）。
    const COLOR_KEYS: [&str; 7] = [
        "map_bg_color",
        "data_color",
        "label_color",
        "unit_color",
        "hint_color",
        "alert_color",
        "stroke_color",
    ];

    /// 不许再出现的颜色键：配置里写着也只当没写（没有 serde alias）。
    const REMOVED_COLOR_KEYS: [&str; 4] =
        ["map_color", "circle_color", "poi_color", "warning_x_color"];

    fn colors(c: &HudLayoutConfig) -> [u32; 7] {
        [
            c.map_bg_color,
            c.data_color,
            c.label_color,
            c.unit_color,
            c.hint_color,
            c.alert_color,
            c.stroke_color,
        ]
    }

    /// 单色包装：只为直接验证 `hex_color` 的字符串 ↔ 内部打包往返。
    #[derive(Debug, Serialize, Deserialize)]
    struct OneColor {
        #[serde(with = "hex_color")]
        color: u32,
    }

    /// 解析按 **RGBA** 拆：`#RRGGBBAA` 前 6 位是 RGB、末 2 位是 alpha；
    /// 6 位 `#RRGGBB` 视为不透明；省略 `#`、大小写不限都认；
    /// 非法值报 serde 错误（不 panic、也不静默取默认色）。
    #[test]
    fn hex_color_parses_rrggbbaa() {
        // #RRGGBBAA → 内部 0xAARRGGBB：R=3E G=C8 B=65 A=9C
        let c: OneColor = serde_json::from_str(r##"{"color":"#3EC8659C"}"##).unwrap();
        assert_eq!(c.color, 0x9C_3E_C8_65, "R/G/B 应取前 6 位、A 取末 2 位");

        // 宽容：省略 '#'、小写、首尾空白
        let c: OneColor = serde_json::from_str(r##"{"color":" 3ec8659c "}"##).unwrap();
        assert_eq!(c.color, 0x9C_3E_C8_65);

        // 6 位 = 不透明（AA=FF），内部 alpha 在高字节
        let c: OneColor = serde_json::from_str(r##"{"color":"#50FF82"}"##).unwrap();
        assert_eq!(c.color, 0xFF_50_FF_82);

        // 非法值：非十六进制 / 位数不是 6 或 8（前端 parseHudColor 同样只认这两种）
        for bad in ["#GGGGGGGG", "#12345", "#1234567", "#123456789", ""] {
            let json = format!(r##"{{"color":"{}"}}"##, bad);
            assert!(
                serde_json::from_str::<OneColor>(&json).is_err(),
                "非法颜色 \"{bad}\" 应报 serde 反序列化错误"
            );
        }

        // 序列化统一 8 位大写 #RRGGBBAA：6 位输入的补全值也要按 RGBA 写出
        assert_eq!(
            serde_json::to_string(&OneColor { color: 0x9C_3E_C8_65 }).unwrap(),
            r##"{"color":"#3EC8659C"}"##
        );
        assert_eq!(
            serde_json::to_string(&OneColor { color: 0xFF_50_FF_82 }).unwrap(),
            r##"{"color":"#50FF82FF"}"##
        );

        // 整数写法（旧配置格式）：按内部 0xAARRGGBB 打包原样收下
        let json = format!(r##"{{"color":{}}}"##, 0xFF00FF00u32);
        let c: OneColor = serde_json::from_str(&json).unwrap();
        assert_eq!(c.color, 0xFF00FF00);
    }

    /// 7 个颜色键存出来**必须**是 `#RRGGBBAA`（8 位、大写、带 `#`），
    /// 配置器"保存 → 重新打开"才是同一格式、同一数值；删掉的 4 个键一个都不许再出现。
    #[test]
    fn layout_config_colors_serialize_as_rrggbbaa() {
        let cfg = HudLayoutConfig::default();
        let json = serde_json::to_string_pretty(&cfg).unwrap();

        for key in COLOR_KEYS {
            let needle = format!("\"{}\": \"#", key);
            let at = json
                .find(&needle)
                .unwrap_or_else(|| panic!("序列化结果里缺少颜色键 {key}（或没写成带 # 的字符串）"))
                + needle.len();
            let hex = &json[at..at + 8];
            assert!(
                hex.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b)),
                "{key} 必须是 8 位大写 #RRGGBBAA，实际 \"#{hex}\""
            );
            assert!(json[at + 8..].starts_with('"'), "{key} 的 8 位色值后应紧跟引号");
        }
        for key in REMOVED_COLOR_KEYS {
            assert!(!json.contains(key), "已删除的颜色键 {key} 不该再写进配置：\n{json}");
        }

        // 内部 0xAARRGGBB → 文本 RGBA：默认 data_color 0x9C3EC865（A=9C R=3E G=C8 B=65）
        assert!(
            json.contains("\"data_color\": \"#3EC8659C\""),
            "data_color 应写成 #3EC8659C（与 config/*.json 同源），实际序列化结果里没有"
        );
        // 其余 4 个文字色默认值也与 config/*.json 里的写法同源
        for (key, want) in [
            ("label_color", "#14502428"),
            ("unit_color", "#40404028"),
            ("hint_color", "#A2982666"),
            ("alert_color", "#A2261966"),
        ] {
            assert!(
                json.contains(&format!("\"{key}\": \"{want}\"")),
                "{key} 应写成 {want}（迁移补偿色），实际序列化结果里没有"
            );
        }
        // 往返幂等：存出来的配置再读回来，8 个颜色逐位不变
        let back: HudLayoutConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(
            colors(&back),
            colors(&HudLayoutConfig::default()),
            "颜色保存后再打开必须逐位不变"
        );

        // 旧键按"没写"处理（不留 alias）：配置里还写着它们也读不到 → 落到默认值
        let legacy: HudLayoutConfig = serde_json::from_str(
            r##"{"map_color":"#FF0000FF","circle_color":"#00FF00FF","poi_color":"#0000FFFF","warning_x_color":"#00FF00FF"}"##,
        )
        .unwrap();
        assert_eq!(
            colors(&legacy),
            colors(&HudLayoutConfig::default()),
            "已删除的 map_color/circle_color/poi_color/warning_x_color 必须被忽略（没有 alias）"
        );
    }

    /// 内置默认色是**文字色的补偿值**：配置里没写颜色键时也要保持迁移前的观感，
    /// 所以默认值与 `config/*.json` 里的写法同源（缘由见 `draw/glyph_cache.rs`）。
    ///
    /// 断言方式：默认色过一遍**真正的字形管线**落屏，必须等于**迁移前管线**用旧色的落屏；
    /// 末尾锁住"serde 缺键默认值"与"无配置文件时的 `Default::default()`"两份默认值一致
    /// —— 这两份各写一遍时最容易漏改。
    #[test]
    fn default_colors_are_migrated_compensated_colors() {
        /// 迁移前的字形混合（与 glyph_cache tests 里的内联旧管线同一式子）：
        ///   fa = cov×A/255；ch = (src_ch×fa + bg_ch×(255-fa))/255；a 同式
        /// → 满覆盖时写出 `((RGB×A)/255, (A×A)/255)`。
        fn old_pipeline(color: u32, cov: u32, bg: u32) -> u32 {
            let src_a = (color >> 24) & 0xFF;
            let fa = cov * src_a / 255;
            if fa == 0 {
                return bg;
            }
            let ba = 255 - fa;
            let ch = |src: u32, shift: u32| (src * fa + ((bg >> shift) & 0xFF) * ba) / 255;
            let a_ch = (src_a * fa + ((bg >> 24) & 0xFF) * ba) / 255;
            (ch(color & 0xFF, 0) & 0xFF)
                | ((ch((color >> 8) & 0xFF, 8) & 0xFF) << 8)
                | ((ch((color >> 16) & 0xFF, 16) & 0xFF) << 16)
                | ((a_ch & 0xFF) << 24)
        }

        // (迁移前的旧默认色, 现在的默认色 = 补偿值)：与 config/*.json 的 5 个文字色同源
        let migrated: [(u32, u32); 5] = [
            (0xC850_FF82, 0x9C_3E_C8_65), // data_color  #3EC8659C
            (0x6632_C85A, 0x28_14_50_24), // label_color #14502428
            (0x66A0_A0A0, 0x28_40_40_40), // unit_color  #40404028
            (0xA2FF_F03C, 0x66_A2_98_26), // hint_color  #A2982666
            (0xA2FF_3C28, 0x66_A2_26_19), // alert_color #A2261966
        ];

        let d = HudLayoutConfig::default();
        let defaults = [d.data_color, d.label_color, d.unit_color, d.hint_color, d.alert_color];
        let glyph = crate::draw::glyph_cache::CachedGlyph {
            width: 1,
            height: 1,
            coverage: vec![255],
            bounds_min_y: 0.0,
        };

        for (i, (old_argb, compensated)) in migrated.iter().enumerate() {
            assert_eq!(
                defaults[i], *compensated,
                "默认文字色必须是迁移补偿值（旧口径 {old_argb:#010X}）"
            );
            let mut buf = vec![0u32; 1];
            crate::draw::glyph_cache::blit_cached_glyph(
                &mut buf,
                1,
                1,
                &glyph,
                0.0,
                0.0,
                defaults[i],
            );
            assert_eq!(
                buf[0],
                old_pipeline(*old_argb, 255, 0x0000_0000),
                "默认色走直通字形管线后，满覆盖落屏必须等于迁移前用旧色的观感"
            );
        }

        // 其它 2 个默认色数值不动（它们不走字形，只需换顺序，不需要补偿）
        assert_eq!(d.map_bg_color, 0x40000000);
        assert_eq!(d.stroke_color, 0x66002814);

        // 两份默认值副本同源：serde 缺键（default_*_color()）vs 无配置文件（Default::default()）
        let missing_keys: HudLayoutConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(
            colors(&missing_keys),
            colors(&d),
            "缺键走 default_*_color()、Default::default() 走字面量 —— 两者必须一致"
        );
    }

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

    /// 环槽数配置：缺键 = 默认 32，越界钳到 2..=256（GUI 的 min/max 与这里同口径）。
    #[test]
    fn ring_config_defaults_and_clamps() {
        let c: RingConfig = serde_json::from_str("{}").unwrap();
        assert_eq!((c.frames, c.map_samples), (32, 32), "缺键取默认，不是 0");
        assert_eq!((c.frames_clamped(), c.map_samples_clamped()), (32, 32));

        let c: RingConfig = serde_json::from_str(r#"{"frames":1,"map_samples":9999}"#).unwrap();
        assert_eq!(c.frames_clamped(), 2, "1 个槽时环的语义不成立");
        assert_eq!(c.map_samples_clamped(), 256);

        // 没有 ring 段的老配置照常工作
        let cfg: HudLayoutConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.ring.frames_clamped(), 32);
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

    /// 旧配置（`config/*.json`）里还能见到 `record.poll_ms`（记录频率）——
    /// 这个键已删除，写了也只当没写；序列化回去不再出现。
    #[test]
    fn record_poll_ms_key_is_ignored() {
        let old: RecordConfig = serde_json::from_str(r#"{"enabled":true,"poll_ms":33}"#).unwrap();
        assert!(old.enabled, "老配置必须照常能读（未知键忽略）");
        assert_eq!(old.pool_mb, RecordConfig::default().pool_mb, "其它键取默认值");
        let json = serde_json::to_string(&old).unwrap();
        assert!(!json.contains("poll_ms"), "记录频率不再写回配置：{json}");
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

    /// 记录配置：段名是 `record`，且**没有** origin_lat/origin_lon/format/poll_ms ——
    /// 那些概念不属于 wp8f 的记录格式（旧键一律按未知字段忽略；记录频率跟随地图刷新）。
    #[test]
    fn record_section_has_no_legacy_keys() {
        let new: HudLayoutConfig =
            serde_json::from_str(r#"{"record":{"enabled":true,"pool_mb":8}}"#).unwrap();
        assert!(new.record.enabled && new.record.pool_mb == 8);

        // 旧键必须被忽略（serde 默认忽略未知字段）：读出来是默认值，而不是报错
        let old: HudLayoutConfig =
            serde_json::from_str(r#"{"record":{"enabled":true,"origin_lat":1.0,"format":"acmi"}}"#)
                .unwrap();
        assert!(old.record.enabled);

        let json = serde_json::to_string(&new).unwrap();
        assert!(json.contains("\"record\""), "{json}");
        assert!(!json.contains("tacview"), "不该出现旧段名：{json}");
        assert!(!json.contains("origin_"), "零点经纬度已从配置里删除：{json}");
        assert!(!json.contains("\"format\""), "不再有输出格式选项：{json}");
        assert!(!json.contains("poll_ms"), "记录频率已删除（跟随地图刷新）：{json}");
    }

    /// core 数据刷新频率：默认 30 Hz（= 历史固定节拍），越界钳制，缺键走默认。
    #[test]
    fn refresh_hz_defaults_to_30_and_clamps() {
        let mut c = HudLayoutConfig::default();
        assert_eq!(c.refresh_hz, 30, "默认 30 Hz —— 与解耦前的固定节拍一致");
        assert_eq!(c.refresh_interval_ns(), 33_333_333, "30 Hz → 33.33 ms 节拍（历史值）");

        // 缺键 / 空配置：必须拿到同一个默认值（serde default 与 Default 同源）
        let missing: HudLayoutConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(missing.refresh_hz, default_refresh_hz());
        assert_eq!(missing.refresh_interval_ns(), c.refresh_interval_ns());

        // 越界钳制：0/1000 都不该让主循环变成忙等或跑飞
        c.refresh_hz = 0;
        assert_eq!(c.refresh_hz_clamped(), REFRESH_HZ_MIN);
        assert_eq!(c.refresh_interval_ns(), 1_000_000_000 / REFRESH_HZ_MIN as u64);
        c.refresh_hz = 1000;
        assert_eq!(c.refresh_hz_clamped(), REFRESH_HZ_MAX);
        assert_eq!(c.refresh_interval_ns(), 1_000_000_000 / REFRESH_HZ_MAX as u64);

        // 上限 60 Hz 也是记录采样率的上限（记录周期最小 = 一个节拍）：节拍不会小到 0
        c.refresh_hz = REFRESH_HZ_MAX;
        assert!(c.refresh_interval_ns() >= 16_666_666, "60 Hz 的节拍");
    }

    /// 地图记录间隔：默认值来自常量（不许抄字面量）、钳制区间 `1..=refresh_hz`、缺键走同一个默认。
    /// 判据只看刷新率；记录线程的采样周期跟随本值（core 侧 `mapobj::period_ms` 现算）。
    #[test]
    fn map_obj_record_every_frames_defaults_to_the_constant_and_clamps_to_refresh_hz() {
        let mut c = HudLayoutConfig::default();
        assert_eq!(
            c.map_obj_record_every_frames, MAP_OBJ_INTERVAL_FRAME,
            "Default 必须取 default_map_obj_record_every_frames()（= MAP_OBJ_INTERVAL_FRAME），不是字面量"
        );
        // 默认 30 Hz → 上限 30，默认值 8 在区间内：原样生效
        assert_eq!(c.map_obj_record_every_frames_clamped(), MAP_OBJ_INTERVAL_FRAME);
        assert_eq!(c.refresh_hz_clamped(), 30);
        assert_eq!(c.map_obj_record_every_frames_clamped(), 8, "8 ≤ 30，不该被夹");

        // 缺键 / 空配置：必须拿到同一个默认值（serde default 与 Default 同源）
        let missing: HudLayoutConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(missing.map_obj_record_every_frames, default_map_obj_record_every_frames());
        assert_eq!(missing.map_obj_record_every_frames_clamped(), c.map_obj_record_every_frames_clamped());

        // 旧键名已不再被识别（语义/命名订正）：写了也只当没写 → 回到缺键默认值
        let old: HudLayoutConfig =
            serde_json::from_str(r#"{"map_obj_interval_frames": 1}"#).unwrap();
        assert_eq!(old.map_obj_record_every_frames, MAP_OBJ_INTERVAL_FRAME, "旧键不再生效");

        // 下界：0 会让闸门每帧触发（8111 请求打满）→ 保底 1（1 = 每帧记录，是合法值，不被抬高）
        c.map_obj_record_every_frames = 0;
        assert_eq!(c.map_obj_record_every_frames_clamped(), MAP_OBJ_RECORD_EVERY_FRAMES_MIN);
        assert_eq!(MAP_OBJ_RECORD_EVERY_FRAMES_MIN, 1);
        c.map_obj_record_every_frames = 1;
        assert_eq!(c.map_obj_record_every_frames_clamped(), 1, "1 数据帧 = 每帧记录，是合法下限");

        // 上界 = 1 秒的数据帧数：30 Hz → 30；超了就夹到 30（600 帧不再是允许值）
        c.map_obj_record_every_frames = 30;
        assert_eq!(c.map_obj_record_every_frames_clamped(), 30, "正好 1 秒，仍在区间内");
        c.map_obj_record_every_frames = 31;
        assert_eq!(c.map_obj_record_every_frames_clamped(), 30, "31 帧 @30 Hz = 1033 ms > 1 s → 夹到 30");
        c.map_obj_record_every_frames = 600;
        assert_eq!(c.map_obj_record_every_frames_clamped(), 30, "旧的 600 上限已删除，按刷新率封顶");
        c.map_obj_record_every_frames = u64::MAX;
        assert_eq!(c.map_obj_record_every_frames_clamped(), 30);

        // 上限**跟着 refresh_hz 走**：同一份配置在 5 Hz / 60 Hz 下夹出不同的生效值
        c.map_obj_record_every_frames = 600;
        c.refresh_hz = 5;
        assert_eq!(c.map_obj_record_every_frames_clamped(), 5, "5 Hz → 上限 5 帧（1 秒）");
        c.refresh_hz = 60;
        assert_eq!(c.map_obj_record_every_frames_clamped(), 60, "60 Hz → 上限 60 帧（1 秒）");
        // 刷新率越界（0 / 1000）也要用钳制后的刷新率算上限，不能是 0
        c.refresh_hz = 0;
        assert_eq!(c.map_obj_record_every_frames_clamped(), REFRESH_HZ_MIN as u64);
        c.refresh_hz = 1000;
        assert_eq!(c.map_obj_record_every_frames_clamped(), REFRESH_HZ_MAX as u64);
    }
}
