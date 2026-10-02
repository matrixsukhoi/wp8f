//! 帧调度（纯）：主循环只喂 `now`，帧号 / 实测间隔 / 丢帧判定 / 地图采样闸门全在这里算。
//!
//! 抽出来的理由：这些整数运算最容易出错（第 0 帧、跳拍、时间回退、闸门初值下溢），
//! 而它们原本内联在 async 主循环里 —— 测试只能把那段循环逐字复刻一份，改主循环忘了改副本
//! 照样绿。现在测试直接喂时间序列，不需要 tokio、不需要 async。

/// 一拍的调度结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tick {
    /// 帧号（自 `ts_start` 起按标称节拍数出来的整数帧）
    pub frame: u64,
    /// 与本拍同一基准的 UNIX 纳秒 —— `DisplayData.timestamp`（飞行记录的时间列）
    pub timestamp_ns: u64,
    /// 距上一次**成功**拍的实测间隔 = 中间跳过的帧数 × 标称节拍
    pub interval_ns: u64,
    /// 标称节拍（配置的刷新率）："标称"类换算（如飞行时长）用它，不用实测值
    pub nominal_interval_ns: u64,
    /// > 0 = 本拍采一次地图对象；值 = 该采样窗口的时长
    pub mapobj_interval_ns: u64,
    /// 实测间隔 ≥ 2 个标称节拍（主循环迟到）
    pub dropped: bool,
}

/// 帧节拍器：`peek()` 只看不推进，两处状态转移（[`advance`](Self::advance) /
/// [`accept`](Self::accept)）由主循环显式调用。
pub struct FrameClock {
    ts_start: u64,
    nominal_interval_ns: u64,
    map_obj_frames: u64,
    /// 上一次**成功**的帧号。解析失败的拍不推进它，于是下一次的实测间隔覆盖那段空档
    last_frame: u64,
    /// 下一次采地图对象的帧号；初值 = 一个采样周期，所以第 0 帧不触发
    next_mapobj_frame: u64,
}

impl FrameClock {
    /// `map_obj_frames` = 地图记录间隔（帧，>= 1，0 兜到 1）。
    pub fn new(ts_start: u64, nominal_interval_ns: u64, map_obj_frames: u64) -> Self {
        let map_obj_frames = map_obj_frames.max(1);
        Self {
            ts_start,
            nominal_interval_ns,
            map_obj_frames,
            last_frame: 0,
            next_mapobj_frame: map_obj_frames,
        }
    }

    /// 本拍该用哪些数（**纯**，不改状态）。`interval_ns == 0` = 与上一拍同一帧（定时器提前醒来），
    /// 调用方跳过后重等即可 —— 那样也不消耗地图采样机会。
    pub fn peek(&self, now: u64) -> Tick {
        let frame = now.saturating_sub(self.ts_start) / self.nominal_interval_ns;
        let interval_ns = frame.saturating_sub(self.last_frame) * self.nominal_interval_ns;
        let mapobj_interval_ns = if interval_ns > 0 && frame >= self.next_mapobj_frame {
            // 闸门初值 = 一个采样周期 → 内层不下溢，`elapsed` 恒 ≥ 一个周期
            let elapsed = frame - (self.next_mapobj_frame - self.map_obj_frames);
            elapsed * self.nominal_interval_ns
        } else {
            0
        };
        Tick {
            frame,
            timestamp_ns: now,
            interval_ns,
            nominal_interval_ns: self.nominal_interval_ns,
            mapobj_interval_ns,
            dropped: interval_ns >= 2 * self.nominal_interval_ns,
        }
    }

    /// 本拍**开始取数据**：消耗掉这次地图采样机会。闸门与旧实现同序（取数据之前推进），
    /// 所以解析失败的拍同样会消耗一次采样。
    pub fn advance(&mut self, tick: &Tick) {
        if tick.mapobj_interval_ns > 0 {
            self.next_mapobj_frame += self.map_obj_frames;
        }
    }

    /// 本拍**真的产出了帧**（主循环末尾）：只有它推进 `last_frame`。
    pub fn accept(&mut self, tick: &Tick) {
        self.last_frame = tick.frame;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 100_000_000; // 标称节拍 10 Hz
    const T0: u64 = 1_000;

    fn clock(frames: u64) -> FrameClock {
        FrameClock::new(T0, S, frames)
    }

    /// 一拍一次的稳态：帧号逐拍 +1，实测间隔 = 标称值，不报丢帧。
    #[test]
    fn steady_ticks_are_nominal() {
        let mut c = clock(4);
        for k in 1..6u64 {
            let t = c.peek(T0 + k * S);
            assert_eq!(t.frame, k);
            assert_eq!(t.interval_ns, S);
            assert_eq!(t.nominal_interval_ns, S, "标称节拍原样带出去");
            assert!(!t.dropped);
            assert_eq!(t.timestamp_ns, T0 + k * S, "本拍时间戳 = 喂进来的 now");
            c.accept(&t);
        }
    }

    /// 定时器提前醒来（还没到下一个节拍）：不是一拍，也不消耗地图采样机会。
    #[test]
    fn early_wake_is_not_a_tick() {
        let c = clock(2);
        let early = c.peek(T0);
        assert_eq!((early.frame, early.interval_ns), (0, 0));
        assert_eq!(early.mapobj_interval_ns, 0, "提前醒来不采样");

        let t = c.peek(T0 + S);
        assert_eq!((t.frame, t.interval_ns), (1, S));
    }

    /// 迟到：跳过的帧数累进同一拍的实测间隔，并置 `dropped`。
    #[test]
    fn skipped_periods_accumulate_and_flag_dropped() {
        let c = clock(8);
        let t = c.peek(T0 + 3 * S);
        assert_eq!(t.frame, 3);
        assert_eq!(t.interval_ns, 3 * S);
        assert!(t.dropped, "3 个标称节拍 > 2 个 → 报丢帧");

        // 恰好两拍算丢帧边界（与旧实现同口径：>= 2 倍）
        let c = clock(8);
        let t = c.peek(T0 + 2 * S);
        assert_eq!(t.interval_ns, 2 * S);
        assert!(t.dropped);
    }

    /// `last_frame` 只由 `accept()` 推进：解析失败的拍不推进 → 空档累进下一拍的间隔。
    #[test]
    fn last_frame_advances_only_on_accept() {
        let mut c = clock(8);
        let failed = c.peek(T0 + 2 * S); // 这一拍取数据失败：不 accept
        assert_eq!(failed.frame, 2);

        let next = c.peek(T0 + 3 * S);
        assert_eq!(next.interval_ns, 3 * S, "空档算进下一拍的间隔（fuel/SMA 的积分步长）");

        c.accept(&next);
        assert_eq!(c.peek(T0 + 4 * S).interval_ns, S, "接上之后回到标称节拍");
    }

    /// 地图闸门：每 `map_obj_frames` 帧恰好一次，窗口时长恒 = 一个采样周期（不是 2 倍），
    /// 第 0 帧不触发（初值为 0 会让内层下溢）。
    #[test]
    fn mapobj_gate_fires_once_per_period() {
        const FRAMES: u64 = 3;
        let mut c = clock(FRAMES);
        let mut trips: Vec<(u64, u64)> = Vec::new();
        for k in 0..=(FRAMES * 2) {
            let t = c.peek(T0 + k * S);
            if t.mapobj_interval_ns > 0 {
                trips.push((t.frame, t.mapobj_interval_ns));
                c.advance(&t);
            }
            c.accept(&t);
        }
        assert_eq!(
            trips,
            vec![(FRAMES, FRAMES * S), (FRAMES * 2, FRAMES * S)],
            "0..={} 帧内只该触发两次，窗口恒为一个采样周期",
            FRAMES * 2
        );
    }

    /// 时间回退（系统时钟被改）不 panic、不下溢，只是当作"还没到下一拍"。
    #[test]
    fn backwards_time_does_not_underflow() {
        let c = clock(3);
        let t = c.peek(T0.saturating_sub(10 * S));
        assert_eq!((t.frame, t.interval_ns, t.dropped), (0, 0, false));
    }
}
