//! 配置派生量的**合法区间**（`Clamp`）：区间和默认值写在一起，调用点不再抄上下限字面量。
//!
//! 只用于配置派生量（刷新率 / 地图记录间隔 / 环槽数 …）；几何与像素的钳制属于布局语义，不在这里。
//!
//! 从 `lib.rs` 拆出的第三块；对外路径不变（`lib.rs` 有 `pub use clamp::{...}`）。

use super::{REFRESH_HZ_MAX, REFRESH_HZ_MIN, RING_SLOTS_MAX};

// -------------------------------------------------------- 配置派生量区间 --
/// 配置派生量的**合法区间**：区间和默认值写在一起，调用点不再抄上下限字面量。
///
/// 只用于**配置派生量**（刷新率 / 地图记录间隔 / 环槽数 …）；几何与像素的钳制属于布局语义，
/// 不在这里。
#[derive(Debug, Clone, Copy)]
pub struct Clamp<T> {
    pub lo: T,
    pub hi: T,
}

impl<T: PartialOrd + Copy> Clamp<T> {
    pub const fn new(lo: T, hi: T) -> Self {
        Self { lo, hi }
    }

    /// 收进区间。`lo > hi`（配置写反）时按 `lo` 处理 —— 不 panic。
    pub fn apply(self, v: T) -> T {
        if v < self.lo {
            self.lo
        } else if v > self.hi {
            self.hi
        } else {
            v
        }
    }

    pub fn contains(self, v: T) -> bool {
        v >= self.lo && v <= self.hi
    }
}

/// 数据刷新频率区间（Hz）。上限同时是「地图记录间隔最多 1 秒的数据帧数」与
/// 「记录采样率最高 = 一个主循环节拍」的上限。
pub const REFRESH_HZ_RANGE: Clamp<u32> = Clamp::new(REFRESH_HZ_MIN, REFRESH_HZ_MAX);
/// 环形缓冲槽数区间（`2..=256`）：1 槽时"下一个待写槽"就是正在读的那个。
pub const RING_SLOTS_RANGE: Clamp<u32> =
    Clamp::new(wp8f_ring::Ring::<()>::MIN_SLOTS as u32, RING_SLOTS_MAX);
