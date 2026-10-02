use crate::parser::{get_f64_first, BlkxValue};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum EngineType {
    #[default]
    Piston,
    Jet,
    Turboprop,
    Rocket,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct EngineMode {
    pub throttle: f64,
    pub rpm: f64,
    pub thrust_mult: f64,
    pub consumption_mult: f64,
}

impl EngineMode {
    pub fn new(throttle: f64, rpm: f64, thrust_mult: f64, consumption_mult: f64) -> Self {
        Self {
            throttle,
            rpm,
            thrust_mult,
            consumption_mult,
        }
    }
}

fn interp_engine_mode(modes: &[EngineMode], throttle: f64, f: impl Fn(&EngineMode) -> f64) -> f64 {
    if modes.is_empty() {
        return 1.0;
    }
    if throttle <= modes[0].throttle {
        return f(&modes[0]);
    }
    if throttle >= modes[modes.len() - 1].throttle {
        return f(&modes[modes.len() - 1]);
    }
    for i in 0..modes.len() - 1 {
        if throttle >= modes[i].throttle && throttle < modes[i + 1].throttle {
            let t0 = modes[i].throttle;
            let t1 = modes[i + 1].throttle;
            let frac = (throttle - t0) / (t1 - t0);
            return f(&modes[i]) * (1.0 - frac) + f(&modes[i + 1]) * frac;
        }
    }
    f(&modes[modes.len() - 1])
}

#[derive(Debug, Clone)]
pub struct Engine {
    pub engine_type: EngineType,
    pub thrust_max: f64,
    pub power: f64,
    pub rpm_min: f64,
    pub rpm_max: f64,
    pub rpm_max_allowed: f64,
    pub afterburner_boost: f64,
    pub throttle_boost: f64,
    pub nitro_consumption: f64,
    pub consumption_omega_max: f64,
    pub compressor_stages: Vec<CompressorStage>,
    pub altitudes: Vec<f64>,
    pub velocities: Vec<f64>,
    pub coefficients: Vec<Vec<Vec<f64>>>, // [altitude][velocity][0=military, 1=wep]
    pub fuel_consumption_idle: f64,
    pub fuel_consumption_half: f64,
    pub fuel_consumption_full: f64,
    pub fuel_consumption_wep: f64,
    pub engine_modes: Vec<EngineMode>,
}

impl Engine {
    pub fn engine_type(&self) -> EngineType {
        self.engine_type
    }

    pub fn thrust(&self) -> f64 {
        match self.engine_type {
            EngineType::Jet | EngineType::Rocket => self.thrust_max,
            _ => 0.0,
        }
    }

    pub fn power(&self) -> f64 {
        match self.engine_type {
            EngineType::Piston => self.power,
            _ => 0.0,
        }
    }

    pub fn fuel_consumption_idle(&self) -> f64 {
        self.fuel_consumption_idle
    }

    pub fn fuel_consumption_half(&self) -> f64 {
        self.fuel_consumption_half
    }

    pub fn fuel_consumption_full(&self) -> f64 {
        let base = self.fuel_consumption_full;
        if self.consumption_omega_max > 0.0 {
            base.max(self.consumption_omega_max)
        } else {
            base
        }
    }

    pub fn fuel_consumption_wep(&self) -> f64 {
        self.fuel_consumption_full()
            * self.afterburner_boost
            * self
                .engine_modes
                .last()
                .map(|m| m.consumption_mult)
                .unwrap_or(1.0)
    }

    pub fn nitro_consumption(&self) -> f64 {
        self.nitro_consumption
    }

    pub fn get_thrust_at(&self, velocity: f64, altitude: f64, throttle: f64) -> f64 {
        if self.engine_type == EngineType::Rocket {
            return self.thrust_max;
        }
        if self.engine_type != EngineType::Jet {
            return 0.0;
        }
        if self.altitudes.is_empty() || self.velocities.is_empty() {
            return 0.0;
        }

        let mil_thrust = bilinear_interp(
            &self.velocities,
            &self.altitudes,
            &self.coefficients,
            velocity,
            altitude,
            0,
        ) * self.thrust_max;

        if throttle <= 100.0 || !self.has_wep() {
            return mil_thrust;
        }

        let ab_base = bilinear_interp(
            &self.velocities,
            &self.altitudes,
            &self.coefficients,
            velocity,
            altitude,
            1,
        ) * self.thrust_max
            * self.afterburner_boost;

        if self.engine_modes.is_empty() {
            return ab_base;
        }

        if throttle <= self.engine_modes[0].throttle || self.engine_modes.len() <= 1 {
            let frac = (throttle - 100.0) / (self.engine_modes[0].throttle - 100.0);
            let first_ab_thrust = ab_base * self.engine_modes[0].thrust_mult;
            return mil_thrust * (1.0 - frac) + first_ab_thrust * frac;
        }

        let thrust_mult = interp_engine_mode(&self.engine_modes, throttle, |m| m.thrust_mult);
        ab_base * thrust_mult
    }

    pub fn get_fuel_rate_at(&self, velocity: f64, altitude: f64, throttle: f64) -> f64 {
        let thrust = self.get_thrust_at(velocity, altitude, throttle);
        let consumption_mult = self.get_consumption_mult(throttle);
        self.fuel_consumption_full() * thrust * consumption_mult / 3600.0
    }

    /// 计算活塞发动机功率 (hp)
    /// 注意: 压缩机(增压器)模型尚未完善，当前仅 0 米高度的计算值是准确的。
    ///       高空功率使用线性插值或简单衰减公式，可能与实际不符。
    pub fn get_power_at(&self, _velocity: f64, altitude: f64, throttle: f64) -> f64 {
        if self.engine_type != EngineType::Piston {
            return 0.0;
        }

        let base_power = self.power;
        let wep_mult = if throttle > 100.0 { self.throttle_boost.max(self.afterburner_boost) } else { 1.0 };

        self.compressor_stages.iter()
            .filter(|s| s.altitude > 0.0)
            .map(|stage| {
                if altitude <= stage.altitude {
                    stage.power * wep_mult
                } else if altitude >= stage.ceiling {
                    stage.power_at_ceiling * wep_mult
                } else {
                    let frac = (altitude - stage.altitude) / (stage.ceiling - stage.altitude);
                    (stage.power + (stage.power_at_ceiling - stage.power) * frac) * wep_mult
                }
            })
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or_else(|| base_power * wep_mult * (-0.10 * (altitude / 1000.0)).exp())
    }

    pub fn get_consumption_mult(&self, throttle: f64) -> f64 {
        if self.engine_modes.is_empty() {
            return 1.0;
        }

        if throttle <= 100.0 || !self.has_wep() {
            if self.engine_modes.len() >= 2 && self.has_wep() {
                return self.engine_modes[self.engine_modes.len() - 2].consumption_mult;
            }
            return self.engine_modes.last().unwrap().consumption_mult;
        }

        interp_engine_mode(&self.engine_modes, throttle, |m| m.consumption_mult)
    }

    pub fn wep_multiplier(&self) -> f64 {
        self.afterburner_boost * self.wep_thrust_mult()
    }

    pub fn afterburner_boost(&self) -> f64 {
        self.afterburner_boost
    }

    pub fn wep_thrust_mult(&self) -> f64 {
        self.engine_modes
            .last()
            .map(|m| m.thrust_mult)
            .unwrap_or(1.0)
    }

    pub fn engine_modes(&self) -> &[EngineMode] {
        &self.engine_modes
    }

    pub fn has_wep(&self) -> bool {
        if self.throttle_boost > 1.0 || self.afterburner_boost > 1.0 {
            return true;
        }
        self.coefficients
            .iter()
            .flat_map(|alt| alt.iter())
            .any(|vel| vel.len() > 1 && vel[1] > vel[0])
    }

    pub fn fuel_rates(&self) -> EngineFuelRates {
        EngineFuelRates {
            idle: self.fuel_consumption_idle(),
            half: self.fuel_consumption_half(),
            full: self.fuel_consumption_full(),
            wep: self.fuel_consumption_wep(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct EngineFuelRates {
    pub idle: f64,
    pub half: f64,
    pub full: f64,
    pub wep: f64,
}

impl EngineFuelRates {
    fn add(a: Self, b: Self) -> Self {
        Self {
            idle: a.idle + b.idle,
            half: a.half + b.half,
            full: a.full + b.full,
            wep: a.wep + b.wep,
        }
    }
}


fn get_engine_data_prefix(
    fm_data: &HashMap<String, BlkxValue>,
    engine_idx: usize,
) -> (String, String) {
    let engine_prefix = format!("Engine{}.", engine_idx);

    let engine_type_key = format!("{}Type", engine_prefix);
    let type_idx = if let Some(v) = fm_data.get(&engine_type_key) {
        v.as_string().parse::<usize>().ok()
    } else {
        None
    };

    let data_prefix = if let Some(idx) = type_idx {
        format!("EngineType{}.", idx)
    } else {
        engine_prefix.clone()
    };

    (engine_prefix, data_prefix)
}

fn parse_engine_type_string(s: &str) -> Option<EngineType> {
    if s.contains("Jet") {
        Some(EngineType::Jet)
    } else if s.contains("Piston") || s.contains("Radial") || s.contains("Inline") {
        Some(EngineType::Piston)
    } else if s.contains("Turboprop") {
        Some(EngineType::Turboprop)
    } else if s.contains("Rocket") {
        Some(EngineType::Rocket)
    } else {
        None
    }
}

fn detect_engine_type(fm_data: &HashMap<String, BlkxValue>, engine_idx: usize) -> EngineType {
    let check_main_type = |engine_idx: usize| -> Option<EngineType> {
        fm_data
            .get(&format!("Engine{}.Type", engine_idx))
            .and_then(|v| v.as_string().parse::<usize>().ok())
            .and_then(|idx| fm_data.get(&format!("EngineType{}.Main.Type", idx)))
            .and_then(|v| parse_engine_type_string(v.as_string().as_str()))
    };

    check_main_type(engine_idx)
        .or_else(|| {
            fm_data
                .get(&format!("Engine{}.Main.Type", engine_idx))
                .and_then(|v| parse_engine_type_string(v.as_string().as_str()))
        })
        // **`EngineType{N}.Main.Type` 缺键（或值不认识）→ 默认活塞**。
        //
        // 这里**不再**看 `Main.Power` / `ThrustMax.ThrustMax0` 来推断：喷气机上游客型都有
        // Type 键，缺键实际只出现在活塞机上（实测 `i-16_type24` / `f3f-2` / `tu-2_early` /
        // `fw_190f_8*`），默认活塞更简单也更符合用户预期。
        // 反例见 `tests::missing_engine_type_key_defaults_to_piston`（有 ThrustMax 也必须是 Piston）。
        .unwrap_or(EngineType::Piston)
}

fn parse_rpm_fields(fm_data: &HashMap<String, BlkxValue>, data_prefix: &str, engine_prefix: &str) -> (f64, f64, f64) {
    let rpm_min = get_f64_first(fm_data, &[
        &format!("{}Main.RPMMin", data_prefix),
        &format!("{}RPMMin", data_prefix),
        &format!("{}Main.RPMMin", engine_prefix),
        &format!("{}RPMMin", engine_prefix),
    ]).unwrap_or(0.0);
    let rpm_max = get_f64_first(fm_data, &[
        &format!("{}Main.RPMMax", data_prefix),
        &format!("{}RPMMax", data_prefix),
        &format!("{}Main.RPMMax", engine_prefix),
        &format!("{}RPMMax", engine_prefix),
    ]).unwrap_or(f64::MAX);
    let rpm_max_allowed = get_f64_first(fm_data, &[
        &format!("{}Main.RPMMaxAllowed", data_prefix),
        &format!("{}RPMMaxAllowed", data_prefix),
        &format!("{}Main.RPMMaxAllowed", engine_prefix),
        &format!("{}RPMMaxAllowed", engine_prefix),
    ]).unwrap_or(f64::MAX);
    (rpm_min, rpm_max, rpm_max_allowed)
}

fn parse_jet_engine(fm_data: &HashMap<String, BlkxValue>, engine_idx: usize) -> Option<Engine> {
    let (engine_prefix, data_prefix) = get_engine_data_prefix(fm_data, engine_idx);

    let thrust_max = parse_jet_thrust_max(fm_data, &data_prefix, &engine_prefix)?;
    let afterburner_boost = parse_jet_afterburner(fm_data, &data_prefix, &engine_prefix);
    let engine_modes = parse_engine_modes(fm_data, &data_prefix, &engine_prefix, true);
    let (fuel_consumption_idle, fuel_consumption_half, fuel_consumption_full, fuel_consumption_wep, consumption_omega_max) =
        parse_fuel_consumptions(fm_data, &data_prefix, &engine_prefix);
    let (altitudes, velocities, coefficients) = parse_thrust_coefficients(fm_data, &data_prefix)?;

    let (rpm_min, rpm_max, rpm_max_allowed) = parse_rpm_fields(fm_data, &data_prefix, &engine_prefix);

    Some(Engine {
        engine_type: EngineType::Jet,
        thrust_max,
        power: 0.0,
        rpm_min,
        rpm_max,
        rpm_max_allowed,
        afterburner_boost,
        throttle_boost: 1.0,
        nitro_consumption: 0.0,
        consumption_omega_max,
        compressor_stages: Vec::new(),
        altitudes,
        velocities,
        coefficients,
        fuel_consumption_idle,
        fuel_consumption_half,
        fuel_consumption_full,
        fuel_consumption_wep,
        engine_modes,
    })
}

fn parse_jet_thrust_max(
    fm_data: &HashMap<String, BlkxValue>,
    data_prefix: &str,
    engine_prefix: &str,
) -> Option<f64> {
    get_f64_first(
        fm_data,
        &[
            &format!("{}Main.ThrustMax.ThrustMax0", data_prefix),
            &format!("{}ThrustMax.ThrustMax0", data_prefix),
            &format!("{}Main.Thrust", data_prefix),
            &format!("{}Main.Thrust", engine_prefix),
        ],
    )
    .filter(|&v| v > 0.0)
}

fn parse_jet_afterburner(
    fm_data: &HashMap<String, BlkxValue>,
    data_prefix: &str,
    engine_prefix: &str,
) -> f64 {
    get_f64_first(
        fm_data,
        &[
            &format!("{}Main.AfterburnerBoost", data_prefix),
            &format!("{}AfterburnerBoost", data_prefix),
            &format!("{}Main.AfterburnerBoost", engine_prefix),
        ],
    )
    .unwrap_or(1.0)
}

fn parse_engine_modes(
    fm_data: &HashMap<String, BlkxValue>,
    data_prefix: &str,
    engine_prefix: &str,
    use_main_suffix: bool,
) -> Vec<EngineMode> {
    let prefixes: Vec<String> = if use_main_suffix {
        vec![
            format!("{}Main.", data_prefix),
            format!("{}Main.", engine_prefix),
        ]
    } else {
        vec![data_prefix.to_string(), engine_prefix.to_string()]
    };
    let mut engine_modes = Vec::new();

    for prefix in &prefixes {
        for i in 0..10 {
            let throttle_key = format!("{}Mode{}.Throttle", prefix, i);
            if let Some(throttle) = fm_data.get(&throttle_key).and_then(|v| v.as_f64()) {
                let rpm_key = format!("{}Mode{}.RPM", prefix, i);
                let thrust_mult_key = format!("{}Mode{}.ThrustMult", prefix, i);
                let consumption_mult_key = format!("{}Mode{}.ConsumptionMult", prefix, i);

                engine_modes.push(EngineMode::new(
                    throttle,
                    fm_data.get(&rpm_key).and_then(|v| v.as_f64()).unwrap_or(0.0),
                    fm_data.get(&thrust_mult_key).and_then(|v| v.as_f64()).unwrap_or(1.0),
                    fm_data.get(&consumption_mult_key).and_then(|v| v.as_f64()).unwrap_or(1.0),
                ));
            } else {
                break;
            }
        }
        if !engine_modes.is_empty() {
            break;
        }
    }
    engine_modes.sort_by(|a, b| a.throttle.partial_cmp(&b.throttle).unwrap());
    if let Some(last) = engine_modes.last() {
        if last.throttle <= 1.5 {
            for mode in &mut engine_modes {
                mode.throttle *= 100.0;
            }
        }
    }
    engine_modes
}

fn parse_fuel_consumptions(
    fm_data: &HashMap<String, BlkxValue>,
    data_prefix: &str,
    engine_prefix: &str,
) -> (f64, f64, f64, f64, f64) {
    let fuel_consumption_idle = get_f64_first(
        fm_data,
        &[
            &format!("{}Main.FuelConsumptionOnIdle", data_prefix),
            &format!("{}Main.FuelConsumptionOnIdle", engine_prefix),
        ],
    )
    .unwrap_or(0.0);

    let fuel_consumption_half = get_f64_first(
        fm_data,
        &[
            &format!("{}Main.FuelConsumptionOnHalfThr", data_prefix),
            &format!("{}Main.FuelConsumptionOnHalfThr", engine_prefix),
        ],
    )
    .unwrap_or(0.0);

    let fuel_consumption_full = get_f64_first(
        fm_data,
        &[
            &format!("{}Main.FuelConsumptionOnFullThr", data_prefix),
            &format!("{}Main.FuelConsumptionOnFullThr", engine_prefix),
        ],
    )
    .unwrap_or(fuel_consumption_half);

    let fuel_consumption_wep = get_f64_first(
        fm_data,
        &[
            &format!("{}Main.FuelConsumptionOnWEP", data_prefix),
            &format!("{}Main.FuelConsumptionOnWEP", engine_prefix),
        ],
    )
    .unwrap_or(fuel_consumption_full);

    let consumption_omega_max = get_f64_first(
        fm_data,
        &[&format!("{}Main.ConsumptionOmegaMax", data_prefix)],
    )
    .unwrap_or(1.0);

    (fuel_consumption_idle, fuel_consumption_half, fuel_consumption_full, fuel_consumption_wep, consumption_omega_max)
}

fn parse_thrust_coefficients(
    fm_data: &HashMap<String, BlkxValue>,
    data_prefix: &str,
) -> Option<(Vec<f64>, Vec<f64>, Vec<Vec<Vec<f64>>>)> {
    let search_prefixes = vec![data_prefix.to_string()];

    let mut altitudes = Vec::new();
    let mut velocities = Vec::new();

    for (key, value) in fm_data {
        for prefix in &search_prefixes {
            if key.starts_with(prefix) && key.contains("ThrustMax") {
                if let Some(alt) = key.split("Altitude_").nth(1) {
                    if let Ok(idx) = alt.trim().parse::<usize>() {
                        if let Some(v) = value.as_f64() {
                            while altitudes.len() <= idx {
                                altitudes.push(0.0);
                            }
                            altitudes[idx] = v;
                        }
                    }
                }
                if let Some(vel) = key.split("Velocity_").nth(1) {
                    if let Ok(idx) = vel.trim().parse::<usize>() {
                        if let Some(v) = value.as_f64() {
                            while velocities.len() <= idx {
                                velocities.push(0.0);
                            }
                            velocities[idx] = v;
                        }
                    }
                }
            }
        }
    }

    altitudes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    velocities.sort_by(|a, b| a.partial_cmp(b).unwrap());

    if altitudes.is_empty() && velocities.is_empty() {
        let has_coeff = fm_data.keys().any(|k| {
            k.starts_with(data_prefix) && k.contains("ThrustMaxCoeff_")
        });
        if !has_coeff {
            return None;
        }
        altitudes.push(0.0);
        velocities.push(0.0);
    } else if altitudes.is_empty() || velocities.is_empty() {
        return None;
    }

    let mut coefficients = Vec::new();
    for alt_idx in 0..altitudes.len() {
        let mut row = Vec::new();
        for vel_idx in 0..velocities.len() {
            let coeff = get_f64_first(
                fm_data,
                &[
                    &format!("{}Main.ThrustMax.ThrustMaxCoeff_{}_{}", data_prefix, alt_idx, vel_idx),
                    &format!("{}ThrustMax.ThrustMaxCoeff_{}_{}", data_prefix, alt_idx, vel_idx),
                ],
            )
            .unwrap_or(0.0);
            row.push(vec![coeff, coeff]);
        }
        coefficients.push(row);
    }

    let has_aft_coeff = fm_data.contains_key(&format!("{}Main.ThrustMax.ThrAftMaxCoeff_0_0", data_prefix))
        || fm_data.contains_key(&format!("{}ThrustMax.ThrAftMaxCoeff_0_0", data_prefix));

    if has_aft_coeff {
        for (alt_idx, alt_row) in coefficients.iter_mut().enumerate() {
            for (vel_idx, vel_col) in alt_row.iter_mut().enumerate() {
                if let Some(v) = get_f64_first(
                    fm_data,
                    &[
                        &format!("{}Main.ThrustMax.ThrAftMaxCoeff_{}_{}", data_prefix, alt_idx, vel_idx),
                        &format!("{}ThrustMax.ThrAftMaxCoeff_{}_{}", data_prefix, alt_idx, vel_idx),
                    ],
                ) {
                    vel_col[1] *= v;
                }
            }
        }
    }

    Some((altitudes, velocities, coefficients))
}

fn parse_piston_engine(fm_data: &HashMap<String, BlkxValue>, engine_idx: usize) -> Option<Engine> {
    let (engine_prefix, data_prefix) = get_engine_data_prefix(fm_data, engine_idx);

    let power = parse_piston_power(fm_data, &data_prefix, &engine_prefix)?;

    let throttle_boost = get_f64_first(fm_data, &[&format!("{}Main.ThrottleBoost", data_prefix)])
        .unwrap_or(1.0);
    let afterburner_boost = parse_jet_afterburner(fm_data, &data_prefix, &engine_prefix);
    let nitro_consumption =
        get_f64_first(fm_data, &[&format!("{}Main.NitroConsumption", data_prefix)])
            .unwrap_or(0.0);

    let engine_modes = parse_engine_modes(fm_data, &data_prefix, &engine_prefix, false);
    let compressor_stages = parse_compressor_stages(fm_data, &data_prefix, &engine_prefix, power);
    let (fuel_consumption_idle, fuel_consumption_half, fuel_consumption_full, fuel_consumption_wep, consumption_omega_max) =
        parse_fuel_consumptions(fm_data, &data_prefix, &engine_prefix);

    let (rpm_min, rpm_max, rpm_max_allowed) = parse_rpm_fields(fm_data, &data_prefix, &engine_prefix);

    Some(Engine {
        engine_type: EngineType::Piston,
        thrust_max: 0.0,
        power,
        rpm_min,
        rpm_max,
        rpm_max_allowed,
        afterburner_boost,
        throttle_boost,
        nitro_consumption,
        consumption_omega_max,
        compressor_stages,
        altitudes: Vec::new(),
        velocities: Vec::new(),
        coefficients: Vec::new(),
        fuel_consumption_idle,
        fuel_consumption_half,
        fuel_consumption_full,
        fuel_consumption_wep,
        engine_modes,
    })
}

fn parse_piston_power(
    fm_data: &HashMap<String, BlkxValue>,
    data_prefix: &str,
    engine_prefix: &str,
) -> Option<f64> {
    get_f64_first(
        fm_data,
        &[
            &format!("{}Main.Power", data_prefix),
            &format!("{}Power", data_prefix),
            &format!("{}Main.Power", engine_prefix),
            &format!("{}Power", engine_prefix),
        ],
    )
    .filter(|&v| v > 0.0)
}

fn parse_compressor_stages(
    fm_data: &HashMap<String, BlkxValue>,
    data_prefix: &str,
    engine_prefix: &str,
    default_power: f64,
) -> Vec<CompressorStage> {
    let prefixes = vec![
        data_prefix.to_string(),
        engine_prefix.to_string(),
        format!("{}Compressor.", data_prefix),
        format!("{}Compressor.", engine_prefix),
    ];
    let old_prefixes = vec![data_prefix.to_string(), engine_prefix.to_string()];
    let mut compressor_stages = Vec::new();

    // Try: Compressor.Altitude{i} with Power{i} / AfterburnerBoostMul{i} / Ceiling{i} / PowerAtCeiling{i}
    for prefix in &prefixes {
        for i in 0..10 {
            let alt_key = format!("{}Altitude{}", prefix, i);
            let power_key = format!("{}Power{}", prefix, i);
            let mul_key = format!("{}AfterburnerBoostMul{}", prefix, i);
            let ceil_key = format!("{}Ceiling{}", prefix, i);
            let pow_ceil_key = format!("{}PowerAtCeiling{}", prefix, i);

            if let Some(alt) = fm_data.get(&alt_key).and_then(|v| v.as_f64()) {
                let power_at_alt = fm_data.get(&power_key).and_then(|v| v.as_f64())
                    .or_else(|| fm_data.get(&mul_key).and_then(|v| v.as_f64()).map(|m| default_power * m))
                    .unwrap_or(default_power);
                let ceiling = fm_data.get(&ceil_key).and_then(|v| v.as_f64()).unwrap_or(alt * 1.6);
                let power_at_ceiling = fm_data.get(&pow_ceil_key).and_then(|v| v.as_f64())
                    .unwrap_or(power_at_alt * 0.3);
                compressor_stages.push(CompressorStage {
                    altitude: alt,
                    power: power_at_alt,
                    ceiling,
                    power_at_ceiling,
                });
            } else {
                break;
            }
        }
        if !compressor_stages.is_empty() {
            break;
        }
    }

    // Fallback to old format: CompressorAltitude{i} with CompressorPower{i}
    if compressor_stages.is_empty() {
        for prefix in &old_prefixes {
            for i in 0..10 {
                let alt_key = format!("{}CompressorAltitude{}", prefix, i);
                let power_key = format!("{}CompressorPower{}", prefix, i);

                if let Some(alt) = fm_data.get(&alt_key).and_then(|v| v.as_f64()) {
                    let power_at_alt = fm_data
                        .get(&power_key)
                        .and_then(|v| v.as_f64())
                        .unwrap_or(default_power);
                    compressor_stages.push(CompressorStage {
                        altitude: alt,
                        power: power_at_alt,
                        ceiling: alt * 1.5,
                        power_at_ceiling: power_at_alt * 0.6,
                    });
                } else {
                    break;
                }
            }
            if !compressor_stages.is_empty() {
                break;
            }
        }
    }

    compressor_stages
}

fn parse_rocket_engine(fm_data: &HashMap<String, BlkxValue>, engine_idx: usize) -> Option<Engine> {
    let (engine_prefix, data_prefix) = get_engine_data_prefix(fm_data, engine_idx);

    let thrust = parse_rocket_thrust(fm_data, &data_prefix, &engine_prefix)?;
    let afterburner_boost = parse_jet_afterburner(fm_data, &data_prefix, &engine_prefix);
    let engine_modes = parse_engine_modes(fm_data, &data_prefix, &engine_prefix, true);
    let (fuel_consumption_idle, fuel_consumption_half, fuel_consumption_full, fuel_consumption_wep, consumption_omega_max) =
        parse_fuel_consumptions(fm_data, &data_prefix, &engine_prefix);

    Some(Engine {
        engine_type: EngineType::Rocket,
        thrust_max: thrust,
        power: 0.0,
        rpm_min: 0.0,
        rpm_max: f64::MAX,
        rpm_max_allowed: f64::MAX,
        afterburner_boost,
        throttle_boost: 1.0,
        nitro_consumption: 0.0,
        consumption_omega_max,
        compressor_stages: Vec::new(),
        altitudes: Vec::new(),
        velocities: Vec::new(),
        coefficients: Vec::new(),
        fuel_consumption_idle,
        fuel_consumption_half,
        fuel_consumption_full,
        fuel_consumption_wep,
        engine_modes,
    })
}

fn parse_rocket_thrust(
    fm_data: &HashMap<String, BlkxValue>,
    data_prefix: &str,
    engine_prefix: &str,
) -> Option<f64> {
    get_f64_first(
        fm_data,
        &[
            &format!("{}Main.ThrustMax.ThrustMax0", data_prefix),
            &format!("{}ThrustMax.ThrustMax0", data_prefix),
            &format!("{}Main.Thrust", data_prefix),
            &format!("{}Main.Thrust", engine_prefix),
            &format!("{}Thrust", data_prefix),
        ],
    )
    .filter(|&v| v > 0.0)
}

pub fn read_all_engines(fm_data: &HashMap<String, BlkxValue>) -> Vec<Engine> {
    let mut engines = Vec::new();

    for engine_idx in 0..20 {
        let engine_type = detect_engine_type(fm_data, engine_idx);

        let engine = match engine_type {
            EngineType::Jet => parse_jet_engine(fm_data, engine_idx),
            EngineType::Piston => parse_piston_engine(fm_data, engine_idx),
            EngineType::Rocket => parse_rocket_engine(fm_data, engine_idx),
            _ => None,
        };

        if let Some(e) = engine {
            engines.push(e);
        }
    }

    engines
}

pub fn engine_count(fm_data: &HashMap<String, BlkxValue>) -> usize {
    read_all_engines(fm_data).len()
}

pub fn thrusts_at(engines: &[Engine], velocity: f64, altitude: f64, throttle: f64) -> f64 {
    engines
        .iter()
        .map(|e| e.get_thrust_at(velocity, altitude, throttle))
        .sum()
}

pub fn powers_at(engines: &[Engine], velocity: f64, altitude: f64, throttle: f64) -> f64 {
    engines
        .iter()
        .map(|e| e.get_power_at(velocity, altitude, throttle))
        .sum()
}

pub fn fuel_rate_at(engines: &[Engine], velocity: f64, altitude: f64, throttle: f64) -> f64 {
    engines
        .iter()
        .filter(|e| e.engine_type() == EngineType::Jet || e.engine_type() == EngineType::Piston)
        .map(|e| e.get_fuel_rate_at(velocity, altitude, throttle))
        .sum()
}

pub fn total_fuel_rates(engines: &[Engine]) -> EngineFuelRates {
    engines
        .iter()
        .filter(|e| e.engine_type() == EngineType::Jet || e.engine_type() == EngineType::Piston)
        .map(|e| e.fuel_rates())
        .reduce(EngineFuelRates::add)
        .unwrap_or_default()
}

pub fn total_nitro_consumption(engines: &[Engine]) -> f64 {
    engines.iter().map(|e| e.nitro_consumption()).sum()
}

fn merge_unique_values(engines: &[Engine], getter: impl Fn(&Engine) -> &Vec<f64>) -> Vec<f64> {
    let mut all_values: Vec<f64> = engines
        .iter()
        .flat_map(|e| getter(e).iter().copied())
        .collect();
    all_values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    all_values.dedup();
    if all_values.is_empty() || !all_values.contains(&0.0) {
        all_values.insert(0, 0.0);
    }
    all_values
}

pub fn get_merged_altitudes(engines: &[Engine]) -> Vec<f64> {
    merge_unique_values(engines, |e| &e.altitudes)
}

pub fn get_merged_velocities(engines: &[Engine]) -> Vec<f64> {
    merge_unique_values(engines, |e| &e.velocities)
}

pub fn get_thrust_table(engines: &[Engine], throttle: f64) -> Vec<Vec<f64>> {
    let altitudes = get_merged_altitudes(engines);
    let velocities = get_merged_velocities(engines);

    let mut table = Vec::new();
    for vel in &velocities {
        let mut row = Vec::new();
        for alt in &altitudes {
            let thrust = thrusts_at(engines, *vel, *alt, throttle);
            row.push(thrust);
        }
        table.push(row);
    }
    table
}

pub fn has_any_wep(engines: &[Engine]) -> bool {
    engines.iter().any(|e| e.has_wep())
}

pub fn print_thrust_table(engines: &[Engine], throttle: f64) {
    let altitudes = get_merged_altitudes(engines);
    let velocities = get_merged_velocities(engines);
    let table = get_thrust_table(engines, throttle);

    let title = if throttle > 100.0 { "WEP" } else { "Military" };
    println!("\n--- Thrust-Velocity-Altitude Table ({}) ---", title);
    print!("{:>12}", "Vel(km/h)");
    for alt in &altitudes {
        print!(" {:>10.0}", *alt);
    }
    println!();

    for (vel_idx, vel) in velocities.iter().enumerate() {
        print!("{:>12.0}", *vel);
        for alt_idx in 0..altitudes.len() {
            let thrust = table
                .get(vel_idx)
                .and_then(|row| row.get(alt_idx))
                .copied()
                .unwrap_or(0.0);
            print!(" {:>10.0}", thrust);
        }
        println!();
    }
}

pub fn print_compressor_stages(engines: &[Engine]) {
    for engine in engines {
        if engine.engine_type == EngineType::Piston && !engine.compressor_stages.is_empty() {
            println!("Compressor Stages: {}", engine.compressor_stages.len());
            for (i, stage) in engine.compressor_stages.iter().enumerate() {
                let wep_power = stage.power * engine.throttle_boost;
                println!(
                    "  Stage {}: Alt={:.0}m, Power={:.0}hp, WEP={:.0}hp, Ceiling={:.0}m, CeilPow={:.0}hp",
                    i, stage.altitude, stage.power, wep_power, stage.ceiling, stage.power_at_ceiling
                );
            }
        }
    }
}


pub fn print_power_altitude_table(engines: &[Engine], throttle: f64) {
    if engines.is_empty() || engines[0].engine_type != EngineType::Piston {
        return;
    }

    let engine = &engines[0];
    let stages = &engine.compressor_stages;
    if stages.is_empty() {
        return;
    }

    println!(
        "\n--- Power-Altitude Table ({}) ---",
        if throttle > 100.0 { "WEP" } else { "Military" }
    );
    print!("{:>12}", "Alt(m)");
    for stage in stages {
        print!(" {:>10.0}", stage.altitude);
    }
    println!();

    print!("{:>12}", "Power(hp)");
    for stage in stages {
        let power = if throttle > 100.0 {
            stage.power * engine.throttle_boost
        } else {
            stage.power
        };
        print!(" {:>10.0}", power);
    }
    println!();
}

pub fn print_power_table(engines: &[Engine], throttle: f64) {
    let altitudes = get_merged_altitudes(engines);
    let velocities = get_merged_velocities(engines);

    let title = if throttle > 100.0 { "WEP" } else { "Military" };
    println!("\n--- Power-Velocity-Altitude Table ({}) ---", title);
    print!("{:>12}", "Vel(km/h)");
    for alt in &altitudes {
        print!(" {:>10.0}", *alt);
    }
    println!();

    for vel in &velocities {
        print!("{:>12.0}", *vel);
        for alt in &altitudes {
            let power = powers_at(engines, *vel, *alt, throttle);
            print!(" {:>10.0}", power);
        }
        println!();
    }
}

pub fn print_engine_modes(engines: &[Engine]) {
    if engines.is_empty() {
        return;
    }

    let mut printed = false;
    for engine in engines {
        if !engine.engine_modes.is_empty() && !printed {
            println!("Consumption Mode Multipliers:");
            for (i, mode) in engine.engine_modes.iter().enumerate() {
                println!(
                    "  Mode {}: Throttle={:.0}, RPM={:.0}, Mult={:.2}",
                    i, mode.throttle, mode.rpm, mode.consumption_mult
                );
            }
            printed = true;
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompressorStage {
    pub altitude: f64,
    pub power: f64,
    pub ceiling: f64,
    pub power_at_ceiling: f64,
}

fn bilinear_interp(
    velocities: &[f64],
    altitudes: &[f64],
    coeffs: &[Vec<Vec<f64>>],
    vel: f64,
    alt: f64,
    wep_idx: usize,
) -> f64 {
    if velocities.is_empty() || altitudes.is_empty() || coeffs.is_empty() {
        return 0.0;
    }
    let min_a = altitudes[0];
    let max_a = *altitudes.last().unwrap();

    // 不要clamp，让外推值自然计算
    let v = vel;
    let a = alt.clamp(min_a, max_a);

    let vi = find_lower_bound(velocities, v).min(velocities.len().saturating_sub(2));
    let ai = find_lower_bound(altitudes, a).min(altitudes.len().saturating_sub(2));

    let v0 = velocities[vi];
    let v1 = velocities[vi + 1];
    let a0 = altitudes[ai];
    let a1 = altitudes[ai + 1];

    let get_coeff = |ai: usize, vi: usize| -> f64 {
        coeffs
            .get(ai)
            .and_then(|row| row.get(vi))
            .and_then(|wep_arr| wep_arr.get(wep_idx).copied())
            .unwrap_or(0.0)
    };

    let q00 = get_coeff(ai, vi);
    let q10 = get_coeff(ai, vi + 1);
    let q01 = get_coeff(ai + 1, vi);
    let q11 = get_coeff(ai + 1, vi + 1);

    let tx = (v - v0) / (v1 - v0);
    let ty = (a - a0) / (a1 - a0);

    let i0 = q00 * (1.0 - tx) + q10 * tx;
    let i1 = q01 * (1.0 - tx) + q11 * tx;

    i0 * (1.0 - ty) + i1 * ty
}

fn find_lower_bound(arr: &[f64], value: f64) -> usize {
    for (i, &v) in arr.iter().enumerate() {
        if value <= v {
            return if i == 0 { 0 } else { i - 1 };
        }
    }
    arr.len() - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合成一张扁平表（不依赖任何解析 feature / 磁盘数据）。
    fn map(pairs: &[(&str, &str)]) -> HashMap<String, BlkxValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), BlkxValue::from_raw(v.to_string())))
            .collect()
    }

    /// `EngineType{N}.Main.Type` 缺键（或值不认识）→ **默认活塞**，不看 `Main.Power` /
    /// `ThrustMax.ThrustMax0` 做启发式。
    ///
    /// 这条必须能失败：把 `unwrap_or(EngineType::Piston)` 换成按 Power/ThrustMax 推断，
    /// 第一个用例（有 ThrustMax、没有 Type）就会得到 `Jet` 而红。
    #[test]
    fn missing_engine_type_key_defaults_to_piston() {
        // ① 缺 Type、**有 ThrustMax**：旧启发式给 Jet —— 现在必须是 Piston（缺键只出现在活塞机上）
        let m = map(&[
            ("Engine0.Type", "0"),
            ("EngineType0.Main.ThrustMax.ThrustMax0", "8000"),
        ]);
        assert_eq!(
            detect_engine_type(&m, 0),
            EngineType::Piston,
            "缺 EngineType0.Main.Type 时必须默认活塞（用户口径：不再按 ThrustMax 推断）"
        );

        // ② 缺 Type、有 Power：同样 Piston（这条以前也成立，留着防"改成 Unknown"）
        let m = map(&[("Engine0.Type", "0"), ("EngineType0.Main.Power", "930")]);
        assert_eq!(detect_engine_type(&m, 0), EngineType::Piston);

        // ③ 连 Engine{N}.Type 都没有（只有 Engine{N}.Main.*）：还是 Piston
        let m = map(&[("Engine0.Main.ThrustMax.ThrustMax0", "8000")]);
        assert_eq!(detect_engine_type(&m, 0), EngineType::Piston);

        // ④ 值不认识（拼写变了）：也走"默认活塞"，不再是 Unknown
        let m = map(&[("Engine0.Type", "0"), ("EngineType0.Main.Type", "turbofan")]);
        assert_eq!(detect_engine_type(&m, 0), EngineType::Piston);
    }

    /// **反例（防误判）**：有 Type 键时照旧按字符串判 —— 喷气绝不能被"默认活塞"吞掉。
    ///
    /// 取值用**真实数据里的写法**（实测分布：`Jet` 464 台 / `Radial` 588 / `Inline` 409 /
    /// `TurboProp` 118 / `Rocket` 37 / `PVRD` 1）。
    #[test]
    fn explicit_engine_type_string_still_wins() {
        for (text, want) in [
            ("Jet", EngineType::Jet),
            ("Radial", EngineType::Piston),
            ("Inline", EngineType::Piston),
            ("Rocket", EngineType::Rocket),
        ] {
            let m = map(&[("Engine0.Type", "0"), ("EngineType0.Main.Type", text)]);
            assert_eq!(detect_engine_type(&m, 0), want, "EngineType0.Main.Type={text:?}");
            // `Engine{N}.Main.Type` 那条回退路（没有 Engine{N}.Type 时）同样按字符串判
            let m = map(&[("Engine0.Main.Type", text)]);
            assert_eq!(detect_engine_type(&m, 0), want, "Engine0.Main.Type={text:?}");
        }
        // ⚠️ 已知的历史毛刺（**本次改动之前就是这样，未改**）：`parse_engine_type_string` 是
        // **大小写敏感**的，数据里写的是 `TurboProp`，而这里找的是 "Turboprop" → 认不出 →
        // 走"默认活塞"。用户可见的「引擎」那一格来自 `FlightModel::engine_type`
        // （`flight_model.rs`，那边先 lowercase 再判，`nt_tu_95m`/`wyvern_s4` 正确显示涡桨），
        // 所以这里不影响界面文案；改它会动到引擎列表的燃油/加力判定，不在本次口径内。
        let m = map(&[("Engine0.Type", "0"), ("EngineType0.Main.Type", "TurboProp")]);
        assert_eq!(detect_engine_type(&m, 0), EngineType::Piston);
        // 认不出来的值（`PVRD`：真数据里有 1 台）同样落到默认活塞
        let m = map(&[("Engine0.Type", "0"), ("EngineType0.Main.Type", "PVRD")]);
        assert_eq!(detect_engine_type(&m, 0), EngineType::Piston);
    }
}
