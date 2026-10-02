//! 无锁"最新值"环形缓冲区：单生产端写、任意消费端直读最新一版。
//!
//! 生产端 `next_slot()` 写槽、`commit()` 发布；消费端 `latest()` 取最新、`poll()` 统计落后版本数。
//! 全程无锁、无 `Arc`、无逐帧分配（槽在 `with_slots()` 时一次性分配）。
//! HUD/logger 的帧缓冲区（`wp8f_disp`）与数据链状态槽（`datalink`）共用这一个原语。
//!
//! # 契约
//! * **单生产端**：`next_slot()` 与 `commit()` 之间不互斥，两个线程同时写会互相踩。
//!   消费端数量不限，各持一个 `Cursor`。
//! * **载荷必须是 POD**：消费端拿到的是槽的借用，生产端绕满一圈后会改写同一槽。
//!   值类型只会读到新旧混合的数据（这一版作废，不影响下一版）；带堆指针的类型
//!   （`Vec`/`String`）会被读者在脚下释放，不能进环。
//!   （唯一例外：`disp` 的地图槽，4 Hz × 槽数 才追上读者 —— 理由见那里的槽声明。）
//! * **消费端不改动生产端状态**：读端只推进自己的 `Cursor`，写端不等待任何读者。

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub struct Ring<T> {
    slots: Box<[UnsafeCell<T>]>,
    /// 已发布的最新槽下标，只在 `0..slots` 内回绕
    write_index: AtomicUsize,
    /// 单调写计数：每次 `commit()` +1（只增不减；回绕判断只能靠它）
    writes: AtomicU64,
    /// 生产端重置代数：`reset()` 每次 +1（消费端据此识别断流）
    generation: AtomicU64,
}

// SAFETY: 槽的访问全靠上面的原子索引/计数同步（生产端只写未发布的槽、消费端只读已发布的槽）；
// `T: Send` 保证槽里的值能跨线程移动，`T: Sync` 保证共享出来的 `&T` 能跨线程读。
unsafe impl<T: Send> Send for Ring<T> {}
unsafe impl<T: Send + Sync> Sync for Ring<T> {}

/// 消费端游标：记住上次见到的写计数与代数（每个消费者一个）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    seen: u64,
    generation: u64,
}

/// 一次轮询的结果。
#[derive(Debug)]
pub struct Poll<'a, T> {
    /// 最新一版的借用；`None` = 自上次轮询以来没有新版本
    pub value: Option<&'a T>,
    /// 被跳过的中间版本数（`value` 之前的）
    pub skipped: u64,
    /// 检测到生产端 `reset()` → 旧游标作废
    pub reset: bool,
}

impl<T: Default> Default for Ring<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Default> Ring<T> {
    /// 默认槽数（配置里的缺省值就是它）：30 Hz 下 ≈ 1 秒的覆盖层。
    pub const SLOTS: usize = 32;
    /// 槽数下限：1 个槽时"下一个待写槽"就是正在被读的那个，环的语义不成立。
    pub const MIN_SLOTS: usize = 2;

    /// 默认槽数（[`Self::SLOTS`]）：datalink 的"最新值"槽与没配槽数的调用点用它。
    pub fn new() -> Self {
        Self::with_slots(Self::SLOTS)
    }

    /// `slots` 是槽数（下限 [`Self::MIN_SLOTS`]），一次分配完 —— 运行期不再分配。
    pub fn with_slots(slots: usize) -> Self {
        let n = slots.max(Self::MIN_SLOTS);
        let slots = (0..n)
            .map(|_| UnsafeCell::new(T::default()))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            slots,
            write_index: AtomicUsize::new(0),
            writes: AtomicU64::new(0),
            generation: AtomicU64::new(0),
        }
    }

    /// 槽数（消费者据此判断"落后几版算被覆盖"）。
    pub fn slots(&self) -> usize {
        self.slots.len()
    }

    /// 当前最新一版（消费端便捷入口）：不阻塞、不等待，从未发布过时是 `T::default()`。
    pub fn latest(&self) -> &T {
        let idx = self.write_index.load(Ordering::Acquire);
        // SAFETY: `write_index` 只指向已 `commit()` 的槽（初始为槽 0）。
        unsafe { &*self.slots[idx].get() }
    }

    /// 生产端：下一个待写槽（写完必须 `commit()` 才算发布）。
    pub fn next_slot(&self) -> &mut T {
        let idx = self.write_index.load(Ordering::Relaxed);
        let next = self.wrap(idx + 1);
        // SAFETY: 单生产端 —— 这个槽是下一个待发布槽，此刻除了本端没人写它。
        unsafe { &mut *self.slots[next].get() }
    }

    /// 生产端：写一版新值并发布（`next_slot()` + `commit()`）。
    pub fn publish(&self, value: T) {
        *self.next_slot() = value;
        self.commit();
    }

    /// 生产端：发布 `next_slot()` 写好的那一版。
    pub fn commit(&self) {
        let idx = self.write_index.load(Ordering::Relaxed);
        let next = self.wrap(idx + 1);
        self.write_index.store(next, Ordering::Release);
        // 计数必须在索引 store 之后发布：消费端先读索引、后读计数，看到 `writes = N+1`
        // 就必然能看到第 N+1 版的内容；反过来会读到"新计数 + 旧索引"，重复交付上一版。
        self.writes.fetch_add(1, Ordering::Release);
    }

    fn wrap(&self, idx: usize) -> usize {
        if idx >= self.slots.len() {
            0
        } else {
            idx
        }
    }

    pub fn writes(&self) -> u64 {
        self.writes.load(Ordering::Acquire)
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// 在生产端当前状态处的游标：只关心此后的版本（新建消费者时用，历史版本不补发）。
    pub fn cursor(&self) -> Cursor {
        Cursor {
            seen: self.writes.load(Ordering::Acquire),
            generation: self.generation.load(Ordering::Acquire),
        }
    }

    /// 推进代数：重连/重建时调用。消费端下次 `poll()` 会看到 `reset = true`。
    /// 本函数**不清空**值 —— 要不要顺带发布一份空值由调用方决定。
    pub fn reset(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// 消费端轮询：只在生产端有新版本时返回 `Some`，并统计跳过了几版。
    ///
    /// 检测到 `reset()` 时把当前版本当作重置后的第一版交付（`skipped = 0`）：
    /// 重置前的版本消费端本来就看不到，算成"跳过"既没意义、也会在极端时序下下溢。
    pub fn poll<'a>(&'a self, cursor: &mut Cursor) -> Poll<'a, T> {
        let gen = self.generation.load(Ordering::Acquire);
        if gen != cursor.generation {
            cursor.generation = gen;
            cursor.seen = self.writes.load(Ordering::Acquire);
            return Poll { value: Some(self.latest()), skipped: 0, reset: true };
        }
        // 先索引、后计数：顺序与 `commit()` 的发布顺序配对（见那里的注释）
        let idx = self.write_index.load(Ordering::Acquire);
        let total = self.writes.load(Ordering::Acquire);
        if total == cursor.seen {
            return Poll { value: None, skipped: 0, reset: false };
        }
        let skipped = total.saturating_sub(cursor.seen).saturating_sub(1);
        cursor.seen = total;
        // SAFETY: 同上 —— `idx` 指向已发布的槽。
        Poll { value: Some(unsafe { &*self.slots[idx].get() }), skipped, reset: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小 POD 载荷（真实载荷：`DisplayData` / `OwnState` / `Snap`）。
    #[derive(Debug, Clone, Copy, Default, PartialEq)]
    struct Cell {
        frame: u64,
    }

    fn ring() -> &'static Ring<Cell> {
        Box::leak(Box::new(Ring::new()))
    }

    fn write(r: &Ring<Cell>, from: u64, n: u64) {
        for i in 0..n {
            r.publish(Cell { frame: from + i });
        }
    }

    /// 槽数可配：绕圈按配置的槽数回绕，`slots()` 把它报给消费者。
    #[test]
    fn slot_count_is_configurable_and_clamped() {
        let r = Ring::<Cell>::with_slots(4);
        assert_eq!(r.slots(), 4);
        let mut c = r.cursor();
        write(&r, 1, 9); // 4 槽绕两圈多
        let out = r.poll(&mut c);
        assert_eq!(out.value.expect("新版本").frame, 9, "绕圈后仍是最新一版");
        assert_eq!(r.writes(), 9, "写计数与槽数无关");

        // 1 个槽时"下一个待写槽"就是正在被读的那个 → 兜到下限 2
        assert_eq!(Ring::<Cell>::with_slots(1).slots(), Ring::<Cell>::MIN_SLOTS);
        assert_eq!(Ring::<Cell>::with_slots(0).slots(), Ring::<Cell>::MIN_SLOTS);
        assert_eq!(Ring::<Cell>::new().slots(), Ring::<Cell>::SLOTS, "默认 = SLOTS");
    }

    #[test]
    fn latest_is_default_before_any_write() {
        let r = ring();
        assert_eq!(r.latest(), &Cell::default());
        assert_eq!(r.writes(), 0);
    }

    #[test]
    fn poll_returns_latest_and_counts_skips() {
        let r = ring();
        let mut c = Cursor::default();
        assert!(r.poll(&mut c).value.is_none(), "没写过不该报新版本");

        write(r, 1, 5);
        let out = r.poll(&mut c);
        assert_eq!(out.value.expect("有新版本").frame, 5, "只取最新一版");
        assert_eq!(out.skipped, 4, "其余 4 版计入 skipped");
        assert!(!out.reset);

        let out = r.poll(&mut c);
        assert!(out.value.is_none(), "写端没动不该重复报");
        assert_eq!((out.skipped, out.reset), (0, false));

        write(r, 6, 3);
        let out = r.poll(&mut c);
        assert_eq!(out.value.expect("新版本").frame, 8);
        assert_eq!(out.skipped, 2, "增量按上次轮询之后的写入计数");
    }

    #[test]
    fn cursor_starts_at_the_current_state() {
        let r = ring();
        write(r, 1, 5);
        r.reset();
        write(r, 6, 2);

        let mut c = r.cursor();
        let out = r.poll(&mut c);
        assert!(out.value.is_none() && !out.reset, "同步点：历史版本不补发，创建前的重置不算本次的 reset");

        write(r, 8, 4);
        let out = r.poll(&mut c);
        assert_eq!(out.value.expect("新版本").frame, 11);
        assert_eq!(out.skipped, 3, "只统计游标之后的写入");
    }

    #[test]
    fn reset_invalidates_cursor_without_underflow() {
        let r = ring();
        let mut c = Cursor::default();
        write(r, 1, 5);
        assert_eq!(r.poll(&mut c).skipped, 4);

        r.reset();
        let out = r.poll(&mut c);
        assert!(out.reset, "消费端必须能识别重置");
        assert_eq!(out.skipped, 0, "重置不该算成跳过，更不该下溢");
        assert_eq!(out.value.expect("重置后仍能取到最新一版").frame, 5);

        write(r, 100, 3);
        let out = r.poll(&mut c);
        assert!(!out.reset);
        assert_eq!(out.value.expect("新版本").frame, 102);
        assert_eq!(out.skipped, 2, "重置后只按新的写入增量计数");

        for _ in 0..3 {
            r.reset();
            let out = r.poll(&mut c);
            assert!(out.reset);
            assert_eq!((out.skipped, out.reset), (0, true), "反复重置不累加丢帧");
        }
    }

    #[test]
    fn wrap_around_reuses_slots_and_keeps_latest() {
        let r = ring();
        let mut c = r.cursor();
        let n = Ring::<Cell>::SLOTS as u64 * 2 + 8;
        write(r, 1, n);

        let out = r.poll(&mut c);
        assert_eq!(out.value.expect("新版本").frame, n, "绕圈后最新一版仍是最后一版");
        assert_eq!(out.skipped, n - 1, "落后超过容量时 skipped 照实累计");
        assert_eq!(r.writes(), n, "写计数单调，不受索引回绕影响");
        assert_eq!(r.latest().frame, n, "latest() 直读已发布的槽");
    }
}
