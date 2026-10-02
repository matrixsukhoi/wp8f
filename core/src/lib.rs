//! wp8f-core：8111 端口数据拉取 → 解析 → 状态计算 → 写入 disp 帧槽。
//!
//! 导出面刻意收窄到 **bin（main.rs）真正用到的那一份**：workspace 里没有第二个
//! 消费者，而 lib 一旦把内部辅助函数都 `pub use` 出去，rustc 就无法再判定死代码
//! （这就是"默认 cargo check 恒 0 告警"的原因）。新增导出前先问：谁会用？

pub mod channel;
/// 帧调度（纯）：帧号 / 实测间隔 / 丢帧判定 / 地图采样闸门，主循环只喂 `now`。
pub mod frame_clock;
pub mod maneuver_tone;
/// 地图记录间隔（数据帧 ↔ 毫秒）的唯一换算入口（`period_ms`）；
/// 夹紧（`1..=refresh_hz`，上限 = 1 秒的数据帧数）在 `wp8f_disp::HudLayoutConfig` 侧完成。
pub mod mapobj;
pub mod parser;
pub mod warnings;

mod constants;
mod context;
mod display;
mod fmt;
mod math;

pub use channel::async_channel::AsyncChannel;
pub use constants::GRAVITY;
pub use fmt::value::format_adaptive;
pub use context::{get_timestamp_ms, get_timestamp_ns, FlightContext};
pub use display::updater::{update_display_from_state, update_map_display};
pub use parser::{FlightState, Indicators, MapInfo, MapObjData};
pub use wp8f_disp::DisplayData;

/// 同一类告警**最多每秒打一行**（每个调用点一个 `AtomicU64`，无锁、无分配）。
///
/// 控制台/日志写是同步 I/O：8111 断开时 `read_state error` 之类会以数据帧率刷屏，
/// 既淹没有用信息，也可能在主循环里被控制台写阻塞。
pub fn warn_throttled(last_ms: &std::sync::atomic::AtomicU64, msg: impl FnOnce() -> String) {
    use std::sync::atomic::Ordering;
    let now = get_timestamp_ms();
    let prev = last_ms.load(Ordering::Relaxed);
    if now.saturating_sub(prev) < 1000 {
        return;
    }
    if last_ms
        .compare_exchange(prev, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    eprintln!("{}", msg());
}

