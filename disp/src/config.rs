//! 布局配置（`config/*.json` 的整份 HUD 段 + `record`/`ring`/`datalink` 子段）与缺省值。
//!
//! 从 `lib.rs` 拆出的一块；对外路径不变（`lib.rs` 有 `pub use config::{...}`）。
//! 这里只放**数据类型 + 缺省值 + 区间钳制**：`OnceLock` 全局、读写盘、拖拽状态仍在 `lib.rs`。
//!
//! 一条约定：每个键的缺省值**只写一次** —— `default_*()` 是唯一来源，`Default::default()`
//! 调它们，容器级 `#[serde(default)]` 让"缺键"也走同一处。抄一份字面量就多一个真值来源
//! （`empty_json_equals_builtin_default_for_every_config_section` 盯着这条）。

use super::clamp::{Clamp, REFRESH_HZ_RANGE, RING_SLOTS_RANGE};
use super::font;
use super::hud_type::{default_hud_type, HudType};
use serde::{Deserialize, Serialize};

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
#[serde(default)]
pub struct RecordConfig {
    /// 启用飞行记录
    pub enabled: bool,
    /// 输出目录（空 = ./logs）
    pub output_dir: String,
    /// 每组 CSV 内存池的**初始**容量（MiB，默认 8）：记录期间只往内存里追加行、写满自动扩容。
    /// 按默认采样周期（8 数据帧 @30 Hz = 266 ms）估算：1 MiB ≈ 1500 行 ≈ 6.5 分钟。
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
#[serde(default)]
pub struct RingConfig {
    /// `DisplayData` 帧环槽数（默认 32 ≈ 30 Hz 下 1 秒）
    pub frames: u32,
    /// `MapDisplay` 地图环槽数（默认 32 ≈ 4 Hz 采样下 8 秒）
    pub map_samples: u32,
}

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
        RING_SLOTS_RANGE.apply(self.frames) as usize
    }

    pub fn map_samples_clamped(&self) -> usize {
        RING_SLOTS_RANGE.apply(self.map_samples) as usize
    }
}

/// Link-8 数据链客户端配置（并入布局配置 JSON 的 `datalink` 段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DataLinkConfig {
    /// 是否启用 Link-8 上报/接收
    pub enabled: bool,
    /// 服务端 IP/主机名
    pub server: String,
    /// 服务端端口
    pub port: u16,
    /// 加密密钥（须与服务端一致）
    pub key: String,
    /// 对局编号（同对局才互相共享）
    pub session_id: u8,
    /// 上报频率（Hz，整数）
    pub send_hz: u32,
    /// 友军快照超时（秒）：这么久没收到服务端应答就清空友军（地图标记 + 数据标签面板）；
    /// `0` = 一直保留最后一份快照。与服务端自己的追踪超时同口径。
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
    pub hud_type: HudType,
    pub circle_x: i32,
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
    pub flight_info_enabled: bool,
    pub engine_info_enabled: bool,
    pub map_enabled: bool,
    pub datalink_panel_enabled: bool,
    /// 语音/提示音开关（默认开）。关掉后不出任何声音，但视觉告警（告警 X）照旧。
    pub voice_warnings_enabled: bool,
    /// F-18 风格**机动告警声**开关（默认开）：与语音告警是两个独立音层，
    /// 关掉只停蜂鸣/长鸣，`voice_warnings_enabled` 与视觉告警都不受影响。
    pub maneuver_tone_enabled: bool,
    /// **core 数据刷新频率（Hz）**：主循环读 8111 + 更新 `DisplayData` 的节拍，也决定面板重绘
    /// 频率。缺省 30，钳到 [`REFRESH_HZ_MIN`]..=[`REFRESH_HZ_MAX`]。
    ///
    /// ⚠️ 帧计数类常量（地图记录间隔、语音告警检查 8 帧、告警保持 30 帧）按**帧**算，
    /// 改本值它们的**时间口径一起变**；记录线程的采样周期（= 地图刷新周期）也跟着变。
    pub refresh_hz: u32,
    /// **地图记录间隔（数据帧）**：本机地图位置、友军快照、地图对象列表每多少数据帧记一次
    /// （core 闸门 `frame_count >= next_mapobj_frame`，初值 = 本值 → 首次记录在第 N 个数据帧）。
    ///
    /// * 单位是数据帧：毫秒 = `帧数 × 1000 / refresh_hz`（换算只在 core 的 `mapobj`）；
    /// * 取值 `1..=refresh_hz`（缺省 [`MAP_OBJ_INTERVAL_FRAME`]），夹紧见
    ///   [`Self::map_obj_record_every_frames_clamped`]；
    /// * 记录线程的起点也用本值（首个周期 `pos_x/pos_y` 还是初值 `(0.5, 0.5)`，记下来会闪）。
    pub map_obj_record_every_frames: u64,
    pub map_font_size: f32,
    pub map_icon_size: f32,
    pub map_obj_font_size: f32,
    pub circle_font_size: f32,
    pub circle_ring_radius: f32,
    pub mini_font_size: f32,
    pub grid_spacing: f32,
    pub grid_padding: f32,
    // —— 颜色（7 个键）：配置里一律 **`#RRGGBBAA`**，换算在 `hex_color` 模块里成内部 `0xAARRGGBB`。
    //    值**直通落屏、不预乘**（半透明合成交给窗口）；其中 data/label/unit/hint/alert 是
    //    **文字色**，值是补偿值 —— 改动前先看 `draw/glyph_cache.rs`。
    #[serde(with = "hex_color")]
    pub map_bg_color: u32,
    #[serde(with = "hex_color")]
    pub data_color: u32,
    #[serde(with = "hex_color")]
    pub label_color: u32,
    #[serde(with = "hex_color")]
    pub unit_color: u32,
    #[serde(with = "hex_color")]
    pub hint_color: u32,
    #[serde(with = "hex_color")]
    pub alert_color: u32,
    #[serde(with = "hex_color")]
    pub stroke_color: u32,
    /// Warning X blink frequency in Hz (on/off cycles per second, 50% duty
    /// cycle; `<= 0` disables blinking and keeps the X visible).
    pub warning_blink_hz: f32,
    /// Link-8 数据链客户端配置
    #[serde(alias = "link8")]
    pub datalink: DataLinkConfig,
    /// 飞行记录配置（`.wpr`）
    pub record: RecordConfig,
    /// 环形缓冲区槽数（帧环 / 地图环）
    pub ring: RingConfig,
    /// 友军数据标签 Panel 位置（旧配置键 `link8_x`/`link8_y` 仍兼容）
    #[serde(alias = "link8_x")]
    pub datalink_x: i32,
    #[serde(alias = "link8_y")]
    pub datalink_y: i32,
    /// **界面语言**：`resource/i18n/<code>.json` 的 code（`zh_simp`/`zh_trad`/`en`/…，旧写法也认），
    /// `auto` = 跟随系统。消费方是 GUI/后端；**HUD 不看这个键**（看 `hud_lang_path`）。
    /// HUD 会把它原样读入、原样存回，别在保存时弄丢。
    pub language: String,
    /// **HUD 标签语言文件**：相对安装根的路径，例如 `resource/lang/zh.json`（绝对路径也接受）。
    /// 自建 `resource/lang/xxx.json`（键集与随包两份一致）指过来就能换一套标签。
    ///
    /// * 随包的 `resource/lang/{zh,en}.json` 是**生成产物**（真值 = `resource/i18n/` 里的
    ///   `hud.*` 键，由 `scripts/gen_lang.py` 生成；见 `disp/src/i18n.rs` 的头注释）；
    /// * 空 = 随包的中文表 `resource/lang/zh.json`（不看 [`Self::language`]）；
    /// * 由 **core 初始化时读**（`core/src/main.rs`）：只留 `hud.*` 标签，JSON 与中间对象立刻释放；
    /// * 文件不存在 / 解析失败 → HUD 报错退出（`exit 2`），不静默回退。
    pub hud_lang_path: String,
    /// **语音包目录**：相对安装根的目录，例如 `resource/voice/<包名>`（绝对路径也接受）。
    /// 目录里放 `*.wav`（文件主干 = 代码里点播的名字，如 `warn_gear`）；**放一个新子目录就是一个新包**。
    ///
    /// * 启动时整目录读进内存（`core/src/warnings.rs::preload`），之后播放零磁盘 I/O；
    /// * 单个 wav 缺失/损坏 → 去重警告后跳过；目录找不到 → 警告后静音运行（语音是增强功能）。
    pub voice_path: String,
    /// **HUD 正文字体**：指向真实字体文件的路径，值自带 `resource/fonts/` 这一段（绝对路径也接受）。
    ///
    /// * **必填**：空、文件不存在或解析不了 → HUD 报错退出（`exit 2`），不会挑别的字体顶上；
    /// * 解析基准 = 仓库/安装根（cwd → 上一级 → exe 目录 → exe 目录上一级）；
    /// * 取值来源 = `GET /api/fonts`（GUI 的「字体」卡片）；细节见 `disp/src/font.rs`。
    pub font_path: String,
}

/// `font_path` 的缺省值。与"显式写空字符串"是两回事：缺键 = 用随包那份；
/// 写了 `""` = 配置没写完 → HUD 退出（`config/default.json` 里显式写着同一个值）。
pub(crate) fn default_font_path() -> String {
    format!("{}/{}", font::FONT_DIR, font::DEFAULT_TEXT_FILE)
}

impl Default for HudLayoutConfig {
    fn default() -> Self {
        Self {
            // 每个键的默认值**只写在这一个函数里**（serde 缺键也走它：容器级 `#[serde(default)]`），
            // 所以一律调 `default_*()`，不要再抄字面量 —— 抄一份就多一个真值来源。
            hud_type: default_hud_type(),
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
            map_font_size: default_map_font(),
            map_icon_size: default_map_icon(),
            map_obj_font_size: default_map_obj_font(),
            circle_font_size: default_circle_font(),
            circle_ring_radius: default_circle_radius(),
            mini_font_size: default_mini_font(),
            grid_spacing: default_grid_spacing(),
            grid_padding: default_grid_padding(),
            // 颜色一律走 default_*_color()：**单一来源**。这里原来自己抄了一份字面量，
            // 迁移文字色时"改了函数、漏了这份副本"就得到两套默认色（缺键配置与无配置文件不一致）。
            map_bg_color: default_map_bg_color(),
            data_color: default_data_color(),
            label_color: default_label_color(),
            unit_color: default_unit_color(),
            hint_color: default_hint_color(),
            alert_color: default_alert_color(),
            stroke_color: default_stroke_color(),
            warning_blink_hz: default_warning_blink_hz(),
            datalink: DataLinkConfig::default(),
            flight_info_enabled: default_true(),
            engine_info_enabled: default_true(),
            map_enabled: default_true(),
            datalink_panel_enabled: default_true(),
            voice_warnings_enabled: default_true(),
            maneuver_tone_enabled: default_true(),
            refresh_hz: default_refresh_hz(),
            map_obj_record_every_frames: default_map_obj_record_every_frames(),
            record: RecordConfig::default(),
            ring: RingConfig::default(),
            datalink_x: default_datalink_x(),
            datalink_y: default_datalink_y(),
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

impl HudLayoutConfig {
    /// 钳制后的 core 数据刷新频率（Hz）。
    pub fn refresh_hz_clamped(&self) -> u32 {
        REFRESH_HZ_RANGE.apply(self.refresh_hz)
    }

    /// 主循环节拍（纳秒）—— core 侧"读 8111 / 更新 DisplayData"周期的**唯一来源**
    /// （`refresh_hz` 越界时按 [`REFRESH_HZ_RANGE`] 钳制）。
    pub fn refresh_interval_ns(&self) -> u64 {
        1_000_000_000 / self.refresh_hz_clamped() as u64
    }

    /// 钳制后的**地图记录间隔（数据帧）**：`1..=refresh_hz_clamped()`（0 会让闸门每帧触发，
    /// 超过 1 秒的间隔已经不是"记录"而是"快照"）。调用方一律用这个值，不要直接读字段。
    pub fn map_obj_record_every_frames_clamped(&self) -> u64 {
        Clamp::new(
            MAP_OBJ_RECORD_EVERY_FRAMES_MIN,
            self.refresh_hz_clamped() as u64,
        )
        .apply(self.map_obj_record_every_frames)
    }
}

/// 地图记录间隔的**缺键默认值**：本机地图位置、友军快照、地图对象列表都按这个间隔记录
/// （8 数据帧 ≈ 30 Hz 下 4 Hz）。
///
/// 真正生效的是配置项 [`HudLayoutConfig::map_obj_record_every_frames`]，本常量只提供缺键默认值
/// 与 `logger` 的记录起点阈值 —— 默认值只此一处，别在别处再抄字面量。
pub const MAP_OBJ_INTERVAL_FRAME: u64 = 8;

#[cfg(test)]
mod tests {
    use super::*;

    /// **缺键 == 内置默认**：这是"容器级 `#[serde(default)]`"该给出的唯一保证，
    /// 也是删掉字段级 `#[serde(default = "…")]`（冗余）的安全网。
    /// 谁把某字段的 `Default` 改成与 serde 原本的默认不同，这里立刻红。
    #[test]
    fn empty_json_equals_builtin_default_for_every_config_section() {
        fn same<T>(name: &str)
        where
            T: serde::Serialize + serde::de::DeserializeOwned + Default,
        {
            let empty: T = serde_json::from_str("{}").unwrap();
            assert_eq!(
                serde_json::to_value(&empty).unwrap(),
                serde_json::to_value(T::default()).unwrap(),
                "{name}: 缺键读出来的值必须与 Default::default() 完全一致",
            );
        }
        same::<HudLayoutConfig>("HudLayoutConfig");
        same::<RecordConfig>("record");
        same::<RingConfig>("ring");
        same::<DataLinkConfig>("datalink");

        // 嵌套段也一样（整份 `{}` 里的 record/ring/datalink 必须就是各自的 Default）
        let whole: HudLayoutConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(
            serde_json::to_value(&whole.record).unwrap(),
            serde_json::to_value(RecordConfig::default()).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&whole.ring).unwrap(),
            serde_json::to_value(RingConfig::default()).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&whole.datalink).unwrap(),
            serde_json::to_value(DataLinkConfig::default()).unwrap()
        );
    }

    /// 配置派生量的区间：默认值必须落在区间内，且区间端点就是"生效上下限"。
    #[test]
    fn config_ranges_contain_their_defaults() {
        let c = HudLayoutConfig::default();
        assert!(REFRESH_HZ_RANGE.contains(c.refresh_hz), "默认刷新率必须在区间内");
        assert!(REFRESH_HZ_RANGE.contains(c.refresh_hz_clamped()));
        assert_eq!(RING_SLOTS_RANGE.lo, 2, "1 槽时下一个待写槽就是正在读的那个");
        assert_eq!(RING_SLOTS_RANGE.hi, 256);
        let r = RingConfig::default();
        assert!(RING_SLOTS_RANGE.contains(r.frames));
        assert!(RING_SLOTS_RANGE.contains(r.map_samples));
        // 地图记录间隔的区间是"动态上界 = refresh_hz"，两端都要在区间里
        let clamp = Clamp::new(MAP_OBJ_RECORD_EVERY_FRAMES_MIN, c.refresh_hz_clamped() as u64);
        assert!(
            clamp.contains(c.map_obj_record_every_frames_clamped()),
            "生效的地图记录间隔必须落在 {clamp:?} 里"
        );
    }

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
