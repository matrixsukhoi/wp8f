use crate::aero::{cl_max_mach_corrected, cx0_mach_mult, get_drag_coefficient, read_aero_surfaces, AeroSurface, VariableSweepWing};
use crate::engine_model::{
    fuel_rate_at, has_any_wep, powers_at, read_all_engines, thrusts_at,
    total_nitro_consumption, CompressorStage, Engine, EngineType,
};
use crate::parser::{get_f64_first, get_f64_first_value, get_f64_multi, get_string_first, parse_fm_text, BlkxValue};
use crate::weapon_model::{load_weapon, load_weapon_with_preset, load_weapons_from_fm, BeltInfo};
use std::collections::HashMap;
use std::path::Path;

/// CritOverload 取值键（优先级从高到低），**单位牛顿**（p2 = (负向, 正向)），换算见
/// `allowed_load_factor`。变后掠机型（F-14/F-111/MiG-23/Su-17/Tornado 等）的值在
/// `WingPlaneSweep{0..3}.Strength` 下且没有顶层兜底 → 取 Sweep0（最小后掠档）。
const CRIT_OVERLOAD_KEYS: [&str; 7] = [
    "Aerodynamics.WingPlane.Strength.CritOverload",
    "WingPlane.Strength.CritOverload",
    "Aerodynamics.WingPlaneSweep0.Strength.CritOverload",
    "WingPlaneSweep0.Strength.CritOverload",
    "Strength.CritOverload",
    "Mass.WingCritOverload",
    "WingCritOverload",
];

#[derive(Debug, Clone, Default)]
pub struct DragBreakdown {
    pub velocity_ms: f64,
    pub altitude_m: f64,
    pub weight_n: f64,
    pub dynamic_pressure: f64,
    pub lift_coeff: f64,
    pub drag_area: f64,
    pub parasitic_drag: f64,
    pub induced_drag_factor: f64,
    pub induced_drag: f64,
    pub radiator_drag: f64,
    pub oil_radiator_drag: f64,
    pub airbrake_drag: f64,
    pub mach: f64,
    pub wave_drag: f64,
    pub total_drag: f64,
}

impl DragBreakdown {
    pub fn print(&self) {
        println!(
            "=== 阻力分解 (v={:.0} m/s, h={:.0} m) ===",
            self.velocity_ms, self.altitude_m
        );
        println!("  动压 q: {:.1} Pa", self.dynamic_pressure);
        println!("  升力系数 Cl: {:.3}", self.lift_coeff);
        println!("  马赫数: {:.3}", self.mach);
        println!();
        println!("  寄生阻力:");
        println!("    阻力面积 CdS: {:.3}", self.drag_area);
        println!("    寄生阻力 D: {:.0} N", self.parasitic_drag);
        println!();
        println!("  诱导阻力:");
        println!("    诱导因子 k: {:.4}", self.induced_drag_factor);
        println!("    诱导阻力 D: {:.0} N", self.induced_drag);
        println!();
        println!("  附加阻力:");
        println!("    散热器: {:.0} N", self.radiator_drag);
        println!("    油冷器: {:.0} N", self.oil_radiator_drag);
        println!("    减速板: {:.0} N", self.airbrake_drag);
        if self.wave_drag > 0.0 {
            println!("    波阻: {:.0} N", self.wave_drag);
        }
        println!();
        println!("  === 总阻力: {:.0} N ===", self.total_drag);
    }
}

#[derive(Debug, Clone)]
pub struct Aerodynamics {
    pub wingspan: f64,
    pub wing_area: f64,
    pub aspect_ratio: f64,
    pub cd_min: f64,
    pub cl_max_no_flaps: f64,
    pub cl_max_full_flaps: f64,
    pub aoa_cl_max_no_flaps: f64,
    pub aoa_cl_max_full_flaps: f64,
    pub oswalds_efficiency: f64,
    pub radiator_cd: f64,
    pub oil_radiator_cd: f64,
    pub airbrake_cd: f64,
    pub elevator_effective_speed: f64,
    pub aileron_effective_speed: f64,
    pub rudder_effective_speed: f64,
    pub wing_geometry: VariableSweepWing,
}

impl Default for Aerodynamics {
    fn default() -> Self {
        Self {
            wingspan: 10.0,
            wing_area: 20.0,
            aspect_ratio: 5.0,
            cd_min: 0.02,
            cl_max_no_flaps: 1.5,
            cl_max_full_flaps: 1.5,
            aoa_cl_max_no_flaps: 15.0,
            aoa_cl_max_full_flaps: 25.0,
            oswalds_efficiency: 0.8,
            radiator_cd: 0.0,
            oil_radiator_cd: 0.0,
            airbrake_cd: 0.0,
            elevator_effective_speed: 0.0,
            aileron_effective_speed: 0.0,
            rudder_effective_speed: 0.0,
            wing_geometry: VariableSweepWing::default(),
        }
    }
}

impl Aerodynamics {
    fn parse(fm_data: &HashMap<String, BlkxValue>, reference_area: f64) -> Self {
        let wing_geometry = VariableSweepWing::read_from(fm_data);

        let wingspan = wing_geometry.span();
        let wing_area = if reference_area > 0.0 { reference_area } else { wing_geometry.wing_area() };
        let aspect_ratio = if wing_area > 0.0 { wingspan * wingspan / wing_area } else { 5.0 };

        Self {
            wingspan,
            wing_area,
            aspect_ratio,
            cd_min: wing_geometry.cd_min(),
            cl_max_no_flaps: wing_geometry.wing_sweep0.cl_max_no_flaps(),
            cl_max_full_flaps: wing_geometry.wing_sweep0.cl_max_full_flaps(),
            aoa_cl_max_no_flaps: wing_geometry.wing_sweep0.aoa_cl_max_no_flaps(),
            aoa_cl_max_full_flaps: wing_geometry.wing_sweep0.aoa_cl_max_full_flaps(),
            oswalds_efficiency: wing_geometry.oswalds_efficiency(),
            radiator_cd: Self::parse_radiator_cd(fm_data),
            oil_radiator_cd: Self::parse_oil_radiator_cd(fm_data),
            airbrake_cd: Self::parse_airbrake_cd(fm_data),
            elevator_effective_speed: Self::parse_elevator_effective_speed(fm_data),
            aileron_effective_speed: Self::parse_aileron_effective_speed(fm_data),
            rudder_effective_speed: Self::parse_rudder_effective_speed(fm_data),
            wing_geometry,
        }
    }

    fn parse_radiator_cd(fm_data: &HashMap<String, BlkxValue>) -> f64 {
        get_f64_first(fm_data, &["Aerodynamics.RadiatorCd", "RadiatorCd"]).unwrap_or(0.0)
    }

    fn parse_oil_radiator_cd(fm_data: &HashMap<String, BlkxValue>) -> f64 {
        get_f64_first(fm_data, &["Aerodynamics.OilRadiatorCd", "OilRadiatorCd"]).unwrap_or(0.0)
    }

    fn parse_airbrake_cd(fm_data: &HashMap<String, BlkxValue>) -> f64 {
        get_f64_first(fm_data, &["Aerodynamics.AirbrakeCd", "AirbrakeCd"]).unwrap_or(0.0)
    }

    fn parse_elevator_effective_speed(fm_data: &HashMap<String, BlkxValue>) -> f64 {
        // p2 多值（如 "950.005, 950.005"）：取第一分量，单值 as_f64 会解析失败
        get_f64_first_value(fm_data, &[
            "ElevatorsEffectiveSpeed",
            "Aerodynamics.ElevatorsEffectiveSpeed",
            "Controls.ElevatorsEffectiveSpeed",
        ]).unwrap_or(0.0)
    }

    fn parse_aileron_effective_speed(fm_data: &HashMap<String, BlkxValue>) -> f64 {
        get_f64_first_value(fm_data, &[
            "AileronEffectiveSpeed",
            "Aerodynamics.AileronEffectiveSpeed",
            "Controls.AileronEffectiveSpeed",
        ]).unwrap_or(0.0)
    }

    fn parse_rudder_effective_speed(fm_data: &HashMap<String, BlkxValue>) -> f64 {
        get_f64_first_value(fm_data, &[
            "RudderEffectiveSpeed",
            "Aerodynamics.RudderEffectiveSpeed",
            "Controls.RudderEffectiveSpeed",
        ]).unwrap_or(0.0)
    }

    pub fn control_surface_effective_speeds(&self) -> [f64; 3] {
        [
            self.elevator_effective_speed,
            self.aileron_effective_speed,
            self.rudder_effective_speed,
        ]
    }

    pub fn get_vne(&self, wing_sweep: f64) -> f64 {
        self.wing_geometry.get_vne(wing_sweep)
    }

    pub fn get_cl_aoa_max(&self, flaps: f64, wing_sweep: f64) -> (f64, f64) {
        self.wing_geometry.get_cl_aoa_max(flaps, wing_sweep)
    }

    /// 负向临界攻角（度，负值），口径同 `get_cl_aoa_max`
    pub fn get_aoa_min(&self, flaps: f64, wing_sweep: f64) -> f64 {
        self.wing_geometry.get_aoa_min(flaps, wing_sweep)
    }

    // TODO: Mach correction via wing_geometry polars
    pub fn cl_at_mach(&self, aoa: f64, flaps: f64, wing_sweep: f64, _mach: f64) -> f64 {
        let (cl_max, aoa_high) = self.wing_geometry.get_cl_aoa_max(flaps, wing_sweep);
        if aoa >= aoa_high { cl_max }
        else if aoa <= -aoa_high { -cl_max }
        else { self.wing_geometry.wing_sweep0.no_flaps_polar.cl0 + self.wing_geometry.wing_sweep0.no_flaps_polar.line_cl_coeff * aoa }
    }

    pub fn vne_mach(&self) -> f64 {
        self.wing_geometry.vne_mach()
    }

    pub fn has_sweep_data(&self) -> bool {
        self.wing_geometry.is_variable_sweep()
    }

    pub fn get_ias_warning_line(&self, wing_sweep: f64) -> f64 {
        let vne_interp = self.get_vne(wing_sweep);
        if vne_interp > 0.0 {
            vne_interp * 0.95
        } else {
            3000.0
        }
    }

    pub fn get_mach_warning_line(&self, wing_sweep: f64) -> f64 {
        let mne_interp = self.wing_geometry.get_vne_mach(wing_sweep);

        if mne_interp > 0.0 {
            mne_interp * 0.95
        } else {
            10.0
        }
    }
}

#[derive(Debug, Clone)]
pub struct Propulsion {
    pub engines: Vec<Engine>,
    pub engine_count: usize,
    pub total_thrust_max: f64,
    pub engine_power: f64,
    pub has_wep: bool,
}

impl Default for Propulsion {
    fn default() -> Self {
        Self {
            engines: Vec::new(),
            engine_count: 0,
            total_thrust_max: 0.0,
            engine_power: 0.0,
            has_wep: false,
        }
    }
}

impl Propulsion {
    fn parse(fm_data: &HashMap<String, BlkxValue>, _engine_type: EngineType) -> Self {
        let engines = read_all_engines(fm_data);
        let engine_count = engines.len();
        let has_wep = has_any_wep(&engines);

        let total_thrust_max: f64 = engines.iter().map(|e| e.thrust_max).sum();

        let engine_power = if total_thrust_max > 0.0 {
            0.0
        } else {
            fm_data
                .get("EngineType0.Main.Power")
                .or_else(|| fm_data.get("Engine0.Main.Power"))
                .or_else(|| fm_data.get("Main.Power"))
                .or_else(|| fm_data.get("EngineType0.Power"))
                .or_else(|| fm_data.get("Engine0.Power"))
                .or_else(|| fm_data.get("Power"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
        };

        Self {
            engines,
            engine_count,
            total_thrust_max,
            engine_power,
            has_wep,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MassProperties {
    pub empty_weight: f64,
    pub max_fuel: f64,
    pub max_external_fuel: f64,
    pub oil_mass: f64,
    pub pilot_mass: f64,
    pub nitro: f64,
}

#[derive(Debug, Clone)]
pub struct Weapon {
    pub trigger: String,
    pub blk: String,
    pub bullets: usize,
    pub mass: f64,
    pub bullet_mass: f64,
    pub belts: Vec<BeltInfo>,
}

#[derive(Debug, Clone)]
pub struct WeaponSlot {
    pub index: usize,
    pub preset_name: String,
    pub weapons: Vec<Weapon>,
}

#[derive(Debug, Clone, Default)]
pub struct LoadoutWeight {
    pub cannon_weight: f64,
    pub ammo_weight: f64,
    pub countermeasures_weight: f64,
    pub total_weapons_weight: f64,
    pub weapon_slots: Vec<WeaponSlot>,
}

impl LoadoutWeight {
    fn parse(
        data: &HashMap<String, BlkxValue>,
        fm_data: &HashMap<String, BlkxValue>,
        weapons_dir: Option<&Path>,
    ) -> Self {
        let (cannon_weight, ammo_weight) = if let Some(weapons_dir) = weapons_dir {
            let dir_str = weapons_dir.to_str().unwrap_or("");
            let mut weapons = load_weapons_from_fm(fm_data, dir_str);
            weapons.extend(load_weapons_from_fm(data, dir_str));

            let cannon_weight = weapons
                .iter()
                .filter(|w| w.name.to_lowercase().contains("cannon"))
                .map(|w| w.mass)
                .sum();

            let ammo_weight = weapons
                .iter()
                .filter(|w| !w.name.to_lowercase().contains("countermeasure"))
                .map(|w| w.bullet_mass * w.ammo_count as f64)
                .sum();

            (cannon_weight, ammo_weight)
        } else {
            (
                Self::parse_cannon_weight(data, fm_data),
                Self::parse_ammo_weight(data, fm_data),
            )
        };

        let countermeasures_weight = if let Some(weapons_dir) = weapons_dir {
            let dir_str = weapons_dir.to_str().unwrap_or("");
            let mut cm_weapons = load_weapons_from_fm(fm_data, dir_str);
            cm_weapons.extend(load_weapons_from_fm(data, dir_str));
            cm_weapons
                .iter()
                .filter(|w| w.name.to_lowercase().contains("countermeasure"))
                .map(|w| w.mass)
                .sum()
        } else {
            Self::parse_countermeasures_weight(data, fm_data)
        };

        Self {
            cannon_weight,
            ammo_weight,
            countermeasures_weight,
            total_weapons_weight: cannon_weight + ammo_weight + countermeasures_weight,
            weapon_slots: Vec::new(),
        }
    }

    fn parse_cannon_weight(data: &HashMap<String, BlkxValue>, fm_data: &HashMap<String, BlkxValue>) -> f64 {
        get_f64_multi(&[fm_data, data], &[
            "WeaponMass",
            "CannonMass",
            "GunMass",
            "Weapons.Cannon.Mass",
            "mass",
        ]).unwrap_or(0.0)
    }

    fn parse_ammo_weight(data: &HashMap<String, BlkxValue>, fm_data: &HashMap<String, BlkxValue>) -> f64 {
        let ammo_count = get_f64_multi(&[fm_data, data], &[
            "WeaponAmmo",
            "CannonAmmo",
            "GunAmmo",
            "Weapons.Cannon.Ammo",
            "bullets",
        ]).unwrap_or(0.0);

        let bullet_mass = get_f64_multi(&[fm_data, data], &[
            "BulletMass",
            "AmmoMass",
            "Weapons.Cannon.BulletMass",
        ]).unwrap_or(0.176);

        ammo_count * bullet_mass
    }

    fn parse_countermeasures_weight(data: &HashMap<String, BlkxValue>, fm_data: &HashMap<String, BlkxValue>) -> f64 {
        let flare_count = get_f64_multi(&[fm_data, data], &[
            "FlareCount",
            "Countermeasure.FlareCount",
            "countermeasures_flare_count",
        ]).unwrap_or(0.0);
        let flare_mass = get_f64_multi(&[fm_data, data], &[
            "FlareMass",
            "Countermeasure.FlareMass",
        ]).unwrap_or(0.09);

        let chaff_count = get_f64_multi(&[fm_data, data], &[
            "ChaffCount",
            "Countermeasure.ChaffCount",
            "countermeasures_chaff_count",
        ]).unwrap_or(0.0);
        let chaff_mass = get_f64_multi(&[fm_data, data], &[
            "ChaffMass",
            "Countermeasure.ChaffMass",
        ]).unwrap_or(0.1);

        flare_count * flare_mass + chaff_count * chaff_mass
    }

}

pub fn parse_weapon_slots_from_data(
    data: &HashMap<String, BlkxValue>,
    weapons_dir: Option<&Path>,
) -> Vec<WeaponSlot> {
    // WeaponSlots { WeaponSlot { ... } } format
    let slots = parse_indexed_slots(data, "WeaponSlots.WeaponSlot", weapons_dir);
    if !slots.is_empty() { return slots; }
    // Fallback: commonWeapons { Weapon { ... } } format (Yak-3, etc.)
    parse_common_weapons(data, weapons_dir)
}

fn parse_indexed_slots(
    data: &HashMap<String, BlkxValue>,
    base_key: &str,
    weapons_dir: Option<&Path>,
) -> Vec<WeaponSlot> {
    (0..)
        .map(|i| if i == 0 { base_key.to_string() } else { format!("{}[{}]", base_key, i) })
        .take_while(|prefix| data.contains_key(&format!("{}.index", prefix)))
        .filter_map(|prefix| {
            let index = get_f64_first(data, &[&format!("{}.index", prefix)]).unwrap_or(0.0) as usize;
            let preset_prefix = format!("{}.WeaponPreset", prefix);
            let preset_name = get_string_first(data, &[&format!("{}.name", preset_prefix)]).unwrap_or_default();
            let weapons = parse_indexed_weapons(data, &format!("{}.Weapon", preset_prefix), &preset_name, weapons_dir);
            if weapons.is_empty() { None } else {
                Some(WeaponSlot { index, preset_name, weapons })
            }
        })
        .collect()
}

fn parse_indexed_weapons(
    data: &HashMap<String, BlkxValue>,
    base_key: &str,
    preset_name: &str,
    weapons_dir: Option<&Path>,
) -> Vec<Weapon> {
    (0..)
        .map(|i| if i == 0 { base_key.to_string() } else { format!("{}[{}]", base_key, i) })
        .take_while(|prefix| data.contains_key(&format!("{}.trigger", prefix)))
        .filter_map(|prefix| {
            let trigger = get_string_first(data, &[&format!("{}.trigger", prefix)])?;
            let blk = get_string_first(data, &[&format!("{}.blk", prefix)])?;
            let bullets = get_f64_first(data, &[&format!("{}.bullets", prefix)]).unwrap_or(0.0) as usize;
            let (mass, bullet_mass, belts) = load_weapon_mass(&blk, preset_name, weapons_dir);
            Some(Weapon { trigger, blk, bullets, mass, bullet_mass, belts })
        })
        .collect()
}

fn parse_common_weapons(
    data: &HashMap<String, BlkxValue>,
    weapons_dir: Option<&Path>,
) -> Vec<WeaponSlot> {
    let weapons: Vec<Weapon> = (0..)
        .map(|i| if i == 0 { "commonWeapons.Weapon".to_string() } else { format!("commonWeapons.Weapon[{}]", i) })
        .take_while(|prefix| data.contains_key(&format!("{}.trigger", prefix)))
        .filter_map(|prefix| {
            let trigger = get_string_first(data, &[&format!("{}.trigger", prefix)])?;
            let blk = get_string_first(data, &[&format!("{}.blk", prefix)])?;
            let bullets = get_f64_first(data, &[&format!("{}.bullets", prefix)]).unwrap_or(0.0) as usize;
            let (mass, bullet_mass, belts) = load_weapon_mass(&blk, "stealth", weapons_dir);
            Some(Weapon { trigger, blk, bullets, mass, bullet_mass, belts })
        })
        .collect();
    if weapons.is_empty() { Vec::new() } else {
        vec![WeaponSlot { index: 0, preset_name: "default".to_string(), weapons }]
    }
}

fn load_weapon_mass(blk: &str, preset: &str, weapons_dir: Option<&Path>) -> (f64, f64, Vec<BeltInfo>) {
    let Some(dir) = weapons_dir else { return (0.0, 0.0, Vec::new()) };
    let weapon_path = blk
        .trim_start_matches("gameData/Weapons/")
        .trim_end_matches(".blk")
        .trim_end_matches(".blkx");
    let dir_str = dir.to_str().unwrap_or("");
    let w = if preset.is_empty() {
        load_weapon(weapon_path, dir_str)
    } else {
        load_weapon_with_preset(weapon_path, preset, dir_str)
    };
    match w {
        Some(w) => (w.mass, w.bullet_mass, w.belts),
        None => (0.0, 0.0, Vec::new()),
    }
}

impl MassProperties {
    fn parse(data: &HashMap<String, BlkxValue>, fm_data: &HashMap<String, BlkxValue>) -> Self {
        Self {
            empty_weight: Self::parse_empty_weight(data, fm_data),
            max_fuel: Self::parse_max_fuel(data, fm_data),
            max_external_fuel: Self::parse_max_external_fuel(data, fm_data),
            oil_mass: Self::parse_oil_mass(data, fm_data),
            pilot_mass: Self::parse_pilot_mass(data, fm_data),
            nitro: Self::parse_nitro(data, fm_data),
        }
    }

    fn parse_empty_weight(data: &HashMap<String, BlkxValue>, fm_data: &HashMap<String, BlkxValue>) -> f64 {
        get_f64_multi(&[fm_data, data], &[
            "EmptyMass",
            "Mass.EmptyMass",
            "emptyWeight",
            "Mass.emptyWeight",
        ]).unwrap_or(0.0)
    }

    fn parse_max_fuel(data: &HashMap<String, BlkxValue>, fm_data: &HashMap<String, BlkxValue>) -> f64 {
        get_f64_multi(&[fm_data, data], &["MaxFuelMass0", "Mass.MaxFuelMass0"]).unwrap_or(0.0)
    }

    fn parse_max_external_fuel(data: &HashMap<String, BlkxValue>, fm_data: &HashMap<String, BlkxValue>) -> f64 {
        get_f64_multi(&[fm_data, data], &["MaxFuelMassExternal0", "Mass.MaxFuelMassExternal0"]).unwrap_or(0.0)
    }

    fn parse_oil_mass(data: &HashMap<String, BlkxValue>, fm_data: &HashMap<String, BlkxValue>) -> f64 {
        get_f64_multi(&[fm_data, data], &["OilMass", "Mass.OilMass"]).unwrap_or(0.0)
    }

    fn parse_pilot_mass(_data: &HashMap<String, BlkxValue>, _fm_data: &HashMap<String, BlkxValue>) -> f64 {
        90.0
    }

    fn parse_nitro(data: &HashMap<String, BlkxValue>, fm_data: &HashMap<String, BlkxValue>) -> f64 {
        get_f64_multi(&[fm_data, data], &["MaxNitro", "Mass.MaxNitro", "Afterburner.MaxNitro"]).unwrap_or(0.0)
    }

    pub fn takeoff_weight(&self, fuel_percent: f64) -> f64 {
        self.empty_weight + self.max_fuel * (fuel_percent / 100.0)
    }

    pub fn flight_weight(&self, fuel_kg: f64) -> f64 {
        self.empty_weight + self.oil_mass + self.pilot_mass + self.nitro + fuel_kg
    }
}

#[derive(Debug, Clone, Default)]
pub struct FlightModel {
    pub name: String,
    pub fm_path: String,
    pub data: HashMap<String, BlkxValue>,
    pub fm_data: HashMap<String, BlkxValue>,
    pub engine_type: EngineType,
    pub is_jet: bool,
    pub wing: Option<AeroSurface>,
    pub fuselage: Option<AeroSurface>,
    pub hor_stab: Option<AeroSurface>,
    pub reference_area: f64,
    pub aerodynamics: Aerodynamics,
    pub propulsion: Propulsion,
    pub mass: MassProperties,
    pub loadout: LoadoutWeight,
    pub rpm_min: f64,
    pub rpm_max: f64,
    pub rpm_max_allowed: f64,
    pub gear_warning_speed: f64,
    cached_flap_pairs: Vec<(f64, f64)>,
    pub engine_loads: Vec<EngineLoad>,
}

#[derive(Debug, Clone)]
pub struct EngineLoad {
    pub water_limit: f64,
    pub oil_limit: f64,
    pub work_time: f64,
    pub recover_time: f64,
}

impl FlightModel {
    pub fn default_placeholder() -> Self {
        Self {
            name: String::new(),
            fm_path: String::new(),
            data: HashMap::new(),
            fm_data: HashMap::new(),
            engine_type: EngineType::Jet,
            is_jet: true,
            wing: None,
            fuselage: None,
            hor_stab: None,
            reference_area: 0.0,
            aerodynamics: Aerodynamics::default(),
            propulsion: Propulsion::default(),
            mass: MassProperties::default(),
            loadout: LoadoutWeight::default(),
            rpm_min: 0.0,
            rpm_max: f64::MAX,
            rpm_max_allowed: f64::MAX,
            gear_warning_speed: f64::MAX,
            cached_flap_pairs: Vec::new(),
            engine_loads: Vec::new(),
        }
    }

    /// 解析（**不失败**的兼容入口）。
    ///
    /// 解析出错时打一行 `[WARN]` 并退回占位模型（legacy 解析器
    /// 不会失败）。需要把可读错误交给上层的调用点请用 [`FlightModel::parse_checked`]。
    pub fn parse(blkx_content: &str, fm_content: Option<&str>, weapons_dir: Option<&Path>) -> Self {
        Self::parse_checked(blkx_content, fm_content, weapons_dir).unwrap_or_else(|e| {
            eprintln!("[WARN] FM 解析失败: {e}");
            Self::default_placeholder()
        })
    }

    /// 解析（可报错）：数据是另一种格式而本构建没带对应 feature 时（例如 legacy-only 收到 JSON），
    /// 把可读错误原样返回，而不是静默产出"性能全是 0"的空模型。
    pub fn parse_checked(
        blkx_content: &str,
        fm_content: Option<&str>,
        weapons_dir: Option<&Path>,
    ) -> Result<Self, String> {
        let data = parse_fm_text(blkx_content)?;
        let fm_data = match fm_content {
            Some(c) => parse_fm_text(c)?,
            None => HashMap::new(),
        };

        let name = data
            .get("model")
            .or_else(|| fm_data.get("model"))
            .map(|v| v.as_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let engine_type_str = data
            .get("EngineType0.Main.Type")
            .or_else(|| fm_data.get("EngineType0.Main.Type"))
            .or_else(|| fm_data.get("Engine0.Main.Type"))
            .or_else(|| fm_data.get("Main.Type"))
            .map(|v| v.as_string().to_lowercase())
            .unwrap_or_default();

        let engine_type =
            if engine_type_str.contains("piston") || engine_type_str.contains("inline") {
                EngineType::Piston
            } else if engine_type_str.contains("jet") {
                EngineType::Jet
            } else if engine_type_str.contains("turboprop") {
                EngineType::Turboprop
            } else if engine_type_str.contains("rocket") {
                EngineType::Rocket
            } else {
                // `EngineType0.Main.Type` 缺键或值不认识 → 默认活塞：不做"有 Power = 活塞 /
                // 有 ThrustMax = 喷气"的启发式（缺键实际只出现在活塞机上）。
                // 反例断言见 `engine_model.rs::missing_engine_type_key_defaults_to_piston`。
                EngineType::Piston
            };

        let is_jet = engine_type == EngineType::Jet;

        // 加载气动数据
        let (wing, fuselage, hor_stab, reference_area) = read_aero_surfaces(&fm_data);

        // Cache flap destruction pairs
        let cached_flap_pairs = Self::compute_flap_destruction_pairs(&fm_data);

        // 解析子结构
        let aerodynamics = Aerodynamics::parse(&fm_data, reference_area);
        let propulsion = Propulsion::parse(&fm_data, engine_type);
        let mass = MassProperties::parse(&data, &fm_data);
        let mut loadout = LoadoutWeight::parse(&data, &fm_data, weapons_dir);

        let mut weapon_slots = parse_weapon_slots_from_data(&data, weapons_dir);
        if fm_content.is_some() {
            // 复用上面已解析好的 `fm_data`：不在这里重复解析 fmContent
            // （等价结果、白付一次解析 + 一次 JSON 中间对象的峰值内存）。
            weapon_slots.extend(parse_weapon_slots_from_data(&fm_data, weapons_dir));
        }
        loadout.weapon_slots = weapon_slots;

        let engine_loads = Self::parse_engine_loads(&fm_data);

        let rpm_min = propulsion.engines.iter().map(|e| e.rpm_min).fold(0.0, f64::max);
        let rpm_max = propulsion.engines.iter().map(|e| e.rpm_max).filter(|&v| v > 0.0).fold(f64::MAX, f64::min);
        let rpm_max_allowed = propulsion.engines.iter().map(|e| e.rpm_max_allowed).filter(|&v| v > 0.0).fold(f64::MAX, f64::min);

        let gear_warning_speed = ["Mass.GearDestructionIndSpeed", "GearDestructionIndSpeed"]
            .into_iter()
            .find_map(|k| data.get(k).or_else(|| fm_data.get(k)).and_then(|v| v.as_f64()))
            .unwrap_or(f64::MAX);

        Ok(FlightModel {
            name,
            fm_path: String::new(),
            data,
            fm_data,
            engine_type,
            is_jet,
            wing: Some(wing),
            fuselage: Some(fuselage),
            hor_stab: Some(hor_stab),
            reference_area,
            aerodynamics,
            propulsion,
            mass,
            loadout,
            rpm_min,
            rpm_max,
            rpm_max_allowed,
            gear_warning_speed,
            cached_flap_pairs,
            engine_loads,
        })
    }

    fn parse_engine_loads(fm_data: &HashMap<String, BlkxValue>) -> Vec<EngineLoad> {
        let mut loads = Vec::new();
        let parent_prefixes = [
            "",
            "EngineType0.Temperature.",
            "Engine0.Temperature.",
        ];
        for i in 0.. {
            let mut found = false;
            for parent in &parent_prefixes {
                let prefix = format!("{}Load{}.", parent, i);
                let water_key = format!("{}WaterTemperature", prefix);
                let oil_key = format!("{}OilTemperature", prefix);

                let water_limit = match fm_data.get(&water_key).and_then(|v| v.as_f64()) {
                    Some(v) if v > 0.0 => v,
                    _ => continue,
                };
                let oil_limit = match fm_data.get(&oil_key).and_then(|v| v.as_f64()) {
                    Some(v) if v > 0.0 => v,
                    _ => continue,
                };
                let work_key = format!("{}WorkTime", prefix);
                let recover_key = format!("{}RecoverTime", prefix);
                let work_time = fm_data.get(&work_key).and_then(|v| v.as_f64()).unwrap_or(0.0);
                let recover_time = fm_data.get(&recover_key).and_then(|v| v.as_f64()).unwrap_or(0.0);

                loads.push(EngineLoad {
                    water_limit,
                    oil_limit,
                    work_time,
                    recover_time,
                });
                found = true;
                break;
            }
            if !found {
                break;
            }
        }
        loads
    }

    pub fn engine_loads(&self) -> &[EngineLoad] {
        &self.engine_loads
    }

    fn compute_flap_destruction_pairs(fm_data: &HashMap<String, BlkxValue>) -> Vec<(f64, f64)> {
        let pairs = [
            {
                let pairs: Vec<_> = (0..5).filter_map(|i| {
                    let key = format!("FlapsDestructionIndSpeedP{i}");
                    let alt = format!("Mass.FlapsDestructionIndSpeedP{i}");
                    fm_data.get(&key).or_else(|| fm_data.get(&alt))
                        .and_then(|v| {
                            let n = v.as_f64_vec();
                            (n.len() >= 2 && n[1] != 0.0).then_some((n[0], n[1]))
                        })
                }).collect();
                (!pairs.is_empty()).then_some(pairs)
            },
            {
                let pairs: Vec<_> = ["Mass.", ""].iter().filter_map(|p| {
                    let nums = fm_data.get(&format!("{p}FlapsDestructionIndSpeedP"))?.as_f64_vec();
                    let r: Vec<_> = nums.chunks(4).filter(|c| c.len() >= 4).flat_map(|c| [(c[0], c[1]), (c[2], c[3])]).collect();
                    (!r.is_empty()).then_some(r)
                }).flatten().collect();
                (!pairs.is_empty()).then_some(pairs)
            },
            {
                let pairs: Vec<_> = ["Mass.", ""].iter().filter_map(|p| {
                    let speed = fm_data.get(&format!("{p}FlapsDestructionIndSpeed"))?.as_f64().filter(|&s| s > 0.0)?;
                    Some((1.0, speed))
                }).collect();
                (!pairs.is_empty()).then_some(pairs)
            },
        ].into_iter().find_map(|r| r).unwrap_or_default();

        Self::finalize_pairs(pairs)
    }

    fn finalize_pairs(mut pairs: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
        pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        if pairs.is_empty() {
            pairs.push((1.0, 350.0));
        }
        pairs.push((1.25, 0.0));
        pairs
    }

    /// 获取总功率 (hp) - 所有发动机之和
    pub fn total_power(&self) -> f64 {
        powers_at(self.engines(), 0.0, 0.0, 100.0)
    }

    pub fn engine_count(&self) -> usize {
        self.propulsion.engine_count
    }

    pub fn engines(&self) -> &[Engine] {
        &self.propulsion.engines
    }

    fn get_value(&self, key: &str) -> Option<&BlkxValue> {
        self.data.get(key).or_else(|| self.fm_data.get(key))
    }

    fn get_f64(&self, key: &str) -> Option<f64> {
        self.get_value(key).and_then(|v| v.as_f64())
    }

    pub fn empty_weight(&self) -> f64 {
        self.mass.empty_weight
    }

    pub fn max_fuel(&self) -> f64 {
        self.mass.max_fuel
    }

    pub fn max_external_fuel(&self) -> f64 {
        self.mass.max_external_fuel
    }

    pub fn oil_mass(&self) -> f64 {
        self.mass.oil_mass
    }

    pub fn pilot_mass(&self) -> f64 {
        self.mass.pilot_mass
    }

    pub fn flight_weight(&self, fuel_kg: f64) -> f64 {
        self.mass.flight_weight(fuel_kg)
            + self.cannon_ammo_weight()
            + self.countermeasure_weight()
            + self.external_weapons_weight()
    }

    pub fn takeoff_weight(&self, fuel_percent: f64) -> f64 {
        self.mass.takeoff_weight(fuel_percent)
    }

    pub fn takeoff_weight_with_ext_tanks(&self, fuel_percent: f64) -> f64 {
        self.mass.empty_weight + (self.mass.max_fuel + self.mass.max_external_fuel) * (fuel_percent / 100.0)
    }

    pub fn vne(&self) -> f64 {
        self.aerodynamics.wing_geometry.vne()
    }

    pub fn vne_mach(&self) -> f64 {
        self.aerodynamics.wing_geometry.vne_mach()
    }

    pub fn vne_limit(&self) -> VneLimit {
        let vne = self.vne();
        let mne = self.vne_mach();
        VneLimit {
            vne_kmh: vne,
            vne_ias_allowed: vne / 0.95,
            mne,
            mne_allowed: mne / 0.95,
        }
    }

    pub fn gear_warning_speed(&self) -> f64 {
        self.gear_warning_speed
    }

    pub fn crit_overload_vec(&self) -> Vec<f64> {
        CRIT_OVERLOAD_KEYS
            .iter()
            .find_map(|&key| {
                self.data
                    .get(key)
                    .or_else(|| self.fm_data.get(key))
                    .and_then(|v| {
                        let vec = v.as_f64_vec();
                        if vec.len() >= 2 { Some(vec) } else { None }
                    })
            })
            .unwrap_or_default()
    }

    /// 允许过载 `(负, 正)`，按传入重量（kg）算。`CritOverload` 单位是牛顿（机翼结构极限力）：
    /// `n = sign(F) × (2·|F| / (m·g) − 1)`，g = 9.81。
    pub fn allowed_load_factor(&self, weight_kg: f64) -> (f64, f64) {
        let crit_vec = self.crit_overload_vec();
        if crit_vec.len() < 2 || weight_kg <= 0.0 {
            return (0.0, 0.0);
        }
        let denom = weight_kg * Self::GRAVITY;
        let map = |f: f64| -> f64 {
            // 结构力不足以支撑 1g 时钳到 0，防止结果翻号
            let mag = (2.0 * f.abs() / denom - 1.0).max(0.0);
            mag * f.signum()
        };
        (map(crit_vec[0]), map(crit_vec[1]))
    }

    /// 兼容旧名，等价于 [`Self::allowed_load_factor`]。
    pub fn limit_load_factor_range(&self, weight_kg: f64) -> (f64, f64) {
        self.allowed_load_factor(weight_kg)
    }

    fn get_flap_destruction_pairs(&self) -> &[(f64, f64)] {
        &self.cached_flap_pairs
    }

    pub fn get_flap_destruction_speeds(&self) -> Vec<(f64, f64)> {
        self.cached_flap_pairs.to_vec()
    }

    fn calc_k(x0: f64, _y0: f64, x1: f64, y1: f64) -> f64 {
        if x1 == x0 {
            0.0
        } else {
            (y1 - _y0) / (x1 - x0)
        }
    }

    fn norm_flap_angle(t: f64) -> f64 {
        if t < 0.0 {
            0.0
        } else if t < 125.0 {
            t
        } else {
            125.0
        }
    }

    pub fn flap_allow_speed(&self, flap_percent: f64, is_downing_flap: bool) -> f64 {
        let pairs = self.get_flap_destruction_pairs();

        if pairs.len() < 2 || flap_percent <= 0.0 {
            return f64::MAX;
        }

        let flap_pct = flap_percent * 100.0;

        let mut i = 0;
        for j in 0..pairs.len() - 1 {
            if flap_pct < pairs[j].0 * 100.0 {
                break;
            }
            i = j;
        }

        if i == 0 && flap_pct < pairs[0].0 * 100.0 {
            if is_downing_flap && !pairs.is_empty() {
                return pairs[0].1;
            }
            return f64::MAX;
        }

        if (flap_pct - pairs[i].0 * 100.0).abs() < 0.001 {
            return pairs[i].1;
        }

        let x0 = pairs[i].0 * 100.0;
        let y0 = pairs[i].1;
        let x1 = pairs[i + 1].0 * 100.0;
        let y1 = pairs[i + 1].1;
        let k = Self::calc_k(x0, y0, x1, y1);

        y0 + (flap_pct - x0) * k
    }

    pub fn flap_allow_angle(&self, ias: f64, _is_downing_flap: bool) -> f64 {
        let pairs = self.get_flap_destruction_pairs();

        if pairs.len() < 2 || ias <= 0.0 {
            return 125.0;
        }

        let mut i = 0;
        for j in 0..pairs.len() - 1 {
            if ias > pairs[j].1 {
                break;
            }
            i = j;
        }

        let x0: f64;
        let y0: f64;
        let x1: f64;
        let y1: f64;

        if i == 0 {
            x0 = pairs[0].1;
            y0 = pairs[0].0 * 100.0;
            x1 = pairs[1].1;
            y1 = pairs[1].0 * 100.0;
        } else {
            if (ias - pairs[i - 1].1).abs() < 0.001 {
                return pairs[i - 1].0 * 100.0;
            }
            x0 = pairs[i - 1].1;
            y0 = pairs[i - 1].0 * 100.0;
            x1 = pairs[i].1;
            y1 = pairs[i].0 * 100.0;
        }

        let k = Self::calc_k(x0, y0, x1, y1);
        let t = y0 + (ias - x0) * k;

        Self::norm_flap_angle(t)
    }

    pub fn wingspan(&self) -> f64 {
        self.aerodynamics.wingspan
    }

    pub fn wing_area(&self) -> f64 {
        self.aerodynamics.wing_area
    }

    pub fn aspect_ratio(&self) -> f64 {
        self.aerodynamics.aspect_ratio
    }

    pub fn engine_power(&self) -> f64 {
        self.propulsion.engine_power
    }

    pub fn thrust_max(&self) -> f64 {
        self.propulsion.total_thrust_max
    }

    /// Get maximum WEP static thrust at sea level (kgf)
    pub fn wep_thrust_max(&self) -> f64 {
        if self.propulsion.engines.is_empty() {
            return 0.0;
        }
        thrusts_at(&self.propulsion.engines, 0.0, 0.0, 110.0)
    }

    /// Get maximum WEP static power at sea level (hp), for piston engines
    pub fn wep_power_max(&self) -> f64 {
        if self.propulsion.engines.is_empty() {
            return 0.0;
        }
        powers_at(&self.propulsion.engines, 0.0, 0.0, 110.0)
    }

    pub fn fuel_consumption_coefficient(&self, is_wep: bool) -> f64 {
        let throttle = if is_wep { 110.0 } else { 100.0 };
        let main_engines: Vec<&Engine> = self
            .engines()
            .iter()
            .filter(|e| e.engine_type() == EngineType::Jet || e.engine_type() == EngineType::Piston)
            .collect();
        let total_thrust: f64 = main_engines.iter().map(|e| e.thrust_max.max(0.0)).sum();
        if total_thrust <= 0.0 {
            return 0.0;
        }
        main_engines
            .iter()
            .map(|e| e.thrust_max.max(0.0) * e.fuel_consumption_full() * e.get_consumption_mult(throttle))
            .sum::<f64>()
            / total_thrust
    }

    pub fn compressor_stages(&self) -> Vec<CompressorStage> {
        let mut stages = Vec::new();

        let prefixes = [
            "Compressor.",
            "EngineType0.Compressor.",
            "Engine0.Compressor.",
        ];

        for prefix in &prefixes {
            let mut i = 0;
            loop {
                let alt_key = format!("{}Altitude{}", prefix, i);
                let pwr_key = format!("{}Power{}", prefix, i);
                let ceil_key = format!("{}Ceiling{}", prefix, i);
                let ceiling_key = format!("{}PowerAtCeiling{}", prefix, i);

                if let Some(altitude) = self.get_f64(&alt_key) {
                    let power = self.get_f64(&pwr_key).unwrap_or(0.0);
                    let ceiling = self.get_f64(&ceil_key).unwrap_or(altitude * 1.6);
                    let power_at_ceiling = self.get_f64(&ceiling_key).unwrap_or(0.0);
                    if power > 0.0 || power_at_ceiling > 0.0 {
                        stages.push(CompressorStage {
                            altitude,
                            power,
                            ceiling,
                            power_at_ceiling,
                        });
                    }
                    i += 1;
                } else {
                    break;
                }
            }

            if !stages.is_empty() {
                break;
            }
        }

        stages.sort_by(|a, b| a.altitude.partial_cmp(&b.altitude).unwrap());
        stages
    }

    /// Get fuel consumption rate (kg/s) at given velocity (km/h) and altitude (m), with optional WEP
    pub fn get_fuel_rate_at(&self, velocity_kmh: f64, altitude_m: f64, is_wep: bool) -> f64 {
        let throttle = if is_wep { 110.0 } else { 100.0 };
        fuel_rate_at(self.engines(), velocity_kmh, altitude_m, throttle)
    }

    /// Get nitro consumption (L/s)
    pub fn nitro_consumption(&self) -> f64 {
        total_nitro_consumption(self.engines())
    }

    pub fn has_nitro(&self) -> bool {
        self.nitro_consumption() > 0.0
    }

    pub fn max_nitro(&self) -> f64 {
        self.mass.nitro
    }

    pub fn cannon_ammo_weight(&self) -> f64 {
        let from_slots: f64 = self.loadout.weapon_slots
            .iter()
            .flat_map(|slot| slot.weapons.iter())
            .filter(|w| w.trigger != "countermeasures")
            .map(|w| w.bullet_mass * w.bullets as f64)
            .sum();
        if from_slots > 0.0 { from_slots } else { self.loadout.ammo_weight }
    }

    pub fn countermeasure_weight(&self) -> f64 {
        let from_slots: f64 = self.loadout.weapon_slots
            .iter()
            .flat_map(|slot| slot.weapons.iter())
            .filter(|w| w.trigger == "countermeasures")
            .map(|w| w.bullet_mass * w.bullets as f64)
            .sum();

        if from_slots > 0.0 {
            from_slots
        } else {
            self.loadout.countermeasures_weight
        }
    }

    fn abbrev_bullet_type(bt: &str) -> &str {
        match bt {
            "he_frag_i_t" => "hfit",
            "he_frag_i" => "hefi",
            "he_frag" => "hef",
            "frag_i_t" => "fit",
            "frag_i" => "fi",
            "he_i" => "hei",
            "he_i_t" => "heit",
            "ap_i" => "api",
            "ap_t" => "apt",
            "ap_i_t" => "apit",
            "ap_he" | "aphe" => "aphe",
            "ap" => "ap",
            "he" => "he",
            "practice" => "p",
            "ap_ball_M2" | "ap_ball" => "ap",
            "i_ball_M1" | "i_ball" => "i",
            _ => bt,
        }
    }

    pub fn print_weapon_info(&self) {
        for slot in &self.loadout.weapon_slots {
            let mut groups: Vec<(&Weapon, usize)> = Vec::new();
            for w in &slot.weapons {
                if !w.blk.contains("cannon") && !w.blk.contains("gun") { continue; }
                if let Some(g) = groups.iter_mut().find(|(gw, _)| gw.blk == w.blk) {
                    g.1 += 1;
                } else {
                    groups.push((w, 1));
                }
            }
            for (w, count) in &groups {
                let name = w.blk.trim_start_matches("gameData/Weapons/").trim_end_matches(".blk");
                let parts: Vec<String> = w.belts.iter().map(|belt| {
                    let types: Vec<String> = belt.bullets.iter().map(|b| Self::abbrev_bullet_type(&b.bullet_type).to_string()).collect();
                    format!("{}({})", belt.name, types.join("/"))
                }).collect();
                if *count > 1 {
                    println!("  {} x{} bullets={} mass={}kg belts=[{}]", name, count, w.bullets * *count, w.mass * *count as f64, parts.join(", "));
                } else {
                    println!("  {} bullets={} mass={}kg belts=[{}]", name, w.bullets, w.mass, parts.join(", "));
                }
                let mut seen: Vec<&str> = Vec::new();
                for b in w.belts.iter().flat_map(|belt| belt.bullets.iter()) {
                    if seen.contains(&b.bullet_type.as_str()) { continue; }
                    seen.push(&b.bullet_type);
                    let abbr = Self::abbrev_bullet_type(&b.bullet_type);
                    let hit = b.hit_power * b.hit_power_mult;
                    print!("    {}: mass={:.1}g", abbr, b.mass * 1000.0);
                    if b.armor_power > 0.0 { print!(" AP={}mm", b.armor_power); }
                    if hit > 0.0 { print!(" DMG={:.0}", hit); }
                    if b.explode_hit_power > 0.0 { print!(" HEhit={:.0}", b.explode_hit_power); }
                    if b.explode_armor_power > 0.0 { print!(" HEap={}mm", b.explode_armor_power); }
                    if b.explode_radius > 0.0 { print!(" HEr={:.1}m", b.explode_radius); }
                    if b.fire_chance > 0.0 { print!(" Fire={}", b.fire_chance); }
                    println!();
                }
            }
        }
    }

    pub fn external_weapons_weight(&self) -> f64 {
        0.0
    }

    pub fn has_wep(&self) -> bool {
        self.propulsion.has_wep
    }

    pub fn engine_type_string(&self) -> String {
        match self.engine_type {
            EngineType::Piston => "Piston".to_string(),
            EngineType::Jet => "Jet".to_string(),
            EngineType::Turboprop => "Turboprop".to_string(),
            EngineType::Rocket => "Rocket".to_string(),
            EngineType::Unknown => "Unknown".to_string(),
        }
    }

    pub fn summary(&self) -> String {
        let mut s = format!("=== {} ===\n", self.name);
        s.push_str(&format!("Engine Type: {}\n", self.engine_type_string()));
        s.push_str(&format!("Empty Weight: {:.0} kg\n", self.empty_weight()));
        s.push_str(&format!("Max Fuel: {:.0} kg\n", self.max_fuel()));
        s.push_str(&format!("VNE: {:.0} km/h\n", self.vne()));
        s.push_str(&format!("Wingspan: {:.1} m\n", self.wingspan()));
        s.push_str(&format!("Wing Area: {:.2} m²\n", self.wing_area()));
        s.push_str(&format!("Aspect Ratio: {:.2}\n", self.aspect_ratio()));

        if self.is_jet {
            s.push_str(&format!("Thrust Max: {:.0} kgs\n", self.thrust_max()));
        } else {
            s.push_str(&format!("Power: {:.0} hp\n", self.engine_power()));
            let stages = self.compressor_stages();
            if !stages.is_empty() {
                s.push_str(&format!("Compressor Stages: {}\n", stages.len()));
                for (i, stage) in stages.iter().enumerate() {
                    s.push_str(&format!(
                        "  Stage {}: Alt={:.0}m, Power={:.0}hp, Ceiling={:.0}hp\n",
                        i, stage.altitude, stage.power, stage.power_at_ceiling
                    ));
                }
            }
        }

        s
    }

    pub fn print_flight_limits(&self) {
        println!("=== Flight Limits ===");
        println!(
            "  VNE: {:.0} km/h ({:.2} Mach)",
            self.vne(),
            self.vne_mach()
        );
        println!(
            "  IAS Warning Line: {:.0} km/h",
            self.aerodynamics.get_ias_warning_line(0.0)
        );
        println!(
            "  Mach Warning Line: {:.3}",
            self.aerodynamics.get_mach_warning_line(0.0)
        );
        if self.gear_warning_speed < f64::MAX {
            println!("  Gear Destruction Speed: {:.0} km/h", self.gear_warning_speed);
        }
        if self.rpm_max_allowed < f64::MAX {
            println!("  RPM Min: {:.0}  Max: {:.0}  MaxAllowed: {:.0}", self.rpm_min, self.rpm_max, self.rpm_max_allowed);
        }

        if !self.is_jet {
            println!(
                "  Radiator Cd: {:.4}  Oil Cooler Cd: {:.4}",
                self.radiator_cd(),
                self.oil_radiator_cd()
            );
        }

        let flap_pairs = self.get_flap_destruction_pairs();
        if !flap_pairs.is_empty() {
            println!("  Flap Destruction Speeds:");
            for (flap_pct, speed) in flap_pairs {
                println!("    Flap {:.0}: {:.0} km/h", flap_pct * 100.0, speed);
            }
        } else {
            println!("  Flap Destruction Speeds: N/A");
        }

        if self.has_sweep_data() {
            println!("  Sweep wing lift coeff:");
            let (cl0, aoa0) = self.sweep_cl_max_no_flaps(0.0);
            let (cl50, aoa50) = self.sweep_cl_max_no_flaps(0.5);
            let (cl100, aoa100) = self.sweep_cl_max_no_flaps(1.0);
            println!("    0% sweep: Cl={:.2} @ {:.1}°", cl0, aoa0);
            println!("    50% sweep: Cl={:.2} @ {:.1}°", cl50, aoa50);
            println!("    100% sweep: Cl={:.2} @ {:.1}°", cl100, aoa100);
        }
    }
}

#[derive(Debug, Clone)]
pub struct VneLimit {
    pub vne_kmh: f64,
    pub vne_ias_allowed: f64,
    pub mne: f64,
    pub mne_allowed: f64,
}

// ============================================================
// 能量机动(EM)计算相关函数
// ============================================================

impl FlightModel {
    /// 重力加速度 (m/s²)：全项目统一 9.81（Dagor atmosphere.cpp:19），单一来源 aero::GRAVITY
    pub const GRAVITY: f64 = crate::aero::GRAVITY;

    /// 计算指定高度下的空气密度 (kg/m³)
    /// 使用完整的标准大气模型
    pub fn air_density(&self, altitude_m: f64) -> f64 {
        crate::aero::calculate_density(altitude_m)
    }

    /// 计算指定高度下的音速 (m/s)
    pub fn speed_of_sound(&self, altitude_m: f64) -> f64 {
        crate::aero::calculate_sound_speed(altitude_m)
    }

    /// 计算指示空速 (IAS) km/h
    /// IAS = TAS * sqrt(rho/rho0), rho0 = 1.225 kg/m³
    pub fn ias_kmh(&self, altitude_m: f64, tas_kmh: f64) -> f64 {
        let rho = self.air_density(altitude_m);
        tas_kmh * (rho / 1.225).sqrt()
    }

    /// 将km/h转换为m/s
    pub fn kmh_to_ms(kmh: f64) -> f64 {
        kmh / 3.6
    }

    /// 将m/s转换为km/h
    pub fn ms_to_kmh(ms: f64) -> f64 {
        ms * 3.6
    }

    pub fn cd_min(&self) -> f64 {
        self.aerodynamics.cd_min
    }

    pub fn cl_max_no_flaps(&self) -> f64 {
        self.aerodynamics.cl_max_no_flaps
    }

    pub fn aoa_cl_max_no_flaps(&self) -> f64 {
        self.aerodynamics.aoa_cl_max_no_flaps
    }

    pub fn cl_max_full_flaps(&self) -> f64 {
        self.aerodynamics.cl_max_full_flaps
    }

    pub fn aoa_cl_max_full_flaps(&self) -> f64 {
        self.aerodynamics.aoa_cl_max_full_flaps
    }

    pub fn has_sweep_data(&self) -> bool {
        self.aerodynamics.has_sweep_data()
    }

    pub fn sweep_cl_max_no_flaps(&self, sweep: f64) -> (f64, f64) {
        self.aerodynamics.get_cl_aoa_max(0.0, sweep)
    }

    pub fn oswalds_efficiency(&self) -> f64 {
        self.aerodynamics.oswalds_efficiency
    }

    pub fn fuselage_cd_min(&self) -> f64 {
        [
            "FuselagePlane.Polar.CdMin",
            "FuselagePlane.CdMin",
            "Aerodynamics.FuselagePlane.Polar.CdMin",
            "Aerodynamics.Fuselage.CdMin",
        ]
        .into_iter()
        .find_map(|k| self.get_f64(k))
        .unwrap_or(0.0)
    }

    pub fn radiator_cd(&self) -> f64 {
        self.aerodynamics.radiator_cd
    }

    pub fn oil_radiator_cd(&self) -> f64 {
        self.aerodynamics.oil_radiator_cd
    }

    pub fn airbrake_cd(&self) -> f64 {
        self.aerodynamics.airbrake_cd
    }

    /// 获取阻力面积 CdS (Drag Area)
    /// CdS = AWing * CdMin_wing + AFuselage * CdMin_fuselage
    /// 这是所有部件零升阻力的总和
    pub fn drag_area(&self) -> f64 {
        let wing_area = self.wing_area();
        let wing_cd_min = self.cd_min();
        let fuselage_cd_min = self.fuselage_cd_min();

        let fuselage_area = [
            "Aerodynamics.FuselagePlane.Areas.Main",
            "FuselagePlane.Areas.Main",
            "WingPlane.Areas.Fuselage",
            "Areas.Fuselage",
        ]
        .into_iter()
        .find_map(|k| self.get_f64(k))
        .unwrap_or(wing_area * 0.3);

        wing_area * wing_cd_min + fuselage_area * fuselage_cd_min
    }

    /// 获取诱导阻力系数因子 k = 1/(π × e × AR)
    pub fn induced_drag_factor(&self) -> f64 {
        let oswald = self.oswalds_efficiency();
        let ar = self.aspect_ratio();

        if ar <= 0.0 || oswald <= 0.0 {
            return 0.0;
        }

        1.0 / (std::f64::consts::PI * oswald * ar)
    }

    /// 计算给定升力系数下的阻力系数
    /// 使用极曲线公式: Cd = Cd_min + Cl² / (π × e × AR)
    pub fn drag_coefficient(&self, cl: f64) -> f64 {
        let cd_min = self.cd_min();
        let oswald = self.oswalds_efficiency();
        let ar = self.aspect_ratio();

        if ar <= 0.0 {
            return cd_min;
        }

        let k = 1.0 / (std::f64::consts::PI * oswald * ar);
        cd_min + k * cl * cl
    }

    /// 计算阻力 (N)
    /// D = 0.5 × ρ × V² × S × Cd
    pub fn drag(&self, altitude_m: f64, velocity_ms: f64, lift_coeff: f64) -> f64 {
        let rho = self.air_density(altitude_m);
        let s = self.wing_area();
        let cd = self.drag_coefficient(lift_coeff);

        0.5 * rho * velocity_ms * velocity_ms * s * cd
    }

    /// 水平飞行阻力 (N)：假设 L = W，自动求所需升力系数。阻力项见 [`Self::drag_with_airbrake`]。
    /// 减速板要用那个带 `airbrake` 参数的重载，不是这个。
    pub fn drag_level_flight(&self, altitude_m: f64, velocity_ms: f64, weight_n: f64) -> f64 {
        self.drag_with_airbrake(altitude_m, velocity_ms, weight_n, false)
    }

    /// 水平飞行阻力 (N)，可带减速板。阻力项 = 寄生阻力(CdS：机翼 + 机身，非机翼部分带 CX0 马赫修正)
    /// + 诱导阻力(k·Cl²) + 散热器/油冷器阻力 + 减速板（`airbrake`）+ 波阻（由 CX0 马赫曲线覆盖）。
    pub fn drag_with_airbrake(
        &self,
        altitude_m: f64,
        velocity_ms: f64,
        weight_n: f64,
        airbrake_deployed: bool,
    ) -> f64 {
        // 使用气动模块计算
        if let (Some(wing), Some(_), Some(_)) = (&self.wing, &self.fuselage, &self.hor_stab) {
            if self.reference_area > 0.0 && velocity_ms > 0.0 {
                let rho = self.air_density(altitude_m);
                let q = 0.5 * rho * velocity_ms * velocity_ms;

                // 根据所需升力计算 cl
                let cl_needed = weight_n / (q * self.reference_area);

                // 使用极曲线计算给定 cl 下的 cd
                // 使用极曲线计算阻力，包含 Mach 修正
                let a = self.speed_of_sound(altitude_m);
                let mach = if a > 0.0 { velocity_ms / a } else { 0.0 };

                // 从极曲线获取 cd（包含机翼的零升阻力和诱导阻力）
                // get_drag_coefficient 按 DagorEngine 方式以攻角为输入，内部用未裁剪线性 Cl 计算诱导阻力
                let aoa_needed = if wing.polar.line_cl_coeff > 0.0 {
                    (cl_needed - wing.polar.cl0) / wing.polar.line_cl_coeff
                } else {
                    cl_needed
                };
                let cd_wing =
                    get_drag_coefficient(&wing.polar, aoa_needed, mach, wing.area, wing.span);

                // 零升阻力拆分：机翼部分已含在 cd_wing（CX0 马赫波阻修正），
                // 其余部件（机身/尾翼）静态 CdS 按同一 CX0 马赫因子增长
                let wing_cd_min_area = wing.polar.cd_min * wing.area;
                let cd0_rest = ((self.drag_area() - wing_cd_min_area).max(0.0)
                    / self.reference_area)
                    * cx0_mach_mult(&wing.polar, mach);

                // 散热器、油冷器阻力
                let cd_extra = self.radiator_cd() + self.oil_radiator_cd();
                // 减速板阻力
                let cd_brake = if airbrake_deployed { self.airbrake_cd() } else { 0.0 };

                // 波阻已由 get_drag_coefficient 的 CX0 马赫曲线覆盖 —— 不再叠加
                // 经验波阻（0.08(M-0.85)²+0.15(M-1)² 会与之重复计、高估跨声速阻力）
                return q * self.reference_area * (cd0_rest + cd_wing + cd_extra + cd_brake);
            }
        }

        // 回退到另一种算法
        let rho = self.air_density(altitude_m);
        let s = self.wing_area();

        if s <= 0.0 || velocity_ms <= 0.0 {
            return 0.0;
        }

        let q = 0.5 * rho * velocity_ms * velocity_ms;
        let cl = weight_n / (q * s);

        // 寄生阻力 (阻力面积 CdS)
        let mut cd = self.drag_area() / s;

        // 诱导阻力
        let ind_factor = self.induced_drag_factor();
        cd += ind_factor * cl * cl;

        // 散热器阻力
        cd += self.radiator_cd();

        // 油冷器阻力
        cd += self.oil_radiator_cd();

        // 减速板阻力 (仅在展开时)
        if airbrake_deployed {
            cd += self.airbrake_cd();
        }

        // 波阻 (马赫 > 0.85 时)
        let a = self.speed_of_sound(altitude_m);
        if a > 0.0 {
            let mach = velocity_ms / a;
            if mach > 0.85 {
                let wave_cd = 0.1 * (mach - 0.85).powi(2);
                cd += wave_cd;
            }
        }

        q * s * cd
    }

    /// 获取阻力分解 (用于调试/显示)
    pub fn drag_breakdown(
        &self,
        altitude_m: f64,
        velocity_ms: f64,
        weight_n: f64,
    ) -> DragBreakdown {
        self.drag_breakdown_with_airbrake(altitude_m, velocity_ms, weight_n, false)
    }

    /// 获取阻力分解 (带减速板选项)
    pub fn drag_breakdown_with_airbrake(
        &self,
        altitude_m: f64,
        velocity_ms: f64,
        weight_n: f64,
        airbrake_deployed: bool,
    ) -> DragBreakdown {
        let rho = self.air_density(altitude_m);
        let s = self.wing_area();

        let q = 0.5 * rho * velocity_ms * velocity_ms;
        let cl = if s > 0.0 && q > 0.0 {
            weight_n / (q * s)
        } else {
            0.0
        };

        let cds = self.drag_area();
        let ind_factor = self.induced_drag_factor();

        DragBreakdown {
            velocity_ms,
            altitude_m,
            weight_n,
            dynamic_pressure: q,
            lift_coeff: cl,

            // 寄生阻力分量
            drag_area: cds,
            parasitic_drag: q * cds,

            // 诱导阻力
            induced_drag_factor: ind_factor,
            induced_drag: q * s * ind_factor * cl * cl,

            // 附加阻力
            radiator_drag: q * s * self.radiator_cd(),
            oil_radiator_drag: q * s * self.oil_radiator_cd(),
            airbrake_drag: if airbrake_deployed {
                q * s * self.airbrake_cd()
            } else {
                0.0
            },

            // 波阻
            mach: if self.speed_of_sound(altitude_m) > 0.0 {
                velocity_ms / self.speed_of_sound(altitude_m)
            } else {
                0.0
            },
            wave_drag: if self.speed_of_sound(altitude_m) > 0.0
                && velocity_ms / self.speed_of_sound(altitude_m) > 0.85
            {
                let mach = velocity_ms / self.speed_of_sound(altitude_m);
                q * s * 0.1 * (mach - 0.85).powi(2)
            } else {
                0.0
            },

            // 总阻力
            total_drag: self.drag_with_airbrake(
                altitude_m,
                velocity_ms,
                weight_n,
                airbrake_deployed,
            ),
        }
    }

    /// 计算需要给定升力时的阻力 (N)
    /// 与 drag_with_airbrake **同一阻力口径**（Dagor 极曲线 + CX0 马赫波阻修正，
    /// 波阻不重复计）——Ps/EM 计算走这里。
    pub fn drag_for_lift(&self, altitude_m: f64, velocity_ms: f64, required_lift_n: f64) -> f64 {
        self.drag_with_airbrake(altitude_m, velocity_ms, required_lift_n, false)
    }

    /// 计算喷气机推力 (N)
    /// 速度输入为m/s，返回推力(N)
    pub fn thrust(&self, altitude_m: f64, velocity_ms: f64) -> f64 {
        let velocity_kmh = Self::ms_to_kmh(velocity_ms);
        thrusts_at(self.engines(), velocity_kmh, altitude_m, 100.0) * Self::GRAVITY
    }

    /// 计算喷气机加力推力 (N)
    pub fn afterburner_thrust(&self, altitude_m: f64, velocity_ms: f64) -> f64 {
        let velocity_kmh = Self::ms_to_kmh(velocity_ms);
        thrusts_at(self.engines(), velocity_kmh, altitude_m, 110.0) * Self::GRAVITY
    }

    /// 计算最大平飞速度 (m/s)
    /// 找到推力等于阻力的速度点
    /// use_wep: true = 加力推力, false = 军推
    pub fn max_level_flight_speed(&self, altitude_m: f64, weight_n: f64, use_wep: bool) -> f64 {
        if !self.is_jet && self.engine_type != EngineType::Rocket {
            return 0.0;
        }

        let vne_kmh = self.vne();
        if vne_kmh <= 0.0 {
            return 0.0;
        }

        // 搜索速度范围: 从 50 m/s 到 6000 km/h (允许超过 VNE)
        let mut best_speed = 0.0;
        let search_limit_ms = 6000.0 / 3.6; // 6000 km/h

        for v_ms in 50..=(search_limit_ms as i32) {
            let v = v_ms as f64;

            let thrust = if use_wep {
                self.afterburner_thrust(altitude_m, v)
            } else {
                self.thrust(altitude_m, v)
            };

            let drag = self.drag_level_flight(altitude_m, v, weight_n);
            let excess = thrust - drag;

            // 找到推力刚好能克服阻力的最大速度
            if excess >= 0.0 && v > best_speed {
                best_speed = v;
            }
        }

        best_speed
    }

    /// 计算剩余功率 (m/s)
    /// P_s = (T - D) × V / W
    pub fn specific_excess_power(
        &self,
        altitude_m: f64,
        velocity_ms: f64,
        weight_n: f64,
        use_ab: bool,
    ) -> f64 {
        let thrust = if use_ab {
            self.afterburner_thrust(altitude_m, velocity_ms)
        } else {
            self.thrust(altitude_m, velocity_ms)
        };

        // 需要升力等于重量才能水平飞行
        let lift_required = weight_n;
        let drag = self.drag_for_lift(altitude_m, velocity_ms, lift_required);

        // 剩余功率 = (推力 - 阻力) × 速度 / 重量
        (thrust - drag) * velocity_ms / weight_n
    }

    /// 计算稳定盘旋可用过载（气动上限，CLmax 带马赫修正）
    pub fn available_load_factor(&self, altitude_m: f64, velocity_ms: f64, weight_n: f64) -> f64 {
        let rho = self.air_density(altitude_m);
        let s = self.wing_area();

        // CLmax(M)：按 Dagor CY_MAX 马赫曲线修正（cl_max_mach_corrected 单一实现）
        let a = self.speed_of_sound(altitude_m);
        let mach = if a > 0.0 { velocity_ms / a } else { 0.0 };
        let cl_max_base = self.cl_max_no_flaps();
        let cl_max = self
            .wing
            .as_ref()
            .map(|w| cl_max_mach_corrected(&w.polar, cl_max_base, mach))
            .unwrap_or(cl_max_base);

        // 最大升力 L_max = 0.5 × ρ × V² × S × Cl_max
        let v2 = velocity_ms * velocity_ms;
        let l_max = 0.5 * rho * v2 * s * cl_max;

        // 可用过载 n = L_max / W
        l_max / weight_n
    }

    /// 计算稳定盘旋转弯率 (rad/s)
    /// ω = g × sqrt(n² - 1) / V
    pub fn turn_rate(&self, altitude_m: f64, velocity_ms: f64, weight_n: f64) -> f64 {
        let n = self.available_load_factor(altitude_m, velocity_ms, weight_n);

        if n <= 1.0 || velocity_ms <= 0.0 {
            return 0.0;
        }

        Self::GRAVITY * (n * n - 1.0).sqrt() / velocity_ms
    }

    /// 计算稳定盘旋转弯率 (度/秒)
    pub fn turn_rate_deg_per_sec(&self, altitude_m: f64, velocity_ms: f64, weight_n: f64) -> f64 {
        self.turn_rate(altitude_m, velocity_ms, weight_n) * 180.0 / std::f64::consts::PI
    }

}
