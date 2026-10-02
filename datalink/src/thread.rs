//! Link-8 数据链线程：**core 只调用 `spawn_link()` 一次**，之后主循环里再没有任何
//! socket 句柄、收发点或计时器。
//!
//! 线程做三件事（全部在独立线程里，主循环不受影响）：
//!
//! 1. **定期轮询上报**：每 `send_hz` 一次，从本机状态槽（`link_ring::OwnState`）取**最新**一版，
//!    拼成 `StatePacket` 用 UDP 发出（丢包/断连自动恢复，协议本身无状态）。
//! 2. **排空收包**：每个轮询节拍（`TICK_MS`）都把接收队列排空一次，只保留**最新一份全量应答**
//!    （低延迟：收包不必等到下一个上报周期）。
//! 3. **写友军槽**：把新快照 `publish` 进 `link_ring`，HUD 绘制线程自己取最新消费 ——
//!    三个线程共享的是无锁环形槽，彼此不等。
//!
//! 与 logger 同源：都是"消费端只取最新"，只是这里每轮发布的是一份全量快照而不是一帧。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::client::DataLinkClient;
use crate::link_ring::{self, Cursor, OwnState};
use crate::StatePacket;

/// 线程轮询节拍（ms）：**收**包每拍排空（低延迟），**发**包按 `send_hz`。
const TICK_MS: u64 = 50;
/// 统计行周期（拍）：100 × 50 ms = 5 s（与 logger 的"定期一行、不刷屏"同口径）
const STATS_EVERY_TICKS: u64 = 100;
/// `send_hz` 钳制范围（与 `DataLinkConfig::send_hz` 的 1..=30 同口径）
const HZ_MIN: u32 = 1;
const HZ_MAX: u32 = 30;

/// 数据链线程配置（core 从布局配置的 `datalink` 段搬过来）。
#[derive(Debug, Clone)]
pub struct LinkConfig {
    pub enabled: bool,
    pub server: String,
    pub port: u16,
    pub key: String,
    pub session_id: u8,
    /// 上报频率（Hz，钳到 1..=30）
    pub send_hz: u32,
    /// 友军快照超时（秒）：这么久没收到服务端应答就清空一次
    /// （`0` = 关闭超时清空，一直保留最后一份快照）
    pub peer_timeout_secs: u32,
}

/// 线程累计统计（HUD/工具可读；日志里每 5 s 打一行同样的数）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkStats {
    pub sent: u64,
    pub replies: u64,
    pub send_errors: u64,
    /// 上报时被跳过的本机状态版本数（主循环帧率高于 `send_hz` 时的正常现象）
    pub skipped: u64,
    /// 友军快照槽被重置的次数（重连）
    pub resets: u64,
    /// 因超时清空友军的次数
    pub expired: u64,
    /// 最新一份应答里的友军条数
    pub peers: usize,
}

#[derive(Debug)]
struct Shared {
    stop: AtomicBool,
    sent: AtomicU64,
    replies: AtomicU64,
    send_errors: AtomicU64,
    skipped: AtomicU64,
    resets: AtomicU64,
    expired: AtomicU64,
    peers: AtomicU64,
    send_hz: AtomicU64,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            stop: AtomicBool::new(false),
            sent: AtomicU64::new(0),
            replies: AtomicU64::new(0),
            send_errors: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            resets: AtomicU64::new(0),
            expired: AtomicU64::new(0),
            peers: AtomicU64::new(0),
            send_hz: AtomicU64::new(0),
        }
    }
}

impl Shared {
    fn stats(&self) -> LinkStats {
        LinkStats {
            sent: self.sent.load(Ordering::Relaxed),
            replies: self.replies.load(Ordering::Relaxed),
            send_errors: self.send_errors.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
            resets: self.resets.load(Ordering::Relaxed),
            expired: self.expired.load(Ordering::Relaxed),
            peers: self.peers.load(Ordering::Relaxed) as usize,
        }
    }
}

/// 数据链线程句柄：`stop()`（或 Drop）停线程；线程内只 sleep 小节拍，收尾很快。
#[derive(Debug)]
pub struct LinkHandle {
    shared: Arc<Shared>,
    join: Option<JoinHandle<()>>,
}

impl LinkHandle {
    /// 当前累计统计。
    pub fn stats(&self) -> LinkStats {
        self.shared.stats()
    }

    /// 上报频率（Hz，钳制后的实际值）。
    pub fn send_hz(&self) -> u32 {
        self.shared.send_hz.load(Ordering::Relaxed) as u32
    }

    /// 停线程并等待收尾（幂等：`stop()` 之后 Drop 不会重复 join）。
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

impl Drop for LinkHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// `send_hz` 钳制（0 → 1 Hz，>30 → 30 Hz），与 `DataLinkConfig::send_hz` 同口径。
fn clamp_hz(hz: u32) -> u32 {
    hz.clamp(HZ_MIN, HZ_MAX)
}

/// 创建一次进程级的数据链线程。`None` = 配置没启用，或建 socket 失败
/// （失败只打一行日志，程序继续跑，只是没有数据链）。
pub fn spawn_link(cfg: LinkConfig) -> Option<LinkHandle> {
    if !cfg.enabled {
        return None;
    }
    let hz = clamp_hz(cfg.send_hz);
    let addr = format!("{}:{}", cfg.server, cfg.port);
    let client = match DataLinkClient::new(&addr, &cfg.key, cfg.session_id) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[DATALINK] init failed ({}), continuing without datalink", e);
            return None;
        }
    };
    eprintln!(
        "[DATALINK] enabled: server={} client_id={} session={} send_hz={} peer_timeout={}s",
        addr,
        client.client_id(),
        client.session_id(),
        hz,
        cfg.peer_timeout_secs
    );

    link_ring::init_slots();
    let shared = Arc::new(Shared::default());
    shared.send_hz.store(hz as u64, Ordering::Relaxed);
    let period = Duration::from_millis((1000 / hz as u64).max(1));
    let ttl = Duration::from_secs(cfg.peer_timeout_secs as u64);
    let sh = Arc::clone(&shared);
    match thread::Builder::new().name("datalink".to_string()).spawn(move || {
        run(client, period, ttl, sh)
    }) {
        Ok(join) => Some(LinkHandle { shared, join: Some(join) }),
        Err(e) => {
            eprintln!(
                "[DATALINK] cannot spawn datalink thread ({}): no datalink for this connection",
                e
            );
            None
        }
    }
}

/// 友军快照的新鲜度看门狗：多久没收到应答就清空一次（`ttl.is_zero()` = 关闭）。
///
/// 抽成纯状态机是为了能确定性地测"什么时候清、清了之后什么时候再清"，
/// 不用 sleep 真的等 5 秒（线程每拍调 `should_clear()`）。
#[derive(Debug, Default)]
struct PeerWatch {
    last_reply: Option<Instant>,
    /// 已经因超时清空过（避免每拍重复发布空快照/刷屏）
    expired: bool,
}

impl PeerWatch {
    /// 收到一份应答：刷新时间戳，允许下一次超时。
    fn on_reply(&mut self, now: Instant) {
        self.last_reply = Some(now);
        self.expired = false;
    }

    /// 现在该不该发布一份空快照？同一轮超时只回答一次 `true`。
    /// 从来没收到过应答时不回答（本来就没东西可清）。
    fn should_clear(&mut self, now: Instant, ttl: Duration) -> bool {
        if ttl.is_zero() || self.expired {
            return false;
        }
        match self.last_reply {
            Some(t) if now.saturating_duration_since(t) >= ttl => {
                self.expired = true;
                true
            }
            _ => false,
        }
    }
}

/// 线程主体：固定 `TICK_MS` 节拍；收包每拍排空，发包按 `period`，
/// 友军快照超过 `ttl` 没更新就清空一次（`ttl` 为 0 = 不清空）。
fn run(
    mut client: DataLinkClient,
    period: Duration,
    ttl: Duration,
    sh: Arc<Shared>,
) {
    let hz = sh.send_hz.load(Ordering::Relaxed);
    let mut cursor = Cursor::default();
    let mut last_own: Option<&'static OwnState> = None;
    // 第一拍就发一帧（别等服务端等一个周期）：last_send 往回拨一个周期
    let mut last_send = Instant::now() - period;
    let mut ticks: u64 = 0;
    // 重连（core 的 `reset_tracks()`）会让代数前进：线程据此统计换局次数
    let mut tracks_gen = link_ring::tracks_generation();
    let mut watch = PeerWatch::default();
    // 线程启动 = 从"没有友军"开始（避免上一局/上一次线程留下的快照挂在屏幕上）
    link_ring::publish_tracks(&[]);

    loop {
        if sh.stop.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(TICK_MS));
        ticks += 1;

        // 1) 收：每拍排空，只留最新一份全量应答
        if let Some(list) = client.recv_latest() {
            sh.peers.store(list.len() as u64, Ordering::Relaxed);
            sh.replies.fetch_add(1, Ordering::Relaxed);
            watch.on_reply(Instant::now());
            link_ring::publish_tracks(&list);
        } else if watch.should_clear(Instant::now(), ttl) {
            // 服务端（或网络）没动静了：清空友军，避免旧标记一直挂在 HUD 上
            let stale = sh.peers.load(Ordering::Relaxed) as usize;
            sh.expired.fetch_add(1, Ordering::Relaxed);
            sh.peers.store(0, Ordering::Relaxed);
            link_ring::publish_tracks(&[]);
            if stale > 0 {
                println!(
                    "[DATALINK] peers expired: no reply for {}s, cleared {} peers",
                    ttl.as_secs(),
                    stale
                );
            }
        }
        let gen = link_ring::tracks_generation();
        if gen != tracks_gen {
            tracks_gen = gen;
            sh.resets.fetch_add(1, Ordering::Relaxed);
        }

        // 2) 发：按 send_hz 取本机状态槽的最新一版
        if last_send.elapsed() >= period {
            last_send = Instant::now();
            let out = link_ring::poll_own(&mut cursor);
            sh.skipped.fetch_add(out.skipped, Ordering::Relaxed);
            if let Some(o) = out.value {
                last_own = Some(o);
            }
            if let Some(o) = last_own {
                let mut st = StatePacket::new(client.client_id());
                st.aircraft_type = o.aircraft_type;
                st.ias = o.ias;
                st.tas = o.tas;
                st.altitude = o.altitude;
                st.mach = o.mach;
                st.heading = o.heading;
                st.vy = o.vy;
                st.x = o.x;
                st.y = o.y;
                st.poi_enabled = if o.poi_enabled { 1 } else { 0 };
                st.poi_x = o.poi_x;
                st.poi_y = o.poi_y;
                match client.send_state(st) {
                    Ok(_) => {
                        sh.sent.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        sh.send_errors.fetch_add(1, Ordering::Relaxed);
                        eprintln!("[DATALINK] send error: {}", e);
                    }
                }
            }
        }

        // 3) 统计：每 5 s 一行，且只在这个窗口确实有活动时打（不刷屏）
        if ticks % STATS_EVERY_TICKS == 0 {
            let s = sh.stats();
            if s.sent > 0 || s.replies > 0 {
                println!(
                    "[DATALINK] send_hz={} sent={} replies={} peers={} skipped={} reset={} expired={}",
                    hz, s.sent, s.replies, s.peers, s.skipped, s.resets, s.expired
                );
            }
        }
    }

    let s = sh.stats();
    println!(
        "[DATALINK] stopped: send_hz={} sent={} replies={} peers={} errors={} expired={}",
        hz, s.sent, s.replies, s.peers, s.send_errors, s.expired
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(enabled: bool, server: &str) -> LinkConfig {
        LinkConfig {
            enabled,
            server: server.to_string(),
            port: 2887,
            key: "k".to_string(),
            session_id: 1,
            send_hz: 4,
            peer_timeout_secs: crate::PEER_TIMEOUT_SECS,
        }
    }

    #[test]
    fn disabled_config_creates_no_thread() {
        assert!(spawn_link(cfg(false, "127.0.0.1")).is_none(), "没启用就不该建线程");
    }

    #[test]
    fn bad_address_returns_none_instead_of_panicking() {
        // 端口越界 → DataLinkClient::new 报错 → 只打一行日志并返回 None
        assert!(spawn_link(cfg(true, "1.2.3.4:99999")).is_none());
    }

    #[test]
    fn send_hz_is_clamped_like_the_config() {
        assert_eq!(clamp_hz(0), HZ_MIN, "0 Hz 会被钳到 1（否则周期除零）");
        assert_eq!(clamp_hz(1), 1);
        assert_eq!(clamp_hz(4), 4);
        assert_eq!(clamp_hz(30), 30);
        assert_eq!(clamp_hz(999), HZ_MAX, "上限与 DataLinkConfig 的 1..=30 一致");
    }

    #[test]
    fn period_never_zero_for_any_allowed_hz() {
        // 线程用 1000/hz 毫秒算周期：钳制后永远 ≥ 1 ms，不会出现"忙等"
        for hz in 0..=1000u32 {
            let period = Duration::from_millis((1000 / clamp_hz(hz) as u64).max(1));
            assert!(period >= Duration::from_millis(1), "hz={hz} 的周期不该为 0");
        }
    }

    // ---- 友军快照超时清空（PeerWatch）------------------------------------

    #[test]
    fn peer_watch_never_fires_before_any_reply() {
        let mut w = PeerWatch::default();
        let t0 = Instant::now();
        assert!(!w.should_clear(t0 + Duration::from_secs(3600), Duration::from_secs(5)),
                "从来没收到过应答就没有东西可清");
    }

    #[test]
    fn peer_watch_fires_once_at_ttl_and_rearms_on_reply() {
        let ttl = Duration::from_secs(5);
        let t0 = Instant::now();
        let mut w = PeerWatch::default();
        w.on_reply(t0);

        assert!(!w.should_clear(t0 + Duration::from_millis(4999), ttl), "没到点不该清");
        assert!(w.should_clear(t0 + Duration::from_secs(5), ttl), "到点该清一次");
        assert!(!w.should_clear(t0 + Duration::from_secs(6), ttl), "同一轮超时只清一次（不重复发布空快照）");
        assert!(!w.should_clear(t0 + Duration::from_secs(99), ttl));

        // 服务端回来了：重新计时，之后还能再超时一次
        let t1 = t0 + Duration::from_secs(100);
        w.on_reply(t1);
        assert!(!w.should_clear(t1 + Duration::from_secs(4), ttl));
        assert!(w.should_clear(t1 + Duration::from_secs(5), ttl), "重新计时后应能再次触发");
    }

    #[test]
    fn peer_watch_zero_ttl_disables_clearing() {
        let t0 = Instant::now();
        let mut w = PeerWatch::default();
        w.on_reply(t0);
        assert!(!w.should_clear(t0 + Duration::from_secs(86400), Duration::ZERO),
                "配置 0 秒 = 关闭超时清空（保留最后一份快照）");
    }
}
