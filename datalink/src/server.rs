//! Link-8 服务端：收包 → 解密/魔数校验 → 追踪池 upsert → 应答全量。
//!
//! 纯请求/响应、无会话：单个 `poll()` 处理一个包，方便嵌入与测试。
//! 按 `session_id` 分池：**只有同一对局的数据互相共享**。

use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::track::TrackPool;
use crate::{xor_crypt, xor_key, Reply, StatePacket, DEFAULT_HZ, DEFAULT_PORT, DEFAULT_TIMEOUT_SECS, MAX_TRACKS};

/// 应答限频余量：允许比 `hz` 快 25%，避免客户端同频发包时被周期性丢应答。
const REPLY_INTERVAL_SLACK: f64 = 0.75;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub key: String,
    /// 数据周期（Hz）。>0 时对单客户端的应答限频；<=0 不限频。
    pub hz: f64,
    pub timeout: Duration,
    pub max_tracks: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            key: String::new(),
            hz: DEFAULT_HZ,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            max_tracks: MAX_TRACKS,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ServerStats {
    pub rx_ok: u64,
    pub rx_bad: u64,
    pub tx_replies: u64,
    pub tx_throttled: u64,
    pub spawned: u64,
    pub expired: u64,
}

pub struct Server {
    socket: UdpSocket,
    key: u32,
    cfg: ServerConfig,
    /// session_id → 追踪池（对局隔离）
    sessions: HashMap<u8, TrackPool>,
    last_reply: HashMap<u64, Instant>,
    server_seq: u8,
    stats: ServerStats,
    buf: [u8; 2048],
}

impl Server {
    pub fn bind(cfg: ServerConfig) -> io::Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", cfg.port))?;
        // 有限读超时：让 run() 循环能周期性 prune 超时对象
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        let key = xor_key(&cfg.key);
        Ok(Self {
            socket,
            key,
            cfg,
            sessions: HashMap::new(),
            last_reply: HashMap::new(),
            server_seq: 0,
            stats: ServerStats::default(),
            buf: [0u8; 2048],
        })
    }

    pub fn stats(&self) -> ServerStats {
        self.stats
    }

    /// 实际绑定的本地地址（`port = 0` 时用于获取系统分配的随机端口，测试友好）。
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub fn track_count(&self) -> usize {
        self.sessions.values().map(|p| p.len()).sum()
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// 释放超时追踪对象，并回收空对局。
    pub fn prune(&mut self) {
        for pool in self.sessions.values_mut() {
            self.stats.expired += pool.prune(Instant::now()) as u64;
        }
        self.sessions.retain(|_, p| !p.is_empty());
    }

    /// 处理一个请求（收包→应答）。读超时返回 `Ok(false)`。
    pub fn poll(&mut self) -> io::Result<bool> {
        let (n, from) = match self.socket.recv_from(&mut self.buf) {
            Ok(v) => v,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
            {
                return Ok(false)
            }
            Err(e) => return Err(e),
        };

        let mut raw = self.buf[..n].to_vec();
        xor_crypt(&mut raw, self.key);
        let Some(entry) = StatePacket::from_bytes(&raw) else {
            self.stats.rx_bad += 1;
            return Ok(false);
        };
        self.stats.rx_ok += 1;

        let now = Instant::now();
        let timeout = self.cfg.timeout;
        let pool = self
            .sessions
            .entry(entry.session_id)
            .or_insert_with(|| TrackPool::new(timeout));
        if pool.upsert(entry, now) {
            self.stats.spawned += 1;
        }

        // 单客户端应答限频（hz > 0 时）
        if self.cfg.hz > 0.0 {
            let min_interval =
                Duration::from_secs_f64(REPLY_INTERVAL_SLACK / self.cfg.hz);
            if let Some(last) = self.last_reply.get(&entry.client_id) {
                if now.duration_since(*last) < min_interval {
                    self.stats.tx_throttled += 1;
                    return Ok(true);
                }
            }
        }
        self.last_reply.insert(entry.client_id, now);

        self.server_seq = self.server_seq.wrapping_add(1);
        let tracks = self
            .sessions
            .get(&entry.session_id)
            .map(|p| p.snapshot(entry.client_id, self.cfg.max_tracks))
            .unwrap_or_default();
        let reply = Reply {
            session_id: entry.session_id,
            server_seq: self.server_seq,
            tracks,
        };
        let mut wire = reply.to_bytes();
        xor_crypt(&mut wire, self.key);
        self.send(&wire, from)?;
        self.stats.tx_replies += 1;
        Ok(true)
    }

    fn send(&self, wire: &[u8], to: SocketAddr) -> io::Result<()> {
        self.socket.send_to(wire, to).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FLAG_SENDER;

    /// 随机端口起测试服务端。
    fn bind_server(cfg: ServerConfig) -> Server {
        Server::bind(ServerConfig { port: 0, ..cfg }).expect("bind test server")
    }

    /// 独立"客户端"socket（带读超时，便于断言"没有应答"）。
    fn sender() -> UdpSocket {
        let s = UdpSocket::bind(("127.0.0.1", 0)).expect("bind sender");
        s.set_read_timeout(Some(Duration::from_millis(300))).expect("set timeout");
        s
    }

    fn send_state(sock: &UdpSocket, port: u16, key: &str, p: &StatePacket) {
        let mut wire = p.to_bytes();
        xor_crypt(&mut wire, xor_key(key));
        sock.send_to(&wire, ("127.0.0.1", port)).expect("send packet");
    }

    fn recv_reply(sock: &UdpSocket, key: &str) -> Option<Reply> {
        let mut buf = [0u8; 2048];
        let (n, _) = match sock.recv_from(&mut buf) {
            Ok(v) => v,
            Err(_) => return None,
        };
        let mut raw = buf[..n].to_vec();
        xor_crypt(&mut raw, xor_key(key));
        Reply::from_bytes(&raw)
    }

    /// 循环 poll 直到条件满足（poll 自带 200ms 读超时，最多等 3s）。
    fn poll_until(server: &mut Server, pred: impl Fn(&Server) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let _ = server.poll();
            if pred(server) {
                return true;
            }
        }
        false
    }

    #[test]
    fn bad_packets_counted_and_get_no_reply() {
        let key = "srv-key";
        assert_ne!(xor_key(key), xor_key("wrong-key"), "测试前提：两密钥异或值不同");
        let mut server = bind_server(ServerConfig {
            key: key.into(),
            hz: 0.0,
            ..Default::default()
        });
        let port = server.local_addr().expect("local_addr").port();
        let sock = sender();

        // 1) 随机垃圾（短包）
        sock.send_to(&[0x5A; 37], ("127.0.0.1", port)).expect("send garbage");
        // 2) 错误密钥加密的"合法"包
        let mut forged = StatePacket::new(1234);
        forged.set_type("bad");
        send_state(&sock, port, "wrong-key", &forged);

        assert!(poll_until(&mut server, |s| s.stats().rx_bad >= 2), "坏包应计入 rx_bad");
        assert_eq!(server.stats().rx_ok, 0, "坏包不应计为好包");
        assert_eq!(server.track_count(), 0, "坏包不应建档");
        assert_eq!(server.stats().tx_replies, 0, "坏包不应产生应答");
        assert!(recv_reply(&sock, key).is_none(), "坏包不应收到任何应答");
    }

    #[test]
    fn sessions_isolated_and_sender_flagged() {
        let key = "k";
        let mut server = bind_server(ServerConfig {
            key: key.into(),
            hz: 0.0,
            ..Default::default()
        });
        let port = server.local_addr().expect("local_addr").port();
        let sock_a = sender();
        let sock_b = sender();

        let mut a = StatePacket::new(100);
        a.session_id = 1;
        a.set_type("alpha");
        let mut b = StatePacket::new(200);
        b.session_id = 2;
        b.set_type("bravo");

        // A（session 1）：应答只含自己，且带 FLAG_SENDER
        send_state(&sock_a, port, key, &a);
        assert!(poll_until(&mut server, |s| s.stats().rx_ok >= 1));
        let reply_a = recv_reply(&sock_a, key).expect("A 应收到应答");
        assert_eq!(reply_a.session_id, 1);
        assert_eq!(reply_a.tracks.len(), 1, "session 1 池里只有自己");
        assert_eq!(reply_a.tracks[0].client_id, 100);
        assert_eq!(
            reply_a.tracks[0].flags & FLAG_SENDER,
            FLAG_SENDER,
            "应答中的自己应带 FLAG_SENDER"
        );

        // B（session 2）：只看到 session 2 的数据
        send_state(&sock_b, port, key, &b);
        assert!(poll_until(&mut server, |s| s.stats().rx_ok >= 2));
        let reply_b = recv_reply(&sock_b, key).expect("B 应收到应答");
        assert_eq!(reply_b.session_id, 2);
        assert_eq!(reply_b.tracks.len(), 1, "不同 session互相隔离");
        assert!(reply_b.tracks.iter().all(|t| t.session_id == 2));

        // A 再发：仍看不到 session 2 的 B
        send_state(&sock_a, port, key, &a);
        assert!(poll_until(&mut server, |s| s.stats().rx_ok >= 3));
        let reply_a2 = recv_reply(&sock_a, key).expect("A 应再次收到应答");
        assert_eq!(reply_a2.tracks.len(), 1, "A 的应答不应包含 session 2 条目");
        assert_eq!(reply_a2.tracks[0].client_id, 100);

        assert_eq!(server.session_count(), 2);
        assert_eq!(server.track_count(), 2);
        assert_eq!(server.stats().spawned, 2);
    }

    #[test]
    fn reply_throttled_per_client_by_hz() {
        // hz = 1 → 限频窗口 0.75/1 = 750ms（同一客户端窗口内只应答一次）
        let mut server =
            bind_server(ServerConfig { hz: 1.0, ..Default::default() });
        let port = server.local_addr().expect("local_addr").port();
        let sock_a = sender();
        let sock_b = sender();

        let p1 = StatePacket::new(1);
        send_state(&sock_a, port, "", &p1);
        assert!(poll_until(&mut server, |s| s.stats().rx_ok >= 1));
        assert_eq!(server.stats().tx_replies, 1);
        assert!(recv_reply(&sock_a, "").is_some(), "首次发包应收到应答");

        // 同客户端立即再发 → 收包计数但被限频、不回包
        send_state(&sock_a, port, "", &p1);
        assert!(poll_until(&mut server, |s| s.stats().rx_ok >= 2));
        assert_eq!(server.stats().tx_throttled, 1, "窗口内应被限频");
        assert_eq!(server.stats().tx_replies, 1, "限频窗口内不再应答");
        assert!(recv_reply(&sock_a, "").is_none(), "限频时不应收到应答");

        // 限频按 client_id 隔离：另一客户端不受影响
        send_state(&sock_b, port, "", &StatePacket::new(2));
        assert!(poll_until(&mut server, |s| s.stats().rx_ok >= 3));
        assert_eq!(server.stats().tx_replies, 2, "限频只作用于同一客户端");
        assert!(recv_reply(&sock_b, "").is_some());
    }

    #[test]
    fn hz_nonpositive_disables_throttle() {
        let mut server =
            bind_server(ServerConfig { hz: 0.0, ..Default::default() });
        let port = server.local_addr().expect("local_addr").port();
        let sock = sender();

        let p = StatePacket::new(1);
        for want in 1..=3u64 {
            send_state(&sock, port, "", &p);
            assert!(poll_until(&mut server, |s| s.stats().rx_ok >= want));
            assert!(recv_reply(&sock, "").is_some(), "hz=0 时每包必回（第 {want} 次）");
        }
        assert_eq!(server.stats().tx_replies, 3);
        assert_eq!(server.stats().tx_throttled, 0);
    }

    #[test]
    fn prune_expires_stale_and_recycles_sessions() {
        let mut server = bind_server(ServerConfig {
            hz: 0.0,
            timeout: Duration::from_millis(50),
            ..Default::default()
        });
        let port = server.local_addr().expect("local_addr").port();
        let sock = sender();

        send_state(&sock, port, "", &StatePacket::new(5));
        assert!(poll_until(&mut server, |s| s.stats().rx_ok >= 1));
        assert_eq!(server.stats().spawned, 1);
        assert_eq!(server.track_count(), 1);
        assert_eq!(server.session_count(), 1);

        std::thread::sleep(Duration::from_millis(80));
        server.prune();
        assert_eq!(server.stats().expired, 1, "超时追踪对象应被释放");
        assert_eq!(server.track_count(), 0);
        assert_eq!(server.session_count(), 0, "空对局池应被回收");
    }

    #[test]
    fn reply_capped_by_max_tracks() {
        let mut server = bind_server(ServerConfig {
            hz: 0.0,
            max_tracks: 2,
            ..Default::default()
        });
        let port = server.local_addr().expect("local_addr").port();
        let socks: Vec<UdpSocket> = (0..4).map(|_| sender()).collect();

        for (i, id) in (10u64..=13).enumerate() {
            send_state(&socks[i], port, "", &StatePacket::new(id));
            let want = (i + 1) as u64;
            assert!(poll_until(&mut server, |s| s.stats().rx_ok >= want), "id={id} 应被接收");
        }

        // 最后一个客户端（id=13）：快照按 id 升序截断到 2 条 → [10, 11]，自己被截掉
        let last = recv_reply(&socks[3], "").expect("id=13 应收到应答");
        assert_eq!(last.tracks.len(), 2, "应答被 max_tracks 截断");
        let ids: Vec<u64> = last.tracks.iter().map(|t| t.client_id).collect();
        assert_eq!(ids, vec![10, 11], "截断保留 client_id 最小的条目");
        assert!(
            last.tracks.iter().all(|t| t.flags & FLAG_SENDER == 0),
            "requester 被截掉 → 应答中没有带 FLAG_SENDER 的自己"
        );
    }
}
