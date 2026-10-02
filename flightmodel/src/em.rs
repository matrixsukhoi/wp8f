//! EM（能量机动）求解器：单位剩余功率 Ps、瞬时/持续盘旋率、corner speed。
//!
//! 全部建立在已统一的计算口径之上：
//! - 大气/声速：Dagor `gamePhys/props/atmosphere`（g = 9.81）
//! - 阻力：Dagor 极曲线 `get_drag_coefficient`（含 CX0 马赫波阻、诱导阻力、失速后增长）
//! - CLmax(M)：`cl_max_mach_corrected`（CY_MAX 马赫曲线）
//! - 结构限制：`allowed_load_factor`（H4 口径，n = sign(F)·(2|F|/(m·g) − 1)）
//!
//! 单位：SI（m、m/s、N、rad/s），盘旋率对外接口返回 deg/s。

use crate::aero::GRAVITY;
use crate::flight_model::FlightModel;

/// 速度矢量转动率 ω [rad/s] 与载荷因数 n 的关系：`ω = g·√(n²−1) / V`
#[inline]
pub fn turn_rate_rad(load_factor: f64, velocity_ms: f64) -> f64 {
    if velocity_ms <= 0.0 || load_factor <= 1.0 {
        return 0.0;
    }
    GRAVITY * (load_factor * load_factor - 1.0).sqrt() / velocity_ms
}

/// 由盘旋率反解载荷因数：`n = √((ω·V/g)² + 1)`（EM 图网格采样用）
#[inline]
pub fn load_factor_for_turn(omega_rad: f64, velocity_ms: f64) -> f64 {
    let x = omega_rad * velocity_ms / GRAVITY;
    (x * x + 1.0).sqrt()
}

impl FlightModel {
    /// 指定载荷因数下的单位剩余功率
    /// `Ps = (T − D(n·W))·V / W` [m/s]。`use_ab` 选择加力/军推。
    pub fn ps_at_load(
        &self,
        altitude_m: f64,
        velocity_ms: f64,
        weight_n: f64,
        load_factor: f64,
        use_ab: bool,
    ) -> f64 {
        if velocity_ms <= 0.0 || weight_n <= 0.0 {
            return 0.0;
        }
        let thrust = if use_ab {
            self.afterburner_thrust(altitude_m, velocity_ms)
        } else {
            self.thrust(altitude_m, velocity_ms)
        };
        let drag = self.drag_for_lift(altitude_m, velocity_ms, load_factor * weight_n);
        (thrust - drag) * velocity_ms / weight_n
    }

    /// EM 口径瞬时可用过载：`min(结构限制, 气动 CLmax(M))`
    /// （FBW 限载字段已弃用，不参与）
    pub fn em_available_load_factor(&self, altitude_m: f64, velocity_ms: f64, weight_n: f64) -> f64 {
        let n_aero = self.available_load_factor(altitude_m, velocity_ms, weight_n);
        let n_struct = self.allowed_load_factor(weight_n / GRAVITY).1;
        let n_struct = if n_struct > 0.0 { n_struct } else { f64::MAX };
        n_aero.min(n_struct)
    }

    /// 瞬时盘旋：`(ω [deg/s], n)` —— 拉满可用过载的转动率
    pub fn instant_turn(&self, altitude_m: f64, velocity_ms: f64, weight_n: f64) -> (f64, f64) {
        let n = self.em_available_load_factor(altitude_m, velocity_ms, weight_n);
        (turn_rate_rad(n, velocity_ms).to_degrees(), n)
    }

    /// 持续盘旋：`(ω [deg/s], n)` —— 解 `Ps(n) = 0`（n ∈ [1, n_av] 二分）。
    /// 平飞都维持不了时返回 (0, 1)；连极限过载都拉得住时返回瞬时盘旋值。
    pub fn sustained_turn(
        &self,
        altitude_m: f64,
        velocity_ms: f64,
        weight_n: f64,
        use_ab: bool,
    ) -> (f64, f64) {
        let n_av = self.em_available_load_factor(altitude_m, velocity_ms, weight_n);
        if n_av <= 1.0 {
            return (0.0, 1.0);
        }
        let ps = |n: f64| self.ps_at_load(altitude_m, velocity_ms, weight_n, n, use_ab);
        if ps(1.0) <= 0.0 {
            return (0.0, 1.0);
        }
        if ps(n_av) >= 0.0 {
            return (turn_rate_rad(n_av, velocity_ms).to_degrees(), n_av);
        }
        let (mut lo, mut hi) = (1.0_f64, n_av);
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if ps(mid) > 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let n = 0.5 * (lo + hi);
        (turn_rate_rad(n, velocity_ms).to_degrees(), n)
    }

    /// 等高度 corner speed [m/s]：瞬时盘旋率最大的速度（数值扫描）
    pub fn corner_speed(&self, altitude_m: f64, weight_n: f64) -> f64 {
        let mut best_v = 0.0;
        let mut best_w = -1.0;
        let mut v = 50.0;
        while v <= 750.0 {
            let (w, _) = self.instant_turn(altitude_m, v, weight_n);
            if w > best_w {
                best_w = w;
                best_v = v;
            }
            v += 2.5;
        }
        best_v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aero::GRAVITY as G;

    #[test]
    fn turn_rate_and_load_factor_are_inverse() {
        // ω ↔ n 互为反函数
        let v: f64 = 250.0;
        for n in [1.5f64, 3.0, 6.0, 9.0] {
            let w = turn_rate_rad(n, v);
            let back = load_factor_for_turn(w, v);
            assert!((back - n).abs() < 1e-9, "n={} roundtrip {}", n, back);
        }
        // 经典公式：ω = g·√(n²−1)/V
        let n: f64 = 5.0;
        let expect = G * (n * n - 1.0).sqrt() / v;
        assert!((turn_rate_rad(n, v) - expect).abs() < 1e-9);
        assert_eq!(turn_rate_rad(1.0, v), 0.0, "n<=1 无转动");
    }

    #[test]
    fn em_solver_sanity_jet() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let fm = crate::parse_aircraft("f_16c_block_50", data_dir).expect("parse f_16c");
        let weight_n = fm.takeoff_weight(50.0) * G;
        let alt = 0.0;

        // corner speed 在合理飞行范围内
        let vc = fm.corner_speed(alt, weight_n);
        assert!(vc > 50.0 && vc < 750.0, "corner speed {:.1} m/s out of range", vc);

        // corner speed 处瞬时盘旋率应为全程最大
        let (w_vc, _) = fm.instant_turn(alt, vc, weight_n);
        let (w_lo, _) = fm.instant_turn(alt, 100.0, weight_n);
        let (w_hi, _) = fm.instant_turn(alt, 700.0, weight_n);
        assert!(w_vc >= w_lo && w_vc >= w_hi, "corner {:.2} vs {:.2}/{:.2}", w_vc, w_lo, w_hi);

        // 持续盘旋 ≤ 瞬时盘旋；Ps 随载荷单调下降
        let v = vc;
        let (w_inst, n_av) = fm.instant_turn(alt, v, weight_n);
        let (w_sus, n_sus) = fm.sustained_turn(alt, v, weight_n, true);
        assert!(n_sus <= n_av + 1e-9, "n_sus {:.2} > n_av {:.2}", n_sus, n_av);
        assert!(w_sus <= w_inst + 1e-6, "ω_sus {:.2} > ω_inst {:.2}", w_sus, w_inst);

        let ps = |n: f64| fm.ps_at_load(alt, v, weight_n, n, true);
        assert!(ps(1.0) > ps(2.0), "Ps 应随载荷下降");
        if w_sus > 0.0 {
            assert!(ps(n_sus).abs() < 0.5, "Ps(n_sus) 应≈0，got {:.3}", ps(n_sus));
        }
    }
}
