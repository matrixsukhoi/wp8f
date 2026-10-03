//! 飞行记录线程：消费 `DisplayData` 环形缓冲区，收尾时写一个 `.wpr` 文件。
//!
//! 只做两件事：按 `poll_ms` 把最新一帧**原样**记成一行、stop 时把「地图信息 + 底图 + 各组 CSV」
//! 打包成 `.wpr` 落盘。不认识任何外部格式，也不做单位/符号换算。
//!
//! 采样口径：只取最新帧、不 drain 历史帧 → **记录频率 = 轮询频率**。`poll_ms` 由调用方给出，
//! core 传的是**地图刷新周期**（`period_ms(map_obj_record_every_frames, refresh_hz)`，
//! 默认 8 数据帧 @30 Hz = 266 ms）—— 地图坐标每这么多帧才更新一次，记更快只会写下重复坐标。
//!
//! 起始口径：`DisplayData.frame` > `RecordConfig::start_frame` 才开始记 —— 本机位置在地图对象
//! 采样到位前是初值 `(0.5, 0.5)`，不跳过的话录制开头会记成地图中心、回放时"闪现"到真实位置
//! （见 [`map_ready`]）。
//!
//! 存储口径：每帧只往 [`CsvPool`] 追加一行（按 `pool_mb` 预分配、写满自动扩容），
//! **记录期间不碰磁盘**；飞行结束才把各组内存池拼成一个 `.wpr`。
//!
//! 多机口径：组 0 恒为**玩家飞机**（core 只喂这一组）；其他飞机走"预留接口" ——
//! [`RecordHandle::register_group`] 领组号（>= 1）、[`RecordHandle::write_row`] 追加行。
//! 记录线程独占内存池，所以这两步走 channel：`write_row` 用 `try_send`，满了就丢并计入
//! `dropped_other`（记录绝不反过来拖慢调用方）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use wp8f_disp::{get_map_data, DisplayData, FrameReader};
use crate::wpr::{self, CsvPool, MapMeta, Record, RecordGroup, RecordMeta, RecordRow};

/// 统计行周期（秒）：到点就打印一行累计统计（只在这段时间确实写过东西时打）。
const STATS_EVERY_SECS: u64 = 5;

/// 其他飞机的指令队列长度（每条约 400 B：`RecordRow` 43 个 f64 + 机型；队列上限约 1.6 MiB）。
const CMD_QUEUE: usize = 4096;

/// 记录线程配置（core 从布局配置的 `record` 段搬过来）。
#[derive(Debug, Clone)]
pub struct RecordConfig {
    pub enabled: bool,
    /// 输出目录；空 = `./logs`
    pub output_dir: String,
    /// 轮询间隔（ms）。**记录频率 = 轮询频率**；0 会被钳到 1 ms。
    /// core 传的是地图刷新周期（见文件头的采样口径），本 crate 不读任何配置文件。
    pub poll_ms: u64,
    /// 每组 CSV 内存池的初始容量（MiB）；0 → 用 [`wpr::DEFAULT_POOL_MB`]。
    pub pool_mb: u32,
    /// **记录起点**：`DisplayData.frame` 超过这个帧号才开始记（判据见 [`map_ready`]）。
    ///
    /// core 传进来的是**配置里生效的地图记录间隔（数据帧）**
    /// （`HudLayoutConfig::map_obj_record_every_frames_clamped()` = `1..=refresh_hz`）——本机位置
    /// `pos_x/pos_y` 每帧都在写，但只有"地图对象被记录"的那几帧
    /// 是真实坐标，之前是初值 `(0.5, 0.5)`。0 = 不设门槛（第 1 帧就开记）。
    pub start_frame: u64,
}

impl Default for RecordConfig {
    fn default() -> Self {
        // start_frame = 0：**默认不设门槛**（logger 不认识任何配置默认值，门槛由 core 给；
        // 这里写 8 就会多出一份"抄来的"地图采样周期）。
        Self { enabled: false, output_dir: String::new(), poll_ms: 100, pool_mb: wpr::DEFAULT_POOL_MB, start_frame: 0 }
    }
}

impl RecordConfig {
    /// 内存池初始容量（字节）：0 取默认值；上限 4 GiB（手滑写个天文数字也不至于 OOM 申请）。
    fn pool_bytes(&self) -> usize {
        let mb = if self.pool_mb == 0 { wpr::DEFAULT_POOL_MB } else { self.pool_mb };
        mb.min(4096) as usize * 1024 * 1024
    }
}

/// 线程累计统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecordStats {
    /// 实际写进记录的行数（玩家组）
    pub recorded: u64,
    /// 被跳过的历史帧数（帧率高于记录频率时的正常现象）
    pub skipped: u64,
    /// 一轮轮询之间写入超过环形容量（32 帧）而丢帧的次数
    pub overwritten: u64,
    /// 检测到的重连次数（环形缓冲区代数前进）
    pub reset: u64,
    /// 地图信息就绪、本机位置还没采样到（初值 (0.5, 0.5) 不算）而放弃的采样次数
    pub waiting_sample: u64,
    /// 其他飞机（组 >= 1）写进记录的行数
    pub other_rows: u64,
    /// 其他飞机因队列满 / 组号不存在而被丢掉的行数
    pub dropped_other: u64,
}

/// 其他飞机（组 >= 1）的写入指令。`Add` 一定先于同组的 `Row`（同一个发送端，顺序有保证）。
#[derive(Debug)]
enum GroupCmd {
    /// 建组：`group` 是调用方从 [`Shared`] 领到的组号（>= 1）
    Add { group: u64, aircraft: String },
    /// 追加一行
    Row { group: u64, row: Box<RecordRow> },
}

#[derive(Debug, Default)]
struct Shared {
    stop: AtomicBool,
    recorded: AtomicU64,
    skipped: AtomicU64,
    overwritten: AtomicU64,
    reset: AtomicU64,
    waiting_sample: AtomicU64,
    other_rows: AtomicU64,
    dropped_other: AtomicU64,
    next_group: AtomicU64,
    poll_ms: AtomicU64,
}

impl Shared {
    fn stats(&self) -> RecordStats {
        RecordStats {
            recorded: self.recorded.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
            overwritten: self.overwritten.load(Ordering::Relaxed),
            reset: self.reset.load(Ordering::Relaxed),
            waiting_sample: self.waiting_sample.load(Ordering::Relaxed),
            other_rows: self.other_rows.load(Ordering::Relaxed),
            dropped_other: self.dropped_other.load(Ordering::Relaxed),
        }
    }
}

/// 记录线程句柄：`stop_and_write()`（或 Drop）停线程并把记录落盘。
#[derive(Debug)]
pub struct RecordHandle {
    shared: Arc<Shared>,
    tx: SyncSender<GroupCmd>,
    join: Option<JoinHandle<()>>,
}

impl RecordHandle {
    pub fn stats(&self) -> RecordStats {
        self.shared.stats()
    }

    /// 记录频率（Hz，= 1000 / poll_ms）。
    pub fn poll_hz(&self) -> f64 {
        let ms = self.shared.poll_ms.load(Ordering::Relaxed).max(1);
        1000.0 / ms as f64
    }

    /// **预留接口**：注册一组"其他飞机"，返回组号（恒 >= 1；组 0 是玩家，不可占用）。
    ///
    /// 之后用 [`Self::write_row`] 往这一组追加行；`aircraft` 只用于元数据里的说明
    /// （真正的机型以该组首行的 `type` 列为准）。
    pub fn register_group(&self, aircraft: &str) -> u64 {
        let group = self.shared.next_group.fetch_add(1, Ordering::Relaxed).max(1);
        // 队满（记录线程卡住/已退出）时这一步失败：组号照样发给调用方，
        // 后续 `write_row` 会如实返回 false 并计入 dropped_other，不会静默丢数据。
        let _ = self.tx.try_send(GroupCmd::Add { group, aircraft: aircraft.to_string() });
        group
    }

    /// 往某个"其他飞机"组追加一行（`try_send`：队满 / 线程已停 → false 并计入 `dropped_other`）。
    ///
    /// 组 0 由记录线程自己写（core 的 `DisplayData` 环形缓冲区），传 0 会被丢掉。
    pub fn write_row(&self, group: u64, row: RecordRow) -> bool {
        if group == 0 {
            self.shared.dropped_other.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        match self.tx.try_send(GroupCmd::Row { group, row: Box::new(row) }) {
            Ok(()) => {
                self.shared.other_rows.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.shared.dropped_other.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// 停线程并写盘（幂等：调用后 Drop 不会再写第二次）。
    pub fn stop_and_write(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

impl Drop for RecordHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 创建一次连接的记录线程 —— **core 侧唯一的飞行记录接口**。
///
/// `None` = 配置未启用（或窗口还没建起来、拿不到 `FrameReader`）。
pub fn spawn_recorder(cfg: RecordConfig, reader: FrameReader) -> Option<RecordHandle> {
    if !cfg.enabled {
        return None;
    }
    let poll_ms = cfg.poll_ms.max(1);
    let pool_bytes = cfg.pool_bytes();
    let shared = Arc::new(Shared::default());
    shared.poll_ms.store(poll_ms, Ordering::Relaxed);
    shared.next_group.store(1, Ordering::Relaxed); // 组 0 留给玩家
    println!(
        "[RECORD] poll={}ms (latest frame only: record rate = poll rate), pool={} MiB/group (group 0 = player), start_frame={} (map sampling period in main-loop frames)",
        poll_ms,
        pool_bytes / (1024 * 1024),
        cfg.start_frame
    );

    let (tx, rx) = mpsc::sync_channel(CMD_QUEUE);
    let shared_thread = Arc::clone(&shared);
    match thread::Builder::new().name("record".to_string()).spawn(move || {
        run(cfg, poll_ms, pool_bytes, reader, rx, shared_thread)
    }) {
        Ok(join) => Some(RecordHandle { shared, tx, join: Some(join) }),
        Err(e) => {
            eprintln!("[RECORD] cannot spawn record thread ({}): no recording this session", e);
            None
        }
    }
}

/// 线程主体：按 `poll_ms` 采样最新帧；退出时把内存池拼成 `.wpr`。
///
/// **起始条件**：地图没就绪（[`map_ready`] 为假）时一帧都不记 —— 那时的归一化坐标相对的是
/// 空地图，记下来也只是同一堆原点；等到就绪再开始，记下的第一帧正好是回放原点。
fn run(
    cfg: RecordConfig,
    poll_ms: u64,
    pool_bytes: usize,
    mut reader: FrameReader,
    rx: Receiver<GroupCmd>,
    sh: Arc<Shared>,
) {
    let created_ns = now_ns();
    let mut pools: Vec<CsvPool> = vec![CsvPool::new(pool_bytes)]; // 组 0 = 玩家
    let mut names: Vec<String> = vec![String::new()];
    let mut last_stats = Instant::now();
    let mut started = false;     // 是否已经写出第一行（用于"开始记录"那一行日志）
    let mut wait_logged = false; // 同一段等待只打一次（统计行负责每 5 s 报进度）

    loop {
        thread::sleep(Duration::from_millis(poll_ms));
        if sh.stop.load(Ordering::Acquire) {
            break;
        }
        let out = reader.poll_latest();
        sh.skipped.fetch_add(out.skipped, Ordering::Relaxed);
        if out.overwritten {
            sh.overwritten.fetch_add(1, Ordering::Relaxed);
        }
        if out.reset {
            sh.reset.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(d) = out.frame {
            if map_ready(&d, cfg.start_frame) {
                if !started {
                    started = true;
                    let waited = sh.waiting_sample.load(Ordering::Relaxed);
                    if waited > 0 {
                        println!(
                            "[RECORD] first map sample passed (frame {}/{} after {} held-off polls): recording starts",
                            d.frame, cfg.start_frame, waited
                        );
                    }
                }
                pools[0].push(&row_of(d, created_ns));
                sh.recorded.fetch_add(1, Ordering::Relaxed);
            } else {
                // 还没过首个地图采样周期：不记、也不推进时间轴，否则开头几帧是初值 (0.5,0.5)。
                sh.waiting_sample.fetch_add(1, Ordering::Relaxed);
                if !wait_logged {
                    wait_logged = true;
                    println!(
                        "[RECORD] waiting for the first map sample (frame {}/{}): recording is held off",
                        d.frame, cfg.start_frame
                    );
                }
            }
        }
        // 其他飞机：把队列里的指令落到池子上（不阻塞、不分配大对象）
        drain_cmds(&rx, &mut pools, &mut names, pool_bytes);
        // 统计行：每 ~5 s 一行；写过东西就报行数，一直在等位置采样就报等待进度（不刷屏、也不静默）
        if last_stats.elapsed() >= Duration::from_secs(STATS_EVERY_SECS) {
            last_stats = Instant::now();
            let s = sh.stats();
            if s.recorded > 0 || s.other_rows > 0 {
                println!(
                    "[RECORD] poll={}ms groups={} rows={} skipped={} overwritten={} reset={} waiting_sample={} other_rows={} dropped_other={}",
                    poll_ms,
                    pools.len(),
                    s.recorded,
                    s.skipped,
                    s.overwritten,
                    s.reset,
                    s.waiting_sample,
                    s.other_rows,
                    s.dropped_other
                );
            } else if s.waiting_sample > 0 {
                println!(
                    "[RECORD] still waiting for the first player position sample: {} polls held off ({}s since thread start)",
                    s.waiting_sample,
                    now_ns().saturating_sub(created_ns) / 1_000_000_000
                );
            }
        }
    }

    write_record(&cfg, created_ns, poll_ms, pools, names, &sh);
}

/// 把队列里的指令落到池子上。
///
/// `Add` 会把 `pools` / `names` 补齐到该组号（组号由计数器连续发放，中间不会留洞）；
/// 组 0 与"没注册过的组号"的行直接丢掉（调用方在 `write_row` 侧已计入 `dropped_other`）。
fn drain_cmds(rx: &Receiver<GroupCmd>, pools: &mut Vec<CsvPool>, names: &mut Vec<String>, pool_bytes: usize) {
    while let Ok(cmd) = rx.try_recv() {
        match cmd {
            GroupCmd::Add { group, aircraft } => {
                let idx = group as usize;
                if idx == 0 {
                    continue; // 组 0 是玩家，注册请求无效
                }
                while pools.len() <= idx {
                    pools.push(CsvPool::new(pool_bytes));
                    names.push(String::new());
                }
                names[idx] = aircraft;
            }
            GroupCmd::Row { group, row } => {
                // 组 0 只由记录线程从 `DisplayData` 环形缓冲区写（`write_row` 也拒收 0）
                if group == 0 {
                    continue;
                }
                if let Some(p) = pools.get_mut(group as usize) {
                    p.push(&row);
                }
            }
        }
    }
}

/// 收尾：抓最新地图（信息 + 底图字节），把各组内存池拼成 `.wpr` 并落盘。
fn write_record(
    cfg: &RecordConfig,
    created_ns: u64,
    poll_ms: u64,
    pools: Vec<CsvPool>,
    names: Vec<String>,
    sh: &Shared,
) {
    let s = sh.stats();
    if !pools.iter().any(|p| p.rows() > 0) {
        // 一帧都没记到（典型：整场游戏都没给 /map_info.json，记录线程一直在门槛外等）：
        // 不写空文件，也不把它报成"写盘失败"——把原因和等待计数说清楚就够了。
        println!(
            "[RECORD] nothing recorded ({} polls held off waiting for map init, other_rows={}): no file written",
            s.waiting_sample, s.other_rows
        );
        return;
    }
    let map = get_map_data();
    let img = wp8f_disp::map_image();
    let (map_meta, map_img) = match map {
        Some(m) if img.w > 0 && img.h > 0 => (
            MapMeta {
                width_m: m.map_maxsize[0].max(0.0),
                height_m: m.map_maxsize[1].max(0.0),
                // 地图 0 点（归一化 0,0 = 西北角）之外，还要世界坐标：换算全靠这两个数
                min_x_m: m.map_min[0],
                min_y_m: m.map_min[1],
                grid_zero: m.grid_zero,
                grid_steps: m.grid_steps,
                img_w: img.w,
                img_h: img.h,
                img_mime: image_mime(&img.raw).to_string(),
            },
            img.raw.as_ref().clone(),
        ),
        // 没有地图（还没收到 map_info / 地图下载失败）：记录仍然有效，只是回放没有底图
        _ => (MapMeta::default(), Vec::new()),
    };

    let meta = RecordMeta {
        container: wpr::VERSION,
        created_ms: created_ns / 1_000_000,
        frames: 0, // encode() 按组 0 回填
        poll_hz: 1000.0 / poll_ms as f64,
        map: map_meta,
        generator: "wp8f".to_string(),
        aircraft: names.first().cloned().unwrap_or_default(), // encode() 会用首帧机型回填
        ..RecordMeta::default()
    };
    // 注册了组却一行都没写进来（比如数据源中途失败）：不写空组，免得回放里多一个空对象
    let groups: Vec<RecordGroup> = pools
        .into_iter()
        .filter(|p| p.rows() > 0)
        .map(|p| p.into_group())
        .collect();
    let rec = Record { meta, groups, map_img };
    if rec.meta.map.img_w == 0 {
        // 记录照写（地图信息就绪就够了），但明确说清回放里不会有底图
        println!("[RECORD] no map image this session: replay will have no background");
    } else if rec.map_img.is_empty() {
        // 元数据说有底图（游戏给了 2048×2048）但字节是空的：这是**上游漏传**的信号
        // （core 漏写 `MapDisplay::map_img_raw` 时写出来的记录 img_len=0）
        // —— 不能静默：回放那边只会显示"记录没带底图"，看不出是记录端的问题。
        eprintln!(
            "[RECORD] map image metadata is present ({}x{}) but the raw bytes are empty: \
             this record will have no background (upstream did not publish map_img_raw)",
            rec.meta.map.img_w, rec.meta.map.img_h
        );
    }
    let dir = if cfg.output_dir.trim().is_empty() { "./logs".to_string() } else { cfg.output_dir.clone() };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("[RECORD] cannot create dir {}: {}", dir, e);
        return;
    }
    // 文件名沿用"UNIX 毫秒"（人眼可排序、与旧文件同风格）；记录内部的时间列是纳秒
    let path = PathBuf::from(&dir).join(format!("wp8f_{}.{}", created_ns / 1_000_000, wpr::EXT));
    match rec.encode().and_then(|bytes| {
        std::fs::write(&path, &bytes).map_err(|e| e.to_string()).map(|_| bytes.len())
    }) {
        Ok(size) => println!(
            "[RECORD] written: {} (groups={} frames={} size={} KiB skipped={} overwritten={} reset={} waiting_sample={} other_rows={} dropped_other={})",
            path.display(),
            rec.meta.groups,
            rec.meta.frames,
            size / 1024,
            s.skipped,
            s.overwritten,
            s.reset,
            s.waiting_sample,
            s.other_rows,
            s.dropped_other
        ),
        Err(e) => eprintln!("[RECORD] write failed ({}): {}", path.display(), e),
    }
}

/// 记录起始条件：**首个地图记录周期已过去**。
///
/// core 的地图记录闸门是 `frame_count >= next_mapobj_frame`（初值 = 生效的地图记录间隔），
/// 而同一轮主循环里写 `DisplayData` 在写本机位置**之前** → `DisplayData` 要到下一帧才带上
/// 真实位置。所以判据是 `frame > start_frame` 而不是 `>=`：取 `>=` 会把开头的初值
/// `(0.5, 0.5)` 记进去，回放一开头就"闪现"到真实位置。
///
/// 线程启动晚（首个拉到的最新帧已远大于阈值）时直接开记，没有额外分支。
fn map_ready(d: &DisplayData, start_frame: u64) -> bool {
    d.frame > start_frame
}

/// 一帧 `DisplayData` → 一行记录：**逐字段原样拷贝**，不换算、不改符号、不认识任何外部格式。
fn row_of(d: &DisplayData, fallback_ns: u64) -> RecordRow {
    // `DisplayData.timestamp` 是纳秒（core 更新器每帧写入，与主循环节拍同基准）；
    // 老路径可能仍是 0 → 退回线程启动时刻，保证时间轴单调、不出现 0。
    let t = if d.timestamp > 0 { d.timestamp } else { fallback_ns };
    let v = [
        d.frame as f64,
        d.pos_x, d.pos_y,
        d.altitude, d.radio_altitude,
        d.ias, d.tas,
        d.mach,
        d.aoa, d.aos,
        d.ny, d.vy,
        d.wx,
        d.heading, d.roll, d.pitch,
        d.aileron, d.elevator, d.rudder,
        d.trimmer,
        d.flaps, d.gear, d.airbrake,
        d.throttle,
        d.wing_sweep,
        d.turn_rate, d.turn_radius,
        d.sep, d.energy_height,
        d.thrust_to_weight,
        d.total_thrust, d.total_hp, d.thrust_percent,
        d.total_drag,
        d.manifold_pressure,
        d.fuel_kg, d.fuel_percent, d.fuel1_kg,
        d.overspeed_warning as f64,
        d.mach_warning as f64,
        d.g_load_warning as f64,
        if d.voice_alarm { 1.0 } else { 0.0 },
        if d.brake_caution { 1.0 } else { 0.0 },
    ];
    RecordRow {
        time_ns: t,
        type_str: String::from_utf8_lossy(d.aircraft_type.as_bytes())
            .trim_end_matches('\0')
            .to_string(),
        v,
    }
}

/// 底图 MIME：按魔数判断（游戏给什么就是什么，通常是 JPEG）。
fn image_mime(bytes: &[u8]) -> &'static str {
    match bytes {
        [0xFF, 0xD8, ..] => "image/jpeg",
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'G', b'I', b'F', ..] => "image/gif",
        [b'B', b'M', ..] => "image/bmp",
        _ => "application/octet-stream",
    }
}

/// 当前 UNIX 纳秒（logger 不反向依赖 core，本地实现同一刻度）。
fn now_ns() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_sniffing_covers_common_map_images() {
        assert_eq!(image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), "image/jpeg");
        assert_eq!(image_mime(b"\x89PNG\r\n\x1a\n"), "image/png");
        assert_eq!(image_mime(b"GIF89a"), "image/gif");
        assert_eq!(image_mime(b"BM....."), "image/bmp");
        assert_eq!(image_mime(b"????"), "application/octet-stream");
        assert_eq!(image_mime(&[]), "application/octet-stream");
    }

    /// 行映射是**逐字段原样拷贝**：不换算、不改符号。
    #[test]
    fn row_keeps_raw_display_semantics() {
        let mut d = DisplayData::default();
        d.aircraft_type.assign(b"f_16c");
        d.timestamp = 1_700_000_000_123_000_000;
        d.pos_x = 0.75;
        d.pos_y = 0.25;
        d.altitude = 3200.0;
        d.ias = 540.0;
        d.tas = 612.0;
        d.mach = 0.87;
        d.aoa = 5.2;
        d.aos = -1.1;
        d.ny = 3.2;
        d.heading = 271.5;
        d.roll = 10.3;
        d.pitch = -2.5;
        d.throttle = 1.05;
        d.airbrake = 50.0;
        d.flaps = 50.0;
        d.gear = 100.0;
        d.fuel_kg = 3200.5;

        let r = row_of(&d, 0);
        assert_eq!(r.time_ns, 1_700_000_000_123_000_000, "DisplayData.timestamp（纳秒）原样");
        assert_eq!(r.type_str, "f_16c");
        assert_eq!(r.v.len(), crate::wpr::NUM_COLS, "数值列数 = 表头约定");
        assert_eq!((r.v[0], r.v[1], r.v[2]), (0.0, 0.75, 0.25), "frame/x/y 原样");
        assert_eq!(r.v[5], 540.0, "ias 原样（km/h）");
        assert_eq!(r.v[15], -2.5, "pitch 原样（HUD 口径，不取负）");
        assert_eq!(r.v[22], 50.0, "airbrake 原样（百分比）");
        assert_eq!(r.v[35], 3200.5, "fuel_kg 原样");
    }

    /// timestamp 为 0（老路径/未赋值）时退回调用方给的时刻，不会写出 0 时间。
    #[test]
    fn row_falls_back_when_timestamp_missing() {
        let d = DisplayData::default();
        let r = row_of(&d, 1_700_000_000_000_000_000);
        assert!(r.time_ns >= 1_700_000_000_000_000_000, "缺 timestamp 时不能写 0：{}", r.time_ns);
    }

    /// 地图没就绪就别开始记：默认 `DisplayData`（位置还没采样）必须被判为"未就绪"。
    /// 起始条件：首个地图采样周期内一律不记，过后才记；线程启动晚（首个最新帧早已超过
    /// 阈值）时直接记。
    ///
    /// 阈值用 **2**（每 2 帧采样一次）而不是默认常量：断言的是"core 传进来的 `start_frame`
    /// 就是那个阈值"，写死 8 的实现会在这里红。语义 = 第 2 帧才发生首次真实采样、
    /// `DisplayData` 到第 3 帧才带上它，**门槛只卡开头**：过了门槛就按记录频率一直记
    /// （采样周期只影响"位置多久变一次"，不影响记录行数）。
    #[test]
    fn recording_waits_for_the_first_map_sample() {
        const EVERY: u64 = 2;
        let mut d = DisplayData::default();
        for f in 0..=EVERY {
            d.frame = f;
            assert!(!map_ready(&d, EVERY), "frame {f} 还在首个采样周期内（位置可能还是初值）→ 不该记");
        }
        d.frame = EVERY + 1;
        assert!(map_ready(&d, EVERY), "采样周期的下一帧起（实测首个真实坐标就在这一帧）可以记录");
        d.frame = 10_000;
        assert!(map_ready(&d, EVERY), "线程启动晚：frame 已远超阈值 → 直接记，无需额外分支");

        // "每 2 帧采样一次"的完整语义：首个真实采样在第 2 帧、第 3 帧起才可记；之后
        // 中间帧（位置与前一帧相同）照记 —— 记录频率仍只由 poll_ms 决定。
        let recorded: Vec<u64> = (0..=8).filter(|f| { d.frame = *f; map_ready(&d, EVERY) }).collect();
        assert_eq!(recorded, vec![3, 4, 5, 6, 7, 8], "阈值为 2 时从第 3 帧起一直记");

        // start_frame = 0 是"不设门槛"（配置没给采样周期时的合法取值）：第 1 帧就开记
        d.frame = 0;
        assert!(!map_ready(&d, 0), "第 0 帧还没有任何采样");
        d.frame = 1;
        assert!(map_ready(&d, 0), "无门槛时第 1 帧即记");
    }

    /// 起始阈值来自配置：同一个帧号在不同 `start_frame` 下判定相反（防止"判据又写死常量"）。
    #[test]
    fn start_frame_comes_from_the_config() {
        let mut d = DisplayData::default();
        d.frame = 9;
        assert!(map_ready(&d, 8), "历史默认 8 帧：第 9 帧已过门槛");
        assert!(!map_ready(&d, 16), "配置改成 16 帧：第 9 帧还在门槛内");
        assert!(map_ready(&d, 1), "配置改成 1 帧（每帧采样）：第 9 帧早就该记");
    }

    /// 等待计数要能被统计读到（core/GUI 侧据此看出"一直在等位置采样"而不是"没在记"）。
    #[test]
    fn waiting_sample_shows_up_in_stats() {
        let sh = Shared::default();
        sh.waiting_sample.fetch_add(3, Ordering::Relaxed);
        assert_eq!(sh.stats().waiting_sample, 3);
        assert_eq!(sh.stats().recorded, 0);
    }

    /// 内存池初始容量：默认 8 MiB、写 0 取默认、写得离谱也不会要一块天文数字。
    #[test]
    fn pool_bytes_defaults_and_clamps() {
        assert_eq!(RecordConfig::default().pool_bytes(), 8 * 1024 * 1024, "默认 8 MiB");
        assert_eq!(RecordConfig { pool_mb: 0, ..Default::default() }.pool_bytes(), 8 * 1024 * 1024);
        assert_eq!(RecordConfig { pool_mb: 32, ..Default::default() }.pool_bytes(), 32 * 1024 * 1024);
        assert_eq!(
            RecordConfig { pool_mb: u32::MAX, ..Default::default() }.pool_bytes(),
            4096 * 1024 * 1024,
            "上限 4 GiB"
        );
    }

    /// 预留接口的落地路径：注册组 → 池子补齐到该组号 → 行进对应组；组 0 的行走不进池子。
    ///
    /// ⚠️ 这里必须用 `try_send`：消息数多于队列容量时 `send` 会永久阻塞（测试进程挂死）。
    /// 凡是"往有界队列灌数据"的测试都用非阻塞发送 + 明确的失败信息。
    #[test]
    fn drain_cmds_builds_groups_and_routes_rows() {
        let (tx, rx) = mpsc::sync_channel(2); // 故意比消息数小：验证非阻塞发送
        let mk = |t: u64| RecordRow { time_ns: t, ..Default::default() };
        let cmds = vec![
            GroupCmd::Add { group: 1, aircraft: "su_27".into() },
            GroupCmd::Row { group: 1, row: Box::new(mk(10)) },
            GroupCmd::Row { group: 1, row: Box::new(mk(20)) },
            GroupCmd::Add { group: 3, aircraft: "a_10".into() }, // 跳过 2 → 补齐空组
            GroupCmd::Row { group: 3, row: Box::new(mk(30)) },
            GroupCmd::Row { group: 0, row: Box::new(mk(40)) },   // 玩家组：无效，忽略
        ];
        let mut pools = vec![CsvPool::new(0)];
        let mut names = vec![String::new()];
        // 边送边收：队列只有 2 格，靠 drain 腾地方；一旦卡住就是失败（try_send 返回 Err）
        for cmd in cmds {
            let mut pending = Some(cmd);
            for _ in 0..1000 {
                match tx.try_send(pending.take().expect("待发命令")) {
                    Ok(()) => break,
                    Err(TrySendError::Full(c)) => pending = Some(c),
                    Err(e) => panic!("测试队列异常：{e}"),
                }
                drain_cmds(&rx, &mut pools, &mut names, 0);
            }
            assert!(pending.is_none(), "队列一直满：drain 没腾出位置（非阻塞发送的失败路径）");
            drain_cmds(&rx, &mut pools, &mut names, 0);
        }
        drop(tx);
        drain_cmds(&rx, &mut pools, &mut names, 0);

        assert_eq!(pools.len(), 4, "注册到组 3 就把池子补到 4 个（含中间的空组 2）");
        assert_eq!(pools[0].rows(), 0, "组 0 只由记录线程自己写");
        assert_eq!(pools[1].rows(), 2, "组 1 收到 2 行");
        assert_eq!(pools[2].rows(), 0);
        assert_eq!(pools[3].rows(), 1, "组 3 收到 1 行");
        assert_eq!(names[1], "su_27");
        assert_eq!(names[3], "a_10");
        assert_eq!(pools[1].text().lines().count(), 3, "表头 + 2 行");
    }

    /// `write_row` 的边界：组 0 与已停止的线程都返回 false 并计入 `dropped_other`。
    #[test]
    fn write_row_rejects_player_group_and_counts_drops() {
        let sh = Arc::new(Shared::default());
        sh.next_group.store(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel(4);
        let h = RecordHandle { shared: Arc::clone(&sh), tx, join: None };
        assert!(!h.write_row(0, RecordRow::default()), "组 0 是玩家的，不接受外部写入");
        assert_eq!(sh.stats().dropped_other, 1);
        let g = h.register_group("su_27");
        assert!(g >= 1, "其他飞机的组号从 1 起：{g}");
        assert!(h.write_row(g, RecordRow::default()));
        assert_eq!(sh.stats().other_rows, 1);
        drop(rx); // 接收端没了（线程已退出）→ try_send 失败也要计入丢弃
        assert!(!h.write_row(g, RecordRow::default()));
        assert_eq!(sh.stats().dropped_other, 2);
    }
}
