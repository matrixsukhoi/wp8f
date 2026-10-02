use crate::parser::BlkxValue;
use std::collections::HashMap;

const WING_AREA_KEYS: [&str; 15] = [
    "Main", "In", "Mid", "Out", "Aileron",
    "LeftIn", "LeftMid", "LeftOut", "LeftAileron", "LeftCut",
    "RightIn", "RightMid", "RightOut", "RightAileron", "RightCut",
];

// ---------------------------------------------------------------------------
// 大气模型：与 Dagor 引擎 gamePhys/props/atmosphere.{h,cpp} 完全一致
// （4 阶多项式系数 + hMax=18300m 之上的 1/h 折叠衰减 + 声速 20.1·√T）。
// 游戏内所有飞行计算都基于该模型，wp8f 对齐它以保证结果一致。
// ---------------------------------------------------------------------------
/// Dagor 最大高度 [m]：超过后密度/压力按 hMax/h 衰减，温度保持不变。
pub const DAGOR_H_MAX: f64 = 18300.0;
/// 海平面标准压力 [Pa]、温度 [K]、密度 [kg/m³]（atmosphere.cpp:15-17）
pub const DAGOR_P0: f64 = 101300.0;
pub const DAGOR_T0: f64 = 288.16;
pub const DAGOR_RO0: f64 = 1.225;

/// 地球重力加速度 [m/s²] —— **wp8f 全项目唯一 g 值**，与 Dagor 一致
/// （`gamePhys/props/atmosphere.cpp:19` `_g = 9.81f`）。
/// 所有 G/过载/能量/推重比计算必须引用本常量，禁止散写 9.80/9.78。
pub const GRAVITY: f64 = 9.81;

// atmosphere.h:27-29 的多项式系数（poly = (((c4·v+c3)·v+c2)·v+c1)·v+c0）
const DENSITY_COEFFS: [f64; 5] = [1.0, -9.59387e-05, 3.53118e-09, -5.83556e-14, 2.28719e-19];
const PRESSURE_COEFFS: [f64; 5] = [1.0, -0.000118441, 5.6763e-09, -1.3738e-13, 1.60373e-18];
const TEMPERATURE_COEFFS: [f64; 5] = [1.0, -2.27712e-05, 2.18069e-10, -5.71104e-14, 3.97306e-18];

#[inline]
fn dagor_poly(tab: &[f64; 5], v: f64) -> f64 {
    (((tab[4] * v + tab[3]) * v + tab[2]) * v + tab[1]) * v + tab[0]
}

fn get_f64_from(data: &HashMap<String, BlkxValue>, key: &str) -> Option<f64> {
    data.get(key).and_then(|v| v.as_f64())
}

#[derive(Debug, Clone, Default)]
pub struct MachCorrection {
    pub mach_crit: f64,
    pub mach_max: f64,
    pub mult_mach_max: f64,
    pub mult_line_coeff: f64,
    pub mult_limit: f64,
}

impl MachCorrection {
    pub fn read_from(fm_data: &HashMap<String, BlkxValue>, prefix: &str, idx: usize) -> Self {
        Self {
            mach_crit: get_f64_from(fm_data, &format!("{}.MachCrit{}", prefix, idx))
                .unwrap_or(0.0),
            mach_max: get_f64_from(fm_data, &format!("{}.MachMax{}", prefix, idx)).unwrap_or(0.0),
            mult_mach_max: get_f64_from(fm_data, &format!("{}.MultMachMax{}", prefix, idx))
                .unwrap_or(1.0),
            mult_line_coeff: get_f64_from(fm_data, &format!("{}.MultLineCoeff{}", prefix, idx))
                .unwrap_or(0.0),
            mult_limit: get_f64_from(fm_data, &format!("{}.MultLimit{}", prefix, idx))
                .unwrap_or(1.0),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PolarData {
    pub oswalds_efficiency: f64,
    pub line_cl_coeff: f64,
    pub cl0: f64,
    pub alpha_crit_high: f64,
    pub alpha_crit_low: f64,
    pub cl_crit_high: f64,
    pub cl_crit_low: f64,
    pub cd_min: f64,
    pub after_crit_parab_angle: f64,
    pub after_crit_decline_coeff: f64,
    pub after_crit_max_dist_angle: f64,
    pub cl_after_crit_high: f64,
    pub cl_after_crit_low: f64,
    pub cx_after_coeff: f64,
    pub mach_corrections: Vec<MachCorrection>,
}

impl Default for PolarData {
    fn default() -> Self {
        Self {
            oswalds_efficiency: 0.8,
            line_cl_coeff: 0.1,
            cl0: 0.0,
            alpha_crit_high: 15.0,
            alpha_crit_low: -15.0,
            cl_crit_high: 1.2,
            cl_crit_low: -1.2,
            cd_min: 0.01,
            after_crit_parab_angle: 10.0,
            after_crit_decline_coeff: 0.01,
            after_crit_max_dist_angle: 30.0,
            cl_after_crit_high: 0.5,
            cl_after_crit_low: -0.5,
            cx_after_coeff: 0.01,
            mach_corrections: Vec::new(),
        }
    }
}

impl PolarData {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn read_from(fm_data: &HashMap<String, BlkxValue>, prefix: &str) -> Self {
        let mut polar = Self::new();

        polar.oswalds_efficiency =
            get_f64_from(fm_data, &format!("{}.OswaldsEfficiencyNumber", prefix)).unwrap_or(0.8);
        polar.line_cl_coeff =
            get_f64_from(fm_data, &format!("{}.lineClCoeff", prefix)).unwrap_or(0.1);
        polar.cl0 = get_f64_from(fm_data, &format!("{}.Cl0", prefix)).unwrap_or(0.0);

        polar.alpha_crit_high =
            get_f64_from(fm_data, &format!("{}.alphaCritHigh", prefix)).unwrap_or(15.0);
        polar.alpha_crit_low =
            get_f64_from(fm_data, &format!("{}.alphaCritLow", prefix)).unwrap_or(-15.0);

        polar.cl_crit_high =
            get_f64_from(fm_data, &format!("{}.ClCritHigh", prefix)).unwrap_or(1.2);
        polar.cl_crit_low =
            get_f64_from(fm_data, &format!("{}.ClCritLow", prefix)).unwrap_or(-1.2);

        polar.cd_min = get_f64_from(fm_data, &format!("{}.CdMin", prefix)).unwrap_or(0.01);

        polar.after_crit_parab_angle =
            get_f64_from(fm_data, &format!("{}.AfterCritParabAngle", prefix)).unwrap_or(10.0);
        polar.after_crit_decline_coeff =
            get_f64_from(fm_data, &format!("{}.AfterCritDeclineCoeff", prefix)).unwrap_or(0.01);
        polar.after_crit_max_dist_angle =
            get_f64_from(fm_data, &format!("{}.AfterCritMaxDistanceAngle", prefix)).unwrap_or(30.0);
        polar.cl_after_crit_high = [
            &format!("{}.ClAfterCritHigh", prefix)[..],
            &format!("{}.ClAfterCrit", prefix)[..],
        ].into_iter().find_map(|k| get_f64_from(fm_data, k)).unwrap_or(0.5);
        polar.cl_after_crit_low =
            get_f64_from(fm_data, &format!("{}.ClAfterCritLow", prefix)).unwrap_or(-0.5);
        polar.cx_after_coeff =
            get_f64_from(fm_data, &format!("{}.CxAfterCoeff", prefix)).unwrap_or(0.01);

        polar.mach_corrections = (1..=7)
            .map(|i| MachCorrection::read_from(fm_data, prefix, i))
            .collect();

        polar
    }

    pub fn cl_slope(&self) -> f64 {
        if self.alpha_crit_high > 0.0 {
            (self.cl_crit_high - self.cl0) / self.alpha_crit_high
        } else {
            0.0
        }
    }

    pub fn best_ld(&self, aspect_ratio: f64) -> Option<(f64, f64, f64, f64)> {
        if aspect_ratio <= 0.0 || self.alpha_crit_high <= 0.0 {
            return None;
        }
        let slope = self.cl_slope();
        if slope <= 0.0 {
            return None;
        }
        let mut best_alpha = 0.0_f64;
        let mut best_ld = 0.0_f64;
        let mut best_cl = 0.0_f64;
        let mut best_cd = 0.0_f64;

        let steps = (self.alpha_crit_high / 0.5).round() as usize;
        for i in 0..=steps {
            let alpha = i as f64 * 0.5;
            let (cl, cd) = self.cl_cd_at(alpha, aspect_ratio);
            if cd > 0.0 {
                let ld = cl / cd;
                if ld > best_ld {
                    best_ld = ld;
                    best_alpha = alpha;
                    best_cl = cl;
                    best_cd = cd;
                }
            }
        }

        if best_ld > 0.0 {
            Some((best_alpha, best_cl, best_cd, best_ld))
        } else {
            None
        }
    }

    pub fn cl_cd_at(&self, alpha_deg: f64, aspect_ratio: f64) -> (f64, f64) {
        let slope = self.cl_slope();
        let cl = self.cl0 + slope * alpha_deg;
        let pi = std::f64::consts::PI;
        let cd_induced = cl * cl / (pi * aspect_ratio.max(0.01) * self.oswalds_efficiency);
        let cd = self.cd_min + cd_induced;
        (cl, cd)
    }

}

fn find_flaps_polar(fm_data: &HashMap<String, BlkxValue>, prefixes: &[String]) -> Option<PolarData> {
    prefixes.iter()
        .find(|p| get_f64_from(fm_data, &format!("{}.CdMin", p)).is_some())
        .map(|p| PolarData::read_from(fm_data, p))
}

#[derive(Debug, Clone, Default)]
pub struct AeroSurface {
    pub name: String,
    pub span: f64,
    pub angle: f64,
    pub area: f64,
    pub taper_ratio: f64,
    pub swept_angle: f64,
    pub polar: PolarData,
    pub flaps_polar_0: Option<PolarData>,
    pub flaps_polar_1: Option<PolarData>,
}

impl AeroSurface {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ..Default::default()
        }
    }

    pub fn read_wing_from(fm_data: &HashMap<String, BlkxValue>, prefix: &str) -> Self {
        let mut surface = Self::new(prefix);

        let span_key = if prefix.is_empty() {
            "Wingspan".to_string()
        } else {
            format!("{}.Span", prefix)
        };
        let angle_key = if prefix.is_empty() { "Angle".to_string() } else { format!("{}.Angle", prefix) };
        let taper_key = if prefix.is_empty() { "TaperRatio".to_string() } else { format!("{}.TaperRatio", prefix) };
        let sweep_key = if prefix.is_empty() { "SweptAngle".to_string() } else { format!("{}.SweptAngle", prefix) };

        surface.span = get_f64_from(fm_data, &span_key).unwrap_or(0.0);
        surface.angle = get_f64_from(fm_data, &angle_key).unwrap_or(0.0);
        surface.taper_ratio = get_f64_from(fm_data, &taper_key).unwrap_or(1.0);
        surface.swept_angle = get_f64_from(fm_data, &sweep_key).unwrap_or(0.0);

        let area = if prefix.is_empty() {
            WING_AREA_KEYS.iter().flat_map(|key| {
                ["Areas.Wing", "Areas.", "Wing"].iter().filter_map(move |fmt| {
                    get_f64_from(fm_data, &format!("{}{}", fmt, key))
                })
            }).sum()
        } else {
            WING_AREA_KEYS.iter().filter_map(|key| {
                get_f64_from(fm_data, &format!("{}.Areas.{}", prefix, key))
            }).sum()
        };
        surface.area = if area == 0.0 {
            get_f64_from(fm_data, "wingArea").unwrap_or(0.0)
        } else {
            area
        };

        let make_pref = |s: &str| if prefix.is_empty() { s.to_string() } else { format!("{}.{}", prefix, s) };
        let flaps_0_prefixes = vec![
            make_pref("FlapsPolar0"),
            make_pref("NoFlaps"),
            make_pref("Aerodynamics.NoFlaps"),
            make_pref("Polar.NoFlaps"),
        ];
        if let Some(polar) = find_flaps_polar(fm_data, &flaps_0_prefixes) {
            surface.polar = polar.clone();
            surface.flaps_polar_0 = Some(polar);
        }

        let flaps_1_prefixes = vec![
            make_pref("FlapsPolar1"),
            make_pref("FullFlaps"),
            make_pref("Aerodynamics.FullFlaps"),
            make_pref("Polar.FullFlaps"),
        ];
        surface.flaps_polar_1 = find_flaps_polar(fm_data, &flaps_1_prefixes);

        surface
    }

    pub fn read_from(fm_data: &HashMap<String, BlkxValue>, prefix: &str) -> Self {
        let mut surface = Self::new(prefix);

        surface.span = get_f64_from(fm_data, &format!("{}.Span", prefix)).unwrap_or(0.0);
        surface.angle = get_f64_from(fm_data, &format!("{}.Angle", prefix)).unwrap_or(0.0);
        surface.taper_ratio =
            get_f64_from(fm_data, &format!("{}.TaperRatio", prefix)).unwrap_or(1.0);
        surface.swept_angle =
            get_f64_from(fm_data, &format!("{}.SweptAngle", prefix)).unwrap_or(0.0);

        surface.area = get_f64_from(fm_data, &format!("{}.Areas.Main", prefix)).unwrap_or(0.0);

        let polar_prefix = format!("{}.Polar", prefix);
        surface.polar = PolarData::read_from(fm_data, &polar_prefix);

        surface
    }

    pub fn get_polar_at_flaps(&self, flaps: f64) -> PolarData {
        let flaps = flaps.clamp(0.0, 1.0);

        if let (Some(p0), Some(p1)) = (&self.flaps_polar_0, &self.flaps_polar_1) {
            blend_polar(p0, p1, flaps)
        } else {
            self.polar.clone()
        }
    }
}

fn blend_polar(p0: &PolarData, p1: &PolarData, t: f64) -> PolarData {
    let t = t.clamp(0.0, 1.0);
    let lerp = |a: f64, b: f64| a + (b - a) * t;

    PolarData {
        oswalds_efficiency: lerp(p0.oswalds_efficiency, p1.oswalds_efficiency),
        line_cl_coeff: lerp(p0.line_cl_coeff, p1.line_cl_coeff),
        cl0: lerp(p0.cl0, p1.cl0),
        alpha_crit_high: lerp(p0.alpha_crit_high, p1.alpha_crit_high),
        alpha_crit_low: lerp(p0.alpha_crit_low, p1.alpha_crit_low),
        cl_crit_high: lerp(p0.cl_crit_high, p1.cl_crit_high),
        cl_crit_low: lerp(p0.cl_crit_low, p1.cl_crit_low),
        cd_min: lerp(p0.cd_min, p1.cd_min),
        after_crit_parab_angle: lerp(p0.after_crit_parab_angle, p1.after_crit_parab_angle),
        after_crit_decline_coeff: lerp(p0.after_crit_decline_coeff, p1.after_crit_decline_coeff),
        after_crit_max_dist_angle: lerp(p0.after_crit_max_dist_angle, p1.after_crit_max_dist_angle),
        cl_after_crit_high: lerp(p0.cl_after_crit_high, p1.cl_after_crit_high),
        cl_after_crit_low: lerp(p0.cl_after_crit_low, p1.cl_after_crit_low),
        cx_after_coeff: lerp(p0.cx_after_coeff, p1.cx_after_coeff),
        mach_corrections: p0.mach_corrections.clone(),
    }
}

#[derive(Debug, Clone)]
pub struct WingGeometry {
    pub sweep_percent: f64,
    pub span: f64,
    pub wing_area: f64,
    pub taper_ratio: f64,
    pub swept_angle: f64,
    pub no_flaps_polar: PolarData,
    pub full_flaps_polar: Option<PolarData>,
    pub high_flaps_polar: Option<PolarData>,
    pub vne: f64,
    pub vne_mach: f64,
    pub is_vwing: bool,
}

impl Default for WingGeometry {
    fn default() -> Self {
        Self {
            sweep_percent: 0.0,
            span: 10.0,
            wing_area: 20.0,
            taper_ratio: 1.0,
            swept_angle: 0.0,
            no_flaps_polar: PolarData::new(),
            full_flaps_polar: None,
            high_flaps_polar: None,
            vne: 1500.0,
            vne_mach: 1.0,
            is_vwing: false,
        }
    }
}

impl WingGeometry {
    pub fn read_from(fm_data: &HashMap<String, BlkxValue>, prefix: &str) -> Self {
        let mut wing = Self::new(0.0);

        let prefix_dot = if prefix.is_empty() { "".to_string() } else { format!("{}.", prefix) };

        wing.span = get_f64_from(fm_data, &format!("{}Span", prefix_dot)).unwrap_or(0.0);
        wing.taper_ratio = get_f64_from(fm_data, &format!("{}WingTaperRatio", prefix_dot)).unwrap_or(1.0);
        wing.swept_angle = get_f64_from(fm_data, &format!("{}SweptWingAngle", prefix_dot)).unwrap_or(0.0);

        wing.wing_area = 0.0;
        for key in &WING_AREA_KEYS {
            if let Some(v) = get_f64_from(fm_data, &format!("{}WingAreas.{}", prefix_dot, key)) {
                wing.wing_area += v;
            }
            if wing.wing_area == 0.0 {
                if let Some(v) = get_f64_from(fm_data, &format!("{}Areas.{}", prefix_dot, key)) {
                    wing.wing_area += v;
                }
            }
        }

        let no_flaps_prefixes = [
            format!("{}NoFlaps", prefix_dot),
            format!("{}FlapsPolar0", prefix_dot),
            format!("{}Aerodynamics.NoFlaps", prefix_dot),
            format!("{}Polar.NoFlaps", prefix_dot),
        ];
        let no_flaps_prefix = no_flaps_prefixes.iter()
            .find(|p| get_f64_from(fm_data, &format!("{}.CdMin", p)).is_some())
            .cloned()
            .unwrap_or_else(|| format!("{}FlapsPolar0", prefix_dot));
        wing.no_flaps_polar = PolarData::read_from(fm_data, &no_flaps_prefix);

        let full_flaps_prefixes = [
            format!("{}FullFlaps", prefix_dot),
            format!("{}FlapsPolar1", prefix_dot),
            format!("{}Aerodynamics.FullFlaps", prefix_dot),
            format!("{}Polar.FullFlaps", prefix_dot),
        ];
        let full_flaps_prefix = full_flaps_prefixes.iter()
            .find(|p| get_f64_from(fm_data, &format!("{}.CdMin", p)).is_some())
            .cloned()
            .unwrap_or_else(|| format!("{}FlapsPolar1", prefix_dot));
        wing.full_flaps_polar = Some(PolarData::read_from(fm_data, &full_flaps_prefix));

        let high_flaps_prefixes = [
            format!("{}HighFlaps", prefix_dot),
            format!("{}FlapsPolar2", prefix_dot),
            format!("{}Aerodynamics.HighFlaps", prefix_dot),
            format!("{}Polar.HighFlaps", prefix_dot),
        ];
        let high_flaps_prefix = high_flaps_prefixes.iter()
            .find(|p| get_f64_from(fm_data, &format!("{}.CdMin", p)).is_some())
            .cloned()
            .unwrap_or_else(|| format!("{}FlapsPolar2", prefix_dot));
        wing.high_flaps_polar = Some(PolarData::read_from(fm_data, &high_flaps_prefix));

        wing.vne = [
            &format!("{}Vne", prefix_dot)[..],
            &format!("{}Strength.VNE", prefix_dot)[..],
        ].into_iter().find_map(|k| get_f64_from(fm_data, k)).unwrap_or(1500.0);
        wing.vne_mach = [
            &format!("{}VneMach", prefix_dot)[..],
            &format!("{}Strength.MNE", prefix_dot)[..],
        ].into_iter().find_map(|k| get_f64_from(fm_data, k)).unwrap_or(1.0);

        wing.is_vwing = get_f64_from(fm_data, &format!("{}AoACritHigh", prefix_dot)).unwrap_or(0.0) != 0.0
            || get_f64_from(fm_data, &format!("{}FullFlaps.AoACritHigh", prefix_dot)).unwrap_or(0.0) != 0.0;

        wing
    }

    fn new(sweep_percent: f64) -> Self {
        Self {
            sweep_percent,
            ..Default::default()
        }
    }

    pub fn cl_max_no_flaps(&self) -> f64 {
        self.no_flaps_polar.cl_crit_high
    }

    pub fn cl_max_full_flaps(&self) -> f64 {
        self.full_flaps_polar.as_ref().map(|p| p.cl_crit_high).unwrap_or(self.cl_max_no_flaps())
    }

    pub fn aoa_cl_max_no_flaps(&self) -> f64 {
        self.no_flaps_polar.alpha_crit_high
    }

    pub fn aoa_cl_max_full_flaps(&self) -> f64 {
        self.full_flaps_polar.as_ref().map(|p| p.alpha_crit_high).unwrap_or(self.aoa_cl_max_no_flaps())
    }

    pub fn aspect_ratio(&self) -> f64 {
        if self.wing_area > 0.0 {
            self.span * self.span / self.wing_area
        } else {
            5.0
        }
    }

    pub fn cd_min(&self) -> f64 {
        self.no_flaps_polar.cd_min
    }

    pub fn oswalds_efficiency(&self) -> f64 {
        self.no_flaps_polar.oswalds_efficiency
    }

    pub fn get_cl_aoa_max(&self, flaps: f64) -> (f64, f64) {
        let flaps = flaps.clamp(0.0, 1.0);
        let cl0 = self.cl_max_no_flaps();
        let aoa0 = self.aoa_cl_max_no_flaps();

        if flaps == 0.0 {
            return (cl0, aoa0);
        }

        let cl1 = self.cl_max_full_flaps();
        let aoa1 = self.aoa_cl_max_full_flaps();

        (cl0 + (cl1 - cl0) * flaps, aoa0 + (aoa1 - aoa0) * flaps)
    }

    /// 负向临界攻角（度，负值）：alpha_crit_low 按襟翼线性插值，
    /// 口径同 `get_cl_aoa_max`（机动告警声负向分支用）
    pub fn get_cl_aoa_min(&self, flaps: f64) -> f64 {
        let flaps = flaps.clamp(0.0, 1.0);
        let a0 = self.no_flaps_polar.alpha_crit_low;
        if flaps == 0.0 {
            return a0;
        }
        let a1 = self
            .full_flaps_polar
            .as_ref()
            .map(|p| p.alpha_crit_low)
            .unwrap_or(a0);
        a0 + (a1 - a0) * flaps
    }

    /// 马赫修正后的最大升力包线（对齐 Dagor `calc_cd` 的 cy_crit 口径）。
    /// 返回 (cl_max, -cl_max, line_cl_coeff, cl0, aoa_crit_high)。
    pub fn cl_max_at_mach(&self, flaps: f64, mach: f64) -> (f64, f64, f64, f64, f64) {
        let (cl_max, aoa_high) = self.get_cl_aoa_max(flaps);
        let polar = &self.no_flaps_polar;
        let cl_m = cl_max_mach_corrected(polar, cl_max, mach);
        let aoa_m = aoa_high * mach_mult(polar, MACH_CURVE_AOA_CRIT, mach);
        (cl_m, -cl_m, polar.line_cl_coeff, polar.cl0, aoa_m)
    }
}

const MAX_SWEEPS: usize = 4;

#[derive(Debug, Clone, Default)]
pub struct VariableSweepWing {
    pub wing_sweep0: WingGeometry,
    pub wing_sweep1: Option<WingGeometry>,
    pub wing_sweep2: Option<WingGeometry>,
    pub wing_sweep3: Option<WingGeometry>,
}

impl VariableSweepWing {
    pub fn read_from(fm_data: &HashMap<String, BlkxValue>) -> Self {
        Self {
            wing_sweep0: Self::read_wing_geometry(fm_data, 0).unwrap_or_default(),
            wing_sweep1: Self::read_wing_geometry(fm_data, 1),
            wing_sweep2: Self::read_wing_geometry(fm_data, 2),
            wing_sweep3: Self::read_wing_geometry(fm_data, 3),
        }
    }

    /// Fill `buf` with the defined sweep positions (in ascending sweep order) and
    /// return how many were written. Allocation-free for the per-frame hot path.
    fn fill_wings<'a>(&'a self, buf: &mut [&'a WingGeometry; MAX_SWEEPS]) -> usize {
        let all = [
            Some(&self.wing_sweep0),
            self.wing_sweep1.as_ref(),
            self.wing_sweep2.as_ref(),
            self.wing_sweep3.as_ref(),
        ];
        let mut n = 0;
        for w in all.into_iter().flatten() {
            buf[n] = w;
            n += 1;
        }
        n
    }

    fn read_wing_geometry(fm_data: &HashMap<String, BlkxValue>, index: usize) -> Option<WingGeometry> {
        let sweep_suffix = format!("Sweep{}", index);
        let fallback_percent = (index as f64 * 0.5).min(1.0);
        let base_prefixes = ["WingPlane", "Aerodynamics.WingPlane"];

        for base in &base_prefixes {
            let prefix = format!("{}{}", base, sweep_suffix);
            if get_f64_from(fm_data, &format!("{}.Span", prefix)).map_or(false, |s| s > 0.0) {
                let mut wing = WingGeometry::read_from(fm_data, &prefix);
                wing.sweep_percent = get_f64_from(fm_data, &format!("{}.Sweep", prefix))
                    .unwrap_or(fallback_percent);
                return Some(wing);
            }

            if index == 0
                && get_f64_from(fm_data, &format!("{}.Span", base)).map_or(false, |s| s > 0.0)
            {
                let mut wing = WingGeometry::read_from(fm_data, base);
                wing.sweep_percent = 0.0;
                return Some(wing);
            }
        }

        if index == 0 {
            let span = ["Span", "Wingspan", "WingSpan"]
                .into_iter()
                .find_map(|k| get_f64_from(fm_data, k));
            if let Some(span) = span {
                if span > 0.0 {
                    let mut wing = WingGeometry::read_from(fm_data, "");
                    wing.sweep_percent = 0.0;
                    wing.span = span;
                    return Some(wing);
                }
            }
            Some(WingGeometry::default())
        } else {
            None
        }
    }

    pub fn is_variable_sweep(&self) -> bool {
        self.wing_sweep1.is_some() || self.wing_sweep2.is_some() || self.wing_sweep3.is_some()
    }

    /// Linear interpolation across the defined sweep positions, keyed by their real
    /// `Sweep` fraction. `keep_positive` drops positions whose extracted value is not
    /// positive (used for VNE/MNE, which may be left unset for some sweep positions).
    fn interp_at<F: Fn(&WingGeometry) -> f64>(
        &self,
        sweep: f64,
        keep_positive: bool,
        get: F,
    ) -> f64 {
        let mut all: [&WingGeometry; MAX_SWEEPS] = [&self.wing_sweep0; MAX_SWEEPS];
        let total = self.fill_wings(&mut all);

        let mut wings: [&WingGeometry; MAX_SWEEPS] = [&self.wing_sweep0; MAX_SWEEPS];
        let mut n = 0;
        for &w in &all[..total] {
            if !keep_positive || get(w) > 0.0 {
                wings[n] = w;
                n += 1;
            }
        }

        if n == 0 {
            return get(&self.wing_sweep0);
        }
        if n == 1 {
            return get(wings[0]);
        }

        let first = wings[0];
        let last = wings[n - 1];
        let s = sweep.clamp(first.sweep_percent, last.sweep_percent);
        for i in 0..n - 1 {
            let (a, b) = (wings[i], wings[i + 1]);
            if s <= b.sweep_percent {
                let span = b.sweep_percent - a.sweep_percent;
                if span.abs() < f64::EPSILON {
                    return get(b);
                }
                let t = (s - a.sweep_percent) / span;
                return get(a) + (get(b) - get(a)) * t;
            }
        }
        get(last)
    }

    pub fn get_vne(&self, wing_sweep: f64) -> f64 {
        if wing_sweep <= 0.0 || !self.is_variable_sweep() {
            return self.wing_sweep0.vne;
        }
        self.interp_at(wing_sweep, true, |w| w.vne)
    }

    pub fn get_cl_aoa_max(&self, flaps: f64, wing_sweep: f64) -> (f64, f64) {
        let wing_sweep = wing_sweep.clamp(0.0, 1.0);

        if wing_sweep == 0.0 || !self.is_variable_sweep() {
            return self.wing_sweep0.get_cl_aoa_max(flaps);
        }

        (
            self.interp_at(wing_sweep, false, |w| w.get_cl_aoa_max(flaps).0),
            self.interp_at(wing_sweep, false, |w| w.get_cl_aoa_max(flaps).1),
        )
    }

    /// 负向临界攻角（度，负值）：按襟翼/后掠线性插值，口径同 `get_cl_aoa_max`
    pub fn get_aoa_min(&self, flaps: f64, wing_sweep: f64) -> f64 {
        let wing_sweep = wing_sweep.clamp(0.0, 1.0);

        if wing_sweep == 0.0 || !self.is_variable_sweep() {
            return self.wing_sweep0.get_cl_aoa_min(flaps);
        }

        self.interp_at(wing_sweep, false, |w| w.get_cl_aoa_min(flaps))
    }

    pub fn span(&self) -> f64 {
        self.wing_sweep0.span
    }

    pub fn wing_area(&self) -> f64 {
        self.wing_sweep0.wing_area
    }

    pub fn cd_min(&self) -> f64 {
        self.wing_sweep0.cd_min()
    }

    pub fn oswalds_efficiency(&self) -> f64 {
        self.wing_sweep0.oswalds_efficiency()
    }

    pub fn aspect_ratio(&self) -> f64 {
        self.wing_sweep0.aspect_ratio()
    }

    pub fn vne_mach(&self) -> f64 {
        self.wing_sweep0.vne_mach
    }

    pub fn vne(&self) -> f64 {
        self.wing_sweep0.vne
    }

    pub fn get_vne_mach(&self, wing_sweep: f64) -> f64 {
        if wing_sweep <= 0.0 || !self.is_variable_sweep() {
            return self.wing_sweep0.vne_mach;
        }
        self.interp_at(wing_sweep, true, |w| w.vne_mach)
    }

    pub fn cl_max_no_flaps(&self) -> f64 {
        self.wing_sweep0.cl_max_no_flaps()
    }

    pub fn cl_max_full_flaps(&self) -> f64 {
        self.wing_sweep0.cl_max_full_flaps()
    }

    pub fn aoa_cl_max_no_flaps(&self) -> f64 {
        self.wing_sweep0.aoa_cl_max_no_flaps()
    }

    pub fn aoa_cl_max_full_flaps(&self) -> f64 {
        self.wing_sweep0.aoa_cl_max_full_flaps()
    }

    // TODO: Mach correction via each sweep WingGeometry's polars
    pub fn cl_at_mach(&self, aoa: f64, flaps: f64, wingsweep: f64, _mach: f64) -> f64 {
        let flaps = flaps.clamp(0.0, 1.0);
        let (cl_max, aoa_high) = self.get_cl_aoa_max(flaps, wingsweep);
        if aoa >= aoa_high { cl_max }
        else if aoa <= -aoa_high { -cl_max }
        else { self.wing_sweep0.no_flaps_polar.cl0 + self.wing_sweep0.no_flaps_polar.line_cl_coeff * aoa }
    }
}

/// Mach factor curve indices (DagorEngine PolaresProps::MachFactor)
const MACH_CURVE_CX0: usize = 0;
const MACH_CURVE_CL_LINE_COEFF: usize = 1;
const MACH_CURVE_CY_MAX: usize = 2;
const MACH_CURVE_AOA_CRIT: usize = 3;
const MACH_CURVE_IND_COEFF: usize = 4;
const MACH_CURVE_CY0: usize = 6;

fn pre_crit_val(index: usize) -> f64 {
    if index == 5 {
        0.0 // AER_CENTER_OFFSET
    } else {
        1.0
    }
}

fn mach_mult(polar: &PolarData, index: usize, mach: f64) -> f64 {
    let c: &MachCorrection = match polar.mach_corrections.get(index) {
        Some(c) => c,
        None => return pre_crit_val(index),
    };
    let pre_crit = pre_crit_val(index);
    if mach < c.mach_crit {
        return pre_crit;
    }
    if mach > c.mach_max {
        let mult = c.mult_mach_max + c.mult_line_coeff * (mach - c.mach_max);
        if c.mult_line_coeff > 0.0 {
            return mult.min(c.mult_limit);
        } else {
            return mult.max(c.mult_limit);
        }
    }
    let coeffs = solve_mach_coeffs(c, pre_crit);
    coeffs[0] + coeffs[1] * mach + coeffs[2] * mach * mach + coeffs[3] * mach * mach * mach
}

fn solve_mach_coeffs(c: &MachCorrection, pre_crit: f64) -> [f64; 4] {
    // Solve for cubic F(M) = c0 + c1*M + c2*M^2 + c3*M^3 with:
    //   F(Mcrit) = pre_crit, F'(Mcrit) = 0, F(Mmax) = mult_mach_max, F'(Mmax) = mult_line_coeff
    let mc = c.mach_crit;
    let mm = c.mach_max;
    // Augmented matrix [A | b]
    let mut m = [
        [0.0, 1.0, 2.0 * mc, 3.0 * mc * mc, 0.0],
        [0.0, 1.0, 2.0 * mm, 3.0 * mm * mm, c.mult_line_coeff],
        [1.0, mc, mc * mc, mc * mc * mc, pre_crit],
        [1.0, mm, mm * mm, mm * mm * mm, c.mult_mach_max],
    ];

    // Gauss-Jordan elimination with partial pivoting
    for col in 0..4 {
        let mut pivot = col;
        for r in (col + 1)..4 {
            if m[r][col].abs() > m[pivot][col].abs() {
                pivot = r;
            }
        }
        if m[pivot][col].abs() < 1e-12 {
            return [pre_crit, 0.0, 0.0, 0.0];
        }
        m.swap(col, pivot);

        let piv = m[col][col];
        for c in col..5 {
            m[col][c] /= piv;
        }
        for r in 0..4 {
            if r != col {
                let f = m[r][col];
                if f.abs() > 1e-15 {
                    for c in col..5 {
                        m[r][c] -= f * m[col][c];
                    }
                }
            }
        }
    }
    [m[0][4], m[1][4], m[2][4], m[3][4]]
}

pub fn get_lift_coefficient(polar: &PolarData, alpha_deg: f64, mach: f64) -> f64 {
    let aoa = alpha_deg;
    let cy_mult = 1.0;

    // Mach-corrected parameters (DagorEngine calc_mach_dependent_polares)
    let cl0 = polar.cl0 * mach_mult(polar, MACH_CURVE_CY0, mach);
    let cl_line = polar.line_cl_coeff * mach_mult(polar, MACH_CURVE_CL_LINE_COEFF, mach);
    let cy_crit_mult = mach_mult(polar, MACH_CURVE_CY_MAX, mach);
    let cy_crit_h = polar.cl0 + (polar.cl_crit_high - cl0) * cy_crit_mult;
    let cy_crit_l = polar.cl0 + (polar.cl_crit_low - cl0) * cy_crit_mult;
    let aoa_crit_mult = mach_mult(polar, MACH_CURVE_AOA_CRIT, mach);
    let aoa_crit_h = polar.alpha_crit_high * aoa_crit_mult;
    let aoa_crit_l = polar.alpha_crit_low * aoa_crit_mult;

    // Derived linear-region bounds (DagorEngine prepare_polares)
    let mut aoa_line_h =
        2.0 * (cy_crit_h - cl0) / (cl_line * cy_mult) - aoa_crit_h;
    let mut aoa_line_l =
        2.0 * (cy_crit_l - cl0) / (cl_line * cy_mult) - aoa_crit_l;
    if aoa_line_l > aoa_line_h {
        aoa_line_l = 0.5 * (aoa_line_l + aoa_line_h);
        aoa_line_h = aoa_line_l;
    }

    // Linear part
    if aoa <= aoa_line_h && aoa >= aoa_line_l {
        return cl0 + cl_line * aoa * cy_mult;
    }

    let sign = if aoa - aoa_line_h + 0.01 > 0.0 { 1.0 } else { -1.0 };
    let aoa_crit = if sign > 0.0 { aoa_crit_h } else { aoa_crit_l };
    let cl_after_crit = if sign > 0.0 {
        polar.cl_after_crit_high
    } else {
        polar.cl_after_crit_low
    };
    let cy_crit = if sign > 0.0 { cy_crit_h } else { cy_crit_l };

    // Parabolic part before critical angle
    if sign * (aoa - aoa_crit) <= 0.0 {
        let parab_cy_coeff = if sign > 0.0 {
            (cy_crit_h - (cl0 + aoa_line_h * cl_line * cy_mult))
                / (aoa_crit_h - aoa_line_h).powi(2)
        } else {
            (-cy_crit_l + (cl0 + aoa_line_l * cl_line * cy_mult))
                / (aoa_line_l - aoa_crit_l).powi(2)
        };
        return cy_crit - sign * parab_cy_coeff * (aoa_crit - aoa).powi(2);
    }

    // Post-stall multi-regime (DagorEngine calc_cl)
    let max_ang = 40.0f64.max(polar.after_crit_max_dist_angle);
    if sign * aoa <= max_ang {
        if sign * aoa <= polar.after_crit_max_dist_angle {
            let d_a = aoa - aoa_crit;
            if sign * d_a < polar.after_crit_parab_angle {
                return cy_crit - sign * polar.after_crit_decline_coeff * d_a.powi(2);
            } else {
                let h = cl_after_crit
                    * (std::f64::consts::PI * 0.0125 * polar.after_crit_max_dist_angle).sin();
                let max_da =
                    sign * (polar.after_crit_max_dist_angle - polar.after_crit_parab_angle)
                        - aoa_crit;
                let need_inc = cy_crit
                    - sign * polar.after_crit_decline_coeff * polar.after_crit_parab_angle.powi(2)
                    - h;
                let parab_coeff = need_inc / max_da.powi(2);
                return h + parab_coeff * (sign * polar.after_crit_max_dist_angle - aoa).powi(2);
            }
        } else {
            return cl_after_crit * (std::f64::consts::PI * 0.0125 * sign * aoa).sin();
        }
    } else if sign * aoa <= 140.0 {
        // Angles between maxAng and 140 (inverted-flight blend)
        let mut local_sign = sign;
        let mut local_aoa = sign * aoa;
        if sign * aoa > 90.0 {
            local_sign = -sign;
            local_aoa = 40.0 + (140.0 - local_aoa);
        }
        let t = (sign * aoa - 40.0) / (140.0 - 40.0);
        let blend_a = (std::f64::consts::PI * 0.0125 * max_ang).sin()
            - (std::f64::consts::PI * 0.5 + std::f64::consts::PI * 0.01 * (max_ang - 40.0).max(0.0)).sin();
        let lerp_val = blend_a * (1.0 - t);
        sign * cl_after_crit
            * (lerp_val + local_sign
                * (std::f64::consts::PI * 0.5 + std::f64::consts::PI * 0.01 * (local_aoa - 40.0)).sin())
    } else {
        // Inverted flight (mirror of sine regime)
        let local_sign = -sign;
        let local_aoa = 180.0 - sign * aoa;
        sign * cl_after_crit * (std::f64::consts::PI * 0.0125 * local_aoa).sin() * local_sign
    }
}

pub fn get_drag_coefficient(polar: &PolarData, aoa: f64, mach: f64, area: f64, span: f64) -> f64 {
    if area <= 0.0 || span <= 0.0 {
        return polar.cd_min;
    }

    let e = polar.oswalds_efficiency;
    if !e.is_finite() || e <= 0.0 {
        return polar.cd_min;
    }

    // Mach-corrected parameters (DagorEngine calc_mach_dependent_polares)
    let cd0 = polar.cd_min * mach_mult(polar, MACH_CURVE_CX0, mach);
    let ind_coeff0 = 1.0 / (std::f64::consts::PI * span * span / area * e);
    let ind_coeff = ind_coeff0 * mach_mult(polar, MACH_CURVE_IND_COEFF, mach);

    let cl0 = polar.cl0 * mach_mult(polar, MACH_CURVE_CY0, mach);
    let cl_line = polar.line_cl_coeff * mach_mult(polar, MACH_CURVE_CL_LINE_COEFF, mach);
    // Unclipped linear lift used for induced drag (DagorEngine calc_cd)
    let cy = cl0 + cl_line * aoa;

    let mut ind_drag = cd0 + cy * cy * ind_coeff;

    // Post-stall drag growth
    let sign = if aoa >= 0.0 { 1.0 } else { -1.0 };
    let aoa_crit = if sign > 0.0 {
        polar.alpha_crit_high * mach_mult(polar, MACH_CURVE_AOA_CRIT, mach)
    } else {
        polar.alpha_crit_low * mach_mult(polar, MACH_CURVE_AOA_CRIT, mach)
    };
    if sign * (aoa - aoa_crit) >= 0.0 {
        ind_drag += polar.cx_after_coeff * sign * (aoa - aoa_crit);
    }

    // Drag cap (flat-plate-like limit)
    let cy_crit_h = polar.cl0
        + (polar.cl_crit_high - cl0) * mach_mult(polar, MACH_CURVE_CY_MAX, mach);
    let cap = 0.15 + cy_crit_h * (aoa.to_radians().sin()).abs();

    ind_drag.min(cap)
}

/// CX0 马赫波阻乘子（Dagor `calc_mach_dependent_polares` 的 CX0 曲线），
/// 供非机翼零升阻力按同一口径做马赫修正（避免经验波阻重复计）。
pub fn cx0_mach_mult(polar: &PolarData, mach: f64) -> f64 {
    mach_mult(polar, MACH_CURVE_CX0, mach)
}

/// Dagor `calc_cd` cy_crit 口径的马赫修正最大升力系数：
/// `Cl_max(M) = Cl0 + (Cl_max − Cl0·mult_CY0)·mult_CY_MAX`。
/// 单一实现：WingGeometry::cl_max_at_mach 与飞行模型的可用过载计算共用。
pub fn cl_max_mach_corrected(polar: &PolarData, cl_max: f64, mach: f64) -> f64 {
    let cl0_m = polar.cl0 * mach_mult(polar, MACH_CURVE_CY0, mach);
    polar.cl0 + (cl_max - cl0_m) * mach_mult(polar, MACH_CURVE_CY_MAX, mach)
}

/// Dagor 声速 [m/s]：`20.1·√T`（atmosphere.cpp:73）
pub fn calculate_sound_speed(altitude: f64) -> f64 {
    20.1 * calculate_temperature(altitude).sqrt()
}

/// Dagor 温度 [K]：`T0·poly(TEMPERATURE_COEFFS, min(h, hMax))`（atmosphere.cpp:70）
pub fn calculate_temperature(altitude: f64) -> f64 {
    DAGOR_T0 * dagor_poly(&TEMPERATURE_COEFFS, altitude.min(DAGOR_H_MAX))
}

/// Dagor 压力 [Pa]：`P0·poly(·, min(h,hMax))·(hMax/max(hMax,h))`（atmosphere.cpp:67）
pub fn calculate_pressure(altitude: f64) -> f64 {
    DAGOR_P0
        * dagor_poly(&PRESSURE_COEFFS, altitude.min(DAGOR_H_MAX))
        * (DAGOR_H_MAX / DAGOR_H_MAX.max(altitude))
}

/// Dagor 密度 [kg/m³]：`ro0·poly(·, min(h,hMax))·(hMax/max(hMax,h))`（atmosphere.cpp:76）
pub fn calculate_density(altitude: f64) -> f64 {
    DAGOR_RO0
        * dagor_poly(&DENSITY_COEFFS, altitude.min(DAGOR_H_MAX))
        * (DAGOR_H_MAX / DAGOR_H_MAX.max(altitude))
}

pub fn calculate_mach(velocity: f64, altitude: f64) -> f64 {
    let a = calculate_sound_speed(altitude);
    if a > 0.0 {
        velocity / a
    } else {
        0.0
    }
}

pub struct AeroForces {
    pub lift: f64,
    pub drag: f64,
    pub cl: f64,
    pub cd: f64,
    pub lift_wing: f64,
    pub drag_wing: f64,
    pub lift_fuselage: f64,
    pub drag_fuselage: f64,
    pub lift_stab: f64,
    pub drag_stab: f64,
}

impl AeroForces {
    pub fn default() -> Self {
        Self {
            lift: 0.0,
            drag: 0.0,
            cl: 0.0,
            cd: 0.0,
            lift_wing: 0.0,
            drag_wing: 0.0,
            lift_fuselage: 0.0,
            drag_fuselage: 0.0,
            lift_stab: 0.0,
            drag_stab: 0.0,
        }
    }
}

pub fn calculate_aero_forces(
    wing: &AeroSurface,
    fuselage: &AeroSurface,
    hor_stab: &AeroSurface,
    reference_area: f64,
    velocity: f64,
    altitude: f64,
    alpha: f64,
    flaps: f64,
    gear_deployed: bool,
    airbrake_deployed: bool,
) -> AeroForces {
    let rho = calculate_density(altitude);
    let q = 0.5 * rho * velocity * velocity;
    let mach = calculate_mach(velocity, altitude);

    let wing_polar = wing.get_polar_at_flaps(flaps);
    let wing_alpha = alpha + wing.angle;
    let cl_wing = get_lift_coefficient(&wing_polar, wing_alpha, mach);
    let cd_wing = get_drag_coefficient(&wing_polar, wing_alpha, mach, wing.area, wing.span);
    let wing_lift = q * wing.area * cl_wing;
    let wing_drag = q * wing.area * cd_wing;

    let fuse_alpha = alpha + fuselage.angle;
    let cl_fuse = get_lift_coefficient(&fuselage.polar, fuse_alpha, mach);

    let cd_fuse = if fuselage.area > 0.0 && fuselage.span > 0.0 {
        get_drag_coefficient(&fuselage.polar, fuse_alpha, mach, fuselage.area, fuselage.span)
    } else {
        fuselage.polar.cd_min
    };

    let fuse_lift = q * fuselage.area * cl_fuse;
    let fuse_drag = q * fuselage.area * cd_fuse;

    let downwash = 0.0;
    let stab_alpha = alpha - downwash + hor_stab.angle;
    let cl_stab = get_lift_coefficient(&hor_stab.polar, stab_alpha, mach);
    let cd_stab = if hor_stab.area > 0.0 && hor_stab.span > 0.0 {
        get_drag_coefficient(&hor_stab.polar, stab_alpha, mach, hor_stab.area, hor_stab.span)
    } else {
        hor_stab.polar.cd_min
    };
    let stab_lift = q * hor_stab.area * cl_stab;
    let stab_drag = q * hor_stab.area * cd_stab;

    let mut extra_cd = 0.0;
    if gear_deployed {
        extra_cd += 0.035;
    }
    if airbrake_deployed {
        extra_cd += 0.035;
    }
    let extra_drag = q * reference_area * extra_cd;

    let total_lift = wing_lift + fuse_lift + stab_lift;
    let total_drag = wing_drag + fuse_drag + stab_drag + extra_drag;

    let cl = if q > 0.0 && reference_area > 0.0 {
        total_lift / (q * reference_area)
    } else {
        0.0
    };
    let cd = if q > 0.0 && reference_area > 0.0 {
        total_drag / (q * reference_area)
    } else {
        0.0
    };

    AeroForces {
        lift: total_lift,
        drag: total_drag,
        cl,
        cd,
        lift_wing: wing_lift,
        drag_wing: wing_drag,
        lift_fuselage: fuse_lift,
        drag_fuselage: fuse_drag,
        lift_stab: stab_lift,
        drag_stab: stab_drag,
    }
}

pub fn read_aero_surfaces(
    fm_data: &HashMap<String, BlkxValue>,
) -> (AeroSurface, AeroSurface, AeroSurface, f64) {
    let wing = read_wing_surface(fm_data);
    let fuselage = read_fuselage_surface(fm_data);
    let hor_stab = read_hor_stab_surface(fm_data);

    let reference_area = wing.area;

    (wing, fuselage, hor_stab, reference_area)
}

fn read_wing_surface(fm_data: &HashMap<String, BlkxValue>) -> AeroSurface {
    let prefixes = ["WingPlane", "WingPlaneSweep0", "Aerodynamics.WingPlane", "Aerodynamics.WingPlaneSweep0", ""];

    for prefix in &prefixes {
        let span = if prefix.is_empty() {
            fm_data.get("Span")
                .or_else(|| fm_data.get("Wingspan"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
        } else {
            fm_data
                .get(&format!("{}.Span", prefix))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
        };

        if span > 0.0 {
            let surface = AeroSurface::read_wing_from(fm_data, prefix);
            if surface.area > 0.0 {
                return surface;
            }
        }
    }

    AeroSurface::new("WingPlane")
}

fn read_fuselage_surface(fm_data: &HashMap<String, BlkxValue>) -> AeroSurface {
    let prefixes = [
        "Aerodynamics.FuselagePlane",
        "FuselagePlane",
        "Fuselage",
        "WingPlaneSweep0",
        "",
    ];

    for prefix in &prefixes {
        let area = if prefix.is_empty() {
            get_f64_from(fm_data, "Areas.Fuselage")
                .or_else(|| get_f64_from(fm_data, "Fuselage.Areas.Main"))
                .or_else(|| get_f64_from(fm_data, "Fuselage.Area"))
                .unwrap_or(0.0)
        } else {
            get_f64_from(fm_data, &format!("{}.Areas.Main", prefix))
                .or_else(|| get_f64_from(fm_data, &format!("{}.Area", prefix)))
                .unwrap_or(0.0)
        };

        if area > 0.0 {
            let mut surface = AeroSurface::new(prefix);
            surface.area = area;
            surface.span = if prefix.is_empty() {
                get_f64_from(fm_data, "Fuselage.Span")
                    .or_else(|| get_f64_from(fm_data, "Length"))
                    .unwrap_or(0.0)
            } else {
                get_f64_from(fm_data, &format!("{}.Span", prefix))
                    .unwrap_or(0.0)
            };
            surface.angle = get_f64_from(fm_data, &format!("{}.Angle", prefix))
                .unwrap_or(0.0);

            let polar_prefixes = [
                if prefix.is_empty() { "Aerodynamics.Fuselage".to_string() } else { format!("{}.Polar", prefix) },
                if prefix.is_empty() { "Fuselage".to_string() } else { prefix.to_string() },
            ];
            surface.polar = polar_prefixes.iter()
                .find_map(|p| {
                    let pol = PolarData::read_from(fm_data, p);
                    (pol.cd_min.abs() > 0.0001).then_some(pol)
                })
                .unwrap_or_default();

            return surface;
        }
    }

    AeroSurface::new("FuselagePlane")
}

fn read_hor_stab_surface(fm_data: &HashMap<String, BlkxValue>) -> AeroSurface {
    let prefixes = ["Aerodynamics.HorStabPlane", "HorStabPlane", "Stab", ""];

    for prefix in &prefixes {
        let area = if prefix.is_empty() {
            ["Areas.Stabilizer", "Stab.Areas.Main", "Stab.Area", "Areas.Elevator"]
                .iter()
                .filter_map(|k| get_f64_from(fm_data, k))
                .max_by(|a, b| a.partial_cmp(b).unwrap())
                .unwrap_or(0.0)
        } else {
            [&format!("{}.Areas.Main", prefix), &format!("{}.Areas.Elevator", prefix), &format!("{}.Area", prefix)]
                .iter()
                .filter_map(|k| get_f64_from(fm_data, k))
                .max_by(|a, b| a.partial_cmp(b).unwrap())
                .unwrap_or(0.0)
        };

        if area > 0.0 {
            let mut surface = AeroSurface::new(prefix);
            surface.area = area;
            surface.span = if prefix.is_empty() {
                get_f64_from(fm_data, "StabWidth")
                    .or_else(|| get_f64_from(fm_data, "Stab.Span"))
                    .unwrap_or(0.0)
            } else {
                get_f64_from(fm_data, &format!("{}.Span", prefix))
                    .unwrap_or(0.0)
            };
            surface.angle = get_f64_from(fm_data, &format!("{}.Angle", prefix))
                .unwrap_or(0.0);

            let polar_prefixes = [
                if prefix.is_empty() { "Aerodynamics.Stab".to_string() } else { format!("{}.Polar", prefix) },
                if prefix.is_empty() { "Stab".to_string() } else { prefix.to_string() },
            ];
            surface.polar = polar_prefixes.iter()
                .find_map(|p| {
                    let pol = PolarData::read_from(fm_data, p);
                    (pol.cd_min.abs() > 0.0001).then_some(pol)
                })
                .unwrap_or_default();

            return surface;
        }
    }

    // Fall back to span-based lookup
    for prefix in &prefixes {
        let span = get_f64_from(fm_data, &format!("{}.Span", prefix)).unwrap_or(0.0);

        if span > 0.0 {
            return AeroSurface::read_from(fm_data, prefix);
        }
    }

    AeroSurface::new("HorStabPlane")
}

pub fn print_aero_surface_info(wing: &AeroSurface, fuselage: &AeroSurface, hor_stab: &AeroSurface) {
    println!("=== Wing ===");
    println!("  Span: {:.2} m", wing.span);
    println!("  Area: {:.2} m²", wing.area);
    println!("  Angle: {:.2}°", wing.angle);
    println!("  CdMin: {:.4}", wing.polar.cd_min);
    println!("  Cl0: {:.4}", wing.polar.cl0);
    println!("  lineClCoeff: {:.4}", wing.polar.line_cl_coeff);
    println!("  alphaCritHigh: {:.2}°", wing.polar.alpha_crit_high);
    println!("  alphaCritLow: {:.2}°", wing.polar.alpha_crit_low);
    println!("  ClCritHigh: {:.2}", wing.polar.cl_crit_high);
    println!("  ClCritLow: {:.2}", wing.polar.cl_crit_low);

    if let (Some(ref p0), Some(ref p1)) = (&wing.flaps_polar_0, &wing.flaps_polar_1) {
        println!("  No flaps polar: cl_max={:.2}, alpha_crit={:.1}°", p0.cl_crit_high, p0.alpha_crit_high);
        println!("  Full flaps polar: cl_max={:.2}, alpha_crit={:.1}°", p1.cl_crit_high, p1.alpha_crit_high);
    }

    println!("\n=== Fuselage ===");
    println!("  Span: {:.2} m", fuselage.span);
    println!("  Area: {:.2} m²", fuselage.area);
    println!("  Angle: {:.2}°", fuselage.angle);
    println!("  CdMin: {:.4}", fuselage.polar.cd_min);

    println!("\n=== Horizontal Stabilizer ===");
    println!("  Span: {:.2} m", hor_stab.span);
    println!("  Area: {:.2} m²", hor_stab.area);
    println!("  Angle: {:.2}°", hor_stab.angle);
    println!("  CdMin: {:.4}", hor_stab.polar.cd_min);
}

#[cfg(test)]
mod atmosphere_tests {
    use super::*;

    /// 与 Dagor atmosphere.{h,cpp} 多项式手算值对照
    #[test]
    fn dagor_atmosphere_sea_level() {
        assert!((calculate_temperature(0.0) - DAGOR_T0).abs() < 1e-9);
        assert!((calculate_density(0.0) - DAGOR_RO0).abs() < 1e-9);
        assert!((calculate_pressure(0.0) - DAGOR_P0).abs() < 1e-6);
        // 声速 = 20.1·√288.16 ≈ 341.20 m/s（Dagor 口径，非常规 340.3）
        assert!((calculate_sound_speed(0.0) - 341.20).abs() < 0.1);
    }

    #[test]
    fn dagor_atmosphere_matches_polynomial() {
        // poly(DENSITY_COEFFS, 11000) = 0.297623 → ρ = 1.225·0.297623 ≈ 0.364588
        assert!((calculate_density(11000.0) - 0.364588).abs() < 1e-4);
        // poly(TEMPERATURE_COEFFS, 11000) = 0.7580578 → T ≈ 218.44 K
        assert!((calculate_temperature(11000.0) - 218.44).abs() < 0.05);
    }

    #[test]
    fn dagor_atmosphere_above_hmax_folds_as_one_over_h() {
        // h > hMax：温度冻结在 T(hMax)，密度/压力按 hMax/h 衰减
        let d1 = calculate_density(DAGOR_H_MAX);
        let d2 = calculate_density(DAGOR_H_MAX * 2.0);
        assert!((d1 / d2 - 2.0).abs() < 1e-9, "密度应按 1/h 衰减");
        assert!(
            (calculate_temperature(40000.0) - calculate_temperature(DAGOR_H_MAX)).abs() < 1e-9,
            "hMax 之上温度冻结"
        );
    }
}
