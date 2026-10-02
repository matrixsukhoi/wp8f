//! `.wpr`（wp8f record）：wp8f 自有的飞行记录容器。
//!
//! 一份记录 = 一个文件：**地图信息 + 底图 + N 组逐帧飞行数据（CSV）**。只存 wp8f 自己的数据
//! （归一化地图坐标、km/h、百分比操纵面、纳秒时间戳……），不含任何外部格式的概念与换算 ——
//! 那些只在导出时由控制台的转换器处理（`crate::convert`，只在启用 `convert` feature 时编译）。
//!
//! # 容器布局（小端，版本 2）
//!
//! ```text
//! offset 0   magic  4B  "WPR1"
//! offset 4   u32       容器版本（当前 = 2）
//! offset 8   u32       meta_len     JSON 元信息长度（UTF-8）
//! offset 12  u32       group_count  CSV 组数（恒 >= 1；**组 0 = 玩家飞机**）
//! offset 16  u32       img_len      地图底图长度（原样字节，通常是 JPEG）
//! offset 20  u32 × G   csv_len[]    每组 CSV 的长度（UTF-8，含表头），按组号排列
//! offset 20+4G ...     meta | csv[0] | csv[1] | … | csv[G-1] | img
//! ```
//!
//! 组数表放在 meta 之前：先读长度表就能直接定位每一段，不用先解 JSON。元数据里另有一份
//! `groups` 与 `group_info[]`（机型 + 帧数，给人看/给列表用），解码时会与头里的 `group_count`
//! 交叉校验 —— 不一致直接报错，不猜。
//!
//! 组 0 永远是玩家飞机（回放默认选中它）；其余组由记录线程的 `register_group` 预留接口写入。
//!
//! # 内存池
//!
//! 记录期间**不碰磁盘**：每帧只往 [`CsvPool`] 追加一行 CSV，飞行结束才把 meta + 各组 CSV + 底图
//! 拼成一个 `.wpr` 写回 —— 既没有逐帧 I/O，也没有"结束时再做一次全量序列化"的第二遍开销。

use serde::{Deserialize, Serialize};

/// 容器魔数。
pub const MAGIC: [u8; 4] = *b"WPR1";
/// 容器版本（2 = 多组：meta + 地图 + N 份 CSV）。
pub const VERSION: u32 = 2;
/// 固定头长度（magic + 版本 + meta_len + group_count + img_len）。
pub const HEADER_LEN: usize = 20;
/// 组数表里每组的长度字段（u32 CSV 长度）。
pub const GROUP_LEN: usize = 4;
/// 文件扩展名（小写，不含点）。
pub const EXT: &str = "wpr";
/// 每组 CSV 内存池的默认初始容量（MiB）—— 配置项 `record.pool_mb` 的默认值。
pub const DEFAULT_POOL_MB: u32 = 8;

/// 除 `time_ns` / `type` 之外的数据列数（= [`COL_DECIMALS`] 与 [`RecordRow::v`] 的长度）。
pub const NUM_COLS: usize = 43;

/// 表头：前两列是 `time_ns`（UNIX 纳秒）与 `type`（机型），其余**顺序即 [`RecordRow::v`] 的下标**。
pub const CSV_HEADER: &str = "time_ns,type,frame,x,y,altitude,radio_altitude,ias,tas,mach,aoa,aos,ny,vy,wx,heading,roll,pitch,aileron,elevator,rudder,trimmer,flaps,gear,airbrake,throttle,wing_sweep,turn_rate,turn_radius,sep,energy_height,thrust_to_weight,total_thrust,total_hp,thrust_percent,total_drag,manifold_pressure,fuel_kg,fuel_percent,fuel1_kg,overspeed_warning,mach_warning,g_load_warning,voice_alarm,brake_caution";

/// 每列小数位（与 `CSV_HEADER` 的第 3 列起一一对应）。
pub const COL_DECIMALS: [usize; NUM_COLS] = [
    0, // frame
    7, 7, // x, y（归一化地图坐标 0..1）
    2, 2, // altitude, radio_altitude (m)
    2, 2, // ias, tas (km/h)
    3, // mach
    2, 2, // aoa, aos (deg)
    3, 3, // ny (g), vy (m/s)
    2, // wx (deg/s)
    1, 1, 1, // heading, roll, pitch (deg)
    2, 2, 2, // aileron, elevator, rudder
    2, // trimmer
    2, 2, 2, // flaps, gear, airbrake（百分比）
    3, // throttle（0..1，加力可 >1）
    2, // wing_sweep（可变后掠翼比例）
    2, 2, // turn_rate (deg/s), turn_radius (m)
    1, // sep (m/s)
    1, // energy_height (m)
    3, // thrust_to_weight
    1, // total_thrust (kgf)
    1, // total_hp
    2, // thrust_percent
    1, // total_drag (kgf)
    3, // manifold_pressure
    1, 1, 1, // fuel_kg, fuel_percent, fuel1_kg
    0, 0, 0, 0, 0, // overspeed_warning, mach_warning, g_load_warning, voice_alarm, brake_caution
];

/// 一帧飞行记录：`time_ns` + 机型 + 定长数值列（列义见 [`CSV_HEADER`]）。
///
/// 用数组而不是 40 个具名字段：增删列只动表头/小数位表/写入端三处，CSV 读写都是循环，
/// 不会出现"加了字段忘了写"的漏项；列名交给读端（控制台回放/转换器）按名字取。
#[derive(Debug, Clone, PartialEq)]
pub struct RecordRow {
    pub time_ns: u64,
    pub type_str: String,
    /// 数值列，下标与 [`CSV_HEADER`] 的第 3 列起一一对应
    pub v: [f64; NUM_COLS],
}

// `[f64; 41]` 超出数组 `Default` 的实现范围（≤32），手写一份
impl Default for RecordRow {
    fn default() -> Self {
        Self { time_ns: 0, type_str: String::new(), v: [0.0; NUM_COLS] }
    }
}

impl RecordRow {
    /// 一行 CSV（逗号分隔；机型里的逗号会被替换掉，保证列数固定）。
    pub fn to_csv_line(&self) -> String {
        let mut s = String::with_capacity(64 + NUM_COLS * 8);
        s.push_str(&self.time_ns.to_string());
        s.push(',');
        s.push_str(&sanitize(&self.type_str));
        for (i, val) in self.v.iter().enumerate() {
            s.push(',');
            s.push_str(&format!("{:.*}", COL_DECIMALS[i], fin(*val)));
        }
        s
    }

    /// 解析一行 CSV（列数必须与表头一致）。
    pub fn from_csv_line(line: &str) -> Result<Self, String> {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() != NUM_COLS + 2 {
            return Err(format!("列数 {} != {}：{line}", f.len(), NUM_COLS + 2));
        }
        let time_ns = f[0].trim().parse::<u64>().map_err(|e| format!("time_ns 非法：{e}"))?;
        let mut v = [0f64; NUM_COLS];
        for (i, slot) in v.iter_mut().enumerate() {
            let raw = f[i + 2].trim();
            *slot = if raw.is_empty() {
                0.0
            } else {
                raw.parse::<f64>().map_err(|e| format!("第 {} 列不是数字（{raw}）：{e}", i + 3))?
            };
        }
        Ok(Self { time_ns, type_str: f[1].to_string(), v })
    }
}

/// 表头里的列名，供读端按名字取值（下标与 [`RecordRow::v`] 相差 2）。
pub fn column_names() -> Vec<&'static str> {
    CSV_HEADER.split(',').collect()
}

/// 数值列下标（严格等于 [`CSV_HEADER`] 里该列的位置 **减 2**）。
/// 读写端按名字取值，避免散落的魔数；`column_index_matches_header` 测试会逐列核对。
pub mod col {
    pub const FRAME: usize = 0;
    pub const X: usize = 1;
    pub const Y: usize = 2;
    pub const ALTITUDE: usize = 3;
    pub const RADIO_ALTITUDE: usize = 4;
    pub const IAS: usize = 5;
    pub const TAS: usize = 6;
    pub const MACH: usize = 7;
    pub const AOA: usize = 8;
    pub const AOS: usize = 9;
    pub const NY: usize = 10;
    pub const VY: usize = 11;
    pub const WX: usize = 12;
    pub const HEADING: usize = 13;
    pub const ROLL: usize = 14;
    pub const PITCH: usize = 15;
    pub const AILERON: usize = 16;
    pub const ELEVATOR: usize = 17;
    pub const RUDDER: usize = 18;
    pub const TRIMMER: usize = 19;
    pub const FLAPS: usize = 20;
    pub const GEAR: usize = 21;
    pub const AIRBRAKE: usize = 22;
    pub const THROTTLE: usize = 23;
    pub const WING_SWEEP: usize = 24;
    pub const TURN_RATE: usize = 25;
    pub const TURN_RADIUS: usize = 26;
    pub const SEP: usize = 27;
    pub const ENERGY_HEIGHT: usize = 28;
    pub const FUEL_KG: usize = 35;
}

/// 地图信息：**归一化坐标 → 米/km 的全部换算数据**（与 HUD 地图面板同一口径）。
///
/// ```text
/// 相对地图 0 点（地图西/南边界）的 km：东 = x × width_m/1000，北 = (1 − y) × height_m/1000
/// 世界坐标（米）= (min_x_m + 东 km×1000, min_y_m + 北 km×1000)
/// ```
///
/// 归一化 `(0,0)` = 地图西北角、`(1,1)` = 东南角；y **向南增大**，所以换算成"北为正"时要竖直
/// 翻转。`min_x_m/min_y_m` 是地图矩形**西/南边界**的世界坐标（= `map_info.map_min`），
/// 于是贴图、画点、画格、导出经纬度都用同一组数字，谁都不用猜"相对谁"。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MapMeta {
    /// 地图东西向尺寸（米，= `map_info.maxsize[0]`）：归一化 x × 它 = 相对地图 0 点的米
    pub width_m: f64,
    /// 地图南北向尺寸（米，= `map_info.maxsize[1]`）
    pub height_m: f64,
    /// 地图矩形**西**边界的世界坐标（米，= `map_info.map_min[0]`；缺省 0 = 西边界在世界 0 点）
    #[serde(default)]
    pub min_x_m: f64,
    /// 地图矩形**南**边界的世界坐标（米，= `map_info.map_min[1]`）
    #[serde(default)]
    pub min_y_m: f64,
    /// 区域格零点（= `map_info.grid_zero`；HUD 地图面板推导区域标识用，回放可选）
    #[serde(default)]
    pub grid_zero: [f64; 2],
    /// 区域格步长（= `map_info.grid_steps`）
    #[serde(default)]
    pub grid_steps: [f64; 2],
    /// 底图像素宽
    pub img_w: u32,
    /// 底图像素高
    pub img_h: u32,
    /// 底图 MIME（游戏给什么记什么，通常是 `image/jpeg`）
    pub img_mime: String,
}

impl MapMeta {
    /// 归一化地图坐标 → **相对地图 0 点的 km**（地图西/南边界为 0；向东、向北为正）
    /// —— 与 HUD 地图面板同口径（`归一化 × maxsize`，y 竖直翻转）。
    pub fn norm_to_km(&self, x: f64, y: f64) -> (f64, f64) {
        (x * self.width_m / 1000.0, (1.0 - y) * self.height_m / 1000.0)
    }

    /// 归一化地图坐标 → **世界坐标（米）**：地图 0 点的 km × 1000 加上矩形西/南边界。
    pub fn norm_to_world_m(&self, x: f64, y: f64) -> (f64, f64) {
        let (east_km, north_km) = self.norm_to_km(x, y);
        (self.min_x_m + east_km * 1000.0, self.min_y_m + north_km * 1000.0)
    }

    /// 归一化地图坐标 → **世界坐标（km，向北为正）**——回放/导出用的就是这个：
    /// 它同时是"相对地图 0 点的偏移 + `min_*`"，所以底图矩形、机体位置、ACMI 偏移全在一个系里。
    pub fn norm_to_world_km(&self, x: f64, y: f64) -> (f64, f64) {
        let (wx, wy) = self.norm_to_world_m(x, y);
        (wx / 1000.0, wy / 1000.0)
    }

    /// 地图矩形（世界坐标 km，向北为正）：`(x0, x1, y0, y1)`。
    pub fn world_rect_km(&self) -> (f64, f64, f64, f64) {
        (
            self.min_x_m / 1000.0,
            (self.min_x_m + self.width_m) / 1000.0,
            self.min_y_m / 1000.0,
            (self.min_y_m + self.height_m) / 1000.0,
        )
    }
}

/// 一组（一架飞机）的说明：机型 + 帧数。下标与 CSV 组号一一对应，**组 0 = 玩家**。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GroupInfo {
    /// 机型（该组首行的 `type`；空 = 没记到机型）
    pub aircraft: String,
    /// 该组帧数（= CSV 数据行数）
    pub frames: u64,
}

/// 记录元信息（JSON 段）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecordMeta {
    /// 容器版本（与 [`VERSION`] 一致）
    pub container: u32,
    /// 记录创建时刻（UNIX 毫秒；只用于文件名与显示）
    pub created_ms: u64,
    /// **CSV 组数**（与头里的 `group_count` 一致，恒 >= 1；组 0 = 玩家）
    #[serde(default)]
    pub groups: u32,
    /// 玩家组（组 0）的帧数；各组帧数见 [`GroupInfo::frames`]
    pub frames: u64,
    /// 记录频率（Hz，= 1000 / poll_ms）
    pub poll_hz: f64,
    /// 地图信息
    pub map: MapMeta,
    /// 生成者（"wp8f"）
    pub generator: String,
    /// 机型（组 0 的机型，便于列表里直接显示）
    pub aircraft: String,
    /// 每组一行说明（机型 + 帧数），下标 = 组号
    #[serde(default)]
    pub group_info: Vec<GroupInfo>,
}

/// 一组 CSV 文本（表头 + 每帧一行）。**组 0 = 玩家飞机**。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordGroup {
    /// 完整 CSV 文本（UTF-8，第一行是 [`CSV_HEADER`]）
    pub csv: String,
}

impl RecordGroup {
    /// 从 CSV 文本建一组（校验表头）。
    pub fn from_csv(csv: String) -> Result<Self, String> {
        check_csv_header(&csv)?;
        Ok(Self { csv })
    }

    /// 解析回行（导出/回放用；8 MiB 文本约十几毫秒 —— 只在读端按需做）。
    pub fn rows(&self) -> Result<Vec<RecordRow>, String> {
        parse_csv_rows(&self.csv)
    }

    /// 数据行数（不含表头）。
    pub fn frames(&self) -> u64 {
        self.csv.lines().skip(1).filter(|l| !l.trim().is_empty()).count() as u64
    }

    /// 该组的机型（首行的 `type`；空 = 没记到）。
    pub fn aircraft(&self) -> String {
        self.csv
            .lines()
            .nth(1)
            .and_then(|l| l.split(',').nth(1))
            .unwrap_or("")
            .to_string()
    }

    /// 首/末帧的纳秒时间（只看首尾两行，不解全表）。
    pub fn time_span_ns(&self) -> Option<(u64, u64)> {
        let mut it = self.csv.lines().skip(1).filter(|l| !l.trim().is_empty());
        let first = it.next()?.split(',').next()?.trim().parse::<u64>().ok()?;
        let last = self
            .csv
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .and_then(|l| l.split(',').next()?.trim().parse::<u64>().ok())
            .unwrap_or(first);
        Some((first, last))
    }
}

/// **CSV 内存池**：记录线程按 `record.pool_mb` 预分配一块容量，逐帧追加 `to_csv_line()`。
/// 写满自动扩容（`String` 自己的动态分配），记录期间不碰磁盘。
#[derive(Debug, Clone)]
pub struct CsvPool {
    buf: String,
    rows: u64,
}

impl CsvPool {
    /// 新建一个池：容量按字节给（含表头），至少装得下表头。
    pub fn new(capacity_bytes: usize) -> Self {
        let mut buf = String::with_capacity(capacity_bytes.max(CSV_HEADER.len() + 1));
        buf.push_str(CSV_HEADER);
        buf.push('\n');
        Self { buf, rows: 0 }
    }

    /// 追加一帧（不做任何换算 —— 记录就是 `DisplayData` 本身）。
    pub fn push(&mut self, row: &RecordRow) {
        self.buf.push_str(&row.to_csv_line());
        self.buf.push('\n');
        self.rows += 1;
    }

    /// 已写入的数据行数。
    pub fn rows(&self) -> u64 {
        self.rows
    }

    /// 当前占用字节数。
    pub fn bytes(&self) -> usize {
        self.buf.len()
    }

    /// 当前容量（>= [`Self::new`] 传入的初始值；写满后由 `String` 自动扩容）。
    pub fn capacity(&self) -> usize {
        self.buf.capacity()
    }

    /// 池内文本（含表头）。
    pub fn text(&self) -> &str {
        &self.buf
    }

    /// 收尾：交出 CSV 文本，变成一份 [`RecordGroup`]。
    pub fn into_group(self) -> RecordGroup {
        RecordGroup { csv: self.buf }
    }
}

/// 一份完整记录：meta + N 组 CSV + 底图（`groups[0]` = 玩家）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Record {
    pub meta: RecordMeta,
    pub groups: Vec<RecordGroup>,
    pub map_img: Vec<u8>,
}

impl Record {
    /// 玩家组（组 0）。
    pub fn player(&self) -> Option<&RecordGroup> {
        self.groups.first()
    }

    /// 打包成 `.wpr` 字节（纯函数，不做 I/O）。
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        if self.groups.is_empty() {
            return Err("记录至少要有一组 CSV（组 0 = 玩家飞机）".to_string());
        }
        let mut meta = self.meta.clone();
        meta.container = VERSION;
        meta.groups = self.groups.len() as u32;
        meta.group_info = self
            .groups
            .iter()
            .map(|g| GroupInfo { aircraft: g.aircraft(), frames: g.frames() })
            .collect();
        // 元数据里的 frames / aircraft 指玩家组（列表与时长显示看的就是它）
        meta.frames = meta.group_info[0].frames;
        if meta.aircraft.is_empty() {
            meta.aircraft = meta.group_info[0].aircraft.clone();
        }
        let meta_json = serde_json::to_vec(&meta).map_err(|e| format!("meta 序列化失败：{e}"))?;

        let mut out = Vec::with_capacity(
            HEADER_LEN + GROUP_LEN * self.groups.len() + meta_json.len() + self.map_img.len() + 4096,
        );
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&(meta_json.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.groups.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.map_img.len() as u32).to_le_bytes());
        for g in &self.groups {
            out.extend_from_slice(&(g.csv.len() as u32).to_le_bytes());
        }
        out.extend_from_slice(&meta_json);
        for g in &self.groups {
            out.extend_from_slice(g.csv.as_bytes());
        }
        out.extend_from_slice(&self.map_img);
        Ok(out)
    }

    /// 解包 `.wpr` 字节（纯函数；结构不对就返回可读的错误，绝不 panic）。
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let l = layout(bytes)?;
        let meta: RecordMeta = serde_json::from_slice(&bytes[l.meta.clone()])
            .map_err(|e| format!("meta JSON 解析失败：{e}"))?;
        let gc = l.csvs.len();
        if meta.groups as usize != gc {
            return Err(format!("元数据组数 {} != 头里的 {}", meta.groups, gc));
        }
        let mut groups = Vec::with_capacity(gc);
        for (i, r) in l.csvs.iter().enumerate() {
            let csv = std::str::from_utf8(&bytes[r.clone()])
                .map_err(|e| format!("第 {i} 组 CSV 不是 UTF-8：{e}"))?
                .to_string();
            check_csv_header(&csv).map_err(|e| format!("第 {i} 组：{e}"))?;
            groups.push(RecordGroup { csv });
        }
        Ok(Self { meta, groups, map_img: bytes[l.img.clone()].to_vec() })
    }

    /// 玩家组的时长（秒）：首末帧的纳秒差。
    pub fn duration_secs(&self) -> f64 {
        match self.player().and_then(|g| g.time_span_ns()) {
            Some((a, b)) if b > a => (b - a) as f64 / 1e9,
            _ => 0.0,
        }
    }
}

/// **容器头 + meta**（不解析 CSV、不读底图字节）—— 枚举"可当底图来源的记录"时用：
/// 扫一遍 `logs/*.wpr` 不该把每份记录的几 MB CSV 都解一遍。
#[derive(Debug, Clone)]
pub struct ContainerHead {
    /// meta JSON
    pub meta: RecordMeta,
    img_off: usize,
    img_len: usize,
}

impl ContainerHead {
    /// 只解容器头 + meta；结构不对返回可读错误，绝不 panic。
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let l = layout(bytes)?;
        let meta: RecordMeta = serde_json::from_slice(&bytes[l.meta.clone()])
            .map_err(|e| format!("meta JSON 解析失败：{e}"))?;
        Ok(Self { meta, img_off: l.img.start, img_len: l.img.len() })
    }

    /// 底图字节长度（0 = 这份记录没带底图）。
    pub fn img_len(&self) -> usize {
        self.img_len
    }

    /// 底图原始字节（`img_len == 0` → 空切片）。
    pub fn image<'a>(&self, bytes: &'a [u8]) -> &'a [u8] {
        &bytes[self.img_off..self.img_off + self.img_len]
    }
}

/// `.wpr` 容器的**布局**：只解析头与各段长度，不碰 meta JSON / CSV 内容 / 底图字节。
///
/// 两个解码器（[`Record::decode`] 与 [`ContainerHead::decode`]）共用它 —— 布局知识只有这一份，
/// 改格式不会出现"一处改了另一处忘"（那会表现成"底图读不到"这种看不出原因的故障）。
struct Layout {
    meta: std::ops::Range<usize>,
    /// 各组的 CSV 区间
    csvs: Vec<std::ops::Range<usize>>,
    img: std::ops::Range<usize>,
}

/// 容器布局只有一个版本：
/// `magic | ver | meta_len | group_count | img_len | csv_len[G] | meta | csv[] | img`。
fn layout(bytes: &[u8]) -> Result<Layout, String> {
    if bytes.len() < HEADER_LEN {
        return Err(format!("文件太短（{} 字节 < {} 字节头）", bytes.len(), HEADER_LEN));
    }
    if bytes[0..4] != MAGIC {
        return Err(format!("不是 .wpr 文件（magic = {:?}，应为 {:?}）", &bytes[0..4], MAGIC));
    }
    let u32at = |o: usize| -> usize {
        u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]) as usize
    };
    let version = u32at(4) as u32;
    if version != VERSION {
        return Err(format!(".wpr 版本 {version} 不认识（本程序只读版本 {VERSION}）"));
    }
    let trunc = |need: usize, have: usize| format!("文件截断：需要 {need} 字节，实际 {have}");
    let (ml, gc, il) = (u32at(8), u32at(12), u32at(16));
    if gc == 0 {
        return Err("组数为 0：记录至少要有 1 组（组 0 = 玩家飞机）".to_string());
    }
    let meta_start = HEADER_LEN
        .checked_add(GROUP_LEN * gc)
        .ok_or_else(|| "组数表长度溢出".to_string())?;
    if bytes.len() < meta_start {
        return Err(trunc(meta_start, bytes.len()));
    }
    let csv_start = meta_start.checked_add(ml).ok_or_else(|| "meta 长度溢出".to_string())?;
    let end = (0..gc)
        .try_fold(csv_start, |acc, i| acc.checked_add(u32at(HEADER_LEN + GROUP_LEN * i)))
        .and_then(|e| e.checked_add(il))
        .ok_or_else(|| "长度字段溢出".to_string())?;
    if bytes.len() < end {
        return Err(trunc(end, bytes.len()));
    }
    let mut csvs = Vec::with_capacity(gc);
    let mut off = csv_start;
    for i in 0..gc {
        let len = u32at(HEADER_LEN + GROUP_LEN * i);
        csvs.push(off..off + len);
        off += len;
    }
    Ok(Layout { meta: meta_start..csv_start, csvs, img: off..end })
}

/// 非有限值（NaN/inf）归 0.0：写进 `.wpr` 的文本里绝不出现 `NaN`/`inf` 字面量
/// （FlatCSV 导出原样吐出这份文本，所以守卫必须在唯一格式化点）。
pub(crate) fn fin(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// 把值里的逗号/换行换成下划线：机型名带逗号（`F-16C,Block 50`）也不会破坏列结构。
pub(crate) fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ',' => '_',
            '\n' | '\r' => ' ',
            _ => c,
        })
        .collect()
}

/// 校验 CSV 首行就是 [`CSV_HEADER`]（读写两端共用，改列时不会一边改一边忘）。
fn check_csv_header(csv: &str) -> Result<(), String> {
    match csv.lines().next() {
        Some(h) if h.trim() == CSV_HEADER => Ok(()),
        Some(h) => Err(format!("CSV 表头不匹配：{h}")),
        None => Err("CSV 是空的（缺表头）".to_string()),
    }
}

/// CSV 文本 → 行（空行跳过；行内列数不对就报错，带上行号）。
fn parse_csv_rows(csv: &str) -> Result<Vec<RecordRow>, String> {
    let mut rows = Vec::new();
    for (i, line) in csv.lines().skip(1).enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        rows.push(RecordRow::from_csv_line(line).map_err(|e| format!("第 {} 行：{e}", i + 1))?);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一行：`v` 全 0，只填坐标与帧号（便于断言）。
    fn row(t: u64, x: f64, y: f64) -> RecordRow {
        let mut v = [0f64; NUM_COLS];
        v[0] = 42.0;
        v[1] = x;
        v[2] = y;
        RecordRow { time_ns: t, type_str: "f_16c".to_string(), v }
    }

    /// 一组：走内存池写入（与记录线程同一条路径）。
    fn group(aircraft: &str, rows: &[(u64, f64, f64)]) -> RecordGroup {
        let mut pool = CsvPool::new(0);
        for (t, x, y) in rows {
            let mut r = row(*t, *x, *y);
            r.type_str = aircraft.to_string();
            pool.push(&r);
        }
        pool.into_group()
    }

    fn rec() -> Record {
        Record {
            meta: RecordMeta {
                created_ms: 1_700_000_000_000,
                poll_hz: 10.0,
                map: MapMeta {
                    width_m: 100_000.0,
                    height_m: 80_000.0,
                    // 西南角世界坐标（地图中心 = 世界 0 点）
                    min_x_m: -50_000.0,
                    min_y_m: -40_000.0,
                    grid_zero: [6_494.3, 19_547.5],
                    grid_steps: [5_500.0, 5_500.0],
                    img_w: 1024,
                    img_h: 819,
                    img_mime: "image/jpeg".to_string(),
                },
                generator: "wp8f".to_string(),
                ..RecordMeta::default()
            },
            groups: vec![group(
                "f_16c",
                &[(1_700_000_000_000_000_000, 0.5, 0.5), (1_700_000_000_100_000_000, 0.75, 0.25)],
            )],
            map_img: vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3],
        }
    }

    /// 换算口径 = HUD 地图面板那套（**归一化 × maxsize**）：元数据必须自带这些数据，
    /// 回放/导出不再各自猜"相对谁、乘什么"。
    #[test]
    fn map_meta_converts_like_the_hud_map_panel() {
        let m = MapMeta {
            width_m: 131_072.0,
            height_m: 131_072.0,
            min_x_m: -65_536.0,
            min_y_m: -65_536.0,
            ..MapMeta::default()
        };
        // 归一化 (0,0) = 西北角 ⇒ 东 0、北 = 地图高度；(1,1) = 东南角 ⇒ 东 = 宽度、北 0
        let (e, n) = m.norm_to_km(0.0, 0.0);
        assert!(e.abs() < 1e-9 && (n - 131.072).abs() < 1e-9, "西北角：{e},{n}");
        let (e, n) = m.norm_to_km(1.0, 1.0);
        assert!((e - 131.072).abs() < 1e-9 && n.abs() < 1e-9, "东南角：{e},{n}");
        let (e, n) = m.norm_to_km(0.5, 0.5);
        assert!((e - 65.536).abs() < 1e-9 && (n - 65.536).abs() < 1e-9, "地图中心：{e},{n}");
        // 世界坐标 = 地图 0 点的 km × 1000 + 西南角：中心正好落在世界 0 点
        let (wx, wy) = m.norm_to_world_m(0.5, 0.5);
        assert!(wx.abs() < 1e-6 && wy.abs() < 1e-6, "地图中心 = 世界 0 点：{wx},{wy}");
        let (wx, wy) = m.norm_to_world_m(0.0, 0.0);
        assert!((wx + 65_536.0).abs() < 1e-6 && (wy - 65_536.0).abs() < 1e-6, "西北角世界坐标：{wx},{wy}");
        let (x0, x1, y0, y1) = m.world_rect_km();
        assert!(
            (x0 + 65.536).abs() < 1e-9 && (x1 - 65.536).abs() < 1e-9
                && (y0 + 65.536).abs() < 1e-9 && (y1 - 65.536).abs() < 1e-9,
            "地图矩形：{x0},{x1},{y0},{y1}"
        );
    }

    /// 表头列数必须与「数值列 + 2」严格一致（加列时最容易漏的一处）。
    #[test]
    fn header_columns_match_row_layout() {
        let names = column_names();
        assert_eq!(names.len(), NUM_COLS + 2, "表头 {} 列 != NUM_COLS+2", names.len());
        assert_eq!(COL_DECIMALS.len(), NUM_COLS, "小数位表长度必须与数值列数一致");
        assert_eq!(names[0], "time_ns");
        assert_eq!(names[1], "type");
        for must in ["wing_sweep", "vy", "turn_rate", "turn_radius", "radio_altitude", "wx", "x", "y"] {
            assert!(names.contains(&must), "缺少列 {must}");
        }
    }

    /// `col::*` 常量必须与表头逐列对齐（写错下标正是最容易犯、又最难发现的错）。
    #[test]
    fn column_index_matches_header() {
        let names = column_names();
        let pairs: [(usize, &str); 30] = [
            (col::FRAME, "frame"),
            (col::X, "x"),
            (col::Y, "y"),
            (col::ALTITUDE, "altitude"),
            (col::RADIO_ALTITUDE, "radio_altitude"),
            (col::IAS, "ias"),
            (col::TAS, "tas"),
            (col::MACH, "mach"),
            (col::AOA, "aoa"),
            (col::AOS, "aos"),
            (col::NY, "ny"),
            (col::VY, "vy"),
            (col::WX, "wx"),
            (col::HEADING, "heading"),
            (col::ROLL, "roll"),
            (col::PITCH, "pitch"),
            (col::AILERON, "aileron"),
            (col::ELEVATOR, "elevator"),
            (col::RUDDER, "rudder"),
            (col::TRIMMER, "trimmer"),
            (col::FLAPS, "flaps"),
            (col::GEAR, "gear"),
            (col::AIRBRAKE, "airbrake"),
            (col::THROTTLE, "throttle"),
            (col::WING_SWEEP, "wing_sweep"),
            (col::TURN_RATE, "turn_rate"),
            (col::TURN_RADIUS, "turn_radius"),
            (col::SEP, "sep"),
            (col::ENERGY_HEIGHT, "energy_height"),
            (col::FUEL_KG, "fuel_kg"),
        ];
        for (idx, name) in pairs {
            assert_eq!(names[idx + 2], name, "col::{name} 指向了下标 {idx}，表头该位置是 {}", names[idx + 2]);
        }
    }

    /// 内存池：按初始容量预分配、写满自动扩容、文本就是容器里的 CSV。
    #[test]
    fn csv_pool_reserves_capacity_and_grows() {
        let mut pool = CsvPool::new(1024);
        assert!(pool.capacity() >= 1024, "初始容量必须按参数预分配：{}", pool.capacity());
        assert_eq!(pool.rows(), 0);
        assert_eq!(pool.text().lines().next().unwrap(), CSV_HEADER, "池子开头就是表头");
        for i in 0..200 {
            pool.push(&row(i, 0.5, 0.5));
        }
        let cap_after = pool.capacity();
        assert_eq!(pool.rows(), 200);
        assert!(pool.bytes() > 1024, "200 行早就超过 1 KiB 了");
        assert!(cap_after >= pool.bytes(), "容量必须跟得上占用");
        assert_eq!(pool.text().lines().count(), 201, "表头 + 200 行");
    }

    #[test]
    fn encode_decode_roundtrip() {
        let r = rec();
        let bytes = r.encode().expect("encode");
        assert_eq!(&bytes[0..4], b"WPR1");
        assert_eq!(u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]), 2, "容器版本 2");
        assert_eq!(u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]), 1, "组数 1");
        let back = Record::decode(&bytes).expect("decode");
        assert_eq!(back.groups, r.groups, "CSV 组逐字节一致");
        assert_eq!(back.groups[0].rows().unwrap(), r.groups[0].rows().unwrap(), "逐列一致");
        assert_eq!(back.groups[0].rows().unwrap()[0].time_ns, 1_700_000_000_000_000_000, "纳秒原样");
        assert_eq!(back.map_img, r.map_img, "底图字节一致");
        assert_eq!(back.meta.map, r.meta.map, "地图信息一致");
        assert_eq!((back.meta.groups, back.meta.frames), (1, 2), "元数据组数与玩家组帧数");
        assert_eq!(back.meta.aircraft, "f_16c", "机型回填进 meta（列表显示用）");
        assert_eq!(back.meta.group_info.len(), 1, "每组一行说明");
        assert!((back.duration_secs() - 0.1).abs() < 1e-9, "时长按纳秒差换算成秒");
    }

    /// 多组：组 0 = 玩家，其余组按组号顺序保存在容器里，元数据逐组登记机型/帧数。
    #[test]
    fn multi_group_roundtrip_keeps_order_and_info() {
        let mut r = rec();
        r.groups.push(group("su_27", &[(1_700_000_000_200_000_000, 0.4, 0.6)]));
        r.groups.push(group("a_10", &[
            (1_700_000_000_300_000_000, 0.45, 0.55),
            (1_700_000_000_400_000_000, 0.46, 0.54),
        ]));
        let bytes = r.encode().unwrap();
        assert_eq!(u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]), 3);

        let back = Record::decode(&bytes).unwrap();
        assert_eq!(back.groups.len(), 3);
        assert_eq!(back.player().unwrap().aircraft(), "f_16c", "组 0 是玩家");
        assert_eq!(back.groups[1].aircraft(), "su_27");
        assert_eq!(back.groups[2].frames(), 2);
        assert_eq!(back.meta.groups, 3, "元数据里也有组数");
        assert_eq!(back.meta.frames, 2, "frames = 玩家组帧数");
        let info: Vec<(String, u64)> =
            back.meta.group_info.iter().map(|g| (g.aircraft.clone(), g.frames)).collect();
        assert_eq!(
            info,
            vec![("f_16c".into(), 2), ("su_27".into(), 1), ("a_10".into(), 2)],
            "逐组登记机型与帧数"
        );
        // 只用首尾两行取时长（组 0）
        assert!((back.duration_secs() - 0.1).abs() < 1e-9);
    }

    /// 容器里每组的 CSV 必须与池子里的文本逐字节一致（同一处拼装，不做二次序列化）。
    #[test]
    fn embedded_csv_is_the_pool_text() {
        let r = rec();
        let bytes = r.encode().unwrap();
        let ml = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
        let mut off = HEADER_LEN + GROUP_LEN; // 1 组的长度表
        let csv_start = off + ml;
        let text = r.groups[0].csv.clone();
        assert_eq!(&bytes[csv_start..csv_start + text.len()], text.as_bytes());
        assert!(text.starts_with(CSV_HEADER));
        assert_eq!(text.lines().count(), 3, "表头 + 2 帧");
        off = csv_start;
        assert_eq!(off + text.len() + r.map_img.len(), bytes.len(), "meta | csv | img 三段排满");
    }

    #[test]
    fn row_text_keeps_precision_and_column_count() {
        let mut r = row(123, 0.75, 0.25);
        r.v[3] = 3200.5; // altitude → 2 位
        r.v[7] = 0.8704; // mach → 3 位
        r.v[24] = 0.5; // wing_sweep → 2 位
        let line = r.to_csv_line();
        assert_eq!(line.split(',').count(), NUM_COLS + 2, "列数固定");
        assert!(line.contains(",3200.50,"), "{line}");
        assert!(line.contains(",0.870,"), "mach 3 位：{line}");
        let back = RecordRow::from_csv_line(&line).unwrap();
        assert_eq!(back.time_ns, 123);
        assert_eq!(back.type_str, "f_16c");
        assert_eq!(back.v[0], 42.0, "frame");
        assert_eq!((back.v[1], back.v[2]), (0.75, 0.25), "坐标 7 位小数精确回来");
        assert_eq!(back.v[3], 3200.5, "高度 2 位");
        assert_eq!(back.v[7], 0.870, "mach 按 3 位小数回来（文本会四舍五入）");
        assert_eq!(back.v[24], 0.5, "wing_sweep 2 位");
    }

    /// NaN/inf 不能进写进容器的 CSV 文本（FlatCSV 导出原样吐出这份文本）。
    #[test]
    fn row_text_never_contains_nan_or_inf() {
        let mut r = row(1, 0.5, 0.5);
        r.v[3] = f64::NAN;
        r.v[4] = f64::INFINITY;
        r.v[5] = f64::NEG_INFINITY;
        let line = r.to_csv_line();
        assert!(!line.contains("NaN") && !line.contains("inf"), "{line}");
        let back = RecordRow::from_csv_line(&line).unwrap();
        assert_eq!((back.v[3], back.v[4], back.v[5]), (0.0, 0.0, 0.0), "非有限值归 0");
    }

    #[test]
    fn decode_rejects_garbage_truncation_and_bad_header() {
        assert!(Record::decode(b"short").is_err());
        assert!(Record::decode(b"XXXX\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00").is_err());
        let good = rec().encode().unwrap();
        assert!(Record::decode(&good[..good.len() - 3]).is_err(), "截断必须报错");
        assert!(Record::decode(&good[..HEADER_LEN + 2]).is_err(), "组数表都不全必须报错");
        let bad = String::from_utf8_lossy(&good).replace(CSV_HEADER, "wrong,header");
        assert!(Record::decode(bad.as_bytes()).is_err(), "表头不对必须报错，不能静默给空数据");
    }

    /// 别的版本号（旧格式 / 未来的格式）一律拒绝，不猜。
    #[test]
    fn decode_rejects_unknown_container_version() {
        let mut old = rec().encode().unwrap();
        old[4] = 1;
        assert!(Record::decode(&old).is_err());
        old[4] = 9;
        assert!(Record::decode(&old).is_err());
    }

    /// `ContainerHead`：**不解析 CSV** 就能拿到 meta 与底图（三段的偏移要跨过"组数表 + 各组 CSV"）。
    #[test]
    fn container_head_reads_meta_and_image_without_parsing_csv() {
        let r = rec();
        let bytes = r.encode().unwrap();
        let head = ContainerHead::decode(&bytes).unwrap();
        assert_eq!(head.meta.map.img_w, 1024);
        assert_eq!(head.img_len(), r.map_img.len());
        assert_eq!(head.image(&bytes), &r.map_img[..], "底图字节必须逐字节一致");

        // 没带底图的记录：长度为 0、切片为空（不是 panic）
        let mut empty = r.clone();
        empty.map_img = Vec::new();
        let eb = empty.encode().unwrap();
        let eh = ContainerHead::decode(&eb).unwrap();
        assert_eq!((eh.img_len(), eh.image(&eb).len()), (0, 0));

        // 错误路径：长度不够 / 头不对 / 版本不认识 / 长度字段撒谎（截断）都报错、不 panic
        assert!(ContainerHead::decode(&bytes[..HEADER_LEN - 1]).is_err());
        assert!(ContainerHead::decode(b"XXXX\x02\x00\x00\x00\x00\x00\x00\x00\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00").is_err());
        let mut bad_ver = bytes.clone();
        bad_ver[4] = 7;
        assert!(ContainerHead::decode(&bad_ver).unwrap_err().contains("版本 7"));
        assert!(ContainerHead::decode(&bytes[..bytes.len() - 1]).is_err(), "截断必须报错");
    }

    /// 组数为 0（头里或元数据里）都不接受：玩家那一组必须在。
    #[test]
    fn decode_rejects_zero_groups() {
        let mut zero = rec().encode().unwrap();
        zero[12..16].copy_from_slice(&0u32.to_le_bytes());
        assert!(Record::decode(&zero).unwrap_err().contains("组数为 0"));

        let mut r = rec();
        r.meta.groups = 3; // 元数据与头不一致
        let bytes = r.encode().unwrap(); // encode 会覆盖成真实组数
        let back = Record::decode(&bytes).unwrap();
        assert_eq!(back.meta.groups, 1);

        // 手工把头里的组数改成 2（长度表随之错位）→ 必须报错，不能读出垃圾
        let mut two = bytes.clone();
        two[12..16].copy_from_slice(&2u32.to_le_bytes());
        assert!(Record::decode(&two).is_err(), "组数与元数据不符必须报错");
    }

    #[test]
    fn encode_rejects_empty_groups() {
        let r = Record { meta: RecordMeta::default(), groups: vec![], map_img: vec![] };
        assert!(r.encode().unwrap_err().contains("至少要有"));
    }

    #[test]
    fn type_with_comma_does_not_break_columns() {
        let mut r = rec();
        let mut pool = CsvPool::new(0);
        let mut bad = row(1, 0.5, 0.5);
        bad.type_str = "F-16C,Block 50".to_string();
        pool.push(&bad);
        r.groups[0] = pool.into_group();
        let back = Record::decode(&r.encode().unwrap()).unwrap();
        let rows = back.groups[0].rows().unwrap();
        assert_eq!(rows[0].type_str, "F-16C_Block 50", "写入时把逗号替换掉");
        assert_eq!(rows.len(), 1, "列结构没被破坏");
    }
}
