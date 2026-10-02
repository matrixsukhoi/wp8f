//! 地图记录间隔（**数据帧 ↔ 毫秒**）的**唯一换算入口**。
//!
//! 配置里的 `map_obj_record_every_frames` 语义是"**每多少个数据帧记录一次地图对象**"
//! （最小 1 = 每个数据帧都记录，等价于数据帧刷新率），而"多久记录一次"要有毫秒概念：
//!
//! ```text
//! map_period_ms = frames × 1000 / refresh_hz
//! ```
//!
//! **夹紧区间 = `1..=refresh_hz`**（上限 = **1 秒的数据帧数**：30 Hz → 最多 30 帧，
//! 5 Hz → 最多 5 帧）。下界 1 是语义本身（1 = 每帧记录 = 数据帧刷新率）；
//! 上界只是"超过 1 秒就不是记录而是快照"的口径，**与本项之外的任何配置无关**。
//!
//! 夹紧的**实现**在 `wp8f_disp::HudLayoutConfig::map_obj_record_every_frames_clamped()`
//! ——只有那里同时拿得到 `refresh_hz` 与本键（disp 是配置的持有者，core 反过来依赖 disp，
//! 夹紧不可能写在 core 里）。core 主循环/`FlightContext` 初值/记录起点
//! （`RecordConfig::start_frame`）一律用那个结果；GUI 用同一口径复算滑杆上限
//! （`gui/static/config-form.js` 里写明了这层对应关系，改口径要两边一起改）。
//!
//! ⚠️ **这里曾经还有一层"采样周期约束"**（`map_period_ms ≤ record.poll_ms`，超了就夹到
//! `poll_ms × refresh_hz / 1000`）：在出厂配置（8 帧 @30 Hz = 266 ms > poll_ms 100 ms）下
//! 会把 8 夹成 3、把 8 夹成 1 —— 用户明确要求删掉，记录间隔只看 `refresh_hz`，不再看
//! 记录采样周期。`record.poll_ms` 现在只影响**记录线程自己的采样频率**。
//!
//! 单位换算的性质（改之前先看）：毫秒**向下取整**（`frames × 1000 / refresh_hz`，整数除法），
//! 8 数据帧 @30 Hz → 266 ms；`refresh_hz = 0` 按 1 Hz 处理（不能除零）。

/// 地图记录间隔（数据帧）→ 毫秒（向下取整）。`refresh_hz = 0` 按 1 Hz 处理。
///
/// 只用于**显示/日志**（`[MAPOBJ]` 那行说明夹紧后的 ms 口径）：真正决定记录频率的是
/// 帧数本身（主循环闸门按帧计数）。数值口径与 GUI 侧 `Math.round(frames * 1000 / hz)` 一致。
pub fn period_ms(frames: u64, refresh_hz: u32) -> u64 {
    frames.saturating_mul(1000) / refresh_hz.max(1) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 帧 → 毫秒：唯一公式 `frames × 1000 / refresh_hz`（整数向下取整）。
    #[test]
    fn period_is_frames_times_1000_over_hz() {
        assert_eq!(period_ms(8, 30), 266, "8 帧 @30 Hz = 266 ms");
        assert_eq!(period_ms(1, 30), 33, "1 帧 = 一个主循环节拍");
        assert_eq!(period_ms(2, 10), 200);
        assert_eq!(period_ms(30, 30), 1000, "30 帧 @30 Hz = 1 s（= 上限所在的 1 秒口径）");
        // 边界：不能除零、不能溢出
        assert_eq!(period_ms(8, 0), 8000, "refresh_hz=0 按 1 Hz 处理");
        assert_eq!(period_ms(0, 30), 0);
        assert_eq!(period_ms(u64::MAX, 1), u64::MAX / 1);
    }
}
