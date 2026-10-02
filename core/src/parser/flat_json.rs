use crate::constants::{MAX_ENGINES, MAX_FUEL_TANKS};

pub struct FlatJsonIter<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> FlatJsonIter<'a> {
    pub fn new(json: &'a str) -> Self {
        let input = json.as_bytes();
        let pos = input.iter().position(|&b| b == b'{').map(|p| p + 1).unwrap_or(input.len());
        Self { input, pos }
    }

    pub fn next_key(&mut self, skip: usize) -> Option<&'a str> {
        while self.input[self.pos] != b'"' && self.input[self.pos] != b'}' {
            self.pos += 1;
        }
        if self.input[self.pos] == b'}' { return None; }

        let ks = self.pos + 1;
        self.pos = ks;
        while self.input[self.pos] != b'"' {
            self.pos += 1;
        }
        let key = unsafe { std::str::from_utf8_unchecked(&self.input[ks..self.pos]) };

        self.pos += skip;
        Some(key)
    }

    pub fn next_value(&mut self) -> &'a str {
        let vs = self.pos;
        while self.input[self.pos] != b',' && self.input[self.pos] != b'}' && self.input[self.pos] != b'\n' {
            self.pos += 1;
        }
        unsafe { std::str::from_utf8_unchecked(&self.input[vs..self.pos]) }
    }

    pub fn next_values(&mut self) -> &'a str {
        if self.input[self.pos] != b'[' {
            return self.next_value();
        }
        let vs = self.pos;
        while self.input[self.pos] != b']' {
            self.pos += 1;
        }
        self.pos += 1;
        unsafe { std::str::from_utf8_unchecked(&self.input[vs..self.pos]) }
    }
}

pub fn parse_f64(val: &str) -> Option<f64> {
    val.parse().ok()
}

pub fn parse_f64_or(val: &str, default: f64) -> f64 {
    parse_f64(val).unwrap_or(default)
}

pub fn parse_bool(val: &str) -> bool {
    val.as_bytes()[0] == b't'
}

fn parse_str<'a>(val: &'a str) -> &'a str {
    &val[1..val.len() - 1]
}

fn parse_map_obj_item(iter: &mut FlatJsonIter, item: &mut crate::parser::MapObjItem) {
    while let Some(key) = iter.next_key(2) {
        match key {
            "type" => item.obj_type.assign(parse_str(iter.next_value()).as_bytes()),
            "color" => item.color.assign(parse_str(iter.next_value()).as_bytes()),
            "blink" => item.blink = iter.next_value().parse().unwrap_or(0),
            "icon" => item.icon.assign(parse_str(iter.next_value()).as_bytes()),
            "icon_bg" => item.icon_bg.assign(parse_str(iter.next_value()).as_bytes()),
            "sx" => item.sx = parse_f64(iter.next_value()),
            "sy" => item.sy = parse_f64(iter.next_value()),
            "ex" => item.ex = parse_f64(iter.next_value()),
            "ey" => item.ey = parse_f64(iter.next_value()),
            "x" => item.x = parse_f64(iter.next_value()),
            "y" => item.y = parse_f64(iter.next_value()),
            "dx" => item.dx = parse_f64(iter.next_value()),
            "dy" => item.dy = parse_f64(iter.next_value()),
            _ => { iter.next_values(); }
        }
    }
}

pub fn parse_map_obj_data_json(json: &str, data: &mut crate::MapObjData) -> bool {
    let bytes = json.as_bytes();
    let mut pos = 0;

    data.objects.clear();

    while pos < bytes.len() {
        if bytes[pos] == b'{' {
            let obj_bytes = &bytes[pos..];
            let obj_str = unsafe { std::str::from_utf8_unchecked(obj_bytes) };
            let mut iter = FlatJsonIter::new(obj_str);
            let mut item = crate::parser::MapObjItem::default();
            parse_map_obj_item(&mut iter, &mut item);
            data.objects.push(item);
            pos += iter.pos;
        } else {
            pos += 1;
        }
    }

    data.valid = true;
    true
}

fn extract_index(key: &str, prefix: &str, suffix: &str) -> Option<usize> {
    key[prefix.len()..key.len() - suffix.len()].parse().ok()
}

pub fn parse_f64_2(val: &str) -> [f64; 2] {
    let b = val.as_bytes();
    let mut p = 2;
    let a_start = p;
    while b[p] != b',' { p += 1; }
    let a: f64 = unsafe { std::str::from_utf8_unchecked(&b[a_start..p]) }.parse().unwrap_or(0.0);
    p += 2;
    let b_start = p;
    while b[p] != b' ' { p += 1; }
    let b_val: f64 = unsafe { std::str::from_utf8_unchecked(&b[b_start..p]) }.parse().unwrap_or(0.0);
    [a, b_val]
}

pub fn parse_map_info_json(json: &str, info: &mut crate::MapInfo) -> bool {
    let mut found_valid = false;
    let mut iter = FlatJsonIter::new(json);

    while let Some(key) = iter.next_key(4) {
        match key {
            "valid" => { found_valid = parse_bool(iter.next_value()); info.valid = found_valid; }
            "grid_size" => info.grid_size = parse_f64_2(iter.next_values()),
            "grid_steps" => info.grid_steps = parse_f64_2(iter.next_values()),
            "grid_zero" => info.grid_zero = parse_f64_2(iter.next_values()),
            "hud_type" => info.hud_type = iter.next_value().parse().unwrap_or(0),
            "map_generation" => info.map_generation = iter.next_value().parse().unwrap_or(0),
            "map_max" => info.map_max = parse_f64_2(iter.next_values()),
            "map_min" => info.map_min = parse_f64_2(iter.next_values()),
            _ => { iter.next_values(); }
        }
    }
    info.maxsize = [info.map_max[0] - info.map_min[0], info.map_max[1] - info.map_min[1]];
    found_valid
}

pub fn parse_state_json(json: &str, state: &mut crate::FlightState) -> bool {
    if json.trim().is_empty() { state.valid = false; return false; }

    let mut found_valid = false;
    let mut max_engine_idx = 0usize;
    let mut engine_seen = [false; MAX_ENGINES];
    let mut iter = FlatJsonIter::new(json);

    while let Some(key) = iter.next_key(3) {
        let val = iter.next_value();
        match key {
            "valid" => {
                found_valid = parse_bool(val);
                if !found_valid { state.valid = false; return true; }
                state.valid = true;
            }
            "aileron, %" => state.aileron = parse_f64_or(val, 0.0),
            "elevator, %" => state.elevator = parse_f64_or(val, 0.0),
            "rudder, %" => state.rudder = parse_f64_or(val, 0.0),
            "flaps, %" => state.flaps = parse_f64_or(val, 0.0),
            "gear, %" => state.gear = parse_f64_or(val, 0.0),
            "airbrake, %" => state.airbrake = parse_f64_or(val, 0.0),
            "H, m" => state.altitude = parse_f64_or(val, 0.0),
            "TAS, km/h" => state.tas = parse_f64_or(val, 0.0),
            "IAS, km/h" => state.ias = parse_f64_or(val, 0.0),
            "M" => state.M = parse_f64_or(val, 0.0),
            "AoA, deg" => state.aoa = parse_f64_or(val, 0.0),
            "AoS, deg" => state.aos = parse_f64_or(val, 0.0),
            "Ny" => state.Ny = parse_f64_or(val, 1.0),
            "Vy, m/s" => state.vy = parse_f64_or(val, 0.0),
            "Wx, deg/s" => state.wx = parse_f64_or(val, 0.0),
            "Mfuel, kg" => state.fuel = parse_f64_or(val, 0.0),
            "Mfuel0, kg" => state.fuel0 = parse_f64_or(val, 0.0),
            "Mfuel 1, kg" => state.fuel1 = parse_f64_or(val, 0.0),

            _ if key.starts_with("throttle ") => {
                if let Some(idx) = extract_index(key, "throttle ", ", %") {
                    let s = idx - 1;
                    if s < MAX_ENGINES {
                        if !engine_seen[s] { state.engines[s] = Default::default(); engine_seen[s] = true; }
                        state.engines[s].throttle = parse_f64_or(val, 0.0);
                        if idx > max_engine_idx { max_engine_idx = idx; }
                    }
                }
            }
            _ if key.starts_with("power ") => {
                if let Some(idx) = extract_index(key, "power ", ", hp") {
                    let s = idx - 1;
                    if s < MAX_ENGINES {
                        if !engine_seen[s] { state.engines[s] = Default::default(); engine_seen[s] = true; }
                        state.engines[s].power = parse_f64_or(val, 0.0);
                        if idx > max_engine_idx { max_engine_idx = idx; }
                    }
                }
            }
            _ if key.starts_with("thrust ") => {
                if let Some(idx) = extract_index(key, "thrust ", ", kgs") {
                    let s = idx - 1;
                    if s < MAX_ENGINES {
                        if !engine_seen[s] { state.engines[s] = Default::default(); engine_seen[s] = true; }
                        state.engines[s].thrust = parse_f64_or(val, 0.0);
                        if idx > max_engine_idx { max_engine_idx = idx; }
                    }
                }
            }
            _ if key.starts_with("RPM ") => {
                if let Some(idx) = extract_index(key, "RPM ", "") {
                    let s = idx - 1;
                    if s < MAX_ENGINES {
                        if !engine_seen[s] { state.engines[s] = Default::default(); engine_seen[s] = true; }
                        state.engines[s].rpm = parse_f64_or(val, 0.0);
                        if idx > max_engine_idx { max_engine_idx = idx; }
                    }
                }
            }
            _ if key.starts_with("manifold pressure ") => {
                if let Some(idx) = extract_index(key, "manifold pressure ", ", atm") {
                    let s = idx - 1;
                    if s < MAX_ENGINES {
                        if !engine_seen[s] { state.engines[s] = Default::default(); engine_seen[s] = true; }
                        state.engines[s].manifold = parse_f64_or(val, 0.0);
                        if idx > max_engine_idx { max_engine_idx = idx; }
                    }
                }
            }
            _ if key.starts_with("efficiency ") => {
                if let Some(idx) = extract_index(key, "efficiency ", ", %") {
                    let s = idx - 1;
                    if s < MAX_ENGINES {
                        if !engine_seen[s] { state.engines[s] = Default::default(); engine_seen[s] = true; }
                        state.engines[s].efficiency = parse_f64_or(val, 0.0);
                        if idx > max_engine_idx { max_engine_idx = idx; }
                    }
                }
            }
            _ if key.starts_with("pitch ") => {
                if let Some(idx) = extract_index(key, "pitch ", ", deg") {
                    let s = idx - 1;
                    if s < MAX_ENGINES {
                        if !engine_seen[s] { state.engines[s] = Default::default(); engine_seen[s] = true; }
                        state.engines[s].pitch = parse_f64_or(val, 0.0);
                        if idx > max_engine_idx { max_engine_idx = idx; }
                    }
                }
            }
            _ if key.starts_with("oil temp ") => {
                if let Some(idx) = extract_index(key, "oil temp ", ", C") {
                    let s = idx - 1;
                    if s < MAX_ENGINES {
                        if !engine_seen[s] { state.engines[s] = Default::default(); engine_seen[s] = true; }
                        state.engines[s].oil_temp = parse_f64_or(val, 0.0);
                        if idx > max_engine_idx { max_engine_idx = idx; }
                    }
                }
            }
            _ if key.starts_with("mixture ") => {
                if let Some(idx) = extract_index(key, "mixture ", ", %") {
                    let s = idx - 1;
                    if s < MAX_ENGINES {
                        if !engine_seen[s] { state.engines[s] = Default::default(); engine_seen[s] = true; }
                        state.engines[s].mixture = parse_f64_or(val, 0.0);
                        if idx > max_engine_idx { max_engine_idx = idx; }
                    }
                }
            }
            _ if key.starts_with("radiator ") => {
                if let Some(idx) = extract_index(key, "radiator ", ", %") {
                    let s = idx - 1;
                    if s < MAX_ENGINES {
                        if !engine_seen[s] { state.engines[s] = Default::default(); engine_seen[s] = true; }
                        state.engines[s].radiator = parse_f64_or(val, 0.0);
                        if idx > max_engine_idx { max_engine_idx = idx; }
                    }
                }
            }
            _ if key.starts_with("magneto ") => {
                if let Some(idx) = extract_index(key, "magneto ", "") {
                    let s = idx - 1;
                    if s < MAX_ENGINES {
                        if !engine_seen[s] { state.engines[s] = Default::default(); engine_seen[s] = true; }
                        state.engines[s].magneto = parse_f64(val).unwrap_or(0.0) as i32;
                        if idx > max_engine_idx { max_engine_idx = idx; }
                    }
                }
            }

            _ => { iter.next_values(); }
        }
    }

    state.engine_count = max_engine_idx;
    if !found_valid { state.valid = false; }
    found_valid
}

pub fn parse_indicators_json(json: &str, indic: &mut crate::Indicators) -> bool {
    if json.trim().is_empty() { indic.valid = false; return false; }

    let mut found_valid = false;
    let mut head_temp_seen = false;
    let mut iter = FlatJsonIter::new(json);

    while let Some(key) = iter.next_key(3) {
        let val = iter.next_value();
        match key {
            "valid" => {
                found_valid = parse_bool(val);
                if !found_valid { indic.valid = false; return true; }
                indic.valid = true;
            }
            "army" => indic.army.assign(parse_str(val).as_bytes()),
            "type" => indic.aircraft_type.assign(parse_str(val).as_bytes()),
            "speed" => indic.speed = parse_f64_or(val, 0.0),
            "vario" => indic.vario = parse_f64_or(val, 0.0),
            "altitude_hour" => indic.altitude = parse_f64_or(val, 0.0),
            "compass" => indic.compass = parse_f64_or(val, 0.0),
            "aviahorizon_roll" => { indic.roll = parse_f64_or(val, 0.0); indic.has_attitude = true; },
            "aviahorizon_pitch" => { indic.pitch = parse_f64_or(val, 0.0); indic.has_attitude = true; },
            "rpm" => indic.rpm = parse_f64_or(val, 0.0),
            "mach" => indic.mach = parse_f64_or(val, 0.0),
            "g_meter" => indic.g_meter = parse_f64_or(val, 1.0),
            "aoa" => indic.aoa = parse_f64_or(val, 0.0),
            "throttle" => indic.throttle = parse_f64_or(val, 0.0),
            "gears" => indic.gear = parse_f64_or(val, 0.0),
            "airbrake_lever" => indic.airbrake = parse_f64_or(val, 0.0),
            "flaps" => indic.flaps = parse_f64_or(val, 0.0),
            "radio_altitude" => indic.radio_altitude = parse_f64_or(val, 0.0),
            "altitude_10k" => indic.altitude_10k = parse_f64_or(val, 0.0),
            "trimmer" => indic.trimmer = parse_f64_or(val, 0.0),
            "manifold_pressure" => indic.manifold_pressure = parse_f64_or(val, 0.0),
            "fuel" => indic.fuel = parse_f64_or(val, 0.0),
            "compass1" => indic.compass1 = parse_f64(val),
            "head_temperature" => { head_temp_seen = true; indic.temperature = parse_f64_or(val, 0.0); }
            "water_temperature" => { if !head_temp_seen { indic.temperature = parse_f64_or(val, 0.0); } }
            "oil_temperature" => indic.oil_temp = parse_f64_or(val, 0.0),
            "wing_sweep_indicator" => indic.wing_sweep_indicator = parse_f64_or(val, 0.0),

            _ if key.starts_with("fuel") && key.len() > 4
                && key.as_bytes()[4..].iter().all(|&b| b.is_ascii_digit()) =>
            {
                if let Ok(tank) = key[4..].parse::<usize>() {
                    if tank > 0 && tank <= MAX_FUEL_TANKS {
                        indic.fuels[tank - 1] = parse_f64_or(val, 0.0);
                        if tank > indic.fuel_tank_count { indic.fuel_tank_count = tank; }
                    }
                }
            }

            _ => {}
        }
    }

    if !found_valid { indic.valid = false; }
    found_valid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flat_json_iter_basic() {
        let json = r#"{"valid": true, "speed": 123.4}"#;
        let mut iter = FlatJsonIter::new(json);
        let mut pairs = Vec::new();
        while let Some(k) = iter.next_key(3) {
            pairs.push((k, iter.next_value()));
        }
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("valid", "true"));
        assert_eq!(pairs[1], ("speed", "123.4"));
    }

    #[test]
    fn test_parse_f64() {
        assert_eq!(parse_f64("123.4"), Some(123.4));
        assert_eq!(parse_f64("null"), None);
        assert_eq!(parse_f64("abc"), None);
    }

    #[test]
    fn test_parse_bool() {
        assert!(parse_bool("true"));
        assert!(!parse_bool("false"));
    }

    #[test]
    fn test_state_json() {
        let json = r#"{"valid": true, "TAS, km/h": 800.5, "Mfuel0, kg": 3000}"#;
        let mut state = crate::FlightState::default();
        parse_state_json(json, &mut state);
        assert!(state.valid);
        assert!((state.tas - 800.5).abs() < 0.01);
        assert!((state.fuel0 - 3000.0).abs() < 0.01);
    }

    #[test]
    fn test_indicators_json() {
        let json = r#"{"valid": true, "army": "air", "speed": 500.0}"#;
        let mut indic = crate::Indicators::default();
        parse_indicators_json(json, &mut indic);
        assert!(indic.valid);
        assert_eq!(indic.army.as_str(), "air");
        assert!((indic.speed - 500.0).abs() < 0.01);
    }

    #[test]
    fn test_engine_field() {
        let json = r#"{"valid": true, "throttle 1, %": 75.0, "power 1, hp": 2000.0, "thrust 2, kgs": 500.0}"#;
        let mut state = crate::FlightState::default();
        parse_state_json(json, &mut state);
        assert_eq!(state.engine_count, 2);
        assert!((state.engines[0].throttle - 75.0).abs() < 0.01);
        assert!((state.engines[0].power - 2000.0).abs() < 0.01);
        assert!((state.engines[1].thrust - 500.0).abs() < 0.01);
    }

    #[test]
    fn test_fuel_tank() {
        let json = r#"{"valid": true, "fuel": 1000.0, "fuel1": 200.0, "fuel2": 50.0}"#;
        let mut indic = crate::Indicators::default();
        parse_indicators_json(json, &mut indic);
        assert!((indic.fuel - 1000.0).abs() < 0.01);
        assert_eq!(indic.fuel_tank_count, 2);
        assert!((indic.fuels[0] - 200.0).abs() < 0.01);
        assert!((indic.fuels[1] - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_valid_false() {
        let json = r#"{"valid": false}"#;
        let mut state = crate::FlightState::default();
        parse_state_json(json, &mut state);
        assert!(!state.valid);
    }

    #[test]
    fn test_temperature_fallback() {
        let json = r#"{"valid": true, "water_temperature": 85.0}"#;
        let mut indic = crate::Indicators::default();
        parse_indicators_json(json, &mut indic);
        assert!((indic.temperature - 85.0).abs() < 0.01);

        let json2 = r#"{"valid": true, "head_temperature": 90.0, "water_temperature": 85.0}"#;
        let mut indic2 = crate::Indicators::default();
        parse_indicators_json(json2, &mut indic2);
        assert!((indic2.temperature - 90.0).abs() < 0.01);
    }

    #[test]
    fn test_parse_f64_2() {
        let result = parse_f64_2("[ 52719.39843750, 55385.300781250 ]");
        assert!((result[0] - 52719.39843750).abs() < 0.001);
        assert!((result[1] - 55385.300781250).abs() < 0.001);
    }

    #[test]
    fn test_parse_map_info_json() {
        let json = r#"{"grid_size" : [ 52719.39843750, 55385.300781250 ], "hud_type" : 0, "map_generation" : 1, "valid" : true}"#;
        let mut info = crate::MapInfo::default();
        parse_map_info_json(json, &mut info);
        assert!(info.valid);
        assert!((info.grid_size[0] - 52719.39843750).abs() < 0.001);
    }

    #[test]
    fn test_flat_json_iter_array() {
        let json = r#"{"grid_size": [ 1.0, 2.0 ], "valid": true}"#;
        let mut iter = FlatJsonIter::new(json);
        let mut pairs = Vec::new();
        while let Some(k) = iter.next_key(3) {
            pairs.push((k, iter.next_value()));
        }
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "grid_size");
        assert_eq!(pairs[1], ("valid", "true"));
    }

    #[test]
    fn test_real_indicators_multi_line() {
        let json = "{\n  \"valid\": true,\n  \"army\": \"air\",\n  \"type\": \"yak-9p\",\n  \"speed\": 146.089279,\n  \"vario\": 16.575209,\n  \"altitude_hour\": 615.050110,\n  \"compass\": 305.543427,\n  \"oil_temperature\": 99.103058,\n  \"water_temperature\": 99.610443,\n  \"throttle\": 1.100000,\n  \"flaps\": 0.500000,\n  \"gears\": 0.500000,\n  \"aoa\": 0.000000,\n  \"mach\": 0.440000,\n  \"wing_sweep_indicator\": 0.000000\n}";
        let mut indic = crate::Indicators::default();
        parse_indicators_json(json, &mut indic);
        assert!(indic.valid);
        assert_eq!(indic.army.as_str(), "air");
        assert_eq!(indic.aircraft_type.as_str(), "yak-9p");
        assert!((indic.speed - 146.089279).abs() < 0.001);
        assert!((indic.vario - 16.575209).abs() < 0.001);
        assert!((indic.altitude - 615.050110).abs() < 0.001);
        assert!((indic.compass - 305.543427).abs() < 0.001);
        assert!((indic.oil_temp - 99.103058).abs() < 0.001);
        assert!((indic.temperature - 99.610443).abs() < 0.001);
        assert!((indic.throttle - 1.100000).abs() < 0.001);
        assert!((indic.flaps - 0.500000).abs() < 0.001);
        assert!((indic.gear - 0.500000).abs() < 0.001);
    }

    #[test]
    fn test_real_state_multi_line() {
        let json = "{\n  \"valid\": true,\n  \"aileron, %\": -2,\n  \"elevator, %\": 0,\n  \"rudder, %\": -4,\n  \"flaps, %\": 0,\n  \"gear, %\": 0,\n  \"H, m\": 615,\n  \"TAS, km/h\": 541,\n  \"IAS, km/h\": 526,\n  \"M\": 0.44,\n  \"AoA, deg\": 1.1,\n  \"AoS, deg\": 0,\n  \"Ny\": 1.59,\n  \"Vy, m/s\": 16.6,\n  \"Wx, deg/s\": 2,\n  \"Mfuel, kg\": 151,\n  \"Mfuel0, kg\": 516,\n  \"throttle 1, %\": 110,\n  \"power 1, hp\": 1633,\n  \"RPM 1\": 3200,\n  \"manifold pressure 1, atm\": 1.46,\n  \"efficiency 1, %\": 85,\n  \"pitch 1, deg\": 40.7,\n  \"oil temp 1, C\": 99,\n  \"mixture 1, %\": 100,\n  \"radiator 1, %\": 0,\n  \"magneto 1\": 3,\n  \"thrust 1, kgs\": 700\n}";
        let mut state = crate::FlightState::default();
        parse_state_json(json, &mut state);
        assert!(state.valid);
        assert!((state.aileron - (-2.0)).abs() < 0.1);
        assert!((state.elevator - 0.0).abs() < 0.1);
        assert!((state.rudder - (-4.0)).abs() < 0.1);
        assert!((state.altitude - 615.0).abs() < 0.5);
        assert!((state.tas - 541.0).abs() < 0.5);
        assert!((state.ias - 526.0).abs() < 0.5);
        assert!((state.M - 0.44).abs() < 0.01);
        assert!((state.aoa - 1.1).abs() < 0.01);
        assert!((state.Ny - 1.59).abs() < 0.01);
        assert!((state.vy - 16.6).abs() < 0.1);
        assert!((state.wx - 2.0).abs() < 0.1);
        assert!((state.fuel - 151.0).abs() < 0.5);
        assert!((state.fuel0 - 516.0).abs() < 0.5);
        assert_eq!(state.engine_count, 1);
        assert!((state.engines[0].throttle - 110.0).abs() < 0.5);
        assert!((state.engines[0].power - 1633.0).abs() < 0.5);
        assert!((state.engines[0].rpm - 3200.0).abs() < 0.5);
        assert!((state.engines[0].manifold - 1.46).abs() < 0.01);
        assert!((state.engines[0].efficiency - 85.0).abs() < 0.5);
        assert!((state.engines[0].pitch - 40.7).abs() < 0.1);
        assert!((state.engines[0].oil_temp - 99.0).abs() < 0.5);
        assert!((state.engines[0].mixture - 100.0).abs() < 0.5);
        assert!((state.engines[0].radiator - 0.0).abs() < 0.5);
        assert_eq!(state.engines[0].magneto, 3);
        assert!((state.engines[0].thrust - 700.0).abs() < 0.5);
    }

    #[test]
    fn test_real_map_info_multi_line() {
        let json = "{\n   \"grid_size\" : [ 52719.39843750, 55385.300781250 ],\n   \"grid_steps\" : [ 5500.0, 5500.0 ],\n   \"grid_zero\" : [ 6494.300781250, 19547.50 ],\n   \"hud_type\" : 0,\n   \"map_generation\" : 1,\n   \"map_max\" : [ 65536.0, 65536.0 ],\n   \"map_min\" : [ -65536.0, -65536.0 ],\n   \"valid\" : true\n}";
        let mut info = crate::MapInfo::default();
        parse_map_info_json(json, &mut info);
        assert!(info.valid);
        assert!((info.grid_size[0] - 52719.39843750).abs() < 0.001);
        assert!((info.grid_size[1] - 55385.300781250).abs() < 0.001);
        assert!((info.grid_steps[0] - 5500.0).abs() < 0.001);
        assert!((info.grid_steps[1] - 5500.0).abs() < 0.001);
        assert!((info.grid_zero[0] - 6494.300781250).abs() < 0.001);
        assert!((info.grid_zero[1] - 19547.50).abs() < 0.001);
        assert_eq!(info.hud_type, 0);
        assert_eq!(info.map_generation, 1);
        assert!((info.map_max[0] - 65536.0).abs() < 0.001);
        assert!((info.map_max[1] - 65536.0).abs() < 0.001);
        assert!((info.map_min[0] - (-65536.0)).abs() < 0.001);
        assert!((info.map_min[1] - (-65536.0)).abs() < 0.001);
    }

    #[test]
    fn test_map_obj_parse() {
        let json = "[\n\
{\"type\":\"airfield\",\"color\":\"#174DFF\",\"color[]\":[23,77,255],\"blink\":0,\"icon\":\"none\",\"icon_bg\":\"none\",\"sx\":0.688678,\"sy\":0.488574,\"ex\":0.664311,\"ey\":0.490086},\n\
{\"type\":\"aircraft\",\"color\":\"#faC81E\",\"color[]\":[250,200,30],\"blink\":0,\"icon\":\"Player\",\"icon_bg\":\"none\",\"x\":0.629527,\"y\":0.416373,\"dx\":0.893540,\"dy\":-0.448983},\n\
{\"type\":\"aircraft\",\"color\":\"#f00C00\",\"color[]\":[240,12,0],\"blink\":2,\"icon\":\"Fighter\",\"icon_bg\":\"none\",\"x\":0.671751,\"y\":0.478026,\"dx\":-0.027066,\"dy\":0.999634},\n\
{\"type\":\"ground_model\",\"color\":\"#fa0C00\",\"color[]\":[250,12,0],\"blink\":0,\"icon\":\"TorpedoBoat\",\"icon_bg\":\"none\",\"x\":0.604958,\"y\":0.466427},\n\
{\"type\":\"ground_model\",\"color\":\"#fa0C00\",\"color[]\":[250,12,0],\"blink\":0,\"icon\":\"LightTank\",\"icon_bg\":\"none\",\"x\":0.654875,\"y\":0.485577},\n\
{\"type\":\"ground_model\",\"color\":\"#fa0C00\",\"color[]\":[250,12,0],\"blink\":0,\"icon\":\"MediumTank\",\"icon_bg\":\"none\",\"x\":0.653106,\"y\":0.485577},\n\
{\"type\":\"ground_model\",\"color\":\"#fa0C00\",\"color[]\":[250,12,0],\"blink\":0,\"icon\":\"SPAA\",\"icon_bg\":\"none\",\"x\":0.647668,\"y\":0.517617},\n\
{\"type\":\"ground_model\",\"color\":\"#fa0C00\",\"color[]\":[250,12,0],\"blink\":0,\"icon\":\"Boat\",\"icon_bg\":\"none\",\"x\":0.629246,\"y\":0.497568},\n\
{\"type\":\"ground_model\",\"color\":\"#fa0C00\",\"color[]\":[250,12,0],\"blink\":0,\"icon\":\"Ship\",\"icon_bg\":\"none\",\"x\":0.636611,\"y\":0.481949},\n\
{\"type\":\"bombing_point\",\"color\":\"#fa0C00\",\"color[]\":[250,12,0],\"blink\":0,\"icon\":\"bombing_point\",\"icon_bg\":\"none\",\"x\":0.659695,\"y\":0.456213}\n\
]";
        let mut data = crate::MapObjData::default();
        parse_map_obj_data_json(json, &mut data);
        assert!(data.valid);
        assert_eq!(data.objects.len(), 10);
        assert_eq!(data.objects[0].obj_type.as_str(), "airfield");
        assert_eq!(data.objects[0].icon.as_str(), "none");
        assert!((data.objects[0].sx.unwrap() - 0.688678).abs() < 0.0001);
        assert!((data.objects[0].ex.unwrap() - 0.664311).abs() < 0.0001);

        let player = &data.objects[1];
        assert_eq!(player.obj_type.as_str(), "aircraft");
        assert_eq!(player.icon.as_str(), "Player");
        assert!((player.x.unwrap() - 0.629527).abs() < 0.0001);
        assert!((player.y.unwrap() - 0.416373).abs() < 0.0001);
        assert!((player.dx.unwrap() - 0.893540).abs() < 0.0001);
        assert!((player.dy.unwrap() - (-0.448983)).abs() < 0.0001);

        let fighter = &data.objects[2];
        assert_eq!(fighter.obj_type.as_str(), "aircraft");
        assert_eq!(fighter.icon.as_str(), "Fighter");

        assert_eq!(data.objects[3].obj_type.as_str(), "ground_model");
        assert_eq!(data.objects[3].icon.as_str(), "TorpedoBoat");

        assert_eq!(data.objects[4].icon.as_str(), "LightTank");
        assert_eq!(data.objects[5].icon.as_str(), "MediumTank");
        assert_eq!(data.objects[6].icon.as_str(), "SPAA");
        assert_eq!(data.objects[7].icon.as_str(), "Boat");
        assert_eq!(data.objects[8].icon.as_str(), "Ship");
        assert_eq!(data.objects[9].obj_type.as_str(), "bombing_point");
    }

    #[test]
    #[ignore = "100 万次迭代的基准，默认跳过（cargo test -- --ignored 可跑）"]
    fn bench_flat_json_parse() {
        let state_json = "{\n  \"valid\": true,\n  \"aileron, %\": -2,\n  \"elevator, %\": 0,\n  \"rudder, %\": -4,\n  \"flaps, %\": 0,\n  \"gear, %\": 0,\n  \"H, m\": 615,\n  \"TAS, km/h\": 541,\n  \"IAS, km/h\": 526,\n  \"M\": 0.44,\n  \"AoA, deg\": 1.1,\n  \"AoS, deg\": 0,\n  \"Ny\": 1.59,\n  \"Vy, m/s\": 16.6,\n  \"Wx, deg/s\": 2,\n  \"Mfuel, kg\": 151,\n  \"Mfuel0, kg\": 516\n}";
        let indic_json = "{\n  \"valid\": true,\n  \"army\": \"air\",\n  \"type\": \"yak-9p\",\n  \"speed\": 146.089279,\n  \"vario\": 16.575209,\n  \"altitude_hour\": 615.050110,\n  \"compass\": 305.543427,\n  \"oil_temperature\": 99.103058,\n  \"water_temperature\": 99.610443,\n  \"throttle\": 1.100000,\n  \"flaps\": 0.500000,\n  \"gears\": 0.500000\n}";
        let map_info_json = "{\n   \"grid_size\" : [ 52719.39843750, 55385.300781250 ],\n   \"grid_steps\" : [ 5500.0, 5500.0 ],\n   \"grid_zero\" : [ 6494.300781250, 19547.50 ],\n   \"hud_type\" : 0,\n   \"map_generation\" : 1,\n   \"map_max\" : [ 65536.0, 65536.0 ],\n   \"map_min\" : [ -65536.0, -65536.0 ],\n   \"valid\" : true\n}";
        let map_obj_json = "[\n\
{\"type\":\"airfield\",\"color\":\"#174DFF\",\"color[]\":[23,77,255],\"blink\":0,\"icon\":\"none\",\"icon_bg\":\"none\",\"sx\":0.688678,\"sy\":0.488574,\"ex\":0.664311,\"ey\":0.490086},\n\
{\"type\":\"aircraft\",\"color\":\"#faC81E\",\"color[]\":[250,200,30],\"blink\":0,\"icon\":\"Player\",\"icon_bg\":\"none\",\"x\":0.629527,\"y\":0.416373,\"dx\":0.893540,\"dy\":-0.448983}]";

        const N: u64 = 1_000_000;
        use std::time::Instant;

        let start = Instant::now();
        for _ in 0..N {
            let mut state = crate::FlightState::default();
            parse_state_json(state_json, &mut state);
            let mut indic = crate::Indicators::default();
            parse_indicators_json(indic_json, &mut indic);
            let mut info = crate::MapInfo::default();
            parse_map_info_json(map_info_json, &mut info);
            let mut obj = crate::MapObjData::default();
            parse_map_obj_data_json(map_obj_json, &mut obj);
        }
        let elapsed = start.elapsed();
        let ops = (N * 4) as f64 / elapsed.as_secs_f64();
        eprintln!(
            "bench: {:.3}s for {} iterations (state+indic+map_info+map_obj), {:.0} obj/s",
            elapsed.as_secs_f64(), N, ops,
        );
    }
}
