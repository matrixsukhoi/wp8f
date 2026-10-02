//! Link-8 客户端封装：非阻塞收发，协议层无状态（丢包/断连自动恢复）。

use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};

use crate::{generate_client_id, xor_crypt, xor_key, Reply, StatePacket, DEFAULT_SESSION_ID};

pub struct DataLinkClient {
    socket: UdpSocket,
    server: SocketAddr,
    key: u32,
    client_id: u64,
    session_id: u8,
    seq: u8,
}

impl DataLinkClient {
    /// 连接服务端（UDP 无连接，只是记录地址）；`client_id` 自动生成为启用时刻。
    pub fn new(server: &str, key: &str, session_id: u8) -> io::Result<Self> {
        Self::with_id(server, key, session_id, generate_client_id())
    }

    pub fn with_id(server: &str, key: &str, session_id: u8, client_id: u64) -> io::Result<Self> {
        let server = server
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad server address"))?;
        let socket = UdpSocket::bind(("0.0.0.0", 0))?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            server,
            key: xor_key(key),
            client_id,
            session_id,
            seq: 0,
        })
    }

    pub fn client_id(&self) -> u64 {
        self.client_id
    }

    pub fn session_id(&self) -> u8 {
        self.session_id
    }

    /// 发送一帧自身状态：填入 `client_id`/`session_id`/`magic`/`version`/`seq`，
    /// 加密后发出。发送失败不影响后续（下个周期重试）。
    pub fn send_state(&mut self, mut state: StatePacket) -> io::Result<usize> {
        state.client_id = self.client_id;
        state.session_id = self.session_id;
        state.magic = crate::DATALINK_MAGIC;
        state.version = crate::DATALINK_VERSION;
        state.seq = self.seq;
        self.seq = self.seq.wrapping_add(1);

        let mut wire = state.to_bytes();
        xor_crypt(&mut wire, self.key);
        self.socket.send_to(&wire, self.server)
    }

    /// 排空收包队列，返回**最新一份**全量追踪对象（多份应答取最后一份，
    /// 应答本身就是全量状态）。没有新应答返回 `None`；解密后魔数校验不过的
    /// 包静默丢弃；非本对局（session_id 不符）的条目直接过滤。
    pub fn recv_latest(&mut self) -> Option<Vec<StatePacket>> {
        let mut latest: Option<Vec<StatePacket>> = None;
        let mut buf = [0u8; 4096];
        loop {
            match self.socket.recv_from(&mut buf) {
                Ok((n, _)) => {
                    let mut raw = buf[..n].to_vec();
                    xor_crypt(&mut raw, self.key);
                    if let Some(reply) = Reply::from_bytes(&raw) {
                        if reply.session_id == self.session_id {
                            latest = Some(reply.tracks);
                        }
                    }
                }
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => break,
            }
        }
        // 双保险：条目级 session 过滤
        if let Some(ref mut tracks) = latest {
            tracks.retain(|t| t.session_id == self.session_id);
        }
        latest
    }
}

/// 便于测试/默认场景：默认对局 ID。
pub fn default_session_id() -> u8 {
    DEFAULT_SESSION_ID
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DATALINK_MAGIC, DATALINK_VERSION};
    use std::time::Duration;

    /// 假服务端：一个本地 UDP socket + 可用于构造客户端的地址串。
    fn fake_server() -> (UdpSocket, String) {
        let s = UdpSocket::bind(("127.0.0.1", 0)).expect("bind fake server");
        s.set_read_timeout(Some(Duration::from_millis(500))).expect("set timeout");
        let addr = s.local_addr().expect("local_addr").to_string();
        (s, addr)
    }

    fn send_reply_to(sock: &UdpSocket, to: SocketAddr, key: &str, r: &Reply) {
        let mut wire = r.to_bytes();
        xor_crypt(&mut wire, xor_key(key));
        sock.send_to(&wire, to).expect("send reply");
    }

    #[test]
    fn send_state_overrides_header_and_wraps_seq() {
        let (srv, addr) = fake_server();
        let mut client = DataLinkClient::with_id(&addr, "ck", 9, 42).expect("create client");
        assert_eq!(client.client_id(), 42);
        assert_eq!(client.session_id(), 9);
        let key = xor_key("ck");

        // 故意传入错误头部 → send_state 应全部覆盖为客户端实际值
        let mut state = StatePacket::new(999_999);
        state.session_id = 77;
        state.magic = 0xDEAD_BEEF;
        state.version = 42;
        state.seq = 123;
        state.set_type("f_16c");
        state.ias = 555.0;

        let total = 259usize; // 256 + 3：覆盖 seq 255 → 0 回绕
        let mut seqs = Vec::with_capacity(total);
        let mut buf = [0u8; 256];
        for _ in 0..total {
            client.send_state(state).expect("loopback 发送");
            let (n, _) = srv.recv_from(&mut buf).expect("每个包都应到达");
            let mut raw = buf[..n].to_vec();
            xor_crypt(&mut raw, key);
            let p = StatePacket::from_bytes(&raw).expect("应可解出状态包");
            assert_eq!(p.client_id, 42, "client_id 应被客户端覆盖");
            assert_eq!(p.session_id, 9, "session_id 应被客户端覆盖");
            assert_eq!(p.magic, DATALINK_MAGIC, "magic 应被回填");
            assert_eq!(p.version, DATALINK_VERSION, "version 应被回填");
            assert_eq!(p.type_str(), "f_16c");
            assert_eq!(p.ias, 555.0);
            seqs.push(p.seq);
        }
        let expected: Vec<u8> = (0..=255u8).chain([0u8, 1, 2]).collect();
        assert_eq!(seqs, expected, "seq 应 0→255 递增并回绕到 0");
    }

    #[test]
    fn recv_latest_filters_garbage_foreign_and_mixed_sessions() {
        let (srv, addr) = fake_server();
        let mut client = DataLinkClient::with_id(&addr, "ck", 3, 1).expect("create client");
        let key = "ck";

        // 先发一帧，让假服务端获知客户端地址
        client.send_state(StatePacket::new(1)).expect("first send");
        let mut buf = [0u8; 2048];
        let (_, from) = srv.recv_from(&mut buf).expect("server 收到握手帧");

        // (a) 垃圾包（短包 + 长包）与 (b) 异对局应答 → 全部静默丢弃
        srv.send_to(&[0x11; 20], from).expect("garbage 1");
        srv.send_to(&[0x22; 200], from).expect("garbage 2");
        let foreign =
            Reply { session_id: 4, server_seq: 1, tracks: vec![StatePacket::new(7)] };
        send_reply_to(&srv, from, key, &foreign);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(client.recv_latest(), None, "垃圾与异对局应答都不应返回");

        // (c) 本对局应答，但混入异对局条目 → 条目级过滤只留本对局
        let mut own = StatePacket::new(77);
        own.session_id = 3;
        let mut mixed = StatePacket::new(88);
        mixed.session_id = 9;
        let good = Reply { session_id: 3, server_seq: 2, tracks: vec![own, mixed] };
        send_reply_to(&srv, from, key, &good);
        std::thread::sleep(Duration::from_millis(50));
        let tracks = client.recv_latest().expect("本对局应答应返回");
        assert_eq!(tracks.len(), 1, "异对局条目应被剔除");
        assert_eq!(tracks[0].client_id, 77);

        // (d) 连续两份应答 → 排空后取最后一份（全量）
        let mut own2 = StatePacket::new(78);
        own2.session_id = 3;
        let first = Reply { session_id: 3, server_seq: 3, tracks: vec![own] };
        let second = Reply { session_id: 3, server_seq: 4, tracks: vec![own, own2] };
        send_reply_to(&srv, from, key, &first);
        send_reply_to(&srv, from, key, &second);
        std::thread::sleep(Duration::from_millis(50));
        let tracks = client.recv_latest().expect("应取到应答");
        assert_eq!(tracks.len(), 2, "应取最后一份全量应答");
        let mut ids: Vec<u64> = tracks.iter().map(|t| t.client_id).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![77, 78]);
    }

    #[test]
    fn bad_address_rejected_and_default_session() {
        assert!(DataLinkClient::with_id("1.2.3.4:99999", "k", 1, 1).is_err(), "非法端口应报错");
        assert_eq!(default_session_id(), crate::DEFAULT_SESSION_ID);
    }
}
