use crate::constants::{MAX_ENGINES, MAX_FUEL_TANKS};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use wp8f_disp::FixedBytes;

pub mod flat_json;

// json_helpers module kept for backward compatibility
#[derive(Error, Debug)]
pub enum ParseError {
    #[error("Invalid JSON: {0}")]
    InvalidJson(String),
    #[error("Missing field: {0}")]
    MissingField(String),
    #[error("Type mismatch: {0}")]
    TypeMismatch(String),
    #[error("Empty input")]
    EmptyInput,
    #[error("Invalid data: {0}")]
    InvalidData(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct EngineState {
    #[serde(default)]
    pub throttle: f64,
    #[serde(default)]
    pub power: f64,
    #[serde(default)]
    pub rpm: f64,
    #[serde(default)]
    pub manifold: f64,
    #[serde(default)]
    pub thrust: f64,
    #[serde(default)]
    pub efficiency: f64,
    #[serde(default)]
    pub pitch: f64,
    #[serde(default)]
    pub oil_temp: f64,
    #[serde(default)]
    pub mixture: f64,
    #[serde(default)]
    pub radiator: f64,
    #[serde(default)]
    pub magneto: i32,
}

impl EngineState {
    pub fn copy_to_display(
        &self,
        indic: &Indicators,
        throttle: &mut [f64],
        power: &mut [f64],
        rpm: &mut [f64],
        thrust: &mut [f64],
        temp_water: &mut [f64],
        temp_oil: &mut [f64],
        index: usize,
    ) {
        if index < throttle.len() {
            throttle[index] = self.throttle;
            power[index] = self.power;
            rpm[index] = self.rpm;
            thrust[index] = self.thrust;
            temp_water[index] = if indic.temperature > 0.0 {
                indic.temperature
            } else {
                0.0
            };
            temp_oil[index] = self.oil_temp;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(non_snake_case)]
pub struct FlightState {
    #[serde(default = "default_valid")]
    pub valid: bool,
    #[serde(default)]
    pub aileron: f64,
    #[serde(default)]
    pub elevator: f64,
    #[serde(default)]
    pub rudder: f64,
    #[serde(default)]
    pub flaps: f64,
    #[serde(default)]
    pub gear: f64,
    #[serde(default)]
    pub airbrake: f64,
    #[serde(rename = "H, m", default)]
    pub altitude: f64,
    #[serde(rename = "TAS, km/h", default)]
    pub tas: f64,
    #[serde(rename = "IAS, km/h", default)]
    pub ias: f64,
    #[serde(default)]
    pub M: f64,
    #[serde(rename = "AoA, deg", default)]
    pub aoa: f64,
    #[serde(rename = "AoS, deg", default)]
    pub aos: f64,
    #[serde(default)]
    pub Ny: f64,
    #[serde(rename = "Vy, m/s", default)]
    pub vy: f64,
    #[serde(rename = "Wx, deg/s", default)]
    pub wx: f64,
    #[serde(rename = "Mfuel, kg", default)]
    pub fuel: f64,
    #[serde(rename = "Mfuel0, kg", default)]
    pub fuel0: f64,
    #[serde(rename = "Mfuel 1, kg", default)]
    pub fuel1: f64,
    #[serde(default)]
    pub engines: [EngineState; MAX_ENGINES],
    #[serde(default)]
    pub engine_count: usize,
}

fn default_valid() -> bool {
    false
}

impl Default for FlightState {
    fn default() -> Self {
        Self {
            valid: false,
            aileron: 0.0,
            elevator: 0.0,
            rudder: 0.0,
            flaps: 0.0,
            gear: 0.0,
            airbrake: 0.0,
            altitude: 0.0,
            tas: 0.0,
            ias: 0.0,
            M: 0.0,
            aoa: 0.0,
            aos: 0.0,
            Ny: 1.0,
            vy: 0.0,
            wx: 0.0,
            fuel: 0.0,
            fuel0: 0.0,
            fuel1: 0.0,
            engines: [EngineState::default(); MAX_ENGINES],
            engine_count: 0,
        }
    }
}

#[allow(non_snake_case)]
pub fn parse_state(state: &mut FlightState, json_str: &str) -> Result<(), ParseError> {
    if json_str.trim().is_empty() {
        return Err(ParseError::EmptyInput);
    }
    flat_json::parse_state_json(json_str, state);
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Indicators {
    #[serde(default = "default_valid")]
    pub valid: bool,
    #[serde(default)]
    pub has_attitude: bool,
    #[serde(default)]
    pub army: FixedBytes<8>,
    #[serde(default)]
    pub aircraft_type: FixedBytes<32>,
    #[serde(default)]
    pub speed: f64,
    #[serde(default)]
    pub vario: f64,
    #[serde(default)]
    pub altitude: f64,
    #[serde(default)]
    pub compass: f64,
    #[serde(default)]
    pub roll: f64,
    #[serde(default)]
    pub pitch: f64,
    #[serde(default)]
    pub rpm: f64,
    #[serde(default)]
    pub mach: f64,
    #[serde(default)]
    pub g_meter: f64,
    #[serde(default)]
    pub aoa: f64,
    #[serde(default)]
    pub throttle: f64,
    #[serde(default)]
    pub gear: f64,
    #[serde(default)]
    pub airbrake: f64,
    #[serde(default)]
    pub flaps: f64,
    #[serde(default)]
    pub radio_altitude: f64,
    #[serde(default)]
    pub altitude_10k: f64,
    #[serde(default)]
    pub trimmer: f64,
    #[serde(default)]
    pub manifold_pressure: f64,
    #[serde(default)]
    pub fuel: f64,
    #[serde(default)]
    pub fuels: [f64; MAX_FUEL_TANKS],
    #[serde(default)]
    pub fuel_tank_count: usize,

    #[serde(default)]
    pub compass1: Option<f64>,
    #[serde(default)]
    pub temperature: f64,
    #[serde(default)]
    pub oil_temp: f64,
    pub wing_sweep_indicator: f64,
}

impl Default for Indicators {
    fn default() -> Self {
        Self {
            valid: false,
            army: FixedBytes::new(),
            aircraft_type: FixedBytes::new(),
            speed: 0.0,
            vario: 0.0,
            altitude: 0.0,
            compass: 0.0,
            roll: 0.0,
            pitch: 0.0,
            rpm: 0.0,
            mach: 0.0,
            g_meter: 1.0,
            aoa: 0.0,
            throttle: 0.0,
            gear: 0.0,
            airbrake: 0.0,
            flaps: 0.0,
            radio_altitude: 0.0,
            altitude_10k: 0.0,
            trimmer: 0.0,
            manifold_pressure: 0.0,
            fuel: 0.0,
            fuels: [0.0; MAX_FUEL_TANKS],
            fuel_tank_count: 0,
            compass1: None,
            temperature: 0.0,
            oil_temp: 0.0,
            wing_sweep_indicator: 0.0,
            has_attitude: false,
        }
    }
}

pub fn parse_indicators(indic: &mut Indicators, json_str: &str) -> Result<(), ParseError> {
    if json_str.trim().is_empty() {
        return Err(ParseError::EmptyInput);
    }
    flat_json::parse_indicators_json(json_str, indic);
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MapObject {
    pub id: i64,
    pub name: String,
    pub obj_type: String,
    pub lat: f64,
    pub lon: f64,
    pub alt: f64,
    pub lat1: Option<f64>,
    pub lon1: Option<f64>,
    pub lat2: Option<f64>,
    pub lon2: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MapInfo {
    pub valid: bool,
    pub grid_size: [f64; 2],
    pub grid_steps: [f64; 2],
    pub grid_zero: [f64; 2],
    pub hud_type: i32,
    pub map_generation: i32,
    pub map_max: [f64; 2],
    pub map_min: [f64; 2],
    pub maxsize: [f64; 2],
}

pub fn parse_map_info(info: &mut MapInfo, json_str: &str) -> Result<(), ParseError> {
    if json_str.trim().is_empty() {
        return Err(ParseError::EmptyInput);
    }
    flat_json::parse_map_info_json(json_str, info);
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MapObjItem {
    pub obj_type: FixedBytes<24>,
    pub color: FixedBytes<8>,
    pub blink: i32,
    pub icon: FixedBytes<16>,
    pub icon_bg: FixedBytes<16>,
    pub sx: Option<f64>,
    pub sy: Option<f64>,
    pub ex: Option<f64>,
    pub ey: Option<f64>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub dx: Option<f64>,
    pub dy: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MapObjData {
    pub valid: bool,
    pub objects: Vec<MapObjItem>,
}

pub fn parse_map_obj_data(data: &mut MapObjData, json_str: &str) -> Result<(), ParseError> {
    if json_str.trim().is_empty() {
        return Err(ParseError::EmptyInput);
    }
    flat_json::parse_map_obj_data_json(json_str, data);
    Ok(())
}


