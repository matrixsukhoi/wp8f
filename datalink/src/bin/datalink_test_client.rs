//! Link-8 测试客户端：按周期上报合成飞行状态，并打印收到的全部追踪对象。
//!
//! 用法示例：
//! ```text
//! datalink-test-client --server 127.0.0.1:2887 --key my-secret --ac-type f_16c --count 20
//! ```

use std::time::Duration;

use clap::Parser;
use datalink::client::DataLinkClient;
use datalink::StatePacket;

#[derive(Parser, Debug)]
#[command(name = "datalink-test-client", about = "Link-8 测试客户端（合成数据收发）")]
struct Args {
    /// 服务端地址（ip:port）
    #[arg(long, default_value = "127.0.0.1:2887")]
    server: String,

    /// 加密密钥（须与服务端一致）
    #[arg(long, default_value = "")]
    key: String,

    /// 对局编号（同对局才互相共享）
    #[arg(long, default_value_t = datalink::DEFAULT_SESSION_ID)]
    session: u8,

    /// 发送频率 Hz
    #[arg(long, default_value_t = 4.0)]
    hz: f64,

    /// 机型
    #[arg(long, default_value = "test_plane")]
    ac_type: String,

    /// 初始归一化地图坐标 x
    #[arg(long, default_value_t = 0.5)]
    x: f64,

    /// 初始归一化地图坐标 y
    #[arg(long, default_value_t = 0.5)]
    y: f64,

    /// 发送多少个包后退出（0 = 一直运行）
    #[arg(long, default_value_t = 0)]
    count: u64,

    /// 固定序列回放模式：8 个航路点绕图心循环（表速/高度/马赫固定值），
    /// 方便游戏内核对友军显示
    #[arg(long)]
    fixed: bool,

    /// 启用 POI 同步（配合 --poi-x/--poi-y）
    #[arg(long)]
    poi: bool,

    /// POI 归一化地图坐标 x
    #[arg(long, default_value_t = 0.6)]
    poi_x: f64,

    /// POI 归一化地图坐标 y
    #[arg(long, default_value_t = 0.4)]
    poi_y: f64,
}

/// 固定序列：(x, y, heading, ias, altitude)，绕 (0.5,0.5) 半径 0.1 的 8 航路点。
const FIXED_SEQ: &[(f64, f64, f64, f64, f64)] = &[
    (0.50, 0.40, 0.0, 523.0, 3129.0),
    (0.57, 0.43, 45.0, 523.0, 3129.0),
    (0.60, 0.50, 90.0, 523.0, 3129.0),
    (0.57, 0.57, 135.0, 523.0, 3129.0),
    (0.50, 0.60, 180.0, 523.0, 3129.0),
    (0.43, 0.57, 225.0, 523.0, 3129.0),
    (0.40, 0.50, 270.0, 523.0, 3129.0),
    (0.43, 0.43, 315.0, 523.0, 3129.0),
];

fn main() {
    let args = Args::parse();
    let mut client =
        DataLinkClient::new(&args.server, &args.key, args.session).expect("failed to create client");
    let period = Duration::from_secs_f64(1.0 / args.hz.max(0.1));
    println!(
        "[test-client] server={} client_id={} session={} type={} period={:?}",
        args.server,
        client.client_id(),
        args.session,
        args.ac_type,
        period
    );

    let mut state = StatePacket::new(client.client_id());
    state.set_type(&args.ac_type);
    if args.poi {
        state.poi_enabled = 1;
        state.poi_x = args.poi_x;
        state.poi_y = args.poi_y;
        println!("[test-client] POI enabled at ({:.4},{:.4})", args.poi_x, args.poi_y);
    }
    state.ias = 500.0;
    state.tas = 560.0;
    state.altitude = 3000.0;
    state.mach = 0.8;
    state.heading = 90.0;
    state.x = args.x;
    state.y = args.y;

    let mut sent = 0u64;
    let mut step = 0usize;
    loop {
        if args.fixed {
            // 固定序列回放：数值可预期，便于游戏内核对
            let (x, y, hdg, ias, alt) = FIXED_SEQ[step % FIXED_SEQ.len()];
            state.x = x;
            state.y = y;
            state.heading = hdg;
            state.ias = ias;
            state.tas = 601.0;
            state.altitude = alt;
            state.mach = 0.87;
            state.vy = 0.0;
        } else {
            // 简单运动模型：向东漂移、缓慢爬升，便于肉眼观察坐标变化
            state.x = (state.x + 0.0004).min(1.0);
            state.heading = (state.heading + 0.5) % 360.0;
            state.altitude += 1.0;
        }
        step += 1;

        if let Err(e) = client.send_state(state) {
            eprintln!("[test-client] send error: {e}");
        }
        sent += 1;

        std::thread::sleep(period);

        if let Some(tracks) = client.recv_latest() {
            println!("[test-client] reply: {} track(s)", tracks.len());
            for t in &tracks {
                let self_mark = if t.flags & datalink::FLAG_SENDER != 0 { "*" } else { " " };
                println!(
                    "  {} id={} s={} type={:<16} ias={:>5.0} tas={:>5.0} alt={:>6.0} M={:.2} hdg={:>5.1} vy={:>5.1} pos=({:.4},{:.4}){}",
                    self_mark,
                    t.client_id,
                    t.session_id,
                    t.type_str(),
                    t.ias,
                    t.tas,
                    t.altitude,
                    t.mach,
                    t.heading,
                    t.vy,
                    t.x,
                    t.y,
                    if t.poi_enabled != 0 {
                        format!(" POI=({:.3},{:.3})", t.poi_x, t.poi_y)
                    } else {
                        String::new()
                    }
                );
            }
        }

        if args.count > 0 && sent >= args.count {
            println!("[test-client] sent {sent} packet(s), exiting");
            break;
        }
    }
}
