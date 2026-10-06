//! F-18 风格机动告警声（maneuver tone）。
//!
//! 机动告警参数 `m = MAX(攻角比, 过载比, 超速项)`（正负向各按自身限值），
//! 越接近临界蜂鸣越密集；`m ≥ 100%` 转连续长鸣：
//!
//! | 区间 | 行为 |
//! |------|------|
//! | m < 67% | 静默（起音阈值 70%，迟滞至 67% 才停） |
//! | 70% ≤ m < 100% | 蜂鸣，频率 2Hz → 12Hz 线性加密 |
//! | m ≥ 100% | 连续长鸣（回落到 97% 以下恢复蜂鸣） |
//!
//! 与语音告警**并存**：机动告警声是独立座舱音效层，不进语音冷却/闪烁 X 体系。
//! 时间由调用方注入（`Instant`），便于单元测试。
//!
//! **表速门限**：表速 ≤ [`MIN_IAS_KMH`]（64 km/h）时参数压到 0 —— 地面滑跑、落地滑行时
//! 攻角/过载比容易虚假触阈，低速一律不出声；状态机照常推进，长鸣会正常释放。

use std::time::{Duration, Instant};

/// 起音阈值（机动告警参数）
pub const START_RATIO: f64 = 0.70;
/// 停音阈值（迟滞：起音 70%，停音 67%）
pub const STOP_RATIO: f64 = 0.67;
/// 长鸣阈值
pub const SOLID_RATIO: f64 = 1.0;
/// 长鸣释放阈值（迟滞：长鸣 100%，释放 97%）
pub const SOLID_RELEASE_RATIO: f64 = 0.97;
/// 蜂鸣频率下限（m = 起音阈值 时）
pub const BEEP_HZ_MIN: f64 = 2.0;
/// 蜂鸣频率上限（m = 长鸣阈值 时）
pub const BEEP_HZ_MAX: f64 = 12.0;
/// 超速项起点：MAX(表速/VNE, 马赫/MNE) 达到该比例即进入映射段（落点=起音阈值）
pub const SPEED_START_RATIO: f64 = 0.95;
/// 超速项斜率 = (长鸣阈值 − 起音阈值) / (1.0 − 起点) = 0.30/0.05 = 6
pub const SPEED_SLOPE: f64 = (SOLID_RATIO - START_RATIO) / (1.0 - SPEED_START_RATIO);
/// 机动告警声的最低表速（km/h）：**表速 ≤ 本值一律不出声**。
/// 地面滑跑/落地滑行时低速大攻角、过载比也容易顶到阈值，卡一道速度门限免得一直响。
pub const MIN_IAS_KMH: f64 = 64.0;

/// 超速项映射（**连续线，不硬切**）：`m = 起音阈值 + (r − 0.95) × 6`。
/// - r = 0.95 → 起音阈值（70%）；r = 1.00 → 长鸣阈值（100%）；r > 1 越限 → >1 长鸣
/// - r 低于起点自然掉到阈值下（94.5% 恰为停音阈值），tone 自带迟滞覆盖 0.945~0.950，
///   避免在 95% 边界随速度抖动反复开关
/// - 下限夹到 0，低速时不干扰攻角/过载项
pub fn speed_margin(speed_ratio: f64) -> f64 {
    (START_RATIO + (speed_ratio - SPEED_START_RATIO) * SPEED_SLOPE).max(0.0)
}

/// 机动告警参数 `m = MAX(攻角比, 过载比, 超速项)`。
/// 攻角/过载正负向各按自身限值取比值（负向用 |值| / |负限值|）；限值 ≤ 0 时该向不参与。
/// `speed_ratio` 为 MAX(表速/VNE, 马赫/MNE)，经 `speed_margin` 线性映射后参与取最大。
pub fn maneuver_margin(
    aoa: f64,
    aoa_crit_pos: f64,
    aoa_crit_neg: f64,
    ny: f64,
    n_pos: f64,
    n_neg: f64,
    speed_ratio: f64,
) -> f64 {
    let aoa_r = if aoa >= 0.0 {
        if aoa_crit_pos > 0.0 {
            aoa / aoa_crit_pos
        } else {
            0.0
        }
    } else {
        let lim = aoa_crit_neg.abs();
        if lim > 0.0 {
            aoa.abs() / lim
        } else {
            0.0
        }
    };
    let g_r = if ny >= 0.0 {
        if n_pos > 0.0 {
            ny / n_pos
        } else {
            0.0
        }
    } else {
        let lim = n_neg.abs();
        if lim > 0.0 {
            ny.abs() / lim
        } else {
            0.0
        }
    };
    aoa_r.max(g_r).max(speed_margin(speed_ratio))
}

/// 表速门限：`ias_kmh > MIN_IAS_KMH` 时原样返回，否则压到 0（静默）。
///
/// 只压**参数**、不跳过状态机推进 —— 收到 0 会走正常的停音/释放长鸣分支
/// （见 [`ManeuverTone::update`]），所以空中拉出长鸣后落地滑行不会把长鸣卡住。
pub fn gate_by_ias(margin: f64, ias_kmh: f64) -> f64 {
    if ias_kmh > MIN_IAS_KMH {
        margin
    } else {
        0.0
    }
}

/// 蜂鸣间隔：m 从 70% → 100% 时频率 2Hz → 12Hz 线性加密
pub fn beep_period(margin: f64) -> Duration {
    let t = ((margin - START_RATIO) / (SOLID_RATIO - START_RATIO)).clamp(0.0, 1.0);
    let hz = BEEP_HZ_MIN + (BEEP_HZ_MAX - BEEP_HZ_MIN) * t;
    Duration::from_secs_f64(1.0 / hz)
}

/// 每帧输出给音频层的指令
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ToneOutput {
    /// 响一声蜂鸣
    pub beep: bool,
    /// 进入长鸣（启动无缝循环音）
    pub solid_start: bool,
    /// 退出长鸣（停止循环音）
    pub solid_stop: bool,
}

/// 机动告警声状态机（迟滞防抖，节拍调度）
#[derive(Debug, Default)]
pub struct ManeuverTone {
    active: bool,
    solid: bool,
    next_beep: Option<Instant>,
}

impl ManeuverTone {
    pub fn new() -> Self {
        Self::default()
    }

    /// 每帧推进。返回该帧音频指令（可同时出现 solid_stop + beep：长鸣切回蜂鸣）。
    pub fn update(&mut self, margin: f64, now: Instant) -> ToneOutput {
        let mut out = ToneOutput::default();

        if self.solid {
            if margin < SOLID_RELEASE_RATIO {
                self.solid = false;
                out.solid_stop = true;
                self.active = margin >= STOP_RATIO;
                self.next_beep = if self.active { Some(now) } else { None };
            } else {
                return out; // 长鸣保持
            }
        } else if margin >= SOLID_RATIO {
            self.solid = true;
            self.active = true;
            self.next_beep = None;
            out.solid_start = true;
            return out;
        }

        if !self.active && margin >= START_RATIO {
            self.active = true;
            self.next_beep = Some(now); // 起音立即响第一声
        } else if self.active && margin < STOP_RATIO {
            self.active = false;
            self.next_beep = None;
        }

        if self.active && self.next_beep.map_or(true, |t| now >= t) {
            out.beep = true;
            self.next_beep = Some(now + beep_period(margin));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn margin_takes_worst_of_aoa_and_g_both_directions() {
        // 正向攻角 80% > 正过载 50% → 0.8
        let m = maneuver_margin(8.0, 10.0, -8.0, 5.0, 10.0, -8.0, 0.0);
        assert!((m - 0.8).abs() < 1e-9, "{m}");
        // 负过载 -7.2/-8 = 0.9 主导
        let m = maneuver_margin(2.0, 10.0, -8.0, -7.2, 10.0, -8.0, 0.0);
        assert!((m - 0.9).abs() < 1e-9, "{m}");
        // 负攻角 -7.6/-8 = 0.95 主导
        let m = maneuver_margin(-7.6, 10.0, -8.0, 1.0, 10.0, -8.0, 0.0);
        assert!((m - 0.95).abs() < 1e-9, "{m}");
        // 限值缺失（≤0）该向不参与
        assert_eq!(maneuver_margin(50.0, 0.0, 0.0, 50.0, 0.0, 0.0, 0.0), 0.0);
    }

    #[test]
    fn speed_margin_maps_95_to_start_and_100_to_solid() {
        // 端点：95% → 起音阈值，100% → 长鸣阈值
        assert!((speed_margin(0.95) - START_RATIO).abs() < 1e-9);
        assert!((speed_margin(1.0) - SOLID_RATIO).abs() < 1e-9);
        // 迟滞锚点：94.5% → 停音阈值（连续线，不硬切）
        assert!((speed_margin(0.945) - STOP_RATIO).abs() < 1e-9);
        // 越限 → 超过 1（长鸣）；远低于起点 → 夹到 0 不干扰他项
        assert!(speed_margin(1.05) > 1.0);
        assert_eq!(speed_margin(0.5), 0.0);
    }

    #[test]
    fn speed_term_joins_the_max() {
        // 超速项主导：攻角 20%、过载 10%，超速比 97% → 0.70 + 0.02×6 = 0.82
        let m = maneuver_margin(2.0, 10.0, -8.0, 1.0, 10.0, -8.0, 0.97);
        assert!((m - 0.82).abs() < 1e-9, "{m}");
        // 过载主导：超速比 80%（速度项夹到 0）
        let m = maneuver_margin(1.0, 10.0, -8.0, 9.0, 10.0, -8.0, 0.80);
        assert!((m - 0.9).abs() < 1e-9, "{m}");
        // 越限超速 → 长鸣段（>1）
        let m = maneuver_margin(1.0, 10.0, -8.0, 1.0, 10.0, -8.0, 1.02);
        assert!(m > 1.0, "{m}");
    }

    /// 表速门限：地面滑跑/落地滑行不出声（≤ 64 km/h 压到 0），空中原样透传。
    #[test]
    fn ias_gate_mutes_at_or_below_64_kmh() {
        assert_eq!(gate_by_ias(0.8, 0.0), 0.0, "停机/滑行");
        assert_eq!(gate_by_ias(1.2, 64.0), 0.0, "正好 64：不出声（严格大于才触发）");
        assert!((gate_by_ias(0.8, 64.1) - 0.8).abs() < 1e-9, "刚过门限：原样");
        assert!((gate_by_ias(1.5, 300.0) - 1.5).abs() < 1e-9, "长鸣段也原样透传");
        // 状态机收 0 会正常复位：空中长鸣 → 落地滑行 = 停音指令，不会卡住
        let t0 = Instant::now();
        let mut tone = ManeuverTone::new();
        assert!(tone.update(gate_by_ias(1.0, 400.0), t0).solid_start);
        let o = tone.update(gate_by_ias(1.0, 30.0), t0 + Duration::from_millis(50));
        assert!(o.solid_stop, "滑行时应当释放长鸣：{o:?}");
    }

    #[test]
    fn beep_period_speeds_up_toward_solid() {
        let p70 = beep_period(0.70);
        let p95 = beep_period(0.95);
        let p100 = beep_period(1.0);
        assert!(p70 > p95 && p95 > p100);
        assert!((p70.as_secs_f64() - 0.5).abs() < 1e-6, "70% → 2Hz");
        assert!((p100.as_secs_f64() - 1.0 / 12.0).abs() < 1e-6, "100% → 12Hz");
    }

    #[test]
    fn state_machine_with_hysteresis() {
        let t0 = Instant::now();
        let mut tone = ManeuverTone::new();
        let ms = |n: u64| t0 + Duration::from_millis(n);

        // 起音阈值以下：静默
        assert_eq!(tone.update(0.65, ms(0)), ToneOutput::default());
        // 起音：立即第一声（0.70 → 2Hz = 500ms 节拍）
        let o = tone.update(0.70, ms(10));
        assert!(o.beep && !o.solid_start, "{o:?}");
        // 节拍未到：无指令
        assert_eq!(tone.update(0.75, ms(60)), ToneOutput::default());
        // 节拍到：第二声
        let o = tone.update(0.75, ms(520));
        assert!(o.beep, "{o:?}");
        // 迟滞：0.68 仍在响（停音阈值 0.67）
        let o = tone.update(0.68, ms(800));
        assert!(o.beep, "0.68 应继续蜂鸣 {o:?}");
        // 低于停音阈值：停止
        assert_eq!(tone.update(0.65, ms(810)), ToneOutput::default());
        // 再次起音
        assert!(tone.update(0.73, ms(900)).beep);
        // 长鸣
        let o = tone.update(1.0, ms(1000));
        assert!(o.solid_start && !o.beep, "{o:?}");
        // 长鸣保持（0.98 ≥ 释放阈值 0.97）：无指令
        assert_eq!(tone.update(0.98, ms(1100)), ToneOutput::default());
        // 释放：长鸣停止并立即恢复蜂鸣
        let o = tone.update(0.96, ms(1200));
        assert!(o.solid_stop && o.beep, "{o:?}");
        // 大幅回落直接静默
        assert_eq!(tone.update(0.5, ms(1300)), ToneOutput::default());
    }

    #[test]
    fn solid_from_silent_skips_beeps() {
        let t0 = Instant::now();
        let mut tone = ManeuverTone::new();
        let o = tone.update(1.2, t0);
        assert!(o.solid_start && !o.beep, "{o:?}");
    }
}
