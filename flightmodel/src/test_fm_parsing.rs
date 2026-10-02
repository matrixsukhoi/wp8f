#[cfg(test)]
mod tests {
    use crate::engine_model::{fuel_rate_at, has_any_wep, powers_at, print_compressor_stages, print_engine_modes, print_power_table, thrusts_at, EngineType};
    use crate::parse_aircraft;

    struct TestAircraft {
        name: &'static str,
        expected_engine_type: &'static str,
        min_vne: f64,
        min_wingspan: f64,
        min_wing_area: f64,
        min_empty_weight: f64,
        min_engine_count: usize,
    }

    fn print_flight_model_details(name: &str, fm: &crate::flight_model::FlightModel) {
        println!("\n=== {} ===", name);
        println!("Aerodynamics:");
        println!(
            "  wingspan: {:.1}m, wing_area: {:.2}m², aspect_ratio: {:.2}",
            fm.aerodynamics.wingspan,
            fm.aerodynamics.wing_area,
            fm.aerodynamics.aspect_ratio
        );
        println!(
            "  cl_max_no_flaps: {:.2}, cl_max_full_flaps: {:.2}",
            fm.aerodynamics.cl_max_no_flaps, fm.aerodynamics.cl_max_full_flaps
        );
        println!(
            "  vne: {:.0} km/h, vne_mach: {:.2}",
            fm.aerodynamics.wing_geometry.vne(), fm.aerodynamics.wing_geometry.vne_mach()
        );
        println!(
            "  cd_min: {:.4}, oswalds_efficiency: {:.2}",
            fm.aerodynamics.cd_min, fm.aerodynamics.oswalds_efficiency
        );
        println!("  has_sweep_data: {}", fm.aerodynamics.has_sweep_data());
        let speeds = fm.aerodynamics.control_surface_effective_speeds();
        println!(
            "  control_surface_effective_speeds: elevator={:.0}, aileron={:.0}, rudder={:.0} km/h",
            speeds[0], speeds[1], speeds[2]
        );

        println!("AeroSurface:");
        if let Some(ref wing) = fm.wing {
            println!(
                "  wing: span={:.1}m, area={:.2}m², taper={:.2}, sweep={:.1}°",
                wing.span, wing.area, wing.taper_ratio, wing.swept_angle
            );
            if let (Some(ref p0), Some(ref p1)) = (&wing.flaps_polar_0, &wing.flaps_polar_1) {
                println!(
                    "    no_flaps_polar: cl_max={:.2}, alpha_crit={:.1}°",
                    p0.cl_crit_high, p0.alpha_crit_high
                );
                println!(
                    "    full_flaps_polar: cl_max={:.2}, alpha_crit={:.1}°",
                    p1.cl_crit_high, p1.alpha_crit_high
                );
            } else {
                println!(
                    "    polar: cl0={:.1}, cl_max={:.2}, alpha_crit={:.1}, cd_min={:.2}",
                    wing.polar.cl0, wing.polar.cl_crit_high, wing.polar.alpha_crit_high, wing.polar.cd_min
                );
            }
        }
        if let Some(ref fuselage) = fm.fuselage {
            println!("  fuselage: span={:.1}m, area={:.2}m²", fuselage.span, fuselage.area);
            println!(
                "    polar: cl0={:.1}, cl_max={:.2}, alpha_crit={:.1}, cd_min={:.2}",
                fuselage.polar.cl0, fuselage.polar.cl_crit_high, fuselage.polar.alpha_crit_high, fuselage.polar.cd_min
            );
        }
        if let Some(ref hor_stab) = fm.hor_stab {
            println!("  hor_stab: span={:.1}m, area={:.2}m²", hor_stab.span, hor_stab.area);
            println!(
                "    polar: cl0={:.1}, cl_max={:.2}, alpha_crit={:.1}, cd_min={:.2}",
                hor_stab.polar.cl0, hor_stab.polar.cl_crit_high, hor_stab.polar.alpha_crit_high, hor_stab.polar.cd_min
            );
        }

        if fm.aerodynamics.has_sweep_data() {
            println!("Sweep Wing Data:");
            let wg = &fm.aerodynamics.wing_geometry;
            println!(
                "  Sweep0 (0%): cl_max={:.2}, alpha={:.1}°, vne={:.0} km/h",
                wg.wing_sweep0.cl_max_no_flaps(),
                wg.wing_sweep0.aoa_cl_max_no_flaps(),
                wg.wing_sweep0.vne
            );
            if let Some(ref sweep1) = wg.wing_sweep1 {
                println!(
                    "  Sweep1 (fraction {:.2}): cl_max={:.2}, alpha={:.1}°, vne={:.0} km/h",
                    sweep1.sweep_percent,
                    sweep1.cl_max_no_flaps(),
                    sweep1.aoa_cl_max_no_flaps(),
                    if sweep1.vne > 0.0 { sweep1.vne } else { wg.wing_sweep0.vne }
                );
            }
            if let Some(ref sweep2) = wg.wing_sweep2 {
                println!(
                    "  Sweep2 (fraction {:.2}): cl_max={:.2}, alpha={:.1}°, vne={:.0} km/h",
                    sweep2.sweep_percent,
                    sweep2.cl_max_no_flaps(),
                    sweep2.aoa_cl_max_no_flaps(),
                    if sweep2.vne > 0.0 { sweep2.vne } else { wg.wing_sweep0.vne }
                );
            }
            if let Some(ref sweep3) = wg.wing_sweep3 {
                println!(
                    "  Sweep3 (fraction {:.2}): cl_max={:.2}, alpha={:.1}°, vne={:.0} km/h",
                    sweep3.sweep_percent,
                    sweep3.cl_max_no_flaps(),
                    sweep3.aoa_cl_max_no_flaps(),
                    if sweep3.vne > 0.0 { sweep3.vne } else { wg.wing_sweep0.vne }
                );
            }
        }

        println!("Propulsion:");
        println!("  engine_count: {}", fm.propulsion.engine_count);
        if fm.engine_type_string() == "Piston" || fm.engine_type_string() == "Turboprop" {
            println!("  engine_type: {}, engine_power: {:.0} hp", fm.engine_type_string(), fm.propulsion.engine_power);
        } else if fm.engine_type_string() == "Jet" {
            println!("  engine_type: {}, total_thrust_max: {:.0} kgf", fm.engine_type_string(), fm.propulsion.total_thrust_max);
        } else {
            println!("  engine_type: {}", fm.engine_type_string());
        }
        println!("  has_wep: {}", fm.propulsion.has_wep);

        println!("Mass:");
        println!("  empty_weight: {:.0} kg, max_fuel: {:.0} kg", fm.mass.empty_weight, fm.mass.max_fuel);
        if fm.mass.nitro > 0.0 {
            println!("  nitro: {:.0} kg", fm.mass.nitro);
        }

        println!("Loadout:");
        if fm.loadout.cannon_weight > 0.0 || fm.loadout.ammo_weight > 0.0 {
            println!("  cannon_weight: {:.1} kg", fm.loadout.cannon_weight);
            println!("  ammo_weight: {:.1} kg", fm.loadout.ammo_weight);
        }
        if fm.loadout.countermeasures_weight > 0.0 {
            println!("  countermeasures_weight: {:.1} kg", fm.loadout.countermeasures_weight);
        }
        if fm.loadout.total_weapons_weight > 0.0 {
            println!("  total_weapons_weight: {:.1} kg", fm.loadout.total_weapons_weight);
        }
        if !fm.loadout.weapon_slots.is_empty() {
            println!("  weapon_slots:");
            for slot in &fm.loadout.weapon_slots {
                println!("    Slot {}: preset='{}' ({} weapons)", slot.index, slot.preset_name, slot.weapons.len());
                for weapon in &slot.weapons {
                    println!("      - trigger='{}', blk='{}', bullets={}, mass={:.1}kg, bullet_mass={:.2}kg", 
                        weapon.trigger, weapon.blk, weapon.bullets, weapon.mass, weapon.bullet_mass);
                }
            }
        }
    }

    struct AeroValidationParams {
        wingspan_min: f64,
        wingspan_max: f64,
        wing_area_min: f64,
        wing_area_max: f64,
        aspect_ratio_min: f64,
        aspect_ratio_max: f64,
        cl_max_no_flaps_min: f64,
        cl_max_no_flaps_max: f64,
        cl_max_full_flaps_min: f64,
        cl_max_full_flaps_max: f64,
        vne_min: f64,
        vne_max: f64,
        vne_mach_min: f64,
        vne_mach_max: f64,
        cd_min_min: f64,
        cd_min_max: f64,
        oswalds_efficiency_min: f64,
        oswalds_efficiency_max: f64,
        expect_sweep: bool,
    }

    impl Default for AeroValidationParams {
        fn default() -> Self {
            Self {
                wingspan_min: 5.0,
                wingspan_max: 50.0,
                wing_area_min: 5.0,
                wing_area_max: 200.0,
                aspect_ratio_min: 3.0,
                aspect_ratio_max: 20.0,
                cl_max_no_flaps_min: 0.5,
                cl_max_no_flaps_max: 3.0,
                cl_max_full_flaps_min: 0.5,
                cl_max_full_flaps_max: 3.5,
                vne_min: 200.0,
                vne_max: 3000.0,
                vne_mach_min: 0.5,
                vne_mach_max: 3.0,
                cd_min_min: 0.005,
                cd_min_max: 0.05,
                oswalds_efficiency_min: 0.5,
                oswalds_efficiency_max: 1.0,
                expect_sweep: false,
            }
        }
    }

    fn validate_aerodynamics(name: &str, fm: &crate::flight_model::FlightModel, params: AeroValidationParams) {
        let aero = &fm.aerodynamics;

        let validate = |value: f64, min: f64, max: f64, param_name: &str| {
            assert!(
                value >= min && value <= max,
                "{}: {} = {:.4} is outside expected range [{:.4}, {:.4}]. \
                 Possible root cause: incorrect parsing or data file issue.",
                name, param_name, value, min, max
            );
        };

        validate(aero.wingspan, params.wingspan_min, params.wingspan_max, "wingspan");
        validate(aero.wing_area, params.wing_area_min, params.wing_area_max, "wing_area");

        validate(
            aero.aspect_ratio,
            params.aspect_ratio_min * 0.8,
            params.aspect_ratio_max * 1.2,
            "aspect_ratio"
        );

        validate(aero.cl_max_no_flaps, params.cl_max_no_flaps_min, params.cl_max_no_flaps_max, "cl_max_no_flaps");
        validate(aero.cl_max_full_flaps, params.cl_max_full_flaps_min, params.cl_max_full_flaps_max, "cl_max_full_flaps");

        assert!(
            aero.cl_max_full_flaps >= aero.cl_max_no_flaps * 0.95,
            "{}: cl_max_full_flaps ({:.2}) should be >= cl_max_no_flaps ({:.2}) or very close. \
             Possible root cause: flap parsing issue.",
            name, aero.cl_max_full_flaps, aero.cl_max_no_flaps
        );

        validate(aero.wing_geometry.vne(), params.vne_min, params.vne_max, "vne");
        validate(aero.wing_geometry.vne_mach(), params.vne_mach_min, params.vne_mach_max, "vne_mach");

        validate(aero.cd_min, params.cd_min_min, params.cd_min_max, "cd_min");
        validate(aero.oswalds_efficiency, params.oswalds_efficiency_min, params.oswalds_efficiency_max, "oswalds_efficiency");

        if params.expect_sweep {
            assert!(
                aero.has_sweep_data(),
                "{}: has_sweep_data should be true for variable sweep wing aircraft. \
                 Possible root cause: sweep angle data not parsed correctly.",
                name
            );
        }
    }

    fn get_test_aircraft() -> Vec<TestAircraft> {
        vec![
            TestAircraft {
                name: "f_111f",
                expected_engine_type: "Jet",
                min_vne: 0.0,
                min_wingspan: 0.0,
                min_wing_area: 0.0,
                min_empty_weight: 10000.0,
                min_engine_count: 1,
            },
            TestAircraft {
                name: "f_16c_block_50",
                expected_engine_type: "Jet",
                min_vne: 0.0,
                min_wingspan: 0.0,
                min_wing_area: 0.0,
                min_empty_weight: 8000.0,
                min_engine_count: 1,
            },
            TestAircraft {
                name: "mig_23mld",
                expected_engine_type: "Jet",
                min_vne: 0.0,
                min_wingspan: 0.0,
                min_wing_area: 0.0,
                min_empty_weight: 9000.0,
                min_engine_count: 1,
            },
            TestAircraft {
                name: "yak-3",
                expected_engine_type: "Piston",
                min_vne: 0.0,
                min_wingspan: 0.0,
                min_wing_area: 0.0,
                min_empty_weight: 2000.0,
                min_engine_count: 1,
            },
            TestAircraft {
                name: "fa_18a",
                expected_engine_type: "Jet",
                min_vne: 0.0,
                min_wingspan: 0.0,
                min_wing_area: 0.0,
                min_empty_weight: 10000.0,
                min_engine_count: 2,
            },
            TestAircraft {
                name: "fw_190f_8_hungary",
                // 同 `test_additional_aircraft_parsing`：缺 Type 键 → 靠 Power 键兜底成 Piston
                expected_engine_type: "Piston",
                min_vne: 0.0,
                min_wingspan: 0.0,
                min_wing_area: 0.0,
                min_empty_weight: 3000.0,
                min_engine_count: 1,
            },
            TestAircraft {
                name: "wyvern_s4",
                expected_engine_type: "Turboprop",
                min_vne: 0.0,
                min_wingspan: 0.0,
                min_wing_area: 0.0,
                min_empty_weight: 4000.0,
                min_engine_count: 1,
            },
            TestAircraft {
                name: "f_15e",
                expected_engine_type: "Jet",
                min_vne: 1600.0,
                min_wingspan: 13.0,
                min_wing_area: 0.0,
                min_empty_weight: 15000.0,
                min_engine_count: 2,
            },
        ]
    }

    #[test]
    fn test_parse_multiple_aircraft() {
        let aircraft_list = get_test_aircraft();
        let data_dir = "../resource/data/gamedata/flightmodels";

        for test_ac in aircraft_list {
            let result = parse_aircraft(test_ac.name, data_dir);

            match result {
                Ok(fm) => {
                    print_flight_model_details(test_ac.name, &fm);

                    assert_eq!(
                        fm.engine_type_string(),
                        test_ac.expected_engine_type,
                        "{}: engine type mismatch",
                        test_ac.name
                    );

                    assert!(
                        fm.vne() > test_ac.min_vne,
                        "{}: VNE {} should be > {}",
                        test_ac.name,
                        fm.vne(),
                        test_ac.min_vne
                    );

                    assert!(
                        fm.wingspan() > test_ac.min_wingspan,
                        "{}: wingspan {} should be > {}",
                        test_ac.name,
                        fm.wingspan(),
                        test_ac.min_wingspan
                    );

                    assert!(
                        fm.wing_area() > test_ac.min_wing_area,
                        "{}: wing_area {} should be > {}",
                        test_ac.name,
                        fm.wing_area(),
                        test_ac.min_wing_area
                    );

                    assert!(
                        fm.empty_weight() > test_ac.min_empty_weight,
                        "{}: empty_weight {} should be > {}",
                        test_ac.name,
                        fm.empty_weight(),
                        test_ac.min_empty_weight
                    );

                    assert!(
                        fm.engine_count() >= test_ac.min_engine_count,
                        "{}: engine_count {} should be >= {}",
                        test_ac.name,
                        fm.engine_count(),
                        test_ac.min_engine_count
                    );

                    validate_aerodynamics(test_ac.name, &fm, AeroValidationParams::default());
                }
                Err(e) => {
                    panic!("{}: failed to parse: {}", test_ac.name, e);
                }
            }
        }
    }

    #[test]
    fn test_parse_nonexistent_aircraft() {
        let result = parse_aircraft("nonexistent_aircraft_xyz", "../resource/data/gamedata/flightmodels");
        assert!(result.is_err(), "Parsing nonexistent aircraft should fail");
    }

    #[test]
    fn test_parsed_values_are_reasonable() {
        let data_dir = "../resource/data/gamedata/flightmodels";

        let fm = parse_aircraft("yak-3", data_dir).expect("Failed to parse yak-3");
        print_flight_model_details("yak-3", &fm);

        assert_eq!(fm.engine_type_string(), "Piston", "yak-3 should be piston engine");
        assert_eq!(fm.is_jet, false, "yak-3 should not be a jet");
        assert!(fm.wingspan() > 5.0 && fm.wingspan() < 20.0, "yak-3 wingspan should be reasonable");
        assert!(fm.wing_area() > 5.0 && fm.wing_area() < 50.0, "yak-3 wing_area should be reasonable");
        assert!(fm.vne() > 300.0 && fm.vne() < 2000.0, "yak-3 VNE should be reasonable");
        assert!(fm.empty_weight() > 1000.0 && fm.empty_weight() < 10000.0, "yak-3 empty_weight should be reasonable");

        let ammo = fm.cannon_ammo_weight();
        println!("  ammo_weight: {:.6} kg", ammo);
        assert!(ammo > 0.0, "yak-3 ammo weight should be > 0, got {:.6}", ammo);

        let fm2 = parse_aircraft("f_16c_block_50", data_dir).expect("Failed to parse f_16c_block_50");
        print_flight_model_details("f_16c_block_50", &fm2);

        assert_eq!(fm2.engine_type_string(), "Jet", "f_16c_block_50 should be jet engine");
        assert_eq!(fm2.is_jet, true, "f_16c_block_50 should be a jet");
        assert!(fm2.wingspan() > 5.0 && fm2.wingspan() < 20.0, "f_16c wingspan should be reasonable");
        assert!(fm2.wing_area() > 10.0 && fm2.wing_area() < 100.0, "f_16c wing_area should be reasonable");
        assert!(fm2.vne() > 500.0 && fm2.vne() < 2500.0, "f_16c VNE should be reasonable");
        assert!(fm2.empty_weight() > 5000.0 && fm2.empty_weight() < 20000.0, "f_16c empty_weight should be reasonable");
        assert!(fm2.engine_count() >= 1, "f_16c should have at least 1 engine");

        validate_aerodynamics("yak-3", &fm, AeroValidationParams::default());
        validate_aerodynamics("f_16c_block_50", &fm2, AeroValidationParams::default());
    }

    #[test]
    fn test_su30sm2_not_available() {
        let result = parse_aircraft("su-30sm2", "../resource/data/gamedata/flightmodels");
        assert!(result.is_err(), "su-30sm2 should not exist in test data");
    }

    #[test]
    fn test_additional_aircraft_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";

        let fa_18a = parse_aircraft("fa_18a", data_dir).expect("Failed to parse fa_18a");
        print_flight_model_details("fa_18a", &fa_18a);
        assert_eq!(fa_18a.engine_type_string(), "Jet", "fa_18a should be jet");
        assert!(fa_18a.engine_count() >= 2, "fa_18a should have 2 engines");

        let fw_190f_8 = parse_aircraft("fw_190f_8_hungary", data_dir).expect("Failed to parse fw_190f_8_hungary");
        print_flight_model_details("fw_190f_8_hungary", &fw_190f_8);
        // 上游这份数据里没有 `EngineType0.Main.Type`（`EngineType0.Main.Power` 在）——
        // **缺键 → 默认活塞**（不按 Power/ThrustMax 推断）；Fw 190 F-8 本来就是星型活塞机。
        assert_eq!(fw_190f_8.engine_type_string(), "Piston",
                   "fw_190f_8 缺 EngineType0.Main.Type 时必须默认 Piston（不再显示未知）");
        assert!(fw_190f_8.engine_count() > 0, "fw_190f_8 should parse engine count correctly");

        validate_aerodynamics("fa_18a", &fa_18a, AeroValidationParams::default());
        validate_aerodynamics("fw_190f_8_hungary", &fw_190f_8, AeroValidationParams::default());
    }

    #[test]
    fn test_piston_aircraft_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let test_cases = vec![
            ("yak-3", 2000.0, 0),
            ("fw-190a-8", 3000.0, 0),
            ("spitfire_ix", 2000.0, 0),
            ("p-51d-10", 3000.0, 0),
            ("bf-109g-6", 2000.0, 0),
        ];

        for (name, min_weight, _min_engines) in test_cases {
            let result = parse_aircraft(name, data_dir);
            match result {
                Ok(fm) => {
                    print_flight_model_details(name, &fm);
                    assert!(!fm.name.is_empty(), "{}: name should not be empty", name);
                    assert!(fm.wingspan() > 0.0, "{}: wingspan should be > 0", name);
                    assert!(fm.wing_area() > 0.0, "{}: wing_area should be > 0", name);
                    assert!(fm.empty_weight() > min_weight, "{}: empty_weight too low", name);
                    validate_aerodynamics(name, &fm, AeroValidationParams::default());
                }
                Err(e) => println!("WARNING: {} failed to parse: {}", name, e),
            }
        }
    }

    #[test]
    fn test_rocket_engine_aircraft_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let result = parse_aircraft("me-163b", data_dir);
        match result {
            Ok(fm) => {
                print_flight_model_details("me-163b (Rocket)", &fm);
                assert!(!fm.name.is_empty(), "me-163b: name should not be empty");
                assert_eq!(
                    fm.engine_type_string(),
                    "Rocket",
                    "me-163b: engine type should be Rocket"
                );
                assert!(fm.wingspan() > 0.0, "me-163b: wingspan should be > 0");
                assert!(fm.wing_area() > 0.0, "me-163b: wing_area should be > 0");
                assert!(fm.empty_weight() >= 0.0, "me-163b: empty_weight should be >= 0 (rocket)");
                assert!(fm.engine_count() >= 1, "me-163b: rocket has engine count >= 1");

                let mut rocket_params = AeroValidationParams::default();
                rocket_params.cl_max_no_flaps_min = 0.3;
                rocket_params.cl_max_full_flaps_min = 0.3;
                rocket_params.wingspan_max = 20.0;
                validate_aerodynamics("me-163b", &fm, rocket_params);
            }
            Err(e) => println!("WARNING: me-163b failed to parse: {}", e),
        }
    }

    #[test]
    fn test_variable_sweep_wing_aircraft_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let test_cases = vec![
            ("f_14a_early", 1),
            ("mig_23mld", 1),
            ("su_17m4", 1),
            ("su_24m", 1),
        ];

        for (name, min_engines) in test_cases {
            let result = parse_aircraft(name, data_dir);
            match result {
                Ok(fm) => {
                    print_flight_model_details(name, &fm);
                    assert!(!fm.name.is_empty(), "{}: name should not be empty", name);
                    assert!(fm.wingspan() > 0.0, "{}: wingspan should be > 0", name);
                    assert!(fm.wing_area() > 0.0, "{}: wing_area should be > 0", name);
                    assert!(fm.empty_weight() > 0.0, "{}: empty_weight should be > 0", name);
                    assert!(fm.engine_count() >= min_engines, "{}: engine_count too low", name);

                    assert!(
                        fm.aerodynamics.has_sweep_data(),
                        "{}: has_sweep_data should be true for variable sweep wing aircraft",
                        name
                    );

                    if fm.aerodynamics.has_sweep_data() {
                        let wg = &fm.aerodynamics.wing_geometry;
                        let has_valid_sweep1 = wg.wing_sweep1.as_ref().map_or(false, |s| s.cl_max_no_flaps() > 0.0 || s.vne > 0.0);
                        assert!(
                            has_valid_sweep1 || wg.vne() > 0.0,
                            "{}: wing_sweep1 should have valid data or vne should be > 0",
                            name
                        );
                    }

                    let mut sweep_params = AeroValidationParams::default();
                    sweep_params.expect_sweep = true;
                    validate_aerodynamics(name, &fm, sweep_params);
                }
                Err(e) => println!("WARNING: {} failed to parse: {}", name, e),
            }
        }
    }

    #[test]
    fn test_old_biplane_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let test_cases = vec![
            ("gladiator_mk2", 0.0),
            ("i-153_m62", 0.0),
        ];

        for (name, min_weight) in test_cases {
            let result = parse_aircraft(name, data_dir);
            match result {
                Ok(fm) => {
                    print_flight_model_details(name, &fm);
                    assert!(!fm.name.is_empty(), "{}: name should not be empty", name);
                    assert!(fm.wingspan() > 0.0, "{}: wingspan should be > 0", name);
                    assert!(fm.wing_area() > 0.0, "{}: wing_area should be > 0", name);
                    assert!(fm.empty_weight() >= min_weight, "{}: empty_weight too low", name);

                    let mut biplane_params = AeroValidationParams::default();
                    biplane_params.wing_area_min = 10.0;
                    biplane_params.wing_area_max = 50.0;
                    biplane_params.aspect_ratio_min = 2.0;
                    biplane_params.aspect_ratio_max = 10.0;
                    validate_aerodynamics(name, &fm, biplane_params);
                }
                Err(e) => println!("WARNING: {} failed to parse: {}", name, e),
            }
        }
    }

    #[test]
    fn test_helicopter_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let test_cases = vec![
            ("mi_8tb", 2),
            ("ah_1g", 1),
            ("ka_50", 1),
            ("mi_24v", 2),
        ];

        for (name, min_engines) in test_cases {
            let result = parse_aircraft(name, data_dir);
            match result {
                Ok(fm) => {
                    print_flight_model_details(name, &fm);
                    assert!(!fm.name.is_empty(), "{}: name should not be empty", name);
                    assert!(fm.wingspan() > 0.0, "{}: wingspan should be > 0", name);
                    assert!(fm.wing_area() > 0.0, "{}: wing_area should be > 0", name);
                    assert!(fm.empty_weight() > 0.0, "{}: empty_weight should be > 0", name);
                    assert!(fm.engine_count() >= min_engines, "{}: engine_count too low", name);

                    let mut heli_params = AeroValidationParams::default();
                    heli_params.cl_max_no_flaps_min = 0.2;
                    heli_params.cl_max_full_flaps_min = 0.2;
                    heli_params.wingspan_min = 1.0;
                    heli_params.wingspan_max = 50.0;
                    heli_params.wing_area_min = 1.0;
                    heli_params.wing_area_max = 400.0;
                    heli_params.aspect_ratio_min = 0.1;
                    heli_params.aspect_ratio_max = 100.0;
                    heli_params.oswalds_efficiency_min = 0.01;
                    heli_params.oswalds_efficiency_max = 1.5;
                    validate_aerodynamics(name, &fm, heli_params);
                }
                Err(e) => println!("WARNING: {} failed to parse: {}", name, e),
            }
        }
    }

    #[test]
    fn test_multi_engine_piston_aircraft_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let test_cases = vec![
            ("do_335a_1", 7000.0),
            ("b-29", 20000.0),
            ("b-17g", 15000.0),
        ];

        for (name, min_weight) in test_cases {
            let result = parse_aircraft(name, data_dir);
            match result {
                Ok(fm) => {
                    print_flight_model_details(name, &fm);
                    assert!(!fm.name.is_empty(), "{}: name should not be empty", name);
                    assert!(fm.wingspan() > 0.0, "{}: wingspan should be > 0", name);
                    assert!(fm.wing_area() > 0.0, "{}: wing_area should be > 0", name);
                    assert!(fm.empty_weight() >= min_weight, "{}: empty_weight too low", name);
                    validate_aerodynamics(name, &fm, AeroValidationParams::default());
                }
                Err(e) => println!("WARNING: {} failed to parse: {}", name, e),
            }
        }
    }

    #[test]
    fn test_multi_engine_jet_aircraft_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let result = parse_aircraft("b_52h", data_dir);
        match result {
            Ok(fm) => {
                print_flight_model_details("b_52h", &fm);
                assert!(!fm.name.is_empty(), "b_52h: name should not be empty");
                assert!(fm.wingspan() > 0.0, "b_52h: wingspan should be > 0");
                assert!(fm.wing_area() > 0.0, "b_52h: wing_area should be > 0");
                assert!(fm.empty_weight() > 0.0, "b_52h: empty_weight should be > 0");
                assert!(fm.engine_count() >= 4, "b_52h: should have at least 4 engines");

                let mut bomber_params = AeroValidationParams::default();
                bomber_params.wing_area_min = 100.0;
                bomber_params.wing_area_max = 400.0;
                bomber_params.wingspan_max = 70.0;
                validate_aerodynamics("b_52h", &fm, bomber_params);
            }
            Err(e) => println!("WARNING: b_52h failed to parse: {}", e),
        }
    }

    #[test]
    fn test_asymmetric_multi_engine_aircraft_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let result = parse_aircraft("yak_141", data_dir);
        match result {
            Ok(fm) => {
                print_flight_model_details("yak_141", &fm);
                assert!(!fm.name.is_empty(), "yak_141: name should not be empty");
                assert!(fm.wingspan() > 0.0, "yak_141: wingspan should be > 0");
                assert!(fm.wing_area() > 0.0, "yak_141: wing_area should be > 0");
                assert!(fm.empty_weight() > 0.0, "yak_141: empty_weight should be > 0");
                assert!(fm.engine_count() >= 1, "yak_141: engine_count too low");
                validate_aerodynamics("yak_141", &fm, AeroValidationParams::default());
            }
            Err(e) => println!("WARNING: yak_141 failed to parse: {}", e),
        }
    }

    #[test]
    fn test_vtol_stol_aircraft_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let result = parse_aircraft("harrier_gr7", data_dir);
        match result {
            Ok(fm) => {
                print_flight_model_details("harrier_gr7", &fm);
                assert!(!fm.name.is_empty(), "harrier_gr7: name should not be empty");
                assert!(fm.wingspan() > 0.0, "harrier_gr7: wingspan should be > 0");
                assert!(fm.wing_area() > 0.0, "harrier_gr7: wing_area should be > 0");
                assert!(fm.empty_weight() > 0.0, "harrier_gr7: empty_weight should be > 0");
                assert!(fm.engine_count() >= 1, "harrier_gr7: engine_count too low");
                validate_aerodynamics("harrier_gr7", &fm, AeroValidationParams::default());
            }
            Err(e) => println!("WARNING: harrier_gr7 failed to parse: {}", e),
        }
    }

    #[test]
    fn test_turboprop_bomber_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let result = parse_aircraft("nt_tu_95m", data_dir);
        match result {
            Ok(fm) => {
                print_flight_model_details("nt_tu_95m", &fm);
                assert!(!fm.name.is_empty(), "nt_tu_95m: name should not be empty");
                assert!(fm.wingspan() > 0.0, "nt_tu_95m: wingspan should be > 0");
                assert!(fm.wing_area() > 0.0, "nt_tu_95m: wing_area should be > 0");
                assert!(fm.empty_weight() > 30000.0, "nt_tu_95m: empty_weight too low");
                assert!(fm.engine_count() >= 4, "nt_tu_95m: should have 4 engines");
                assert_eq!(fm.engine_type_string(), "Turboprop", "nt_tu_95m should be turboprop");

                let mut turboprop_params = AeroValidationParams::default();
                turboprop_params.wingspan_max = 70.0;
                turboprop_params.wing_area_min = 200.0;
                turboprop_params.wing_area_max = 500.0;
                turboprop_params.aspect_ratio_max = 15.0;
                validate_aerodynamics("nt_tu_95m", &fm, turboprop_params);
            }
            Err(e) => println!("WARNING: nt_tu_95m failed to parse: {}", e),
        }
    }

    #[test]
    fn test_carrier_fighter_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let test_cases = vec![
            ("f-4j", 12000.0),
            ("f-4e", 12000.0),
            ("f-4c", 12000.0),
        ];

        for (name, min_weight) in test_cases {
            let result = parse_aircraft(name, data_dir);
            match result {
                Ok(fm) => {
                    print_flight_model_details(name, &fm);
                    assert!(!fm.name.is_empty(), "{}: name should not be empty", name);
                    assert!(fm.wingspan() > 0.0, "{}: wingspan should be > 0", name);
                    assert!(fm.wing_area() > 0.0, "{}: wing_area should be > 0", name);
                    assert!(fm.empty_weight() > min_weight, "{}: empty_weight too low", name);
                    assert!(fm.engine_count() >= 2, "{}: should have 2 engines", name);
                    assert_eq!(fm.engine_type_string(), "Jet", "{} should be jet engine", name);
                    validate_aerodynamics(name, &fm, AeroValidationParams::default());
                }
                Err(e) => println!("WARNING: {} failed to parse: {}", name, e),
            }
        }
    }

    #[test]
    fn test_attack_aircraft_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let test_cases = vec![
            ("a-10a_early", 9000.0),
            ("a-10a_late", 9000.0),
            ("a-10c", 9000.0),
        ];

        for (name, min_weight) in test_cases {
            let result = parse_aircraft(name, data_dir);
            match result {
                Ok(fm) => {
                    print_flight_model_details(name, &fm);
                    assert!(!fm.name.is_empty(), "{}: name should not be empty", name);
                    assert!(fm.wingspan() > 0.0, "{}: wingspan should be > 0", name);
                    assert!(fm.wing_area() > 0.0, "{}: wing_area should be > 0", name);
                    assert!(fm.empty_weight() > min_weight, "{}: empty_weight too low", name);
                    assert!(fm.engine_count() >= 1, "{}: should have at least 1 engine", name);
                    assert_eq!(fm.engine_type_string(), "Jet", "{} should be jet engine", name);

                    let mut attack_params = AeroValidationParams::default();
                    attack_params.wingspan_min = 8.0;
                    attack_params.wingspan_max = 20.0;
                    attack_params.wing_area_min = 30.0;
                    attack_params.wing_area_max = 100.0;
                    validate_aerodynamics(name, &fm, attack_params);
                }
                Err(e) => println!("WARNING: {} failed to parse: {}", name, e),
            }
        }
    }

    #[test]
    fn test_f_4e_aero_surfaces() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        for name in &["f-4e", "f-4c"] {
            let result = parse_aircraft(name, data_dir);
            match result {
                Ok(fm) => {
                    print_flight_model_details(name, &fm);
                    // Basic checks
                    assert_eq!(fm.engine_type_string(), "Jet", "{} should be jet", name);
                    assert!(fm.engine_count() >= 2, "{} should have >= 2 engines", name);
                    assert!(fm.wingspan() > 10.0, "{}: wingspan > 10m", name);
                    assert!(fm.wing_area() > 40.0, "{}: wing_area > 40m²", name);
                    assert!(fm.vne() > 1000.0, "{}: VNE > 1000 km/h", name);

                    // Wing NoFlaps polar must be parsed (not default zeros)
                    if let Some(ref wing) = fm.wing {
                        assert!(wing.polar.cd_min > 0.005,
                            "{} wing NoFlaps CdMin should be > 0.005, got {:.4}", name, wing.polar.cd_min);
                        assert!((wing.polar.cl0 - 0.03).abs() < 0.02 ||
                                (wing.polar.cl0 - 0.12).abs() < 0.02,  // f-4c may differ
                            "{} wing NoFlaps Cl0 unexpected: {:.4}", name, wing.polar.cl0);
                        assert!(wing.polar.alpha_crit_high > 20.0,
                            "{} wing NoFlaps alpha_crit_high > 20", name);

                        // Wing FullFlaps polar must be parsed
                        if let Some(ref ff) = wing.flaps_polar_1 {
                            assert!(ff.cd_min > 0.01,
                                "{} wing FullFlaps CdMin should be > 0.01, got {:.4}", name, ff.cd_min);
                            assert!(ff.cl_crit_high > 1.0,
                                "{} wing FullFlaps ClCritHigh > 1.0", name);
                        } else {
                            panic!("{} wing FullFlaps polar not parsed", name);
                        }
                    } else {
                        panic!("{} wing not parsed", name);
                    }

                    // Fuselage must have CdMin parsed (if area is defined)
                    if let Some(ref fuselage) = fm.fuselage {
                        if fuselage.area > 0.0 {
                            assert!(fuselage.polar.cd_min > 0.005,
                                "{} fuselage CdMin should be > 0.005, got {:.4}", name, fuselage.polar.cd_min);
                        }
                    }

                    // HorStab must have CdMin and Cl0 parsed
                    if let Some(ref stab) = fm.hor_stab {
                        assert!(stab.polar.cd_min > 0.005,
                            "{} hor_stab CdMin should be > 0.005, got {:.4}", name, stab.polar.cd_min);
                        assert!(stab.polar.cl0 < 0.0,
                            "{} hor_stab Cl0 should be negative, got {:.4}", name, stab.polar.cl0);
                    }

                    // Validate aero params within sane ranges
                    validate_aerodynamics(name, &fm, AeroValidationParams::default());
                }
                Err(e) => println!("WARNING: {} failed to parse: {}", name, e),
        }
        }
    }

    #[test]
    fn test_all_aircraft_all_readable_fields() {
        let data_dir = "../resource/data/gamedata/flightmodels";

        let aircraft_list = vec![
            "f_111f", "f_16c_block_50", "mig_23mld", "yak-3",
            "fa_18a", "fw_190f_8_hungary", "wyvern_s4", "f_15e"
        ];

        for name in aircraft_list {
            println!("\n{}", "=".repeat(60));
            println!("Testing all readable fields for: {}", name);
            println!("{}", "=".repeat(60));

            let fm = parse_aircraft(name, data_dir);
            match fm {
                Ok(fm) => {
                    print_flight_model_details(name, &fm);

                    let vne = fm.vne();
                    assert!(vne > 0.0, "{}: VNE should be > 0", name);
                    println!("  [OK] vne: {} km/h", vne);

                    let vne_mach = fm.vne_mach();
                    if vne_mach > 0.0 {
                        println!("  [OK] vne_mach: {}", vne_mach);
                    } else {
                        println!("  [WARN] vne_mach is 0, expected > 0.5");
                    }

                    assert!(fm.empty_weight() > 0.0, "{}: empty_weight should be > 0", name);
                    println!("  [OK] empty_weight: {} kg", fm.empty_weight());

                    assert!(fm.wingspan() > 0.0, "{}: wingspan should be > 0", name);
                    println!("  [OK] wingspan: {} m", fm.wingspan());

                    assert!(fm.wing_area() > 0.0, "{}: wing_area should be > 0", name);
                    println!("  [OK] wing_area: {} m²", fm.wing_area());

                    println!("\n--- Aerodynamic Parameters ---");
                    assert!(fm.cl_max_no_flaps() > 0.5, "{}: cl_max_no_flaps {} should be > 0.5", name, fm.cl_max_no_flaps());
                    println!("  [OK] cl_max_no_flaps: {} (> 0.5)", fm.cl_max_no_flaps());

                    assert!(fm.cl_max_full_flaps() > 0.5, "{}: cl_max_full_flaps {} should be > 0.5", name, fm.cl_max_full_flaps());
                    println!("  [OK] cl_max_full_flaps: {} (> 0.5)", fm.cl_max_full_flaps());

                    let cd_min = fm.cd_min();
                    assert!(cd_min >= 0.005 && cd_min <= 0.05, "{}: cd_min {} should be between 0.005 and 0.05", name, cd_min);
                    println!("  [OK] cd_min: {} (0.005 ~ 0.05)", cd_min);

                    let oswalds = fm.oswalds_efficiency();
                    assert!(oswalds >= 0.5 && oswalds <= 1.0, "{}: oswalds_efficiency {} should be between 0.5 and 1.0", name, oswalds);
                    println!("  [OK] oswalds_efficiency: {} (0.5 ~ 1.0)", oswalds);

                    let ar = fm.aspect_ratio();
                    if ar >= 2.5 && ar <= 20.0 {
                        println!("  [OK] aspect_ratio: {} (2.5 ~ 20.0)", ar);
                    } else {
                        println!("  [WARN] aspect_ratio {} is outside typical range (2.5 ~ 20.0)", ar);
                    }

                    println!("\n--- Strength Parameters ---");
                    let crit_vec = fm.crit_overload_vec();
                    if !crit_vec.is_empty() {
                        println!("  [OK] crit_overload_vec length: {}", crit_vec.len());
                        println!("    crit_overload values: {:?}", crit_vec);
                    } else {
                        println!("  [WARN] crit_overload_vec is empty (no Strength.CritOverload found in data)");
                    }

                    let load_range = fm.limit_load_factor_range(fm.empty_weight());
                    let range_nonzero = load_range.0 != 0.0 || load_range.1 != 0.0;
                    if range_nonzero {
                        println!("  [OK] limit_load_factor_range at {}kg: ({}, {})",
                            fm.empty_weight(), load_range.0, load_range.1);
                    } else {
                        println!("  [WARN] limit_load_factor_range is (0, 0) - may indicate missing CritOverload data");
                    }

                    println!("\n--- Other Readable Fields ---");
                    let gear_warning = fm.gear_warning_speed();
                    assert!(gear_warning > 0.0, "{}: gear_warning_speed should be > 0, got {}", name, gear_warning);
                    println!("  [OK] gear_warning_speed: {} km/h", gear_warning);

                    let flap_pairs = fm.get_flap_destruction_speeds();
                    println!("  [OK] flap_destruction_speeds: {} entries", flap_pairs.len());
                    if !flap_pairs.is_empty() && flap_pairs.len() <= 5 {
                        for (i, (flap_pct, speed)) in flap_pairs.iter().enumerate() {
                            println!("    Flap {}: {:.0}% -> {} km/h", i, flap_pct * 100.0, speed);
                        }
                    }

                    let engine_count = fm.engine_count();
                    assert!(engine_count > 0, "{}: engine_count should be > 0", name);
                    println!("  [OK] engine_count: {}", engine_count);

                    let max_fuel = fm.max_fuel();
                    assert!(max_fuel >= 0.0, "{}: max_fuel should be >= 0", name);
                    println!("  [OK] max_fuel: {} kg", max_fuel);

                    let thrust_max = fm.thrust_max();
                    let engine_power = fm.engine_power();
                    if fm.is_jet {
                        if thrust_max > 0.0 {
                            println!("  [OK] thrust_max: {} kgf", thrust_max);
                        }
                    } else {
                        if engine_power > 0.0 {
                            println!("  [OK] engine_power: {} hp", engine_power);
                        } else {
                            println!("  [WARN] engine_power is 0 (may be valid for some aircraft)");
                        }
                    }

                    if fm.has_nitro() {
                        let nitro_cons = fm.nitro_consumption();
                        println!("  [OK] has_nitro: true (consumption: {} L/s)", nitro_cons);
                    }

                    if fm.has_wep() {
                        let wep_thrust = fm.wep_thrust_max();
                        println!("  [OK] has_wep: true (wep_thrust_max: {} kgf)", wep_thrust);
                    }

                    println!("\n--- Additional Fields ---");
                    let fuselage_cd = fm.fuselage_cd_min();
                    if fuselage_cd > 0.0 {
                        println!("  [OK] fuselage_cd_min: {}", fuselage_cd);
                    } else {
                        println!("  [WARN] fuselage_cd_min is 0 (may be expected)");
                    }

                    let radiator_cd = fm.radiator_cd();
                    if radiator_cd > 0.0 {
                        println!("  [OK] radiator_cd: {}", radiator_cd);
                    }

                    let oil_radiator_cd = fm.oil_radiator_cd();
                    if oil_radiator_cd > 0.0 {
                        println!("  [OK] oil_radiator_cd: {}", oil_radiator_cd);
                    }

                    let airbrake_cd = fm.airbrake_cd();
                    if airbrake_cd > 0.0 {
                        println!("  [OK] airbrake_cd: {}", airbrake_cd);
                    }

                    let control_speeds = fm.aerodynamics.control_surface_effective_speeds();
                    println!("  control_surface_effective_speeds: elevator={}, aileron={}, rudder={}",
                        control_speeds[0], control_speeds[1], control_speeds[2]);

                    if fm.has_sweep_data() {
                        println!("  [OK] has_sweep_data: true");
                        let (cl0, aoa0) = fm.sweep_cl_max_no_flaps(0.0);
                        let (cl50, aoa50) = fm.sweep_cl_max_no_flaps(0.5);
                        let (cl100, aoa100) = fm.sweep_cl_max_no_flaps(1.0);
                        println!("    sweep 0%: cl={:.2} aoa={:.1}°", cl0, aoa0);
                        println!("    sweep 50%: cl={:.2} aoa={:.1}°", cl50, aoa50);
                        println!("    sweep 100%: cl={:.2} aoa={:.1}°", cl100, aoa100);
                    }

                    let drag_area = fm.drag_area();
                    assert!(drag_area > 0.0, "{}: drag_area should be > 0", name);
                    println!("  [OK] drag_area: {} m²", drag_area);

                    println!("\n{}", "-".repeat(60));
                    println!("All fields for {} verified successfully!", name);
                    println!("{}", "-".repeat(60));

                }
                Err(e) => panic!("{}: failed to parse: {}", name, e),
            }
        }
    }

    #[test]
    fn test_specific_aircraft_detailed_validation() {
        let data_dir = "../resource/data/gamedata/flightmodels";

        let f111f = parse_aircraft("f_111f", data_dir).expect("f_111f parse failed");
        assert!(f111f.aerodynamics.has_sweep_data(), "f_111f should have sweep data");
        assert!(f111f.vne() > 700.0, "f_111f VNE should be > 700 km/h");
        assert!(f111f.vne_mach() > 0.8, "f_111f VNE Mach should be > 0.8");

        let wg = &f111f.aerodynamics.wing_geometry;
        assert!(wg.wing_sweep0.vne > 0.0, "f_111f sweep0 vne should be > 0");
        if let Some(ref sweep1) = wg.wing_sweep1 {
            assert!(sweep1.vne > 0.0 || sweep1.vne_mach > 0.0,
                "f_111f sweep1 should have vne or vne_mach");
        }
        if let Some(ref sweep2) = wg.wing_sweep2 {
            assert!(sweep2.vne > 0.0 || sweep2.vne_mach > 0.0,
                "f_111f sweep2 should have vne or vne_mach");
        }

        let f111f_crit = f111f.crit_overload_vec();
        if !f111f_crit.is_empty() {
            println!("  [OK] f_111f CritOverload: {:?}", f111f_crit);
        }

        let mig23 = parse_aircraft("mig_23mld", data_dir).expect("mig_23mld parse failed");
        assert!(mig23.aerodynamics.has_sweep_data(), "mig_23mld should have sweep data");
        assert!(mig23.vne() > 700.0, "mig_23mld VNE should be > 700 km/h");
        assert!(mig23.engine_count() >= 1, "mig_23mld should have at least 1 engine");

        let mig23_wg = &mig23.aerodynamics.wing_geometry;
        assert!(mig23_wg.wing_sweep0.cl_max_no_flaps() > 0.5,
            "mig_23mld sweep0 cl_max should be > 0.5");
        if let Some(ref sweep1) = mig23_wg.wing_sweep1 {
            assert!(sweep1.cl_max_no_flaps() > 0.0 || sweep1.vne > 0.0,
                "mig_23mld sweep1 should have cl_max or vne");
        }

        let yak3 = parse_aircraft("yak-3", data_dir).expect("yak-3 parse failed");
        assert_eq!(yak3.engine_type_string(), "Piston", "yak-3 should be Piston engine");
        assert!(yak3.engine_power() > 1200.0 && yak3.engine_power() < 1400.0,
            "yak-3 engine_power should be ~1290 hp");
        assert!(yak3.wingspan() > 8.0 && yak3.wingspan() < 12.0,
            "yak-3 wingspan should be 8-12m");
        assert!(yak3.wing_area() > 10.0 && yak3.wing_area() < 20.0,
            "yak-3 wing_area should be 10-20 m²");

        let fa18a = parse_aircraft("fa_18a", data_dir).expect("fa_18a parse failed");
        assert_eq!(fa18a.engine_type_string(), "Jet", "fa_18a should be Jet engine");
        assert!(fa18a.engine_count() >= 2, "fa_18a should have 2 engines");

        let flap_speeds = fa18a.get_flap_destruction_speeds();
        assert!(!flap_speeds.is_empty(), "fa_18a should have flap_destruction_speeds");
        println!("  [OK] fa_18a flap_destruction_speeds: {:?}", flap_speeds);

        let fw190 = parse_aircraft("fw_190f_8_hungary", data_dir).expect("fw_190f_8_hungary parse failed");
        assert!(fw190.empty_weight() > 3000.0, "fw_190f_8 empty_weight should be > 3000 kg");
        assert!(fw190.wingspan() > 8.0 && fw190.wingspan() < 15.0,
            "fw_190f_8 wingspan should be reasonable");

        let fw190_weapons = fw190.loadout.total_weapons_weight;
        if fw190_weapons > 0.0 {
            println!("  [OK] fw_190f_8 has weapons weight: {:.1} kg", fw190_weapons);
        }

        let wyvern = parse_aircraft("wyvern_s4", data_dir).expect("wyvern_s4 parse failed");
        assert_eq!(wyvern.engine_type_string(), "Turboprop", "wyvern_s4 should be Turboprop engine");
        assert!(wyvern.engine_count() >= 1, "wyvern_s4 should have at least 1 engine");

        let wyvern_thrust = wyvern.thrust_max();
        if wyvern_thrust > 0.0 {
            println!("  [OK] wyvern_s4 thrust_max: {:.0} kgf", wyvern_thrust);
        }

        println!("\n=== Specific Aircraft Detailed Validation Complete ===");
    }

    #[test]
    fn test_tornado_gr4_variable_sweep_vne() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let fm = parse_aircraft("tornado_gr4", data_dir).expect("Failed to parse tornado_gr4");
        print_flight_model_details("tornado_gr4", &fm);

        assert!(
            fm.aerodynamics.has_sweep_data(),
            "tornado_gr4 should have variable sweep wing data"
        );

        let wg = &fm.aerodynamics.wing_geometry;

        // The Tornado FM defines four sweep positions (0, 0.05, 0.5, 1.0), unlike the
        // three positions used by most variable-sweep aircraft. All four must be read.
        let sweep3 = wg
            .wing_sweep3
            .as_ref()
            .expect("tornado_gr4 must parse WingPlaneSweep3");
        let sweeps = [
            (&wg.wing_sweep0, 0.0, 972.0, 0.85),
            (wg.wing_sweep1.as_ref().expect("sweep1"), 0.05, 972.0, 0.85),
            (wg.wing_sweep2.as_ref().expect("sweep2"), 0.5, 1230.0, 1.68),
            (sweep3, 1.0, 1555.0, 1.89),
        ];
        for (i, (wing, fraction, vne, mne)) in sweeps.iter().enumerate() {
            assert!(
                (wing.sweep_percent - fraction).abs() < 0.01,
                "sweep{} fraction should be {:.2}, got {:.3}",
                i, fraction, wing.sweep_percent
            );
            assert!(
                (wing.vne - vne).abs() < 1.0,
                "sweep{} VNE should be {:.0}, got {:.0}",
                i, vne, wing.vne
            );
            assert!(
                (wing.vne_mach - mne).abs() < 0.02,
                "sweep{} MNE should be {:.2}, got {:.2}",
                i, mne, wing.vne_mach
            );
        }

        // Interpolation across the real sweep fractions.
        assert!((wg.get_vne(0.0) - 972.0).abs() < 1.0, "get_vne(0.0)");
        assert!((wg.get_vne(0.5) - 1230.0).abs() < 1.0, "get_vne(0.5)");
        assert!((wg.get_vne(1.0) - 1555.0).abs() < 1.0, "get_vne(1.0)");
        let mid = wg.get_vne(0.75);
        assert!(mid > 1230.0 && mid < 1555.0, "get_vne(0.75) should interpolate, got {:.0}", mid);

        assert!((wg.get_vne_mach(1.0) - 1.89).abs() < 0.02, "get_vne_mach(1.0)");

        // Full-sweep warning line must reflect WingPlaneSweep3, not WingPlaneSweep2.
        // Regression: previously the 4th position was ignored, giving ~1168 km/h.
        let warning = fm.aerodynamics.get_ias_warning_line(1.0);
        assert!(
            (warning - 1555.0 * 0.95).abs() < 1.0,
            "full-sweep IAS warning should be {:.0}, got {:.0}",
            1555.0 * 0.95, warning
        );
        assert!(
            warning > 1400.0,
            "full-sweep IAS warning should exceed 1400 km/h, got {:.0}",
            warning
        );
        let mach_warning = fm.aerodynamics.get_mach_warning_line(1.0);
        assert!(
            (mach_warning - 1.89 * 0.95).abs() < 0.02,
            "full-sweep Mach warning should be {:.3}, got {:.3}",
            1.89 * 0.95, mach_warning
        );

        // Three-position variable-sweep aircraft must keep their old mapping (0/0.5/1.0).
        let f111f = parse_aircraft("f_111f", data_dir).expect("Failed to parse f_111f");
        let f111f_wg = &f111f.aerodynamics.wing_geometry;
        assert!(
            f111f_wg.wing_sweep3.is_none(),
            "f_111f should only define three sweep positions"
        );
        assert!(
            (f111f_wg.wing_sweep1.as_ref().unwrap().sweep_percent - 0.5).abs() < 0.01,
            "f_111f sweep1 fraction should stay 0.5"
        );
        assert!(
            (f111f_wg.get_vne(1.0) - f111f_wg.wing_sweep2.as_ref().unwrap().vne).abs() < 1.0,
            "f_111f get_vne(1.0) should equal sweep2 VNE"
        );
    }

    #[test]
    fn test_f15e_mach_warning_line() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let fm = parse_aircraft("f_15e", data_dir).expect("Failed to parse f_15e");
        print_flight_model_details("f_15e", &fm);

        let vne = fm.vne();
        assert!(
            (vne - 1629.0).abs() < 1.0,
            "f_15e VNE should be 1629 from Strength.VNE, got {:.1}",
            vne
        );

        let mach_warning = fm.vne_mach() * 0.95;
        assert!(
            mach_warning > 2.0,
            "f_15e Mach warning line should be > 2.0 (from MNE 2.55), got {:.3}",
            mach_warning
        );

        let mne = fm.vne_mach();
        assert!(
            (mne - 2.55).abs() < 0.01,
            "f_15e MNE should be 2.55, got {:.3}",
            mne
        );

        let warning_line = fm.aerodynamics.get_mach_warning_line(0.0);
        assert!(
            warning_line > 2.0,
            "f_15e aerodynamics Mach warning line should be > 2.0, got {:.3}",
            warning_line
        );
    }

    #[test]
    fn test_f15e_flap_destruction_speeds() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let fm = parse_aircraft("f_15e", data_dir).expect("Failed to parse f_15e");

        let flap_pairs = fm.get_flap_destruction_speeds();
        assert!(
            flap_pairs.len() >= 2,
            "f_15e should have at least 2 flap speed pairs, got {}",
            flap_pairs.len()
        );

        let p0 = flap_pairs.iter().find(|(pct, _)| (*pct - 0.2).abs() < 0.01);
        assert!(
            p0.is_some(),
            "f_15e should have flap pair at 0.2 (20%)"
        );
        assert!(
            (p0.unwrap().1 - 1018.0).abs() < 1.0,
            "f_15e flap at 20% should have speed 1018 km/h, got {:.0}",
            p0.unwrap().1
        );

        let p1 = flap_pairs.iter().find(|(pct, _)| (*pct - 1.0).abs() < 0.01);
        assert!(
            p1.is_some(),
            "f_15e should have flap pair at 1.0 (100%)"
        );
        assert!(
            (p1.unwrap().1 - 481.0).abs() < 1.0,
            "f_15e flap at 100% should have speed 481 km/h, got {:.0}",
            p1.unwrap().1
        );
    }

    #[test]
    fn test_f111f_flap_destruction_speeds() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let fm = parse_aircraft("f_111f", data_dir).expect("Failed to parse f_111f");

        let flap_pairs = fm.get_flap_destruction_speeds();
        assert!(
            flap_pairs.len() >= 3,
            "f_111f should have at least 3 flap speed pairs, got {}",
            flap_pairs.len()
        );

        let p0 = flap_pairs.iter().find(|(pct, _)| (*pct - 0.2).abs() < 0.01);
        assert!(
            p0.is_some(),
            "f_111f should have flap pair at 0.2 (20%)"
        );
        assert!(
            (p0.unwrap().1 - 800.0).abs() < 1.0,
            "f_111f flap at 20% should have speed 800 km/h, got {:.0}",
            p0.unwrap().1
        );

        let p1 = flap_pairs.iter().find(|(pct, _)| (*pct - 0.33).abs() < 0.01);
        assert!(
            p1.is_some(),
            "f_111f should have flap pair at 0.33 (33%)"
        );
        assert!(
            (p1.unwrap().1 - 611.0).abs() < 1.0,
            "f_111f flap at 33% should have speed 611 km/h, got {:.0}",
            p1.unwrap().1
        );

        let p2 = flap_pairs.iter().find(|(pct, _)| (*pct - 1.0).abs() < 0.01);
        assert!(
            p2.is_some(),
            "f_111f should have flap pair at 1.0 (100%)"
        );
        assert!(
            (p2.unwrap().1 - 555.0).abs() < 1.0,
            "f_111f flap at 100% should have speed 555 km/h, got {:.0}",
            p2.unwrap().1
        );
    }

    #[test]
    fn test_mig_23mld_flap_destruction_speeds() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let fm = parse_aircraft("mig_23mld", data_dir).expect("Failed to parse mig_23mld");

        let flap_pairs = fm.get_flap_destruction_speeds();
        assert!(
            flap_pairs.len() >= 3,
            "mig_23mld should have at least 3 flap speed pairs, got {}",
            flap_pairs.len()
        );

        let p0 = flap_pairs.iter().find(|(pct, _)| (*pct - 0.2).abs() < 0.01);
        assert!(
            p0.is_some(),
            "mig_23mld should have flap pair at 0.2 (20%)"
        );
        assert!(
            (p0.unwrap().1 - 700.0).abs() < 1.0,
            "mig_23mld flap at 20% should have speed 700 km/h, got {:.0}",
            p0.unwrap().1
        );

        let p1 = flap_pairs.iter().find(|(pct, _)| (*pct - 0.33).abs() < 0.01);
        assert!(
            p1.is_some(),
            "mig_23mld should have flap pair at 0.33 (33%)"
        );
        assert!(
            (p1.unwrap().1 - 570.0).abs() < 1.0,
            "mig_23mld flap at 33% should have speed 570 km/h, got {:.0}",
            p1.unwrap().1
        );

        let p2 = flap_pairs.iter().find(|(pct, _)| (*pct - 1.0).abs() < 0.01);
        assert!(
            p2.is_some(),
            "mig_23mld should have flap pair at 1.0 (100%)"
        );
        assert!(
            (p2.unwrap().1 - 473.0).abs() < 1.0,
            "mig_23mld flap at 100% should have speed 473 km/h, got {:.0}",
            p2.unwrap().1
        );
    }

    #[test]
    fn test_all_find_map_fields_correctly_parsed() {
        let data_dir = "../resource/data/gamedata/flightmodels";

        let f_15e = parse_aircraft("f_15e", data_dir).expect("Failed to parse f_15e");
        let f_16c = parse_aircraft("f_16c_block_50", data_dir).expect("Failed to parse f_16c_block_50");

        assert_eq!(
            f_15e.vne(), 1629.0,
            "f_15e VNE should be 1629 from Strength.VNE (find_map found it, not default 1500)"
        );
        assert!(
            (f_15e.vne_mach() - 2.55).abs() < 0.01,
            "f_15e MNE should be 2.55 from Strength.MNE (find_map found it, not default 1.0)"
        );

        assert_eq!(
            f_16c.vne(), 1555.0,
            "f_16c VNE should be 1555 from Strength.VNE (find_map found it, not default 1500)"
        );
        assert!(
            (f_16c.vne_mach() - 2.2).abs() < 0.01,
            "f_16c MNE should be 2.2 from Strength.MNE (find_map found it, not default 1.0)"
        );

        let f_15e_crit_vec = f_15e.crit_overload_vec();
        assert!(
            f_15e_crit_vec.len() >= 2,
            "f_15e CritOverload should be parsed as vector (2 values expected)"
        );
        assert!(
            (f_15e_crit_vec[0] - (-400000.0)).abs() < 1.0,
            "f_15e CritOverload negative should be -400000, got {}",
            f_15e_crit_vec[0]
        );
        assert!(
            (f_15e_crit_vec[1] - 1190000.0).abs() < 1.0,
            "f_15e CritOverload positive should be 1190000, got {}",
            f_15e_crit_vec[1]
        );

        let f_16c_crit_vec = f_16c.crit_overload_vec();
        assert!(
            f_16c_crit_vec.len() >= 2,
            "f_16c CritOverload should be parsed as vector (2 values expected)"
        );
        assert!(
            (f_16c_crit_vec[0] - (-400000.0)).abs() < 1.0,
            "f_16c CritOverload negative should be -400000, got {}",
            f_16c_crit_vec[0]
        );
        assert!(
            (f_16c_crit_vec[1] - 770000.0).abs() < 1.0,
            "f_16c CritOverload positive should be 770000, got {}",
            f_16c_crit_vec[1]
        );

        assert_eq!(
            f_15e.gear_warning_speed(), 700.0,
            "f_15e GearDestructionIndSpeed should be 700 (find_map found it, not default 450)"
        );

        let f_15e_fuselage_cd = f_15e.fuselage_cd_min();
        assert!(
            (f_15e_fuselage_cd - 0.008).abs() < 0.0001,
            "f_15e fuselage CdMin should be 0.008 from FuselagePlane.Polar.CdMin (find_map found it, not default 0.0)"
        );

let f_16c_fuselage_cd = f_16c.fuselage_cd_min();
        assert!(
            (f_16c_fuselage_cd - 0.0068).abs() < 0.0001,
            "f_16c fuselage CdMin should be 0.0068 from FuselagePlane.Polar.CdMin (find_map found it, not default 0.0)"
        );

        let f_15e_load_range = f_15e.limit_load_factor_range(20000.0);
        assert!(
            f_15e_load_range.0 != 0.0 || f_15e_load_range.1 != 0.0,
            "f_15e limit_load_factor_range should return non-zero values (derived from CritOverload vector)"
        );

        let f_16c_load_range = f_16c.limit_load_factor_range(10000.0);
        assert!(
            f_16c_load_range.0 != 0.0 || f_16c_load_range.1 != 0.0,
            "f_16c limit_load_factor_range should return non-zero values (derived from CritOverload vector)"
        );

        let f_15e_wing = &f_15e.aerodynamics.wing_geometry.wing_sweep0;
        assert!(
            (f_15e_wing.no_flaps_polar.alpha_crit_high - 30.0).abs() < 0.1,
            "f_15e NoFlaps alphaCritHigh should be 30.0 (from FlapsPolar0), got {:.1}",
            f_15e_wing.no_flaps_polar.alpha_crit_high
        );
        assert!(
            (f_15e_wing.no_flaps_polar.cl_crit_high - 1.18).abs() < 0.01,
            "f_15e NoFlaps ClCritHigh should be 1.18 (from FlapsPolar0), got {:.2}",
            f_15e_wing.no_flaps_polar.cl_crit_high
        );
        assert!(
            f_15e_wing.full_flaps_polar.is_some(),
            "f_15e full_flaps_polar should be Some"
        );
        if let Some(ref full_flaps) = f_15e_wing.full_flaps_polar {
            assert!(
                (full_flaps.alpha_crit_high - 29.0).abs() < 0.1,
                "f_15e FullFlaps alphaCritHigh should be 29.0 (from FlapsPolar1), got {:.1}",
                full_flaps.alpha_crit_high
            );
            assert!(
                (full_flaps.cl_crit_high - 1.4).abs() < 0.01,
                "f_15e FullFlaps ClCritHigh should be 1.4 (from FlapsPolar1), got {:.2}",
                full_flaps.cl_crit_high
            );
        }
    }

    #[test]
    fn test_additional_aircraft_batch() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        struct AcCase {
            name: &'static str,
            engine_type: &'static str,
            min_empty_weight: f64,
        }
        // 活塞那几台（`tu-2_early` / `i-16_type24` / `f3f-2`）上游缺 `EngineType0.Main.Type`
        // → **默认活塞**；喷气那几台有 Type 键，必须仍然是 Jet（防一刀切）。
        let test_cases = vec![
            AcCase { name: "mig-19s", engine_type: "Jet", min_empty_weight: 4000.0 },
            AcCase { name: "j_7e", engine_type: "Jet", min_empty_weight: 4000.0 },
            AcCase { name: "q_5_early", engine_type: "Jet", min_empty_weight: 5000.0 },
            AcCase { name: "f-86f-30_china", engine_type: "Jet", min_empty_weight: 4000.0 },
            AcCase { name: "tu-2_early", engine_type: "Piston", min_empty_weight: 6000.0 },
            AcCase { name: "i-16_type24", engine_type: "Piston", min_empty_weight: 1000.0 },
            AcCase { name: "f3f-2", engine_type: "Piston", min_empty_weight: 1000.0 },
        ];

        for tc in test_cases {
            let result = parse_aircraft(tc.name, data_dir);
            match result {
                Ok(fm) => {
                    print_flight_model_details(tc.name, &fm);
                    assert!(!fm.name.is_empty(), "{}: name should not be empty", tc.name);
                    assert_eq!(
                        fm.engine_type_string(),
                        tc.engine_type,
                        "{}: engine type mismatch",
                        tc.name
                    );
                    assert!(
                        fm.wingspan() > 0.0,
                        "{}: wingspan should be > 0",
                        tc.name
                    );
                    assert!(
                        fm.wing_area() > 0.0,
                        "{}: wing_area should be > 0",
                        tc.name
                    );
                    assert!(
                        fm.empty_weight() > tc.min_empty_weight,
                        "{}: empty_weight {} should be > {}",
                        tc.name,
                        fm.empty_weight(),
                        tc.min_empty_weight
                    );
                    assert!(
                        fm.engine_count() >= 1,
                        "{}: engine_count should be >= 1",
                        tc.name
                    );
                    assert!(
                        fm.vne() > 200.0,
                        "{}: VNE {} should be > 200 km/h",
                        tc.name,
                        fm.vne()
                    );

                    if let Some(ref wing) = fm.wing {
                        if let Some(ref fp0) = wing.flaps_polar_0 {
                            assert!(
                                fp0.cd_min > 0.0,
                                "{}: wing flaps_polar_0 cd_min should be > 0, got {:.4}",
                                tc.name,
                                fp0.cd_min
                            );
                        }
                    }

                    validate_aerodynamics(tc.name, &fm, AeroValidationParams::default());
                }
                Err(e) => {
                    panic!("{}: failed to parse: {}", tc.name, e);
                }
            }
        }
    }

    #[test]
    fn test_me_163b_0_rocket_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let result = parse_aircraft("me-163b-0", data_dir);
        match result {
            Ok(fm) => {
                print_flight_model_details("me-163b-0 (Rocket)", &fm);

                assert_eq!(
                    fm.engine_type_string(),
                    "Rocket",
                    "me-163b-0: engine type should be Rocket"
                );
                assert!(
                    fm.engine_count() >= 1,
                    "me-163b-0: engine_count should be >= 1, got {}",
                    fm.engine_count()
                );
                assert!(
                    fm.wingspan() > 0.0,
                    "me-163b-0: wingspan should be > 0, got {}",
                    fm.wingspan()
                );
                assert!(
                    fm.wing_area() > 0.0,
                    "me-163b-0: wing_area should be > 0, got {}",
                    fm.wing_area()
                );
                assert!(
                    fm.empty_weight() > 0.0,
                    "me-163b-0: empty_weight should be > 0, got {}",
                    fm.empty_weight()
                );
                assert!(
                    fm.vne() > 0.0,
                    "me-163b-0: VNE should be > 0, got {}",
                    fm.vne()
                );

                let mut rocket_params = AeroValidationParams::default();
                rocket_params.cl_max_no_flaps_min = 0.3;
                rocket_params.cl_max_full_flaps_min = 0.3;
                rocket_params.wingspan_max = 20.0;
                validate_aerodynamics("me-163b-0", &fm, rocket_params);
            }
            Err(e) => {
                panic!("me-163b-0: failed to parse: {}", e);
            }
        }
    }

    #[test]
    fn test_f_80_jet_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        for name in &["f-80", "f-80a"] {
            let result = parse_aircraft(name, data_dir);
            match result {
                Ok(fm) => {
                    print_flight_model_details(name, &fm);
                    assert_eq!(fm.engine_type_string(), "Jet", "{} should be Jet", name);
                    assert!(fm.engine_count() >= 1, "{}: engine count >= 1", name);
                    assert!(fm.wingspan() > 10.0, "{}: wingspan > 10m", name);
                    assert!(fm.wing_area() > 20.0, "{}: wing_area > 20m²", name);
                    assert!(fm.empty_weight() > 3000.0, "{}: empty_weight > 3000kg", name);
                    assert!(fm.vne() > 800.0, "{}: VNE > 800 km/h", name);
                    assert!(fm.thrust_max() > 1000.0,
                        "{}: thrust_max should be > 1000 kgf, got {:.0}", name, fm.thrust_max());
                    validate_aerodynamics(name, &fm, AeroValidationParams::default());
                }
                Err(e) => {
                    panic!("{}: failed to parse: {}", name, e);
                }
            }
        }
    }

    #[test]
    fn test_hornet_mk1_wing_polar_parsing() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let fm = parse_aircraft("hornet_mk1", data_dir).expect("Failed to parse hornet_mk1");
        print_flight_model_details("hornet_mk1", &fm);

        // Verify wing area/span from AeroSurface
        if let Some(ref wing) = fm.wing {
            assert!(wing.span > 10.0, "hornet_mk1 wing span should be > 10m, got {:.1}", wing.span);
            assert!(wing.area > 20.0, "hornet_mk1 wing area should be > 20m², got {:.2}", wing.area);

            // NoFlaps polar must be parsed (not default zeros)
            assert!(wing.polar.cd_min > 0.005,
                "hornet_mk1 wing NoFlaps CdMin should be > 0.005, got {:.4}", wing.polar.cd_min);
            assert!(wing.polar.cl_crit_high > 0.5,
                "hornet_mk1 wing NoFlaps ClCritHigh should be > 0.5, got {:.2}", wing.polar.cl_crit_high);
            assert!(wing.polar.cl_slope() > 0.05,
                "hornet_mk1 wing NoFlaps cl_slope should be > 0.05, got {:.4}", wing.polar.cl_slope());
            assert!(wing.polar.alpha_crit_high > 10.0,
                "hornet_mk1 wing NoFlaps alpha_crit_high should be > 10°, got {:.1}", wing.polar.alpha_crit_high);

            // FullFlaps polar must be parsed
            if let Some(ref ff) = wing.flaps_polar_1 {
                assert!(ff.cd_min > 0.01,
                    "hornet_mk1 wing FullFlaps CdMin should be > 0.01, got {:.4}", ff.cd_min);
                assert!(ff.cl_crit_high > 1.0,
                    "hornet_mk1 wing FullFlaps ClCritHigh should be > 1.0, got {:.2}", ff.cl_crit_high);
                assert!(ff.cl0 > 0.3,
                    "hornet_mk1 wing FullFlaps Cl0 should be > 0.3, got {:.2}", ff.cl0);
            } else {
                panic!("hornet_mk1 wing FullFlaps polar not parsed");
            }
        } else {
            panic!("hornet_mk1 wing not parsed");
        }

        // Verify wing geometry polar (Aerodynamics path)
        let wg = &fm.aerodynamics.wing_geometry.wing_sweep0;
        assert!(wg.cl_max_no_flaps() > 0.5,
            "hornet_mk1 WingGeometry cl_max_no_flaps should be > 0.5, got {:.2}", wg.cl_max_no_flaps());
        assert!(wg.cd_min() > 0.005,
            "hornet_mk1 WingGeometry cd_min should be > 0.005, got {:.4}", wg.cd_min());
        assert!(wg.no_flaps_polar.cl_slope() > 0.05,
            "hornet_mk1 WingGeometry no_flaps_polar.cl_slope() should be > 0.05, got {:.4}",
            wg.no_flaps_polar.cl_slope());

        // Verify FullFlaps polar in WingGeometry
        if let Some(ref ff) = wg.full_flaps_polar {
            assert!(ff.cl_crit_high > 1.0,
                "hornet_mk1 WingGeometry FullFlaps ClCritHigh should be > 1.0, got {:.2}", ff.cl_crit_high);
            assert!(ff.cl_slope() > 0.05,
                "hornet_mk1 WingGeometry FullFlaps cl_slope should be > 0.05, got {:.4}", ff.cl_slope());
        } else {
            panic!("hornet_mk1 WingGeometry full_flaps_polar not parsed");
        }

        // Verify engine type
        assert_eq!(fm.engine_type_string(), "Piston", "hornet_mk1 should be Piston engine");
        assert!(fm.engine_count() >= 2, "hornet_mk1 should have 2 engines");
        assert!(fm.wingspan() > 10.0, "hornet_mk1 wingspan should be > 10m");
        assert!(fm.wing_area() > 20.0, "hornet_mk1 wing_area should be > 20m²");
        assert!(fm.vne() > 500.0, "hornet_mk1 VNE should be > 500 km/h");

        validate_aerodynamics("hornet_mk1", &fm, AeroValidationParams::default());
    }

    #[test]
    fn test_f_4m_fgr2_wing_aoa_data() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let fm = parse_aircraft("f-4m_fgr2", data_dir).expect("Failed to parse f-4m_fgr2");
        print_flight_model_details("f-4m_fgr2", &fm);

        assert!(
            fm.aoa_cl_max_no_flaps() > 20.0,
            "f-4m_fgr2 aoa_cl_max_no_flaps should be > 20°, got {:.1}°",
            fm.aoa_cl_max_no_flaps()
        );
        assert!(
            fm.aoa_cl_max_full_flaps() > 25.0,
            "f-4m_fgr2 aoa_cl_max_full_flaps should be > 25°, got {:.1}°",
            fm.aoa_cl_max_full_flaps()
        );
        assert!(
            (fm.cl_max_no_flaps() - 1.05).abs() < 0.1,
            "f-4m_fgr2 cl_max_no_flaps should be ~1.05, got {:.2}",
            fm.cl_max_no_flaps()
        );
        assert!(
            (fm.cl_max_full_flaps() - 1.25).abs() < 0.1,
            "f-4m_fgr2 cl_max_full_flaps should be ~1.25, got {:.2}",
            fm.cl_max_full_flaps()
        );

        let (cl_no_flaps, aoa_no_flaps) = fm.sweep_cl_max_no_flaps(0.0);
        assert!(
            (aoa_no_flaps - 27.0).abs() < 1.0,
            "f-4m_fgr2 aoa_cl_max_no_flaps via sweep should be 27°, got {:.1}°",
            aoa_no_flaps
        );
        assert!(
            (cl_no_flaps - 1.05).abs() < 0.1,
            "f-4m_fgr2 cl_max_no_flaps via sweep should be ~1.05, got {:.2}",
            cl_no_flaps
        );
    }

    #[test]
    fn test_mirage_3s_c70_switzerland_rpm() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let fm = parse_aircraft("mirage_3s_c70_switzerland", data_dir)
            .expect("Failed to parse mirage_3s_c70_switzerland");

        assert_eq!(fm.engine_count(), 4, "should have 4 engines (1 jet + 3 rocket boosters)");
        assert!(
            fm.rpm_min > 0.0,
            "rpm_min should be > 0, got {}",
            fm.rpm_min
        );
        assert!(
            fm.rpm_max > 0.0 && fm.rpm_max < f64::MAX,
            "rpm_max should be a finite positive value, got {}",
            fm.rpm_max
        );
        assert!(
            fm.rpm_max_allowed > 0.0 && fm.rpm_max_allowed < f64::MAX,
            "rpm_max_allowed should be a finite positive value, got {}",
            fm.rpm_max_allowed
        );
    }

    #[test]
    fn test_mirage_3s_c70_switzerland_fuel_consumption() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let fm = parse_aircraft("mirage_3s_c70_switzerland", data_dir)
            .expect("Failed to parse mirage_3s_c70_switzerland");
        print_flight_model_details("mirage_3s_c70_switzerland", &fm);

        assert_eq!(fm.engine_type_string(), "Jet", "should be Jet engine");
        assert_eq!(fm.engine_count(), 4, "should have 4 engines (1 jet + 3 rocket boosters)");

        let sfc_mil = fm.fuel_consumption_coefficient(false);
        let sfc_wep = fm.fuel_consumption_coefficient(true);

        assert!(
            sfc_mil < 2.0,
            "Military SFC should be < 2.0 (kg/h)/kgf, got {:.3}",
            sfc_mil
        );
        assert!(
            sfc_mil > 0.5,
            "Military SFC should be > 0.5 (kg/h)/kgf, got {:.3}",
            sfc_mil
        );
        assert!(
            sfc_wep < 2.0,
            "WEP SFC should be < 2.0 (kg/h)/kgf, got {:.3}",
            sfc_wep
        );
        assert!(
            sfc_wep > sfc_mil,
            "WEP SFC ({:.3}) should be > Military SFC ({:.3})",
            sfc_wep,
            sfc_mil
        );
    }

    #[test]
    fn test_f15a_thrust_and_sfc() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let fm = parse_aircraft("f_15a", data_dir).expect("Failed to parse f_15a");
        print_flight_model_details("f_15a", &fm);

        let test_cases = [
            (100.0, 11800.0, 0.740),
            (102.0, 19500.0, 1.030),
            (105.0, 21200.0, 1.377),
            (108.0, 22900.0, 1.800),
            (110.0, 23800.0, 2.015),
        ];

        for &(throttle, measured_thrust, measured_sfc) in &test_cases {
            let thrust = thrusts_at(&fm.engines(), 800.0, 0.0, throttle);
            let fuel_rate = fuel_rate_at(&fm.engines(), 800.0, 0.0, throttle);
            let sfc = fuel_rate * 3600.0 / thrust;

            let thrust_error = (thrust - measured_thrust).abs() / measured_thrust;
            let sfc_error = (sfc - measured_sfc).abs() / measured_sfc;

            assert!(
                thrust_error <= 0.05,
                "F-15A @ throttle={}: thrust {:.0} vs expected {:.0} (error {:.1}%)",
                throttle, thrust, measured_thrust, thrust_error * 100.0
            );

            assert!(
                sfc_error <= 0.05,
                "F-15A @ throttle={}: SFC {:.3} vs expected {:.3} (error {:.1}%)",
                throttle, sfc, measured_sfc, sfc_error * 100.0
            );
        }
    }

    #[test]
    fn test_f15a_countermeasure_weight() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let fm = parse_aircraft("f_15a", data_dir).expect("Failed to parse f_15a");
        let cm = fm.countermeasure_weight();
        println!("  countermeasure_weight: {:.6} kg", cm);
        // 口径（2026-09 定）：干扰弹 + 箔条都算。
        // 21.6 = 240 flare × 0.09 kg；49.2 = 492 chaff × 0.1 kg；合计 70.8 kg。
        // 期望值原为 21.6（只算 flare），与实现不符而被列入 scripts/test.sh 的白名单；
        // 现在按"flare+chaff"对齐，白名单随之清空。
        assert!((cm - 70.8).abs() < 0.01,
            "F-15A countermeasure weight should be ~70.8 kg (flares+chaff), got {:.6}", cm);
    }

    /// 结构过载口径回归（逆向结论：n = sign(F)·(2|F|/(m·g) − 1)，F 单位牛顿）
    /// 期望值取自游戏数据空重档实测（社区 "Zero 15G" 等），
    /// a6m2_zero = 15.35 对应社区实测 "Zero 15G"，f_14a_early 验证变后掠键表修复。
    #[test]
    fn test_crit_overload_g_limit_formula() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let cases: &[(&str, f64, f64)] = &[
            ("a6m2_zero", -8.24, 15.35),
            ("yak-3", -9.81, 13.41),
            ("mig-21_bis", -8.13, 15.09),
            ("f_16c_block_50", -8.03, 16.38),
            ("f_14a_early", -5.67, 13.44),
        ];

        for (name, want_neg, want_pos) in cases {
            let fm = parse_aircraft(name, data_dir)
                .unwrap_or_else(|_| panic!("Failed to parse {}", name));
            let (neg, pos) = fm.limit_load_factor_range(fm.empty_weight());
            assert_eq!(
                fm.allowed_load_factor(fm.empty_weight()),
                (neg, pos),
                "{} allowed_load_factor 接口应与底层换算一致",
                name
            );
            assert!(
                (neg - want_neg).abs() < 0.15 && (pos - want_pos).abs() < 0.15,
                "{} G limit @empty mismatch: got ({:.2}, {:.2}), want ({:.2}, {:.2})",
                name, neg, pos, want_neg, want_pos
            );
            // 满油更重 → 两个方向的幅值都更小
            let (neg_f, pos_f) = fm.limit_load_factor_range(fm.flight_weight(fm.max_fuel()));
            assert!(
                neg_f > neg && pos_f < pos,
                "{} full-fuel tier should shrink: empty ({:.2},{:.2}) full ({:.2},{:.2})",
                name, neg, pos, neg_f, pos_f
            );
        }
    }

    #[test]
    fn test_su11_load_factors() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let fm = parse_aircraft("su-9", data_dir).expect("Failed to parse su-9");
        print_flight_model_details("su-9", &fm);

        assert_eq!(
            fm.engine_type_string(),
            "Jet",
            "su-9 should be Jet engine"
        );
        assert!(
            fm.empty_weight() > 4000.0 && fm.empty_weight() < 5000.0,
            "su-9 empty_weight should be around 4060, got {:.0}",
            fm.empty_weight()
        );
        assert!(
            (fm.empty_weight() - 4060.0).abs() < 100.0,
            "su-9 empty_weight should be ~4060, got {:.0}",
            fm.empty_weight()
        );
        assert!(
            (fm.max_fuel() - 1846.0).abs() < 10.0,
            "su-9 max_fuel should be ~1846, got {:.0}",
            fm.max_fuel()
        );

        let load_range = fm.limit_load_factor_range(fm.empty_weight());
        assert!(
            load_range.0 < -1.0,
            "su-9 limit_load_factor_range neg should be < -1.0, got {:.2}",
            load_range.0
        );
        assert!(
            load_range.1 > 1.0,
            "su-9 limit_load_factor_range pos should be > 1.0, got {:.2}",
            load_range.1
        );

        let wep_thrust = fm.wep_thrust_max();
        assert!(
            (wep_thrust - 1747.0).abs() < 50.0,
            "su-9 wep_thrust_max should be ~1747 kgf, got {:.0}",
            wep_thrust
        );
    }

    #[test]
    fn test_su11_fuel_rate() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let fm = parse_aircraft("su-9", data_dir).expect("Failed to parse su-9");
        print_flight_model_details("su-9", &fm);

        let fuel_rate = fm.get_fuel_rate_at(800.0, 0.0, true);
        assert!(
            fuel_rate > 0.0,
            "su-9 get_fuel_rate_at(800, 0, true) should be > 0, got {:.6}",
            fuel_rate
        );

        let endurance_secs = fm.max_fuel() / fuel_rate;
        assert!(
            endurance_secs > 600.0,
            "su-9 endurance at 800km/h should be > 10 min, got {:.0}s ({:.1} min)",
            endurance_secs,
            endurance_secs / 60.0
        );
        assert!(
            endurance_secs < 36000.0,
            "su-9 endurance at 800km/h should be < 10 h, got {:.0}s ({:.1} h)",
            endurance_secs,
            endurance_secs / 3600.0
        );
    }

    #[test]
    fn test_piston_power_curves() {
        let data_dir = "../resource/data/gamedata/flightmodels";
        let aircraft_names = vec!["p-51d-10", "bf-109g-10", "p-38l"];

        for name in &aircraft_names {
            let fm = parse_aircraft(name, data_dir)
                .unwrap_or_else(|e| panic!("Failed to parse {}: {}", name, e));
            print_flight_model_details(name, &fm);
            print_compressor_stages(&fm.engines());
            print_engine_modes(&fm.engines());

            for engine in fm.engines() {
                assert_eq!(
                    engine.engine_type(),
                    EngineType::Piston,
                    "{}: engine type should be Piston",
                    name
                );
                println!(
                    "  Engine: power={:.0} hp, modes={}",
                    engine.power(),
                    engine.engine_modes().len()
                );
            }

            assert!(
                fm.engine_count() > 0,
                "{}: should have at least one engine",
                name
            );

            println!("\n--- Power Table (Military, throttle=100) ---");
            print_power_table(&fm.engines(), 100.0);

            let mil_power = powers_at(&fm.engines(), 0.0, 0.0, 100.0);
            assert!(
                mil_power > 0.0,
                "{}: military power at sea level static should be positive, got {:.0}",
                name, mil_power
            );
            println!(
                "{}: Military power at sea level static: {:.0} hp",
                name, mil_power
            );

            if has_any_wep(fm.engines()) {
                println!("\n--- Power Table (WEP, throttle=110) ---");
                print_power_table(&fm.engines(), 110.0);

                let wep_power = powers_at(&fm.engines(), 0.0, 0.0, 110.0);
                assert!(
                    wep_power > mil_power,
                    "{}: WEP power ({:.0}) should be greater than military power ({:.0})",
                    name, wep_power, mil_power
                );
                println!(
                    "{}: WEP power at sea level static: {:.0} hp",
                    name, wep_power
                );
            } else {
                println!("{}: No WEP available", name);
            }
        }
    }

}