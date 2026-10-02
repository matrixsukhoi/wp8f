//! wp8f-flightmodel：War Thunder 飞行模型解析 + 气动/发动机/重量计算。
//!
//! # 两套数据源，一个出口
//!
//! FM 文本有两种来源，扁平化后是**同一套键值**（D13，实测键集合 100% 相同）：
//!
//! * **JSON**（GitHub `War-Thunder-Datamine`）：**默认**（`fm-json`），由 [`json_adapter`] 适配
//!   —— 用户手上的 `resource/data` 自 2.59.0.43 起就是 JSON 版 blkx；
//! * **legacy blkx**（旧游戏解包）：**仅作备份**，`fm-legacy` 要显式打开才编译（默认不带）。
//!
//! 分派在 [`parser::parse_fm_text`]，按**内容**判断（首个非空白字节 `{` → JSON），
//! 上层（`flight_model.rs` / weapons / GUI / logger）完全不用知道数据是哪种。
//!
//! # 数据根
//!
//! [`fm_paths::resolve_data_root`] 是 HUD 与 GUI 共用的唯一实现（`resource/data` 与
//! `resource/data_new` 两个根都接受）；[`fm_paths::normalize_blkx_ref`] 是 `.blk` → `.blkx` +
//! 小写的唯一入口（D14）。

pub mod aero;
pub mod em;
pub mod engine_model;
pub mod flight_model;
pub mod fm_paths;
#[cfg(feature = "fm-json")]
pub mod json_adapter;
pub mod parser;
/// FM 数据库更新器（P3）：版本检查 / 逐文件下载 / 校验 / A-B 分区切换。
/// 与 `fm_paths` 同处一 crate：**数据根的唯一实现**在这里，更新器改的就是它。
pub mod update;
pub mod weapon_model;

// 两个 feature 都关掉时没有任何解析器可用 —— 这不是合法配置，直接在编译期报错
// （比"跑起来才发现数据解析全空"清楚得多）。
#[cfg(not(any(feature = "fm-legacy", feature = "fm-json")))]
compile_error!(
    "wp8f-flightmodel 至少需要启用一个解析 feature：`fm-json`（GitHub JSON，默认）或 `fm-legacy`（旧 blkx，备份）"
);

#[cfg(test)]
mod test_fm_parsing;
#[cfg(test)]
mod test_p2_parity;

pub use aero::{
    calculate_aero_forces, calculate_density, calculate_mach, calculate_sound_speed,
    print_aero_surface_info, read_aero_surfaces, AeroForces, AeroSurface, PolarData,
    VariableSweepWing, WingGeometry, GRAVITY,
};
pub use engine_model::{
    engine_count, fuel_rate_at, get_merged_altitudes, get_merged_velocities, has_any_wep,
    powers_at, print_compressor_stages, print_engine_modes, print_power_altitude_table,
    print_power_table, print_thrust_table, read_all_engines, thrusts_at, total_fuel_rates, total_nitro_consumption,
};
pub use engine_model::{CompressorStage, Engine, EngineFuelRates, EngineMode, EngineType};
pub use flight_model::{Aerodynamics, DragBreakdown, EngineLoad, FlightModel, MassProperties, Propulsion, VneLimit};
pub use fm_paths::{
    data_root_candidates_in, flightmodels_dir, is_data_root, normalize_blkx_ref, normalize_weapon_ref,
    resolve_data_root, resolve_data_root_in, strip_blkx_extension, version_file as data_version_file,
    weapons_dir as data_weapons_dir, DATA_DIR_NAME, DATA_NEW_DIR_NAME, DATA_ROOT_ENV,
    FLIGHTMODELS_SUBDIR, GAMEDATA_DIR, RESOURCE_DIR, VERSION_FILE as DATA_VERSION_FILE, WEAPONS_SUBDIR,
};
pub use parser::{
    detect_fm_text_kind, get_f64_first, get_f64_multi, get_string_first, parse_fm_text, BlkxValue,
    BlkxValueType, FmTextKind, FM_JSON_ENABLED, FM_LEGACY_ENABLED,
};
#[cfg(feature = "fm-legacy")]
pub use parser::parse_legacy_blkx_string;
#[cfg(feature = "fm-json")]
pub use json_adapter::parse_json_fm;

use std::path::Path;

/// 解析机型：`data_dir` 是**机型目录**（`<数据根>/gamedata/flightmodels`）。
///
/// 两条数据源共用这一条路径：
/// * 机型文件 `<name>.blkx`（名字先过 [`normalize_blkx_ref`]：补扩展名 + 小写）；
/// * `fmFile` 引用（写的是 `fm/a-20g.blk`，实际文件 `.blkx`）同样归一后再读；
/// * 没有 `fmFile` 时退到 `fm/<name>.blkx`。
pub fn parse_aircraft(aircraft_name: &str, data_dir: &str) -> Result<FlightModel, String> {
    let fm_dir = Path::new(data_dir);
    let weapons_dir = fm_dir.parent().map(|p| p.join("weapons")).unwrap_or_default();

    let blkx_path = fm_dir.join(normalize_blkx_ref(aircraft_name));

    let blkx_content = std::fs::read_to_string(&blkx_path)
        .map_err(|e| format!("Failed to read {}: {}", blkx_path.display(), e))?;

    let data = parse_fm_text(&blkx_content)?;

    let fm_file = data.get("fmFile").map(|v| v.as_string());
    let fm_content = if let Some(ref fm_path) = fm_file {
        // `.blk` → `.blkx` + 小写（D14 归一，唯一入口）
        let fm_full_path = fm_dir.join(normalize_blkx_ref(fm_path));

        if fm_full_path.exists() {
            std::fs::read_to_string(&fm_full_path).ok()
        } else {
            None
        }
    } else {
        let auto_fm_path = fm_dir.join("fm").join(normalize_blkx_ref(aircraft_name));
        if auto_fm_path.exists() {
            std::fs::read_to_string(&auto_fm_path).ok()
        } else {
            None
        }
    };

    let resolved_fm_path = if let Some(fm_path) = fm_file {
        normalize_blkx_ref(&fm_path)
    } else {
        format!("fm/{}", normalize_blkx_ref(aircraft_name))
    };

    let weapons_dir_ref = if weapons_dir.exists() {
        Some(weapons_dir.as_path())
    } else {
        None
    };

    let mut model = FlightModel::parse_checked(&blkx_content, fm_content.as_deref(), weapons_dir_ref)?;
    model.fm_path = resolved_fm_path;
    Ok(model)
}

pub fn parse_aircraft_or_default(aircraft_name: &str, data_dir: &str) -> FlightModel {
    parse_aircraft(aircraft_name, data_dir).unwrap_or_else(|e| {
        eprintln!("[WARN] FM parse failed for {}: {}, using defaults", aircraft_name, e);
        FlightModel::default_placeholder()
    })
}

/// 解析单个 `.blkx` 文件（`fmFile` 指向的文件相对它自己所在目录解析）。
pub fn parse_blkx(blkx_path: &str) -> Result<FlightModel, String> {
    let content = std::fs::read_to_string(blkx_path)
        .map_err(|e| format!("Failed to read {}: {}", blkx_path, e))?;

    let data = parse_fm_text(&content)?;

    let fm_file = data.get("fmFile").map(|v| v.as_string());
    let fm_content = if let Some(ref fm_path) = fm_file {
        let fm_full_path = Path::new(blkx_path)
            .parent()
            .map(|p| p.join(normalize_blkx_ref(fm_path)))
            .unwrap_or_default();

        if fm_full_path.exists() {
            std::fs::read_to_string(&fm_full_path).ok()
        } else {
            None
        }
    } else {
        None
    };

    let resolved_fm_path = if let Some(fm_path) = fm_file {
        normalize_blkx_ref(&fm_path)
    } else {
        String::new()
    };

    let weapons_dir = Path::new(blkx_path)
        .parent()
        .map(|p| p.join("weapons"));
    let weapons_dir_ref = weapons_dir.as_ref().filter(|p| p.exists()).map(|p| p.as_path());

    let mut model = FlightModel::parse_checked(&content, fm_content.as_deref(), weapons_dir_ref)?;
    model.fm_path = resolved_fm_path;
    Ok(model)
}
