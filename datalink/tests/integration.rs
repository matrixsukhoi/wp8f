//! Server ↔ DataLinkClient 本地回环集成测试。
//!
//! 每个测试在 127.0.0.1 随机端口起一个 Server 线程（`port = 0` + `local_addr()`），
//! 用真实 `DataLinkClient` 收发；时序用重试/短 sleep 兜底。

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use datalink::client::DataLinkClient;
use datalink::server::{Server, ServerConfig, ServerStats};
use datalink::{xor_key, StatePacket, FLAG_SENDER};

/// 数据链线程的用例共用**进程级**槽（真实进程里也只有一个数据链线程），所以这组用例必须串行：
/// 否则两个线程往同一个友军槽里写，断言会看到对方的快照。其余用例只走 `DataLinkClient`，可并行。
static LINK_THREAD_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 每个**进程**独立的密钥（`<tag>-<pid>`）：固定字符串会让同机并行跑的另一个测试进程
/// 用同一把密钥与 session 生成本进程会认账的应答包。见
/// [`link_thread_clears_stale_peers_after_timeout`]。
fn run_key(tag: &str) -> String {
    format!("{tag}-{}", std::process::id())
}

/// 测试用服务端：独立线程跑 `poll() + prune()`，Drop 时停线程并回收。
struct LoopbackServer {
    server: Arc<Mutex<Server>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    port: u16,
}

impl LoopbackServer {
    fn start(key: &str, hz: f64) -> Self {
        let cfg = ServerConfig {
            port: 0,
            key: key.to_string(),
            hz,
            ..ServerConfig::default()
        };
        let server = Server::bind(cfg).expect("bind server on random port");
        let port = server.local_addr().expect("local_addr").port();
        let server = Arc::new(Mutex::new(server));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, st) = (Arc::clone(&server), Arc::clone(&stop));
        let handle = thread::spawn(move || {
            while !st.load(Ordering::SeqCst) {
                {
                    let mut srv = s.lock().expect("lock server");
                    let _ = srv.poll();
                    srv.prune();
                }
                thread::sleep(Duration::from_millis(2));
            }
        });
        Self { server, stop, handle: Some(handle), port }
    }

    fn addr(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    fn stats(&self) -> ServerStats {
        self.server.lock().expect("lock server").stats()
    }

    fn track_count(&self) -> usize {
        self.server.lock().expect("lock server").track_count()
    }

    fn session_count(&self) -> usize {
        self.server.lock().expect("lock server").session_count()
    }

    /// 服务端下线：停掉应答线程，但**把 UDP 套接字继续攥在手里**（不关、不解绑）。
    ///
    /// 不能简单 `drop(srv)`：那会释放临时端口，并行的另一个测试进程可能抢到它并用同样的
    /// key/session 继续应答，"服务端不再应答"这条前提就失效了。
    fn go_offline(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// 反复发送直到收到**满足谓词**的应答（hz = 0 时每包必回；应答为全量，取最新一份）。
/// 谓词用于跳过队列里残留的旧应答（例如上一阶段的全量快照）。
fn send_until(
    client: &mut DataLinkClient,
    state: StatePacket,
    budget: Duration,
    want: impl Fn(&[StatePacket]) -> bool,
) -> Option<Vec<StatePacket>> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        client.send_state(state).ok()?;
        if let Some(tracks) = client.recv_latest() {
            if want(&tracks) {
                return Some(tracks);
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    None
}

/// 轮询条件直到成立（或超时返回 false）。
fn wait_until(f: impl Fn() -> bool, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn client_roundtrip_self_state_with_sender_flag_and_poi() {
    let key = run_key("itest");
    let srv = LoopbackServer::start(&key, 0.0);
    let mut client = DataLinkClient::with_id(&srv.addr(), &key, 3, 1111).expect("client");
    assert_eq!(client.client_id(), 1111);
    assert_eq!(client.session_id(), 3);

    let mut state = StatePacket::new(0); // client_id/session 由 send_state 覆盖
    state.set_type("f_16c");
    state.ias = 512.0;
    state.tas = 590.0;
    state.altitude = 3200.0;
    state.mach = 0.85;
    state.heading = 123.5;
    state.vy = -2.5;
    state.x = 0.41;
    state.y = 0.72;
    state.poi_enabled = 1;
    state.poi_x = 0.6;
    state.poi_y = 0.33;

    let tracks = send_until(&mut client, state, Duration::from_secs(3), |t| {
        t.iter().any(|x| x.client_id == 1111)
    })
    .expect("应收到应答");
    let me = tracks.iter().find(|t| t.client_id == 1111).expect("应答应包含自己");
    assert_eq!(me.flags & FLAG_SENDER, FLAG_SENDER, "自己应带 FLAG_SENDER");
    assert_eq!(me.session_id, 3);
    assert_eq!(me.type_str(), "f_16c");
    assert_eq!(me.ias, 512.0);
    assert_eq!(me.tas, 590.0);
    assert_eq!(me.altitude, 3200.0);
    assert_eq!(me.mach, 0.85);
    assert_eq!(me.heading, 123.5);
    assert_eq!(me.vy, -2.5);
    assert_eq!(me.x, 0.41);
    assert_eq!(me.y, 0.72);
    assert_eq!(me.poi_enabled, 1, "POI 启用标志应端到端往返");
    assert_eq!(me.poi_x, 0.6);
    assert_eq!(me.poi_y, 0.33);
    assert!(tracks.iter().all(|t| t.session_id == 3), "应答条目都在本对局");

    assert!(wait_until(|| srv.stats().rx_ok >= 1, Duration::from_secs(1)), "应收到好包");
    assert_eq!(srv.stats().rx_bad, 0);
    assert_eq!(srv.track_count(), 1);
}

#[test]
fn two_clients_same_session_share_state() {
    let key = run_key("k2");
    let srv = LoopbackServer::start(&key, 0.0);
    let mut a = DataLinkClient::with_id(&srv.addr(), &key, 5, 2001).expect("client a");
    let mut b = DataLinkClient::with_id(&srv.addr(), &key, 5, 2002).expect("client b");

    let mut sa = StatePacket::new(0);
    sa.set_type("a_plane");
    sa.x = 0.1;
    let mut sb = StatePacket::new(0);
    sb.set_type("b_plane");
    sb.y = 0.9;

    // A 先发：池里暂时只有自己
    let ra = send_until(&mut a, sa, Duration::from_secs(3), |t| t.len() == 1)
        .expect("A 应收到应答");
    assert_eq!(ra.len(), 1);
    assert_eq!(ra[0].client_id, 2001);

    // B 发：同对局互相可见，按 client_id 升序，自己带 FLAG_SENDER
    let rb = send_until(&mut b, sb, Duration::from_secs(3), |t| t.len() == 2)
        .expect("B 应收到应答");
    assert_eq!(rb.len(), 2, "同对局应互相可见");
    let ids: Vec<u64> = rb.iter().map(|t| t.client_id).collect();
    assert_eq!(ids, vec![2001, 2002], "按 client_id 升序");
    let me_b = rb.iter().find(|t| t.client_id == 2002).expect("应含自己");
    assert_eq!(me_b.flags & FLAG_SENDER, FLAG_SENDER);
    let other = rb.iter().find(|t| t.client_id == 2001).expect("应含 A");
    assert_eq!(other.flags & FLAG_SENDER, 0, "别人不应带 FLAG_SENDER");

    // A 再发：应答含双方，且能取回 B 的字段
    //（谓词跳过 A 队列里残留的上一阶段"仅自己"的旧应答）
    let ra2 = send_until(&mut a, sa, Duration::from_secs(3), |t| {
        t.iter().any(|x| x.client_id == 2002)
    })
    .expect("A 再次收到应答");
    assert_eq!(ra2.len(), 2);
    let from_b = ra2.iter().find(|t| t.client_id == 2002).expect("应含 B");
    assert_eq!(from_b.type_str(), "b_plane");
    assert_eq!(from_b.y, 0.9);
    assert_eq!(from_b.session_id, 5);
}

#[test]
fn sessions_isolated_over_the_wire() {
    let key = run_key("k3");
    let srv = LoopbackServer::start(&key, 0.0);
    let mut c1 = DataLinkClient::with_id(&srv.addr(), &key, 1, 3001).expect("c1");
    let mut c2 = DataLinkClient::with_id(&srv.addr(), &key, 2, 3002).expect("c2");

    let r1 = send_until(&mut c1, StatePacket::new(0), Duration::from_secs(3), |t| {
        t.iter().any(|x| x.client_id == 3001)
    })
    .expect("c1 应收到应答");
    assert_eq!(r1.len(), 1);
    assert_eq!(r1[0].client_id, 3001);
    assert!(r1.iter().all(|t| t.session_id == 1));

    // session 2 的客户端看不到 session 1 的 3001
    let r2 = send_until(&mut c2, StatePacket::new(0), Duration::from_secs(3), |t| {
        t.iter().any(|x| x.client_id == 3002)
    })
    .expect("c2 应收到应答");
    assert_eq!(r2.len(), 1, "不同对局互相隔离");
    assert_eq!(r2[0].client_id, 3002);
    assert!(r2.iter().all(|t| t.session_id == 2));

    assert!(wait_until(|| srv.session_count() == 2, Duration::from_secs(1)));
    assert_eq!(srv.track_count(), 2, "两个对局各 1 条");
}

#[test]
fn bad_and_wrong_key_packets_counted_rx_bad() {
    let key = run_key("srv-key");
    assert_ne!(xor_key(&key), xor_key(&format!("{key}-wrong")), "测试前提：两密钥异或值不同");
    let srv = LoopbackServer::start(&key, 0.0);

    // 1) 裸 socket 垃圾包 ×2（短包 + 超长包）
    let raw = UdpSocket::bind(("127.0.0.1", 0)).expect("raw socket");
    raw.send_to(&[0x37; 12], ("127.0.0.1", srv.port)).expect("send garbage 1");
    raw.send_to(&[0x37; 200], ("127.0.0.1", srv.port)).expect("send garbage 2");

    // 2) 错误密钥的"合法"客户端：被服务端解密校验拒绝
    let mut wrong =
        DataLinkClient::with_id(&srv.addr(), &format!("{key}-wrong"), 1, 4001).expect("wrong client");
    wrong.send_state(StatePacket::new(0)).expect("send with wrong key");

    // 3) 正确密钥客户端仍能正常往返
    let mut ok = DataLinkClient::with_id(&srv.addr(), &key, 1, 4002).expect("ok client");
    let tracks = send_until(&mut ok, StatePacket::new(0), Duration::from_secs(3), |t| {
        t.iter().any(|x| x.client_id == 4002)
    })
    .expect("正确密钥应收到应答");
    assert!(tracks.iter().any(|t| t.client_id == 4002));

    assert!(
        wait_until(|| srv.stats().rx_bad >= 3, Duration::from_secs(3)),
        "2 个垃圾包 + 1 个错误密钥包应计入 rx_bad: {:?}",
        srv.stats()
    );
    assert_eq!(srv.stats().rx_bad, 3, "只计入这 3 个坏包");
    assert!(srv.stats().rx_ok >= 1, "正确密钥的包应计为好包");
    assert_eq!(srv.track_count(), 1, "只有正确密钥的客户端建档");
    assert_eq!(srv.session_count(), 1);
}

/// 数据链线程端到端：core 只 `publish_own()`（没有任何 socket），线程负责上报 + 排空应答 +
/// 把全量快照写进友军槽，HUD 侧用 `latest_tracks()` 取最新 —— 三方不共享锁、不互相等待。
#[test]
fn link_thread_publishes_latest_peers_without_main_loop_io() {
    use datalink::link_ring::{self, OwnState};
    use datalink::thread::{spawn_link, LinkConfig};

    let _serial = LINK_THREAD_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let key = run_key("l8thread");
    // 槽是**进程级**的：先清干净，别继承上一个用例留下的快照（见文件头与下一条用例的说明）
    link_ring::reset_tracks();
    let srv = LoopbackServer::start(&key, 0.0);
    let handle = spawn_link(LinkConfig {
        enabled: true,
        server: "127.0.0.1".to_string(),
        port: srv.port,
        key: key.clone(),
        session_id: 3,
        send_hz: 10,
        // 本用例不测超时清空（下一条用例专测），给个足够长的值避免误清
        peer_timeout_secs: 60,
    })
    .expect("线程应能建立（UDP 客户端 + 槽）");
    assert_eq!(handle.send_hz(), 10);

    // core 侧：模拟 3 帧本机状态（线程只该上报**最新**那一版）
    for i in 0..3 {
        let mut own = OwnState {
            ias: 500.0 + i as f64,
            tas: 600.0,
            altitude: 3200.0,
            mach: 0.8,
            heading: 271.0,
            vy: -12.0,
            x: 0.75,
            y: 0.25,
            poi_enabled: true,
            poi_x: 0.8,
            poi_y: 0.2,
            ..OwnState::default()
        };
        own.set_type("f_16c");
        link_ring::publish_own(own);
        thread::sleep(Duration::from_millis(5));
    }

    // HUD 侧：等线程写出第一份友军快照（本机是唯一客户端，服务端会回填 FLAG_SENDER）
    //
    // 判据用**本线程自己的计数**（`handle.stats().replies`，新线程从 0 起）+ 快照内容，
    // 而不是"槽里有没有值"：槽是进程级的，上个用例结束时可能还留着非空快照（见文件头）。
    assert!(
        wait_until(|| handle.stats().replies >= 1, Duration::from_secs(5)),
        "5s 内应收到应答"
    );
    assert!(
        wait_until(
            || link_ring::latest_tracks().iter().any(|t| t.type_str() == "f_16c"),
            Duration::from_secs(5)
        ),
        "5s 内应把**本局**的应答快照写进友军槽"
    );
    let tracks = link_ring::latest_tracks();
    assert_eq!(tracks.len(), 1, "服务端只认识一个客户端");
    let me = &tracks[0];
    assert_ne!(me.flags & FLAG_SENDER, 0, "自己那一条应带服务端回填的 FLAG_SENDER");
    assert_eq!(me.session_id, 3);
    assert_eq!(me.type_str(), "f_16c");
    assert_eq!(me.altitude, 3200.0);
    assert_eq!(me.poi_enabled, 1);
    assert!(
        (me.ias - 502.0).abs() < 1e-9,
        "上报的应是投递时槽里的最新一版（ias=502），实际 {}",
        me.ias
    );

    let st = handle.stats();
    assert!(st.sent >= 1, "应至少上报一次：{st:?}");
    assert!(st.replies >= 1, "应至少收到一份应答：{st:?}");
    assert_eq!(st.peers, 1, "最新一份应答里有 1 个友军（自己）");

    // 重连：清空后线程继续发布，HUD 不会一直空着
    link_ring::reset_tracks();
    assert!(link_ring::latest_tracks().is_empty(), "reset_tracks() 后应立刻清空");
    assert!(
        wait_until(
            || link_ring::latest_tracks().iter().any(|t| t.type_str() == "f_16c"),
            Duration::from_secs(5)
        ),
        "重置后线程应继续把新快照写回来"
    );

    handle.stop();
    // 收尾：槽是进程级的，走之前清干净 —— 下一个用例的"先要收到一份快照"不能被本用例的残值满足
    link_ring::reset_tracks();
}

/// 服务端停止应答后，友军快照必须在 `peer_timeout_secs` 内被清空
/// （否则 HUD 上会一直挂着早已离线的友军标记）。
///
/// ⚠️ 等待条件只看**本线程自己的计数**（`handle.stats()` 从 0 起）与**快照内容**
/// （`type_str() == "f_16c"`），不看"槽里有没有值"：槽是进程级的，别的用例的残值会瞬间
/// 满足"先收到一份快照"，于是断言链条错位。用例开头故意把槽弄脏，把这个时序变成确定性前置条件；
/// 收尾清干净。
///
/// "服务端下线"用 [`LoopbackServer::go_offline`]（停应答但攥住端口）而不是 `drop`：
/// `drop` 会把端口还给系统，并行的另一个测试进程可能抢到它并用同样的 key/session 继续应答。
#[test]
fn link_thread_clears_stale_peers_after_timeout() {
    use datalink::link_ring::{self, OwnState};
    use datalink::thread::{spawn_link, LinkConfig};

    let _serial = LINK_THREAD_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let key = run_key("l8ttl");

    // **故意弄脏进程级槽**（等价于"上一个用例刚跑完"），让"等残值满足断言"这条错误路径
    // 每次都会走到 —— 修好之后这里不影响任何断言。
    link_ring::init_slots();
    for i in 0..3 {
        let mut own = OwnState { ias: 111.0 + i as f64, ..OwnState::default() };
        own.set_type("prev_test_plane");
        link_ring::publish_own(own);
    }
    link_ring::publish_tracks(&[datalink::StatePacket::new(4242)]);
    assert!(!link_ring::latest_tracks().is_empty(), "前置条件：槽里有一份上一个用例的残值");

    let mut srv = LoopbackServer::start(&key, 0.0);
    let port = srv.port;
    let handle = spawn_link(LinkConfig {
        enabled: true,
        server: "127.0.0.1".to_string(),
        port,
        key: key.clone(),
        session_id: 1,
        send_hz: 10,
        peer_timeout_secs: 1, // 测试用最短超时（秒粒度）
    })
    .expect("线程应能建立");

    let mut own = OwnState { ias: 480.0, altitude: 2500.0, ..OwnState::default() };
    own.set_type("f_16c");
    link_ring::publish_own(own);

    // 先拿到一份友军（服务端在跑）。
    // **判据只看本线程**：槽里的残值（上面故意塞的那份）不算数 —— 要等本线程真的收到应答
    // （`replies` 从 0 起）**且**槽里出现本局上报的 `f_16c` 快照。
    assert!(
        wait_until(|| handle.stats().replies >= 1, Duration::from_secs(15)),
        "先要收到一份应答，才谈得上过期清空"
    );
    assert!(
        wait_until(
            || link_ring::latest_tracks().iter().any(|t| t.type_str() == "f_16c"),
            Duration::from_secs(15)
        ),
        "槽里应当出现本局写回的友军快照（而不是上一个用例的残值）"
    );
    assert!(handle.stats().peers >= 1, "应答里应当有 1 个友军（自己）");

    // 服务端下线：停应答，但端口仍归本进程（见 go_offline 的说明）
    srv.go_offline();

    // **不变量**：端口不能被别人抢走 —— 一旦被抢走，"没有应答"就不再成立。
    // 这里用一个同 key/session 的"抢端口"服务端做探针：它必须绑不上（地址仍被占用）。
    let thief = Server::bind(ServerConfig {
        port,
        key: key.clone(),
        hz: 0.0,
        ..ServerConfig::default()
    });
    assert!(
        thief.is_err(),
        "服务端下线后端口不该被释放：{} 上还能再绑一个同 key 的服务端，\
         并行跑测试时它会顶替应答，友军就永远不会过期",
        port
    );
    drop(thief);

    // 超时清空：同样看本线程的计数（`expired` 从 0 → 1）与槽内容，不看"槽是不是空的"
    //（槽空可能只是"线程刚启动还没收到应答"，那不代表超时清空发生过）
    assert!(
        wait_until(|| handle.stats().expired >= 1, Duration::from_secs(15)),
        "超过 peer_timeout_secs(1s) 没有应答后，应记录一次超时清空：{:?}",
        handle.stats()
    );
    assert!(
        wait_until(|| link_ring::latest_tracks().is_empty(), Duration::from_secs(5)),
        "超时清空之后，友军快照必须是空的"
    );
    let st = handle.stats();
    assert_eq!(st.peers, 0, "清空后 peers 归零");

    handle.stop();
    // 收尾：不留残值给下一个用例
    link_ring::reset_tracks();
}
