use clap::{Parser, ValueEnum};
use std::path::{Path, PathBuf};
use wp8f_flightmodel::{
    calculate_aero_forces, flightmodels_dir, fuel_rate_at, get_merged_altitudes, get_merged_velocities,
    has_any_wep, parse_aircraft, powers_at, print_aero_surface_info, print_compressor_stages,
    print_engine_modes, print_power_altitude_table, print_power_table, print_thrust_table,
    read_aero_surfaces, resolve_data_root, thrusts_at, total_fuel_rates, EngineType, FlightModel, GRAVITY,
};

#[derive(Parser, Debug)]
#[command(name = "flightmodel")]
#[command(about = "War Thunder flight model file parser", long_about = None)]
struct Cli {
    /// 飞机性能数据根目录（**数据根**，`gamedata/` 就在它下面）。
    /// 不传时按 `$WP8F_DATA_DIR` → 当前工作目录/可执行文件附近的 `resource/data`（不存在则
    /// `resource/data_new`）依次找 —— 与 HUD、GUI 共用同一个解析函数
    ///（`wp8f_flightmodel::resolve_data_root`，见 `fm_paths.rs`）。
    #[arg(short, long)]
    data_dir: Option<PathBuf>,

    #[arg(short, long)]
    aircraft: Option<String>,

    #[arg(short, long)]
    list: bool,

    #[arg(short, long, value_enum)]
    format: Option<OutputFormat>,

    #[arg(short, long)]
    verbose: bool,

    #[arg(short, long)]
    plot: bool,

    #[arg(long)]
    em: bool,

    #[arg(long)]
    drag: bool,

    /// 输出曲线 JSON（CL-α 极曲线 + 推力/功率-速度族，供 GUI ECharts 绘制）
    #[arg(long)]
    curves_json: bool,

    /// 计算用燃油比例（%，0..100；takeoff_weight/允许过载/TAS-高度 按此重量）
    #[arg(long, default_value_t = 50.0)]
    fuel_pct: f64,

    /// 额外重量（kg，如外挂/损伤附加）
    #[arg(long, default_value_t = 0.0)]
    extra_weight: f64,

    #[arg(long)]
    max_speed: bool,

    #[arg(long)]
    polar: bool,

    #[arg(long)]
    aero: bool,

    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Clone, ValueEnum)]
enum OutputFormat {
    Json,
    Tsv,
    Info,
}

/// `--data-dir` 指的是**数据根**（`gamedata/` 就在它下面），真正放 .blkx 的是再往下两层。
/// 抽出来只此一份：原先两处各拼一遍，报错还打印的是入参而不是拼好的目录。
fn resolved_fm_dir(data_dir: &Path) -> PathBuf {
    flightmodels_dir(data_dir)
}

fn list_aircraft(fm_dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(fm_dir) {
        let mut aircraft = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext == "blkx" {
                    if let Some(stem) = path.file_stem() {
                        aircraft.push(stem.to_string_lossy().to_string());
                    }
                }
            }
        }

        aircraft.sort();

        println!("=== Aircraft ({}) ===", aircraft.len());
        for name in &aircraft {
            println!("  {}", name);
        }
    } else {
        // 报错要打印**拼好的**目录（不是入参）：`--data-dir` 指的是数据根，
        // 只说入参看不出真正去找的是哪里。
        eprintln!("Error: Cannot read directory {}", fm_dir.display());
    }
}

fn print_aircraft_info(aircraft: &str, data: &FlightModel, verbose: bool) {
    println!("\n=== Aircraft: {} ===", aircraft);
    println!("Engine Type: {}", data.engine_type_string());
    println!("Is Jet: {}", data.is_jet);
    println!("Empty Weight (FM): {:.0} kg", data.empty_weight());
    println!("Max Internal Fuel: {:.0} kg", data.max_fuel());
    let ext_fuel = data.max_external_fuel();
    if ext_fuel > 0.0 {
        println!("Max External Fuel: {:.0} kg", ext_fuel);
    }
    println!("Oil Mass: {:.0} kg", data.oil_mass());

    // Calculate takeoff weight at 100% fuel
    let takeoff_100 = data.takeoff_weight(100.0);
    let takeoff_50 = data.takeoff_weight(50.0);
    println!("\n--- Takeoff Weight ---");
    println!("  At 100% fuel: {:.0} kg", takeoff_100);
    println!("  At 50% fuel: {:.0} kg", takeoff_50);
    if ext_fuel > 0.0 {
        println!(
            "  At 100% fuel (with ext tanks): {:.0} kg",
            data.takeoff_weight_with_ext_tanks(100.0)
        );
    }

    println!("VNE: {:.0} km/h", data.vne());
    println!("Wingspan: {:.1} m", data.wingspan());
    println!("Wing Area: {:.2} m²", data.wing_area());
    println!("Aspect Ratio: {:.2}", data.aspect_ratio());

    if data.is_jet {
        let engine_count = data.engine_count();
        println!("\n--- Jet Engine Data ---");
        println!("Engine Count: {}", engine_count);

        let thrust_max = data.thrust_max();

        let engines = data.engines().to_vec();

        if !engines.is_empty() {
            // Print WEP coefficients and max thrust
            let engine = &engines[0];
            println!("\n--- WEP Coefficients ---");
            println!("AfterburnerBoost: {:.2}", engine.afterburner_boost());
            println!("Last Mode ThrustMult: {:.2}", engine.wep_thrust_mult());
            println!(
                "WEP Multiplier (AfterburnerBoost × ThrustMult): {:.2}",
                engine.wep_multiplier()
            );
            if engine.has_wep() {
                println!("Afterburner Coefficients: Yes (using ThrAftMaxCoeff matrix)");
            } else {
                println!("Afterburner Coefficients: No (using ThrustMaxCoeff × WEP multiplier)");
            }

            // Print max thrust values
            println!("\n--- Max Thrust ---");
            println!(
                "Thrust Max (static, military, total): {:.0} kgs",
                thrust_max
            );
            if has_any_wep(&engines) {
                let max_wep_thrust = thrusts_at(&engines, 0.0, 0.0, 110.0);
                println!("Thrust Max (static, WEP, total): {:.0} kgs", max_wep_thrust);
            }

            // Print thrust tables using helper function
            let has_wep = has_any_wep(&engines);
            print_thrust_table(&engines, 100.0);

            if has_wep {
                print_thrust_table(&engines, 110.0);
            }

            // Print fuel endurance table
            let max_fuel = data.max_fuel();
            let altitudes = get_merged_altitudes(&engines);
            let velocities = get_merged_velocities(&engines);

            println!("\n--- Fuel Endurance Table (seconds, Military) ---");
            print!("{:>12}", "Vel(km/h)");
            for alt in &altitudes {
                print!(" {:>10.0}", *alt);
            }
            println!();

            for vel in &velocities {
                print!("{:>12.0}", *vel);
                for alt in &altitudes {
                    let fuel_rate = fuel_rate_at(&engines, *vel, *alt, 100.0);
                    let endurance = if fuel_rate > 0.0 {
                        max_fuel / fuel_rate
                    } else {
                        0.0
                    };
                    print!(" {:>10.0}", endurance);
                }
                println!();
            }

            if has_wep {
                println!("\n--- Fuel Endurance Table (seconds, WEP) ---");
                print!("{:>12}", "Vel(km/h)");
                for alt in &altitudes {
                    print!(" {:>10.0}", *alt);
                }
                println!();

                for vel in &velocities {
                    print!("{:>12.0}", *vel);
                    for alt in &altitudes {
                        let fuel_rate = fuel_rate_at(&engines, *vel, *alt, 110.0);
                        let endurance = if fuel_rate > 0.0 {
                            max_fuel / fuel_rate
                        } else {
                            0.0
                        };
                        print!(" {:>10.0}", endurance);
                    }
                    println!();
                }
            }

            // 0高度800km/h 军推和WEP
            let fuel_at_0_800_mil = fuel_rate_at(&engines, 800.0, 0.0, 100.0);
            let fuel_at_0_800_wep = fuel_rate_at(&engines, 800.0, 0.0, 110.0);
            let endurance_0_800_mil = if fuel_at_0_800_mil > 0.0 {
                max_fuel / fuel_at_0_800_mil
            } else {
                0.0
            };
            let endurance_0_800_wep = if fuel_at_0_800_wep > 0.0 {
                max_fuel / fuel_at_0_800_wep
            } else {
                0.0
            };

            println!("\n--- Fuel at 0m, 800km/h ---");
            println!(
                "Military: {:.4} kg/s, Endurance: {:.0} s ({:.1} min)",
                fuel_at_0_800_mil,
                endurance_0_800_mil,
                endurance_0_800_mil / 60.0
            );
            if has_wep {
                println!(
                    "WEP: {:.4} kg/s, Endurance: {:.0} s ({:.1} min)",
                    fuel_at_0_800_wep,
                    endurance_0_800_wep,
                    endurance_0_800_wep / 60.0
                );
            }

            // 5分钟WEP燃油量对应的军推时间
            let fuel_5min_wep = fuel_at_0_800_wep * 300.0;
            let mil_time_for_5min_wep_fuel = if fuel_at_0_800_mil > 0.0 {
                fuel_5min_wep / fuel_at_0_800_mil
            } else {
                0.0
            };
            println!(
                "5min WEP fuel ({:.0}kg) at Military: {:.0} s ({:.1} min)",
                fuel_5min_wep,
                mil_time_for_5min_wep_fuel,
                mil_time_for_5min_wep_fuel / 60.0
            );

            let fuel_rates = total_fuel_rates(&engines);
            println!("\n--- Fuel Consumption ---");
            println!("Idle: {:.4} kg/s", fuel_rates.idle);
            println!("Half Throttle: {:.4} kg/s", fuel_rates.half);
            println!("Full Throttle: {:.4} kg/s", fuel_rates.full);
            if has_wep {
                println!("WEP: {:.4} kg/s", fuel_rates.wep);
            }

            let sfc_mil = data.fuel_consumption_coefficient(false);
            let sfc_wep = data.fuel_consumption_coefficient(true);
            println!("\n--- Specific Fuel Consumption ---");
            println!("Fuel Consumption Coefficient ((kg/h)/kgf): Mil={:.3}  WEP={:.3}", sfc_mil, sfc_wep);

            print_engine_modes(&engines);
        }
    } else {
        let engine_type_str = data.engine_type_string();
        println!("\n--- {} Engine Data ---", engine_type_str);
        println!("Engine Count: {}", data.engine_count());

        let engines = data.engines().to_vec();
        let has_wep = has_any_wep(&engines);

        if engine_type_str == "Rocket" {
            let thrust_max = data.thrust_max();
            println!("Thrust Max (static): {:.0} kgf", thrust_max);
        } else {
            println!("Power (total): {:.0} hp", data.total_power());
            print_power_altitude_table(&engines, 100.0);
            if has_wep {
                print_power_altitude_table(&engines, 110.0);
            }
            print_compressor_stages(&engines);
            print_power_table(&engines, 100.0);
            if has_wep {
                print_power_table(&engines, 110.0);
            }
            print_engine_modes(&engines);

            let empty_weight = data.empty_weight();
            println!("\n--- Power & T/W vs Altitude ---");
            println!("{:>8} {:>10} {:>10} {:>10} {:>10}", "Alt(m)", "Mil(hp)", "Mil T/W", "WEP(hp)", "WEP T/W");
            for alt in (0..=12000).step_by(1000) {
                let power_mil = powers_at(&engines, 0.0, alt as f64, 100.0);
                let power_wep = if has_wep { powers_at(&engines, 0.0, alt as f64, 110.0) } else { 0.0 };
                let tw_mil = if empty_weight > 0.0 { power_mil / empty_weight } else { 0.0 };
                let tw_wep = if empty_weight > 0.0 { power_wep / empty_weight } else { 0.0 };
                println!("{:>8} {:>10.0} {:>10.2} {:>10.0} {:>10.2}", alt, power_mil, tw_mil, power_wep, tw_wep);
            }
        }

        let fuel_rates = total_fuel_rates(&engines);
        println!("\n--- Fuel Consumption ---");
        println!("Idle: {:.4} kg/s", fuel_rates.idle);
        println!("Half Throttle: {:.4} kg/s", fuel_rates.half);
        println!("Full Throttle: {:.4} kg/s", fuel_rates.full);
        if has_wep {
            println!("WEP: {:.4} kg/s", fuel_rates.wep);
        }
        if data.has_nitro() {
            println!("Nitro: {:.4} L/s", data.nitro_consumption());
        }
    }

    if verbose {
        println!("\n--- Full Summary ---");
        println!("{}", data.summary());
    }
}

fn main() {
    let cli = Cli::parse();

    // 数据根：显式 `--data-dir` > `$WP8F_DATA_DIR` > cwd / 可执行文件附近的
    // `resource/data`（不存在则 `resource/data_new`，A/B 分区的暂存根也能直接跑）。
    // 找不到给可读错误并退出非 0 —— 之前默认值是 clap 里硬编码的 `./resource/data`（相对 cwd），
    // 从别处启动就读不到、还查不出原因。
    let data_root = match resolve_data_root(cli.data_dir.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(2);
        }
    };

    if cli.list {
        let fm_dir = resolved_fm_dir(&data_root);
        println!("Available aircraft (in {}):", fm_dir.display());
        list_aircraft(&fm_dir);
        return;
    }

    if let Some(ref aircraft) = cli.aircraft {
        let fm_dir = resolved_fm_dir(&data_root);
        let data_dir = fm_dir.to_string_lossy().to_string();

        match parse_aircraft(aircraft, &data_dir) {
            Ok(data) => {
                if cli.curves_json {
                    print_curves_json(aircraft, &data, &cli);
                } else if cli.em {
                    generate_em_curves(aircraft, &data, &cli);
                } else if cli.drag {
                    generate_drag_breakdown(aircraft, &data, &cli);
                } else if cli.max_speed {
                    generate_max_speed(aircraft, &data, &cli);
                } else if cli.polar {
                    generate_polar_curve(aircraft, &data, &cli);
                } else if cli.aero {
                    generate_aero_test(aircraft, &data, &cli);
                } else if cli.plot {
                    if data.is_jet {
                        generate_jet_thrust_plot(aircraft, &data, &cli);
                    } else {
                        generate_piston_power_plot(aircraft, &data, &cli);
                    }
                } else {
                    match cli.format {
                        Some(OutputFormat::Json) => {
                            print_json(aircraft, &data, &cli);
                        }
                        Some(OutputFormat::Tsv) => {
                            print_tsv(aircraft, &data);
                        }
                        Some(OutputFormat::Info) | None => {
                            print_aircraft_info(aircraft, &data, cli.verbose);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }
    } else {
        println!("Use --list to see available aircraft, or --aircraft <name> to parse a specific aircraft");
        println!("\nExamples:");
        println!("  flightmodel --list");
        println!("  flightmodel --aircraft yak-3 --verbose");
        println!("  flightmodel --aircraft su-30sm2 --plot");
    }
}

/// JSON 数值格式化：非有限（NaN/±inf）输出 `null`（保证合法 JSON）；
/// 有限值与 `{:.*}` 的输出逐字节一致。
fn json_num(precision: usize, v: f64) -> String {
    if v.is_finite() {
        format!("{:.*}", precision, v)
    } else {
        "null".to_string()
    }
}

/// JSON 字符串转义（`\`/`"`/控制字符），防止机型名等破坏 JSON 结构。
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// 曲线 JSON：CL-α 极曲线（光洁/全襟翼）+ 推力(喷气)/功率(活塞)-速度族（0/4/8km）。
/// 供 GUI ECharts 绘制与多机型对比。
fn print_curves_json(aircraft: &str, data: &FlightModel, cli: &Cli) {
    let fmt_vec = |v: &[f64], p: usize| -> String {
        v.iter()
            .map(|x| json_num(p, *x))
            .collect::<Vec<_>>()
            .join(", ")
    };

    // 计算重量（--fuel-pct / --extra-weight；默认 50% 燃油 = 既有口径）
    let fuel_pct = if cli.fuel_pct.is_finite() { cli.fuel_pct.clamp(0.0, 100.0) } else { 50.0 };
    let extra = if cli.extra_weight.is_finite() && cli.extra_weight > 0.0 {
        cli.extra_weight
    } else {
        0.0
    };
    let weight_kg = data.takeoff_weight(fuel_pct) + extra;
    let weight_n = weight_kg * GRAVITY;

    let aoa: Vec<f64> = (-25..=30).map(|a| a as f64).collect();
    let cl_clean: Vec<f64> = aoa
        .iter()
        .map(|&a| data.aerodynamics.cl_at_mach(a, 0.0, 0.0, 0.3))
        .collect();
    let cl_full: Vec<f64> = aoa
        .iter()
        .map(|&a| data.aerodynamics.cl_at_mach(a, 1.0, 0.0, 0.3))
        .collect();

    let alts = [0.0f64, 4000.0, 8000.0];
    let engines = data.engines();
    let speeds_kmh: Vec<f64> = (0..=28).map(|i| i as f64 * 90.0).collect(); // 0..2520 km/h
    let unit = if data.is_jet { "kgf" } else { "hp" };

    // —— 活塞功率-高度（0..20km 步 500m，v=0；与 generate_piston_power_plot 同口径）——
    // 喷气无活塞功率概念 → null（前端隐藏该图，对应 matplotlib 只对活塞出图）
    let power_line = if data.is_jet {
        "  \"power_alt\": null,".to_string()
    } else {
        let p_alts: Vec<f64> = (0..=40).map(|i| i as f64 * 500.0).collect();
        let p_mil: Vec<f64> = p_alts
            .iter()
            .map(|&a| wp8f_flightmodel::powers_at(&engines, 0.0, a, 100.0))
            .collect();
        let p_wep: Vec<f64> = p_alts
            .iter()
            .map(|&a| wp8f_flightmodel::powers_at(&engines, 0.0, a, 110.0))
            .collect();
        format!(
            "  \"power_alt\": {{\"alt\": [{}], \"mil\": [{}], \"wep\": [{}]}},",
            fmt_vec(&p_alts, 0),
            fmt_vec(&p_mil, 2),
            fmt_vec(&p_wep, 2)
        )
    };

    // —— 推力 3D 曲面（喷气；velocity × altitude → 军推，21×21 网格）——
    // 与 plot_flight_model.py 的 plot_jet_thrust_3d 同口径；活塞 → null
    let thrust3d_block = if data.is_jet {
        let vs: Vec<f64> = (0..21).map(|i| i as f64 * 120.0).collect(); // 0..2400 km/h
        let als: Vec<f64> = (0..21).map(|i| i as f64 * 1000.0).collect(); // 0..20000 m
        let mut rows: Vec<String> = Vec::with_capacity(als.len());
        for &a in &als {
            let row: Vec<String> = vs
                .iter()
                .map(|&v| json_num(2, wp8f_flightmodel::thrusts_at(&engines, v, a, 100.0)))
                .collect();
            rows.push(format!("    [{}]", row.join(", ")));
        }
        format!(
            "  \"thrust3d\": {{\"v\": [{}], \"alt\": [{}], \"z\": [\n{}\n  ]}},",
            fmt_vec(&vs, 0),
            fmt_vec(&als, 0),
            rows.join(",\n")
        )
    } else {
        "  \"thrust3d\": null,".to_string()
    };

    // —— 真空速-高度：121 点（0..12000m 步 100m），与 `_tas_alt.txt` 生成完全同口径 ——
    // tas_wep/mil = max_level_flight_speed(alt, W, use_ab)×3.6（0 = 不可维持）；
    // vne_tas = VNE_ias/√(ρ/ρ0)、mne_tas = MNE×声速×3.6；阻力 = 该点平飞阻力（kgf）
    let tas_alt_line = {
        let mut alts_v = Vec::with_capacity(121);
        let mut tw = Vec::with_capacity(121);
        let mut tm = Vec::with_capacity(121);
        let mut vn = Vec::with_capacity(121);
        let mut mn = Vec::with_capacity(121);
        let mut dw = Vec::with_capacity(121);
        let mut dm = Vec::with_capacity(121);
        for i in 0..=120u32 {
            let alt = (i * 100) as f64;
            let v_wep = data.max_level_flight_speed(alt, weight_n, true);
            let v_mil = data.max_level_flight_speed(alt, weight_n, false);
            let ratio =
                (wp8f_flightmodel::calculate_density(alt) / wp8f_flightmodel::aero::DAGOR_RO0)
                    .sqrt();
            let vne_tas = data.vne() / ratio;
            let mne_tas = data.vne_mach() * wp8f_flightmodel::calculate_sound_speed(alt) * 3.6;
            let drag_w = if v_wep > 0.0 {
                data.drag_level_flight(alt, v_wep, weight_n) / GRAVITY
            } else {
                0.0
            };
            let drag_m = if v_mil > 0.0 {
                data.drag_level_flight(alt, v_mil, weight_n) / GRAVITY
            } else {
                0.0
            };
            alts_v.push(alt);
            tw.push(v_wep * 3.6);
            tm.push(v_mil * 3.6);
            vn.push(vne_tas);
            mn.push(mne_tas);
            dw.push(drag_w);
            dm.push(drag_m);
        }
        format!(
            "  \"tas_alt\": {{\"alt\": [{}], \"tas_wep\": [{}], \"tas_mil\": [{}], \
             \"vne_tas\": [{}], \"mne_tas\": [{}], \"drag_wep\": [{}], \"drag_mil\": [{}]}},",
            fmt_vec(&alts_v, 0),
            fmt_vec(&tw, 1),
            fmt_vec(&tm, 1),
            fmt_vec(&vn, 1),
            fmt_vec(&mn, 1),
            fmt_vec(&dw, 1),
            fmt_vec(&dm, 1)
        )
    };

    // —— 每高度速度限制（推力-速度图裁剪/边界；0/4/8km 与推力族一致）——
    // 与 `_limits.txt` 同口径，另附 ratio=√(ρ/ρ0)（前端把 TAS 轴换算成 IAS 轴）
    let limits_line = {
        let mut parts: Vec<String> = Vec::with_capacity(alts.len());
        for &alt in &alts {
            let ratio =
                (wp8f_flightmodel::calculate_density(alt) / wp8f_flightmodel::aero::DAGOR_RO0)
                    .sqrt();
            let vne_tas = data.vne() / ratio;
            let m_tas = data.vne_mach() * wp8f_flightmodel::calculate_sound_speed(alt) * 3.6;
            let m_ias = m_tas * ratio;
            parts.push(format!(
                "    \"{}\": {{\"vne_tas\": {}, \"vne_ias\": {}, \"mach_tas\": {}, \
                 \"mach_ias\": {}, \"ratio\": {}}}",
                format!("{:.0}", alt),
                json_num(1, vne_tas),
                json_num(1, data.vne()),
                json_num(1, m_tas),
                json_num(1, m_ias),
                json_num(6, ratio)
            ));
        }
        format!("  \"limits\": {{\n{}\n  }},", parts.join(",\n"))
    };

    // —— EM（能量机动）数据：只对喷气机有意义（与 `--em` 的 TSV **同一份计算** `em_compute`）——
    // 前端按 flightmodel/scripts/plot_flight_model.py 的 `plot_em` 画法绘制：
    // Ps 色带（WEP）+ 瞬时/持续盘旋包线 + 峰值标注 + 等过载线 + VNE 竖线。
    // 等过载线的横轴换算用 `ias_per_tas`（IAS = TAS·√(ρ/ρ0)），逐高度给一次。
    let em_block = if data.is_jet {
        let em = em_compute(data, weight_n);
        let mut rows: Vec<String> = Vec::with_capacity(em.len());
        for a in &em {
            rows.push(format!(
                "    {{\"alt\": {}, \"ias_per_tas\": {}, \
                  \"grid\": {{\"v\": [{}], \"w\": [{}], \"ps_wep\": [{}], \"ps_mil\": [{}]}}, \
                  \"curve\": {{\"v\": [{}], \"n_av\": [{}], \"n_sus\": [{}], \"w_inst\": [{}], \
                  \"w_sus_wep\": [{}], \"w_sus_mil\": [{}], \"ps_level\": [{}]}}, \
                  \"sum\": {{\"corner_ias\": {}, \"corner_tas\": {}, \"max_inst\": {}, \"max_sus\": {}, \
                  \"vmax_wep_ias\": {}, \"vmax_wep_tas\": {}, \"vmax_mil_tas\": {}}}}}",
                json_num(0, a.alt),
                json_num(5, a.ias_per_tas),
                fmt_vec(&a.g_v_ias, 1),
                fmt_vec(&a.g_w, 1),
                fmt_vec(&a.g_ps_wep, 1),
                fmt_vec(&a.g_ps_mil, 1),
                fmt_vec(&a.c_v_ias, 1),
                fmt_vec(&a.c_n_av, 3),
                fmt_vec(&a.c_n_sus, 3),
                fmt_vec(&a.c_w_inst, 2),
                fmt_vec(&a.c_w_sus_wep, 2),
                fmt_vec(&a.c_w_sus_mil, 2),
                fmt_vec(&a.c_ps_level, 2),
                json_num(1, a.corner_ias),
                json_num(1, a.corner_tas),
                json_num(2, a.max_inst),
                json_num(2, a.max_sus),
                json_num(1, a.vmax_wep_ias),
                json_num(1, a.vmax_wep_tas),
                json_num(1, a.vmax_mil_tas),
            ));
        }
        format!(
            "  \"em\": {{\"weight_kg\": {}, \"fuel_pct\": {}, \"vne_ias\": {}, \"alts\": [\n{}\n  ]}},",
            json_num(1, weight_kg),
            json_num(1, fuel_pct),
            json_num(1, data.vne()),
            rows.join(",\n")
        )
    } else {
        "  \"em\": null,".to_string()
    };

    let mut thrust_block = String::new();
    for &alt in &alts {
        let mil: Vec<f64> = speeds_kmh
            .iter()
            .map(|&v| {
                if data.is_jet {
                    wp8f_flightmodel::thrusts_at(&engines, v, alt, 100.0)
                } else {
                    wp8f_flightmodel::powers_at(&engines, v, alt, 100.0)
                }
            })
            .collect();
        let wep: Vec<f64> = speeds_kmh
            .iter()
            .map(|&v| {
                if data.is_jet {
                    wp8f_flightmodel::thrusts_at(&engines, v, alt, 110.0)
                } else {
                    wp8f_flightmodel::powers_at(&engines, v, alt, 110.0)
                }
            })
            .collect();
        thrust_block.push_str(&format!(
            "    \"{:.0}\": {{\"mil\": [{}], \"wep\": [{}]}}\n",
            alt,
            fmt_vec(&mil, 2),
            fmt_vec(&wep, 2)
        ));
        if alt != *alts.last().unwrap() {
            thrust_block.push_str(",\n");
        }
    }

    println!("{{");
    println!("  \"aircraft\": \"{}\",", json_str(aircraft));
    println!("  \"is_jet\": {},", data.is_jet);
    println!("  \"unit\": \"{}\",", unit);
    println!("  \"aoa\": [{}],", fmt_vec(&aoa, 1));
    println!("  \"cl_clean\": [{}],", fmt_vec(&cl_clean, 4));
    println!("  \"cl_full\": [{}],", fmt_vec(&cl_full, 4));
    println!("  \"speed_kmh\": [{}],", fmt_vec(&speeds_kmh, 1));
    println!("{}", power_line);
    println!("{}", thrust3d_block);
    println!("{}", tas_alt_line);
    println!("{}", limits_line);
    println!("{}", em_block);
    println!("  \"thrust\": {{");
    println!("{}", thrust_block);
    println!("  }}");
    println!("}}");
}

fn print_json(aircraft: &str, data: &FlightModel, cli: &Cli) {
    // 计算重量：W = takeoff_weight(fuel_pct) + extra_weight（CLI 可配；
    // 默认 50% 燃油、0 额外 = 既有口径，输出逐字节不变）
    let fuel_pct = if cli.fuel_pct.is_finite() { cli.fuel_pct.clamp(0.0, 100.0) } else { 50.0 };
    let extra = if cli.extra_weight.is_finite() && cli.extra_weight > 0.0 {
        cli.extra_weight
    } else {
        0.0
    };
    let weight_now = data.takeoff_weight(fuel_pct) + extra;
    let weight_50 = data.takeoff_weight(50.0);
    // 允许过载按当前重量 W 计算
    let (neg_lim, pos_lim) = data.allowed_load_factor(weight_now);
    println!("{{");
    println!("  \"aircraft\": \"{}\",", json_str(aircraft));
    println!("  \"engine_type\": \"{}\",", json_str(&data.engine_type_string()));
    println!("  \"is_jet\": {},", data.is_jet);
    println!("  \"has_wep\": {},", data.has_wep());
    println!("  \"empty_weight\": {},", json_num(0, data.empty_weight()));
    println!("  \"empty_flight_weight\": {},", json_num(0, data.flight_weight(0.0)));
    println!("  \"max_fuel\": {},", json_num(0, data.max_fuel()));
    println!("  \"takeoff_weight_50\": {},", json_num(0, weight_50));
    println!("  \"takeoff_weight\": {},", json_num(0, weight_now));
    println!("  \"fuel_pct\": {},", json_num(1, fuel_pct));
    println!("  \"extra_weight\": {},", json_num(1, extra));
    println!("  \"vne\": {},", json_num(0, data.vne()));
    println!("  \"vne_mach\": {},", json_num(2, data.vne_mach()));
    println!("  \"wingspan\": {},", json_num(1, data.wingspan()));
    println!("  \"wing_area\": {},", json_num(2, data.wing_area()));
    println!("  \"aspect_ratio\": {},", json_num(2, data.aspect_ratio()));
    println!("  \"thrust_max\": {},", json_num(0, data.thrust_max()));
    println!("  \"engine_power\": {},", json_num(0, data.engine_power()));
    println!("  \"aoa_crit_no_flaps\": {},", json_num(1, data.aoa_cl_max_no_flaps()));
    println!("  \"aoa_crit_full_flaps\": {},", json_num(1, data.aoa_cl_max_full_flaps()));
    println!("  \"load_limit_neg\": {},", json_num(2, neg_lim));
    println!("  \"load_limit_pos\": {},", json_num(2, pos_lim));
    // —— 以下按 wp8f 控制台输出结构组织 ——
    println!("  \"weight\": {{\"empty\": {}, \"oil\": {}, \"nitro\": {}, \"pilot\": {}, \"ammo\": {}, \"cm\": {}, \"external\": {}, \"empty_flight\": {}}},",
        json_num(0, data.empty_weight()), json_num(0, data.oil_mass()), json_num(0, data.max_nitro()), json_num(0, data.pilot_mass()),
        json_num(0, data.cannon_ammo_weight()), json_num(0, data.countermeasure_weight()), json_num(0, data.external_weapons_weight()),
        json_num(0, data.flight_weight(0.0)));
    let p_mil = if data.is_jet { 0.0 } else { powers_at(&data.engines(), 0.0, 0.0, 100.0) };
    let p_wep = if data.is_jet { 0.0 } else { powers_at(&data.engines(), 0.0, 0.0, 110.0) };
    println!("  \"power\": {{\"mil\": {}, \"wep\": {}, \"tw_mil\": {}}},",
        json_num(0, p_mil), json_num(0, p_wep),
        json_num(2, if data.is_jet { data.thrust_max() / data.empty_weight().max(1.0) } else { p_mil / data.empty_weight().max(1.0) }));
    let (ne, pe) = data.limit_load_factor_range(data.empty_weight());
    let (nf, pf) = data.limit_load_factor_range(data.flight_weight(data.max_fuel()));
    let (nc, pc) = data.limit_load_factor_range(weight_now);
    println!(
        "  \"load_factor\": {{\"empty\": [{}, {}], \"full\": [{}, {}], \"current\": [{}, {}]}},",
        json_num(2, ne), json_num(2, pe), json_num(2, nf), json_num(2, pf),
        json_num(2, nc), json_num(2, pc)
    );
    println!("  \"limits\": {{\"vne\": {}, \"vne_mach\": {}, \"ias_warn\": {}, \"mach_warn\": {}}},",
        json_num(0, data.vne()), json_num(2, data.vne_mach()),
        json_num(0, data.vne() * 0.95), json_num(3, data.vne_mach() * 0.95));
    // 部件极曲线（机翼/机身/平尾）
    fn polar_json(p: &wp8f_flightmodel::aero::PolarData, ar: f64) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "{{\"cd_min\": {}, \"cl0\": {}, \"aoa_crit\": [{}, {}], \"cl_crit\": [{}, {}], \"cl_slope\": {}, \"oswalds\": {}",
            json_num(4, p.cd_min),
            json_num(4, p.cl0),
            json_num(1, p.alpha_crit_low),
            json_num(1, p.alpha_crit_high),
            json_num(3, p.cl_crit_low),
            json_num(3, p.cl_crit_high),
            json_num(4, p.cl_slope()),
            json_num(3, p.oswalds_efficiency)
        ));
        if let Some((alpha, cl, cd, ld)) = p.best_ld(ar) {
            s.push_str(&format!(
                ", \"best_ld\": [{}, {}, {}, {}]",
                json_num(1, alpha),
                json_num(2, cl),
                json_num(3, cd),
                json_num(1, ld)
            ));
        }
        let (cn, cdn) = p.cl_cd_at(p.alpha_crit_low, ar);
        let (cp, cdp) = p.cl_cd_at(p.alpha_crit_high, ar);
        s.push_str(&format!(
            ", \"crit_neg\": [{}, {}, {}, {}], \"crit_pos\": [{}, {}, {}, {}]}}",
            json_num(1, p.alpha_crit_low),
            json_num(2, cn),
            json_num(3, cdn),
            json_num(2, if cdn > 0.0 { cn / cdn } else { 0.0 }),
            json_num(1, p.alpha_crit_high),
            json_num(2, cp),
            json_num(3, cdp),
            json_num(2, if cdp > 0.0 { cp / cdp } else { 0.0 })
        ));
        s
    }
    fn part_json(part: &wp8f_flightmodel::aero::AeroSurface) -> String {
        let ar = if part.area > 0.0 { part.span * part.span / part.area } else { 0.0 };
        let mut s = format!(
            "{{\"angle\": {}, \"area\": {}, \"ar\": {}, \"base\": {}",
            json_num(1, part.angle),
            json_num(2, part.area),
            json_num(2, ar),
            polar_json(&part.polar, ar)
        );
        if let Some(ref p) = part.flaps_polar_1 {
            s.push_str(&format!(", \"flaps\": {}", polar_json(p, ar)));
        }
        s.push('}');
        s
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(ref w) = data.wing { parts.push(format!("\"wing\": {}", part_json(w))); }
    if let Some(ref f) = data.fuselage { parts.push(format!("\"fuselage\": {}", part_json(f))); }
    if let Some(ref h) = data.hor_stab { parts.push(format!("\"hor_stab\": {}", part_json(h))); }
    println!("  \"parts\": {{{}}}", parts.join(", "));
    println!("}}");
}

fn print_tsv(aircraft: &str, data: &FlightModel) {
    println!("aircraft\t{}\tengine_type\t{}\tis_jet\t{}\tempty_weight\t{:.0}\tmax_fuel\t{:.0}\tvne\t{:.0}\twingspan\t{:.1}\twing_area\t{:.2}\taspect_ratio\t{:.2}\tthrust_max\t{:.0}\tengine_power\t{:.0}",
        aircraft,
        data.engine_type_string(),
        data.is_jet,
        data.empty_weight(),
        data.max_fuel(),
        data.vne(),
        data.wingspan(),
        data.wing_area(),
        data.aspect_ratio(),
        data.thrust_max(),
        data.engine_power()
    );
}

fn generate_piston_power_plot(aircraft: &str, data: &FlightModel, cli: &Cli) {
    println!("Generating power-altitude curve for {}...", aircraft);

    let has_wep = data.has_wep();
    println!("WEP available: {}", has_wep);

    let mut content = String::new();

    if has_wep {
        content.push_str("altitude_m\tpower_hp\twep_hp\n");
    } else {
        content.push_str("altitude_m\tpower_hp\n");
    }

    let engines = data.engines();
    for alt in (0..20001).step_by(500) {
        let power = powers_at(&engines, 0.0, alt as f64, 100.0);
        if has_wep {
            let wep = powers_at(&engines, 0.0, alt as f64, 110.0);
            content.push_str(&format!("{}\t{:.0}\t{:.0}\n", alt, power, wep));
        } else {
            content.push_str(&format!("{}\t{:.0}\n", alt, power));
        }
    }

    let output = cli
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("{}_power_alt.txt", aircraft)));

    if let Err(e) = std::fs::write(&output, &content) {
        eprintln!("Error writing output: {}", e);
    } else {
        println!("Power-altitude data written to: {}", output.display());
        if has_wep {
            println!("(Includes WEP curve)");
        }
        println!("\nTo generate a chart, run:");
        println!(
            "python3 scripts/plot_flight_model.py --input {}",
            output.display()
        );
    }
}

fn generate_jet_thrust_plot(aircraft: &str, data: &FlightModel, cli: &Cli) {
    println!(
        "Generating thrust-altitude-velocity curve for {}...",
        aircraft
    );

    let engines = data.engines();
    let jet_engines: Vec<_> = engines.iter().filter(|e| e.engine_type() == EngineType::Jet).collect();
    if jet_engines.is_empty() {
        eprintln!("No jet engine data available for {}", aircraft);
        return;
    }

    let engine = jet_engines[0];
    let altitudes = &engine.altitudes;
    let velocities = &engine.velocities;
    let has_wep = data.has_wep();

    if has_wep {
        println!("Afterburner (WEP) available: true");
    }

    let mut content = String::new();

    content.push_str("velocity\t");

    for alt in altitudes {
        content.push_str(&format!("{:.0}m\t", alt));
    }
    if has_wep {
        for alt in altitudes {
            content.push_str(&format!("{:.0}m_wep\t", alt));
        }
    }
    content.push('\n');

    for vel in velocities {
        content.push_str(&format!("{:.0}\t", vel));

        for alt in altitudes {
            let thrust = engine.get_thrust_at(*vel, *alt, 100.0);
            content.push_str(&format!("{:.0}\t", thrust));
        }

        if has_wep {
            for alt in altitudes {
                let thrust = engine.get_thrust_at(*vel, *alt, 110.0);
                content.push_str(&format!("{:.0}\t", thrust));
            }
        }
        content.push('\n');
    }

    let output = cli
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("{}_thrust.txt", aircraft)));

    if let Err(e) = std::fs::write(&output, &content) {
        eprintln!("Error writing output: {}", e);
    } else {
        println!(
            "Thrust-altitude-velocity data written to: {}",
            output.display()
        );
        if has_wep {
            println!("(Includes WEP/Afterburner curve)");
        }
        println!("\nTo generate a chart, run:");
        println!(
            "python3 scripts/plot_flight_model.py --input {}",
            output.display()
        );
    }
}

/// EM（能量机动）采样高度（与 `--em` 的 TSV、matplotlib 脚本一致）。
const EM_ALTS: [f64; 5] = [0.0, 3000.0, 6000.0, 9000.0, 12000.0];

/// 一个高度的 EM 数据。**计算只有这一份**：`--em` 的 TSV 出口与 GUI 的 JSON 出口共用，
/// 保证"控制台里画的 EM 图"与"matplotlib 脚本吃进去的数据"完全同源。
struct EmAlt {
    alt: f64,
    /// `IAS = TAS·√(ρ/ρ0)` 的比值（同一高度是常数；等过载线的横轴换算用它）
    ias_per_tas: f64,
    // —— Ps 网格（按 (V_tas, ω) 采样：每个速度一行 ω=0..33，横坐标输出表速）——
    g_v_ias: Vec<f64>,
    g_v_tas: Vec<f64>,
    g_w: Vec<f64>,
    g_ps_wep: Vec<f64>,
    g_ps_mil: Vec<f64>,
    // —— 包线曲线（每个速度一个点）——
    c_v_ias: Vec<f64>,
    c_v_tas: Vec<f64>,
    c_n_av: Vec<f64>,
    c_n_sus: Vec<f64>,
    c_w_inst: Vec<f64>,
    c_w_sus_wep: Vec<f64>,
    c_w_sus_mil: Vec<f64>,
    c_ps_level: Vec<f64>,
    // —— 汇总 ——
    corner_ias: f64,
    corner_tas: f64,
    max_inst: f64,
    max_sus: f64,
    vmax_wep_ias: f64,
    vmax_wep_tas: f64,
    vmax_mil_tas: f64,
}

/// 表速换算：`IAS = TAS·√(ρ/ρ0)`（Dagor 大气）—— 全文件唯一实现（EM 计算与 TSV 出口共用）。
fn ias_from_tas(alt: f64, tas_kmh: f64) -> f64 {
    let ratio =
        (wp8f_flightmodel::calculate_density(alt) / wp8f_flightmodel::aero::DAGOR_RO0).sqrt();
    tas_kmh * ratio
}

/// 算一份完整的 EM 数据（喷气机）。`weight_n` = 计算重量（牛顿）。
fn em_compute(data: &FlightModel, weight_n: f64) -> Vec<EmAlt> {
    let mut out = Vec::with_capacity(EM_ALTS.len());
    for &alt in &EM_ALTS {
        let mut a = EmAlt {
            alt,
            ias_per_tas: 1.0,
            g_v_ias: Vec::new(),
            g_v_tas: Vec::new(),
            g_w: Vec::new(),
            g_ps_wep: Vec::new(),
            g_ps_mil: Vec::new(),
            c_v_ias: Vec::new(),
            c_v_tas: Vec::new(),
            c_n_av: Vec::new(),
            c_n_sus: Vec::new(),
            c_w_inst: Vec::new(),
            c_w_sus_wep: Vec::new(),
            c_w_sus_mil: Vec::new(),
            c_ps_level: Vec::new(),
            corner_ias: 0.0,
            corner_tas: 0.0,
            max_inst: 0.0,
            max_sus: 0.0,
            vmax_wep_ias: 0.0,
            vmax_wep_tas: 0.0,
            vmax_mil_tas: 0.0,
        };
        // Ps 网格：ω → n = √((ωV/g)²+1)；WEP / 军推两列
        let mut v_ms: f64 = 100.0;
        while v_ms <= 800.0 {
            let v_ias = ias_from_tas(alt, v_ms * 3.6);
            if a.g_v_tas.is_empty() {
                a.ias_per_tas = if v_ms > 0.0 { v_ias / (v_ms * 3.6) } else { 1.0 };
            }
            let mut w: f64 = 0.0;
            while w <= 33.0 {
                let n = wp8f_flightmodel::em::load_factor_for_turn(w.to_radians(), v_ms);
                a.g_v_ias.push(v_ias);
                a.g_v_tas.push(v_ms * 3.6);
                a.g_w.push(w);
                a.g_ps_wep.push(data.ps_at_load(alt, v_ms, weight_n, n, true));
                a.g_ps_mil.push(data.ps_at_load(alt, v_ms, weight_n, n, false));
                w += 1.0;
            }
            v_ms += 10.0;
        }
        // 包线曲线：瞬时/持续盘旋率、可用/持续过载、平飞 Ps
        let mut v_ms: f64 = 100.0;
        while v_ms <= 800.0 {
            let (w_inst, n_av) = data.instant_turn(alt, v_ms, weight_n);
            let (w_sus_wep, n_sus) = data.sustained_turn(alt, v_ms, weight_n, true);
            let (w_sus_mil, _) = data.sustained_turn(alt, v_ms, weight_n, false);
            a.c_v_ias.push(ias_from_tas(alt, v_ms * 3.6));
            a.c_v_tas.push(v_ms * 3.6);
            a.c_n_av.push(n_av);
            a.c_n_sus.push(n_sus);
            a.c_w_inst.push(w_inst);
            a.c_w_sus_wep.push(w_sus_wep);
            a.c_w_sus_mil.push(w_sus_mil);
            a.c_ps_level.push(data.ps_at_load(alt, v_ms, weight_n, 1.0, true));
            a.max_inst = a.max_inst.max(w_inst);
            a.max_sus = a.max_sus.max(w_sus_wep);
            v_ms += 10.0;
        }
        let vc = data.corner_speed(alt, weight_n);
        a.corner_ias = ias_from_tas(alt, vc * 3.6);
        a.corner_tas = vc * 3.6;
        let vmax_wep = data.max_level_flight_speed(alt, weight_n, true);
        let vmax_mil = data.max_level_flight_speed(alt, weight_n, false);
        a.vmax_wep_ias = ias_from_tas(alt, vmax_wep * 3.6);
        a.vmax_wep_tas = vmax_wep * 3.6;
        a.vmax_mil_tas = vmax_mil * 3.6;
        out.push(a);
    }
    out
}


fn generate_em_curves(aircraft: &str, data: &FlightModel, cli: &Cli) {
    if !data.is_jet {
        println!("EM curves currently only supported for jet aircraft");
        return;
    }

    println!("Generating EM curves for {}...", aircraft);

    // 计算参数：50% 燃油重量（与推力图口径一致）
    let fuel_percent = 50.0;
    let weight_kg = data.takeoff_weight(fuel_percent);
    let weight_n = weight_kg * GRAVITY;
    let vne_ias_kmh = data.vne(); // FM 的 VNE 为表速 (km/h)
    // EM 数据只有这一份计算（GUI 的 JSON 出口也用它），见 em_compute
    let em = em_compute(data, weight_n);

    // ---- 1) Ps 网格：Ps = f(高度, 表速, 盘旋率)，WEP / 军推 两列 ----
    let mut grid = String::new();
    grid.push_str(&format!(
        "# EM grid: Ps [m/s] = f(alt, IAS, turn rate); WEP/mil thrust; weight={:.0}kg; fuel={:.0}%\n",
        weight_kg, fuel_percent
    ));
    grid.push_str("alt_m\tV_ias_kmh\tV_tas_kmh\tomega_dps\tPs_wep_mps\tPs_mil_mps\n");
    for a in &em {
        for i in 0..a.g_v_ias.len() {
            grid.push_str(&format!(
                "{:.0}\t{:.1}\t{:.1}\t{:.1}\t{:.2}\t{:.2}\n",
                a.alt, a.g_v_ias[i], a.g_v_tas[i], a.g_w[i], a.g_ps_wep[i], a.g_ps_mil[i]
            ));
        }
    }

    // ---- 2) 包线曲线：瞬时/持续盘旋率、可用/持续过载、平飞 Ps ----
    let mut curves = String::new();
    curves.push_str(&format!(
        "# EM curves: weight={:.0}kg; V_ias=表速, V_tas=真速; n_av=瞬时可用过载, n_sus=持续盘旋过载\n",
        weight_kg
    ));
    curves.push_str("alt_m\tV_ias_kmh\tV_tas_kmh\tn_av\tn_sus_wep\tomega_inst_dps\tomega_sus_wep_dps\tomega_sus_mil_dps\tPs_level_wep_mps\n");
    let mut summary = String::new();
    summary.push_str("alt_m\tcorner_ias_kmh\tcorner_tas_kmh\tmax_instant_dps\tmax_sus_wep_dps\tmax_level_ias_kmh_wep\tmax_level_tas_kmh_wep\tmax_level_tas_kmh_mil\tvne_ias_kmh\n");
    for a in &em {
        for i in 0..a.c_v_ias.len() {
            curves.push_str(&format!(
                "{:.0}\t{:.1}\t{:.1}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.2}\n",
                a.alt, a.c_v_ias[i], a.c_v_tas[i], a.c_n_av[i], a.c_n_sus[i],
                a.c_w_inst[i], a.c_w_sus_wep[i], a.c_w_sus_mil[i], a.c_ps_level[i]
            ));
        }
        summary.push_str(&format!(
            "{:.0}\t{:.1}\t{:.1}\t{:.3}\t{:.3}\t{:.1}\t{:.1}\t{:.1}\t{:.0}\n",
            a.alt, a.corner_ias, a.corner_tas, a.max_inst, a.max_sus,
            a.vmax_wep_ias, a.vmax_wep_tas, a.vmax_mil_tas, vne_ias_kmh
        ));
    }

    // ---- 3) 推力-速度-高度数据（WEP / 军推，双速度口径，1 m/s 细粒度）----
    let mut thrust = String::new();
    thrust.push_str("# thrust: kgf = f(alt, TAS/IAS); WEP=throttle 110%, mil=100%; 1 m/s steps\n");
    thrust.push_str("alt_m\tV_tas_kmh\tV_ias_kmh\tthrust_wep_kgf\tthrust_mil_kgf\n");
    for &alt in &EM_ALTS {
        let mut v: f64 = 0.0;
        while v <= 800.0 {
            let wep = wp8f_flightmodel::thrusts_at(data.engines(), v * 3.6, alt, 110.0);
            let mil = wp8f_flightmodel::thrusts_at(data.engines(), v * 3.6, alt, 100.0);
            thrust.push_str(&format!(
                "{:.0}\t{:.1}\t{:.1}\t{:.2}\t{:.2}\n",
                alt,
                v * 3.6,
                ias_from_tas(alt, v * 3.6),
                wep,
                mil
            ));
            v += 1.0;
        }
    }

    // ---- 4) 真空速-高度数据：100m 采样；VNE/MNE 各自换算成真空速限制；
    //         附最大平飞点的阻力（= 该点推力，kgf）----
    let mut tas_alt = String::new();
    tas_alt.push_str("# max level TAS vs altitude (100m steps); VNE & MNE converted to TAS; drag at max-level speed (kgf)\n");
    tas_alt.push_str("alt_m\ttas_wep_kmh\ttas_mil_kmh\tvne_tas_kmh\tmne_tas_kmh\tdrag_wep_kgf\tdrag_mil_kgf\n");
    for i in 0..=120u32 {
        let alt = (i * 100) as f64;
        let vmax_wep_ms = data.max_level_flight_speed(alt, weight_n, true);
        let vmax_mil_ms = data.max_level_flight_speed(alt, weight_n, false);
        let ratio = (wp8f_flightmodel::calculate_density(alt) / wp8f_flightmodel::aero::DAGOR_RO0).sqrt();
        let vne_tas = vne_ias_kmh / ratio;
        let mne_tas = data.vne_mach() * wp8f_flightmodel::calculate_sound_speed(alt) * 3.6;
        let drag_wep = if vmax_wep_ms > 0.0 {
            data.drag_level_flight(alt, vmax_wep_ms, weight_n) / GRAVITY
        } else {
            0.0
        };
        let drag_mil = if vmax_mil_ms > 0.0 {
            data.drag_level_flight(alt, vmax_mil_ms, weight_n) / GRAVITY
        } else {
            0.0
        };
        tas_alt.push_str(&format!(
            "{:.0}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\n",
            alt,
            vmax_wep_ms * 3.6,
            vmax_mil_ms * 3.6,
            vne_tas,
            mne_tas,
            drag_wep,
            drag_mil
        ));
    }

    // ---- 5) 速度限制数据：VNE/MNE 每高度换算（TAS/IAS 双口径，推力图裁剪与边界用）----
    let mut limits = String::new();
    limits.push_str("# speed limits per altitude (km/h); VNE=IAS limit converted; MNE=Mach limit converted\n");
    limits.push_str("alt_m\tvne_tas_kmh\tvne_ias_kmh\tmach_tas_kmh\tmach_ias_kmh\n");
    for &alt in &EM_ALTS {
        let ratio =
            (wp8f_flightmodel::calculate_density(alt) / wp8f_flightmodel::aero::DAGOR_RO0).sqrt();
        let vne_tas = vne_ias_kmh / ratio;
        let m_tas = data.vne_mach() * wp8f_flightmodel::calculate_sound_speed(alt) * 3.6;
        let m_ias = m_tas * ratio;
        limits.push_str(&format!(
            "{:.0}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\n",
            alt, vne_tas, vne_ias_kmh, m_tas, m_ias
        ));
    }

    let output = cli
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("{}_em.txt", aircraft)));
    let curves_path = PathBuf::from(
        output.to_string_lossy().replace(".txt", "_curves.txt"),
    );
    let summary_path = PathBuf::from(
        output.to_string_lossy().replace(".txt", "_summary.txt"),
    );
    let thrust_path = PathBuf::from(
        output.to_string_lossy().replace(".txt", "_thrust.txt"),
    );
    let tas_alt_path = PathBuf::from(
        output.to_string_lossy().replace(".txt", "_tas_alt.txt"),
    );
    let limits_path = PathBuf::from(
        output.to_string_lossy().replace(".txt", "_limits.txt"),
    );

    for (path, content) in [
        (&output, &grid),
        (&curves_path, &curves),
        (&summary_path, &summary),
        (&thrust_path, &thrust),
        (&tas_alt_path, &tas_alt),
        (&limits_path, &limits),
    ] {
        if let Err(e) = std::fs::write(path, content) {
            eprintln!("Error writing {}: {}", path.display(), e);
        } else {
            println!("EM data written to: {}", path.display());
        }
    }
}

fn generate_drag_breakdown(aircraft: &str, data: &FlightModel, _cli: &Cli) {
    println!("=== {} 阻力分解 ===\n", aircraft);

    println!("阻力参数:");
    println!("  阻力面积 CdS: {:.3}", data.drag_area());
    println!("  诱导阻力因子 k: {:.4}", data.induced_drag_factor());
    println!("  Cd_min (wing): {:.4}", data.cd_min());
    println!("  Cd_min (fuselage): {:.4}", data.fuselage_cd_min());
    println!("  散热器 Cd: {:.4}", data.radiator_cd());
    println!("  油冷器 Cd: {:.4}", data.oil_radiator_cd());
    println!("  减速板 Cd: {:.4}", data.airbrake_cd());
    println!();

    let weight_50 = data.takeoff_weight(50.0) * GRAVITY;
    let weight_100 = data.takeoff_weight(100.0) * GRAVITY;

    println!(
        "--- 海平面, 满油 ({:.0} kg) ---",
        data.takeoff_weight(100.0)
    );
    let bd = data.drag_breakdown(0.0, 200.0, weight_100);
    bd.print();
    println!();

    println!("--- 6000m, 50% 燃油 ---");
    let bd = data.drag_breakdown(6000.0, 250.0, weight_50);
    bd.print();

    println!("--- 速度扫描 (0m, 50% 燃油) ---");
    println!("速度(m/s)\t速度(km/h)\t推力WEP(N)\t总阻力(N)\t净推力(N)\t升力系数\t马赫数");
    let weight_50_n = weight_50;
    for vel_ms in [
        100.0, 150.0, 200.0, 250.0, 300.0, 350.0, 400.0, 450.0, 500.0, 550.0, 600.0, 650.0, 700.0,
    ] {
        let bd = data.drag_breakdown(0.0, vel_ms, weight_50_n);
        let thrust = data.afterburner_thrust(0.0, vel_ms);
        let net = thrust - bd.total_drag;

        println!(
            "{:.0}\t\t{:.0}\t\t{:.0}\t\t{:.0}\t\t{:.0}\t\t{:.3}\t\t{:.3}",
            vel_ms,
            vel_ms * 3.6,
            thrust,
            bd.total_drag,
            net,
            bd.lift_coeff,
            bd.mach
        );
    }

    println!();
    println!("--- 速度扫描 (12000m, 50% 燃油) ---");
    println!("速度(m/s)\t速度(km/h)\t推力(N)\t总阻力(N)\t净推力(N)\t升力系数\t马赫数\tcd");
    let weight_50_n = weight_50;
    for vel_ms in [
        300.0, 400.0, 500.0, 600.0, 700.0, 800.0, 900.0, 1000.0, 1100.0, 1200.0,
    ] {
        let bd = data.drag_breakdown(12000.0, vel_ms, weight_50_n);
        let thrust = data.thrust(12000.0, vel_ms);
        let net = thrust - bd.total_drag;

        let rho = data.air_density(12000.0);
        let q = 0.5 * rho * vel_ms * vel_ms;
        let cd = if q > 0.0 && data.reference_area > 0.0 {
            bd.total_drag / (q * data.reference_area)
        } else {
            0.0
        };

        println!(
            "{:.0}\t\t{:.0}\t\t{:.0}\t\t{:.0}\t\t{:.0}\t\t{:.3}\t\t{:.3}\t{:.4}",
            vel_ms,
            vel_ms * 3.6,
            thrust,
            bd.total_drag,
            net,
            bd.lift_coeff,
            bd.mach,
            cd
        );
    }

    println!();
    println!("--- 速度扫描 (10000m, 50% 燃油) ---");
    println!("速度(m/s)\t速度(km/h)\t推力(N)\t总阻力(N)\t净推力(N)\t升力系数\t马赫数");
    let weight_50_n = weight_50;
    for vel_ms in [
        200.0, 250.0, 300.0, 320.0, 340.0, 350.0, 360.0, 380.0, 400.0, 420.0, 450.0, 500.0,
    ] {
        let bd = data.drag_breakdown(10000.0, vel_ms, weight_50_n);
        let thrust = data.thrust(10000.0, vel_ms);
        let net = thrust - bd.total_drag;
        println!(
            "{:.0}\t\t{:.0}\t\t{:.0}\t\t{:.0}\t\t{:.0}\t\t{:.3}\t\t{:.3}",
            vel_ms,
            vel_ms * 3.6,
            thrust,
            bd.total_drag,
            net,
            bd.lift_coeff,
            bd.mach
        );
    }
}

fn generate_max_speed(aircraft: &str, data: &FlightModel, _cli: &Cli) {
    if !data.is_jet {
        println!("Max speed calculation currently only supported for jet aircraft");
        return;
    }

    println!("=== {} 最大平飞速度 ===\n", aircraft);

    let fuel_percent = 50.0;
    let weight_kg = data.takeoff_weight(fuel_percent);
    let weight_n = weight_kg * GRAVITY;

    let v = data.vne_limit();

    println!(
        "计算条件: {}% 燃油, 重量 {:.0} kg\n",
        fuel_percent, weight_kg
    );
    println!(
        "VNE: {:.0} km/h, 最大允许表速: {:.0} km/h",
        v.vne_kmh, v.vne_ias_allowed
    );
    if v.mne > 0.0 {
        println!("MNE: {:.3}, 最大允许马赫: {:.3}", v.mne, v.mne_allowed);
    }

    println!();

    println!("高度\tTAS\tIAS\t马赫\t军推\tWEP推力\t阻力\t升阻比\t过载");
    println!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        "-".repeat(6),
        "-".repeat(6),
        "-".repeat(6),
        "-".repeat(5),
        "-".repeat(6),
        "-".repeat(8),
        "-".repeat(6),
        "-".repeat(6),
        "-".repeat(4)
    );

    let altitudes: Vec<f64> = vec![
        0.0, 1000.0, 2000.0, 3000.0, 4000.0, 5000.0, 6000.0, 7000.0, 8000.0, 9000.0, 10000.0,
        11000.0, 12000.0, 13000.0, 14000.0, 15000.0,
    ];

    for alt in &altitudes {
        let alt_f = *alt;
        let speed_wep = data.max_level_flight_speed(alt_f, weight_n, true);
        let speed_mil = data.max_level_flight_speed(alt_f, weight_n, false);

        if speed_wep > 0.0 {
            let thrust_mil = data.thrust(alt_f, speed_mil);
            let thrust_wep = data.afterburner_thrust(alt_f, speed_wep);
            let drag = data.drag_level_flight(alt_f, speed_wep, weight_n);
            let _excess = thrust_wep - drag;

            // 计算空气密度和音速
            let rho = data.air_density(alt_f);
            let sound_speed = data.speed_of_sound(alt_f);

            // 真空速 (TAS) = m/s -> km/h
            let tas_kmh = speed_wep * 3.6;
            // 表速 (IAS) = TAS * sqrt(rho/rho0)，其中 rho0 = 1.225
            let ias_kmh = tas_kmh * (rho / 1.225).sqrt();
            // 马赫数
            let mach = if sound_speed > 0.0 {
                speed_wep / sound_speed
            } else {
                0.0
            };

            // 升阻比 (水平飞行时升力等于重量)
            let ld_ratio = if drag > 0.0 { weight_n / drag } else { 0.0 };
            // 过载 = 升力/重量 = 1.0 (水平飞行)
            let load_factor = 1.0;

            let line = format!(
                " {:4} {:4} {:4} {:.3} {:6} {:6} {:6} {:.2} {:.2}",
                *alt as i32,
                tas_kmh as i32,
                ias_kmh as i32,
                mach,
                thrust_mil as i32,
                thrust_wep as i32,
                drag as i32,
                ld_ratio,
                load_factor
            );
            println!("{}", line);
        } else {
            println!(
                " {:4} -    -    -    -     -     -     -     -",
                *alt as i32
            );
        }
    }

    println!("\n--- 详细推力/阻力对比 (WEP速度) ---\n");
    println!("高度\tTAS\tIAS\t马赫\t推力\t阻力\t升阻比\t最大过载");
    println!("{}", "-".repeat(75));

    for alt in &altitudes {
        let alt_f = *alt;
        let speed_wep = data.max_level_flight_speed(alt_f, weight_n, true);

        if speed_wep > 0.0 {
            let thrust_wep = data.afterburner_thrust(alt_f, speed_wep);
            let drag = data.drag_level_flight(alt_f, speed_wep, weight_n);

            let rho = data.air_density(alt_f);
            let sound_speed = data.speed_of_sound(alt_f);
            let tas_kmh = speed_wep * 3.6;
            let _ias_kmh = tas_kmh * (rho / 1.225).sqrt();
            let mach = if sound_speed > 0.0 {
                speed_wep / sound_speed
            } else {
                0.0
            };

            // 升阻比
            let ld_ratio = if drag > 0.0 { thrust_wep / drag } else { 0.0 };
            // 最大过载 (使用 cl_max)
            let max_load_factor = data.available_load_factor(alt_f, speed_wep, weight_n);

            let line = format!(
                " {:4} {:4} {:.3} {:6} {:6} {:.2} {:.2}",
                *alt as i32,
                tas_kmh as i32,
                mach,
                thrust_wep as i32,
                drag as i32,
                ld_ratio,
                max_load_factor
            );
            println!("{}", line);
        }
    }

    println!("\n--- 不同速度下的最大过载 (0m) ---\n");
    println!("速度\t速度(km/h)\t升力\t最大升力\t最大过载");
    println!("{}", "-".repeat(50));

    let alt_sea = 0.0;
    let vel_range: Vec<f64> = (50..=600).step_by(50).map(|v| v as f64).collect();
    for vel in vel_range {
        let lift = weight_n;
        let max_lift =
            0.5 * data.air_density(alt_sea) * vel * vel * data.wing_area() * data.cl_max_no_flaps();
        let max_load = max_lift / weight_n;

        let line = format!(
            "{:4} {:4} {:6} {:6} {:.2}",
            vel as i32,
            (vel * 3.6) as i32,
            lift as i32,
            max_lift as i32,
            max_load
        );
        println!("{}", line);
    }

    println!("\n注: 军推 = 100%油门(无加力), WEP = 加力推力");
    println!("    TAS = 真空速, IAS = 表速");
    println!("    升阻比 = 推力/阻力 (WEP速度下)");
    println!("    最大过载 = L_max / 重量，使用 Cl_max 计算");
}

fn generate_aero_test(aircraft: &str, data: &FlightModel, _cli: &Cli) {
    println!("=== {} 气动计算测试 ===\n", aircraft);

    if data.fm_data.is_empty() {
        println!("No FM data available");
        return;
    }

    let (wing, fuselage, hor_stab, reference_area) = read_aero_surfaces(&data.fm_data);

    print_aero_surface_info(&wing, &fuselage, &hor_stab);

    println!("\n=== 气动力计算测试 ===");
    println!("参考面积: {:.2} m²\n", reference_area);

    println!("高度(m)\t速度(km/h)\t攻角(°)\t升力(N)\t\t阻力(N)\t\tCl\t\tCd");
    println!("{}", "-".repeat(90));

    let altitudes = vec![0.0, 3000.0, 6000.0, 10000.0];
    let velocities = vec![150.0, 200.0, 250.0, 300.0];
    let alphas = vec![0.0, 5.0, 10.0, 15.0];

    for alt in altitudes {
        for vel in &velocities {
            for alpha in &alphas {
                let forces = calculate_aero_forces(
                    &wing,
                    &fuselage,
                    &hor_stab,
                    reference_area,
                    *vel,
                    alt,
                    *alpha,
                    0.0,
                    false,
                    false,
                );

                println!(
                    "{:.0}\t\t{:.0}\t\t{:.1}\t\t{:.0}\t\t{:.0}\t\t{:.4}\t\t{:.4}",
                    alt,
                    vel * 3.6,
                    alpha,
                    forces.lift,
                    forces.drag,
                    forces.cl,
                    forces.cd
                );
            }
        }
    }
}

fn generate_polar_curve(aircraft: &str, data: &FlightModel, _cli: &Cli) {
    println!("=== {} 海平面极曲线 (0m) ===\n", aircraft);

    if data.fm_data.is_empty() {
        println!("No FM data available");
        return;
    }

    let (wing, fuselage, hor_stab, reference_area) = read_aero_surfaces(&data.fm_data);

    println!("--- 极曲线参数 ---");
    println!("Wing polar:");
    println!("  Cl0: {:.4}", wing.polar.cl0);
    println!("  lineClCoeff: {:.4}", wing.polar.line_cl_coeff);
    println!("  alphaCritHigh: {:.2}°", wing.polar.alpha_crit_high);
    println!("  alphaCritLow: {:.2}°", wing.polar.alpha_crit_low);
    println!("  ClCritHigh: {:.2}", wing.polar.cl_crit_high);
    println!("  ClCritLow: {:.2}", wing.polar.cl_crit_low);
    println!(
        "  AfterCritParabAngle: {:.2}",
        wing.polar.after_crit_parab_angle
    );
    println!(
        "  AfterCritDeclineCoeff: {:.4}",
        wing.polar.after_crit_decline_coeff
    );
    println!("  ClAfterCritHigh: {:.2}", wing.polar.cl_after_crit_high);
    println!("  ClAfterCritLow: {:.2}", wing.polar.cl_after_crit_low);
    println!();

    println!("\n--- 固定速度下的攻角-升力-阻力 ---\n");
    println!("速度(m/s)\t速度(km/h)\t攻角(°)\t升力(N)\t\t阻力(N)\t\tCl\t\tCd\t\t马赫");
    println!("{}", "-".repeat(100));

    let velocities = vec![100.0, 150.0, 200.0, 250.0, 300.0, 350.0];
    let alphas: Vec<f64> = (-20..=30).map(|a| a as f64).collect();

    for vel in velocities {
        for alpha in &alphas {
            let forces = calculate_aero_forces(
                &wing,
                &fuselage,
                &hor_stab,
                reference_area,
                vel,
                0.0,
                *alpha,
                0.0,
                false,
                false,
            );

            let a = data.speed_of_sound(0.0);
            let mach = if a > 0.0 { vel / a } else { 0.0 };

            println!(
                "{:.0}\t\t{:.0}\t\t{:.1}\t\t{:.0}\t\t{:.0}\t\t{:.4}\t\t{:.4}\t\t{:.3}",
                vel,
                vel * 3.6,
                alpha,
                forces.lift,
                forces.drag,
                forces.cl,
                forces.cd,
                mach
            );
        }
        println!();
    }

    println!("\n--- 固定攻角下的速度-升力-阻力 ---\n");
    println!("攻角(°)\t速度(m/s)\t速度(km/h)\t升力(N)\t\t阻力(N)\t\tCl\t\tCd");
    println!("{}", "-".repeat(100));

    let fixed_alphas = vec![0.0, 5.0, 10.0, 15.0, 20.0];
    let speed_range: Vec<f64> = (50..=500).step_by(50).map(|v| v as f64).collect();

    for alpha in fixed_alphas {
        for vel in &speed_range {
            let forces = calculate_aero_forces(
                &wing,
                &fuselage,
                &hor_stab,
                reference_area,
                *vel,
                0.0,
                alpha,
                0.0,
                false,
                false,
            );

            println!(
                "{:.1}\t\t{:.0}\t\t{:.0}\t\t{:.0}\t\t{:.0}\t\t{:.4}\t\t{:.4}",
                alpha,
                vel,
                vel * 3.6,
                forces.lift,
                forces.drag,
                forces.cl,
                forces.cd
            );
        }
        println!();
    }
}
