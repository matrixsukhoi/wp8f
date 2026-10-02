//! Link-8 独立服务端。
//!
//! 用法示例：
//! ```text
//! datalink-server --key my-secret --port 2887 --hz 4 --timeout 5
//! ```

use std::time::{Duration, Instant};

use clap::Parser;
use datalink::server::{Server, ServerConfig};

#[derive(Parser, Debug)]
#[command(name = "datalink-server", about = "Link-8 数据链服务端（UDP 请求/响应）")]
struct Args {
    /// 监听端口
    #[arg(long, default_value_t = datalink::DEFAULT_PORT)]
    port: u16,

    /// 加密密钥（异或值 = 密钥字节求和）
    #[arg(long, default_value = "")]
    key: String,

    /// 数据周期 Hz（单客户端应答限频，<=0 不限）
    #[arg(long, default_value_t = datalink::DEFAULT_HZ)]
    hz: f64,

    /// 追踪对象超时释放时间（秒）
    #[arg(long, default_value_t = datalink::DEFAULT_TIMEOUT_SECS)]
    timeout: u64,

    /// 单个应答最多携带的追踪对象数
    #[arg(long, default_value_t = datalink::MAX_TRACKS)]
    max_tracks: usize,
}

fn main() {
    let args = Args::parse();
    let cfg = ServerConfig {
        port: args.port,
        key: args.key,
        hz: args.hz,
        timeout: Duration::from_secs(args.timeout.max(1)),
        max_tracks: args.max_tracks.min(datalink::MAX_TRACKS).max(1),
    };

    let mut server = Server::bind(cfg.clone()).expect("failed to bind datalink server");
    eprintln!(
        "[datalink-server] listening on 0.0.0.0:{}  hz={} timeout={}s max_tracks={} key_len={}",
        cfg.port,
        cfg.hz,
        cfg.timeout.as_secs(),
        cfg.max_tracks,
        cfg.key.len()
    );

    let mut last_stats = Instant::now();
    loop {
        if let Err(e) = server.poll() {
            eprintln!("[datalink-server] poll error: {e}");
        }
        server.prune();
        if last_stats.elapsed() >= Duration::from_secs(10) {
            let s = server.stats();
            eprintln!(
                "[datalink-server] tracks={} rx_ok={} rx_bad={} replies={} throttled={} spawned={} expired={}",
                server.track_count(),
                s.rx_ok,
                s.rx_bad,
                s.tx_replies,
                s.tx_throttled,
                s.spawned,
                s.expired
            );
            last_stats = Instant::now();
        }
    }
}
