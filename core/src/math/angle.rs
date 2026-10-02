//! 角度归一/差值、罗盘角，以及同批的小工具（由原 `common.rs` + `math_utils.rs` 合并迁入）。
//! **只搬家不改逻辑**：函数体、签名、可见性、常量值全部保持原样。

pub fn normalize_angle_deg(angle: f64) -> f64 {
    let mut normalized = angle % 360.0;
    if normalized < 0.0 {
        normalized += 360.0;
    }
    normalized
}

pub fn angle_difference_deg(from: f64, to: f64) -> f64 {
    let diff = to - from;
    let mut adjusted = diff;
    if adjusted > 180.0 {
        adjusted -= 360.0;
    } else if adjusted < -180.0 {
        adjusted += 360.0;
    }
    adjusted
}

pub fn parse_hex_color(hex: &str) -> u32 {
    let hex = hex.trim_start_matches('#');
    if hex.len() < 6 {
        return 0xFFFFFFFF;
    }
    let val = u32::from_str_radix(hex, 16).unwrap_or(0);
    let r = ((val >> 16) & 0xFF) as u32;
    let g = ((val >> 8) & 0xFF) as u32;
    let b = (val & 0xFF) as u32;
    (255 << 24) | (r << 16) | (g << 8) | b
}

pub fn compass_deg(dx: f64, dy: f64) -> f64 {
    (dx.atan2(-dy) * 180.0 / std::f64::consts::PI + 360.0) % 360.0
}

pub fn target_bearing_deg(x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    compass_deg(x1 - x0, y1 - y0)
}

pub fn round_sep(sep: f64) -> f64 {
    let abs_sep = sep.abs();
    let step = if abs_sep <= 30.0 {
        1.0
    } else if abs_sep <= 50.0 {
        2.0
    } else if abs_sep <= 100.0 {
        5.0
    } else if abs_sep <= 200.0 {
        10.0
    } else if abs_sep <= 300.0 {
        15.0
    } else if abs_sep <= 400.0 {
        20.0
    } else if abs_sep <= 600.0 {
        50.0
    } else {
        100.0
    };
    (sep / step).round() * step
}
