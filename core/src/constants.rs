// core 侧共享常量。
//
// 单位换算的单一来源在这里或 `wp8f_flightmodel`；帧 ↔ 毫秒的换算只在 `crate::mapobj`。

// 主循环节拍不是常量：由配置 `refresh_hz`（默认 30，钳 5..=60）决定，
// 换算入口 `wp8f_disp::HudLayoutConfig::refresh_interval_ns()`。
pub const NSEC_PER_SEC: f64 = 1_000_000_000.0;
pub const SEC_PER_HOUR: f64 = 3600.0;
pub const FIVE_MINUTES_SEC: f64 = 300.0;

/// 重力加速度 [m/s²]：全项目 9.81（Dagor `gamePhys/props/atmosphere.cpp`），
/// 单一来源 `wp8f_flightmodel::GRAVITY`，别散写 9.80/9.78。
pub const GRAVITY: f64 = wp8f_flightmodel::GRAVITY;
pub const MS_TO_KMH: f64 = 3.6;        // m/s → km/h
pub const FEET_TO_METERS: f64 = 0.3048;
pub const HP_CONVERSION: f64 = 735.0;  // 推力(kgf) × G × tas(m/s) / 735 → hp

pub const MAX_ENGINES: usize = 8;
pub const MAX_FUEL_TANKS: usize = 8;
pub const ENGINE_TYPE_COUNTER_MAX: i32 = 16384;

pub const INFINITE_FUEL_TIME: f64 = 32768.0;
pub const FUEL_TIME_ROUNDING: f64 = 10.0;
pub const SMOOTHING_FACTOR_NEW: f64 = 0.01;
pub const SMOOTHING_FACTOR_OLD: f64 = 0.99;

pub const PSI_PER_ATM: f64 = 14.696;       // psi / atm（增压表）
pub const MMHG_PER_ATM: f64 = 760.0;       // mmHg / atm
pub const MM_PER_INCH: f64 = 25.4;         // mm / inch
